//! #555: real Store publication + real router, with the old axum encoders as
//! independent byte oracles. No cached JSON is deserialized as the state oracle.
use super::*;
use crate::api::{AppState, router};
use crate::core::model::{AgentState, Attachment, Workspace};
use axum::body::{Body, Bytes};
use axum::http::{Request, header};
use axum::response::sse::{Event, Sse};
use axum::response::{IntoResponse, Json};
use futures::stream::{self, StreamExt};
use http_body_util::BodyExt;
use tower::ServiceExt;

fn random(seed: &mut u64) -> u64 {
    // Reproducible test data, not cryptographic randomness.
    *seed ^= *seed << 13;
    *seed ^= *seed >> 7;
    *seed ^= *seed << 17;
    *seed
}

fn agent(seed: &mut u64, id: usize) -> Agent {
    let n = random(seed);
    Agent {
        agent_id: format!("fixture-{id}-{:x}", random(seed)),
        source: "herdr".into(),
        tool: "fixture".into(),
        state: [
            AgentState::Idle,
            AgentState::Working,
            AgentState::Blocked,
            AgentState::Done,
            AgentState::Unknown,
        ][n as usize % 5],
        reason: (n & 1 != 0).then(|| format!("Thai ไทย 🐎 \" \\ \r\n\0 {n}")),
        seq: random(seed),
        ts: random(seed),
        capabilities: vec!["read_tail".into(), "read_diff".into()],
        waiting_on: None,
        parent_id: (n & 2 != 0).then(|| "parent".into()),
        host: (n & 4 != 0).then(|| "fixture-host".into()),
        workspace: Workspace {
            repo: Some(format!("repo-{n}")),
            branch: (n & 8 != 0).then(|| "branch\nquoted\"".into()),
            dirty: n & 16 != 0,
            ahead: random(seed),
            behind: random(seed),
            ..Default::default()
        },
        attachment: (n & 32 != 0).then(|| Attachment {
            kind: "fixture".into(),
            reference: "%42".into(),
        }),
        display_name: Some(format!("agent {n}")),
        title: (n & 64 != 0).then(|| "line1\nline2".into()),
    }
}

fn old_wire_value(epoch: &str, data: &impl serde::Serialize) -> serde_json::Value {
    let mut value = serde_json::to_value(data).unwrap();
    value
        .as_object_mut()
        .unwrap()
        .insert("epoch".into(), serde_json::json!(epoch));
    value
}

async fn old_event(kind: &str, rev: u64, epoch: &str, data: &impl serde::Serialize) -> Bytes {
    let event = Event::default()
        .event(kind)
        .id(rev.to_string())
        .json_data(old_wire_value(epoch, data))
        .unwrap();
    Sse::new(stream::iter([Ok::<_, std::convert::Infallible>(event)]))
        .into_response()
        .into_body()
        .collect()
        .await
        .unwrap()
        .to_bytes()
}

