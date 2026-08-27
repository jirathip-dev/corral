//! Corral host daemon (`corrald`) binary entrypoint.
//!
//! Subcommands:
//! - (default) run the daemon: `corrald [--socket <path>] [--port <n>] [--bind <ip>]`
//! - D33 digest, offline against the history ring:
//!   `corrald digest [--since <epoch-millis>] [--config-dir <path>]`
//!   (the cron/launchd artifact — see `crate::history` for the hook).
//! - #237 configless fleet operations: `corrald fleet switch <name>`
//!   delegates to the fleet-ops CLI (`herdr-fleet switch`) — corral does not
//!   own, read, or write `fleets.json`. The registry views/mutations
//!   (`list`/`check`/`add`/`remove`/`pause`/`resume`/`models`/`watch` --
//!   `reap`/`prune`) were the #35 registry-ownership surface and are
//!   superseded by `herdr-fleet` (see docs/corral/G35-registry.md).

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use corrald::adapters::Adapter;
use corrald::adapters::gh_plane::GhPlane;
use corrald::adapters::git_plane::{GitPlane, LiveRepoSourceDiscovery};
use corrald::adapters::herdr::HerdrAdapter;
use corrald::api::AppState;
use corrald::api::drive::ReplayTable;
use corrald::core::events::{Plane, plane_channel};
use corrald::core::store::Store;
use corrald::core::util::now_millis;
use corrald::core::workspace::WorkspaceAttribution;
use corrald::fleet::cli::{CliFleetOpsProvider, FleetIdentity, FleetOpsProvider};
use corrald::history::{Digest, HistoryRing, RotationPolicy};
use corrald::integrate::Integrator;
use tracing_subscriber::EnvFilter;

/// Loopback default. `--bind` may widen to tailnet/private/ULA (#65,
/// `bind_permitted`); public interfaces are always refused.
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

/// `corrald fleet` — the configless fleet-operation surface (#237).
///
/// `switch` is the only subcommand. It delegates the whole auth-gated
/// re-arm to the fleet-ops CLI (`herdr-fleet switch <name>`), which is
/// lanes-aware and validates the fleet identity itself; corral never reads
/// or writes `fleets.json`. The legacy #35 registry subcommands
/// (`list`/`check`/`add`/`remove`/`pause`/`resume`/`models`/`watch`/
/// `reap`/`prune`) are superseded — use `herdr-fleet` for registry
/// operations.
fn run_fleet(args: &[String]) {
    // `corrald fleet` with no subcommand prints the (tiny) help + exit 2.
    let Some(sub) = args.first() else {
        eprintln!("corrald fleet: need a subcommand: switch (see --help)");
        std::process::exit(2);
    };
    match sub.as_str() {
        "switch" => run_fleet_switch(&args[1..]),
        "--help" | "-h" => {
            print_fleet_help();
            std::process::exit(0);
        }
        other => {
            eprintln!("corrald fleet: unknown subcommand: {other} (see --help)");
            std::process::exit(2);
        }
    }
}

fn print_fleet_help() {
    println!(
        "corrald fleet — configless fleet operations (#237)\n\n\
         USAGE: corrald fleet switch <name> [--pane <id>]\n\n\
         switch   auth-gated orchestrator re-arm, delegated to the fleet-ops CLI\n\
         (herdr-fleet switch <name>): the fleet identity, harness, auth gates,\n\
         and continuation brief are fleet-ops' — corral no longer owns, reads,\n\
         or writes the fleet registry file (the registry stays fleet-ops' config).\n\
         Registry views/mutations (list/check/add/remove/\n\
         pause/resume/models/watch/reap/prune) moved to `herdr-fleet`; use\n\
         `herdr-fleet list|check|add|remove|pause|resume|models|switch|doctor`.\n\n\
         --pane <id>   pass an explicit pane id through to herdr-fleet switch\n"
    );
}

