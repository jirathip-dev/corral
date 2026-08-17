//! #35 phase 1 test suite: fleet registry config (parse, validate,
//! `local_path` expansion) and the `corrald fleet list|check` CLI surface,
//! exercised with the real binary against temp-dir fixtures.

use std::path::PathBuf;
use std::process::Command;

use corrald::fleet::config::{load, ConfigError, Fleet, Models, Registry};

const VALID_A: &str = r#"{
    "name": "corral",
    "gh_repo": "jirathip-k/corral",
    "local": "~/Projects/corral",
    "worktree_dir": "corral",
    "orch": "orch-corral",
    "workers": ["p4-w1", "p4-w1-reviewer"],
    "paused": true,
    "models": { "orch": "fable", "impl": "opencode-go/deepseek-v4-flash", "review": "opus" }
}"#;

const VALID_B: &str = r#"{
    "name": "board",
    "gh_repo": "jirathip-k/herdr-board",
    "local": "/opt/board",
    "worktree_dir": "board",
    "orch": "orch-board",
    "workers": [],
    "models": { "orch": "fable", "impl": "sonnet", "review": "opus" }
}"#;

/// A healthy `models` object used by the validation-failure fixtures below.
const MODELS: &str = r#"{"orch": "f", "impl": "i", "review": "r"}"#;

/// Single-fleet registry body with the given field values interpolated.
fn fleet_body(name: &str, repo: &str, local: &str, workers: &str, models: &str) -> String {
    format!(
        r#"{{ "fleets": [{{ "name": "{name}", "gh_repo": "{repo}", "local": "{local}",
            "worktree_dir": "w", "orch": "o", "workers": {workers}, "models": {models} }}] }}"#
    )
}

fn write_registry(dir: &std::path::Path, body: &str) -> PathBuf {
    let path = dir.join("fleets.json");
    std::fs::write(&path, body).expect("write registry fixture");
    path
}

fn registry(body: &str) -> Registry {
    let dir = tempfile::tempdir().expect("temp dir");
    load(&write_registry(dir.path(), body)).expect("registry loads")
}

#[test]
fn valid_registry_with_two_fleets_parses_with_defaults() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = write_registry(
        dir.path(),
        &format!(r#"{{ "fleets": [{VALID_A}, {VALID_B}] }}"#),
    );
    let registry = load(&path).expect("two-fleet registry loads");
    assert_eq!(registry.fleets.len(), 2);

    let a = &registry.fleets[0];
    assert_eq!(a.name, "corral");
    assert_eq!(a.gh_repo, "jirathip-k/corral");
    assert_eq!(a.local, "~/Projects/corral");
    assert_eq!(a.worktree_dir, "corral");
    assert_eq!(a.orch, "orch-corral");
    assert_eq!(a.workers, vec!["p4-w1", "p4-w1-reviewer"]);
    assert!(a.paused, "explicit paused: true lands");

    let b = &registry.fleets[1];
    assert!(b.workers.is_empty(), "empty workers allowed");
    assert!(!b.paused, "absent paused defaults to false");
    assert_eq!(b.models.orch, "fable");
    assert_eq!(b.models.review, "opus");
}

#[test]
fn models_impl_key_lands_in_impl_field() {
    let registry = registry(&format!(r#"{{ "fleets": [{VALID_A}] }}"#));
    let models: &Models = &registry.fleets[0].models;
    assert_eq!(models.impl_, "opencode-go/deepseek-v4-flash");
    assert_eq!(models.orch, "fable");
    assert_eq!(models.review, "opus");
}

#[test]
fn unknown_field_is_a_hard_error() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = write_registry(
        dir.path(),
        r#"{"fleets": [{"name": "corral", "gh_repo": "jirathip-k/corral", "local": "~/p",
            "worktree_dir": "corral", "orch": "o", "workers": [], "paused": false,
            "models": {"orch": "f", "impl": "i", "review": "r"}, "surprise": 1}]}"#,
    );
    let err = load(&path).expect_err("unknown field must fail");
    let msg = err.to_string();
    assert!(matches!(err, ConfigError::Parse { .. }), "kind: {err:?}");
    assert!(msg.contains("surprise"), "names the field: {msg}");
}