#[tokio::test]
async fn randomized_publications_match_old_snapshot_and_sse_bytes() {
    let mut seed = 0x555_450_492_454_u64;
    let mut epochs = std::collections::BTreeSet::new();
    let mut revisions = std::collections::BTreeSet::new();
    for case in 0..64 {
        let store = Store::new(); // fresh OS-random epoch per state
        if case != 0 {
            store.inner.lock().await.rev = random(&mut seed) % 1_000_000;
            for id in 0..(random(&mut seed) % 64 + 1) {
                store
                    .apply(Change::upsert(agent(&mut seed, id as usize)))
                    .await;
            }
            store
                .git_plane_backlog()
                .store(random(&mut seed) & 1 != 0, Ordering::Release);
            store.flush().await;
        }
        let publication = store.published();
        let mut fresh = {
            let inner = store.inner.lock().await;
            store.snapshot_locked(&inner)
        };
        // generated_at describes assembly time, now fixed at publication.
        // Hold that same state field equal when comparing fresh encodings.
        fresh.generated_at = serde_json::from_slice::<serde_json::Value>(&publication.body)
            .unwrap()["generated_at"]
            .as_u64()
            .unwrap();
        let epoch = store.epoch();
        epochs.insert(epoch.to_string());
        revisions.insert(fresh.rev);
        let expected_body = Json(old_wire_value(&epoch, &fresh))
            .into_response()
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes();
        let expected_event = old_event("snapshot", fresh.rev, &epoch, &fresh).await;
        assert_eq!(
            publication.body.as_ref(),
            &expected_body,
            "snapshot case {case}"
        );
        assert_eq!(
            publication.event.as_ref(),
            &expected_event,
            "SSE case {case}"
        );
        let app = router(AppState {
            store: store.clone(),
            ..Default::default()
        });
        let response = app
            .clone()
            .oneshot(Request::get("/snapshot").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.headers()[header::CONTENT_TYPE], "application/json");
        assert_eq!(
            response.into_body().collect().await.unwrap().to_bytes(),
            expected_body
        );
        let request = match case % 4 {
            0 => Request::get("/events"),
            // Legacy-epoch case: old cursor, NO Corral-Epoch header.
            1 => Request::get("/events").header("Last-Event-ID", "0"),
            2 => Request::get("/events")
                .header("Last-Event-ID", fresh.rev.to_string())
                .header("Corral-Epoch", "unknown"),
            _ => Request::get("/events")
                .header("Last-Event-ID", (fresh.rev + 1).to_string())
                .header("Corral-Epoch", epoch.as_ref()),
        };
        let response = app
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            "text/event-stream"
        );
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-cache");
        let mut body = response.into_body().into_data_stream();
        let frame = tokio::time::timeout(Duration::from_millis(100), body.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(frame, expected_event, "router SSE case {case}");
    }
    assert_eq!(epochs.len(), 64);
    assert!(revisions.len() >= 32);
    println!(
        "G555_BYTE_EQUALITY states=64 snapshots=64 sse_frames=64 empty=1 legacy_header_cases=16 epochs={} revisions={}",
        epochs.len(),
        revisions.len()
    );
}

#[tokio::test]
async fn reads_and_initial_sse_do_not_wait_for_writer_or_publisher() {
    let store = Store::new();
    let mut seed = 555;
    for id in 0..2 {
        store.apply(Change::upsert(agent(&mut seed, id))).await;
        store.flush().await;
    }
    let app = router(AppState {
        store: store.clone(),
        ..Default::default()
    });
    let before = store.published();
    let _writer = store.inner.lock().await;
    let _publisher = store.publish_lock.lock().await;
    for request in [
        Request::get("/snapshot"),
        Request::get("/host-key"),
        Request::get("/events"),
        Request::get("/events")
            .header("Last-Event-ID", "1")
            .header("Corral-Epoch", store.epoch().as_ref()),
        Request::get("/events")
            .header("Last-Event-ID", "2")
            .header("Corral-Epoch", "unknown"),
    ] {
        let response = tokio::time::timeout(Duration::from_millis(50), async {
            let response = app
                .clone()
                .oneshot(request.body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), 200);
            response
                .into_body()
                .into_data_stream()
                .next()
                .await
                .unwrap()
                .unwrap()
        })
        .await
        .expect("read queued behind writer/publisher");
        assert!(!response.is_empty());
    }
    assert!(
        Arc::ptr_eq(&before, &store.published()),
        "reads must not rebuild"
    );
}

