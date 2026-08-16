//! Plane integrator tests (WS3): a Store + the Integrator fed SYNTHETIC
//! `PlaneEvent`s over the real plane channel — no network, no real fs, no
//! timers. Asserts merged agent records (branch/repo/dirty/ahead-behind/pr/
//! ci propagation), path-keyed convergence, multi-agent fan-out, topology
//! no-ops, and that all planes share ONE monotonic rev per coalesced batch.

use std::path::PathBuf;
use std::time::Duration;

use tokio::sync::mpsc;

use corrald::core::events::{
    plane_channel, GhPrState, GhRepoState, GitEvent, GitStatus, PlaneEvent,
};
use corrald::core::model::{Agent, AgentState, Change, CiStatus, Workspace};
use corrald::core::store::Store;
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
        cost: None,
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
    }
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
    let integrator =
        Integrator::new(store.clone(), PathBuf::from(REPO_ROOT), PathBuf::from(WTS_ROOT));
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

    sink.send(head(WT_A, "ws2/gh-plane", "abc123")).await.unwrap();
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
    assert_eq!(delta.upd[0].workspace.head_sha.as_deref(), Some("abc123"), "delta carries head facts");
}

#[tokio::test]
async fn gh_facts_map_pr_and_ci_and_reset_when_pr_leaves_the_open_set() {
    let (store, sink) = setup().await;
    store.apply(Change::upsert(agent("a", Some(WT_A)))).await;
    store.flush().await;

    // Git head first: the agent's commit is the PR-matching key.
    sink.send(head(WT_A, "ws2/gh-plane", "abc123")).await.unwrap();
    wait_for(&store, "a", |a| a.workspace.repo.is_some()).await;

    // PR 42's head SHA matches the agent's commit -> pr_number + ci_status.
    sink.send(PlaneEvent::Gh(gh_state("herdr-board", vec![pr(42, "abc123", "SUCCESS")]))).await.unwrap();
    let a = wait_for(&store, "a", |a| a.workspace.pr_number == Some(42)).await;
    assert_eq!(a.workspace.ci_status, Some(CiStatus::Success));

    // The agent commits locally (HEAD moves); the gh cache still carries the
    // OLD head SHA with a FAILURE verdict. PR 42 is still OPEN, so the bound
    // PR survives the lag instead of flashing to None — and the WS2 verdict
    // is mapped verbatim (G4).
    sink.send(head(WT_A, "ws2/gh-plane", "def456")).await.unwrap();
    sink.send(PlaneEvent::Gh(gh_state("herdr-board", vec![pr(42, "abc123", "FAILURE")]))).await.unwrap();
    let a = wait_for(&store, "a", |a| a.workspace.ci_status == Some(CiStatus::Failure)).await;
    assert_eq!(a.workspace.pr_number, Some(42), "still-open bound PR survives head-SHA lag");

    // The PR leaves the open set -> pr/ci reset, git facts untouched.
    sink.send(PlaneEvent::Gh(gh_state("herdr-board", Vec::new()))).await.unwrap();
    let a = wait_for(&store, "a", |a| a.workspace.pr_number.is_none()).await;
    assert_eq!(a.workspace.ci_status, None);
    assert_eq!(a.workspace.branch.as_deref(), Some("ws2/gh-plane"));
    assert!(!a.workspace.dirty && a.workspace.ahead == 0 && a.workspace.behind == 0);
}

#[tokio::test]
async fn converges_when_agent_appears_after_facts_were_cached() {
    let (store, sink) = setup().await;
    // Facts arrive while NO agent matches the path: cached, nothing applied.
    sink.send(head(WT_A, "ws2/gh-plane", "abc123")).await.unwrap();
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
    sink.send(head(WT_A, "ws2/gh-plane", "def456")).await.unwrap();

    let a = wait_for(&store, "a", |a| a.workspace.repo.is_some()).await;
    assert_eq!(a.workspace.branch.as_deref(), Some("ws2/gh-plane"));
    assert_eq!(a.workspace.repo.as_deref(), Some("herdr-board"));
    assert!(a.workspace.dirty, "cached dirty fact applied on first match");
    assert_eq!(a.workspace.ahead, 3);
}

