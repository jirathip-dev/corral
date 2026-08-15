//! Corral host daemon (`corrald`) binary entrypoint.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use corrald::adapters::herdr::HerdrAdapter;
use corrald::adapters::Adapter;
use corrald::api::AppState;
use corrald::core::store::Store;
use tracing_subscriber::EnvFilter;

/// Loopback only — no auth in P1, so never bind a routable interface.
const DEFAULT_BIND: &str = "127.0.0.1";
const DEFAULT_PORT: u16 = 8474;

fn parse_args() -> (PathBuf, SocketAddr) {
    let mut socket: Option<String> = None;
    let mut port: Option<u16> = None;
    let mut bind = DEFAULT_BIND.to_string();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--socket" | "-s" => {
                socket = args.next();
            }
            "--port" | "-p" => {
                port = args.next().and_then(|p| p.parse().ok());
            }
            "--bind" | "-b" => {
                bind = args.next().unwrap_or_else(|| DEFAULT_BIND.to_string());
            }
            "--help" | "-h" => {
                println!(
                    "corrald — agent-fleet control plane daemon (P1)\n\n\
                     USAGE: corrald [--socket <path>] [--port <n>] [--bind <ip>]\n\n\
                     --socket  herdr API socket (default ~/.config/herdr/herdr.sock)\n\
                     --port    HTTP port (default {DEFAULT_PORT})\n\
                     --bind    loopback bind address (default {DEFAULT_BIND})"
                );
                std::process::exit(0);
            }
            other => {
                eprintln!("unknown argument: {other} (see --help)");
                std::process::exit(2);
            }
        }
    }
    let socket_path = socket.map(PathBuf::from).unwrap_or_else(|| {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        PathBuf::from(home).join(".config/herdr/herdr.sock")
    });
    let addr = SocketAddr::from((
        bind.parse::<std::net::IpAddr>()
            .expect("invalid --bind address"),
        port.unwrap_or(DEFAULT_PORT),
    ));
    // Security baseline: loopback only until P3 device signatures. Refuse
    // anything routable rather than silently exposing unauthenticated agent
    // state to the network.
    if !addr.ip().is_loopback() {
        eprintln!(
            "refusing to bind {addr}: P1 corrald has no auth and must stay on \
             loopback until P3 device signatures land"
        );
        std::process::exit(2);
    }
    (socket_path, addr)
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let (socket_path, addr) = parse_args();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");

    runtime.block_on(async_main(socket_path, addr));
}

async fn async_main(socket_path: PathBuf, addr: SocketAddr) {
    let store = Store::new();
    let coalescer = store.clone();
    tokio::spawn(async move { coalescer.run_coalescer().await });

    let adapter: Arc<dyn Adapter> = Arc::new(HerdrAdapter::new(socket_path.clone()));
    adapter.start(store.clone());

    let app = corrald::api::router(AppState { store });
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .unwrap_or_else(|e| panic!("failed to bind {addr}: {e}"));
    tracing::info!(
        %addr,
        socket = %socket_path.display(),
        "corrald listening (loopback only)"
    );
    axum::serve(listener, app)
        .await
        .expect("axum server");
}