#[test]
fn unknown_field_inside_models_is_a_hard_error() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = write_registry(
        dir.path(),
        r#"{"fleets": [{"name": "corral", "gh_repo": "jirathip-k/corral", "local": "~/p",
            "worktree_dir": "corral", "orch": "o", "workers": [],
            "models": {"orch": "f", "impl": "i", "review": "r", "junk": "x"}}]}"#,
    );
    let err = load(&path).expect_err("unknown models field must fail");
    assert!(matches!(err, ConfigError::Parse { .. }));
    assert!(err.to_string().contains("junk"));
}

#[test]
fn unknown_top_level_field_is_a_hard_error() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = write_registry(dir.path(), r#"{"fleets": [], "extra": true}"#);
    let err = load(&path).expect_err("unknown top-level field must fail");
    assert!(matches!(err, ConfigError::Parse { .. }));
    assert!(err.to_string().contains("extra"));
}

#[test]
fn missing_required_field_errors_naming_the_field() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = write_registry(
        dir.path(),
        r#"{"fleets": [{"name": "corral", "gh_repo": "jirathip-k/corral", "local": "~/p",
            "worktree_dir": "corral", "workers": [],
            "models": {"orch": "f", "impl": "i", "review": "r"}}]}"#,
    );
    let err = load(&path).expect_err("missing orch must fail");
    assert!(matches!(err, ConfigError::Parse { .. }), "kind: {err:?}");
    assert!(err.to_string().contains("orch"), "names the field: {err}");
}

#[test]
fn empty_required_field_errors_naming_fleet_and_field() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = write_registry(
        dir.path(),
        r#"{"fleets": [{"name": "corral", "gh_repo": "  ", "local": "~/p",
            "worktree_dir": "corral", "orch": "o", "workers": [],
            "models": {"orch": "f", "impl": "i", "review": "r"}}]}"#,
    );
    let err = load(&path).expect_err("blank gh_repo must fail");
    assert!(matches!(err, ConfigError::Empty { .. }), "kind: {err:?}");
    let msg = err.to_string();
    assert!(msg.contains("corral"), "names the fleet: {msg}");
    assert!(msg.contains("gh_repo"), "names the field: {msg}");
}

#[test]
fn duplicate_fleet_name_is_a_hard_error() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = write_registry(
        dir.path(),
        &format!(r#"{{ "fleets": [{VALID_A}, {VALID_B}, {VALID_A}] }}"#),
    );
    let err = load(&path).expect_err("duplicate name must fail");
    assert!(
        matches!(err, ConfigError::DuplicateFleet { ref name } if name == "corral"),
        "kind: {err:?}"
    );
    assert!(err.to_string().contains("corral"));
}

#[test]
fn malformed_json_is_a_parse_error_not_a_panic() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = write_registry(dir.path(), r#"{ "fleets": [ oops"#);
    let err = load(&path).expect_err("malformed JSON must fail cleanly");
    assert!(matches!(err, ConfigError::Parse { .. }), "kind: {err:?}");
}

#[test]
fn missing_file_is_an_io_error() {
    let dir = tempfile::tempdir().expect("temp dir");
    let err = load(&dir.path().join("nope.json")).expect_err("missing file must fail");
    assert!(matches!(err, ConfigError::Io { .. }), "kind: {err:?}");
}

