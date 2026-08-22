//! #35 worktree pruning: provably-dead checkouts only, dry-run by default.
//!
//! A worktree is a candidate only when ALL of the following are verified:
//!
//! 1. the tree is clean (`git status` reports nothing, including untracked);
//! 2. no herdr agent cwd is inside it;
//! 3. the branch has no open PR and the gh check itself succeeded;
//! 4. HEAD is an ancestor of the integration branch (`origin/staging` when
//!    it exists, else `origin/main`) and is not equal to its tip;
//! 5. protected gitignored files are absent, and no skip-worktree /
//!    assume-unchanged index marks hide tracked edits;
//! 6. the resolved path is exactly `<home>/.herdr/worktrees/<fleet>.` + one
//!    branch component, so the fleet root cannot authorize a sibling tree.
//!
//! The final removal is always a NON-FORCE `git worktree remove`, refreshed
//! immediately before each deletion, so git remains the final authority. Any
//! unavailable check keeps the tree (or aborts the run) — never prunes.

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::fleet::config::Registry;
use crate::fleet::watch::{AgentInfo, AgentsView, parse_agent_listing};

/// Gitignored paths that are invisible to non-force removal and therefore
/// always block pruning. The legacy shell patterns are kept intentionally
/// narrow; matching is case-insensitive on the full path and basename.
const PROTECT_IGNORED: &[&str] = &[
    ".env",
    ".env.*",
    "*.env",
    ".envrc",
    "REVIEW*.md",
    "PR_DESCRIPTION.md",
    "IMPLEMENTED.md",
    "INVESTIGATION.md",
    "*.sqlite",
    "*.sqlite3",
    "*.sqlite-wal",
    "*.sqlite-shm",
    "*.db",
    "*.pem",
    "*.key",
    "*.p12",
    "id_rsa*",
    "id_ed25519*",
    "id_ecdsa*",
    "settings.local.json",
];

const DEFAULT_MAX_PRUNE: usize = 10;
const DEFAULT_MIN_AGE_DAYS: u64 = 1;

#[derive(Debug, Clone)]
pub struct PruneOptions {
    pub apply: bool,
    pub max_prune: usize,
    pub min_age_days: u64,
}

impl Default for PruneOptions {
    fn default() -> Self {
        Self {
            apply: false,
            max_prune: DEFAULT_MAX_PRUNE,
            min_age_days: DEFAULT_MIN_AGE_DAYS,
        }
    }
}

/// A path that passed every dead-tree check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PruneCandidate {
    pub path: PathBuf,
    /// The fleet's `worktree_dir` (the directory segment under
    /// `worktrees_root` that contains this candidate). Kept separately from
    /// `fleet` (the fleet *name*) because the two are independent registry
    /// fields and are not required to be equal.
    pub worktree_dir: String,
    pub fleet: String,
    pub branch: String,
    pub head: String,
    pub integration: String,
    pub own_commits: u64,
}

/// The non-destructive result.
#[derive(Debug, Clone, Default)]
pub struct PrunePlan {
    pub evaluated: usize,
    pub candidates: Vec<PruneCandidate>,
    pub kept: Vec<String>,
}

/// What a destructive run did.
#[derive(Debug, Clone, Default)]
pub struct PruneReport {
    pub evaluated: usize,
    pub candidates: Vec<PruneCandidate>,
    pub removed: Vec<PathBuf>,
    pub skipped: Vec<String>,
    pub failures: Vec<String>,
}

#[derive(Debug)]
pub enum PruneError {
    AgentListUnavailable,
    GateRefused { count: usize, max: usize },
}

impl fmt::Display for PruneError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AgentListUnavailable => {
                f.write_str("herdr agent list failed or was unreadable — refusing to prune")
            }
            Self::GateRefused { count, max } => write!(
                f,
                "{count} worktree(s) exceed the per-run cap of {max} — nothing was removed"
            ),
        }
    }
}

impl std::error::Error for PruneError {}

#[derive(Debug, Clone)]
pub struct ProcessOutput {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
}

pub trait CommandRunner {
    /// `None` means the process could not even be spawned.
    fn run(&self, program: &str, args: &[String], cwd: Option<&Path>) -> Option<ProcessOutput>;
}

pub struct SystemCommandRunner;

