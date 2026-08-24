//! Plane integrator tests (WS3): a Store + the Integrator fed SYNTHETIC
//! `PlaneEvent`s over the real plane channel — no network, no real fs, no
//! timers. Asserts merged agent records (branch/repo/dirty/ahead-behind/pr/
//! ci propagation), path-keyed convergence, multi-agent fan-out, topology
//! no-ops, and that all planes share ONE monotonic rev per coalesced batch.

use std::path::PathBuf;
use std::time::Duration;

use tokio::sync::mpsc;

use corrald::core::events::{
    GhIssueRef, GhPrState, GhRepoState, GitEvent, GitStatus, PlaneEvent, plane_channel,
};
use corrald::core::model::{Agent, AgentState, Change, CiStatus, Workspace};
use corrald::core::store::Store;
use corrald::core::workspace::{RepoRoot, WorkspaceAttribution, WorktreeAlias};
use corrald::integrate::Integrator;

const REPO_ROOT: &str = "/Users/jirathip/Projects/herdr-board";
const WTS_ROOT: &str = "/Users/jirathip/.herdr/worktrees";
const WT_A: &str = "/Users/jirathip/.herdr/worktrees/herdr-board/corral-p2-ws2";
const WT_B: &str = "/Users/jirathip/.herdr/worktrees/herdr-board/corral-p2-int";

fn agent(id: &str, worktree: Option<&str>) -> Agent {
    Agent {
        agent_id: id.to_string(),
        source: "herdr".to_string(),
        tool: "claude".to_string(),
        state: AgentState::Working,
        reason: None,
        seq: 1,
        ts: 0,
        capabilities: vec!["prompt".to_string()],
        waiting_on: None,
        parent_id: None,
        host: None,
        workspace: Workspace {
            worktree_path: worktree.map(str::to_string),
            ..Default::default()
        },
        attachment: None,
        display_name: None,
        title: None,
    }
}

fn gh_state(repo: &str, prs: Vec<GhPrState>) -> GhRepoState {
    GhRepoState {
        repo: repo.to_string(),
        default_branch: "main".to_string(),
        prs,
        ..Default::default()
    }
}

fn pr(number: u64, head_sha: &str, ci: &str) -> GhPrState {
    GhPrState {
        repo: "herdr-board".to_string(),
        pr_number: number,
        title: "t".to_string(),
        state: "OPEN".to_string(),
        mergeable: "MERGEABLE".to_string(),
        ci_status: ci.to_string(),
        head_sha: head_sha.to_string(),
        head_branch: String::new(),
        closing_issues: Vec::new(),
    }
}

/// A PR carrying a head branch (#22) and authoritative closing refs (#23).
fn pr_with_branch(number: u64, head_sha: &str, head_branch: &str, ci: &str) -> GhPrState {
    let mut p = pr(number, head_sha, ci);
    p.head_branch = head_branch.to_string();
    p
}

fn head(worktree: &str, branch: &str, commit: &str) -> PlaneEvent {
    PlaneEvent::Git(GitEvent::HeadMoved {
        worktree: PathBuf::from(worktree),
        branch: branch.to_string(),
        commit: commit.to_string(),
        subject: Some("add head fields".to_string()),
    })
}

/// Store + integrator on the real plane channel. The consumer task runs on
/// the test runtime; tests wait on observable store state (bounded) instead
/// of sleeping blind.
async fn setup() -> (Store, mpsc::Sender<PlaneEvent>) {
    let store = Store::new();
    let integrator = Integrator::new(
        store.clone(),
        PathBuf::from(REPO_ROOT),
        PathBuf::from(WTS_ROOT),
    );
    let (sink, rx) = plane_channel();
    tokio::spawn(async move { integrator.run(rx).await });
    (store, sink)
}

