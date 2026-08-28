//! #232 — `read_diff` computation: unified diff + diffstat for one worktree.
//!
//! Everything is computed through libgit2 (`git2`, vendored) — the daemon
//! never shells out to `git` on this path. Contract:
//!
//! - The diff is the worktree vs its HEAD commit (`git diff HEAD`
//!   semantics, including staged AND unstaged tracked changes:
//!   `diff_tree_to_workdir_with_index`). Untracked files are intentionally
//!   NOT part of the diff (they are not in any tree; the board's dirty
//!   marker still surfaces them).
//! - Output is bounded by design: the changed-files list is capped
//!   ([`READ_DIFF_MAX_FILES`]; `files_truncated` says when), and the unified
//!   diff is served as ONE PAGE (`offset`/`lines` window, clamped to
//!   [`READ_DIFF_MAX_LINES`]) with a byte budget ([`READ_DIFF_MAX_BYTES`])
//!   and a per-line hard truncation. The walk never retains more than one
//!   file's patch plus the page itself — a 10k-file, multi-MB worktree diff
//!   cannot blow up the daemon.
//! - Each page advances the window by exactly the requested line budget, so
//!   lazy paging always makes forward progress even when the byte budget
//!   drops individual pathologically long lines.

use std::fmt;
use std::path::Path;

use git2::{Diff, DiffDelta, DiffFlags, Repository};

use crate::drive::{DiffFileStat, DiffStats, READ_DIFF_MAX_BYTES, ReadDiffQuery, ReadDiffResult};

/// Per-line hard truncation: one pathologically long line (minified blob,
/// data dump) is cut here instead of dominating the page's byte budget.
const MAX_LINE_CHARS: usize = 4096;

/// Typed failures for the read_diff computation. All map onto a dispatch
/// refusal (`DriveError::NoWorktree`) at the adapter boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiffError {
    /// The path is not a git repository (or the repo could not be opened).
    Open(String),
    /// The repository has no HEAD commit yet (unborn checkout) — there is no
    /// base to diff against.
    NoHead(String),
    /// libgit2 failure while building or walking the diff.
    Git(String),
}

impl fmt::Display for DiffError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Open(detail) => write!(f, "not a git repository: {detail}"),
            Self::NoHead(detail) => write!(f, "worktree has no HEAD commit: {detail}"),
            Self::Git(detail) => write!(f, "diff failed: {detail}"),
        }
    }
}

impl std::error::Error for DiffError {}

/// One page-window collector. `stream_pos` is the position in the aggregate
/// line stream, `total` the final stream length; the window served is
/// `[query.offset, query.offset + query.lines)` with a byte budget.
struct PageCollector {
    offset: u32,
    budget: u32,
    lines: Vec<String>,
    bytes: usize,
    stream_pos: u32,
}

impl PageCollector {
    fn new(query: &ReadDiffQuery) -> Self {
        Self {
            offset: query.offset,
            budget: query.lines,
            lines: Vec::new(),
            bytes: 0,
            stream_pos: 0,
        }
    }

    /// Push one stream line. The line is truncated to [`MAX_LINE_CHARS`]
    /// before the byte check; lines inside the window that would exceed the
    /// byte budget are dropped (the window still advances past them so the
    /// next page makes progress). One logical unified line per element —
    /// multi-line payloads (patch file headers come as ONE callback record
    /// with embedded newlines) are split at the caller.
    fn push(&mut self, line: String) {
        let pos = self.stream_pos;
        self.stream_pos += 1;
        if pos < self.offset || pos >= self.offset + self.budget {
            // Outside the window: count only (the walk still provides the
            // stream metadata, never the content).
            return;
        }
        // D9: redact the FULL line before the 4096-char truncation, so a
        // secret straddling the cut never leaks a prefix that survives
        // truncation (the adapter boundary keeps its redaction pass too —
        // `redact` is idempotent).
        let line = crate::core::redact::redact(&line).into_owned();
        let index = if line.chars().count() > MAX_LINE_CHARS {
            let mut cut = line.chars().take(MAX_LINE_CHARS - 1).collect::<String>();
            cut.push('…');
            cut
        } else {
            line
        };
        if self.bytes + index.len() > READ_DIFF_MAX_BYTES {
            // Pathological page: drop the line but advance the window.
            return;
        }
        self.bytes += index.len();
        self.lines.push(index);
    }
}

