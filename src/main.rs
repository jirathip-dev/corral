//! Corral host daemon (`corrald`) binary entrypoint.
//!
//! Subcommands:
//! - (default) run the daemon: `corrald [--socket <path>] [--port <n>] [--bind <ip>]`
//! - D33 digest, offline against the history ring:
//!   `corrald digest [--since <epoch-millis>] [--config-dir <path>]`
//!   (the cron/launchd artifact — see `crate::history` for the hook).

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use corrald::adapters::gh_plane::GhPlane;
use corrald::adapters::git_plane::GitPlane;
use corrald::adapters::herdr::HerdrAdapter;
use corrald::adapters::Adapter;
use corrald::api::drive::ReplayTable;
use corrald::api::AppState;
use corrald::core::events::{Plane, plane_channel};
use corrald::core::store::Store;
use corrald::core::util::now_millis;
use corrald::history::{Digest, HistoryRing, RotationPolicy};
use corrald::integrate::Integrator;
use tracing_subscriber::EnvFilter;

/// Loopback only — no auth in P1, so never bind a routable interface.
const DEFAULT_BIND: &str = "127.0.0.1";
const DEFAULT_PORT: u16 = 8474;
/// Integrator supervisor backoff, mirroring the herdr adapter's reconnect
/// loop (WS3 F4): doubling backoff; a generation that survived at least
/// this long proves the planes were healthy and resets the backoff.
const INTEGRATOR_RECONNECT_BASE: Duration = Duration::from_secs(1);
const INTEGRATOR_RECONNECT_MAX: Duration = Duration::from_secs(30);
const INTEGRATOR_RECONNECT_RESET_AFTER: Duration = Duration::from_secs(2);
/// `corrald digest` default window when `--since` is omitted: the last 24h.
const DIGEST_DEFAULT_WINDOW: Duration = Duration::from_secs(24 * 3600);

/// $CORRAL_CONFIG_DIR, or ~/.config/corral.
fn config_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    std::env::var("CORRAL_CONFIG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(&home).join(".config/corral"))
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("digest") {
        run_digest(&args[1..]);
        return;
    }

    let (socket_path, addr) = parse_args(&args);
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");

    runtime.block_on(async_main(socket_path, addr));
}

/// D33: `corrald digest` — offline daily digest over the history ring.
/// Reads the same `$CORRAL_CONFIG_DIR/history` segments the daemon writes
/// (or `--config-dir`), so it runs without a live daemon or herdr socket.
/// `--since` is epoch millis; the default window is the last 24h.
fn run_digest(args: &[String]) {
    let mut since: Option<u64> = None;
    let mut dir: Option<PathBuf> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--since" => {
                i += 1;
                since = args.get(i).and_then(|v| v.parse().ok());
                if since.is_none() {
                    eprintln!("corrald digest: --since needs epoch millis (e.g. 1784210400000)");
                    std::process::exit(2);
                }
            }
            "--config-dir" => {
                i += 1;
                dir = args.get(i).map(PathBuf::from);
                if dir.is_none() {
                    eprintln!("corrald digest: --config-dir needs a path");
                    std::process::exit(2);
                }
            }
            "--help" | "-h" => {
                println!(
                    "corrald digest — daily per-agent digest from the history ring (D33)\n\n\
                     USAGE: corrald digest [--since <epoch-millis>] [--config-dir <path>]\n\n\
                     --since        window start, epoch millis (default: now - 24h)\n\
                     --config-dir   history dir base (default $CORRAL_CONFIG_DIR or\n\
                     ~/.config/corral); the ring lives under <dir>/history\n\n\
                     cron example:\n\
                     0 9 * * * corrald digest --since \"$(( $(date +%s) * 1000 - 86400000 ))\""
                );
                std::process::exit(0);
            }
            other => {
                eprintln!("corrald digest: unknown argument: {other} (see --help)");
                std::process::exit(2);
            }
        }
        i += 1;
    }
    let dir = dir.unwrap_or_else(config_dir);
    let since = since.unwrap_or_else(|| now_millis().saturating_sub(DIGEST_DEFAULT_WINDOW.as_millis() as u64));
    let ring = HistoryRing::open(dir.join("history"), RotationPolicy::default());
    let events = ring.query(Some(since), None);
    let digest = Digest::compute(&events, since, now_millis());
    print!("{}", digest.render());
}

