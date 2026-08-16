//! corrald-ui binary: tokio runtime + eframe (wgpu renderer).

use eframe::egui;

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
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1320.0, 860.0])
            .with_min_inner_size([900.0, 600.0])
            .with_title("corral fleet"),
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
