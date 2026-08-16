//! gh_plane integration tests: cadence rule wiring, SWR behavior, mapping,
//! dedupe, and a live (ignored) round-trip harness.
//!
//! The HTTP layer is a mock transport — no test here ever touches the
//! network. The cadence DECISION logic (60s/300s with the production
//! constants) is unit-tested in `gh_plane.rs` on a fake clock; these tests
//! verify the loop wiring end-to-end in real time with shrunk cadence
//! durations and generous margins.

use std::collections::{HashSet, VecDeque};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use corrald::adapters::gh_plane::{GhPlane, GhPlaneConfig, GhTransport, TrackedRepo, TRACKED_REPOS};
use corrald::core::events::{plane_channel, GhRepoState, Plane, PlaneEvent};
use corrald::core::store::Store;
use serde_json::{json, Value};

/// Canned GraphQL success body shaped like GitHub's real response.
fn canned_response() -> Value {
    let mut data = serde_json::Map::new();
    for (i, repo) in TRACKED_REPOS.iter().enumerate() {
        data.insert(format!("q{i}"), repo_json(repo));
    }
    json!({ "data": data })
}

fn repo_json(repo: &TrackedRepo) -> Value {
    json!({
        "name": repo.repo,
        "defaultBranchRef": { "name": "main" },
        "pullRequests": { "nodes": [
            {
                "number": 7,
                "title": "P2 three planes",
                "state": "OPEN",
                "mergeable": "MERGEABLE",
                "headRefOid": "abc123",
                "headRefName": "ws2/gh-plane",
                "closingIssuesReferences": { "nodes": [
                    { "number": 4, "title": "P2 planes" }
                ]},
                "statusCheckRollup": {
                    "state": "SUCCESS",
                    "contexts": { "nodes": [
                        { "__typename": "CheckRun", "status": "COMPLETED", "conclusion": "SUCCESS" },
                        { "__typename": "StatusContext", "state": "SUCCESS" }
                    ]}
                }
            }
        ]},
        "issues": { "nodes": [
            { "number": 4, "state": "OPEN", "title": "P2 planes" }
        ]}
    })
}

/// Serves canned bodies in order; the last one repeats forever.
struct MockTransport {
    calls: AtomicUsize,
    times: Mutex<Vec<std::time::Instant>>,
    responses: Mutex<VecDeque<Value>>,
    fallback: Value,
}

impl MockTransport {
    fn new(responses: Vec<Value>) -> Self {
        let fallback = responses.last().cloned().unwrap_or(Value::Null);
        Self {
            calls: AtomicUsize::new(0),
            times: Mutex::new(Vec::new()),
            responses: Mutex::new(responses.into()),
            fallback,
        }
    }

    fn call_count(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }

    fn call_times(&self) -> Vec<std::time::Instant> {
        self.times.lock().unwrap().clone()
    }
}

impl GhTransport for MockTransport {
    fn post<'a>(
        &'a self,
        _url: &'a str,
        token: &'a str,
        _body: Value,
    ) -> corrald::adapters::gh_plane::BoxFuture<'a, Result<Value, corrald::adapters::gh_plane::GhError>>
    {
        Box::pin(async move {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.times.lock().unwrap().push(std::time::Instant::now());
            assert!(!token.is_empty(), "every request must carry a token");
            let next = self
                .responses
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_else(|| self.fallback.clone());
            Ok(next)
        })
    }
}

fn fast_config() -> GhPlaneConfig {
    GhPlaneConfig {
        foreground: Duration::from_millis(150),
        background: Duration::from_millis(600),
        wake: Duration::from_millis(10),
        failure_backoff: Duration::from_millis(30),
    }
}

