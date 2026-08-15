//! Store contract tests: monotonic rev, coalescing ticks, resume semantics,
//! bounded history.

use corrald::core::model::{Agent, AgentState, Change};
use corrald::core::store::Store;
use corrald::core::model::Resume;

fn agent(id: &str) -> Agent {
    Agent {
        agent_id: id.to_string(),
        source: "herdr".to_string(),
        tool: "opencode".to_string(),
        state: AgentState::Working,
        reason: None,
        seq: 1,
        ts: 0,
        capabilities: vec!["prompt".to_string()],
        waiting_on: None,
        cost: None,
        parent_id: None,
        host: None,
        workspace: Default::default(),
        attachment: None,
        display_name: None,
        title: None,
    }
}

#[tokio::test]
async fn snapshot_flushes_and_bumps_rev_once_per_batch() {
    let store = Store::new();
    assert_eq!(store.snapshot().await.rev, 0);

    store.apply(Change::upsert(agent("a"))).await;
    store.apply(Change::upsert(agent("b"))).await;
    store.apply(Change::upsert(agent("c"))).await;

    let snap = store.snapshot().await;
    assert_eq!(snap.rev, 1, "coalesced batch bumps rev once");
    assert_eq!(snap.agents.len(), 3);
    assert_eq!(snap.schema_version, 1);
}

#[tokio::test]
async fn remove_emits_del_delta() {
    let store = Store::new();
    store.apply(Change::upsert(agent("a"))).await;
    store.apply(Change::Remove("a".to_string())).await;
    let d = store.flush().await.expect("delta");
    assert_eq!(d.rev, 1);
    assert!(d.del.contains(&"a".to_string()));
    assert!(store.snapshot().await.agents.is_empty());
}

#[tokio::test]
async fn background_tick_coalesces_at_2s_when_unwatched() {
    let store = Store::new();
    let c = store.clone();
    std::mem::drop(tokio::spawn(async move { c.run_coalescer().await }));

    let start = std::time::Instant::now();
    for i in 0..5 {
        store.apply(Change::upsert(agent(&format!("a{i}")))).await;
    }
    // No subscribers -> 2s background tick.
    tokio::time::sleep(std::time::Duration::from_millis(2400)).await;
    let snap = store.snapshot().await;
    assert_eq!(snap.rev, 1, "5 changes coalesced into one background batch");
    assert_eq!(snap.agents.len(), 5);
    assert!(start.elapsed() >= std::time::Duration::from_secs(2) - std::time::Duration::from_millis(50));
}

#[tokio::test]
async fn foreground_tick_coalesces_at_250ms_when_watched() {
    let store = Store::new();
    let c = store.clone();
    std::mem::drop(tokio::spawn(async move { c.run_coalescer().await }));
    let _rx = store.subscribe(); // a subscriber -> foreground tick
    let _ = tokio::time::sleep(std::time::Duration::from_millis(50)).await; // let task start

    let start = std::time::Instant::now();
    for i in 0..3 {
        store.apply(Change::upsert(agent(&format!("w{i}")))).await;
    }
    tokio::time::sleep(std::time::Duration::from_millis(600)).await;
    let snap = store.snapshot().await;
    assert_eq!(snap.rev, 1);
    assert!(start.elapsed() < std::time::Duration::from_secs(2), "foreground tick must be fast");
}

#[tokio::test]
async fn resume_fresh_cursor_replays_deltas_only() {
    let store = Store::new();
    for i in 1..=5 {
        store.apply(Change::upsert(agent(&format!("a{i}")))).await;
        store.flush().await;
    }
    assert_eq!(store.snapshot().await.rev, 5);

    match store.resume_from(Some(2)).await {
        Resume::Deltas { deltas, live_from_rev } => {
            assert_eq!(live_from_rev, 5);
            let revs: Vec<u64> = deltas.iter().map(|d| d.rev).collect();
            assert_eq!(revs, vec![3, 4, 5]);
            assert!(deltas[0].upd.iter().any(|a| a.agent_id == "a3"));
        }
        other => panic!("expected Deltas, got {other:?}"),
    }
}

#[tokio::test]
async fn resume_too_old_returns_snapshot() {
    let store = Store::new();
    for i in 1..=3 {
        store.apply(Change::upsert(agent(&format!("a{i}")))).await;
        store.flush().await;
    }
    assert!(matches!(
        store.resume_from(Some(0)).await,
        Resume::Snapshot(_)
    ));
    assert!(matches!(store.resume_from(None).await, Resume::Snapshot(_)));
}

#[tokio::test]
async fn resume_current_rev_goes_live() {
    let store = Store::new();
    store.apply(Change::upsert(agent("a"))).await;
    store.flush().await;
    assert!(matches!(store.resume_from(Some(1)).await, Resume::Live { rev: 1 }));
    // Cursor beyond current (client restarted elsewhere) — also live.
    assert!(matches!(store.resume_from(Some(99)).await, Resume::Live { rev: 1 }));
}

#[tokio::test]
async fn history_is_bounded_and_recovers_with_snapshot() {
    let store = Store::new();
    for i in 0..1100u64 {
        store.apply(Change::upsert(agent(&format!("a{i}")))).await;
        store.flush().await;
    }
    assert_eq!(store.snapshot().await.rev, 1100);
    // Cursor older than the retained ring -> full snapshot.
    assert!(matches!(
        store.resume_from(Some(10)).await,
        Resume::Snapshot(_)
    ));
    // Fresh cursor still replays.
    assert!(matches!(
        store.resume_from(Some(1099)).await,
        Resume::Deltas { .. }
    ));
}

#[tokio::test]
async fn monotonic_rev_never_repeats() {
    let store = Store::new();
    let mut last = 0u64;
    for i in 0..20u64 {
        store.apply(Change::upsert(agent(&format!("a{i}")))).await;
        let d = store.flush().await.unwrap();
        assert!(d.rev > last);
        last = d.rev;
    }
}
