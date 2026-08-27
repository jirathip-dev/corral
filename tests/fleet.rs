//! #237 configless test suite: the `corrald fleet switch` delegation
//! surface and the fleet-ops CLI identity parse, exercised with the real
//! binary against a fake fleet-ops CLI (`$CORRALD_FLEET_OPS`).
//!
//! Corral no longer owns, reads, or writes `fleets.json`; the registry
//! views/mutations are fleet-ops' (`herdr-fleet`). `corrald fleet` keeps
//! exactly one subcommand — `switch` — which delegates the auth-gated
//! re-arm to the fleet-ops CLI validated identity path.

use std::process::Command;
use std::sync::atomic::{AtomicU8, Ordering};

use corrald::fleet::cli::{FleetOpsError, parse_fleet_list};

static N: AtomicU8 = AtomicU8::new(0);

/// Serializes tests that mutate `CORRALD_FLEET_OPS`: env mutation is
/// process-wide while the spawned corrald reads it in its own process.
static FLEET_OPS_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Write a fake fleet-ops CLI that records its argv and exits with the
/// requested code, then return (path, env_guard).
fn fake_fleet_ops(exit: i32) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().expect("temp dir");
    let script = dir.path().join(format!(
        "fake-fleet-ops-{}",
        N.fetch_add(1, Ordering::SeqCst)
    ));
    let body = format!(
        "#!/bin/sh\n\necho \"$@\" > \"{}/argv\"\nexit {exit}\n",
        dir.path().display()
    );
    std::fs::write(&script, body).expect("write fake fleet-ops");
    // chmod +x
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perm = std::fs::metadata(&script).unwrap().permissions();
        perm.set_mode(0o755);
        std::fs::set_permissions(&script, perm).expect("chmod fake fleet-ops");
    }
    (dir, script)
}

struct EnvRestore {
    name: &'static str,
    previous: Option<std::ffi::OsString>,
}

impl EnvRestore {
    fn set(name: &'static str, value: &std::path::Path) -> Self {
        let previous = std::env::var_os(name);
        unsafe { std::env::set_var(name, value) };
        Self { name, previous }
    }
}

impl Drop for EnvRestore {
    fn drop(&mut self) {
        match self.previous.take() {
            Some(previous) => unsafe { std::env::set_var(self.name, previous) },
            None => unsafe { std::env::remove_var(self.name) },
        }
    }
}

fn corrald(args: &[&str]) -> i32 {
    Command::new(env!("CARGO_BIN_EXE_corrald"))
        .args(args)
        .output()
        .expect("spawn corrald")
        .status
        .code()
        .unwrap_or(-1)
}

#[test]
fn fleet_switch_delegates_to_the_fleet_ops_cli_and_propagates_exit_zero() {
    let _lock = FLEET_OPS_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (_dir, script) = fake_fleet_ops(0);
    let _guard = EnvRestore::set("CORRALD_FLEET_OPS", &script);
    let code = corrald(&["fleet", "switch", "corral"]);
    assert_eq!(code, 0, "delegated switch success mirrors exit 0");
    let argv = std::fs::read_to_string(_dir.path().join("argv")).expect("argv recorded");
    assert_eq!(
        argv.trim(),
        "switch corral",
        "the fleet-ops CLI receives exactly the switch invocation"
    );
}

#[test]
fn fleet_switch_passes_pane_through_and_propagates_failure_exit_one() {
    let _lock = FLEET_OPS_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (_dir, script) = fake_fleet_ops(1);
    let _guard = EnvRestore::set("CORRALD_FLEET_OPS", &script);
    let code = corrald(&["fleet", "switch", "corral", "--pane", "wM:p1"]);
    assert_eq!(code, 1, "delegated switch failure mirrors exit 1");
    let argv = std::fs::read_to_string(_dir.path().join("argv")).expect("argv recorded");
    assert_eq!(
        argv.trim(),
        "switch corral --pane wM:p1",
        "--pane passes through verbatim"
    );
}

#[test]
fn fleet_switch_requires_a_name_and_rejects_unknown_subcommands() {
    assert_eq!(corrald(&["fleet", "switch"]), 2, "missing name is usage");
    assert_eq!(
        corrald(&["fleet", "list"]),
        2,
        "registry subcommands are gone"
    );
    assert_eq!(
        corrald(&["fleet", "reap", "all"]),
        2,
        "reap stopped being a corrald fleet operation"
    );
    assert_eq!(
        corrald(&["fleet", "add", "x", "--gh", "a/b"]),
        2,
        "registry writes are fleet-ops' job"
    );
}

#[test]
fn fleet_parse_of_real_table_shape() {
    let sample = "corral       ✓  jirathip-dev/corral                                orch=orch-corral workers=2 PAUSED models=orch:glm-5.3\n";
    let fleets = parse_fleet_list(sample).expect("parses");
    assert_eq!(fleets.len(), 1);
    assert_eq!(fleets[0].name, "corral");
    assert_eq!(fleets[0].gh_repo, "jirathip-dev/corral");
    assert_eq!(fleets[0].orch, "orch-corral");
    assert_eq!(fleets[0].workers, 2);
    assert!(fleets[0].paused);
    assert!(matches!(
        parse_fleet_list(""),
        Err(FleetOpsError::Unavailable { .. })
    ));
}