#[test]
fn local_path_expands_tilde_against_home() {
    let registry = registry(&format!(r#"{{ "fleets": [{VALID_A}] }}"#));
    let fleet: &Fleet = &registry.fleets[0];
    let home = std::env::var("HOME").expect("HOME set in test env");
    assert_eq!(
        fleet.local_path(),
        PathBuf::from(&home).join("Projects/corral"),
        "leading ~/ expands against $HOME"
    );
    assert!(
        !fleet.local_path().starts_with("~"),
        "expanded path no longer starts with a tilde"
    );
}

#[test]
fn local_path_passes_absolute_through_unchanged() {
    let registry = registry(&format!(r#"{{ "fleets": [{VALID_B}] }}"#));
    assert_eq!(registry.fleets[0].local_path(), PathBuf::from("/opt/board"));
}

#[test]
fn empty_model_id_is_a_hard_error() {
    let dir = tempfile::tempdir().expect("temp dir");
    for (field, models) in [
        ("models.orch", r#"{"orch": "", "impl": "i", "review": "r"}"#),
        (
            "models.impl",
            r#"{"orch": "f", "impl": "   ", "review": "r"}"#,
        ),
        (
            "models.review",
            r#"{"orch": "f", "impl": "i", "review": ""}"#,
        ),
    ] {
        let path = write_registry(
            dir.path(),
            &fleet_body("corral", "x/y", "~/p", "[]", models),
        );
        let err = load(&path).expect_err("empty model id must fail");
        assert!(matches!(err, ConfigError::Empty { .. }), "kind: {err:?}");
        assert!(err.to_string().contains(field), "names {field}: {err}");
    }
}

#[test]
fn empty_worker_entry_is_a_hard_error() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = write_registry(
        dir.path(),
        &fleet_body("corral", "x/y", "~/p", r#"["", "  "]"#, MODELS),
    );
    let err = load(&path).expect_err("empty worker must fail");
    assert!(matches!(err, ConfigError::Empty { .. }), "kind: {err:?}");
    let msg = err.to_string();
    assert!(msg.contains("workers[0]"), "indexes the worker: {msg}");
}

#[test]
fn whitespace_in_name_or_gh_repo_is_a_hard_error() {
    let dir = tempfile::tempdir().expect("temp dir");
    for (field, name, repo) in [("name", "not a repo", "x/y"), ("gh_repo", "corral", "x /y")] {
        let path = write_registry(dir.path(), &fleet_body(name, repo, "~/p", "[]", MODELS));
        let err = load(&path).expect_err("internal whitespace must fail");
        assert!(
            matches!(err, ConfigError::Whitespace { .. }),
            "kind: {err:?}"
        );
        assert!(err.to_string().contains(field), "names {field}: {err}");
    }
}

#[test]
fn gh_repo_must_be_a_single_owner_repo_slug() {
    let dir = tempfile::tempdir().expect("temp dir");
    for bad in ["x", "a/b/c", "/b", "a/"] {
        let path = write_registry(dir.path(), &fleet_body("corral", bad, "~/p", "[]", MODELS));
        let err = load(&path).expect_err("bad gh_repo must fail");
        assert!(
            matches!(err, ConfigError::GhRepoShape { .. }),
            "kind: {err:?}"
        );
        assert!(err.to_string().contains("owner/repo"), "shape hint: {err}");
    }
}

#[test]
fn local_bare_tilde_is_a_hard_error() {
    let dir = tempfile::tempdir().expect("temp dir");
    for bad in ["~", "~user/p"] {
        let path = write_registry(dir.path(), &fleet_body("corral", "x/y", bad, "[]", MODELS));
        let err = load(&path).expect_err("bare tilde must fail");
        assert!(matches!(err, ConfigError::BadTilde { .. }), "kind: {err:?}");
        assert!(err.to_string().contains("~/"), "hint: {err}");
    }
}

#[test]
fn empty_name_error_carries_index_and_gh_repo_locator() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = write_registry(dir.path(), &fleet_body("", "x/y", "~/p", "[]", MODELS));
    let err = load(&path).expect_err("empty name must fail");
    assert!(matches!(err, ConfigError::Empty { .. }), "kind: {err:?}");
    let msg = err.to_string();
    assert!(msg.contains("#1"), "carries the array index: {msg}");
    assert!(msg.contains("x/y"), "falls back to gh_repo: {msg}");
    assert!(msg.contains("name"), "names the field: {msg}");
}

// ---------------------------------------------------------------------------
// CLI: corrald fleet list / check (real binary, temp-dir fixtures)
// ---------------------------------------------------------------------------

#[test]
fn fleet_list_prints_one_greppable_line_per_fleet() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = write_registry(
        dir.path(),
        &format!(r#"{{ "fleets": [{VALID_A}, {VALID_B}] }}"#),
    );
    let output = Command::new(env!("CARGO_BIN_EXE_corrald"))
        .args(["fleet", "list", "--registry"])
        .arg(&path)
        .output()
        .expect("run corrald fleet list");
    assert!(output.status.success(), "list exit: {:?}", output.status);
    let text = String::from_utf8_lossy(&output.stdout);
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 2, "one line per fleet: {text}");

    assert!(
        lines[0].starts_with("corral jirathip-k/corral"),
        "line: {}",
        lines[0]
    );
    assert!(lines[0].contains("workers=2"), "worker count: {}", lines[0]);
    assert!(
        lines[0].contains("paused=true"),
        "paused flag: {}",
        lines[0]
    );
    assert!(lines[0].contains("orch=fable"), "orch model: {}", lines[0]);
    assert!(
        lines[0].contains("impl=opencode-go/deepseek-v4-flash"),
        "impl model: {}",
        lines[0]
    );
    assert!(
        lines[0].contains("review=opus"),
        "review model: {}",
        lines[0]
    );

    assert!(
        lines[1].starts_with("board jirathip-k/herdr-board"),
        "line: {}",
        lines[1]
    );
    assert!(
        lines[1].contains("workers=0"),
        "empty workers: {}",
        lines[1]
    );
    assert!(
        lines[1].contains("paused=false"),
        "default paused: {}",
        lines[1]
    );
}

#[test]
fn fleet_check_exits_0_when_every_fleet_checks_out() {
    let dir = tempfile::tempdir().expect("temp dir");
    let repo = dir.path().join("local-repo");
    std::fs::create_dir_all(repo.join(".git")).expect("fake repo with .git");
    let path = write_registry(
        dir.path(),
        &format!(
            r#"{{ "fleets": [{{ "name": "corral", "gh_repo": "jirathip-k/corral",
            "local": "{}", "worktree_dir": "corral", "orch": "o", "workers": [],
            "models": {{"orch": "f", "impl": "i", "review": "r"}} }}] }}"#,
            repo.display()
        ),
    );
    let output = Command::new(env!("CARGO_BIN_EXE_corrald"))
        .args(["fleet", "check", "--registry"])
        .arg(&path)
        .output()
        .expect("run corrald fleet check");
    assert!(
        output.status.success(),
        "check exit 0 expected, got {:?}: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.lines().any(|l| l == "ok corral"), "ok line: {text}");
}