/// Compute the bounded diff view for the worktree at `path`.
///
/// `path` is the herdr-owned worktree root (the caller is responsible for
/// resolving it from snapshot state and verifying ownership).
pub fn read_worktree_diff(path: &Path, query: &ReadDiffQuery) -> Result<ReadDiffResult, DiffError> {
    let repo = Repository::open(path).map_err(|e| DiffError::Open(e.to_string()))?;
    let head = repo.head().map_err(|e| DiffError::NoHead(e.to_string()))?;
    let head_tree = head
        .peel_to_tree()
        .map_err(|e| DiffError::NoHead(e.to_string()))?;

    let head_sha = head
        .target()
        .map(|oid| oid.to_string())
        .and_then(|full| full.get(..7).map(str::to_owned));
    let branch = head
        .shorthand()
        .map(|s| s.to_string())
        .filter(|s| !s.is_empty());

    let mut opts = git2::DiffOptions::new();
    // Standard unified context. `diff_tree_to_workdir_with_index` emulates
    // `git diff HEAD` — staging is blended in, never dropped.
    opts.context_lines(3);
    let diff = repo
        .diff_tree_to_workdir_with_index(Some(&head_tree), Some(&mut opts))
        .map_err(|e| DiffError::Git(e.to_string()))?;

    let stats = diff.stats().map_err(|e| DiffError::Git(e.to_string()))?;
    let stats = DiffStats {
        files: stats.files_changed() as u32,
        adds: stats.insertions() as u32,
        dels: stats.deletions() as u32,
    };

    let mut collector = PageCollector::new(query);
    let mut files: Vec<DiffFileStat> = Vec::new();
    let mut truncated = false;

    let file_count = diff.deltas().len();
    for i in 0..file_count {
        if files.len() >= query.files as usize {
            truncated = true;
            // Do NOT generate the (possibly huge) patches beyond the cap:
            // they are not listed and not paged.
            continue;
        }
        let delta = diff
            .get_delta(i)
            .ok_or_else(|| DiffError::Git("delta disappeared mid-walk".to_string()))?;
        let entry = walk_delta(&diff, i, &delta, &mut collector)?;
        files.push(entry);
    }

    let total = collector.stream_pos;
    let next = collector.offset + collector.budget;
    let next_offset = (next < total).then_some(next);
    Ok(ReadDiffResult {
        repo: None,
        branch,
        head: head_sha,
        stats,
        files,
        files_truncated: truncated,
        offset: query.offset,
        lines: collector.lines,
        total,
        has_more: total > next,
        next_offset,
    })
}

