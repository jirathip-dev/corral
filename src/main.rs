//! Corral host daemon (`corrald`) binary entrypoint.
//!
//! Subcommands:
//! - (default) run the daemon: `corrald [--socket <path>] [--port <n>] [--bind <ip>]`
//! - D33 digest, offline against the history ring:
//!   `corrald digest [--since <epoch-millis>] [--config-dir <path>]`
//!   (the cron/launchd artifact — see `crate::history` for the hook).
//! - #35 phase 1: fleet registry views AND writes:
//!   `corrald fleet list|check` (read-only),
//!   `corrald fleet add|remove` (registry CRUD, atomic write), and
//!   `corrald fleet pause|resume|models` (registry mutation, atomic write)
//!   (see `crate::fleet` for the registry format and validation).

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
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
use corrald::core::workspace::{RepoRoot, WorkspaceAttribution};
use corrald::fleet;
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
/// side: `list`, `check`. Write side (slice 1): `add`, `remove`; (slice 2):
/// `pause`, `resume`, `models` — all behind atomic-write discipline and
/// validation in [`crate::fleet::ops`]. All accept `--registry <path>` to
/// override `$CORRAL_FLEETS_PATH` / `$CORRAL_CONFIG_DIR/fleets.json`
/// (default `~/.config/corral/fleets.json`; legacy
/// `~/.hermes/scripts/fleets.json` honoured as a fallback — #66).
/// Everything here runs before the tokio runtime is built; no subcommand
/// touches a running daemon or the herdr socket.
fn run_fleet(args: &[String]) {
    let Some(sub) = args.first().map(String::as_str) else {
        eprintln!(
            "corrald fleet: need a subcommand: list | check | add | remove | pause | resume | models | watch (see --help)"
        );
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
        "pause" => run_fleet_pause_resume("pause", &args[1..]),
        "resume" => run_fleet_pause_resume("resume", &args[1..]),
        "models" => run_fleet_models(&args[1..]),
        "watch" => run_fleet_watch(&args[1..]),
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
         USAGE: corrald fleet add <name> --gh <owner/repo> [--local <path>]\n\
         \t[--worktree <path>] [--orch <agent>] [--workers a,b,c]\n\
         \t[--models orch=..,impl=..,review=..] [--registry <path>]\n\
         \t(<name> may also be passed as --name; --worktree-dir is an\n\
         \talias for --worktree — both match the legacy fleet CLI)\n\
         USAGE: corrald fleet remove <name> [--registry <path>]\n\
         USAGE: corrald fleet watch [--registry <path>]\n\
         USAGE: corrald fleet pause <name> [--registry <path>]\n\
         USAGE: corrald fleet resume <name> [--registry <path>]\n\
         USAGE: corrald fleet models <name> [--orch M] [--impl M] [--impl-alt M]\n\
         \t[--impl-alt2 M] [--review M] [--registry <path>]\n\n\
         list     one line per fleet: name, gh_repo, worker count,\n\
         \tpaused flag, and the three model ids\n\
         check    parse + validate, then verify each fleet's local\n\
         \tdir exists and holds a .git entry; exit 0 when every\n\
         \tfleet checks out, 1 when any fails, 2 on usage/parse error\n\
         add      resolve the repo via `gh repo view`, validate the\n\
         \tcandidate registry, then atomically insert the fleet;\n\
         \tdefaults: local ~/Projects/<name>, worktree_dir <name>,\n\
         \torch orch-<name>, workers empty, models inherited from the\n\
         \tfirst existing fleet (or required via --models on an empty\n\
         \tregistry). The registry file must exist — bootstrap one\n\
         \t(a fresh machine lacks the parent dir, #66) with:\n\
         \t  mkdir -p ~/.config/corral\n\
         \t  echo '{{\"fleets\": []}}' > <path>\n\
         remove   atomically drop exactly one fleet by name\n\
         pause    set paused:true on exactly one fleet; pausing an\n\
         \talready-paused fleet is a no-op success (exit 0)\n\
         resume   clear paused on exactly one fleet; resuming an\n\
         \tunpaused fleet is a no-op success (exit 0)\n\
         models   update only the model slots named; <name> may be\n\
         \t`all` to apply to every fleet (models only — pause/resume\n\
         \ttake a real fleet name; `all` is reserved as a fleet name).\n\
         \t--impl-alt '' / --impl-alt2 '' CLEAR that optional slot; an\n\
         \tempty value for the required orch/impl/review slots is a\n\
         \tusage error\n\
         watch    one READ-ONLY health pass over unpaused fleets: herdr\n\
         \tserver reachability, missing orchestrators, stall flavors\n\
         \t(open PRs / workers still working / plain), missing workers;\n\
         \tprints PROBLEM lines or ALL HEALTHY; exit 0 healthy /\n\
         \t1 problems (an unreadable/invalid registry is itself a\n\
         \tPROBLEM line with exit 1 — NOT check's exit 2, so a cron\n\
         \tmonitor still alerts) / 2 usage error (cron-able, like\n\
         \tdigest)\n\n\
         add/remove/pause/resume/models exit codes: 0 = written (or an\n\
         \tidempotent no-op — already paused/resumed, models unchanged);\n\
         \t1 = refused (duplicate/unresolvable repo/unknown name) or the\n\
         \twrite failed — the registry is left byte-identical;\n\
         \t2 = usage error or unreadable/unparseable/invalid registry\n\n\
         \t--registry   fleet registry JSON (default $CORRAL_FLEETS_PATH\n\
         \tor $CORRAL_CONFIG_DIR/fleets.json, default\n\
         \t~/.config/corral/fleets.json; a pre-existing legacy\n\
         \t~/.hermes/scripts/fleets.json is used as a fallback,\n\
         \twith a stderr note)"
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
            // `--worktree` is the legacy fleet CLI's spelling (#35 design
            // principle 2: same names and semantics); `--worktree-dir`
            // matches the registry field name. Both are accepted.
            "--worktree" | "--worktree-dir" => {
                worktree_dir = value();
                if worktree_dir.is_none() {
                    usage("fleet add: --worktree needs a value");
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
            other => {
                // The legacy fleet CLI takes the name as a positional
                // (`fleet add <name> --gh o/r`); accept that shape too.
                if other.starts_with('-') {
                    usage(&format!("fleet add: unknown argument: {other}"));
                }
                if name.is_some() {
                    usage("fleet add: exactly one fleet name");
                }
                name = Some(other.to_string());
            }
        }
        i += 1;
    }
    let Some(name) = name else {
        usage("fleet add: a fleet name is required (positional or --name)");
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
    // The resolver is deliberately NOT injectable at the CLI layer (reviewed,
    // deferred): the refusal paths are covered through `ops::add` with a stub
    // resolver and the parse-layer refusals are covered e2e; the `fleet add`
    // SUCCESS path through the real binary (resolve → write → success print)
    // remains uncovered end-to-end because it would need the real `gh`.
    // Revisit if the resolver ever becomes injectable here.
    let fleet = match fleet::ops::add(&path, &opts, &fleet::ops::GhCli) {
        Ok(fleet) => fleet,
        Err(error) => {
            // Exit contract: 1 = the operation was refused or the write
            // failed; 2 = usage/parse/validation (see ConfigError::exit_code).
            eprintln!("corrald fleet add: {error}");
            std::process::exit(error.exit_code());
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
            std::process::exit(error.exit_code());
        }
    }
}

/// `corrald fleet pause|resume <name>`: set/clear the fleet's `paused` flag,
/// atomically. Idempotent: pausing a paused (or resuming an unpaused) fleet
/// is a no-op SUCCESS that says so, exit 0. An unknown name is a refusal
/// (exit 1) that leaves the file byte-identical.
fn run_fleet_pause_resume(sub: &str, args: &[String]) {
    let mut registry: Option<PathBuf> = None;
    let mut name: Option<String> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--registry" => {
                i += 1;
                registry = args.get(i).map(PathBuf::from);
                if registry.is_none() {
                    usage(&format!("fleet {sub}: --registry needs a value"));
                }
            }
            "--help" | "-h" => {
                print_fleet_help();
                std::process::exit(0);
            }
            other => {
                if other.starts_with('-') {
                    usage(&format!("fleet {sub}: unknown argument: {other}"));
                }
                if name.is_some() {
                    usage(&format!("fleet {sub}: exactly one fleet name"));
                }
                name = Some(other.to_string());
            }
        }
        i += 1;
    }
    let Some(name) = name else {
        usage(&format!("fleet {sub}: need a fleet name"));
    };
    let path = registry.unwrap_or_else(fleet::config::default_path);
    let result = match sub {
        "pause" => fleet::ops::pause(&path, &name),
        "resume" => fleet::ops::resume(&path, &name),
        _ => unreachable!(),
    };
    match result {
        Ok(true) => {
            let verb = if sub == "pause" { "paused" } else { "resumed" };
            println!("{verb} fleet {name}");
        }
        Ok(false) => {
            let message = if sub == "pause" {
                format!("fleet {name} already paused")
            } else {
                format!("fleet {name} not paused")
            };
            println!("{message} — nothing to do");
        }
        Err(error) => {
            eprintln!("corrald fleet {sub}: {error}");
            std::process::exit(error.exit_code());
        }
    }
}

/// `corrald fleet models <name> [--orch M] [--impl M] [--impl-alt M]
/// [--impl-alt2 M] [--review M]`: update only the model slots named; `<name>`
/// may be `all` (every fleet). `--impl-alt ''` / `--impl-alt2 ''` CLEAR that
/// optional slot; empty values for the required slots are a usage error.
fn run_fleet_models(args: &[String]) {
    let mut registry: Option<PathBuf> = None;
    let mut name: Option<String> = None;
    let mut orch: Option<String> = None;
    let mut impl_: Option<String> = None;
    let mut impl_alt: Option<String> = None;
    let mut impl_alt2: Option<String> = None;
    let mut review: Option<String> = None;
    let mut i = 0;
    while i < args.len() {
        let arg = args[i].as_str();
        let mut value = || {
            i += 1;
            args.get(i).cloned()
        };
        match arg {
            "--orch" => {
                orch = value();
                if orch.is_none() {
                    usage("fleet models: --orch needs a value");
                }
            }
            "--impl" => {
                impl_ = value();
                if impl_.is_none() {
                    usage("fleet models: --impl needs a value");
                }
            }
            "--impl-alt" => {
                impl_alt = value();
                if impl_alt.is_none() {
                    usage("fleet models: --impl-alt needs a value");
                }
            }
            "--impl-alt2" => {
                impl_alt2 = value();
                if impl_alt2.is_none() {
                    usage("fleet models: --impl-alt2 needs a value");
                }
            }
            "--review" => {
                review = value();
                if review.is_none() {
                    usage("fleet models: --review needs a value");
                }
            }
            "--registry" => {
                i += 1;
                registry = args.get(i).map(PathBuf::from);
                if registry.is_none() {
                    usage("fleet models: --registry needs a value");
                }
            }
            "--help" | "-h" => {
                print_fleet_help();
                std::process::exit(0);
            }
            other => {
                if other.starts_with('-') {
                    usage(&format!("fleet models: unknown argument: {other}"));
                }
                if name.is_some() {
                    usage("fleet models: exactly one fleet name");
                }
                name = Some(other.to_string());
            }
        }
        i += 1;
    }
    let Some(name) = name else {
        usage("fleet models: need a fleet name (or `all`)");
    };
    if orch.is_none()
        && impl_.is_none()
        && impl_alt.is_none()
        && impl_alt2.is_none()
        && review.is_none()
    {
        usage("fleet models: pass at least one of --orch/--impl/--impl-alt/--impl-alt2/--review");
    }
    // Empty values are usage errors for the REQUIRED slots (only the
    // optional --impl-alt/--impl-alt2 accept '' to clear). Caught here so
    // the message is a plain usage refusal, not a registry-shaped error.
    for (flag, value) in [("--orch", &orch), ("--impl", &impl_), ("--review", &review)] {
        if let Some(value) = value
            && value.is_empty()
        {
            usage(&format!(
                "fleet models: {flag} must be non-empty (only --impl-alt/--impl-alt2 accept '' to clear)"
            ));
        }
    }
    let path = registry.unwrap_or_else(fleet::config::default_path);

    let update = fleet::ops::ModelUpdate {
        orch,
        impl_,
        impl_alt,
        impl_alt2,
        review,
    };
    let changes = match fleet::ops::models(&path, &name, &update) {
        Ok(changes) => changes,
        Err(error) => {
            eprintln!("corrald fleet models: {error}");
            std::process::exit(error.exit_code());
        }
    };
    // Idempotent no-op (ops wrote nothing): say so instead of printing
    // misleading `x -> x` lines.
    if changes.iter().all(|c| c.before == c.after) {
        let names: Vec<&str> = changes.iter().map(|c| c.name.as_str()).collect();
        println!("models unchanged for {} — nothing to do", names.join(", "));
        return;
    }
    // Print what changed (old -> new), per fleet, so the operator can see
    // exactly which slots moved and confirm the untouched ones did not.
    for change in &changes {
        println!(
            "{} models changed: orch {} -> {}; impl {} -> {}; impl_alt {} -> {}; impl_alt2 {} -> {}; review {} -> {}",
            change.name,
            change.before.orch,
            change.after.orch,
            change.before.impl_,
            change.after.impl_,
            change.before.impl_alt.as_deref().unwrap_or("-"),
            change.after.impl_alt.as_deref().unwrap_or("-"),
            change.before.impl_alt2.as_deref().unwrap_or("-"),
            change.after.impl_alt2.as_deref().unwrap_or("-"),
            change.before.review,
            change.after.review
        );
    }
}

/// `corrald fleet watch`: one read-only health pass over the registry's
/// fleets — herdr reachability, missing orchestrators, stall flavors,
/// missing workers (see [`fleet::watch`]). Exit 0 healthy / 1 problems —
/// an unreadable/invalid registry is itself a PROBLEM line with exit 1
/// (monitor safety; deliberately NOT `check`'s exit-2) / 2 usage errors.
fn run_fleet_watch(args: &[String]) {
    let mut registry: Option<PathBuf> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--registry" => {
                i += 1;
                registry = args.get(i).map(PathBuf::from);
                if registry.is_none() {
                    usage("fleet watch: --registry needs a value");
                }
            }
            "--help" | "-h" => {
                print_fleet_help();
                std::process::exit(0);
            }
            other => usage(&format!("fleet watch: unknown argument: {other}")),
        }
        i += 1;
    }
    let path = registry.unwrap_or_else(fleet::config::default_path);
    let registry = match fleet::config::load(&path) {
        Ok(registry) => registry,
        Err(error) => {
            // Monitor safety (legacy-hardened behavior, review F1): the
            // watchdog must ALERT on the failure that stops it watching,
            // on STDOUT where the cron consumer looks — never die
            // silently to stderr. Exit 1 = "problems found" (the invalid
            // registry IS the problem), keeping the 0/1/2 contract
            // unambiguous (review F2).
            println!("PROBLEM: fleet registry unreadable or corrupt: {error}");
            std::process::exit(1);
        }
    };

    let home = std::env::var("HOME").unwrap_or_default();
    let agents = herdr_agents_with_retry();
    // Fresh-review N7: only fleets whose orchestrator is PRESENT and not
    // working can reach a stall arm (the exact predicate problems()
    // applies), and only stall arms read PR counts — so the healthy
    // steady state makes ZERO gh calls instead of one per repo (up to
    // 30s each, serial). A fleet that skips the query here can never
    // have its count read, so the pr_note semantics are unchanged.
    let mut prs = fleet::watch::PrCounts::new();
    if let Some(agents) = &agents {
        let mut repos: Vec<&str> = registry
            .fleets
            .iter()
            .filter(|f| !f.paused)
            .filter(|f| {
                agents
                    .get(&f.orch)
                    .is_some_and(|orch| orch.status != "working")
            })
            .map(|f| f.gh_repo.as_str())
            .collect();
        repos.sort_unstable();
        repos.dedup();
        for repo in repos {
            prs.insert(repo.to_string(), open_pr_count(repo));
        }
    }

    let problems = fleet::watch::problems(&registry, &agents, &prs, &home);
    if problems.is_empty() {
        println!("ALL HEALTHY");
        return;
    }
    for problem in &problems {
        println!("PROBLEM: {problem}");
    }
    std::process::exit(1);
}

