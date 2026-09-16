//! #555 measurement-only benchmark. Run ignored, in release mode, under the
//! evidence driver's bounded nice-0 CPU load. Identical at baseline and HEAD.
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::extract::Request;
use axum::middleware::{Next, from_fn};
use axum::response::Response;
use corrald::api::{AppState, router};
use corrald::core::model::{Agent, AgentState, Change};
use corrald::core::store::Store;
use http_body_util::BodyExt;
use tower::ServiceExt;

fn agent(index: usize) -> Agent {
    Agent {
        agent_id: format!("fixture-{index:03}"),
        source: "herdr".into(),
        tool: "fixture".into(),
        state: AgentState::Working,
        reason: Some("synthetic poller measurement".into()),
        seq: 1,
        ts: 0,
        capabilities: vec!["read_tail".into()],
        waiting_on: None,
        parent_id: None,
        host: None,
        workspace: Default::default(),
        attachment: None,
        display_name: None,
        title: None,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "bounded CPU-load evidence; run via docs/evidence/issue-555/daemon-run.py"]
async fn snapshot_load_measurement() {
    let _ = tracing_subscriber::fmt().with_ansi(false).try_init();
    let store = Store::new();
    for index in 0..64 {
        store.apply(Change::upsert(agent(index))).await;
    }
    store.flush().await;
    let coalescer_store = store.clone();
    let coalescer = tokio::spawn(async move { coalescer_store.run_coalescer().await });
    let samples = Arc::new(Mutex::new(Vec::new()));
    let measured = samples.clone();
    let app = router(AppState {
        store: store.clone(),
        ..Default::default()
    })
    .layer(from_fn(move |request: Request, next: Next| {
        let samples = measured.clone();
        async move {
            // Daemon-side accounting: enter router -> fully encoded response.
            // JSON inspection/logging below is OUTSIDE the measured interval.
            let start = Instant::now();
            let response = next.run(request).await;
            let (parts, body) = response.into_parts();
            let bytes = body.collect().await.unwrap().to_bytes();
            let serve_ms = start.elapsed().as_secs_f64() * 1000.0;
            let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64;
            let buffer_age_ms = now.saturating_sub(json["generated_at"].as_u64().unwrap());
            samples.lock().unwrap().push(serve_ms);
            println!(
                "G555_SAMPLE {}",
                serde_json::json!({"serve_ms": serve_ms, "buffer_age_ms": buffer_age_ms,
                    "rev": json["rev"], "bytes": bytes.len()})
            );
            Response::from_parts(parts, Body::from(bytes))
        }
    }));
    let (ready, started) = tokio::sync::oneshot::channel();
    let poller = tokio::spawn(async move {
        let mut ready = Some(ready);
        loop {
            store
                .update_where(
                    |_| true,
                    |agent| {
                        // Simulate slow git/GitHub integration holding the same lock
                        // used by herdr applies. No subprocess or live provider.
                        if agent.agent_id == "fixture-000" {
                            let start = Instant::now();
                            while start.elapsed() < Duration::from_millis(25) {
                                std::hint::spin_loop();
                            }
                        }
                        agent.seq += 1;
                    },
                )
                .await;
            if let Some(ready) = ready.take() {
                let _ = ready.send(());
            }
            tokio::task::yield_now().await;
        }
    });
    started.await.unwrap();
    for _ in 0..512 {
        let response = tokio::time::timeout(
            Duration::from_secs(3),
            app.clone()
                .oneshot(Request::get("/snapshot").body(Body::empty()).unwrap()),
        )
        .await
        .expect("bounded router response")
        .unwrap();
        assert_eq!(response.status(), 200);
        tokio::time::sleep(Duration::from_millis(4)).await;
    }
    poller.abort();
    coalescer.abort();
    let _ = poller.await;
    let _ = coalescer.await;
    let mut values = samples.lock().unwrap().clone();
    values.sort_by(f64::total_cmp);
    assert_eq!(values.len(), 512);
    println!(
        "G555_SUMMARY {}",
        serde_json::json!({
            "samples": values.len(), "p50_ms": values[255], "p99_ms": values[506],
            "poller_lock_work_ms": 25, "agents": 64, "runtime_workers": 4
        })
    );
}