#[tokio::test]
async fn publication_is_atomic_and_retained_buffers_stay_unchanged() {
    let store = Store::new();
    let mut seed = 555;
    let mut publications = store.publications();
    let old = store.published();
    let old_bytes = old.body.clone();
    let mut row = agent(&mut seed, 1);
    store.apply(Change::upsert(row.clone())).await;
    assert!(Arc::ptr_eq(&old, &store.published()));
    store.flush().await;
    publications.changed().await.unwrap();
    let first = store.published();
    assert_eq!(first.rev, 1);
    row.seq += 1;
    store.update_where(|_| true, |a| a.seq = row.seq).await;
    store.flush().await;
    assert_eq!(store.published().rev, 2);
    assert!(store.remove_if(&row.agent_id, || true).await);
    store.flush().await;
    let current = store.published();
    assert_eq!(current.rev, 3);
    assert!(
        serde_json::from_slice::<serde_json::Value>(&current.body).unwrap()["agents"]
            .as_object()
            .unwrap()
            .is_empty()
    );
    assert_eq!(old.body, old_bytes);
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&first.body).unwrap()["agents"][&row.agent_id]
            ["seq"],
        row.seq - 1
    );
    assert!(store.flush().await.is_none());
    assert!(
        Arc::ptr_eq(&current, &store.published()),
        "unchanged state must not re-encode"
    );
}

#[tokio::test]
async fn cached_resume_matches_epoch_and_history_rules() {
    let store = Store::new();
    let mut seed = 555;
    let row = agent(&mut seed, 1);
    for _ in 0..(HISTORY_CAP + 2) {
        store.apply(Change::upsert(row.clone())).await;
        store.flush().await;
    }
    let current = store.published();
    for epoch in [None, Some(store.epoch()), Some(Arc::from("unknown"))] {
        for cursor in [
            None,
            Some(0),
            Some(2),
            Some(3),
            Some(current.rev - 1),
            Some(current.rev),
            Some(current.rev + 1),
        ] {
            let expected = match store.resume_from_epoch(cursor, epoch.as_deref()).await {
                Resume::Snapshot(_) => vec![current.event.as_ref().clone()],
                Resume::Live { rev } => {
                    assert_eq!(rev, current.rev);
                    Vec::new()
                }
                Resume::Deltas {
                    deltas,
                    live_from_rev,
                } => {
                    assert_eq!(live_from_rev, current.rev);
                    let mut frames = Vec::new();
                    for d in deltas {
                        frames.push(old_event("delta", d.rev, &store.epoch(), &d).await);
                    }
                    frames
                }
            };
            let actual: Vec<_> = current
                .resume(
                    cursor,
                    epoch.as_deref().is_none_or(|e| e == store.epoch().as_ref()),
                )
                .into_iter()
                .map(|v| v.as_ref().clone())
                .collect();
            assert_eq!(actual, expected, "cursor={cursor:?} epoch={epoch:?}");
        }
    }
}

#[tokio::test]
async fn sse_subscribe_boundary_and_lag_recovery_never_replay_older_deltas() {
    let store = Store::new();
    let mut seed = 555;
    let row = agent(&mut seed, 1);
    store.apply(Change::upsert(row.clone())).await;
    store.flush().await;
    let initial = store.published();
    let app = router(AppState {
        store: store.clone(),
        ..Default::default()
    });
    let response = app
        .oneshot(Request::get("/events").body(Body::empty()).unwrap())
        .await
        .unwrap();
    let mut body = response.into_body().into_data_stream();
    store.apply(Change::upsert(row.clone())).await;
    store.flush().await; // happens after response creation but before first poll
    assert_eq!(body.next().await.unwrap().unwrap(), *initial.event);
    assert_eq!(
        body.next().await.unwrap().unwrap(),
        *store.published().delta(2).unwrap()
    );
    for _ in 0..(BROADCAST_CAP + 2) {
        store.apply(Change::upsert(row.clone())).await;
        store.flush().await;
    }
    let recovered = store.published();
    assert_eq!(body.next().await.unwrap().unwrap(), *recovered.event);
    store.apply(Change::Remove(row.agent_id)).await;
    store.flush().await;
    let next = store.published();
    assert_eq!(
        tokio::time::timeout(Duration::from_millis(100), body.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap(),
        *next.delta(next.rev).unwrap()
    );
}