async fn wait_until(what: &str, timeout: Duration, mut pred: impl FnMut() -> bool) {
    let start = std::time::Instant::now();
    while start.elapsed() < timeout {
        if pred() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(pred(), "timed out waiting for {what}");
}

fn drain_gh_events(rx: &mut tokio::sync::mpsc::Receiver<PlaneEvent>) -> Vec<GhRepoState> {
    let mut states = Vec::new();
    while let Ok(ev) = rx.try_recv() {
        if let PlaneEvent::Gh(state) = ev {
            states.push(state);
        }
    }
    states
}

/// Acceptance criterion 2, leg 1: ZERO polling while no SSE client has ever
/// connected this run (SWR-only). The plane wakes on the in-process timer
/// but must never issue a network call.
#[tokio::test]
async fn zero_subscribers_never_polls() {
    let store = Arc::new(Store::new());
    let mock = Arc::new(MockTransport::new(vec![canned_response()]));
    let plane = Arc::new(GhPlane::with_config(
        store,
        mock.clone(),
        Some("test-token".to_string()),
        fast_config(),
    ));
    let (sink, _rx) = plane_channel();
    plane.start(sink);

    tokio::time::sleep(Duration::from_millis(400)).await;
    assert_eq!(mock.call_count(), 0, "no SSE subscriber ever -> zero polls");
}

/// SWR fetch: the first-ever subscriber triggers an immediate poll (not one
/// at the end of a cadence window), and while the subscriber stays the
/// foreground cadence holds.
#[tokio::test]
async fn first_subscriber_triggers_immediate_fetch_then_foreground_cadence() {
    let store = Arc::new(Store::new());
    let mock = Arc::new(MockTransport::new(vec![canned_response()]));
    let plane = Arc::new(GhPlane::with_config(
        store.clone(),
        mock.clone(),
        Some("test-token".to_string()),
        fast_config(),
    ));
    let (sink, _rx) = plane_channel();
    plane.start(sink);

    tokio::time::sleep(Duration::from_millis(120)).await;
    assert_eq!(mock.call_count(), 0, "SWR: nothing before the first client");

    let _subscriber = store.subscribe();
    wait_until("first subscriber fetch", Duration::from_secs(1), || {
        mock.call_count() >= 1
    })
    .await;

    tokio::time::sleep(Duration::from_millis(60)).await;
    assert_eq!(
        mock.call_count(),
        1,
        "no poll inside the foreground window (150ms in this config)"
    );
    wait_until("foreground cadence poll", Duration::from_secs(1), || {
        mock.call_count() >= 2
    })
    .await;
}

/// After the last subscriber disconnects the cadence falls back to the
/// background rate (300s in production) — the poll already scheduled under
/// the foreground cadence still fires, the one after it waits the background
/// window.
#[tokio::test]
async fn background_cadence_after_all_subscribers_disconnect() {
    let store = Arc::new(Store::new());
    let mock = Arc::new(MockTransport::new(vec![canned_response()]));
    let plane = Arc::new(GhPlane::with_config(
        store.clone(),
        mock.clone(),
        Some("test-token".to_string()),
        fast_config(),
    ));
    let (sink, _rx) = plane_channel();
    plane.start(sink);

    let subscriber = store.subscribe();
    wait_until("first subscriber fetch", Duration::from_secs(1), || {
        mock.call_count() >= 1
    })
    .await;
    drop(subscriber);

    wait_until("foreground poll already scheduled fires", Duration::from_secs(1), || {
        mock.call_count() >= 2
    })
    .await;

    tokio::time::sleep(Duration::from_millis(250)).await;
    assert_eq!(
        mock.call_count(),
        2,
        "no poll inside the background window (600ms in this config)"
    );
    wait_until("background cadence poll", Duration::from_secs(2), || {
        mock.call_count() >= 3
    })
    .await;
}

/// F2 regression: a subscriber reconnecting during a long background sleep
/// must trigger the immediate SWR fetch instead of waiting out the stale
/// background deadline (previously up to ~300s of stale data).
#[tokio::test]
async fn reconnect_during_background_sleep_triggers_immediate_fetch() {
    let store = Arc::new(Store::new());
    let mock = Arc::new(MockTransport::new(vec![canned_response()]));
    let plane = Arc::new(GhPlane::with_config(
        store.clone(),
        mock.clone(),
        Some("test-token".to_string()),
        fast_config(),
    ));
    let (sink, _rx) = plane_channel();
    plane.start(sink);

    // Join -> immediate fetch; drop -> the scheduled foreground poll fires,
    // then the plane is on the 600ms background sleep until T2+600.
    let subscriber = store.subscribe();
    wait_until("first fetch", Duration::from_secs(1), || mock.call_count() >= 1).await;
    drop(subscriber);
    wait_until("scheduled poll after disconnect", Duration::from_secs(1), || {
        mock.call_count() >= 2
    })
    .await;

    // Reconnect ~200ms into the 600ms background sleep: the fetch must come
    // back immediately (well before the background deadline at +600ms).
    tokio::time::sleep(Duration::from_millis(200)).await;
    let _subscriber = store.subscribe();
    wait_until(
        "immediate SWR fetch on reconnect",
        Duration::from_millis(500),
        || mock.call_count() >= 3,
    )
    .await;
    assert!(
        mock.call_times()[2].duration_since(mock.call_times()[1]) < Duration::from_millis(500),
        "reconnect fetch must not wait out the background deadline"
    );
}

/// F6 regression: sustained poll failures back off exponentially (growing
/// gaps) without killing the plane, and a success recovers normal emission.
#[tokio::test]
async fn sustained_failures_back_off_then_recover() {
    let store = Arc::new(Store::new());
    let _subscriber = store.subscribe();

    let mut changed = canned_response();
    {
        let data = changed["data"].as_object_mut().unwrap();
        let herdr_board = data.get_mut("q5").expect("herdr-board alias").as_object_mut().unwrap();
        herdr_board.insert(
            "pullRequests".to_string(),
            json!({ "nodes": [
                {
                    "number": 9,
                    "title": "NEW: failing check",
                    "state": "OPEN",
                    "mergeable": "CONFLICTING",
                    "headRefOid": "def456",
                    "statusCheckRollup": {
                        "state": "FAILURE",
                        "contexts": { "nodes": [
                            { "__typename": "CheckRun", "status": "COMPLETED", "conclusion": "FAILURE" }
                        ]}
                    }
                }
            ]}),
        );
    }
    let failing = json!({ "data": null, "errors": [{ "message": "boom" }] });
    let mock = Arc::new(MockTransport::new(vec![canned_response(), failing.clone(), failing, changed]));
    let plane = Arc::new(GhPlane::with_config(
        store.clone(),
        mock.clone(),
        Some("test-token".to_string()),
        fast_config(),
    ));
    let (sink, mut rx) = plane_channel();
    plane.start(sink);

    wait_until("initial poll", Duration::from_secs(1), || mock.call_count() >= 1).await;
    assert_eq!(drain_gh_events(&mut rx).len(), TRACKED_REPOS.len(), "initial poll emits all repos");
    wait_until("first failure", Duration::from_secs(2), || mock.call_count() >= 2).await;
    wait_until("backoff retry", Duration::from_secs(2), || mock.call_count() >= 3).await;
    wait_until("recovery poll", Duration::from_secs(2), || mock.call_count() >= 4).await;

    // Backoff grows: the third call waits longer after the second than the
    // second did after the first (foreground cadence 150ms -> backoff 30ms).
    let times = mock.call_times();
    let gap12 = times[1].duration_since(times[0]);
    let gap23 = times[2].duration_since(times[1]);
    let gap34 = times[3].duration_since(times[2]);
    eprintln!("gaps: 12={gap12:?} 23={gap23:?} 34={gap34:?}");
    assert!(gap12 >= Duration::from_millis(100), "cadence-gap poll: {gap12:?}");
    assert!(gap23 < gap12, "backoff gap ({gap23:?}) must be below the cadence gap ({gap12:?})");
    assert!(gap34 >= gap23, "backoff grows across failures: {gap34:?} >= {gap23:?}");

    // The recovery poll emits only the changed repo.
    let ev = tokio::time::timeout(Duration::from_secs(1), rx.recv())
        .await
        .expect("recovery event")
        .expect("sink alive");
    let PlaneEvent::Gh(state) = ev else {
        panic!("expected Gh event");
    };
    assert_eq!(state.repo, "herdr-board");
    assert_eq!(state.prs.len(), 1);
    assert_eq!(state.prs[0].ci_status, "FAILURE");
}

/// One round-trip maps all 8 repos into contract types; unchanged state is
/// deduped (no sink spam); a change emits only the changed repo.
#[tokio::test]
async fn maps_all_repos_and_emits_only_changes() {
    let store = Arc::new(Store::new());
    let _subscriber = store.subscribe(); // go live immediately

    let mut changed = canned_response();
    {
        let data = changed["data"].as_object_mut().unwrap();
        let herdr_board = data.get_mut("q5").expect("herdr-board alias").as_object_mut().unwrap();
        herdr_board.insert(
            "pullRequests".to_string(),
            json!({ "nodes": [
                {
                    "number": 7,
                    "title": "P2 three planes",
                    "state": "OPEN",
                    "mergeable": "MERGEABLE",
                    "headRefOid": "abc123",
                    "statusCheckRollup": {
                        "state": "SUCCESS",
                        "contexts": { "nodes": [
                            { "__typename": "CheckRun", "status": "COMPLETED", "conclusion": "SUCCESS" }
                        ]}
                    }
                },
                {
                    "number": 9,
                    "title": "NEW: failing check",
                    "state": "OPEN",
                    "mergeable": "CONFLICTING",
                    "headRefOid": "def456",
                    "statusCheckRollup": {
                        "state": "FAILURE",
                        "contexts": { "nodes": [
                            { "__typename": "CheckRun", "status": "COMPLETED", "conclusion": "FAILURE" }
                        ]}
                    }
                }
            ]}),
        );
    }
    let mock = Arc::new(MockTransport::new(vec![canned_response(), canned_response(), changed]));
    let plane = Arc::new(GhPlane::with_config(
        store.clone(),
        mock.clone(),
        Some("test-token".to_string()),
        fast_config(),
    ));
    let (sink, mut rx) = plane_channel();
    plane.start(sink);

    // Poll 1 (immediate, subscriber already present): all 8 repos emitted.
    wait_until("initial poll", Duration::from_secs(1), || mock.call_count() >= 1).await;
    let states = drain_gh_events(&mut rx);
    assert_eq!(states.len(), TRACKED_REPOS.len(), "first poll emits every repo");
    for (i, state) in states.iter().enumerate() {
        assert_eq!(state.repo, TRACKED_REPOS[i].name);
        assert_eq!(state.default_branch, "main");
        assert_eq!(state.ahead, 0, "ahead/behind are local tracking info (WS1), 0 from gh");
        assert_eq!(state.behind, 0);
        assert_eq!(state.prs.len(), 1);
        assert_eq!(state.prs[0].pr_number, 7);
        assert_eq!(state.prs[0].ci_status, "SUCCESS");
        assert_eq!(state.prs[0].head_sha, "abc123");
        assert_eq!(state.prs[0].head_branch, "ws2/gh-plane", "#22 fragment field mapped");
        // #23: the closing ref's state is enriched from the SAME poll's
        // repo-level issues fetch (issue 4 is among the recent ones).
        assert_eq!(state.prs[0].closing_issues.len(), 1);
        assert_eq!(state.prs[0].closing_issues[0].number, 4);
        assert_eq!(state.prs[0].closing_issues[0].state, "OPEN");
        assert_eq!(state.prs[0].closing_issues[0].title, "P2 planes");
        assert_eq!(state.issues.len(), 1);
        assert_eq!(state.issues[0].number, 4);
        assert_eq!(state.issues[0].state, "OPEN");
        assert_eq!(state.issues[0].title, "P2 planes");
    }

    // Poll 2 (same payload): dedupe — nothing re-emitted into the sink.
    wait_until("second poll", Duration::from_secs(2), || mock.call_count() >= 2).await;
    assert!(
        drain_gh_events(&mut rx).is_empty(),
        "unchanged state must not be re-emitted"
    );

    // Poll 3 (herdr-board gained a PR): exactly one event, for that repo.
    wait_until("third poll", Duration::from_secs(2), || mock.call_count() >= 3).await;
    let ev = tokio::time::timeout(Duration::from_secs(1), rx.recv())
        .await
        .expect("change event within the window")
        .expect("sink alive");
    let PlaneEvent::Gh(state) = ev else {
        panic!("expected Gh event");
    };
    assert_eq!(state.repo, "herdr-board");
    assert_eq!(state.prs.len(), 2);
    assert_eq!(state.prs[0].pr_number, 7, "PRs sorted by number (F7)");
    assert_eq!(state.prs[1].ci_status, "FAILURE");
    assert_eq!(state.prs[1].mergeable, "CONFLICTING");
    assert!(
        drain_gh_events(&mut rx).is_empty(),
        "only the changed repo is emitted"
    );
}

/// Failed polls emit nothing and keep the last-known state for the next
/// successful poll.
#[tokio::test]
async fn failed_poll_emits_nothing() {
    let store = Arc::new(Store::new());
    let _subscriber = store.subscribe();
    let failing = json!({ "data": null, "errors": [{ "message": "boom" }] });
    let mock = Arc::new(MockTransport::new(vec![canned_response(), failing, canned_response()]));
    let plane = Arc::new(GhPlane::with_config(
        store.clone(),
        mock.clone(),
        Some("test-token".to_string()),
        fast_config(),
    ));
    let (sink, mut rx) = plane_channel();
    plane.start(sink);

    wait_until("initial poll", Duration::from_secs(1), || mock.call_count() >= 1).await;
    assert_eq!(drain_gh_events(&mut rx).len(), TRACKED_REPOS.len());

    wait_until("failing poll", Duration::from_secs(2), || mock.call_count() >= 2).await;
    assert!(
        drain_gh_events(&mut rx).is_empty(),
        "failed poll emits nothing"
    );

    wait_until("recovered poll", Duration::from_secs(2), || mock.call_count() >= 3).await;
    assert!(
        drain_gh_events(&mut rx).is_empty(),
        "recovered poll unchanged vs last success -> still deduped"
    );
}

/// A repo that 404s mid-query (alias null) is skipped without killing the
/// other repos or the poll.
#[tokio::test]
async fn one_bad_repo_does_not_poison_the_round_trip() {
    let store = Arc::new(Store::new());
    let _subscriber = store.subscribe();
    let mut response = canned_response();
    response["data"].as_object_mut().unwrap().insert("q3".to_string(), Value::Null);
    let mock = Arc::new(MockTransport::new(vec![response]));
    let plane = Arc::new(GhPlane::with_config(
        store.clone(),
        mock.clone(),
        Some("test-token".to_string()),
        fast_config(),
    ));
    let (sink, mut rx) = plane_channel();
    plane.start(sink);

    wait_until("initial poll", Duration::from_secs(1), || mock.call_count() >= 1).await;
    let names: HashSet<String> = drain_gh_events(&mut rx)
        .into_iter()
        .map(|s| s.repo)
        .collect();
    assert_eq!(names.len(), TRACKED_REPOS.len() - 1);
    assert!(!names.contains("dotfiles"), "null alias emits nothing for that repo");
    assert!(names.contains("herdr-board"));
}

// ---------------------------------------------------------------------------
// Live harness (network): run explicitly, never part of `cargo test`.
//   cargo test --test gh_plane -- --ignored --nocapture live_round_trip_all_repos
// ---------------------------------------------------------------------------

fn resolve_token_for_harness() -> Option<String> {
    if let Ok(token) = std::env::var("GITHUB_TOKEN") {
        let token = token.trim().to_string();
        if !token.is_empty() {
            return Some(token);
        }
    }
    let output = std::process::Command::new("gh")
        .args(["auth", "token"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let token = String::from_utf8(output.stdout).ok()?.trim().to_string();
    (!token.is_empty()).then_some(token)
}

#[tokio::test]
#[ignore = "live: requires GITHUB_TOKEN or `gh auth token`; hits the real GitHub API"]
async fn live_round_trip_all_repos() {
    let Some(token) = resolve_token_for_harness() else {
        eprintln!("skipping live test: no GITHUB_TOKEN and no `gh auth token`");
        return;
    };
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .try_init();
    let store = Arc::new(Store::new());
    let _subscriber = store.subscribe(); // go live (first SSE client ever)
    // Token resolution (env/`gh auth token`) happens BEFORE the clock starts
    // (F3): the measured time is the round-trip only — start -> first event.
    let plane = Arc::new(GhPlane::with_token(store, token));
    let (sink, mut rx) = plane_channel();
    let started = std::time::Instant::now();
    plane.start(sink);

    let mut states: Vec<GhRepoState> = Vec::new();
    while states.len() < TRACKED_REPOS.len() {
        match tokio::time::timeout(Duration::from_secs(10), rx.recv()).await {
            Ok(Some(PlaneEvent::Gh(state))) => states.push(state),
            Ok(Some(_)) => {}
            Ok(None) => break,
            Err(_) => panic!("timed out waiting for the gh plane round-trip"),
        }
    }
    let elapsed = started.elapsed();
    println!(
        "=== gh plane live round-trip: {}/{} repos in {:?} ===",
        states.len(),
        TRACKED_REPOS.len(),
        elapsed
    );
    for state in &states {
        println!(
            "  repo={:<28} default={:<10} prs={:<3} issues={:<3} ahead={} behind={}",
            state.repo,
            state.default_branch,
            state.prs.len(),
            state.issues.len(),
            state.ahead,
            state.behind
        );
        for pr in &state.prs {
            println!(
                "      PR #{:<5} [{}] mergeable={:<12} ci={:<9} head={:.8}",
                pr.pr_number, pr.state, pr.mergeable, pr.ci_status, pr.head_sha
            );
        }
    }
    let names: HashSet<&str> = states.iter().map(|s| s.repo.as_str()).collect();
    for repo in TRACKED_REPOS {
        assert!(names.contains(repo.name), "round-trip missing repo {}", repo.name);
    }
    // Budget: criterion 1 targets <2s for the single round-trip; observed
    // 1.3-2.0s with token resolution excluded. Assert at 3s so server-side
    // variance cannot flake the harness while a real regression (e.g. an
    // accidental per-repo round-trip) still fails hard.
    assert!(
        elapsed < Duration::from_secs(3),
        "round-trip budget exceeded: {elapsed:?} (>3s)"
    );
}