#[test]
fn fleet_check_exits_1_and_reports_reason_on_failure() {
    let dir = tempfile::tempdir().expect("temp dir");
    let good = dir.path().join("good");
    std::fs::create_dir_all(good.join(".git")).expect("fake repo with .git");
    let path = write_registry(
        dir.path(),
        &format!(
            r#"{{ "fleets": [
            {{ "name": "good", "gh_repo": "g/g", "local": "{}", "worktree_dir": "g",
               "orch": "o", "workers": [], "models": {{"orch": "f", "impl": "i", "review": "r"}} }},
            {{ "name": "missing", "gh_repo": "m/m", "local": "{}", "worktree_dir": "m",
               "orch": "o", "workers": [], "models": {{"orch": "f", "impl": "i", "review": "r"}} }}
            ] }}"#,
            good.display(),
            dir.path().join("nowhere").display()
        ),
    );
    let output = Command::new(env!("CARGO_BIN_EXE_corrald"))
        .args(["fleet", "check", "--registry"])
        .arg(&path)
        .output()
        .expect("run corrald fleet check");
    assert_eq!(output.status.code(), Some(1), "check failure exits 1");
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(
        text.lines().any(|l| l == "ok good"),
        "good fleet ok: {text}"
    );
    assert!(
        text.lines().any(|l| l.starts_with("FAIL missing:")),
        "FAIL line: {text}"
    );
    assert!(text.contains("nowhere"), "reason names the path: {text}");
}

#[test]
fn fleet_check_exits_1_when_local_is_not_a_directory() {
    let dir = tempfile::tempdir().expect("temp dir");
    let not_dir = dir.path().join("file");
    std::fs::write(&not_dir, "just a file").expect("write regular file");
    let path = write_registry(
        dir.path(),
        &fleet_body(
            "corral",
            "x/y",
            &not_dir.display().to_string(),
            "[]",
            MODELS,
        ),
    );
    let output = Command::new(env!("CARGO_BIN_EXE_corrald"))
        .args(["fleet", "check", "--registry"])
        .arg(&path)
        .output()
        .expect("run corrald fleet check");
    assert_eq!(output.status.code(), Some(1), "non-directory exits 1");
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(
        text.lines()
            .any(|l| l.starts_with("FAIL corral:") && l.contains("is not a directory")),
        "FAIL line: {text}"
    );
}

#[test]
fn fleet_check_exits_1_when_local_has_no_git_entry() {
    let dir = tempfile::tempdir().expect("temp dir");
    let bare = dir.path().join("bare-dir");
    std::fs::create_dir_all(&bare).expect("create dir without .git");
    let path = write_registry(
        dir.path(),
        &fleet_body("corral", "x/y", &bare.display().to_string(), "[]", MODELS),
    );
    let output = Command::new(env!("CARGO_BIN_EXE_corrald"))
        .args(["fleet", "check", "--registry"])
        .arg(&path)
        .output()
        .expect("run corrald fleet check");
    assert_eq!(output.status.code(), Some(1), "no .git exits 1");
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(
        text.lines()
            .any(|l| l.starts_with("FAIL corral:") && l.contains("has no .git entry")),
        "FAIL line: {text}"
    );
}

