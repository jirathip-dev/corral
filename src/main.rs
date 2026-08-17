//! Corral host daemon (`corrald`) binary entrypoint.
//!
//! Subcommands:
//! - (default) run the daemon: `corrald [--socket <path>] [--port <n>] [--bind <ip>]`
//! - D33 digest, offline against the history ring:
//!   `corrald digest [--since <epoch-millis>] [--config-dir <path>]`
//!   (the cron/launchd artifact — see `crate::history` for the hook).
//! - #35 phase 1: fleet registry views AND writes:
//!   `corrald fleet list|check` (read-only) and
//!   `corrald fleet add|remove` (registry CRUD, atomic write)
//!   (see `crate::fleet` for the registry format and validation).

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use corrald::adapters::Adapter;
use corrald::adapters::gh_plane::GhPlane;
use corrald::adapters::git_plane::GitPlane;
use corrald::adapters::herdr::HerdrAdapter;
use corrald::api::AppState;
use corrald::api::drive::ReplayTable;
use corrald::core::events::{Plane, plane_channel};
use corrald::core::store::Store;
use corrald::core::util::now_millis;
use corrald::fleet;
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
/// G34: how often the D30 per-agent cost cache and the cost-alert watchdog
/// recompute. Bounded reads (SQL WHERE clauses, mtime-skipped file walks)
/// keep this cheap even on a 13GB+ opencode.db, so 5 minutes is plenty
/// fresh without hammering the stores.
const COST_METER_INTERVAL: Duration = Duration::from_secs(5 * 60);

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
    if args.first().map(String::as_str) == Some("fleet") {
        run_fleet(&args[1..]);
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
    let since = since
        .unwrap_or_else(|| now_millis().saturating_sub(DIGEST_DEFAULT_WINDOW.as_millis() as u64));
    let ring = HistoryRing::open(dir.join("history"), RotationPolicy::default());
    let events = ring.query(Some(since), None);
    let digest = Digest::compute(&events, since, now_millis());
    print!("{}", digest.render());
}

/// `corrald fleet` — read/write views over the fleet registry (#35). Read
/// side: `list`, `check`. Write side (slice 1): `add`, `remove`, both behind
/// atomic-write discipline and validation in [`crate::fleet::ops`]. All accept
/// `--registry <path>` to override `$CORRAL_FLEETS_PATH` /
/// `~/.hermes/scripts/fleets.json`. Everything here runs before the tokio
/// runtime is built; no subcommand touches a running daemon or the herdr
/// socket.
fn run_fleet(args: &[String]) {
    let Some(sub) = args.first().map(String::as_str) else {
        eprintln!("corrald fleet: need a subcommand: list | check | add | remove (see --help)");
        std::process::exit(2);
    };
    if matches!(sub, "--help" | "-h") {
        print_fleet_help();
        std::process::exit(0);
    }
    match sub {
        "list" | "check" => run_fleet_read_only(sub, &args[1..]),
        "add" => run_fleet_add(&args[1..]),
        "remove" => run_fleet_remove(&args[1..]),
        other => {
            eprintln!("corrald fleet: unknown subcommand: {other} (see --help)");
            std::process::exit(2);
        }
    }
}

fn print_fleet_help() {
    println!(
        "corrald fleet — read/write views over the fleet registry (#35)\n\n\
         USAGE: corrald fleet list [--registry <path>]\n\
         USAGE: corrald fleet check [--registry <path>]\n\
         USAGE: corrald fleet add --name <n> --gh <owner/repo> [--local <path>]\n\
         \t[--worktree-dir <path>] [--orch <agent>] [--workers a,b,c]\n\
         \t[--models orch=..,impl=..,review=..] [--registry <path>]\n\
         USAGE: corrald fleet remove <name> [--registry <path>]\n\n\
         list     one line per fleet: name, gh_repo, worker count,\n\
         \tpaused flag, and the three model ids\n\
         check    parse + validate, then verify each fleet's local\n\
         \tdir exists and holds a .git entry; exit 0 when every\n\
         \tfleet checks out, 1 when any fails, 2 on usage/parse error\n\
         add      resolve the repo via `gh repo view`, validate the\n\
         \tcandidate registry, then atomically insert the fleet;\n\
         \tdefaults: local ~/Projects/<name>, worktree_dir <name>,\n\
         \torch orch-<name>, workers empty, models inherited from an\n\
         \texisting fleet (or required via --models on an empty registry)\n\
         remove   atomically drop exactly one fleet by name\n\n\
         \t--registry   fleet registry JSON (default $CORRAL_FLEETS_PATH\n\
         \tor ~/.hermes/scripts/fleets.json)"
    );
}