#[tokio::test]
async fn agent_appears_with_zero_subsequent_plane_events_still_converges() {
    // WS3 F1: convergence must NOT be event-gated. Facts are cached while a
    // sentinel agent observes them; a second agent is then created with NO
    // plane events afterwards — the store change signal alone must apply the
    // cached facts.
    let (store, sink) = setup().await;
    store.apply(Change::upsert(agent("sentinel", Some(WT_A)))).await;
    store.flush().await;
    sink.send(head(WT_A, "ws2/gh-plane", "abc123")).await.unwrap();
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
    assert!(late.workspace.dirty, "cached dirty fact applied without any plane event");
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

    sink.send(head(&wt_ph, "feat/plush-visual-fidelity", "abc123")).await.unwrap();
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
    sink.send(PlaneEvent::Gh(gh_state("project-hearthwild", vec![pr(9, "abc123", "SUCCESS")]))).await.unwrap();

    let ph = wait_for(&store, "ph", |a| a.workspace.pr_number == Some(9)).await;
    assert_eq!(ph.workspace.repo.as_deref(), Some("project-hearthwild"));
    assert_eq!(ph.workspace.branch.as_deref(), Some("feat/plush-visual-fidelity"));
    assert_eq!(ph.workspace.ci_status, Some(CiStatus::Success));
}

#[tokio::test]
async fn worktree_removed_resets_git_derived_fields_and_pr_binding() {
    // WS3 F6: removing a worktree must not leave the agent claiming a branch
    // or PR of a nonexistent worktree.
    let (store, sink) = setup().await;
    store.apply(Change::upsert(agent("a", Some(WT_A)))).await;
    store.flush().await;

    sink.send(head(WT_A, "ws2/gh-plane", "abc123")).await.unwrap();
    wait_for(&store, "a", |a| a.workspace.repo.is_some()).await;
    sink.send(PlaneEvent::Gh(gh_state("herdr-board", vec![pr(42, "abc123", "PENDING")]))).await.unwrap();
    wait_for(&store, "a", |a| a.workspace.pr_number == Some(42)).await;

    sink.send(PlaneEvent::Git(GitEvent::WorktreeRemoved { worktree: PathBuf::from(WT_A) })).await.unwrap();
    let a = wait_for(&store, "a", |a| a.workspace.pr_number.is_none()).await;
    assert_eq!(a.workspace.branch, None, "git-derived branch reset");
    assert!(!a.workspace.dirty, "git-derived dirty reset");
    assert_eq!((a.workspace.ahead, a.workspace.behind), (0, 0));
    assert_eq!(a.workspace.ci_status, None, "PR binding dropped");
    assert_eq!(a.workspace.head_sha, None, "head facts dropped with the worktree (G21)");
    assert_eq!(a.workspace.head_subject, None);
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

    sink.send(head(WT_A, "ws2/gh-plane", "abc123")).await.unwrap();
    wait_for(&store, "a", |a| a.workspace.branch.is_some()).await;

    sink.send(head(WT_A, "HEAD", "def456")).await.unwrap();
    let a = wait_for(&store, "a", |a| a.workspace.branch.is_none()).await;
    assert_eq!(a.workspace.repo.as_deref(), Some("herdr-board"), "repo unaffected");
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

    sink.send(head(WT_A, "ws2/gh-plane", "abc123")).await.unwrap();
    sink.send(head(WT_B, "feat/corral-p2", "abc123")).await.unwrap();
    wait_for(&store, "b2", |a| a.workspace.branch.is_some()).await;
    sink.send(PlaneEvent::Gh(gh_state("herdr-board", vec![pr(42, "abc123", "PENDING")]))).await.unwrap();

    for id in ["a1", "a2", "b1", "b2"] {
        let a = wait_for(&store, id, |a| a.workspace.pr_number == Some(42)).await;
        assert_eq!(a.workspace.repo.as_deref(), Some("herdr-board"));
        assert_eq!(a.workspace.ci_status, Some(CiStatus::Pending));
    }
}

