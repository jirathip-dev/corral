//! SSE over pre-encoded publications. Subscribe before reading the boundary;
//! a later delta can never be lost between snapshot capture and subscription.
use std::convert::Infallible;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::body::{Body, Bytes};
use axum::extract::State;
use axum::http::{HeaderMap, header};
use axum::response::{IntoResponse, Response};
use futures::stream::{self, StreamExt};
use tokio::sync::broadcast::error::RecvError;

use super::AppState;

const KEEPALIVE: Duration = Duration::from_secs(15);
const KEEPALIVE_FRAME: Bytes = Bytes::from_static(b":\n\n");

pub(super) async fn events(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    let started = Instant::now();
    let store = state.store.clone();
    let requested_rev = headers
        .get("last-event-id")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<u64>().ok());
    let requested_epoch = headers.get("corral-epoch").and_then(|v| v.to_str().ok());
    let epoch = store.epoch();
    let rx = store.subscribe();
    let publication = store.published();
    // Absent = legacy revision-only resume; stale/unknown epoch = snapshot.
    let initial = publication.resume(
        requested_rev,
        requested_epoch.is_none_or(|v| v == epoch.as_ref()),
    );
    let initial_kind = if initial
        .first()
        .is_some_and(|v| Arc::ptr_eq(v, &publication.event))
    {
        "snapshot"
    } else {
        "delta"
    };
    let initial_rev = publication.rev;
    let captured_at = publication.captured_at;
    let initial = stream::iter(initial.into_iter().map(move |bytes| {
        (
            bytes.as_ref().clone(),
            captured_at,
            initial_kind,
            initial_rev,
        )
    }));
    let live = stream::unfold((rx, initial_rev), move |(mut rx, mut cursor)| {
        let store = store.clone();
        async move {
            // One deadline for the entire receive loop, including skipped
            // duplicates; matches axum's reset-after-emitted-frame cadence.
            let next = async {
                loop {
                    let rev = match rx.recv().await {
                        Ok(delta) if delta.rev <= cursor => continue,
                        Ok(delta) => Some(delta.rev),
                        Err(RecvError::Lagged(_)) => None,
                        Err(RecvError::Closed) => break None,
                    };
                    let current = store.published();
                    if let Some(rev) = rev
                        && let Some(frame) = current.delta(rev)
                    {
                        cursor = rev;
                        break Some((frame.as_ref().clone(), current.captured_at, "delta", cursor));
                    }
                    // Discard an obsolete queue BEFORE taking the replacement
                    // boundary. Otherwise the next write can overflow it again.
                    rx = store.subscribe();
                    let current = store.published();
                    cursor = current.rev;
                    break Some((
                        current.event.as_ref().clone(),
                        current.captured_at,
                        "snapshot",
                        cursor,
                    ));
                }
            };
            match tokio::time::timeout(KEEPALIVE, next).await {
                Ok(Some(frame)) => Some((frame, (rx, cursor))),
                Ok(None) => None,
                Err(_) => Some((
                    (
                        KEEPALIVE_FRAME,
                        store.published().captured_at,
                        "keepalive",
                        cursor,
                    ),
                    (rx, cursor),
                )),
            }
        }
    });
    let mut first = true;
    let frames = initial
        .chain(live)
        .map(move |(bytes, captured_at, kind, rev)| {
            if first {
                first = false;
                if let Some(timing) =
                    super::read_timing::SSE_FIRST.record(started.elapsed(), captured_at.elapsed())
                {
                    tracing::info!(
                        requests = timing.requests,
                        max_serve_ms = timing.serve_ms,
                        max_buffer_age_ms = timing.buffer_age_ms,
                        latest_frame_kind = kind,
                        latest_live_from_rev = rev,
                        "SSE first frame served"
                    );
                }
            }
            Ok::<_, Infallible>(bytes)
        });
    (
        [
            (header::CONTENT_TYPE, "text/event-stream"),
            (header::CACHE_CONTROL, "no-cache"),
        ],
        Body::from_stream(frames),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::response::sse::{Event, Sse};
    use http_body_util::BodyExt;

    #[tokio::test]
    async fn keepalive_bytes_match_axum() {
        let response = Sse::new(stream::iter([Ok::<_, Infallible>(
            Event::DEFAULT_KEEP_ALIVE,
        )]))
        .into_response();
        assert_eq!(
            response.into_body().collect().await.unwrap().to_bytes(),
            KEEPALIVE_FRAME
        );
    }
}