/// `herdr agent list` (JSON) with a 60s timeout and ONE retry after 10s —
/// a transient socket hiccup must not read as "every agent missing" (a
/// legacy-proven false-alarm mode). `None` = the CALL failed or was
/// unparseable after the retry; `Some(empty)` = healthy zero-agent
/// answer (review F3). An EMPTY first answer also gets the 10s retry
/// (review R1): legacy grants a just-restarted server that grace before
/// declaring every agent gone — only a second empty answer is returned
/// as the healthy `Some(empty)`, never as server-down. Stdout is parsed
/// regardless of the child's exit status — the parse decides (review F9).
fn herdr_agents_with_retry() -> fleet::watch::AgentsView {
    // The invariant: the LAST successful answer wins; server-down (None)
    // only when no answer was ever obtained (review R1/S1a).
    let mut last_good: fleet::watch::AgentsView = None;
    for attempt in 0..2 {
        if attempt == 1 {
            std::thread::sleep(Duration::from_secs(10));
        }
        let Some(stdout) = run_with_timeout("herdr", &["agent", "list"], Duration::from_secs(60))
        else {
            continue;
        };
        let Some(map) = fleet::watch::parse_agent_listing(&stdout) else {
            continue;
        };
        if !map.is_empty() || attempt == 1 {
            return Some(map);
        }
        // An empty first answer: hold it (it IS a good answer), but grant
        // the retry before reporting — the server may just be restarting.
        last_good = Some(map);
    }
    last_good
}