#[test]
fn fleet_check_passes_when_git_is_a_regular_file() {
    // Linked worktrees carry `.git` as a FILE (a `gitdir:` pointer), not a
    // directory. Pinning this keeps a future tightening to `.is_dir()` from
    // silently breaking every linked worktree with zero test failures.
    let dir = tempfile::tempdir().expect("temp dir");
    let linked = dir.path().join("linked");
    std::fs::create_dir_all(&linked).expect("create linked-worktree dir");
    std::fs::write(linked.join(".git"), "gitdir: /elsewhere/.git\n").expect("write .git file");
    let path = write_registry(
        dir.path(),
        &fleet_body("corral", "x/y", &linked.display().to_string(), "[]", MODELS),
    );
    let output = Command::new(env!("CARGO_BIN_EXE_corrald"))
        .args(["fleet", "check", "--registry"])
        .arg(&path)
        .output()
        .expect("run corrald fleet check");
    assert!(
        output.status.success(),
        "linked-worktree .git file passes: {:?}",
        output.status
    );
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.lines().any(|l| l == "ok corral"), "ok line: {text}");
}

#[test]
fn fleet_check_exits_2_on_parse_error() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = write_registry(dir.path(), r#"{ "fleets": [ oops"#);
    let output = Command::new(env!("CARGO_BIN_EXE_corrald"))
        .args(["fleet", "check", "--registry"])
        .arg(&path)
        .output()
        .expect("run corrald fleet check");
    assert_eq!(output.status.code(), Some(2), "parse error exits 2");
}

#[test]
fn fleet_list_exits_2_on_usage_error() {
    let output = Command::new(env!("CARGO_BIN_EXE_corrald"))
        .args(["fleet", "list", "--registry"])
        .output()
        .expect("run corrald fleet list");
    assert_eq!(
        output.status.code(),
        Some(2),
        "missing --registry value exits 2"
    );

    let output = Command::new(env!("CARGO_BIN_EXE_corrald"))
        .args(["fleet", "bogus"])
        .output()
        .expect("run corrald fleet bogus");
    assert_eq!(output.status.code(), Some(2), "unknown subcommand exits 2");
}

#[test]
fn fleet_help_exits_0_and_documents_commands() {
    let output = Command::new(env!("CARGO_BIN_EXE_corrald"))
        .args(["fleet", "--help"])
        .output()
        .expect("run corrald fleet --help");
    assert!(output.status.success(), "help exits 0");
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(
        text.contains("corrald fleet list"),
        "documents list: {text}"
    );
    assert!(
        text.contains("corrald fleet check"),
        "documents check: {text}"
    );
    assert!(text.contains("--registry"), "documents --registry: {text}");
}

/// Temp dir + default-path override smoke: `--registry` pointing at a
/// fixture beats $CORRAL_FLEETS_PATH and the HOME default.
#[test]
fn fleet_list_registry_flag_overrides_default_path() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = write_registry(dir.path(), &format!(r#"{{ "fleets": [{VALID_A}] }}"#));
    let output = Command::new(env!("CARGO_BIN_EXE_corrald"))
        .env("CORRAL_FLEETS_PATH", dir.path().join("nonexistent.json"))
        .args(["fleet", "list", "--registry"])
        .arg(&path)
        .output()
        .expect("run corrald fleet list");
    assert!(
        output.status.success(),
        "explicit --registry wins: {:?}",
        output.status
    );
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains("corral"), "lists the fixture fleet: {text}");
}

/// The override test above only proves `--registry` *beats*
/// `$CORRAL_FLEETS_PATH`. This pins the other half: with no `--registry`,
/// the env var is what gets read. Set on the child process only — never
/// `std::env::set_var`, which is UB under Rust 2024 in a threaded test
/// binary.
#[test]
fn fleet_list_reads_corral_fleets_path_as_the_default() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = write_registry(dir.path(), &format!(r#"{{ "fleets": [{VALID_A}] }}"#));
    let output = Command::new(env!("CARGO_BIN_EXE_corrald"))
        .env("CORRAL_FLEETS_PATH", &path)
        .args(["fleet", "list"])
        .output()
        .expect("run corrald fleet list");
    assert!(
        output.status.success(),
        "$CORRAL_FLEETS_PATH is honoured as the default: {:?} {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(
        text.contains("corral"),
        "lists the fleet from the env-var registry: {text}"
    );
}
