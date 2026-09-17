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

#[tokio::test]
async fn g560_unchanged_ticks_keep_publication_identity() {
    let mut counts = Vec::new();
    for enabled in [false, true] {
        let store = Store::new();
        let mut seed = 560;
        for id in 0..200 {
            store.apply(Change::upsert(agent(&mut seed, id))).await;
        }
        store
            .git_plane_health
            .enabled
            .store(enabled, Ordering::Release);
        store.flush().await;
        let mut previous = store.published();
        let mut count = 0;
        for _ in 0..10 {
            tokio::time::sleep(FOREGROUND_TICK).await;
            store.flush().await;
            let next = store.published();
            count += usize::from(Arc::as_ptr(&previous) != Arc::as_ptr(&next));
            previous = next;
        }
        println!("G560_IDENTITY enabled={enabled} agents=200 republications={count}/10");
        counts.push(count);
    }
    assert_eq!(
        counts,
        vec![0, 0],
        "unchanged health-enabled ticks must not republish"
    );
}

#[tokio::test]
async fn g560_dead_plane_ages_reuse_agent_encoding() {
    let store = Store::new();
    let mut seed = 560;
    for id in 0..200 {
        let mut row = agent(&mut seed, id);
        row.workspace.worktree_path = Some("/g560-stopped".into());
        store.apply(Change::upsert(row)).await;
    }
    let health = store.git_plane_health();
    health.enabled.store(true, Ordering::Release);
    health.progress();
    health.facts.lock().unwrap().insert(
        PathBuf::from("/g560-stopped"),
        Some(Instant::now() - Duration::from_secs(10)),
    );
    store.flush().await;
    let initial = store.published();
    let retained = initial.body.clone();
    let mut previous = initial.clone();
    let mut publications = store.publications();
    let coalescer = tokio::spawn({
        let store = store.clone();
        async move { store.run_coalescer().await }
    });
    let mut republications = 0;
    let mut encodings = 0;
    for tick in 0..3 {
        let started = Instant::now();
        if tick == 1 {
            // A wake near the end of the idle window cannot restart its clock.
            tokio::time::sleep(Duration::from_millis(1250)).await;
            store.notify.notify_one();
        }
        tokio::time::timeout(
            Duration::from_secs(3).saturating_sub(started.elapsed()),
            publications.changed(),
        )
        .await
        .unwrap()
        .unwrap();
        let next = publications.borrow_and_update().clone();
        let old: serde_json::Value = serde_json::from_slice(&previous.body).unwrap();
        let new: serde_json::Value = serde_json::from_slice(&next.body).unwrap();
        assert!(
            new["git_worktree_facts"]["/g560-stopped"]["fact_age_ms"]
                .as_u64()
                .unwrap()
                > old["git_worktree_facts"]["/g560-stopped"]["fact_age_ms"]
                    .as_u64()
                    .unwrap()
        );
        assert!(
            new["git_plane_last_event_age_ms"].as_u64().unwrap()
                > old["git_plane_last_event_age_ms"].as_u64().unwrap()
        );
        assert_eq!(new["git_plane_alive"], false);
        assert_eq!(new["git_worktree_facts"]["/g560-stopped"]["stale"], true);
        assert_eq!(
            next.rev, initial.rev,
            "age refresh cannot fabricate a delta"
        );
        assert!(Arc::ptr_eq(&next.agents, &initial.agents));
        republications += usize::from(Arc::as_ptr(&previous) != Arc::as_ptr(&next));
        encodings += next.agent_encodings;
        println!(
            "G560_DEAD_TICK wait_ms={} agent_encodings={} age_ms={}",
            started.elapsed().as_millis(),
            next.agent_encodings,
            new["git_worktree_facts"]["/g560-stopped"]["fact_age_ms"]
        );
        previous = next;
    }
    coalescer.abort();
    let _ = coalescer.await;
    assert_eq!(republications, 3);
    assert_eq!(
        encodings, 0,
        "dead-plane age refresh re-encoded the agent map"
    );
    assert_eq!(initial.body, retained, "retained publication mutated");
    println!(
        "G560_DEAD agents=200 republications={republications}/3 agent_encodings={encodings}/3"
    );
}