/// Open-PR count for one repo via `gh`, 30s timeout. `None` = the check
/// was unavailable (network/auth) — surfaced in the stall wording, never
/// silently treated as zero.
fn open_pr_count(repo: &str) -> Option<u64> {
    let stdout = run_with_timeout(
        "gh",
        &[
            "pr", "list", "--repo", repo, "--state", "open", "--json", "number", "--jq", "length",
        ],
        Duration::from_secs(30),
    )?;
    stdout.trim().parse().ok()
}

/// Run a command with a wall-clock timeout (std has none): spawn with a
/// dedicated reader thread draining stdout (so a child producing more
/// than the pipe buffer can still exit), poll `try_wait`, kill on expiry.
///
/// EVERY wait on the reader is itself deadline-bounded via a channel
/// (review F4): killing the child does NOT guarantee the pipe closes —
/// a grandchild that inherited the write end can hold it open — and an
/// unbounded `join()` would hang the watchdog, the worst failure mode
/// for a cron monitor. A stuck reader thread is abandoned (detached);
/// the process exits shortly after anyway.
///
/// `None` on spawn failure or timeout. The child's exit STATUS is
/// deliberately ignored (review F9, matching legacy): a complete stdout
/// next to a warning exit code is still usable — the caller's parse
/// decides.
fn run_with_timeout(program: &str, args: &[&str], timeout: Duration) -> Option<String> {
    use std::io::Read as _;
    let mut child = std::process::Command::new(program)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;
    let Some(mut stdout) = child.stdout.take() else {
        // No pipe handle (should not happen): don't leak a zombie.
        let _ = child.kill();
        let _ = child.wait();
        return None;
    };
    // Fresh-review N1: the reader PUBLISHES into shared state as it
    // reads, instead of sending one String only at EOF. A persistent
    // grandchild holding the pipe's write end means EOF never comes —
    // the old shape then threw away a complete, valid listing sitting
    // in the reader's local buffer and reported a FALSE server-down
    // (which suppresses every true per-fleet problem). Now the grace
    // expiry returns whatever has been read and the PARSE decides
    // (same F9 principle as the exit status): a truncated buffer fails
    // to parse and retries; a complete one is used. The no-hang
    // property (round-1 F4) is unchanged — every wait stays bounded.
    // Round-2 N8: accumulate BYTES and decode once at take time — a
    // per-chunk from_utf8_lossy silently replaced any multi-byte char
    // straddling an 8KiB boundary with U+FFFD, which is legal inside a
    // JSON string, so the corrupted listing PARSED and a live
    // orchestrator could read as MISSING (reviewer-reproduced with a
    // one-byte control). Interrupted reads retry instead of truncating.
    let buffer = std::sync::Arc::new(std::sync::Mutex::new(Vec::<u8>::new()));
    let writer = buffer.clone();
    let (tx, rx) = std::sync::mpsc::channel::<()>();
    std::thread::spawn(move || {
        let mut chunk = [0u8; 8192];
        loop {
            match stdout.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => {
                    if let Ok(mut out) = writer.lock() {
                        out.extend_from_slice(&chunk[..n]);
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => break,
            }
        }
        let _ = tx.send(());
    });
    let deadline = std::time::Instant::now() + timeout;
    let take_buffer = |buffer: &std::sync::Arc<std::sync::Mutex<Vec<u8>>>| {
        buffer
            .lock()
            .ok()
            .map(|out| String::from_utf8_lossy(&out).into_owned())
    };
    loop {
        match child.try_wait() {
            Ok(Some(_status)) => {
                // Give a well-behaved pipe a moment to reach EOF after
                // child exit, but never block past a short grace even if
                // a grandchild holds the write end open — and return the
                // buffer EITHER WAY (N1).
                let _ = rx.recv_timeout(Duration::from_secs(5));
                return take_buffer(&buffer);
            }
            Ok(None) if std::time::Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(100)),
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
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
            key @ ("impl_alt" | "impl_alt2") => {
                // Deliberate (#56): the alt slots are schema fields but not
                // settable from --models — they inherit or are registry-edited.
                return Err(format!(
                    "--models {key:?} is not settable here; alt slots inherit \
                     from the first fleet — set them after add with \
                     `corrald fleet models <name> --impl-alt <model>`"
                ));
            }
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
        impl_alt: None,
        impl_alt2: None,
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
    let attribution = workspace_attribution(&repo_root, &worktrees_root);

    let adapter: Arc<dyn Adapter> = Arc::new(HerdrAdapter::new_with_attribution(
        socket_path.clone(),
        attribution.clone(),
    ));
    adapter.clone().start(store.clone());

    tokio::spawn(supervise_planes(store.clone(), attribution.clone()));
    tracing::info!(
        repo_roots = ?attribution.repo_roots(),
        worktrees_root = %attribution.worktrees_root().display(),
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
        transcript_roots: corrald::transcript::bind::TranscriptRoots::from_env(),
        transcript_limiter: corrald::api::transcript::TranscriptLimiter::default(),
        role_probe_memo: corrald::transcript::RoleProbeMemo::default(),
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

/// Build the explicit repo-root view shared by Herdr and the git/integrator
/// planes. The configured Corral root is always known; fleet-registry locals
/// add other primary checkouts when the registry exists. The registry's
/// `gh_repo` is the canonical repo identity, so agent names and pane labels
/// never participate in attribution. Registry roots are ordered first and
/// the configured root is appended as a fallback: when both spellings
/// canonicalize to one path, the fleet registry's `gh_repo` wins over the
/// configured directory basename.
fn workspace_attribution(repo_root: &Path, worktrees_root: &Path) -> WorkspaceAttribution {
    let registry_path = fleet::config::default_path();
    let registry = if registry_path.is_file() {
        match fleet::config::load(&registry_path) {
            Ok(registry) => Some(registry),
            Err(error) => {
                tracing::warn!(
                    path = %registry_path.display(),
                    error = %error,
                    "fleet registry unavailable for workspace attribution"
                );
                None
            }
        }
    } else {
        None
    };
    WorkspaceAttribution::from_roots(
        workspace_roots(repo_root, registry.as_ref()),
        worktrees_root.to_path_buf(),
    )
}

/// Return roots in attribution precedence order. Fleet registry identities
/// are canonical for a local checkout; the configured root's basename is only
/// a fallback when no registry entry claims the same canonical path.
fn workspace_roots(repo_root: &Path, registry: Option<&fleet::config::Registry>) -> Vec<RepoRoot> {
    let mut roots = Vec::new();
    if let Some(registry) = registry {
        for fleet in &registry.fleets {
            let Some(repo) = fleet.gh_repo.rsplit('/').next() else {
                continue;
            };
            roots.push(RepoRoot {
                path: fleet.local_path(),
                repo: repo.to_string(),
            });
        }
    }
    if let Some(name) = repo_root.file_name() {
        roots.push(RepoRoot {
            path: repo_root.to_path_buf(),
            repo: name.to_string_lossy().into_owned(),
        });
    }
    roots
}

/// WS3 F4: supervisor for the integrator task, mirroring the herdr
/// adapter's reconnect loop. A panicking integrator would drop the plane
/// channel receiver; both planes then stop on SinkClosed (gh by contract,
/// git via its sink-close exit) and facts would never merge again. The
/// supervisor therefore owns the channel and re-arms both planes per
/// generation. Residual: a previous generation's gh loop notices the dead
/// sink only at its next poll send, so a restart can briefly double-poll.
async fn supervise_planes(store: Store, attribution: WorkspaceAttribution) {
    let mut backoff = INTEGRATOR_RECONNECT_BASE;
    loop {
        // A replacement GitPlane has an empty registry and will re-observe
        // present worktrees during its boot/sweep. Drop branch values from
        // the previous generation first so a missed WorktreeRemoved cannot
        // make a vanished worktree look current to a fresh Herdr record.
        // Repo roots and linked-worktree layout remain intact and valid paths
        // regain their branches from the new plane's git facts.
        attribution.reset_branch_facts();
        // Fresh plane instances per generation (re-review R1/R2): a re-armed
        // GitPlane must boot with an EMPTY registry so the boot rescan
        // re-emits every worktree fact into the new integrator's empty
        // caches (a reused instance would diff against retained facts and
        // emit nothing until the next real change), and the per-instance
        // stopped flag must not couple generations (a lingering old
        // watcher's sink failure must not kill the new watcher too).
        let git_plane: Arc<dyn Plane> = Arc::new(GitPlane::with_repo_roots(
            attribution.repo_roots(),
            attribution.worktrees_root(),
        ));
        let gh_plane: Arc<dyn Plane> = Arc::new(GhPlane::new(Arc::new(store.clone())));
        let (sink, rx) = plane_channel();
        git_plane.start(sink.clone());
        gh_plane.start(sink.clone());
        let integrator = Integrator::new_with_attribution(store.clone(), attribution.clone());
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
    #[cfg(unix)]
    use super::workspace_roots;
    #[cfg(unix)]
    use corrald::core::workspace::WorkspaceAttribution;
    #[cfg(unix)]
    use corrald::fleet::config::{Fleet, Models, Registry};
    use std::net::IpAddr;

    fn ip(s: &str) -> IpAddr {
        s.parse().expect("test ip parses")
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

    #[cfg(unix)]
    #[test]
    fn fleet_registry_identity_wins_configured_canonical_alias() {
        let temp = tempfile::tempdir().expect("tempdir");
        let primary = temp.path().join("configured-directory-name");
        let alias = temp.path().join("fleet-alias");
        let worktrees = temp.path().join("worktrees");
        std::fs::create_dir_all(&primary).unwrap();
        std::fs::create_dir_all(&worktrees).unwrap();
        std::os::unix::fs::symlink(&primary, &alias).unwrap();
        let registry = Registry {
            fleets: vec![Fleet {
                name: "fleet-name".to_string(),
                gh_repo: "owner/canonical-repo".to_string(),
                local: alias.to_string_lossy().into_owned(),
                worktree_dir: "worktrees".to_string(),
                orch: "orch".to_string(),
                workers: Vec::new(),
                paused: false,
                models: Models {
                    orch: "orch-model".to_string(),
                    impl_: "impl-model".to_string(),
                    review: "review-model".to_string(),
                    impl_alt: None,
                    impl_alt2: None,
                },
            }],
        };

        let attribution =
            WorkspaceAttribution::from_roots(workspace_roots(&primary, Some(&registry)), worktrees);
        assert_eq!(
            attribution
                .facts_for(&primary)
                .expect("configured root facts")
                .repo
                .as_deref(),
            Some("canonical-repo")
        );
        assert_eq!(
            attribution
                .facts_for(&alias)
                .expect("canonical alias facts")
                .repo
                .as_deref(),
            Some("canonical-repo")
        );
    }
}