/// `list`/`check` share one `--registry` parse; anything else is a usage
/// error. This is the existing read-only surface — its exit contract is
/// preserved unchanged (0 ok / 1 fleet failed / 2 usage-or-parse).
fn run_fleet_read_only(sub: &str, args: &[String]) {
    let mut registry: Option<PathBuf> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--registry" => {
                i += 1;
                registry = args.get(i).map(PathBuf::from);
                if registry.is_none() {
                    eprintln!("corrald fleet {sub}: --registry needs a path");
                    std::process::exit(2);
                }
            }
            "--help" | "-h" => {
                print_fleet_help();
                std::process::exit(0);
            }
            other => {
                eprintln!("corrald fleet {sub}: unknown argument: {other} (see --help)");
                std::process::exit(2);
            }
        }
        i += 1;
    }
    let path = registry.unwrap_or_else(fleet::config::default_path);
    match sub {
        "list" => run_fleet_list(&path),
        "check" => run_fleet_check(&path),
        _ => unreachable!(),
    }
}

/// `corrald fleet list`: one greppable line per fleet.
fn run_fleet_list(path: &std::path::Path) {
    let registry = match fleet::config::load(path) {
        Ok(registry) => registry,
        Err(error) => {
            eprintln!("corrald fleet list: {error}");
            std::process::exit(2);
        }
    };
    for fleet in &registry.fleets {
        println!(
            "{} {} workers={} paused={} orch={} impl={} review={}",
            fleet.name,
            fleet.gh_repo,
            fleet.workers.len(),
            fleet.paused,
            fleet.models.orch,
            fleet.models.impl_,
            fleet.models.review
        );
    }
}

/// `corrald fleet add`: build the candidate fleet, run the repo-resolves
/// check, validate the candidate registry, then atomically write. Any refusal
/// exits non-zero and leaves the registry byte-identical.
fn run_fleet_add(args: &[String]) {
    let mut registry: Option<PathBuf> = None;
    let mut name: Option<String> = None;
    let mut gh_repo: Option<String> = None;
    let mut local: Option<String> = None;
    let mut worktree_dir: Option<String> = None;
    let mut orch: Option<String> = None;
    let mut workers: Option<String> = None;
    let mut models: Option<String> = None;
    let mut i = 0;
    while i < args.len() {
        let arg = args[i].as_str();
        let mut value = || {
            i += 1;
            args.get(i).cloned()
        };
        match arg {
            "--name" => {
                name = value();
                if name.is_none() {
                    usage("fleet add: --name needs a value");
                }
            }
            "--gh" => {
                gh_repo = value();
                if gh_repo.is_none() {
                    usage("fleet add: --gh needs a value");
                }
            }
            "--local" => {
                local = value();
                if local.is_none() {
                    usage("fleet add: --local needs a value");
                }
            }
            "--worktree-dir" => {
                worktree_dir = value();
                if worktree_dir.is_none() {
                    usage("fleet add: --worktree-dir needs a value");
                }
            }
            "--orch" => {
                orch = value();
                if orch.is_none() {
                    usage("fleet add: --orch needs a value");
                }
            }
            "--workers" => {
                workers = value();
                if workers.is_none() {
                    usage("fleet add: --workers needs a value");
                }
            }
            "--models" => {
                models = value();
                if models.is_none() {
                    usage("fleet add: --models needs a value");
                }
            }
            "--registry" => {
                i += 1;
                registry = args.get(i).map(PathBuf::from);
                if registry.is_none() {
                    usage("fleet add: --registry needs a value");
                }
            }
            "--help" | "-h" => {
                print_fleet_help();
                std::process::exit(0);
            }
            other => usage(&format!("fleet add: unknown argument: {other}")),
        }
        i += 1;
    }
    let Some(name) = name else {
        usage("fleet add: --name is required");
    };
    let Some(gh_repo) = gh_repo else {
        usage("fleet add: --gh is required");
    };
    let path = registry.unwrap_or_else(fleet::config::default_path);

    let workers = workers
        .map(|raw| {
            raw.split(',')
                .map(str::trim)
                .filter(|w| !w.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    let models = match models {
        Some(raw) => match parse_models(&raw) {
            Ok(models) => Some(models),
            Err(message) => usage(&format!("fleet add: {message}")),
        },
        None => None,
    };

    let opts = fleet::ops::AddOptions {
        name: name.clone(),
        gh_repo: gh_repo.clone(),
        local,
        worktree_dir,
        orch,
        workers,
        models,
    };
    let fleet = match fleet::ops::add(&path, &opts, &fleet::ops::GhCli) {
        Ok(fleet) => fleet,
        Err(error) => {
            eprintln!("corrald fleet add: {error}");
            std::process::exit(2);
        }
    };
    println!("added fleet {} ({})", fleet.name, fleet.gh_repo);
    println!(
        "{} {} workers={} paused={} orch={} impl={} review={}",
        fleet.name,
        fleet.gh_repo,
        fleet.workers.len(),
        fleet.paused,
        fleet.models.orch,
        fleet.models.impl_,
        fleet.models.review
    );
}

/// `corrald fleet remove <name>`: drop exactly one fleet by name, atomically.
fn run_fleet_remove(args: &[String]) {
    let mut registry: Option<PathBuf> = None;
    let mut name: Option<String> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--registry" => {
                i += 1;
                registry = args.get(i).map(PathBuf::from);
                if registry.is_none() {
                    usage("fleet remove: --registry needs a value");
                }
            }
            "--help" | "-h" => {
                print_fleet_help();
                std::process::exit(0);
            }
            other => {
                if other.starts_with('-') {
                    usage(&format!("fleet remove: unknown argument: {other}"));
                }
                if name.is_some() {
                    usage("fleet remove: exactly one fleet name");
                }
                name = Some(other.to_string());
            }
        }
        i += 1;
    }
    let Some(name) = name else {
        usage("fleet remove: need a fleet name");
    };
    let path = registry.unwrap_or_else(fleet::config::default_path);
    match fleet::ops::remove(&path, &name) {
        Ok(remaining) => {
            println!("removed fleet {name}; {remaining} remain");
        }
        Err(error) => {
            eprintln!("corrald fleet remove: {error}");
            std::process::exit(2);
        }
    }
}