#[tokio::test]
async fn g560_real_change_reaches_http_and_sse_within_bound() {
    let store = Store::new();
    let mut seed = 560;
    let mut row = agent(&mut seed, 0);
    store.apply(Change::upsert(row.clone())).await;
    store
        .git_plane_health
        .enabled
        .store(true, Ordering::Release);
    store.flush().await;
    let initial = store.published();
    let mut publications = store.publications();
    let app = router(AppState {
        store: store.clone(),
        ..Default::default()
    });
    let coalescer = tokio::spawn({
        let store = store.clone();
        async move { store.run_coalescer().await }
    });
    row.title = Some("background change".into());
    let started = Instant::now();
    store.apply(Change::upsert(row.clone())).await;
    tokio::time::timeout(Duration::from_secs(3), publications.changed())
        .await
        .unwrap()
        .unwrap();
    let background = publications.borrow_and_update().clone();
    println!(
        "G560_CHANGE background_ms={}",
        started.elapsed().as_millis()
    );
    assert_eq!(background.agent_encodings, 1);
    assert!(!Arc::ptr_eq(&background.agents, &initial.agents));
    let response = app
        .clone()
        .oneshot(Request::get("/snapshot").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(
        response.into_body().collect().await.unwrap().to_bytes(),
        *background.body
    );
    let response = app
        .oneshot(Request::get("/events").body(Body::empty()).unwrap())
        .await
        .unwrap();
    let mut stream = response.into_body().into_data_stream();
    assert_eq!(stream.next().await.unwrap().unwrap(), *background.event);
    row.title = Some("foreground change".into());
    let started = Instant::now();
    store.apply(Change::upsert(row.clone())).await;
    let frame = tokio::time::timeout(Duration::from_secs(3), stream.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    println!(
        "G560_CHANGE foreground_ms={}",
        started.elapsed().as_millis()
    );
    let current = store.published();
    assert_eq!(frame, *current.delta(current.rev).unwrap());
    let wire: serde_json::Value = serde_json::from_slice(&current.body).unwrap();
    assert_eq!(wire["agents"][&row.agent_id]["title"], "foreground change");
    coalescer.abort();
    let _ = coalescer.await;
}

#[tokio::test]
async fn g560_agent_cache_invalidates_on_content_and_stale_projection() {
    let store = Store::new();
    let mut seed = 560;
    let mut row = agent(&mut seed, 0);
    row.workspace.worktree_path = Some("/g560-fresh".into());
    row.workspace.branch = Some("main".into());
    row.workspace.pr_number = Some(560);
    let health = store.git_plane_health();
    health.enabled.store(true, Ordering::Release);
    health.alive.store(true, Ordering::Release);
    health
        .facts
        .lock()
        .unwrap()
        .insert(PathBuf::from("/g560-fresh"), Some(Instant::now()));
    store.apply(Change::upsert(row.clone())).await;
    store.flush().await;
    assert_eq!(store.published().agent_encodings, 1);
    // Includes a no-op upsert: revision semantics survive, encoding is reused.
    let before = store.published();
    store.apply(Change::upsert(row.clone())).await;
    store.flush().await;
    assert!(Arc::ptr_eq(&before.agents, &store.published().agents));
    assert_eq!(store.published().agent_encodings, 0);
    for (label, alive, aged, expected_pr) in [
        ("expired", true, true, None),
        ("fresh-again", true, false, Some(560)),
        ("stopped", false, false, None),
        ("recovered", true, false, Some(560)),
    ] {
        let before = store.published();
        health.alive.store(alive, Ordering::Release);
        let observed = Instant::now() - Duration::from_secs(if aged { 121 } else { 0 });
        health
            .facts
            .lock()
            .unwrap()
            .insert(PathBuf::from("/g560-fresh"), Some(observed));
        let delta = store
            .flush()
            .await
            .expect("projection transition reaches SSE");
        let next = store.published();
        assert_eq!(delta.upd[0].workspace.pr_number, expected_pr, "{label}");
        assert!(!Arc::ptr_eq(&before.agents, &next.agents), "{label}");
        assert_eq!(next.agent_encodings, 1, "{label}");
    }
    assert_eq!(
        store
            .update_where(|_| true, |a| a.title = Some("updated".into()))
            .await,
        1
    );
    store.flush().await;
    assert_eq!(store.published().agent_encodings, 1);
    store.apply(Change::Remove(row.agent_id.clone())).await;
    store.flush().await;
    assert_eq!(store.published().agent_encodings, 1);
    assert!(store.published().agents.state.is_empty());
    store.apply(Change::upsert(row)).await;
    store.flush().await;
    assert_eq!(store.published().agent_encodings, 1);
    println!("G560_INVALIDATION upsert/noop/update/remove/reinsert/expiry/stop/recovery verified");
}

#[tokio::test]
async fn g560_cached_encoder_matches_independent_bytes() {
    let store = Store::new();
    let mut seed = 560;
    for id in 0..40 {
        store.apply(Change::upsert(agent(&mut seed, id))).await;
    }
    store.flush().await;
    let initial = store.published();
    for case in 0..64 {
        let mut snapshot = store.snapshot_locked(&*store.inner.lock().await);
        snapshot.git_plane_alive = case % 2 == 0;
        snapshot.git_plane_skipped = case;
        snapshot.git_plane_last_event_age_ms = Some(case * 1234);
        snapshot.git_worktree_facts.insert(
            "/path/ไทย\"\\\n".into(),
            GitFactAge {
                fact_age_ms: Some(case * 999),
                stale: case % 3 == 0,
            },
        );
        let expected = Json(old_wire_value(&store.epoch(), &snapshot))
            .into_response()
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes();
        let expected_event = old_event("snapshot", snapshot.rev, &store.epoch(), &snapshot).await;
        let next = PublishedSnapshot::new(
            &store.epoch(),
            snapshot,
            Instant::now(),
            VecDeque::new(),
            Some(initial.agents.clone()),
        );
        assert_eq!(*next.body, expected, "snapshot {case}");
        assert_eq!(*next.event, expected_event, "event {case}");
        assert_eq!(next.agent_encodings, 0);
    }
    println!("G560_CACHED_BYTES states=64 snapshots=64 sse_frames=64 agent_encodings=0");
}

fn g562_fixture_agents() -> BTreeMap<String, Agent> {
    let fixture: serde_json::Value = serde_json::from_str(include_str!(
        "../../../docs/evidence/issue-556/rust/fixture-fresh.json"
    ))
    .unwrap();
    serde_json::from_value(fixture["agents"].clone()).unwrap()
}

#[cfg(unix)]
#[tokio::test]
async fn g562_aliased_worktree_binding_tracks_fact_freshness() {
    use crate::core::util::canonicalize_existing_prefix;
    use crate::core::workspace::paths_match;

    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let canonical = root.join("repo/worktree");
    std::fs::create_dir_all(&canonical).unwrap();
    for name in ["a-link", "z-link"] {
        std::os::unix::fs::symlink(root.join("repo"), root.join(name)).unwrap();
    }
    let fixture = g562_fixture_agents();
    for alias in [
        root.join("a-link/worktree"),
        root.join("z-link/worktree"),
        canonical.join("../worktree"),
        PathBuf::from(format!("{}//worktree/", root.join("repo").display())),
    ] {
        assert_ne!(alias.as_os_str(), canonical.as_os_str());
        assert_eq!(canonicalize_existing_prefix(&alias), canonical);
        assert!(paths_match(&alias, &canonical));
        let store = Store::new();
        let health = store.git_plane_health();
        health.enabled.store(true, Ordering::Release);
        let mut row = fixture["bound"].clone();
        row.workspace.worktree_path = Some(alias.to_string_lossy().into_owned());
        let mut peer = row.clone();
        peer.agent_id = "canonical-peer".into();
        peer.workspace.worktree_path = Some(canonical.to_string_lossy().into_owned());
        store.apply(Change::upsert(row.clone())).await;
        store.apply(Change::upsert(peer)).await;
        for (label, alive, age, bound) in [
            ("fresh", true, Some(0), true),
            ("expired", true, Some(121), false),
            ("recovered", true, Some(0), true),
            ("stopped", false, Some(0), false),
            ("absent", true, None, false),
            ("fresh-again", true, Some(0), true),
        ] {
            health.alive.store(alive, Ordering::Release);
            {
                let mut facts = health.facts.lock().unwrap();
                facts.clear();
                if let Some(age) = age {
                    facts.insert(
                        canonical.clone(),
                        Some(Instant::now() - Duration::from_secs(age)),
                    );
                }
            }
            let delta = store.flush().await;
            assert_eq!(
                delta.is_some(),
                label != "absent",
                "{label}: binding transition delta"
            );
            let published = store.published();
            let wire: serde_json::Value = serde_json::from_slice(&published.body).unwrap();
            println!(
                "G562_ALIAS alias={} canonical={} phase={label} snapshot={wire}",
                alias.display(),
                canonical.display()
            );
            for id in ["bound", "canonical-peer"] {
                let ws = &published.agents.state[id].workspace;
                assert_eq!(
                    ws.pr_number,
                    bound.then_some(556),
                    "{label}: fresh aliased worktree must retain its PR binding ({})",
                    alias.display()
                );
                assert_eq!(
                    ws.ci_status,
                    bound.then_some(row.workspace.ci_status.unwrap())
                );
                assert_eq!(
                    ws.pr_match_source,
                    bound.then_some(row.workspace.pr_match_source.clone().unwrap())
                );
                assert_eq!(ws.issues.len(), usize::from(bound));
                if let Some(delta) = &delta {
                    assert_eq!(
                        delta
                            .upd
                            .iter()
                            .find(|a| a.agent_id == id)
                            .unwrap()
                            .workspace,
                        *ws
                    );
                }
            }
            if age.is_some() {
                assert_eq!(
                    wire["git_worktree_facts"].as_object().unwrap().len(),
                    1,
                    "an alias must not manufacture a stale fact"
                );
            }
            assert_eq!(
                store.get("bound").await.unwrap(),
                row,
                "projection must not destroy recoverable facts"
            );
        }
    }
    println!("G562_ALIAS_VERIFIED spellings=4 transitions=24 agents_per_transition=2");
}

#[tokio::test]
async fn g562_canonical_snapshot_fixtures() {
    let fresh = g562_fixture_agents();
    for (label, alive, observed) in [
        // A future monotonic instant fixes the fixture age at zero without
        // normalizing any health fields out of the byte comparison.
        (
            "fresh",
            true,
            Some(Instant::now() + Duration::from_secs(60)),
        ),
        (
            "stopped",
            false,
            Some(Instant::now() + Duration::from_secs(60)),
        ),
        ("unobserved", true, None),
        ("absent", true, None),
    ] {
        let store = Store::new();
        let health = store.git_plane_health();
        health.enabled.store(true, Ordering::Release);
        health.alive.store(alive, Ordering::Release);
        for row in fresh.values() {
            if label != "absent" {
                health.facts.lock().unwrap().insert(
                    PathBuf::from(row.workspace.worktree_path.as_ref().unwrap()),
                    observed,
                );
            }
            store.apply(Change::upsert(row.clone())).await;
        }
        store.flush().await;
        let publication = store.published();
        let actual: serde_json::Value = serde_json::from_slice(&publication.body).unwrap();
        assert_eq!(
            actual["agents"]["bound"]["workspace"]["pr_number"],
            if label == "fresh" {
                serde_json::json!(556)
            } else {
                serde_json::Value::Null
            }
        );
        println!(
            "G562_CANONICAL {label} {}",
            std::str::from_utf8(&publication.body).unwrap()
        );
    }
}

#[tokio::test]
async fn g560_equal_age_still_refreshes_its_time_origin() {
    let store = Store::new();
    let mut snapshot = store.snapshot_locked(&*store.inner.lock().await);
    snapshot.git_plane_last_event_age_ms = Some(0);
    snapshot.git_worktree_facts.insert(
        "/g560-reobserved".into(),
        GitFactAge {
            fact_age_ms: Some(0),
            stale: false,
        },
    );
    let first = PublishedSnapshot::new(
        &store.epoch(),
        snapshot.clone(),
        Instant::now(),
        VecDeque::new(),
        None,
    );
    assert!(first.same_health(&snapshot));
    snapshot.generated_at += 2000;
    assert!(
        !first.same_health(&snapshot),
        "equal ages cannot retain an old time origin"
    );
    let next = PublishedSnapshot::new(
        &store.epoch(),
        snapshot.clone(),
        Instant::now(),
        VecDeque::new(),
        Some(first.agents.clone()),
    );
    assert_eq!(next.agent_encodings, 0);
    let wire: serde_json::Value = serde_json::from_slice(&next.body).unwrap();
    assert_eq!(wire["generated_at"], snapshot.generated_at);
}
