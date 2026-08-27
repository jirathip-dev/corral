//! Narrow CORS layer for the credential-free read plane (#215).
//!
//! corrald's read plane (`/healthz`, `/snapshot`, `/events`, `/history`,
//! `/issues`, `/fleets`) is credential-free by design (#65). The read-only
//! web board (`clients/egui` wasm build, GitHub Pages demo) fetches those
//! from a DIFFERENT origin, so `Access-Control-Allow-Origin` is the only
//! daemon-side change a browser page needs to read the live board.
//!
//! Policy — the issue's bounds, never widened:
//!
//! - **Opt-in**: with an empty allowlist the daemon behaves exactly as
//!   before; zero CORS headers are ever added.
//! - **Allowlist only**: `--cors-origin` / `CORRALD_CORS_ORIGIN` carry
//!   exact origins (`scheme://host[:port]`); the middleware reflects ONLY
//!   a request `Origin` that matches the list. `*` is rejected at parse
//!   time and never emitted.
//! - **Read plane only**: this middleware sits on the read routes. The
//!   write plane (`/drive`, device-token, grants-read, auth routes) never
//!   emits CORS headers, so a browser from another origin cannot perform
//!   signed writes — a cross-origin `POST /drive` preflight gets no
//!   `Access-Control-Allow-Origin` and the browser blocks it.
//! - **Permitted binds only**: corrald can never bind a public interface
//!   (`bind_permitted`), so CORS is only ever reachable on loopback /
//!   private / tailnet binds — the same read boundary as the read plane.
//!
//! Matching is exact: an origin is `scheme://host[:port]` with no trailing
//! slash; the daemon compares the `Origin` header byte-for-byte (the
//! configured value comes from our own CLI flag, not from the wire).
//! Preflight `OPTIONS` on a read route is answered with 204 and the read
//! method/header set; `Last-Event-ID` and `Content-Type` are the only
//! headers the web client ever sends cross-origin.

use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{HeaderValue, Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::Response;

use super::AppState;

/// Read-route CORS middleware. No-op unless the allowlist is non-empty AND
/// the request's `Origin` matches it exactly; the write plane never passes
/// through this layer.
pub async fn cors(State(state): State<Arc<AppState>>, request: Request, next: Next) -> Response {
    if state.cors_origins.is_empty() {
        return next.run(request).await;
    }
    let Some(origin) = request
        .headers()
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok())
        .filter(|origin| state.cors_origins.iter().any(|allowed| allowed == origin))
        .map(str::to_string)
    else {
        return next.run(request).await;
    };

    if request.method() == Method::OPTIONS {
        // Preflight on a read route: allow the browser's simple GET +
        // Last-Event-ID resume headers. Still only for a matching origin.
        let mut response = Response::new(Body::empty());
        *response.status_mut() = StatusCode::NO_CONTENT;
        set_allow_origin(response.headers_mut(), &origin);
        response.headers_mut().insert(
            header::ACCESS_CONTROL_ALLOW_METHODS,
            HeaderValue::from_static("GET, OPTIONS"),
        );
        response.headers_mut().insert(
            header::ACCESS_CONTROL_ALLOW_HEADERS,
            HeaderValue::from_static("Last-Event-ID, Content-Type"),
        );
        response.headers_mut().insert(
            header::ACCESS_CONTROL_MAX_AGE,
            HeaderValue::from_static("600"),
        );
        return response;
    }

    let mut response = next.run(request).await;
    set_allow_origin(response.headers_mut(), &origin);
    response
}

fn set_allow_origin(headers: &mut axum::http::HeaderMap, origin: &str) {
    if let Ok(value) = HeaderValue::from_str(origin) {
        headers.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, value);
    }
    // Caches must key on the echoed origin, never serve a pinned value.
    headers.append(header::VARY, HeaderValue::from_static("Origin"));
}
