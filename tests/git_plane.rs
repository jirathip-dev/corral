//! git plane tests: unit coverage lives inline in `git_plane.rs`; here are
//! the end-to-end checks:
//! - `temp_repo_commit_emits_events_under_second` — full pipeline against a
//!   throwaway repo in a temp dir (real fsevents, real `git`), asserting the
//!   <1s acceptance latency for a commit.
//! - `live_herdr_repo_commit_under_one_second` — the acceptance harness
//!   against the real herdr repo (`#[ignore]`d; run explicitly with
//!   `cargo test --test git_plane -- --ignored --nocapture`).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};

use corrald::adapters::git_plane::GitPlane;
use corrald::core::events::{GitEvent, Plane, PlaneEvent};
use corrald::core::plane_channel;
use tokio::sync::mpsc;

/// Run `git` in `cwd`; panic on failure. Read-only flagging is the caller's
/// job — the tests only use commands the plane itself never runs.
fn git(cwd: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .output()
        .expect("git subprocess runs");
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// Wait up to `timeout` for an event matching `pred`; returns the elapsed
/// time to the match (None on timeout / closed channel).
async fn wait_for(
    rx: &mut mpsc::Receiver<PlaneEvent>,
    pred: impl Fn(&PlaneEvent) -> bool,
    timeout: Duration,
) -> Option<Duration> {
    let start = Instant::now();
    loop {
        let remaining = timeout.saturating_sub(start.elapsed());
        if remaining.is_zero() {
            return None;
        }
        match tokio::time::timeout(remaining, rx.recv()).await {
            Ok(Some(event)) if pred(&event) => return Some(start.elapsed()),
            Ok(Some(_)) => continue,
            Ok(None) | Err(_) => return None,
        }
    }
}

fn temp_root(tag: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("corral-git-plane-{}-{nanos}-{tag}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Full pipeline against a temp repo: WorktreeAdded on `worktree add`,
/// DirtyChanged on `git add`, then HeadMoved + CommitOnBranch within <1s of
/// `git commit` (300ms debounce + margin — acceptance criterion 1).
#[tokio::test(flavor = "multi_thread")]
async fn temp_repo_commit_emits_events_under_second() {
    let root = temp_root("integration");
    let repo = root.join("repo");
    let wts = root.join("wts");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::create_dir_all(&wts).unwrap();

    git(&repo, &["init", "-b", "main"]);
    git(&repo, &["config", "user.email", "plane@test.local"]);
    git(&repo, &["config", "user.name", "Plane Test"]);
    std::fs::write(repo.join("README.md"), "hello\n").unwrap();
    git(&repo, &["add", "README.md"]);
    git(&repo, &["commit", "-m", "initial"]);

    let plane = Arc::new(GitPlane::new(repo.clone(), wts.clone()));
    let (sink, mut rx) = plane_channel();
    plane.start(sink);

    // `git worktree add` → WorktreeAdded (discovered via fsevents + rescan).
    // The plane keys events on the CANONICAL worktree path (`git worktree
    // list` resolves symlinks, e.g. /var -> /private/var), so compare
    // against the canonicalized path.
    let wt1 = wts.join("wt1");
    git(&repo, &["worktree", "add", &wt1.to_string_lossy(), "-b", "feat/plane"]);
    let wt1 = fs::canonicalize(&wt1).unwrap();
    assert!(
        wait_for(
            &mut rx,
            |e| matches!(
                e,
                PlaneEvent::Git(GitEvent::WorktreeAdded { worktree }) if worktree == &wt1
            ),
            Duration::from_secs(5),
        )
        .await
        .is_some(),
        "WorktreeAdded for {wt1:?} within 5s"
    );

    // `git add` → DirtyChanged with the index staged.
    std::fs::write(wt1.join("feature.txt"), "one\n").unwrap();
    git(&wt1, &["add", "feature.txt"]);
    assert!(
        wait_for(
            &mut rx,
            |e| matches!(
                e,
                PlaneEvent::Git(GitEvent::DirtyChanged { worktree, status }) if worktree == &wt1 && status.dirty_index
            ),
            Duration::from_secs(5),
        )
        .await
        .is_some(),
        "DirtyChanged(dirty_index) for {wt1:?} after git add"
    );

    // `git commit` → HeadMoved + CommitOnBranch; latency < 1s from commit
    // completion (300ms debounce + margin — acceptance criterion 1).
    git(&wt1, &["commit", "-m", "ws1 integration"]);
    let latency = wait_for(
        &mut rx,
        |e| matches!(
            e,
            PlaneEvent::Git(GitEvent::HeadMoved { worktree, branch, .. })
                if worktree == &wt1 && branch == "feat/plane"
        ),
        Duration::from_secs(3),
    )
    .await
    .expect("HeadMoved within 3s");
    println!(
        "integration: commit -> HeadMoved latency = {latency:?} (measured from commit completion)"
    );
    assert!(
        latency < Duration::from_secs(1),
        "acceptance: git event <1s after commit, got {latency:?}"
    );

    assert!(
        wait_for(
            &mut rx,
            |e| matches!(
                e,
                PlaneEvent::Git(GitEvent::CommitOnBranch { worktree, branch, .. })
                    if worktree == &wt1 && branch == "feat/plane"
            ),
            Duration::from_secs(2),
        )
        .await
        .is_some(),
        "CommitOnBranch for {wt1:?}"
    );
    assert!(
        wait_for(
            &mut rx,
            |e| matches!(
                e,
                PlaneEvent::Git(GitEvent::DirtyChanged { worktree, status })
                    if worktree == &wt1 && !status.is_dirty()
            ),
            Duration::from_secs(2),
        )
        .await
        .is_some(),
        "DirtyChanged(clean) for {wt1:?} after commit"
    );

    let _ = std::fs::remove_dir_all(&root);
}

/// Acceptance criterion 1 (live): against the real herdr repo — start the
/// plane, `git worktree add` a scratch worktree under the herdr root, touch +
/// add + commit, and confirm a GitEvent lands <1s after the commit. Cleans
/// the scratch worktree + branch up afterwards. Run explicitly:
/// `cargo test --test git_plane -- --ignored --nocapture`
#[tokio::test(flavor = "multi_thread")]
#[ignore = "live acceptance against the real herdr repo; run explicitly"]
async fn live_herdr_repo_commit_under_one_second() {
    let home = std::env::var("HOME").expect("HOME");
    let repo = PathBuf::from(
        std::env::var("HERDR_BOARD_REPO").unwrap_or_else(|_| "/Users/jirathip/Projects/herdr-board".to_string()),
    );
    let wts_root = PathBuf::from(
        std::env::var("HERDR_WORKTREES_ROOT")
            .unwrap_or_else(|_| format!("{home}/.herdr/worktrees/herdr-board")),
    );
    let scratch = wts_root.join("ws1-live-test");
    let branch = "ws1-live-test";

    // Fresh start: clear leftovers of a previous (possibly failed) run.
    let _ = Command::new("git")
        .args(["-C", repo.to_str().unwrap(), "worktree", "remove", "--force", scratch.to_str().unwrap()])
        .output();
    let _ = Command::new("git")
        .args(["-C", repo.to_str().unwrap(), "branch", "-D", branch])
        .output();

    let plane = Arc::new(GitPlane::new(repo.clone(), wts_root));
    let (sink, mut rx) = plane_channel();
    plane.start(sink);

    // Worktree add must be picked up as a WorktreeAdded event.
    git(&repo, &["worktree", "add", &scratch.to_string_lossy(), "-b", branch, "ws1/git-plane"]);
    assert!(
        wait_for(
            &mut rx,
            |e| matches!(
                e,
                PlaneEvent::Git(GitEvent::WorktreeAdded { worktree }) if worktree == &scratch
            ),
            Duration::from_secs(5),
        )
        .await
        .is_some(),
        "WorktreeAdded for {scratch:?} within 5s"
    );

    // touch + git add + git commit, then measure commit -> git event.
    std::fs::write(scratch.join("ws1-live-test.txt"), "ws1 live acceptance\n").unwrap();
    git(&scratch, &["add", "ws1-live-test.txt"]);
    git(&scratch, &["commit", "-m", "ws1 live acceptance test"]);

    let t0 = Instant::now();
    let latency = wait_for(
        &mut rx,
        |e| matches!(
            e,
            PlaneEvent::Git(GitEvent::HeadMoved { worktree, branch: b, .. })
                if worktree == &scratch && b == branch
        ),
        Duration::from_secs(3),
    )
    .await
    .expect("HeadMoved within 3s");

    // Clean up the scratch worktree + branch before asserting, so a failure
    // never leaves the herdr repo dirtied.
    let _ = Command::new("git")
        .args(["-C", repo.to_str().unwrap(), "worktree", "remove", "--force", scratch.to_str().unwrap()])
        .output();
    let _ = Command::new("git")
        .args(["-C", repo.to_str().unwrap(), "branch", "-D", branch])
        .output();

    println!("LIVE TEST: commit -> HeadMoved latency = {latency:?} (commit completed {t0:?} ago at measure start)");
    assert!(
        latency < Duration::from_secs(1),
        "acceptance: git event <1s after commit, got {latency:?}"
    );
}