/// `corrald fleet switch <name>`: delegate to the fleet-ops CLI.
fn run_fleet_switch(args: &[String]) {
    let mut name: Option<String> = None;
    let mut pane: Option<String> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--pane" => {
                i += 1;
                pane = args.get(i).cloned();
                if pane.is_none() {
                    eprintln!("corrald fleet switch: --pane needs a value");
                    std::process::exit(2);
                }
            }
            "--help" | "-h" => {
                print_fleet_help();
                std::process::exit(0);
            }
            other if !other.starts_with("--") && name.is_none() => {
                name = Some(other.to_string());
            }
            other => {
                eprintln!("corrald fleet switch: unknown argument: {other} (see --help)");
                std::process::exit(2);
            }
        }
        i += 1;
    }
    let Some(name) = name else {
        eprintln!("corrald fleet switch: need a fleet name");
        std::process::exit(2);
    };
    match corrald::fleet::switch::switch_fleet(&name, pane.as_deref()) {
        Ok(()) => std::process::exit(0),
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(error.exit_code());
        }
    }
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
                     USAGE: corrald digest [--since <epoch-millis>] [--config-dir <path>]\n\
                     USAGE: corrald fleet switch <name> [--pane <id>]\n\n\
                     --socket  herdr API socket (default ~/.config/herdr/herdr.sock)\n\
                     --port    HTTP port (default {DEFAULT_PORT})\n\
                     --bind    bind address (default {DEFAULT_BIND}); loopback,\n\
                     private (RFC 1918), Tailscale/CGNAT 100.64/10, and IPv6\n\
                     unique-local are permitted — public IPs and 0.0.0.0 are\n\
                     refused. Writes are device-signed on every interface;\n\
                     READS are credential-free, so beyond loopback the bound\n\
                     network itself is the read boundary (prefer a tailnet)"
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
    let ip = bind.parse::<std::net::IpAddr>().unwrap_or_else(|_| {
        eprintln!("corrald: --bind {bind:?} is not a valid IP address");
        std::process::exit(2);
    });
    let addr = SocketAddr::from((ip, port.unwrap_or(DEFAULT_PORT)));
    // Security baseline (#65): non-public interfaces only. The P3 auth
    // plane (per-device Ed25519 signatures, grants, step-up, audit) gates
    // every WRITE on every interface; the credential-free READ plane's
    // boundary beyond loopback is the bound network itself, which is why
    // public/unspecified binds stay refused — corrald is never an
    // internet-facing service.
    if !bind_permitted(&addr.ip()) {
        eprintln!(
            "refusing to bind {addr}: only loopback, private (RFC 1918), \
             Tailscale/CGNAT (100.64.0.0/10), and IPv6 unique-local (RFC 4193) \
             addresses are permitted — never public interfaces, 0.0.0.0, or \
             IPv4-mapped IPv6 forms (spell IPv4 addresses plainly)"
        );
        std::process::exit(2);
    }
    (socket_path, addr)
}

