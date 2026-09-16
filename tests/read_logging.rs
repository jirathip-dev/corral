//! The real read handlers must share process-wide budgets, not log per client.
use std::io::Write;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use axum::body::Body;
use axum::http::Request;
use corrald::api::{AppState, router};
use http_body_util::BodyExt;
use tower::ServiceExt;

#[derive(Clone)]
struct Capture(Arc<Mutex<Vec<u8>>>);
impl Write for Capture {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[test]
fn snapshot_and_sse_info_are_bounded_at_the_real_handlers() {
    let bytes = Arc::new(Mutex::new(Vec::new()));
    let writer = Capture(bytes.clone());
    let subscriber = tracing_subscriber::fmt()
        .with_ansi(false)
        .without_time()
        .with_writer(move || writer.clone())
        .finish();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let started = Instant::now();
    tracing::subscriber::with_default(subscriber, || {
        runtime.block_on(async {
            let app = router(AppState::default());
            for _ in 0..256 {
                for path in ["/snapshot", "/events"] {
                    let response = app
                        .clone()
                        .oneshot(Request::get(path).body(Body::empty()).unwrap())
                        .await
                        .unwrap();
                    assert_eq!(response.status(), 200);
                    let mut body = response.into_body();
                    assert!(body.frame().await.unwrap().unwrap().is_data());
                }
            }
        });
    });
    let budget = started.elapsed().as_millis().div_ceil(5_000) as usize + 1;
    assert!(budget < 256, "host too slow to discriminate logging budget");
    let text = String::from_utf8(bytes.lock().unwrap().clone()).unwrap();
    for message in ["snapshot served", "SSE first frame served"] {
        let lines: Vec<_> = text.lines().filter(|line| line.contains(message)).collect();
        assert!(
            !lines.is_empty(),
            "operator timing signal missing: {message}"
        );
        assert!(
            lines.len() <= budget,
            "per-request info is not bounded: {message}: {} > {budget}",
            lines.len()
        );
        assert!(lines.iter().all(|line| line.contains("requests=")
            && line.contains("max_serve_ms=")
            && line.contains("max_buffer_age_ms=")));
        println!(
            "G555_READ_LOG {message}: {} lines for 256 requests; bound={budget}",
            lines.len()
        );
    }
}