#[tokio::test]
async fn main_checkout_derives_repo_from_root_name() {
    let (store, sink) = setup().await;
    store.apply(Change::upsert(agent("main", Some(REPO_ROOT)))).await;
    store.flush().await;

    sink.send(head(REPO_ROOT, "main", "abc123")).await.unwrap();
    let a = wait_for(&store, "main", |a| a.workspace.repo.is_some()).await;
    assert_eq!(a.workspace.repo.as_deref(), Some("herdr-board"), "main checkout repo = root dir name");
    assert_eq!(a.workspace.branch.as_deref(), Some("main"));
}

#[tokio::test]
async fn topology_events_never_create_or_remove_agents() {
    let (store, sink) = setup().await;
    store.apply(Change::upsert(agent("a", Some(WT_A)))).await;
    store.flush().await;

    sink.send(head(WT_A, "ws2/gh-plane", "abc123")).await.unwrap();
    wait_for(&store, "a", |a| a.workspace.repo.is_some()).await;

    // Topology facts carry no read-model payload: nothing to map.
    sink.send(PlaneEvent::Git(GitEvent::WorktreeRemoved { worktree: PathBuf::from(WT_A) })).await.unwrap();
    sink.send(PlaneEvent::Git(GitEvent::WorktreeAdded { worktree: PathBuf::from(WT_A) })).await.unwrap();

    // Facts flow again for the re-added worktree (cache rebuilt).
    sink.send(head(WT_A, "feat/x", "def456")).await.unwrap();
    let a = wait_for(&store, "a", |a| a.workspace.branch.as_deref() == Some("feat/x")).await;
    assert_eq!(a.workspace.repo.as_deref(), Some("herdr-board"));

    let snap = store.snapshot().await;
    assert_eq!(snap.agents.len(), 1, "topology facts never create synthetic agents");
}

#[tokio::test]
async fn unchanged_facts_produce_no_delta() {
    let (store, sink) = setup().await;
    for (id, wt) in [("a", WT_A), ("b", WT_B)] {
        store.apply(Change::upsert(agent(id, Some(wt)))).await;
    }
    store.flush().await;

    // Converge both agents (same commit), then bind them to PR 42.
    sink.send(head(WT_A, "ws2/gh-plane", "abc123")).await.unwrap();
    sink.send(head(WT_B, "feat/corral-p2", "abc123")).await.unwrap();
    wait_for(&store, "b", |a| a.workspace.branch.is_some()).await;
    sink.send(PlaneEvent::Gh(gh_state("herdr-board", vec![pr(42, "abc123", "SUCCESS")]))).await.unwrap();
    wait_for(&store, "a", |a| a.workspace.pr_number == Some(42)).await;
    store.flush().await; // rev 2

    // The SAME gh state re-arrives (e.g. a dedupe miss upstream), followed
    // by a real change to agent b only. The duplicate must not re-upsert a:
    // the flush carries exactly one record.
    sink.send(PlaneEvent::Gh(gh_state("herdr-board", vec![pr(42, "abc123", "SUCCESS")]))).await.unwrap();
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
    assert_eq!(b.workspace.pr_number, Some(42), "duplicate gh re-apply left b's PR intact");

    let delta = store.flush().await.expect("batch with b's dirty change");
    assert_eq!(delta.rev, 3);
    assert_eq!(delta.upd.len(), 1, "unchanged duplicate produced no upsert");
    assert_eq!(delta.upd[0].agent_id, "b");
}