/// Walk one delta: synthesize the unified file header, enrich with hunk
/// content from the patch, count per-file +/- lines, and return the file
/// stat. Binary deltas are summarized (no per-line walk).
fn walk_delta(
    diff: &Diff<'_>,
    index: usize,
    delta: &DiffDelta,
    collector: &mut PageCollector,
) -> Result<DiffFileStat, DiffError> {
    let old_path = delta
        .old_file()
        .path()
        .map(|p| p.to_string_lossy().into_owned());
    let new_path = delta
        .new_file()
        .path()
        .map(|p| p.to_string_lossy().into_owned());
    let path = new_path
        .as_deref()
        .or(old_path.as_deref())
        .unwrap_or("(unknown)")
        .to_string();

    let old_disp = old_path
        .as_deref()
        .map(|p| format!("a/{p}"))
        .unwrap_or_else(|| "/dev/null".to_string());
    let new_disp = new_path
        .as_deref()
        .map(|p| format!("b/{p}"))
        .unwrap_or_else(|| "/dev/null".to_string());
    let header_git = format!("diff --git {old_disp} {new_disp}");

    let mut adds = 0u32;
    let mut dels = 0u32;

    if delta.flags().contains(DiffFlags::BINARY) {
        collector.push(header_git);
        let origin = old_disp.clone();
        collector.push(format!("Binary files {origin} and {new_disp} differ"));
        return Ok(DiffFileStat { path, adds, dels });
    }

    let mut patch = git2::Patch::from_diff(diff, index)
        .map_err(|e| DiffError::Git(e.to_string()))?
        .ok_or_else(|| DiffError::Git("delta has no patch".to_string()))?;
    let (_, a, d) = patch
        .line_stats()
        .map_err(|e| DiffError::Git(e.to_string()))?;
    adds = a as u32;
    dels = d as u32;

    // The patch printer emits the unified file-header record (diff --git /
    // index / --- / +++ plus optional mode lines) as ONE callback with
    // embedded newlines, then one callback per hunk header and content line.
    // Split the header blob so the stream is exactly one unified line per
    // element (the client pages logically, not by git2 record).
    patch
        .print(&mut |_delta, _hunk, line| {
            let mut text = String::from_utf8_lossy(line.content()).into_owned();
            while text.ends_with('\n') || text.ends_with('\r') {
                text.pop();
            }
            if text.contains('\n') {
                // File-header record: the record's origin char (a space) sits
                // on the first piece; the rest carry no origin.
                for (i, piece) in text.split('\n').enumerate() {
                    let piece = if i == 0 {
                        piece.strip_prefix(' ').unwrap_or(piece)
                    } else {
                        piece
                    };
                    collector.push(piece.to_string());
                }
            } else {
                // Content/hunk-header line: the origin prefix is not part of
                // the content, so put it back (standard unified text).
                collector.push(format!("{}{text}", line.origin()));
            }
            true
        })
        .map_err(|e| DiffError::Git(e.to_string()))?;

    Ok(DiffFileStat { path, adds, dels })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    use tempfile::TempDir;

    fn write(path: &Path, content: &str) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("parent dir");
        }
        std::fs::write(path, content).expect("write file");
    }

    fn init_repo(dir: &Path) -> Repository {
        let repo = Repository::init(dir).expect("init repo");
        let mut cfg = repo.config().expect("config");
        cfg.set_str("user.name", "corral-test").expect("name");
        cfg.set_str("user.email", "t@corral.test").expect("email");
        repo
    }

    fn stage_all(repo: &Repository) {
        let mut index = repo.index().expect("index");
        index
            .add_all(["*"].iter(), git2::IndexAddOption::DEFAULT, None)
            .expect("add all");
        index.write().expect("index write");
    }

    fn commit(repo: &Repository, message: &str) {
        stage_all(repo);
        let tree = repo
            .index()
            .expect("index")
            .write_tree()
            .expect("write tree");
        let tree = repo.find_tree(tree).expect("find tree");
        let sig = repo.signature().expect("sig");
        let parent = repo.head().ok().and_then(|h| h.peel_to_commit().ok());
        let parents: Vec<&git2::Commit> = parent.iter().collect();
        repo.commit(Some("HEAD"), &sig, &sig, message, &tree, &parents)
            .expect("commit");
    }

    /// Fixture: one commit with a.txt (3 lines) + gone.txt; then:
    /// - b.txt: staged add (new file)
    /// - a.txt: staged modify (line 2 -> "two!") then unstaged modify again
    /// - gone.txt: deleted (unstaged)
    /// - untracked.txt: NEVER staged (must not appear)
    struct Fixture {
        _dir: TempDir,
        repo_path: PathBuf,
    }

    fn fixture() -> Fixture {
        let dir = tempfile::tempdir().expect("tempdir");
        let repo = init_repo(dir.path());
        write(&dir.path().join("a.txt"), "one\ntwo\nthree\n");
        write(&dir.path().join("gone.txt"), "bye\n");
        commit(&repo, "init");

        // staged new file
        write(&dir.path().join("b.txt"), "alpha\nbeta\n");
        // staged modify of a.txt
        write(&dir.path().join("a.txt"), "one\ntwo!\nthree\n");
        stage_all(&repo);
        // untracked (NEVER staged — written after the staging pass)
        write(&dir.path().join("untracked.txt"), "ghost\n");
        // unstaged modify on top of the staged a.txt change
        write(&dir.path().join("a.txt"), "one\ntwo!!\nthree\n");
        // unstaged delete
        std::fs::remove_file(dir.path().join("gone.txt")).expect("delete");

        let repo_path = dir.path().to_path_buf();
        Fixture {
            _dir: dir,
            repo_path,
        }
    }

    fn query(files: u32, offset: u32, lines: u32) -> ReadDiffQuery {
        ReadDiffQuery {
            files,
            offset,
            lines,
        }
    }

    #[test]
    fn diff_stats_cover_staged_and_unstaged_and_note_untracked_excluded() {
        let fix = fixture();
        let result = read_worktree_diff(&fix.repo_path, &query(10, 0, 50)).expect("diff");
        // Changed tracked files: a.txt (modified), b.txt (added), gone.txt
        // (deleted). untracked.txt is NOT in the diff (not in any tree).
        assert_eq!(result.files.len(), 3, "files: {:?}", result.files);
        assert!(!result.files_truncated);
        let names: Vec<&str> = result.files.iter().map(|f| f.path.as_str()).collect();
        assert!(
            names.contains(&"a.txt") && names.contains(&"b.txt") && names.contains(&"gone.txt"),
            "files: {names:?}"
        );
        // a.txt: staged "two!" -> "two!!" is 1 add/1 del (the staged line 2
        // replacement is blended into HEAD->workdir: one line changed).
        let a = result.files.iter().find(|f| f.path == "a.txt").unwrap();
        assert_eq!((a.adds, a.dels), (1, 1), "a.txt: {a:?}");
        // b.txt: 2 added lines.
        let b = result.files.iter().find(|f| f.path == "b.txt").unwrap();
        assert_eq!((b.adds, b.dels), (2, 0), "b.txt: {b:?}");
        // gone.txt: 1 deleted line.
        let g = result.files.iter().find(|f| f.path == "gone.txt").unwrap();
        assert_eq!((g.adds, g.dels), (0, 1), "gone.txt: {g:?}");
        // Aggregate stats mirror the per-file sums.
        assert_eq!(result.stats.files, 3);
        assert_eq!(result.stats.adds, 3);
        assert_eq!(result.stats.dels, 2);
    }

    #[test]
    fn diff_page_contains_headers_hunks_and_prefixed_lines() {
        let fix = fixture();
        let result = read_worktree_diff(&fix.repo_path, &query(10, 0, 200)).expect("diff");
        let joined = result.lines.join("\n");
        assert!(
            joined.contains("diff --git"),
            "page must carry unified file headers: {joined}"
        );
        assert!(joined.contains("+++ b/b.txt"), "page: {joined}");
        assert!(joined.contains("--- a/gone.txt"), "page: {joined}");
        assert!(
            joined.contains("@@ -1"),
            "page must carry hunk headers: {joined}"
        );
        // Lines carry the origin prefix: '+' additions, '-' deletions.
        assert!(
            joined
                .lines()
                .any(|l| l.starts_with('+') && l.contains("alpha")),
            "page: {joined}"
        );
        assert!(
            joined
                .lines()
                .any(|l| l.starts_with('-') && l.contains("bye")),
            "page: {joined}"
        );
        // Repo identity is filled by the adapter; head/branch from git.
        assert!(result.head.is_some(), "head sha must be present");
        assert!(
            matches!(result.branch.as_deref(), Some("master") | Some("main")),
            "branch: {:?}",
            result.branch
        );
    }

    #[test]
    fn paging_window_advances_and_reports_has_more() {
        let fix = fixture();
        let all = read_worktree_diff(&fix.repo_path, &query(10, 0, 1000)).expect("full");
        assert!(!all.has_more, "small fixture must be a single page");

        let page1 = read_worktree_diff(&fix.repo_path, &query(10, 0, 5)).expect("p1");
        assert_eq!(page1.lines.len(), 5);
        assert_eq!(page1.next_offset, Some(5));
        assert!(page1.has_more);

        let page2 = read_worktree_diff(&fix.repo_path, &query(10, 5, 5)).expect("p2");
        assert_eq!(page2.lines.len(), 5);
        assert_eq!(page2.offset, 5);
        assert_eq!(page2.next_offset, Some(10));

        // Two consecutive pages tile the head of the stream.
        let mut stream: Vec<String> = page1.lines.clone();
        stream.extend(page2.lines.clone());
        assert_eq!(
            stream,
            all.lines[..10].to_vec(),
            "pages must tile the full stream"
        );

        // Walking the pages to exhaustion reproduces the full stream.
        let mut walked: Vec<String> = Vec::new();
        let mut offset = 0u32;
        loop {
            let page = read_worktree_diff(&fix.repo_path, &query(10, offset, 5)).expect("page");
            assert_eq!(page.offset, offset);
            walked.extend(page.lines);
            match page.next_offset {
                Some(next) => offset = next,
                None => break,
            }
            assert!(offset <= all.total, "paging must terminate");
            if offset == all.total {
                break;
            }
        }
        assert_eq!(walked, all.lines, "page walk must reproduce the stream");
        assert!(!walked.is_empty());
    }

    #[test]
    fn files_cap_truncates_the_list_and_stats_stay_complete() {
        let fix = fixture();
        let result = read_worktree_diff(&fix.repo_path, &query(2, 0, 50)).expect("diff");
        assert_eq!(result.files.len(), 2);
        assert!(result.files_truncated);
        // Stats still count the WHOLE diff.
        assert_eq!(result.stats.files, 3);
        assert_eq!(result.stats.adds, 3);
        assert_eq!(result.stats.dels, 2);
    }

    #[test]
    fn very_long_lines_are_truncated_and_byte_budget_bounds_the_page() {
        let dir = tempfile::tempdir().expect("tempdir");
        let repo = init_repo(dir.path());
        write(&dir.path().join("big.txt"), "x\n");
        commit(&repo, "init");
        // 40 lines x ~5000 chars: per-line truncation caps each at 4096
        // chars; the page then runs into the 64 KiB byte budget before the
        // 30-line window is exhausted (stream total is 4 header pieces + 40
        // lines = 44, so the window also proves has_more/next_offset).
        let mut content = String::new();
        for i in 0..40 {
            content.push_str(&format!("line-{i}-"));
            for _ in 0..4992 {
                content.push('x');
            }
            content.push('\n');
        }
        write(&dir.path().join("big.txt"), &content);
        let result = read_worktree_diff(dir.path(), &query(10, 0, 30)).expect("diff");
        assert!(!result.lines.is_empty());
        assert!(
            result
                .lines
                .iter()
                .all(|l| l.chars().count() <= MAX_LINE_CHARS + 1),
            "per-line truncation must bound every line"
        );
        assert!(
            result.lines.len() < 30,
            "byte budget must bound the page: {}",
            result.lines.len()
        );
        assert!(result.has_more);
        assert_eq!(result.next_offset, Some(30));
        assert!(result.total >= 30);
    }

    #[test]
    fn errors_are_typed() {
        let dir = tempfile::tempdir().expect("tempdir");
        write(&dir.path().join("empty"), "");
        let err = read_worktree_diff(dir.path(), &query(10, 0, 50)).unwrap_err();
        assert!(matches!(err, DiffError::Open(_)), "{err:?}");

        // Unborn: fresh repo with no commits.
        let repo_dir = tempfile::tempdir().expect("tempdir");
        init_repo(repo_dir.path());
        let err = read_worktree_diff(repo_dir.path(), &query(10, 0, 50)).unwrap_err();
        assert!(matches!(err, DiffError::NoHead(_)), "{err:?}");
    }
}