/// Wait (bounded) until `pred` holds for `id`, returning the record. The
/// consumer processes channel events FIFO, so reaching a later event's
/// observable effect also proves every earlier event was consumed.
async fn wait_for(store: &Store, id: &str, pred: impl Fn(&Agent) -> bool) -> Agent {
    for _ in 0..400 {
        if let Some(agent) = store.get(id).await
            && pred(&agent)
        {
            return agent;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("timed out waiting for agent {id} to converge");
}

#[tokio::test]
async fn git_facts_merge_and_batch_into_one_rev() {
    let (store, sink) = setup().await;
    store.apply(Change::upsert(agent("a", Some(WT_A)))).await;
    assert_eq!(store.flush().await.expect("seed").rev, 1);

    sink.send(head(WT_A, "ws2/gh-plane", "abc123"))
        .await
        .unwrap();
    sink.send(PlaneEvent::Git(GitEvent::DirtyChanged {
        worktree: PathBuf::from(WT_A),
        status: GitStatus {
            dirty_worktree: true,
            ahead: 1,
            behind: 2,
            ..Default::default()
        },
    }))
    .await
    .unwrap();

    let a = wait_for(&store, "a", |a| a.workspace.dirty).await;
    assert_eq!(a.workspace.branch.as_deref(), Some("ws2/gh-plane"));
    assert_eq!(a.workspace.repo.as_deref(), Some("herdr-board"));
    assert_eq!(a.workspace.ahead, 1);
    assert_eq!(a.workspace.behind, 2);
    // G21: the head commit the PR matcher resolves is carried onto the
    // snapshot — sha + first-line subject, no extra git calls.
    assert_eq!(a.workspace.head_sha.as_deref(), Some("abc123"));
    assert_eq!(a.workspace.head_subject.as_deref(), Some("add head fields"));

    // Both facts landed inside ONE coalesce window: one delta, one rev bump,
    // the agent upserted once (deduped by agent_id).
    let delta = store.flush().await.expect("merged delta");
    assert_eq!(delta.rev, 2);
    assert_eq!(delta.upd.len(), 1, "one record per batch, not per event");
    assert_eq!(delta.upd[0].workspace.ci_status, None, "no gh facts yet");
    assert_eq!(
        delta.upd[0].workspace.head_sha.as_deref(),
        Some("abc123"),
        "delta carries head facts"
    );
}

#[tokio::test]
async fn gh_facts_map_pr_and_ci_and_reset_when_pr_leaves_the_open_set() {
    let (store, sink) = setup().await;
    store.apply(Change::upsert(agent("a", Some(WT_A)))).await;
    store.flush().await;

    // Git head first: the agent's commit is the PR-matching key.
    sink.send(head(WT_A, "ws2/gh-plane", "abc123"))
        .await
        .unwrap();
    wait_for(&store, "a", |a| a.workspace.repo.is_some()).await;

    // PR 42's head SHA matches the agent's commit -> pr_number + ci_status,
    // match source = head-SHA (primary, never the fallback).
    sink.send(PlaneEvent::Gh(gh_state(
        "herdr-board",
        vec![pr_with_branch(42, "abc123", "ws2/gh-plane", "SUCCESS")],
    )))
    .await
    .unwrap();
    let a = wait_for(&store, "a", |a| a.workspace.pr_number == Some(42)).await;
    assert_eq!(a.workspace.ci_status, Some(CiStatus::Success));
    assert_eq!(a.workspace.pr_match_source.as_deref(), Some("head_sha"));
    assert!(
        a.workspace.issues.is_empty(),
        "PR 42 links no issues -> empty"
    );

    // The agent commits locally (HEAD moves); the gh cache still carries the
    // OLD head SHA with a FAILURE verdict. PR 42 is still OPEN and its head
    // branch still equals the agent's branch, so the #22 (repo, branch)
    // fallback re-binds it instead of flashing to None.
    sink.send(head(WT_A, "ws2/gh-plane", "def456"))
        .await
        .unwrap();
    sink.send(PlaneEvent::Gh(gh_state(
        "herdr-board",
        vec![pr_with_branch(42, "abc123", "ws2/gh-plane", "FAILURE")],
    )))
    .await
    .unwrap();
    let a = wait_for(&store, "a", |a| {
        a.workspace.ci_status == Some(CiStatus::Failure)
    })
    .await;
    assert_eq!(
        a.workspace.pr_number,
        Some(42),
        "branch fallback keeps the committed-but-unpushed PR bound"
    );
    assert_eq!(a.workspace.pr_match_source.as_deref(), Some("branch"));

    // The PR leaves the open set -> pr/ci reset, git facts untouched.
    sink.send(PlaneEvent::Gh(gh_state("herdr-board", Vec::new())))
        .await
        .unwrap();
    let a = wait_for(&store, "a", |a| a.workspace.pr_number.is_none()).await;
    assert_eq!(a.workspace.ci_status, None);
    assert_eq!(a.workspace.pr_match_source, None);
    assert!(a.workspace.issues.is_empty());
    assert_eq!(a.workspace.branch.as_deref(), Some("ws2/gh-plane"));
    assert!(!a.workspace.dirty && a.workspace.ahead == 0 && a.workspace.behind == 0);
}

#[tokio::test]
async fn issues_join_into_the_workspace_from_the_bound_prs_closing_refs_only() {
    let (store, sink) = setup().await;
    store.apply(Change::upsert(agent("a", Some(WT_A)))).await;
    store.flush().await;

    let mut bound = pr_with_branch(42, "abc123", "ws2/gh-plane", "SUCCESS");
    bound.closing_issues = vec![GhIssueRef {
        repo: "herdr-board".to_string(),
        number: 4,
        state: "OPEN".to_string(),
        title: "P2 planes".to_string(),
        labels: vec![],
        url: String::new(),
    }];
    let state = gh_state("herdr-board", vec![bound]);

    // Head-SHA binding carries the PR's authoritative closing refs (#23).
    sink.send(head(WT_A, "ws2/gh-plane", "abc123"))
        .await
        .unwrap();
    wait_for(&store, "a", |a| a.workspace.repo.is_some()).await;
    sink.send(PlaneEvent::Gh(state.clone())).await.unwrap();
    let a = wait_for(&store, "a", |a| a.workspace.pr_number == Some(42)).await;
    assert_eq!(a.workspace.pr_match_source.as_deref(), Some("head_sha"));
    assert_eq!(a.workspace.issues.len(), 1);
    assert_eq!(a.workspace.issues[0].number, 4);
    assert_eq!(a.workspace.issues[0].state, "OPEN");
    assert_eq!(a.workspace.issues[0].title, "P2 planes");

    // The repo has OTHER recent issues — they must NOT leak into the agent
    // (the linkage is the PR's closing refs, nothing else).
    let mut repo_with_more = state.clone();
    repo_with_more.issues = vec![GhIssueRef {
        repo: "herdr-board".to_string(),
        number: 99,
        state: "OPEN".to_string(),
        title: "unrelated".to_string(),
        labels: vec![],
        url: String::new(),
    }];
    sink.send(PlaneEvent::Gh(repo_with_more)).await.unwrap();
    let a = wait_for(&store, "a", |a| a.workspace.pr_number == Some(42)).await;
    assert_eq!(
        a.workspace.issues.len(),
        1,
        "repo-level issues never populate the agent"
    );

    // A binding to a PR with NO closing refs -> empty array, no guess.
    sink.send(PlaneEvent::Gh(gh_state(
        "herdr-board",
        vec![pr_with_branch(9, "abc123", "ws2/gh-plane", "PENDING")],
    )))
    .await
    .unwrap();
    let a = wait_for(&store, "a", |a| a.workspace.pr_number == Some(9)).await;
    assert!(
        a.workspace.issues.is_empty(),
        "no closing refs -> empty, never a heuristic"
    );

    // Unbound (PR leaves the open set) -> issues reset to empty.
    sink.send(PlaneEvent::Gh(gh_state("herdr-board", Vec::new())))
        .await
        .unwrap();
    let a = wait_for(&store, "a", |a| a.workspace.pr_number.is_none()).await;
    assert!(a.workspace.issues.is_empty());
    assert_eq!(a.workspace.pr_match_source, None);
}

#[tokio::test]
async fn converges_when_agent_appears_after_facts_were_cached() {
    let (store, sink) = setup().await;
    // Facts arrive while NO agent matches the path: cached, nothing applied.
    sink.send(head(WT_A, "ws2/gh-plane", "abc123"))
        .await
        .unwrap();
    sink.send(PlaneEvent::Git(GitEvent::DirtyChanged {
        worktree: PathBuf::from(WT_A),
        status: GitStatus {
            dirty_index: true,
            ahead: 3,
            ..Default::default()
        },
    }))
    .await
    .unwrap();

    // The agent appears; the NEXT event for the path re-applies the FULL
    // cached fact set (path-keyed convergence).
    store.apply(Change::upsert(agent("a", Some(WT_A)))).await;
    store.flush().await;
    sink.send(head(WT_A, "ws2/gh-plane", "def456"))
        .await
        .unwrap();

    let a = wait_for(&store, "a", |a| a.workspace.repo.is_some()).await;
    assert_eq!(a.workspace.branch.as_deref(), Some("ws2/gh-plane"));
    assert_eq!(a.workspace.repo.as_deref(), Some("herdr-board"));
    assert!(
        a.workspace.dirty,
        "cached dirty fact applied on first match"
    );
    assert_eq!(a.workspace.ahead, 3);
}

#[tokio::test]
async fn agent_appears_with_zero_subsequent_plane_events_still_converges() {
    // WS3 F1: convergence must NOT be event-gated. Facts are cached while a
    // sentinel agent observes them; a second agent is then created with NO
    // plane events afterwards — the store change signal alone must apply the
    // cached facts.
    let (store, sink) = setup().await;
    store
        .apply(Change::upsert(agent("sentinel", Some(WT_A))))
        .await;
    store.flush().await;
    sink.send(head(WT_A, "ws2/gh-plane", "abc123"))
        .await
        .unwrap();
    sink.send(PlaneEvent::Git(GitEvent::DirtyChanged {
        worktree: PathBuf::from(WT_A),
        status: GitStatus {
            dirty_worktree: true,
            ahead: 1,
            behind: 2,
            ..Default::default()
        },
    }))
    .await
    .unwrap();
    // The sentinel converging proves the facts were cached BEFORE the apply.
    wait_for(&store, "sentinel", |a| a.workspace.dirty).await;

    // The new agent appears; NOTHING is sent after this point.
    store.apply(Change::upsert(agent("late", Some(WT_A)))).await;

    let late = wait_for(&store, "late", |a| a.workspace.repo.is_some()).await;
    assert_eq!(late.workspace.branch.as_deref(), Some("ws2/gh-plane"));
    assert!(
        late.workspace.dirty,
        "cached dirty fact applied without any plane event"
    );
    assert_eq!((late.workspace.ahead, late.workspace.behind), (1, 2));
}

#[tokio::test]
async fn other_repo_worktrees_merge_fleet_wide() {
    // WS3 F2: an agent in a project-hearthwild worktree (a repo outside the
    // main checkout) must get git facts + repo + PR/CI like any herdr-board
    // agent.
    let (store, sink) = setup().await;
    let wt_ph = format!("{WTS_ROOT}/project-hearthwild/feat-plush-visual-fidelity");
    store.apply(Change::upsert(agent("ph", Some(&wt_ph)))).await;
    store.flush().await;

    sink.send(head(&wt_ph, "feat/plush-visual-fidelity", "abc123"))
        .await
        .unwrap();
    sink.send(PlaneEvent::Git(GitEvent::DirtyChanged {
        worktree: PathBuf::from(&wt_ph),
        status: GitStatus {
            dirty_index: true,
            ..Default::default()
        },
    }))
    .await
    .unwrap();
    wait_for(&store, "ph", |a| a.workspace.dirty).await;
    sink.send(PlaneEvent::Gh(gh_state(
        "project-hearthwild",
        vec![pr(9, "abc123", "SUCCESS")],
    )))
    .await
    .unwrap();

    let ph = wait_for(&store, "ph", |a| a.workspace.pr_number == Some(9)).await;
    assert_eq!(ph.workspace.repo.as_deref(), Some("project-hearthwild"));
    assert_eq!(
        ph.workspace.branch.as_deref(),
        Some("feat/plush-visual-fidelity")
    );
    assert_eq!(ph.workspace.ci_status, Some(CiStatus::Success));
}

#[tokio::test]
async fn worktree_removed_resets_git_derived_fields_and_pr_binding() {
    // WS3 F6: removing a worktree must not leave the agent claiming a branch
    // or PR of a nonexistent worktree.
    let (store, sink) = setup().await;
    store.apply(Change::upsert(agent("a", Some(WT_A)))).await;
    store.flush().await;

    sink.send(head(WT_A, "ws2/gh-plane", "abc123"))
        .await
        .unwrap();
    wait_for(&store, "a", |a| a.workspace.repo.is_some()).await;
    sink.send(PlaneEvent::Gh(gh_state(
        "herdr-board",
        vec![pr(42, "abc123", "PENDING")],
    )))
    .await
    .unwrap();
    wait_for(&store, "a", |a| a.workspace.pr_number == Some(42)).await;

    sink.send(PlaneEvent::Git(GitEvent::WorktreeRemoved {
        worktree: PathBuf::from(WT_A),
    }))
    .await
    .unwrap();
    let a = wait_for(&store, "a", |a| a.workspace.pr_number.is_none()).await;
    assert_eq!(a.workspace.branch, None, "git-derived branch reset");
    assert!(!a.workspace.dirty, "git-derived dirty reset");
    assert_eq!((a.workspace.ahead, a.workspace.behind), (0, 0));
    assert_eq!(a.workspace.ci_status, None, "PR binding dropped");
    assert_eq!(
        a.workspace.head_sha, None,
        "head facts dropped with the worktree (G21)"
    );
    assert_eq!(a.workspace.head_subject, None);
    assert_eq!(
        a.workspace.pr_match_source, None,
        "match source dropped with the binding"
    );
    assert!(
        a.workspace.issues.is_empty(),
        "issues dropped with the binding"
    );
    // repo is path-derived, not git-fact-derived: it survives the reset.
    assert_eq!(a.workspace.repo.as_deref(), Some("herdr-board"));
}

#[tokio::test]
async fn detached_head_normalizes_branch_to_none() {
    // WS3 F7: the git plane reports detached HEAD as the literal "HEAD";
    // the read model must show branch: null, never "HEAD".
    let (store, sink) = setup().await;
    store.apply(Change::upsert(agent("a", Some(WT_A)))).await;
    store.flush().await;

    sink.send(head(WT_A, "ws2/gh-plane", "abc123"))
        .await
        .unwrap();
    wait_for(&store, "a", |a| a.workspace.branch.is_some()).await;

    sink.send(head(WT_A, "HEAD", "def456")).await.unwrap();
    let a = wait_for(&store, "a", |a| a.workspace.branch.is_none()).await;
    assert_eq!(
        a.workspace.repo.as_deref(),
        Some("herdr-board"),
        "repo unaffected"
    );
}

#[tokio::test]
async fn facts_fan_out_to_every_matching_agent() {
    let (store, sink) = setup().await;
    // Two agents share one worktree; two more share the derived repo via a
    // different worktree path.
    for (id, wt) in [("a1", WT_A), ("a2", WT_A), ("b1", WT_B), ("b2", WT_B)] {
        store.apply(Change::upsert(agent(id, Some(wt)))).await;
    }
    store.flush().await;

    sink.send(head(WT_A, "ws2/gh-plane", "abc123"))
        .await
        .unwrap();
    sink.send(head(WT_B, "feat/corral-p2", "abc123"))
        .await
        .unwrap();
    wait_for(&store, "b2", |a| a.workspace.branch.is_some()).await;
    sink.send(PlaneEvent::Gh(gh_state(
        "herdr-board",
        vec![pr(42, "abc123", "PENDING")],
    )))
    .await
    .unwrap();

    for id in ["a1", "a2", "b1", "b2"] {
        let a = wait_for(&store, id, |a| a.workspace.pr_number == Some(42)).await;
        assert_eq!(a.workspace.repo.as_deref(), Some("herdr-board"));
        assert_eq!(a.workspace.ci_status, Some(CiStatus::Pending));
    }
}

#[tokio::test]
async fn main_checkout_derives_repo_from_root_name() {
    let (store, sink) = setup().await;
    store
        .apply(Change::upsert(agent("main", Some(REPO_ROOT))))
        .await;
    store.flush().await;

    sink.send(head(REPO_ROOT, "main", "abc123")).await.unwrap();
    let a = wait_for(&store, "main", |a| a.workspace.repo.is_some()).await;
    assert_eq!(
        a.workspace.repo.as_deref(),
        Some("herdr-board"),
        "main checkout repo = root dir name"
    );
    assert_eq!(a.workspace.branch.as_deref(), Some("main"));
}

#[cfg(unix)]
#[tokio::test]
async fn shared_attribution_merges_primary_alias_and_keeps_unknown_orphaned() {
    let temp = tempfile::tempdir().expect("tempdir");
    let primary = temp.path().join("primary");
    let worktrees = temp.path().join("worktrees");
    let alias = temp.path().join("primary-alias");
    let unknown = temp.path().join("unknown");
    std::fs::create_dir_all(&primary).unwrap();
    std::fs::create_dir_all(&worktrees).unwrap();
    std::fs::create_dir_all(&unknown).unwrap();
    std::os::unix::fs::symlink(&primary, &alias).unwrap();

    let attribution = WorkspaceAttribution::from_roots(
        [RepoRoot {
            path: primary.clone(),
            repo: "registry-repo".to_string(),
        }],
        worktrees,
    );
    let store = Store::new();
    let integrator = Integrator::new_with_attribution(store.clone(), attribution);
    let (sink, rx) = plane_channel();
    tokio::spawn(async move { integrator.run(rx).await });

    let alias_string = alias.to_string_lossy().into_owned();
    let unknown_string = unknown.to_string_lossy().into_owned();
    let primary_string = primary.to_string_lossy().into_owned();
    store
        .apply(Change::upsert(agent("primary", Some(&alias_string))))
        .await;
    store
        .apply(Change::upsert(agent("unknown", Some(&unknown_string))))
        .await;
    sink.send(head(&primary_string, "main", "abc123"))
        .await
        .unwrap();
    sink.send(head(&unknown_string, "should-not-attribute", "def456"))
        .await
        .unwrap();

    let primary = wait_for(&store, "primary", |agent| {
        agent.workspace.repo.as_deref() == Some("registry-repo")
            && agent.workspace.branch.as_deref() == Some("main")
    })
    .await;
    assert_eq!(primary.workspace.repo.as_deref(), Some("registry-repo"));
    assert_eq!(primary.workspace.branch.as_deref(), Some("main"));
    let unknown = store.get("unknown").await.expect("unknown agent");
    assert_eq!(unknown.workspace.repo, None);
    assert_eq!(unknown.workspace.branch, None);
}

#[cfg(unix)]
#[tokio::test]
async fn registry_worktree_alias_keeps_primary_and_linked_in_one_repo_group() {
    let temp = tempfile::tempdir().expect("tempdir");
    let primary = temp.path().join("stale-checkout");
    let worktrees = temp.path().join("worktrees");
    let linked = worktrees.join("stale-checkout/g182-fix");
    let other = worktrees.join("another-repo/g182-fix");
    std::fs::create_dir_all(&primary).unwrap();
    std::fs::create_dir_all(&linked).unwrap();
    std::fs::create_dir_all(&other).unwrap();

    let attribution = WorkspaceAttribution::from_roots_with_aliases(
        [RepoRoot {
            path: primary.clone(),
            repo: "canonical-repo".to_string(),
        }],
        [WorktreeAlias {
            worktree_dir: "stale-checkout".to_string(),
            repo: "canonical-repo".to_string(),
        }],
        worktrees,
    );
    let store = Store::new();
    let integrator = Integrator::new_with_attribution(store.clone(), attribution);
    let (sink, rx) = plane_channel();
    tokio::spawn(async move { integrator.run(rx).await });

    let primary_string = primary.to_string_lossy().into_owned();
    let linked_string = linked.to_string_lossy().into_owned();
    let other_string = other.to_string_lossy().into_owned();
    store
        .apply(Change::upsert(agent("primary", Some(&primary_string))))
        .await;
    store
        .apply(Change::upsert(agent("linked", Some(&linked_string))))
        .await;
    store
        .apply(Change::upsert(agent("other", Some(&other_string))))
        .await;
    sink.send(head(&primary_string, "main", "abc123"))
        .await
        .unwrap();
    sink.send(head(&linked_string, "g182/fix", "def456"))
        .await
        .unwrap();
    sink.send(head(&other_string, "not-fleet", "789abc"))
        .await
        .unwrap();

    let primary = wait_for(&store, "primary", |agent| {
        agent.workspace.repo.as_deref() == Some("canonical-repo")
            && agent.workspace.branch.as_deref() == Some("main")
    })
    .await;
    let linked = wait_for(&store, "linked", |agent| {
        agent.workspace.repo.as_deref() == Some("canonical-repo")
            && agent.workspace.branch.as_deref() == Some("g182/fix")
    })
    .await;
    assert_eq!(primary.workspace.repo.as_deref(), Some("canonical-repo"));
    assert_eq!(linked.workspace.repo.as_deref(), Some("canonical-repo"));
    let other = store.get("other").await.expect("other agent");
    assert_eq!(
        other.workspace.repo.as_deref(),
        Some("another-repo"),
        "unregistered linked worktree keeps the path-derived fallback"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn generation_restart_clears_vanished_branch_and_reconciles_present_path() {
    let temp = tempfile::tempdir().expect("tempdir");
    let primary = temp.path().join("primary");
    let worktrees = temp.path().join("worktrees");
    let present = worktrees.join("linked-repo/present");
    let vanished = worktrees.join("linked-repo/vanished");
    let unknown = temp.path().join("unknown");
    std::fs::create_dir_all(&primary).unwrap();
    std::fs::create_dir_all(&present).unwrap();
    std::fs::create_dir_all(&vanished).unwrap();

    let attribution = WorkspaceAttribution::from_roots(
        [RepoRoot {
            path: primary,
            repo: "primary-repo".to_string(),
        }],
        worktrees,
    );
    attribution.record_branch(&present, "old-present");
    attribution.record_branch(&vanished, "stale-vanished");
    let present_string = present.to_string_lossy().into_owned();
    let vanished_string = vanished.to_string_lossy().into_owned();
    let unknown_string = unknown.to_string_lossy().into_owned();

    let store = Store::new();
    let first = Integrator::new_with_attribution(store.clone(), attribution.clone());
    let (first_sink, first_rx) = plane_channel();
    let first_task = tokio::spawn(async move { first.run(first_rx).await });
    store
        .apply(Change::upsert(agent("old-present", Some(&present_string))))
        .await;
    store
        .apply(Change::upsert(agent(
            "old-vanished",
            Some(&vanished_string),
        )))
        .await;
    let mut unknown_agent = agent("unknown", Some(&unknown_string));
    unknown_agent.workspace.branch = Some("orphan-branch".to_string());
    let unknown_before = unknown_agent.clone();
    store.apply(Change::upsert(unknown_agent)).await;
    first_sink
        .send(head(&present_string, "old-present", "present-old-sha"))
        .await
        .unwrap();
    first_sink
        .send(PlaneEvent::Git(GitEvent::DirtyChanged {
            worktree: PathBuf::from(&present_string),
            status: GitStatus {
                dirty_worktree: true,
                ahead: 3,
                behind: 2,
                ..Default::default()
            },
        }))
        .await
        .unwrap();
    first_sink
        .send(head(&vanished_string, "stale-vanished", "old-sha"))
        .await
        .unwrap();
    let mut present_pr = pr(7, "present-old-sha", "PENDING");
    present_pr.repo = "linked-repo".to_string();
    present_pr.closing_issues = vec![GhIssueRef {
        repo: "linked-repo".to_string(),
        number: 109,
        state: "OPEN".to_string(),
        title: "generation boundary".to_string(),
        labels: vec![],
        url: String::new(),
    }];
    first_sink
        .send(PlaneEvent::Gh(gh_state("linked-repo", vec![present_pr])))
        .await
        .unwrap();

    let old_present = wait_for(&store, "old-present", |agent| {
        agent.workspace.branch.as_deref() == Some("old-present")
            && agent.workspace.pr_number == Some(7)
            && agent.workspace.dirty
    })
    .await;
    let old_vanished = wait_for(&store, "old-vanished", |agent| {
        agent.workspace.branch.as_deref() == Some("stale-vanished")
    })
    .await;
    assert_eq!(old_present.workspace.repo.as_deref(), Some("linked-repo"));
    assert_eq!(old_vanished.workspace.repo.as_deref(), Some("linked-repo"));

    // The directory disappears while the first generation is unable to
    // deliver its removal event. Its sender then closes, just like a failed
    // plane/integrator generation in production.
    std::fs::remove_dir_all(&vanished).unwrap();
    drop(first_sink);
    first_task.await.expect("first integrator generation exits");

    // The supervisor's generation boundary clears both the shared branch
    // cache and stored recognized rows before replacement-plane facts arrive.
    // The repo/path identity and all other workspace/GitHub fields survive.
    let second = Integrator::new_with_attribution(store.clone(), attribution.clone());
    second.reconcile_generation().await;
    let old_present_cleared = store.get("old-present").await.expect("old present agent");
    let mut expected_present = old_present.clone();
    expected_present.workspace.branch = None;
    assert_eq!(old_present_cleared, expected_present);
    let old_vanished_cleared = store.get("old-vanished").await.expect("old vanished agent");
    assert_eq!(
        old_vanished_cleared.workspace.repo.as_deref(),
        Some("linked-repo")
    );
    assert_eq!(old_vanished_cleared.workspace.branch, None);
    assert_eq!(
        store.get("unknown").await.expect("unknown agent"),
        unknown_before
    );

    let vanished_facts = attribution
        .facts_for(&vanished)
        .expect("layout remains known");
    assert_eq!(vanished_facts.branch, None);
    assert!(!vanished_facts.branch_known);

    let (second_sink, second_rx) = plane_channel();
    let second_task = tokio::spawn(async move { second.run(second_rx).await });
    store
        .apply(Change::upsert(agent(
            "fresh-present",
            Some(&present_string),
        )))
        .await;
    store
        .apply(Change::upsert(agent(
            "fresh-vanished",
            Some(&vanished_string),
        )))
        .await;
    second_sink
        .send(head(&present_string, "current-present", "current-sha"))
        .await
        .unwrap();

    let old_present = wait_for(&store, "old-present", |agent| {
        agent.workspace.branch.as_deref() == Some("current-present")
    })
    .await;
    assert_eq!(old_present.workspace.repo.as_deref(), Some("linked-repo"));
    let present = wait_for(&store, "fresh-present", |agent| {
        agent.workspace.branch.as_deref() == Some("current-present")
    })
    .await;
    assert_eq!(present.workspace.repo.as_deref(), Some("linked-repo"));
    let vanished = wait_for(&store, "fresh-vanished", |agent| {
        agent.workspace.repo.as_deref() == Some("linked-repo") && agent.workspace.branch.is_none()
    })
    .await;
    assert_eq!(vanished.workspace.branch, None);
    drop(second_sink);
    second_task
        .await
        .expect("second integrator generation exits");
}

#[tokio::test]
async fn topology_events_never_create_or_remove_agents() {
    let (store, sink) = setup().await;
    store.apply(Change::upsert(agent("a", Some(WT_A)))).await;
    store.flush().await;

    sink.send(head(WT_A, "ws2/gh-plane", "abc123"))
        .await
        .unwrap();
    wait_for(&store, "a", |a| a.workspace.repo.is_some()).await;

    // Topology facts carry no read-model payload: nothing to map.
    sink.send(PlaneEvent::Git(GitEvent::WorktreeRemoved {
        worktree: PathBuf::from(WT_A),
    }))
    .await
    .unwrap();
    sink.send(PlaneEvent::Git(GitEvent::WorktreeAdded {
        worktree: PathBuf::from(WT_A),
    }))
    .await
    .unwrap();

    // Facts flow again for the re-added worktree (cache rebuilt).
    sink.send(head(WT_A, "feat/x", "def456")).await.unwrap();
    let a = wait_for(&store, "a", |a| {
        a.workspace.branch.as_deref() == Some("feat/x")
    })
    .await;
    assert_eq!(a.workspace.repo.as_deref(), Some("herdr-board"));

    let snap = store.snapshot().await;
    assert_eq!(
        snap.agents.len(),
        1,
        "topology facts never create synthetic agents"
    );
}

#[tokio::test]
async fn unchanged_facts_produce_no_delta() {
    let (store, sink) = setup().await;
    for (id, wt) in [("a", WT_A), ("b", WT_B)] {
        store.apply(Change::upsert(agent(id, Some(wt)))).await;
    }
    store.flush().await;

    // Converge both agents (same commit), then bind them to PR 42.
    sink.send(head(WT_A, "ws2/gh-plane", "abc123"))
        .await
        .unwrap();
    sink.send(head(WT_B, "feat/corral-p2", "abc123"))
        .await
        .unwrap();
    wait_for(&store, "b", |a| a.workspace.branch.is_some()).await;
    sink.send(PlaneEvent::Gh(gh_state(
        "herdr-board",
        vec![pr(42, "abc123", "SUCCESS")],
    )))
    .await
    .unwrap();
    wait_for(&store, "a", |a| a.workspace.pr_number == Some(42)).await;
    store.flush().await; // rev 2

    // The SAME gh state re-arrives (e.g. a dedupe miss upstream), followed
    // by a real change to agent b only. The duplicate must not re-upsert a:
    // the flush carries exactly one record.
    sink.send(PlaneEvent::Gh(gh_state(
        "herdr-board",
        vec![pr(42, "abc123", "SUCCESS")],
    )))
    .await
    .unwrap();
    sink.send(PlaneEvent::Git(GitEvent::DirtyChanged {
        worktree: PathBuf::from(WT_B),
        status: GitStatus {
            dirty_worktree: true,
            ..Default::default()
        },
    }))
    .await
    .unwrap();
    let b = wait_for(&store, "b", |a| a.workspace.dirty).await;
    assert_eq!(
        b.workspace.pr_number,
        Some(42),
        "duplicate gh re-apply left b's PR intact"
    );

    let delta = store.flush().await.expect("batch with b's dirty change");
    assert_eq!(delta.rev, 3);
    assert_eq!(delta.upd.len(), 1, "unchanged duplicate produced no upsert");
    assert_eq!(delta.upd[0].agent_id, "b");
}

/// D9 regression (G21 re-review F1): a commit subject containing a seeded
/// secret must reach the read model (the snapshot's source) redacted —
/// never the raw token — while `head_sha` (identity) stays raw.
#[tokio::test]
async fn head_subject_egresses_redacted_not_raw() {
    const GHP: &str = "ghp_AbCdEfGhIjKlMnOpQrStUvWxYz1234567890";
    let (store, sink) = setup().await;
    store.apply(Change::upsert(agent("a", Some(WT_A)))).await;
    store.flush().await;

    sink.send(PlaneEvent::Git(GitEvent::HeadMoved {
        worktree: PathBuf::from(WT_A),
        branch: "ws2/gh-plane".to_string(),
        commit: "abc123".to_string(),
        subject: Some(format!("fix: rotate {GHP} before release")),
    }))
    .await
    .unwrap();

    let a = wait_for(&store, "a", |a| a.workspace.head_sha.is_some()).await;
    assert_eq!(
        a.workspace.head_subject.as_deref(),
        Some("fix: rotate [REDACTED] before release"),
        "the subject egresses redacted (F1)"
    );
    assert!(
        !a.workspace.head_subject.is_some_and(|s| s.contains(GHP)),
        "no raw PAT may reach the snapshot source"
    );
    assert_eq!(
        a.workspace.head_sha.as_deref(),
        Some("abc123"),
        "the sha stays raw (identity)"
    );

    // The delta carrying the record is equally redacted.
    let delta = store.flush().await.expect("head-facts delta");
    assert_eq!(
        delta.upd[0].workspace.head_subject.as_deref(),
        Some("fix: rotate [REDACTED] before release"),
        "delta payload is redacted (F1)"
    );
}
