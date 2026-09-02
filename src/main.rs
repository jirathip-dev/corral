//! Corral host daemon (`corrald`) binary entrypoint.
//!
//! Subcommands:
//! - (default) run the daemon: `corrald [--socket <path>] [--port <n>] [--bind <ip>]`
//! - D33 digest, offline against the history ring:
//!   `corrald digest [--since <epoch-millis>] [--config-dir <path>]`
//!   (the cron/launchd artifact — see `crate::history` for the hook).
//! - The registry views and mutations belong to the private fleet tool.

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

    let (socket_path, addr, cors_origins) = parse_args(&args);
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");

    runtime.block_on(async_main(socket_path, addr, cors_origins));
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

fn parse_args(args: &[String]) -> (PathBuf, SocketAddr, Vec<String>) {
    let mut socket: Option<String> = None;
    let mut port: Option<u16> = None;
    let mut bind = DEFAULT_BIND.to_string();
    let mut cors_origins: Vec<String> = Vec::new();
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
            // #215: exact-origin allowlist for the read plane's CORS
            // headers (repeatable). Also read from $CORRALD_CORS_ORIGIN
            // (comma-separated) when no flag is given; the flag and the
            // env are never merged.
            "--cors-origin" => {
                cors_origins.push(iter.next().cloned().unwrap_or_default());
            }
            "--help" | "-h" => {
                println!(
                    "corrald — agent-fleet control plane daemon (P1)\n\n\
                     USAGE: corrald [--socket <path>] [--port <n>] [--bind <ip>]\n\
                     USAGE: corrald [--cors-origin <origin>]…\n\
                     USAGE: corrald digest [--since <epoch-millis>] [--config-dir <path>]\n\
                     USAGE: corrald fleet switch <name> [--pane <id>]\n\n\
                     --socket  herdr API socket (default ~/.config/herdr/herdr.sock)\n\
                     --port    HTTP port (default {DEFAULT_PORT})\n\
                     --bind    bind address (default {DEFAULT_BIND}); loopback,\n\
                     private (RFC 1918), Tailscale/CGNAT 100.64/10, and IPv6\n\
                     unique-local are permitted — public IPs and 0.0.0.0 are\n\
                     refused. Writes are device-signed on every interface;\n\
                     READS are credential-free, so beyond loopback the bound\n\
                     network itself is the read boundary (prefer a tailnet)\n\
                     --cors-origin  exact browser origin allowed to READ the\n\
                     credential-free read plane (/healthz, /snapshot,\n\
                     /events, /history, /issues) — repeatable, or\n\
                     set $CORRALD_CORS_ORIGIN (comma-separated). `*` is\n\
                     refused. The write plane (/drive, auth) never gets\n\
                     CORS headers; only the read routes above do. Empty\n\
                     (default) = no CORS headers at all."
                );
                std::process::exit(0);
            }
            other => {
                eprintln!("unknown argument: {other} (see --help)");
                std::process::exit(2);
            }
        }
    }
    if cors_origins.is_empty()
        && let Ok(values) = std::env::var("CORRALD_CORS_ORIGIN")
    {
        cors_origins = values
            .split(',')
            .map(str::trim)
            .filter(|origin| !origin.is_empty())
            .map(str::to_string)
            .collect();
    }
    for origin in &cors_origins {
        if origin == "*" {
            eprintln!(
                "corrald: refusing `*` as --cors-origin — CORS is an exact\n\
                 allowlist, never a wildcard (see #215)"
            );
            std::process::exit(2);
        }
        if !origin_valid(origin) {
            eprintln!(
                "corrald: --cors-origin {origin:?} is not a valid origin\n\
                 (expected scheme://host[:port], e.g. https://user.github.io)"
            );
            std::process::exit(2);
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
    (socket_path, addr, cors_origins)
}

/// #215: an origin is `scheme://host[:port]` with no trailing slash or
/// path. The daemon compares it byte-for-byte against the request's
/// `Origin` header (the value comes from the CLI/env, never from a
/// hostile client), so validation here only needs to keep the list
/// syntactically sane and `*`-free.
fn origin_valid(origin: &str) -> bool {
    let Some(rest) = origin
        .strip_prefix("https://")
        .or_else(|| origin.strip_prefix("http://"))
    else {
        return false;
    };
    !rest.is_empty() && !rest.contains('/') && !rest.contains("://")
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

async fn async_main(socket_path: PathBuf, addr: SocketAddr, cors_origins: Vec<String>) {
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

    if cors_origins.is_empty() {
        tracing::info!("CORS read plane disabled (no --cors-origin / $CORRALD_CORS_ORIGIN)");
    } else {
        tracing::info!(origins = ?cors_origins, "CORS read plane enabled for allowlisted origins only");
    }
    let app = corrald::api::router(AppState {
        store,
        auth,
        adapter,
        replay: Arc::new(ReplayTable::default()),
        issues: issues_cache.clone(),
        provenance: Arc::new(corrald::core::provenance::PromptProvenance::new()),

        cors_origins,
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
    let mut backoff = INTEGRATOR_RECONNECT_BASE;
    loop {
        // Fresh plane instances per generation (re-review R1/R2): a re-armed
        // GitPlane must boot with an EMPTY registry so the boot rescan
        // re-emits every worktree fact into the new integrator's empty
        // caches (a reused instance would diff against retained facts and
        // emit nothing until the next real change), and the per-instance
        // stopped flag must not couple generations (a lingering old
        // watcher's sink failure must not kill the new watcher too).
        let git_plane: Arc<dyn Plane> = Arc::new(
            GitPlane::with_repo_roots_and_discovery(
                vec![fallback_repo_root.clone()],
                attribution.worktrees_root(),
                source_discovery.clone(),
            )
            .with_backlog_flag(store.git_plane_backlog()),
        );
        let gh_plane: Arc<dyn Plane> = Arc::new(GhPlane::with_herdr_scope(
            Arc::new(store.clone()),
            attribution.clone(),
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
    use super::{bind_permitted, origin_valid};
    use corrald::adapters::gh_plane::{github_origin, herdr_workspace_specs};
    use corrald::api::issues::IssuesCache;
    use corrald::core::events::{GhIssueRef, GhRepoState, PlaneEvent, plane_channel};
    use corrald::core::model::{Agent, AgentState, Change, Workspace};
    use corrald::core::store::Store;
    use corrald::core::workspace::{RepoRoot, WorkspaceAttribution};
    use corrald::integrate::Integrator;
    use std::net::IpAddr;
    use std::sync::Arc;

    fn ip(s: &str) -> IpAddr {
        s.parse().expect("test ip parses")
    }

    fn herdr_agent(id: &str, path: &std::path::Path) -> Agent {
        Agent {
            agent_id: id.to_string(),
            source: "herdr".to_string(),
            tool: "fixture".to_string(),
            state: AgentState::Idle,
            reason: None,
            seq: 1,
            ts: 0,
            capabilities: Vec::new(),
            waiting_on: None,
            parent_id: None,
            host: None,
            workspace: Workspace {
                worktree_path: Some(path.to_string_lossy().into_owned()),
                ..Default::default()
            },
            attachment: None,
            display_name: None,
            title: None,
        }
    }

    #[tokio::test]
    async fn live_origin_repo_reaches_issue_cache_through_production_seam() {
        let root = std::env::temp_dir().join(format!("corral-g207-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let checkout = root.join("synergy-costing");
        let repository = git2::Repository::init(&checkout).unwrap();
        repository
            .remote(
                "origin",
                "https://github.com/synergy-services-cooling-tower/synergy-apps.git",
            )
            .unwrap();

        assert_eq!(
            github_origin(&checkout),
            Some((
                "synergy-services-cooling-tower".to_string(),
                "synergy-apps".to_string()
            ))
        );
        assert!(checkout.join(".git").exists());
        let store = Store::new();
        store
            .apply(Change::upsert(herdr_agent("live", &checkout)))
            .await;
        let attribution = WorkspaceAttribution::from_roots(
            [RepoRoot {
                path: checkout.clone(),
                repo: "synergy-costing".to_string(),
            }],
            root.join("worktrees"),
        );
        let specs = herdr_workspace_specs(&store, &attribution).await;
        let spec = specs
            .iter()
            .find(|spec| spec.aliases.contains(&"synergy-costing".to_string()))
            .expect("live origin repo is included in production gh specs");
        assert_eq!(spec.owner, "synergy-services-cooling-tower");
        assert_eq!(spec.name, "synergy-apps");
        assert!(spec.aliases.contains(&"synergy-costing".to_string()));

        let issues = Arc::new(IssuesCache::default());
        let integrator =
            Integrator::with_issues(store.clone(), attribution.clone(), issues.clone());
        let (sink, receiver) = plane_channel();
        let task = tokio::spawn(integrator.run(receiver));
        sink.send(PlaneEvent::Gh(GhRepoState {
            repo: "synergy-costing".to_string(),
            default_branch: "main".to_string(),
            issues: vec![GhIssueRef {
                repo: "synergy-costing".to_string(),
                number: 207,
                state: "OPEN".to_string(),
                title: "hydrate live repo".to_string(),
                labels: vec![],
                url: String::new(),
                body: None,
                comments: vec![],
                comment_total: None,
            }],
            ..Default::default()
        }))
        .await
        .unwrap();
        for _ in 0..20 {
            if issues.get("synergy-costing", 207).is_some() {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert!(issues.get("synergy-costing", 207).is_some());
        drop(sink);
        task.await.unwrap();
        std::fs::remove_dir_all(checkout.parent().unwrap()).unwrap();
    }

    #[tokio::test]
    async fn same_remote_basename_groups_specs_without_native_key_collisions() {
        let root =
            std::env::temp_dir().join(format!("corral-g207-collision-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        for (directory, remote) in [
            (
                "sendmeter-services",
                "git@github.com:sendmeter/sendmeter.git",
            ),
            (
                "sendmeter-jirathip",
                "git@github.com:jirathip-dev/sendmeter.git",
            ),
        ] {
            let checkout = root.join(directory);
            let repository = git2::Repository::init(&checkout).unwrap();
            repository.remote("origin", remote).unwrap();
        }

        let store = Store::new();
        store
            .apply(Change::upsert(herdr_agent(
                "services",
                &root.join("sendmeter-services"),
            )))
            .await;
        store
            .apply(Change::upsert(herdr_agent(
                "jirathip",
                &root.join("sendmeter-jirathip"),
            )))
            .await;
        let attribution = WorkspaceAttribution::from_roots(
            [
                RepoRoot {
                    path: root.join("sendmeter-services"),
                    repo: "sendmeter-services".to_string(),
                },
                RepoRoot {
                    path: root.join("sendmeter-jirathip"),
                    repo: "sendmeter-jirathip".to_string(),
                },
            ],
            root.join("worktrees"),
        );
        let specs = herdr_workspace_specs(&store, &attribution).await;
        let matching: Vec<_> = specs
            .iter()
            .filter(|spec| spec.name == "sendmeter")
            .collect();
        assert_eq!(matching.len(), 2, "one spec per canonical GitHub slug");
        assert_eq!(
            matching
                .iter()
                .filter(|spec| spec.aliases.contains(&"sendmeter-services".to_string()))
                .count(),
            1,
        );
        assert_eq!(
            matching
                .iter()
                .filter(|spec| spec.aliases.contains(&"sendmeter-jirathip".to_string()))
                .count(),
            1,
        );

        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn scoped_specs_use_only_live_herdr_workspaces() {
        let root = std::env::temp_dir().join(format!("corral-g332-scope-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        for (directory, remote) in [
            ("sendmeter", "git@github.com:jirathip-dev/sendmeter.git"),
            (
                "synergy-services-website",
                "git@github.com:synergy-services-cooling-tower/synergy-services-website.git",
            ),
        ] {
            let checkout = root.join(directory);
            let repository = git2::Repository::init(&checkout).unwrap();
            repository.remote("origin", remote).unwrap();
        }

        let store = Store::new();
        store
            .apply(Change::upsert(herdr_agent(
                "only-live",
                &root.join("sendmeter"),
            )))
            .await;
        let attribution = WorkspaceAttribution::from_roots(
            [
                RepoRoot {
                    path: root.join("sendmeter"),
                    repo: "sendmeter".to_string(),
                },
                RepoRoot {
                    path: root.join("synergy-services-website"),
                    repo: "synergy-services-website".to_string(),
                },
            ],
            root.join("worktrees"),
        );
        let specs = herdr_workspace_specs(&store, &attribution).await;
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].slug(), "jirathip-dev/sendmeter");
        assert_eq!(specs[0].aliases, vec!["sendmeter"]);
        std::fs::remove_dir_all(root).unwrap();
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

    /// #215: the CORS allowlist accepts exact `scheme://host[:port]`
    /// origins and nothing wildcard/pathful — `*` is refused upstream.
    #[test]
    fn origin_allowlist_accepts_exact_origins_and_refuses_paths() {
        for allowed in [
            "https://user.github.io",
            "https://user.github.io:8443",
            "http://127.0.0.1:8000",
        ] {
            assert!(origin_valid(allowed), "{allowed} should be accepted");
        }
        for refused in [
            "*",
            "https://github.io/path",
            "localhost:8000",
            "ftp://x",
            "",
        ] {
            assert!(!origin_valid(refused), "{refused:?} should be refused");
        }
    }
}
