//! corrald-ui binary entrypoint (native macOS/Linux): tokio runtime +
//! eframe (wgpu renderer), the full board including the signed drive
//! plane.
//!
//! The #215 read-only WEB build does NOT use this `main`: wasm-pack
//! builds the crate's cdylib lib and the `#[wasm_bindgen(start)]` hook in
//! `lib.rs` boots the wasm app (`crate::web`). See the README's web
//! section.

#[cfg(not(target_arch = "wasm32"))]
use eframe::egui;

/// The window/app icon. macOS uses the transparent squircle source so the
/// runtime override preserves the bundle icon's transparent corners; Linux
/// keeps the existing opaque 256px icon behavior.
#[cfg(all(not(target_arch = "wasm32"), target_os = "macos"))]
const APP_ICON_BYTES: &[u8] = include_bytes!("../../../assets/icon/corral-icon-macos.png");

#[cfg(all(not(target_arch = "wasm32"), not(target_os = "macos")))]
const APP_ICON_BYTES: &[u8] = include_bytes!("../../../assets/icon/corral-icon-256.png");

#[cfg(not(target_arch = "wasm32"))]
fn app_icon() -> egui::IconData {
    let img = image::load_from_memory(APP_ICON_BYTES).expect("embedded icon PNG is valid");
    let rgba = img.to_rgba8();
    let (width, height) = (rgba.width(), rgba.height());
    egui::IconData {
        rgba: rgba.into_raw(),
        width,
        height,
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn viewport_builder() -> egui::ViewportBuilder {
    egui::ViewportBuilder::default()
        .with_inner_size([1320.0, 860.0])
        .with_min_inner_size([900.0, 600.0])
        .with_title("corral fleet")
        .with_icon(app_icon())
}

#[cfg(not(target_arch = "wasm32"))]
fn main() -> eframe::Result {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");

    let options = eframe::NativeOptions {
        viewport: viewport_builder(),
        renderer: eframe::Renderer::Wgpu,
        ..Default::default()
    };

    eframe::run_native(
        "corral fleet",
        options,
        Box::new(|cc| {
            Ok(Box::new(corrald_ui::app::CorralApp::new(
                cc,
                rt.handle().clone(),
            )))
        }),
    )
}

/// #215: the wasm BIN is never what runs in the browser — wasm-pack
/// builds the cdylib lib, whose `#[wasm_bindgen(start)]` hook in lib.rs
/// boots the read-only web app. An empty main keeps the bin target
/// compilable for `wasm32-unknown-unknown` (cargo requires one).
#[cfg(target_arch = "wasm32")]
fn main() {}

#[cfg(test)]
mod tests {
    use super::viewport_builder;

    #[test]
    fn viewport_builder_applies_platform_icon() {
        let viewport = viewport_builder();
        let icon = viewport.icon.expect("viewport icon");
        #[cfg(target_os = "macos")]
        assert_eq!((icon.width, icon.height), (1024, 1024));
        #[cfg(not(target_os = "macos"))]
        assert_eq!((icon.width, icon.height), (256, 256));
        assert_eq!(
            icon.rgba.len(),
            icon.width as usize * icon.height as usize * 4
        );
        assert!(icon.rgba.iter().any(|pixel| *pixel != 0));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_runtime_icon_has_transparent_corners_and_opaque_interior() {
        let icon = viewport_builder().icon.expect("viewport icon");
        let pixel = |x: u32, y: u32| icon.rgba[((y * icon.width + x) * 4 + 3) as usize];
        assert_eq!(pixel(0, 0), 0);
        assert_eq!(pixel(icon.width - 1, 0), 0);
        assert_eq!(pixel(0, icon.height - 1), 0);
        assert_eq!(pixel(icon.width - 1, icon.height - 1), 0);
        assert_eq!(pixel(icon.width / 2, icon.height / 2), 255);
    }
}