/// #65: which interfaces `--bind` may use. Loopback (the default), RFC 1918
/// private ranges, the CGNAT block Tailscale assigns from (100.64.0.0/10),
/// and IPv6 unique-local (RFC 4193, fc00::/7). Explicitly NOT permitted:
/// the unspecified addresses (0.0.0.0 / ::, which would bind every
/// interface) and public routable space. Writes are device-signed on any
/// permitted interface; reads are credential-free and bounded by the
/// network itself (a tailnet gives that boundary real device auth; a
/// plain RFC 1918 LAN does not — prefer tailnet or loopback). This guard
/// keeps the daemon off the open internet regardless.
fn bind_permitted(ip: &std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(v4) => {
            let o = v4.octets();
            v4.is_loopback() || v4.is_private() || (o[0] == 100 && (64..128).contains(&o[1]))
        }
        std::net::IpAddr::V6(v6) => {
            // fc00::/7 — unique-local (Tailscale also assigns fd7a:115c::/48).
            v6.is_loopback() || (v6.segments()[0] & 0xfe00) == 0xfc00
        }
    }
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
    // #237 configless: no fleet registry roots/aliases feed attribution.
    // The configured root plus live Herdr worktree paths are the only
    // attribution sources; repo categories come from the live snapshot.
    let configured_root = repo_root
        .file_name()
        .map(|name| corrald::core::workspace::RepoRoot {
            path: repo_root.clone(),
            repo: name.to_string_lossy().into_owned(),
        });
    let attribution = WorkspaceAttribution::from_roots(configured_root, worktrees_root.clone());
    let repo_source_discovery =
        Arc::new(LiveRepoSourceDiscovery::from_env().with_attribution(attribution.clone()));

    let adapter: Arc<dyn Adapter> = Arc::new(HerdrAdapter::new_with_attribution(
        socket_path.clone(),
        attribution.clone(),
    ));
    adapter.clone().start(store.clone());

    // #113: the read-only repo-level issue view shared between the planes
    // integrator and the API, so `GET /issues` sees the facts the worktree
    // action validates a selected issue against.
    let issues_cache: Arc<corrald::api::issues::IssuesCache> =
        Arc::new(corrald::api::issues::IssuesCache::default());
    tokio::spawn(supervise_planes(
        store.clone(),
        attribution.clone(),
        issues_cache.clone(),
        repo_root.clone(),
        repo_source_discovery,
    ));
    tracing::info!(
        repo_roots = ?attribution.repo_roots(),
        worktrees_root = %attribution.worktrees_root().display(),
        "planes supervisor live: git watcher + gh poller -> integrator -> store"
    );

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

    let fleet_provider: Arc<dyn FleetOpsProvider> = Arc::new(CliFleetOpsProvider);
    let app = corrald::api::router(AppState {
        store,
        auth,
        adapter,
        replay: Arc::new(ReplayTable::default()),
        issues: issues_cache.clone(),
        fleets: fleet_provider,
    });
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .unwrap_or_else(|e| panic!("failed to bind {addr}: {e}"));
    let scope = if addr.ip().is_loopback() {
        "loopback"
    } else {
        "tailnet/private interface"
    };
    tracing::info!(
        %addr,
        scope,
        socket = %socket_path.display(),
        config_dir = %config_dir().display(),
        "corrald listening; auth plane live: GET /host-key, POST /register"
    );
    axum::serve(listener, app).await.expect("axum server");
}

/// Build the gh-plane repo-spec set: the compile-time tracked repos (PR read
/// model) PLUS every fleet-ops CLI validated fleet's `gh_repo` so #113 can
/// issue-start any validated fleet, grouped by its fleet name. A fleet whose
/// `gh_repo` points at a repo that shares a workspace-repo name with a
/// tracked repo is authoritative for that identity (the fleet-ops validated
/// identity is what the operator is working on).
fn gh_repo_specs(identities: &[FleetIdentity]) -> Vec<corrald::adapters::gh_plane::GhRepoSpec> {
    let mut specs = corrald::adapters::gh_plane::tracked_specs();
    add_fleet_specs(&mut specs, identities);
    specs
}

/// Fold each CLI-validated fleet's `gh_repo` into the gh spec set, keyed by
/// fleet name.
fn add_fleet_specs(
    specs: &mut Vec<corrald::adapters::gh_plane::GhRepoSpec>,
    identities: &[FleetIdentity],
) {
    for fleet in identities {
        let Some((owner, name)) = fleet.gh_repo.split_once('/') else {
            continue;
        };
        let slug = format!("{owner}/{name}");
        // Same GitHub repo: fold the fleet's issue-view key onto the existing
        // spec AND force the PR attribution key to the fleet-ops `gh_repo`
        // basename. A tracked repo's compile-time folder-derived name may be
        // stale (e.g. synergy-costing vs synergy-apps) — configless
        // attribution falls back to the live directory name, so the gh fold
        // key is the CLI-validated basename.
        if let Some(existing) = specs.iter_mut().find(|s| s.slug() == slug) {
            existing.key = name.to_string();
            if existing.issues_key.is_none() {
                existing.issues_key = Some(fleet.name.clone());
            }
            continue;
        }
        // A different repo that shares the workspace repo basename: the
        // validated fleet is authoritative for that workspace identity.
        if let Some(pos) = specs.iter().position(|s| s.key == name) {
            specs.remove(pos);
        }
        specs.push(corrald::adapters::gh_plane::GhRepoSpec {
            owner: owner.to_string(),
            name: name.to_string(),
            key: name.to_string(),
            issues_key: Some(fleet.name.clone()),
        });
    }
}