/// Parse `--models orch=..,impl=..,review=..` into a [`fleet::config::Models`].
fn parse_models(raw: &str) -> Result<fleet::config::Models, String> {
    let mut orch = None;
    let mut impl_ = None;
    let mut review = None;
    for pair in raw.split(',') {
        let Some((key, value)) = pair.split_once('=') else {
            return Err(format!("--models entries must be key=value, got {pair:?}"));
        };
        let value = value.trim();
        if value.is_empty() {
            return Err(format!("--models {key}= has an empty value"));
        }
        match key.trim() {
            "orch" => orch = Some(value.to_string()),
            "impl" => impl_ = Some(value.to_string()),
            "review" => review = Some(value.to_string()),
            other => {
                return Err(format!(
                    "--models unknown key {other:?} (want orch, impl, review)"
                ));
            }
        }
    }
    Ok(fleet::config::Models {
        orch: orch.ok_or_else(|| "--models must set orch".to_string())?,
        impl_: impl_.ok_or_else(|| "--models must set impl".to_string())?,
        review: review.ok_or_else(|| "--models must set review".to_string())?,
    })
}

/// Usage error: message, a hint, exit 2.
fn usage(message: &str) -> ! {
    eprintln!("corrald fleet: {message}");
    eprintln!("see `corrald fleet --help` for usage");
    std::process::exit(2);
}

/// `corrald fleet check`: validate, then verify each fleet's `local_path()`
/// exists, is a directory, and holds a `.git` entry ("repo resolves"). Exit
/// 0 when every fleet checks out, 1 when any fails.
fn run_fleet_check(path: &std::path::Path) {
    let registry = match fleet::config::load(path) {
        Ok(registry) => registry,
        Err(error) => {
            eprintln!("corrald fleet check: {error}");
            std::process::exit(2);
        }
    };
    let mut failed = 0;
    for fleet in &registry.fleets {
        let local = fleet.local_path();
        match check_local(&local) {
            None => println!("ok {}", fleet.name),
            Some(reason) => {
                failed += 1;
                println!("FAIL {}: {}", fleet.name, reason);
            }
        }
    }
    if failed > 0 {
        std::process::exit(1);
    }
}

/// Verify a resolved local path is a directory holding a `.git` entry.
fn check_local(path: &std::path::Path) -> Option<String> {
    let meta = match std::fs::metadata(path) {
        Ok(meta) => meta,
        Err(error) => return Some(format!("cannot stat {}: {error}", path.display())),
    };
    if !meta.is_dir() {
        return Some(format!("{} is not a directory", path.display()));
    }
    if !path.join(".git").exists() {
        return Some(format!("{} has no .git entry", path.display()));
    }
    None
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
                bind = iter
                    .next()
                    .cloned()
                    .unwrap_or_else(|| DEFAULT_BIND.to_string());
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

    // G34: D30 per-agent cost cache (herdr.rs reads it synchronously on
    // every pane rebuild) and the cost-alert watchdog (flags a window at
    // its threshold before agents idle from exhaustion).
    corrald::cost::agent_cache::spawn_refresh_loop(COST_METER_INTERVAL);
    corrald::cost::spawn_alert_watchdog(COST_METER_INTERVAL);

    // N6: arm the APNs notifier HERE — the daemon entrypoint — not as a
    // side effect of router() (which is also the test constructor; reading
    // CORRAL_APNS_* inside it made every API test read the ambient env and
    // race the config tests). Disabled (unconfigured / bad p8) -> the
    // daemon runs exactly as before, with a startup warning.
    if let Some(notifier) = corrald::push::Notifier::from_env(store.clone(), auth.registry.clone())
    {
        notifier.start();
    } else {
        tracing::info!("push notifier not configured (set CORRAL_APNS_* to enable APNs)");
    }

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
    axum::serve(listener, app).await.expect("axum server");
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