fn parse_args(args: &[String]) -> (PathBuf, SocketAddr) {
    let mut socket: Option<String> = None;
    let mut port: Option<u16> = None;
    let mut bind = DEFAULT_BIND.to_string();
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--socket" | "-s" => {
                socket = iter.next().cloned();
            }
            "--port" | "-p" => {
                port = iter.next().and_then(|p| p.parse().ok());
            }
            "--bind" | "-b" => {
                bind = iter.next().cloned().unwrap_or_else(|| DEFAULT_BIND.to_string());
            }
            "--help" | "-h" => {
                println!(
                    "corrald — agent-fleet control plane daemon (P1)\n\n\
                     USAGE: corrald [--socket <path>] [--port <n>] [--bind <ip>]\n\
                     USAGE: corrald digest [--since <epoch-millis>] [--config-dir <path>]\n\n\
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

async fn async_main(socket_path: PathBuf, addr: SocketAddr) {
    let store = Store::with_history_dir(config_dir().join("history"));
    let coalescer = store.clone();
    tokio::spawn(async move { coalescer.run_coalescer().await });

    // W3 auth plane: host keypair (X25519), device registry, authorizer,
    // step-up gate, hash-chained audit log, admin token. Config dir is
    // $CORRAL_CONFIG_DIR or ~/.config/corral; all key material is 0600
    // under a 0700 directory (see crate::auth for the rotation story).
    let auth = Arc::new(
        corrald::auth::AuthPlane::load_or_create(config_dir())
            .unwrap_or_else(|e| panic!("auth plane init failed in {:?}: {e}", config_dir())),
    );

    let adapter: Arc<dyn Adapter> = Arc::new(HerdrAdapter::new(socket_path.clone()));
    adapter.clone().start(store.clone());

    // The two P2 data planes + the integrator that folds their facts onto
    // the agent records. `CORRAL_REPO_ROOT`/`CORRAL_WORKTREES_ROOT` override
    // the HOME-derived defaults. The planes keep their push-only contract;
    // the integrator is a pure channel drain (no polling, no SSE receiver),
    // supervised so a panic cannot silently kill the data plane (WS3 F4).
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    let repo_root = std::env::var("CORRAL_REPO_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(&home).join("Projects/herdr-board"));
    let worktrees_root = std::env::var("CORRAL_WORKTREES_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(&home).join(".herdr/worktrees"));

    tokio::spawn(supervise_planes(
        store.clone(),
        repo_root.clone(),
        worktrees_root.clone(),
    ));
    tracing::info!(
        repo_root = %repo_root.display(),
        worktrees_root = %worktrees_root.display(),
        "planes supervisor live: git watcher + gh poller -> integrator -> store"
    );

    let app = corrald::api::router(AppState {
        store,
        auth,
        adapter,
        replay: Arc::new(ReplayTable::default()),
    });
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .unwrap_or_else(|e| panic!("failed to bind {addr}: {e}"));
    tracing::info!(
        %addr,
        socket = %socket_path.display(),
        config_dir = %config_dir().display(),
        "corrald listening (loopback only); auth plane live: GET /host-key, POST /register"
    );
    axum::serve(listener, app)
        .await
        .expect("axum server");
}

/// WS3 F4: supervisor for the integrator task, mirroring the herdr
/// adapter's reconnect loop. A panicking integrator would drop the plane
/// channel receiver; both planes then stop on SinkClosed (gh by contract,
/// git via its sink-close exit) and facts would never merge again. The
/// supervisor therefore owns the channel and re-arms both planes per
/// generation. Residual: a previous generation's gh loop notices the dead
/// sink only at its next poll send, so a restart can briefly double-poll.
async fn supervise_planes(store: Store, repo_root: PathBuf, worktrees_root: PathBuf) {
    let mut backoff = INTEGRATOR_RECONNECT_BASE;
    loop {
        // Fresh plane instances per generation (re-review R1/R2): a re-armed
        // GitPlane must boot with an EMPTY registry so the boot rescan
        // re-emits every worktree fact into the new integrator's empty
        // caches (a reused instance would diff against retained facts and
        // emit nothing until the next real change), and the per-instance
        // stopped flag must not couple generations (a lingering old
        // watcher's sink failure must not kill the new watcher too).
        let git_plane: Arc<dyn Plane> =
            Arc::new(GitPlane::new(repo_root.clone(), worktrees_root.clone()));
        let gh_plane: Arc<dyn Plane> = Arc::new(GhPlane::new(Arc::new(store.clone())));
        let (sink, rx) = plane_channel();
        git_plane.start(sink.clone());
        gh_plane.start(sink.clone());
        let integrator = Integrator::new(store.clone(), repo_root.clone(), worktrees_root.clone());
        let started = tokio::time::Instant::now();
        let generation = tokio::spawn(async move { integrator.run(rx).await });
        match generation.await {
            Ok(()) => tracing::warn!("integrator exited cleanly; restarting planes"),
            Err(error) => tracing::warn!(error = %error, "integrator panicked; restarting planes"),
        }
        if started.elapsed() >= INTEGRATOR_RECONNECT_RESET_AFTER {
            backoff = INTEGRATOR_RECONNECT_BASE;
        }
        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(INTEGRATOR_RECONNECT_MAX);
    }
}