/// WS3 F4: supervisor for the integrator task, mirroring the herdr
/// adapter's reconnect loop. A panicking integrator would drop the plane
/// channel receiver; both planes then stop on SinkClosed (gh by contract,
/// git via its sink-close exit) and facts would never merge again. The
/// supervisor therefore owns the channel and re-arms both planes per
/// generation. Residual: a previous generation's gh loop notices the dead
/// sink only at its next poll send, so a restart can briefly double-poll.
async fn supervise_planes(
    store: Store,
    attribution: WorkspaceAttribution,
    issues: Arc<corrald::api::issues::IssuesCache>,
    fallback_repo_root: PathBuf,
    source_discovery: Arc<LiveRepoSourceDiscovery>,
) {
    // Configless identity set for the gh plane: fleet-ops CLI validated
    // fleets. Unavailable CLI -> tracked specs only; the daemon still runs.
    let identities = match CliFleetOpsProvider.list() {
        Ok(identities) => identities,
        Err(error) => {
            tracing::warn!(
                error = %error,
                "fleet-ops CLI identity path unavailable; gh specs fall back to tracked repos"
            );
            Vec::new()
        }
    };
    let mut backoff = INTEGRATOR_RECONNECT_BASE;
    loop {
        // Fresh plane instances per generation (re-review R1/R2): a re-armed
        // GitPlane must boot with an EMPTY registry so the boot rescan
        // re-emits every worktree fact into the new integrator's empty
        // caches (a reused instance would diff against retained facts and
        // emit nothing until the next real change), and the per-instance
        // stopped flag must not couple generations (a lingering old
        // watcher's sink failure must not kill the new watcher too).
        let git_plane: Arc<dyn Plane> = Arc::new(GitPlane::with_repo_roots_and_discovery(
            vec![fallback_repo_root.clone()],
            attribution.worktrees_root(),
            source_discovery.clone(),
        ));
        let gh_plane: Arc<dyn Plane> = Arc::new(GhPlane::with_specs(
            Arc::new(store.clone()),
            gh_repo_specs(&identities),
        ));
        let (sink, rx) = plane_channel();
        let integrator =
            Integrator::with_issues(store.clone(), attribution.clone(), issues.clone());
        // Clear both the shared branch facts and already-stored recognized
        // rows before either replacement plane can emit. This closes the
        // missed-WorktreeRemoved gap without erasing repo identity or other
        // workspace/GitHub fields; unknown paths remain orphaned.
        integrator.reconcile_generation().await;
        git_plane.start(sink.clone());
        gh_plane.start(sink.clone());
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

#[cfg(test)]
mod tests {
    use super::bind_permitted;
    use corrald::fleet::cli::FleetIdentity;
    use std::net::IpAddr;

    fn ip(s: &str) -> IpAddr {
        s.parse().expect("test ip parses")
    }

    fn identity(name: &str, gh_repo: &str) -> FleetIdentity {
        FleetIdentity {
            name: name.to_string(),
            gh_repo: gh_repo.to_string(),
            local: std::path::PathBuf::from(format!("~/Projects/{name}")),
            worktree_dir: name.to_string(),
            orch: format!("orch-{name}"),
            workers: 0,
            paused: false,
        }
    }

    /// #65: the bind allowlist — loopback, RFC 1918, Tailscale/CGNAT
    /// 100.64/10, IPv6 unique-local in; public and unspecified out.
    #[test]
    fn bind_allowlist_accepts_non_public_and_refuses_public() {
        for allowed in [
            "127.0.0.1",
            "127.0.0.53",
            "10.0.0.1",
            "172.16.0.1",
            "172.31.255.254",
            "192.168.1.10",
            "10.0.0.0",        // exact 10/8 bottom
            "10.255.255.255",  // exact 10/8 top
            "172.16.0.0",      // exact 172.16/12 bottom
            "172.31.255.255",  // exact 172.16/12 top
            "100.64.0.0",      // exact CGNAT range start
            "100.67.222.5",    // a typical tailnet address
            "100.127.255.255", // exact CGNAT range end
            "::1",
            "fd7a:115c:a1e0::1",                       // Tailscale IPv6 ULA
            "fc00::",                                  // exact fc00::/7 bottom
            "fdff:ffff:ffff:ffff:ffff:ffff:ffff:ffff", // exact fc00::/7 top
        ] {
            assert!(bind_permitted(&ip(allowed)), "should permit {allowed}");
        }
        for refused in [
            "0.0.0.0",                                 // every interface — never
            "::",                                      // every interface (v6) — never
            "203.0.113.5",                             // public (TEST-NET-3)
            "8.8.8.8",                                 // public
            "100.63.255.255",                          // just below the CGNAT block
            "100.128.0.0",                             // just above the CGNAT block
            "172.32.0.1",                              // just outside 172.16/12
            "2001:db8::1",                             // public v6 (doc range)
            "fe00::1", // one mask bit above fc00::/7 — the bit the mask turns on
            "fbff:ffff:ffff:ffff:ffff:ffff:ffff:ffff", // just below fc00::/7
            "fe80::1", // link-local: outside fc00::/7
            "169.254.1.1", // v4 link-local
            // #78 (#75 round-2 vectors): IPv4-mapped IPv6 forms are
            // refused WHOLESALE — v6 loopback/ULA checks see the mapped
            // form as neither, which fails closed. Bind literals must
            // use the plain v4 spelling.
            "::ffff:8.8.8.8",      // mapped public v4
            "::ffff:127.0.0.1",    // mapped loopback — refused (fail closed)
            "::ffff:10.0.0.1",     // mapped PRIVATE v4 — wholesale means this too
            "::ffff:192.168.1.10", // the mapped spelling a user would type
            "9.255.255.255",       // just below 10/8
            "11.0.0.0",            // just above 10/8
            "172.15.255.255",      // just below 172.16/12
            "192.167.255.255",     // just below 192.168/16
            "192.169.0.0",         // just above 192.168/16
        ] {
            assert!(!bind_permitted(&ip(refused)), "should refuse {refused}");
        }
    }

    #[test]
    fn fleet_specs_make_corral_issue_startable() {
        // #113 review 1 (#237 configless): a CLI-validated fleet whose
        // `gh_repo` is NOT in the compile-time tracked set (e.g. `corral`)
        // must still get its issues fetched, grouped by the FLEET name so the
        // worktree action can start an issue against it.
        let identities = vec![
            identity("corral", "jirathip-dev/corral"),
            identity("plush", "jirathip-dev/plush-meadow"),
        ];
        let mut specs = super::gh_repo_specs(&identities);

        let corral = specs
            .iter()
            .find(|s| s.owner == "jirathip-dev" && s.name == "corral")
            .expect("corral fleet spec present");
        assert_eq!(
            corral.key, "corral",
            "PR attribution key == gh_repo basename"
        );
        assert_eq!(
            corral.issues_key.as_deref(),
            Some("corral"),
            "issues grouped by the fleet name"
        );

        let plush = specs
            .iter()
            .find(|s| s.owner == "jirathip-dev" && s.name == "plush-meadow")
            .expect("plush fleet spec present");
        assert_eq!(plush.key, "plush-meadow", "attribution key stays basename");
        assert_eq!(
            plush.issues_key.as_deref(),
            Some("plush"),
            "issue view keyed by the fleet name, not the basename"
        );
        let _ = &mut specs;
    }

    #[test]
    fn tracked_fleet_spec_uses_canonical_gh_repo_basename() {
        // #182 review F1 (#237 configless): when a compile-time tracked repo
        // is ALSO a validated fleet whose gh_repo differs from the historical
        // folder name, the PR/CI attribution key follows the CLI-validated
        // basename.
        let identities = vec![identity(
            "synergy",
            "synergy-services-cooling-tower/synergy-apps",
        )];
        let specs = super::gh_repo_specs(&identities);

        let baseline = corrald::adapters::gh_plane::tracked_specs()
            .into_iter()
            .find(|s| s.name == "synergy-apps")
            .expect("tracked synergy spec present");
        assert_eq!(
            baseline.key, "synergy-apps",
            "tracked PR key is the canonical repo basename"
        );

        let synergy = specs
            .iter()
            .find(|s| s.owner == "synergy-services-cooling-tower" && s.name == "synergy-apps")
            .expect("synergy fleet spec present");
        assert_eq!(
            synergy.key, "synergy-apps",
            "fleet gh_repo basename is authoritative for PR attribution"
        );
        assert_eq!(
            synergy.issues_key.as_deref(),
            Some("synergy"),
            "issues still grouped by the fleet name"
        );
    }
}
