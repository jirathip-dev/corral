//! Corral P4 client (`corrald-ui`): a dark-dashboard fleet board
//! speaking corrald's read plane (snapshot/SSE with Last-Event-ID resume)
//! and — on desktop — its signed drive plane (device-keypair writes,
//! claim-based approvals, idempotent retries, transparent step-up).
//!
//! ## Targets
//!
//! - **Native** (`macOS` / `Linux`): the full board — read plane +
//!   signed drive plane, device keyring, registration, evidence capture.
//! - **Wasm** (`wasm32-unknown-unknown`, #215): the READ-ONLY web build.
//!   The board renders `/snapshot` + `/events` SSE (and demo data out of
//!   the box); there is NO `/drive`, NO `/host-key`/`/step-up`, NO
//!   `keyring`, NO registration — writes stay desktop. The native-only
//!   modules (`app`, `keys`, `ui::register`) are cfg-gated out.
//!   [`web`] is the wasm-only app.
//!
//! The daemon's HTTP surface is the contract (`docs/corral/P4-conformance.md`).
//! The binary lives in `main.rs`; this lib exposes the wire/protocol/drive
//! layers so they carry unit tests and the conformance suite.

#[cfg(not(target_arch = "wasm32"))]
pub mod app;
pub mod demo;
pub mod drive;
pub mod infer;
#[cfg(not(target_arch = "wasm32"))]
pub mod keys;
pub mod model;
pub mod protocol;
pub mod state;
pub mod theme;
pub mod ui;
#[cfg(target_arch = "wasm32")]
pub mod web;

/// #215 web entrypoint. wasm-pack builds the crate's `cdylib` lib for
/// `wasm32-unknown-unknown`, so `main.rs` (the native binary's `main`)
/// is NOT part of the wasm artifact — this `#[wasm_bindgen(start)]` hook
/// is. It boots eframe's WebRunner on the page's `<canvas id="corral">`
/// with the read-only [`web::WebCorralApp`]; the canvas + module glue
/// come from `web/index.html` (see the README's build section).
#[cfg(target_arch = "wasm32")]
mod web_wasm_entry {
    #[wasm_bindgen::prelude::wasm_bindgen(start)]
    pub fn start() {
        console_error_panic_hook::set_once();
        let web_options = eframe::WebOptions {
            renderer: eframe::Renderer::Glow,
            ..Default::default()
        };
        wasm_bindgen_futures::spawn_local(async move {
            let canvas = web_sys::window()
                .and_then(|window| window.document())
                .and_then(|document| document.get_element_by_id("corral"))
                .and_then(|element| {
                    wasm_bindgen::JsCast::dyn_into::<web_sys::HtmlCanvasElement>(element).ok()
                })
                .expect("index.html must contain <canvas id=\"corral\"></canvas>");
            eframe::WebRunner::new()
                .start(
                    canvas,
                    web_options,
                    Box::new(|cc| Ok(Box::new(super::web::WebCorralApp::new(cc)))),
                )
                .await
                .expect("failed to start eframe on wasm");
        });
    }
}

/// Serializes tests that mutate the process env (CORRAL_CONFIG_DIR /
/// CORRAL_UI_CONFIG_DIR / CORRAL_UI_DISABLE_KEYRING). Rust test threads
/// share one env; concurrent env mutation is the #249 identity-test race
/// (a keyring read from an un-authorized test binary also BLOCKS on the
/// macOS Keychain prompt — keep the file-store mode in env-bearing tests).
#[cfg(test)]
pub(crate) static TEST_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