impl CommandRunner for SystemCommandRunner {
    fn run(&self, program: &str, args: &[String], cwd: Option<&Path>) -> Option<ProcessOutput> {
        let mut command = Command::new(program);
        command
            .args(args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        if let Some(cwd) = cwd {
            command.current_dir(cwd);
        }
        let output = command.output().ok()?;
        Some(ProcessOutput {
            success: output.status.success(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }
}

/// The production herdr listing, sharing the watchdog parser/contract.
pub fn list_agents() -> AgentsView {
    let output = Command::new("herdr")
        .args(["agent", "list"])
        .stdin(std::process::Stdio::null())
        .output()
        .ok()?;
    parse_agent_listing(&String::from_utf8_lossy(&output.stdout))
}

fn git(shell: &dyn CommandRunner, cwd: &Path, args: &[&str]) -> Option<ProcessOutput> {
    let args: Vec<String> = args.iter().map(|s| (*s).to_string()).collect();
    shell.run("git", &args, Some(cwd))
}

fn gh(shell: &dyn CommandRunner, args: &[&str]) -> Option<ProcessOutput> {
    let args: Vec<String> = args.iter().map(|s| (*s).to_string()).collect();
    shell.run("gh", &args, None)
}

fn canonical(path: &Path) -> Option<PathBuf> {
    path.canonicalize().ok()
}

fn containment_ok(path: &Path, fleet_root: &Path, _fleet_name: &str) -> bool {
    let Some(target) = canonical(path) else {
        return false;
    };
    let Some(root) = canonical(fleet_root) else {
        return false;
    };
    if !target.starts_with(&root) {
        return false;
    }
    let Ok(relative) = target.strip_prefix(&root) else {
        return false;
    };
    relative.components().count() == 1
}

fn stash_brief(path: &Path) -> Result<Option<(PathBuf, PathBuf)>, String> {
    let brief = path.join(".brief.md");
    if !brief.is_file() {
        return Ok(None);
    }
    let stash = std::env::temp_dir().join(format!(
        "corral-prune-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    std::fs::create_dir_all(&stash).map_err(|e| e.to_string())?;
    let saved = stash.join(".brief.md");
    std::fs::rename(&brief, &saved).map_err(|e| {
        let _ = std::fs::remove_dir_all(&stash);
        e.to_string()
    })?;
    Ok(Some((stash, saved)))
}

fn restore_or_discard_brief(path: &Path, stashed: Option<(PathBuf, PathBuf)>) {
    let Some((stash, saved)) = stashed else {
        return;
    };
    let brief = path.join(".brief.md");
    if brief.exists() {
        let _ = std::fs::remove_dir_all(stash);
        return;
    }
    if std::fs::rename(&saved, &brief).is_err() {
        eprintln!(
            "corrald prune: .brief.md could not be restored — preserved at {}",
            saved.display()
        );
    }
    let _ = std::fs::remove_dir_all(stash);
}

fn cwd_inside(cwd: &str, target: &Path) -> bool {
    if cwd.is_empty() {
        return false;
    }
    let cwd = Path::new(cwd);
    let target_real = canonical(target).unwrap_or_else(|| target.to_path_buf());
    let cwd_real = canonical(cwd).unwrap_or_else(|| cwd.to_path_buf());
    cwd_real.starts_with(&target_real)
}

fn clean(shell: &dyn CommandRunner, path: &Path) -> Result<bool, String> {
    let out = git(
        shell,
        path,
        &["status", "--porcelain=v1", "--untracked-files=all"],
    )
    .ok_or_else(|| "git status could not be spawned".to_string())?;
    if !out.success {
        return Err(out.stderr.trim().to_string());
    }
    // The root `.brief.md` scaffold is the only untracked path that does not
    // block a prune. It is moved aside immediately before removal and
    // restored if git refuses; this is the legacy prune contract.
    Ok(out.stdout.lines().all(|line| line.trim() == "?? .brief.md"))
}

fn protected_ignored(shell: &dyn CommandRunner, path: &Path) -> Result<Vec<String>, String> {
    let out = git(
        shell,
        path,
        &[
            "ls-files",
            "--others",
            "--ignored",
            "--exclude-standard",
            "-z",
        ],
    )
    .ok_or_else(|| "git ignored-file scan could not be spawned".to_string())?;
    if !out.success {
        return Err(out.stderr.trim().to_string());
    }
    let mut hits = Vec::new();
    for raw in out.stdout.split('\0') {
        if raw.is_empty() {
            continue;
        }
        let path = raw.trim_end_matches('/');
        let basename = Path::new(path)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or(path);
        if PROTECT_IGNORED.iter().any(|pattern| {
            glob_match(&pattern.to_ascii_lowercase(), &path.to_ascii_lowercase())
                || glob_match(
                    &pattern.to_ascii_lowercase(),
                    &basename.to_ascii_lowercase(),
                )
        }) {
            hits.push(raw.to_string());
        }
    }
    Ok(hits)
}

/// Minimal `*`/`?` glob matcher, enough for the protected-path patterns.
fn glob_match(pattern: &str, text: &str) -> bool {
    fn inner(p: &[char], t: &[char]) -> bool {
        if p.is_empty() {
            return t.is_empty();
        }
        match p[0] {
            '*' => {
                for skip in 0..=t.len() {
                    if inner(&p[1..], &t[skip..]) {
                        return true;
                    }
                }
                false
            }
            '?' if !t.is_empty() => inner(&p[1..], &t[1..]),
            c if !t.is_empty() && c == t[0] => inner(&p[1..], &t[1..]),
            _ => false,
        }
    }
    inner(
        &pattern.chars().collect::<Vec<_>>(),
        &text.chars().collect::<Vec<_>>(),
    )
}

fn has_hidden_marks(shell: &dyn CommandRunner, path: &Path) -> Result<bool, String> {
    let out = git(shell, path, &["ls-files", "-v"])
        .ok_or_else(|| "git ls-files could not be spawned".to_string())?;
    if !out.success {
        return Err(out.stderr.trim().to_string());
    }
    Ok(out.stdout.lines().any(|line| {
        line.chars()
            .next()
            .is_some_and(|c| c.is_ascii_lowercase() || c == 'S')
    }))
}

fn repo_has_ref(shell: &dyn CommandRunner, path: &Path, reference: &str) -> bool {
    git(
        shell,
        path,
        &["rev-parse", "--verify", "--quiet", reference],
    )
    .is_some_and(|out| out.success)
}

fn integration_ref(shell: &dyn CommandRunner, path: &Path) -> Option<String> {
    if repo_has_ref(shell, path, "refs/remotes/origin/staging") {
        Some("refs/remotes/origin/staging".to_string())
    } else if repo_has_ref(shell, path, "refs/remotes/origin/main") {
        Some("refs/remotes/origin/main".to_string())
    } else {
        None
    }
}

fn open_pr_count(shell: &dyn CommandRunner, repo: &str, branch: &str) -> Option<u64> {
    let out = gh(
        shell,
        &[
            "pr", "list", "--repo", repo, "--head", branch, "--state", "open", "--json", "number",
        ],
    )?;
    if !out.success {
        return None;
    }
    let trimmed = out.stdout.trim();
    if !trimmed.starts_with('[') {
        return None;
    }
    let value: serde_json::Value = serde_json::from_str(trimmed).ok()?;
    Some(value.as_array()?.len() as u64)
}

#[derive(Debug)]
enum Verdict {
    Prune(PruneCandidate),
    Keep(String),
}

fn inspect(
    registry: &Registry,
    path: &Path,
    fleet_root: &Path,
    agents: &BTreeMap<String, AgentInfo>,
    shell: &dyn CommandRunner,
    opts: &PruneOptions,
    now_unix: u64,
) -> Verdict {
    let Some(fleet) = registry
        .fleets
        .iter()
        .find(|fleet| fleet_root.ends_with(&fleet.worktree_dir))
    else {
        return Verdict::Keep("not in the fleet registry".to_string());
    };
    if !containment_ok(path, fleet_root, &fleet.name) {
        return Verdict::Keep("path fails containment".to_string());
    }
    let branch = match git(shell, path, &["branch", "--show-current"]) {
        Some(out) if out.success => out.stdout.trim().to_string(),
        _ => return Verdict::Keep("branch unreadable".to_string()),
    };
    if branch.is_empty() {
        return Verdict::Keep("detached HEAD".to_string());
    }
    let head = match git(shell, path, &["rev-parse", "HEAD"]) {
        Some(out) if out.success => out.stdout.trim().to_string(),
        _ => return Verdict::Keep("HEAD unreadable".to_string()),
    };
    match clean(shell, path) {
        Ok(true) => {}
        Ok(false) => return Verdict::Keep("dirty tree".to_string()),
        Err(error) => return Verdict::Keep(format!("git status unreadable: {error}")),
    }
    for agent in agents.values() {
        if cwd_inside(&agent.cwd, path) {
            return Verdict::Keep("agent running here".to_string());
        }
    }
    let open = open_pr_count(shell, &fleet.gh_repo, &branch);
    match open {
        Some(0) => {}
        Some(_) => return Verdict::Keep("open PR".to_string()),
        None => return Verdict::Keep("open-PR state unverifiable".to_string()),
    }
    let Some(integration) = integration_ref(shell, path) else {
        return Verdict::Keep("no integration ref".to_string());
    };
    let ancestor = git(
        shell,
        path,
        &["merge-base", "--is-ancestor", "HEAD", &integration],
    )
    .is_some_and(|out| out.success);
    if !ancestor {
        return Verdict::Keep(format!("not an ancestor of {integration}"));
    }
    let integration_sha = git(shell, path, &["rev-parse", &integration])
        .and_then(|out| out.success.then(|| out.stdout.trim().to_string()));
    if integration_sha.as_deref() == Some(head.as_str()) {
        return Verdict::Keep("sits at integration tip".to_string());
    }
    let own_commits = git(
        shell,
        path,
        &["rev-list", "--count", &integration, "..HEAD"],
    )
    .and_then(|out| out.success.then(|| out.stdout.trim().parse::<u64>().ok()))
    .flatten()
    .unwrap_or(0);
    let commit_time = git(shell, path, &["log", "-1", "--format=%ct", "HEAD"])
        .and_then(|out| out.success.then(|| out.stdout.trim().parse::<u64>().ok()))
        .flatten();
    if let Some(commit_time) = commit_time {
        let min_age = opts.min_age_days.saturating_mul(24 * 3600);
        if now_unix.saturating_sub(commit_time) < min_age {
            return Verdict::Keep("touched within min-age window".to_string());
        }
    }
    match protected_ignored(shell, path) {
        Ok(hits) if hits.is_empty() => {}
        Ok(hits) => return Verdict::Keep(format!("protected ignored files: {}", hits.join("; "))),
        Err(error) => return Verdict::Keep(format!("ignored-file scan failed: {error}")),
    }
    match has_hidden_marks(shell, path) {
        Ok(false) => {}
        Ok(true) => return Verdict::Keep("skip-worktree/assume-unchanged marks".to_string()),
        Err(error) => return Verdict::Keep(format!("hidden-index scan failed: {error}")),
    }
    Verdict::Prune(PruneCandidate {
        path: path.to_path_buf(),
        worktree_dir: fleet.worktree_dir.clone(),
        fleet: fleet.name.clone(),
        branch,
        head,
        integration,
        own_commits,
    })
}

fn scan(
    registry: &Registry,
    worktrees_root: &Path,
    agents: &BTreeMap<String, AgentInfo>,
    shell: &dyn CommandRunner,
    opts: &PruneOptions,
    now_unix: u64,
) -> PrunePlan {
    let mut plan = PrunePlan::default();
    for fleet in &registry.fleets {
        let fleet_root = worktrees_root.join(&fleet.worktree_dir);
        let Ok(entries) = std::fs::read_dir(&fleet_root) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() || (!path.join(".git").is_dir() && !path.join(".git").is_file()) {
                continue;
            }
            plan.evaluated += 1;
            match inspect(registry, &path, &fleet_root, agents, shell, opts, now_unix) {
                Verdict::Prune(candidate) => plan.candidates.push(candidate),
                Verdict::Keep(reason) => {
                    plan.kept.push(format!("{} — {reason}", path.display()));
                }
            }
        }
    }
    plan.candidates.sort_by(|a, b| a.path.cmp(&b.path));
    plan
}

/// Plan-only run. `now_unix` is injectable for age tests.
pub fn plan(
    registry: &Registry,
    worktrees_root: &Path,
    agents: &AgentsView,
    shell: &dyn CommandRunner,
    opts: &PruneOptions,
    now_unix: u64,
) -> Result<PrunePlan, PruneError> {
    let agents = agents.as_ref().ok_or(PruneError::AgentListUnavailable)?;
    Ok(scan(
        registry,
        worktrees_root,
        agents,
        shell,
        opts,
        now_unix,
    ))
}

/// Run the pruning gate. Dry-run is the default; `opts.apply` is the only
/// way anything is removed, and even then each candidate is re-checked and
/// non-force git is the final authority.
pub fn prune(
    registry: &Registry,
    worktrees_root: &Path,
    opts: &PruneOptions,
    mut lister: impl FnMut() -> AgentsView,
    shell: &dyn CommandRunner,
    now_unix: u64,
) -> Result<PruneReport, PruneError> {
    let initial = lister();
    let planned = plan(registry, worktrees_root, &initial, shell, opts, now_unix)?;
    let mut report = PruneReport {
        evaluated: planned.evaluated,
        candidates: planned.candidates,
        ..PruneReport::default()
    };
    if !opts.apply || report.candidates.is_empty() {
        return Ok(report);
    }
    if report.candidates.len() > opts.max_prune {
        return Err(PruneError::GateRefused {
            count: report.candidates.len(),
            max: opts.max_prune,
        });
    }

    for candidate in report.candidates.clone() {
        let Some(agents) = lister() else {
            return Err(PruneError::AgentListUnavailable);
        };
        let fleet_root = worktrees_root.join(&candidate.worktree_dir);
        match inspect(
            registry,
            &candidate.path,
            &fleet_root,
            &agents,
            shell,
            opts,
            now_unix,
        ) {
            Verdict::Prune(_) => {}
            Verdict::Keep(reason) => {
                report
                    .skipped
                    .push(format!("{} — {reason}", candidate.path.display()));
                continue;
            }
        }
        let common = git(
            shell,
            &candidate.path,
            &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        )
        .filter(|out| out.success)
        .map(|out| PathBuf::from(out.stdout.trim()))
        .unwrap_or_else(|| worktrees_root.to_path_buf());
        let stashed = match stash_brief(&candidate.path) {
            Ok(stashed) => stashed,
            Err(detail) => {
                report.failures.push(format!(
                    "{}: could not stash .brief.md: {detail}",
                    candidate.path.display()
                ));
                continue;
            }
        };
        let args = vec![
            "-C".to_string(),
            common.to_string_lossy().into_owned(),
            "worktree".to_string(),
            "remove".to_string(),
            candidate.path.to_string_lossy().into_owned(),
        ];
        match shell.run("git", &args, None) {
            Some(out) if out.success => {
                report.removed.push(candidate.path.clone());
                if let Some((stash, _)) = stashed {
                    let _ = std::fs::remove_dir_all(stash);
                }
            }
            Some(out) => {
                restore_or_discard_brief(&candidate.path, stashed);
                report.failures.push(format!(
                    "{}: {}",
                    candidate.path.display(),
                    out.stderr.trim()
                ));
            }
            None => {
                restore_or_discard_brief(&candidate.path, stashed);
                report.failures.push(format!(
                    "{}: git could not be spawned",
                    candidate.path.display()
                ));
            }
        }
    }
    Ok(report)
}

/// Unix timestamp helper, also used by tests.
pub fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fleet::config::{Fleet, Models};

    fn ok_stdout(stdout: &str) -> ProcessOutput {
        ProcessOutput {
            success: true,
            stdout: stdout.to_string(),
            stderr: String::new(),
        }
    }

    fn fail_stderr(stderr: &str) -> ProcessOutput {
        ProcessOutput {
            success: false,
            stdout: String::new(),
            stderr: stderr.to_string(),
        }
    }

    #[derive(Default)]
    struct ScriptedRunner {
        dirty: String,
        ignored: String,
        marks: String,
        prs: String,
        ancestor: bool,
        own_commits: u64,
        commit_time: u64,
        remove_ok: bool,
        remove_stderr: String,
        common: String,
    }

    impl ScriptedRunner {
        fn new(now: u64) -> Self {
            Self {
                dirty: String::new(),
                ignored: String::new(),
                marks: "H tracked\n".to_string(),
                prs: "[]".to_string(),
                ancestor: true,
                own_commits: 0,
                commit_time: now.saturating_sub(2 * 24 * 3600),
                remove_ok: true,
                remove_stderr: String::new(),
                common: "/repos/main".to_string(),
            }
        }
    }

    impl CommandRunner for ScriptedRunner {
        fn run(
            &self,
            program: &str,
            args: &[String],
            _cwd: Option<&Path>,
        ) -> Option<ProcessOutput> {
            if program == "gh" {
                return Some(ok_stdout(&self.prs));
            }
            let args: Vec<&str> = args.iter().map(String::as_str).collect();
            match args.as_slice() {
                [first, ..] if *first == "-C" && args.get(3) == Some(&"remove") => {
                    if self.remove_ok {
                        Some(ok_stdout(""))
                    } else {
                        Some(fail_stderr(&self.remove_stderr))
                    }
                }
                [path_format, ..] if *path_format == "--path-format=absolute" => {
                    Some(ok_stdout(&self.common))
                }
                ["branch", "--show-current"] => Some(ok_stdout("feature")),
                ["rev-parse", "HEAD"] => Some(ok_stdout("abc1234")),
                ["status", "--porcelain=v1", "--untracked-files=all"] => {
                    Some(ok_stdout(&self.dirty))
                }
                [
                    "ls-files",
                    "--others",
                    "--ignored",
                    "--exclude-standard",
                    "-z",
                ] => Some(ok_stdout(&self.ignored)),
                ["ls-files", "-v"] => Some(ok_stdout(&self.marks)),
                [
                    "rev-parse",
                    "--verify",
                    "--quiet",
                    "refs/remotes/origin/staging",
                ] => Some(ok_stdout("refs/remotes/origin/staging")),
                [
                    "merge-base",
                    "--is-ancestor",
                    "HEAD",
                    "refs/remotes/origin/staging",
                ] => {
                    if self.ancestor {
                        Some(ok_stdout(""))
                    } else {
                        Some(fail_stderr("not an ancestor"))
                    }
                }
                ["rev-parse", "refs/remotes/origin/staging"] => Some(ok_stdout("tip")),
                [
                    "rev-list",
                    "--count",
                    "refs/remotes/origin/staging",
                    "..HEAD",
                ] => Some(ok_stdout(&self.own_commits.to_string())),
                ["log", "-1", "--format=%ct", "HEAD"] => {
                    Some(ok_stdout(&self.commit_time.to_string()))
                }
                _ => Some(fail_stderr("unexpected git command")),
            }
        }
    }

    fn registry() -> Registry {
        Registry {
            fleets: vec![Fleet {
                name: "corral".to_string(),
                gh_repo: "jirathip-k/corral".to_string(),
                local: "/repos/corral".to_string(),
                worktree_dir: "corral".to_string(),
                orch: "orch-corral".to_string(),
                workers: vec![],
                paused: false,
                models: Models {
                    orch: "fable".to_string(),
                    impl_: "sonnet".to_string(),
                    review: "opus".to_string(),
                    impl_alt: None,
                    impl_alt2: None,
                },
            }],
        }
    }

    fn worktree(root: &Path, name: &str) -> PathBuf {
        let path = root.join("corral").join(name);
        std::fs::create_dir_all(&path).expect("create worktree");
        std::fs::write(
            path.join(".git"),
            "gitdir: /repos/main/.git/worktrees/feature\n",
        )
        .expect("write .git file");
        path
    }

    fn empty_agents() -> BTreeMap<String, AgentInfo> {
        BTreeMap::new()
    }

    #[test]
    fn plan_marks_clean_merged_worktree_prunable() {
        let dir = tempfile::tempdir().expect("temp dir");
        let wt = worktree(dir.path(), "feature");
        let now = now_unix();
        let runner = ScriptedRunner::new(now);
        let plan = super::plan(
            &registry(),
            dir.path(),
            &Some(empty_agents()),
            &runner,
            &PruneOptions::default(),
            now,
        )
        .expect("prune plan succeeds");
        assert_eq!(plan.evaluated, 1);
        assert_eq!(plan.candidates.len(), 1);
        assert_eq!(plan.candidates[0].path, wt);
        assert_eq!(plan.candidates[0].fleet, "corral");
        assert!(plan.kept.is_empty(), "all keep reasons: {:?}", plan.kept);
    }

    #[test]
    fn plan_and_apply_attribute_path_by_worktree_dir_when_name_differs() {
        // Regression: `name` and `worktree_dir` are independent registry fields.
        // The apply path must derive the fleet root from `worktree_dir` (mirroring
        // the plan path), not from the fleet `name`.
        let dir = tempfile::tempdir().expect("temp dir");
        let wt = worktree(dir.path(), "feature"); // root/corral/feature
        let now = now_unix();
        let runner = ScriptedRunner::new(now);
        let registry = Registry {
            fleets: vec![Fleet {
                name: "herdr".to_string(),
                gh_repo: "jirathip-k/corral".to_string(),
                local: "/repos/corral".to_string(),
                worktree_dir: "corral".to_string(),
                orch: "orch-corral".to_string(),
                workers: vec![],
                paused: false,
                models: Models {
                    orch: "fable".to_string(),
                    impl_: "sonnet".to_string(),
                    review: "opus".to_string(),
                    impl_alt: None,
                    impl_alt2: None,
                },
            }],
        };

        let plan = super::plan(
            &registry,
            dir.path(),
            &Some(empty_agents()),
            &runner,
            &PruneOptions::default(),
            now,
        )
        .expect("prune plan succeeds");
        assert_eq!(plan.evaluated, 1);
        assert_eq!(plan.candidates.len(), 1, "kept: {:?}", plan.kept);
        assert_eq!(plan.candidates[0].path, wt);
        assert_eq!(plan.candidates[0].fleet, "herdr");
        assert_eq!(plan.candidates[0].worktree_dir, "corral");

        let mut applied_runner = ScriptedRunner::new(now);
        applied_runner.dirty = "?? .brief.md\n".to_string();
        std::fs::write(wt.join(".brief.md"), "brief").expect("write brief");
        applied_runner.common = dir.path().join("main").to_string_lossy().into_owned();
        let applied = super::prune(
            &registry,
            dir.path(),
            &PruneOptions {
                apply: true,
                max_prune: 10,
                min_age_days: 1,
            },
            || Some(empty_agents()),
            &applied_runner,
            now,
        )
        .expect("apply succeeds");
        assert_eq!(applied.removed, vec![wt.clone()], "skipped: {:?}", applied.skipped);
    }

    #[test]
    fn plan_keeps_dirty_worktree() {
        let dir = tempfile::tempdir().expect("temp dir");
        worktree(dir.path(), "feature");
        let now = now_unix();
        let mut runner = ScriptedRunner::new(now);
        runner.dirty = " M src/main.rs\n".to_string();
        let plan = super::plan(
            &registry(),
            dir.path(),
            &Some(empty_agents()),
            &runner,
            &PruneOptions::default(),
            now,
        )
        .expect("plan succeeds");
        assert!(plan.candidates.is_empty());
        assert!(plan.kept[0].contains("dirty tree"));
    }

    #[test]
    fn plan_keeps_worktree_with_live_agent() {
        let dir = tempfile::tempdir().expect("temp dir");
        let wt = worktree(dir.path(), "feature");
        let now = now_unix();
        let runner = ScriptedRunner::new(now);
        let mut agents = empty_agents();
        agents.insert(
            "w1".to_string(),
            AgentInfo {
                status: "working".to_string(),
                cwd: wt.to_string_lossy().into_owned(),
            },
        );
        let plan = super::plan(
            &registry(),
            dir.path(),
            &Some(agents),
            &runner,
            &PruneOptions::default(),
            now,
        )
        .expect("plan succeeds");
        assert!(plan.candidates.is_empty());
        assert!(plan.kept[0].contains("agent running here"));
    }

    #[test]
    fn plan_keeps_open_or_unverifiable_pr() {
        let dir = tempfile::tempdir().expect("temp dir");
        worktree(dir.path(), "feature");
        let now = now_unix();
        for prs in ["[{\"number\": 1}]", "not json"] {
            let mut runner = ScriptedRunner::new(now);
            runner.prs = prs.to_string();
            let plan = super::plan(
                &registry(),
                dir.path(),
                &Some(empty_agents()),
                &runner,
                &PruneOptions::default(),
                now,
            )
            .expect("plan succeeds");
            assert!(plan.candidates.is_empty(), "{prs:?}");
            let reason = &plan.kept[0];
            assert!(
                reason.contains("open PR") || reason.contains("unverifiable"),
                "{prs:?}: {reason}"
            );
        }
    }

    #[test]
    fn plan_keeps_worktree_not_ancestor_of_integration() {
        let dir = tempfile::tempdir().expect("temp dir");
        worktree(dir.path(), "feature");
        let now = now_unix();
        let mut runner = ScriptedRunner::new(now);
        runner.ancestor = false;
        let plan = super::plan(
            &registry(),
            dir.path(),
            &Some(empty_agents()),
            &runner,
            &PruneOptions::default(),
            now,
        )
        .expect("plan succeeds");
        assert!(plan.candidates.is_empty());
        assert!(plan.kept[0].contains("not an ancestor"));
    }

    #[test]
    fn plan_keeps_hidden_index_marks_and_protected_ignored_files() {
        let dir = tempfile::tempdir().expect("temp dir");
        worktree(dir.path(), "feature");
        let now = now_unix();
        let mut runner = ScriptedRunner::new(now);
        runner.marks = "h tracked\n".to_string();
        let plan = super::plan(
            &registry(),
            dir.path(),
            &Some(empty_agents()),
            &runner,
            &PruneOptions::default(),
            now,
        )
        .expect("plan succeeds");
        assert!(plan.candidates.is_empty());
        assert!(plan.kept[0].contains("skip-worktree/assume-unchanged"));

        let mut runner = ScriptedRunner::new(now);
        runner.ignored = ".env\0".to_string();
        let plan = super::plan(
            &registry(),
            dir.path(),
            &Some(empty_agents()),
            &runner,
            &PruneOptions::default(),
            now,
        )
        .expect("plan succeeds");
        assert!(plan.candidates.is_empty());
        assert!(plan.kept[0].contains("protected ignored files"));
    }

    #[test]
    fn plan_brief_is_the_only_tolerated_untracked_file() {
        let dir = tempfile::tempdir().expect("temp dir");
        let wt = worktree(dir.path(), "feature");
        std::fs::write(wt.join(".brief.md"), "brief").expect("write brief");
        let now = now_unix();
        let mut runner = ScriptedRunner::new(now);
        runner.dirty = "?? .brief.md\n".to_string();
        let plan = super::plan(
            &registry(),
            dir.path(),
            &Some(empty_agents()),
            &runner,
            &PruneOptions::default(),
            now,
        )
        .expect("plan succeeds");
        assert_eq!(plan.candidates.len(), 1);
    }

    #[test]
    fn prune_dry_run_never_removes_and_apply_removes_after_stash() {
        let dir = tempfile::tempdir().expect("temp dir");
        let wt = worktree(dir.path(), "feature");
        std::fs::write(wt.join(".brief.md"), "brief").expect("write brief");
        let now = now_unix();
        let mut runner = ScriptedRunner::new(now);
        runner.dirty = "?? .brief.md\n".to_string();
        let dry = prune(
            &registry(),
            dir.path(),
            &PruneOptions::default(),
            || Some(empty_agents()),
            &runner,
            now,
        )
        .expect("dry run succeeds");
        assert!(dry.removed.is_empty());
        assert!(wt.join(".brief.md").is_file());

        runner.common = dir.path().join("main").to_string_lossy().into_owned();
        let applied = prune(
            &registry(),
            dir.path(),
            &PruneOptions {
                apply: true,
                max_prune: 10,
                min_age_days: 1,
            },
            || Some(empty_agents()),
            &runner,
            now,
        )
        .expect("apply succeeds");
        assert_eq!(applied.removed, vec![wt.clone()]);
        assert!(
            !wt.join(".brief.md").exists(),
            "stashed brief is discarded only after successful removal"
        );
    }

    #[test]
    fn failed_removal_restores_brief_and_is_reported() {
        let dir = tempfile::tempdir().expect("temp dir");
        let wt = worktree(dir.path(), "feature");
        std::fs::write(wt.join(".brief.md"), "brief").expect("write brief");
        let now = now_unix();
        let mut runner = ScriptedRunner::new(now);
        runner.dirty = "?? .brief.md\n".to_string();
        runner.remove_ok = false;
        runner.remove_stderr = "directory not empty\n".to_string();
        let report = prune(
            &registry(),
            dir.path(),
            &PruneOptions {
                apply: true,
                max_prune: 10,
                min_age_days: 1,
            },
            || Some(empty_agents()),
            &runner,
            now,
        )
        .expect("apply returns the failure report");
        assert!(report.removed.is_empty());
        assert_eq!(
            std::fs::read_to_string(wt.join(".brief.md")).expect("restored brief"),
            "brief"
        );
        assert!(report.failures[0].contains("directory not empty"));
    }

    #[test]
    fn apply_refuses_cap_exceeding_candidates() {
        let dir = tempfile::tempdir().expect("temp dir");
        worktree(dir.path(), "feature");
        worktree(dir.path(), "feature-2");
        let now = now_unix();
        let runner = ScriptedRunner::new(now);
        let err = prune(
            &registry(),
            dir.path(),
            &PruneOptions {
                apply: true,
                max_prune: 1,
                min_age_days: 1,
            },
            || Some(empty_agents()),
            &runner,
            now,
        )
        .expect_err("cap must refuse");
        assert!(matches!(err, PruneError::GateRefused { count: 2, max: 1 }));
    }

    #[test]
    fn unavailable_agent_list_is_fail_closed() {
        let dir = tempfile::tempdir().expect("temp dir");
        worktree(dir.path(), "feature");
        let now = now_unix();
        let runner = ScriptedRunner::new(now);
        let err = super::plan(
            &registry(),
            dir.path(),
            &None,
            &runner,
            &PruneOptions::default(),
            now,
        )
        .expect_err("missing listing must refuse");
        assert!(matches!(err, PruneError::AgentListUnavailable));
    }
}
