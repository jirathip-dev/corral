//! #35 phase 1 test suite: fleet registry config (parse, validate,
//! `local_path` expansion, atomic write) and the `corrald fleet list|check`
//! plus write-side `add|remove` (slice 1) and `pause|resume|models`
//! (slice 2) CLI surface, exercised with the real binary against
//! temp-dir fixtures. The `gh` repo check is injectable
//! ([`RepoResolver`]), so the add/remove write-path tests run offline.

use std::path::PathBuf;
use std::process::Command;

use corrald::fleet::config::{ConfigError, Fleet, Models, Registry, load, write_atomic};
use corrald::fleet::ops::{AddOptions, ModelUpdate, RepoResolver};

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

/// Like `VALID_A` but with the #56 optional alt-model slots present, mirroring
/// the live registry's `impl_alt` / `impl_alt2` additions.
const VALID_ALT: &str = r#"{
    "name": "corral",
    "gh_repo": "jirathip-k/corral",
    "local": "~/Projects/corral",
    "worktree_dir": "corral",
    "orch": "orch-corral",
    "workers": [],
    "models": { "orch": "fable", "impl": "opencode-go/deepseek-v4-flash",
                "review": "opus", "impl_alt": "opencode-go/deepseek-v4-flash",
                "impl_alt2": "dsh" }
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
    assert_eq!(models.impl_alt, None, "absent alt slots default to None");
    assert_eq!(models.impl_alt2, None, "absent alt2 slots default to None");
}

#[test]
fn models_alt_slots_land_when_present() {
    let registry = registry(&format!(r#"{{ "fleets": [{VALID_ALT}] }}"#));
    let models: &Models = &registry.fleets[0].models;
    assert_eq!(
        models.impl_alt.as_deref(),
        Some("opencode-go/deepseek-v4-flash")
    );
    assert_eq!(models.impl_alt2.as_deref(), Some("dsh"));
    assert_eq!(models.orch, "fable");
    assert_eq!(models.impl_, "opencode-go/deepseek-v4-flash");
    assert_eq!(models.review, "opus");
}

#[test]
fn models_without_alt_slots_round_trip_without_growing_them() {
    // Back-compat pin (#56): a registry that predates the alt slots must load,
    // and an unrelated rewrite must NOT introduce `impl_alt` / `impl_alt2`.
    let dir = tempfile::tempdir().expect("temp dir");
    let path = write_registry(dir.path(), &format!(r#"{{ "fleets": [{VALID_A}] }}"#));
    let registry = load(&path).expect("pre-alt registry loads");
    assert_eq!(registry.fleets[0].models.impl_alt, None);
    write_atomic(&path, &registry).expect("rewrite");
    let text = std::fs::read_to_string(&path).expect("read back");
    assert!(
        !text.contains("impl_alt"),
        "absent key must not grow: {text}"
    );
    assert!(
        !text.contains("impl_alt2"),
        "absent key must not grow: {text}"
    );
}

#[test]
fn add_rewrite_preserves_alt_slots_on_untouched_fleet() {
    // The data-loss pin (#56): `fleet add` rewrites the whole file, so an
    // untouched fleet's present impl_alt / impl_alt2 must survive. The
    // fixture's FIRST fleet has no alt slots, so inheritance (which takes
    // the first fleet) cannot manufacture the asserted strings — the only
    // fleet carrying them is the untouched second one.
    let dir = tempfile::tempdir().expect("temp dir");
    let path = write_registry(
        dir.path(),
        &format!(r#"{{ "fleets": [{VALID_B}, {VALID_ALT}] }}"#),
    );
    let opts = add_options("newfleet", "jirathip-k/corral");
    corrald::fleet::ops::add(
        &path,
        &opts,
        &FakeResolver {
            ok: vec!["jirathip-k/corral".to_string()],
        },
    )
    .expect("add succeeds");

    let text = std::fs::read_to_string(&path).expect("read written registry");
    assert_eq!(
        text.matches("\"impl_alt\": \"opencode-go/deepseek-v4-flash\"")
            .count(),
        1,
        "exactly the untouched fleet carries impl_alt: {text}"
    );
    assert_eq!(
        text.matches("\"impl_alt2\": \"dsh\"").count(),
        1,
        "exactly the untouched fleet carries impl_alt2: {text}"
    );
    let registry = load(&path).expect("re-parses");
    let corral = registry
        .fleets
        .iter()
        .find(|f| f.name == "corral")
        .expect("untouched fleet survives");
    assert_eq!(
        corral.models.impl_alt.as_deref(),
        Some("opencode-go/deepseek-v4-flash"),
        "impl_alt value lands after rewrite"
    );
    assert_eq!(corral.models.impl_alt2.as_deref(), Some("dsh"));
    let newfleet = registry
        .fleets
        .iter()
        .find(|f| f.name == "newfleet")
        .expect("added fleet present");
    assert_eq!(
        newfleet.models.impl_alt, None,
        "inherits from the FIRST fleet (no alt slots), not the alt-carrying one"
    );
}

#[test]
fn add_inherits_alt_slots_from_the_first_fleet_when_present() {
    // DELIBERATE (#56 review F1): omitting --models inherits the first
    // fleet's WHOLE model map, alt slots included. Pinned so a refactor
    // cannot silently flip it either way.
    let dir = tempfile::tempdir().expect("temp dir");
    let path = write_registry(dir.path(), &format!(r#"{{ "fleets": [{VALID_ALT}] }}"#));
    let added = corrald::fleet::ops::add(
        &path,
        &add_options("inheritor", "jirathip-k/corral"),
        &FakeResolver {
            ok: vec!["jirathip-k/corral".to_string()],
        },
    )
    .expect("add succeeds");
    assert_eq!(
        added.models.impl_alt.as_deref(),
        Some("opencode-go/deepseek-v4-flash"),
        "alt slots inherit with the rest of the model map"
    );
    assert_eq!(added.models.impl_alt2.as_deref(), Some("dsh"));
}

#[test]
fn add_inherits_and_preserves_fleet_ops_forward_fields() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = write_registry(
        dir.path(),
        r#"{
            "fleets": [{
                "name": "corral",
                "gh_repo": "jirathip-k/corral",
                "local": "~/Projects/corral",
                "worktree_dir": "corral",
                "orch": "o",
                "workers": [],
                "models": {
                    "orch": "f",
                    "impl": "i",
                    "review": "r",
                    "reasoning_effort": {"orch": "medium", "impl": "max", "review": "xhigh"},
                    "future_model_field": {"nested": true}
                },
                "future_fleet_field": {"nested": true}
            }],
            "admit": {"enabled": true, "budget_mb": 16000}
        }"#,
    );

    corrald::fleet::ops::add(
        &path,
        &add_options("inheritor", "jirathip-k/corral"),
        &FakeResolver {
            ok: vec!["jirathip-k/corral".to_string()],
        },
    )
    .expect("add succeeds");

    let written: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).expect("read back"))
            .expect("written registry is valid JSON");
    let source = &written["fleets"][0];
    let added = &written["fleets"][1];
    assert_eq!(
        source["models"]["reasoning_effort"],
        serde_json::json!({"orch": "medium", "impl": "max", "review": "xhigh"})
    );
    assert_eq!(
        added["models"]["reasoning_effort"], source["models"]["reasoning_effort"],
        "new fleet inherits reasoning_effort from the template"
    );
    assert_eq!(
        added["models"]["future_model_field"], source["models"]["future_model_field"],
        "new fleet inherits unknown model forward fields"
    );
    assert_eq!(
        added["future_fleet_field"], source["future_fleet_field"],
        "new fleet inherits unknown fleet forward fields"
    );
    assert_eq!(
        written["admit"],
        serde_json::json!({"enabled": true, "budget_mb": 16000}),
        "top-level admit survives the add rewrite"
    );
}

#[test]
fn typoed_owned_optional_fields_are_hard_errors() {
    let dir = tempfile::tempdir().expect("temp dir");
    for (body, typo) in [
        (
            r#"{"fleets": [{"name": "corral", "gh_repo": "jirathip-k/corral", "local": "~/p",
                "worktree_dir": "corral", "orch": "o", "workers": [],
                "models": {"orch": "f", "impl": "i", "review": "r", "imp1_alt": "x"}}]}"#,
            "imp1_alt",
        ),
        (
            r#"{"fleets": [{"name": "corral", "gh_repo": "jirathip-k/corral", "local": "~/p",
                "worktree_dir": "corral", "orch": "o", "workers": [], "pausd": true,
                "models": {"orch": "f", "impl": "i", "review": "r"}}]}"#,
            "pausd",
        ),
        (
            r#"{"fleets": [{"name": "corral", "gh_repo": "jirathip-k/corral", "local": "~/p",
                "worktree_dir": "corral", "orch": "o", "workers": [], "puased": true,
                "models": {"orch": "f", "impl": "i", "review": "r"}}]}"#,
            "puased",
        ),
        (
            r#"{"fleets": [{"name": "corral", "gh_repo": "jirathip-k/corral", "local": "~/p",
                "worktree_dir": "corral", "orch": "o", "workers": [],
                "models": {"orch": "f", "impl": "i", "review": "r", "imlp_alt": "x"}}]}"#,
            "imlp_alt",
        ),
        (
            r#"{"fleets": [{"name": "corral", "gh_repo": "jirathip-k/corral", "local": "~/p",
                "worktree_dir": "corral", "orch": "o", "workers": [],
                "models": {"orch": "f", "impl": "i", "review": "r", "imlp_alt2": "x"}}]}"#,
            "imlp_alt2",
        ),
    ] {
        let path = write_registry(dir.path(), body);
        let err = load(&path).expect_err("typo in Corral-owned field must fail");
        assert!(matches!(err, ConfigError::Parse { .. }), "kind: {err:?}");
        let msg = err.to_string();
        assert!(msg.contains(typo), "names {typo:?}: {msg}");
    }
}

#[test]
fn empty_or_whitespace_impl_alt_is_refused_by_validate() {
    let dir = tempfile::tempdir().expect("temp dir");
    for (field, models) in [
        (
            "models.impl_alt",
            r#"{"orch": "f", "impl": "i", "review": "r", "impl_alt": ""}"#,
        ),
        (
            "models.impl_alt",
            r#"{"orch": "f", "impl": "i", "review": "r", "impl_alt": "  "}"#,
        ),
        (
            "models.impl_alt2",
            r#"{"orch": "f", "impl": "i", "review": "r", "impl_alt2": "deep seek"}"#,
        ),
    ] {
        let path = write_registry(
            dir.path(),
            &fleet_body("corral", "x/y", "~/p", "[]", models),
        );
        let err = load(&path).expect_err("bad alt slot must fail");
        let msg = err.to_string();
        assert!(
            matches!(
                err,
                ConfigError::Empty { .. } | ConfigError::Whitespace { .. }
            ),
            "kind: {err:?}"
        );
        assert!(msg.contains(field), "names {field}: {msg}");
    }
}

#[test]
fn fleet_ops_fields_parse_and_survive_a_write_round_trip() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = write_registry(
        dir.path(),
        r#"{
            "fleets": [{
                "name": "corral",
                "gh_repo": "jirathip-k/corral",
                "local": "~/Projects/corral",
                "worktree_dir": "corral",
                "orch": "o",
                "workers": [],
                "models": {
                    "orch": "f",
                    "impl": "i",
                    "review": "r",
                    "reasoning_effort": {"orch": "medium", "impl": "max", "review": "xhigh"},
                    "future_model_field": {"nested": true}
                },
                "future_fleet_field": {"nested": true}
            }],
            "admit": {"enabled": true, "budget_mb": 16000},
            "future_top_level": {"nested": true}
        }"#,
    );

    let registry = load(&path).expect("fleet-ops registry loads");
    assert_eq!(registry.fleets[0].name, "corral");
    write_atomic(&path, &registry).expect("write");

    let written: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).expect("read back"))
            .expect("written registry is valid JSON");
    let fleet = &written["fleets"][0];
    assert_eq!(fleet["name"], "corral");
    assert_eq!(
        fleet["models"]["reasoning_effort"],
        serde_json::json!({"orch": "medium", "impl": "max", "review": "xhigh"})
    );
    assert_eq!(
        fleet["future_fleet_field"],
        serde_json::json!({"nested": true})
    );
    assert_eq!(
        fleet["models"]["future_model_field"],
        serde_json::json!({"nested": true})
    );
    assert_eq!(
        written["admit"],
        serde_json::json!({"enabled": true, "budget_mb": 16000})
    );
    assert_eq!(
        written["future_top_level"],
        serde_json::json!({"nested": true})
    );
    assert_eq!(
        load(&path).expect("written registry re-loads").fleets[0]
            .models
            .impl_,
        "i"
    );
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
fn empty_alt_model_id_is_a_hard_error() {
    // Slice 2: the optional alt slots, when PRESENT in the registry, must
    // hold non-empty values like every other model slot.
    let dir = tempfile::tempdir().expect("temp dir");
    for (field, models) in [
        (
            "models.impl_alt",
            r#"{"orch": "f", "impl": "i", "impl_alt": "", "review": "r"}"#,
        ),
        (
            "models.impl_alt2",
            r#"{"orch": "f", "impl": "i", "impl_alt2": "  ", "review": "r"}"#,
        ),
    ] {
        let path = write_registry(
            dir.path(),
            &fleet_body("corral", "x/y", "~/p", "[]", models),
        );
        let err = load(&path).expect_err("empty alt model id must fail");
        assert!(matches!(err, ConfigError::Empty { .. }), "kind: {err:?}");
        assert!(err.to_string().contains(field), "names {field}: {err}");
    }
}

#[test]
fn alt_model_ids_parse_and_round_trip() {
    // The alt slots are OPTIONAL (absent in older registries) and must
    // survive a load -> write round-trip byte-semantically.
    let dir = tempfile::tempdir().expect("temp dir");
    let body = r#"{"fleets": [{"name": "corral", "gh_repo": "x/y", "local": "~/p",
        "worktree_dir": "corral", "orch": "o", "workers": [],
        "models": {"orch": "f", "impl": "i", "impl_alt": "opencode-go/deepseek-v4-flash",
                   "impl_alt2": "codex/deepseek-v4-flash", "review": "r"}}]}"#;
    let path = write_registry(dir.path(), body);
    let registry = load(&path).expect("parses with alt slots");
    assert_eq!(
        registry.fleets[0].models.impl_alt.as_deref(),
        Some("opencode-go/deepseek-v4-flash")
    );
    assert_eq!(
        registry.fleets[0].models.impl_alt2.as_deref(),
        Some("codex/deepseek-v4-flash")
    );

    write_atomic(&path, &registry).expect("write");
    let text = std::fs::read_to_string(&path).expect("read back");
    assert!(text.contains("impl_alt"), "impl_alt written: {text}");
    assert!(text.contains("impl_alt2"), "impl_alt2 written: {text}");
    let registry2 = load(&path).expect("re-parses");
    assert_eq!(
        registry2.fleets[0].models.impl_alt.as_deref(),
        Some("opencode-go/deepseek-v4-flash")
    );

    // And absent alt slots stay absent in older-style registries.
    let old = write_registry(dir.path(), &fleet_body("board", "x/y", "~/p", "[]", MODELS));
    let old_registry = load(&old).expect("old-style parses");
    assert_eq!(
        old_registry.fleets[0].models.impl_alt, None,
        "absent impl_alt"
    );
    assert_eq!(
        old_registry.fleets[0].models.impl_alt2, None,
        "absent impl_alt2"
    );
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
fn fleet_list_line_is_unchanged_when_alt_slots_are_present() {
    // #56 spec point 4: the greppable `fleet list` line keeps exactly
    // orch/impl/review — consumers split on whitespace, so appending the alt
    // slots would break them. A registry WITH impl_alt/impl_alt2 must list
    // exactly the same shape as one without.
    let dir = tempfile::tempdir().expect("temp dir");
    let path = write_registry(dir.path(), &format!(r#"{{ "fleets": [{VALID_ALT}] }}"#));
    let output = Command::new(env!("CARGO_BIN_EXE_corrald"))
        .args(["fleet", "list", "--registry"])
        .arg(&path)
        .output()
        .expect("run corrald fleet list");
    assert!(output.status.success(), "list exit: {:?}", output.status);
    let text = String::from_utf8_lossy(&output.stdout);
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 1, "one line per fleet: {text}");
    assert!(
        lines[0].starts_with("corral jirathip-k/corral"),
        "line: {}",
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
        !lines[0].contains("impl_alt"),
        "alt slots are NOT appended: {}",
        lines[0]
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

// ---------------------------------------------------------------------------
// Write path (slice 1): `fleet add` / `fleet remove` — atomic-write
// discipline, repo-resolves-before-add, byte-identical on refusal.
// ---------------------------------------------------------------------------

/// A `RepoResolver` stub: resolves exactly the repos the test says resolve,
/// refusing everything else with a diagnostic. This is why add/remove tests
/// run offline — the `gh` check is injected, not shelled out to.
#[derive(Clone)]
struct FakeResolver {
    ok: Vec<String>,
}

impl RepoResolver for FakeResolver {
    fn repo_resolves(&self, repo: &str) -> Result<(), Option<String>> {
        if self.ok.iter().any(|r| r == repo) {
            Ok(())
        } else {
            Err(Some("GraphQL: Could not resolve".to_string()))
        }
    }
}

fn add_options(name: &str, repo: &str) -> AddOptions {
    AddOptions {
        name: name.to_string(),
        gh_repo: repo.to_string(),
        local: None,
        worktree_dir: None,
        orch: None,
        workers: Vec::new(),
        models: None,
    }
}

#[test]
fn add_writes_and_round_trips_through_load() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = write_registry(dir.path(), &format!(r#"{{ "fleets": [{VALID_A}] }}"#));
    let before = std::fs::read(&path).expect("read registry");

    let opts = add_options("newfleet", "jirathip-k/corral");
    let added = corrald::fleet::ops::add(
        &path,
        &opts,
        &FakeResolver {
            ok: vec!["jirathip-k/corral".to_string()],
        },
    )
    .expect("add succeeds");

    assert_eq!(added.name, "newfleet");
    assert_eq!(added.gh_repo, "jirathip-k/corral");
    assert_eq!(added.local, "~/Projects/newfleet", "local default");
    assert_eq!(added.worktree_dir, "newfleet", "worktree_dir default");
    assert_eq!(added.orch, "orch-newfleet", "orch default");
    assert!(added.workers.is_empty(), "workers default");
    assert_eq!(
        added.models.impl_, "opencode-go/deepseek-v4-flash",
        "inherits models"
    );

    let after = std::fs::read(&path).expect("read registry after");
    assert_ne!(before, after, "the file changed");
    let registry = load(&path).expect("written file re-parses");
    let fleet = registry
        .fleets
        .iter()
        .find(|f| f.name == "newfleet")
        .expect("new fleet present");
    assert_eq!(fleet.models.impl_, "opencode-go/deepseek-v4-flash");
    assert_eq!(fleet.worktree_dir, "newfleet");

    // The single highest-consequence silent-data-loss path in the write
    // surface: dropping `"paused": true` from an untouched fleet would
    // release the ops-safety pause gate (#35) on every unrelated add. Pin
    // that the pre-existing paused fleet survives the rewrite paused.
    let corral = registry
        .fleets
        .iter()
        .find(|f| f.name == "corral")
        .expect("existing fleet survives");
    assert!(corral.paused, "paused: true must survive a rewrite");
    let text = std::fs::read_to_string(&path).expect("read written registry");
    assert!(
        text.contains("\"paused\": true"),
        "paused: true is written explicitly: {text}"
    );
}

#[test]
fn add_refuses_duplicate_name_leaving_file_byte_identical() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = write_registry(dir.path(), &format!(r#"{{ "fleets": [{VALID_A}] }}"#));
    let before = std::fs::read(&path).expect("read registry");

    let opts = add_options("corral", "jirathip-k/corral");
    let err = corrald::fleet::ops::add(
        &path,
        &opts,
        &FakeResolver {
            ok: vec!["jirathip-k/corral".to_string()],
        },
    )
    .expect_err("duplicate name must be refused");
    assert!(
        matches!(err, ConfigError::DuplicateFleet { .. }),
        "kind: {err:?}"
    );
    assert_eq!(
        std::fs::read(&path).expect("read registry"),
        before,
        "file byte-identical"
    );
}

#[test]
fn add_refuses_unresolvable_repo_leaving_file_byte_identical() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = write_registry(dir.path(), &format!(r#"{{ "fleets": [{VALID_A}] }}"#));
    let before = std::fs::read(&path).expect("read registry");

    let opts = add_options("ghost", "nope/nothing");
    let err = corrald::fleet::ops::add(
        &path,
        &opts,
        &FakeResolver {
            ok: vec!["jirathip-k/corral".to_string()],
        },
    )
    .expect_err("unresolvable repo must be refused");
    assert!(
        matches!(err, ConfigError::AddRepoUnresolved { .. }),
        "kind: {err:?}"
    );
    assert_eq!(
        std::fs::read(&path).expect("read registry"),
        before,
        "file byte-identical"
    );
}

#[test]
fn add_refuses_invalid_local_leaving_file_byte_identical() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = write_registry(dir.path(), &format!(r#"{{ "fleets": [{VALID_A}] }}"#));
    let before = std::fs::read(&path).expect("read registry");

    // A `--local` of a bare tilde must be refused by validate() before write.
    let mut opts = add_options("tilde", "jirathip-k/corral");
    opts.local = Some("~".to_string());
    let err = corrald::fleet::ops::add(
        &path,
        &opts,
        &FakeResolver {
            ok: vec!["jirathip-k/corral".to_string()],
        },
    )
    .expect_err("bare-tilde local must be refused");
    assert!(matches!(err, ConfigError::BadTilde { .. }), "kind: {err:?}");
    assert_eq!(
        std::fs::read(&path).expect("read registry"),
        before,
        "file byte-identical"
    );
}

#[test]
fn remove_drops_exactly_one_fleet_and_round_trips() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = write_registry(
        dir.path(),
        &format!(r#"{{ "fleets": [{VALID_A}, {VALID_B}] }}"#),
    );
    let remaining = corrald::fleet::ops::remove(&path, "corral").expect("remove succeeds");
    assert_eq!(remaining, 1, "one fleet remains");

    let registry = load(&path).expect("written file re-parses");
    assert_eq!(registry.fleets.len(), 1);
    assert_eq!(registry.fleets[0].name, "board", "the other fleet survives");
    assert_eq!(
        registry.fleets[0].models.impl_, "sonnet",
        "round-trip preserves impl"
    );
}

#[test]
fn remove_refuses_unknown_name_leaving_file_byte_identical() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = write_registry(
        dir.path(),
        &format!(r#"{{ "fleets": [{VALID_A}, {VALID_B}] }}"#),
    );
    let before = std::fs::read(&path).expect("read registry");

    let err =
        corrald::fleet::ops::remove(&path, "no-such-fleet").expect_err("unknown name refused");
    assert!(
        matches!(err, ConfigError::RemoveNotFound { .. }),
        "kind: {err:?}"
    );
    assert_eq!(
        std::fs::read(&path).expect("read registry"),
        before,
        "file byte-identical"
    );
}

#[test]
fn failed_operation_leaves_file_byte_identical_and_no_temp_file() {
    // A write that fails at the filesystem level (target dir made read-only,
    // so `File::create` of the temp file fails with EACCES) must leave the
    // original byte-identical and leak no temp file — the atomic-write
    // guarantee. (Vacuous when run as root, which never applies in CI.)
    let dir = tempfile::tempdir().expect("temp dir");
    let path = write_registry(dir.path(), &format!(r#"{{ "fleets": [{VALID_A}] }}"#));
    let before = std::fs::read(&path).expect("read registry");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o500))
            .expect("make dir read-only");
    }
    let opts = add_options("x", "jirathip-k/corral");
    let err = corrald::fleet::ops::add(
        &path,
        &opts,
        &FakeResolver {
            ok: vec!["jirathip-k/corral".to_string()],
        },
    )
    .expect_err("write to read-only dir must fail");
    assert!(matches!(err, ConfigError::Write { .. }), "kind: {err:?}");

    // Restore so the tempdir cleanup doesn't fail.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o755))
            .expect("restore dir perms");
    }
    assert_eq!(
        std::fs::read(&path).expect("read registry"),
        before,
        "file byte-identical"
    );
    assert_no_temp_files(dir.path());
}

#[test]
fn temp_path_collision_refuses_and_leaves_file_byte_identical() {
    // Fail the write after load/validate succeed: pre-create a DIRECTORY at
    // the temp path so `File::create` fails, and assert the original is
    // byte-identical (the cleanup closure tolerates the unremovable dir).
    // REVIEWED GAP (accepted): the write_all/sync_all failure branches are
    // not portably reachable on macOS (no /dev/full), so the cleanup
    // closure's post-create path rests on code reading; both create-failure
    // paths are pinned here and in
    // failed_operation_leaves_file_byte_identical_and_no_temp_file.
    let dir = tempfile::tempdir().expect("temp dir");
    let path = write_registry(dir.path(), &format!(r#"{{ "fleets": [{VALID_A}] }}"#));
    let before = std::fs::read(&path).expect("read registry");
    let temp_collision = dir
        .path()
        .join(format!(".fleets.json.corrald-tmp.{}", std::process::id()));
    std::fs::create_dir(&temp_collision).expect("create colliding dir");

    let opts = add_options("x", "jirathip-k/corral");
    let err = corrald::fleet::ops::add(
        &path,
        &opts,
        &FakeResolver {
            ok: vec!["jirathip-k/corral".to_string()],
        },
    )
    .expect_err("temp-path collision with a directory must fail");
    assert!(matches!(err, ConfigError::Write { .. }), "kind: {err:?}");
    assert_eq!(
        std::fs::read(&path).expect("read registry"),
        before,
        "file byte-identical"
    );
}

/// No `.…corrald-tmp…` entries left behind in the registry directory.
fn assert_no_temp_files(dir: &std::path::Path) {
    let leftovers: Vec<String> = std::fs::read_dir(dir)
        .expect("read dir")
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.contains("corrald-tmp"))
        .collect();
    assert!(leftovers.is_empty(), "leaked temp files: {leftovers:?}");
}

#[test]
fn written_file_reparses_through_load() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = write_registry(dir.path(), r#"{"fleets": []}"#);
    let mut opts = add_options("roundtrip", "jirathip-k/corral");
    opts.models = Some(Models {
        orch: "fable".to_string(),
        impl_: "deepseek-v4-flash".to_string(),
        review: "opus".to_string(),
        impl_alt: None,
        impl_alt2: None,
    });
    corrald::fleet::ops::add(
        &path,
        &opts,
        &FakeResolver {
            ok: vec!["jirathip-k/corral".to_string()],
        },
    )
    .expect("add succeeds on empty registry");
    let registry = load(&path).expect("written empty->one re-parses");
    assert_eq!(registry.fleets.len(), 1);
}

#[test]
fn impl_serializes_back_to_json_key_impl_not_impl_underscore() {
    // The brief's explicit round-trip contract: `Models::impl_` is
    // `#[serde(rename = "impl")]` and must serialise back to the JSON key
    // `impl`, not `impl_`. `paused` is skipped when false.
    let dir = tempfile::tempdir().expect("temp dir");
    let path = write_registry(dir.path(), r#"{"fleets": []}"#);
    let mut opts = add_options("rt", "jirathip-k/corral");
    opts.models = Some(Models {
        orch: "fable".to_string(),
        impl_: "deepseek-v4-flash".to_string(),
        review: "opus".to_string(),
        impl_alt: None,
        impl_alt2: None,
    });
    corrald::fleet::ops::add(
        &path,
        &opts,
        &FakeResolver {
            ok: vec!["jirathip-k/corral".to_string()],
        },
    )
    .expect("add succeeds");
    let text = std::fs::read_to_string(&path).expect("read written registry");
    assert!(text.contains("\"impl\""), "writes impl key: {text}");
    assert!(!text.contains("impl_"), "never writes impl_: {text}");
    assert!(
        !text.contains("\"paused\""),
        "paused false is skipped: {text}"
    );

    // And it re-parses: the round-trip is lossless.
    let registry = load(&path).expect("re-parses");
    assert_eq!(registry.fleets[0].models.impl_, "deepseek-v4-flash");
}

#[test]
fn write_atomic_round_trips_exact_pretty_json_with_trailing_newline() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = write_registry(dir.path(), &format!(r#"{{ "fleets": [{VALID_A}] }}"#));
    let registry = load(&path).expect("load fixture");
    write_atomic(&path, &registry).expect("write");
    let text = std::fs::read_to_string(&path).expect("read back");
    assert!(
        text.ends_with('\n'),
        "trailing newline for diffability: {text:?}"
    );
    let registry2 = load(&path).expect("re-loads");
    assert_eq!(registry2.fleets.len(), 1);
    assert_eq!(registry2.fleets[0].name, "corral");
}

#[test]
fn write_atomic_refuses_if_current_registry_is_no_longer_parseable() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = write_registry(
        dir.path(),
        r#"{"fleets": [{"name": "corral", "gh_repo": "x/y", "local": "~/p",
            "worktree_dir": "corral", "orch": "o", "workers": [],
            "models": {"orch": "f", "impl": "i", "review": "r",
                       "reasoning_effort": {"orch": "max"}}}]}"#,
    );
    let registry = load(&path).expect("loads forward fields");
    let invalid = br#"{"fleets": ["#;
    std::fs::write(&path, invalid).expect("make current file invalid");

    let err = write_atomic(&path, &registry).expect_err("must refuse to clobber");
    assert!(matches!(err, ConfigError::Parse { .. }), "kind: {err:?}");
    assert_eq!(
        std::fs::read(&path).expect("read back"),
        invalid,
        "the original invalid file remains byte-identical"
    );
}

/// CLI usage contract for the write commands: missing required flags and
/// unknown args exit 2 (same as the read side), and `--help` exits 0 and
/// documents the new commands. The network/`gh` path is covered by the
/// injectable-resolver unit tests above; here we pin the arg-parsing surface
/// that runs before any I/O.
#[test]
fn fleet_add_remove_usage_errors_exit_2_and_help_documents_writes() {
    let bin = env!("CARGO_BIN_EXE_corrald");

    let add_missing_name = Command::new(bin)
        .args(["fleet", "add", "--gh", "x/y"])
        .output()
        .expect("run");
    assert_eq!(
        add_missing_name.status.code(),
        Some(2),
        "add without --name exits 2"
    );

    let add_unknown_flag = Command::new(bin)
        .args(["fleet", "add", "--name", "x", "--gh", "x/y", "--bogus"])
        .output()
        .expect("run");
    assert_eq!(
        add_unknown_flag.status.code(),
        Some(2),
        "add with unknown flag exits 2"
    );

    let remove_no_name = Command::new(bin)
        .args(["fleet", "remove"])
        .output()
        .expect("run");
    assert_eq!(
        remove_no_name.status.code(),
        Some(2),
        "remove without name exits 2"
    );

    let remove_two_names = Command::new(bin)
        .args(["fleet", "remove", "a", "b"])
        .output()
        .expect("run");
    assert_eq!(
        remove_two_names.status.code(),
        Some(2),
        "remove with two names exits 2"
    );

    let help = Command::new(bin)
        .args(["fleet", "--help"])
        .output()
        .expect("run");
    assert!(help.status.success(), "help exits 0");
    let text = String::from_utf8_lossy(&help.stdout);
    assert!(text.contains("fleet add"), "documents add: {text}");
    assert!(text.contains("fleet remove"), "documents remove: {text}");
}

// ---------------------------------------------------------------------------
// Re-review round (PR #58): whitespace-in-models validation, exit-code
// contract, bootstrap error, remove-to-empty, inheritance pin, legacy CLI.
// ---------------------------------------------------------------------------

#[test]
fn whitespace_in_models_is_a_hard_error() {
    // models.* join the whitespace-delimited `fleet list` line; internal
    // whitespace would corrupt it exactly like whitespace in name/gh_repo.
    let dir = tempfile::tempdir().expect("temp dir");
    let path = write_registry(dir.path(), &format!(r#"{{ "fleets": [{VALID_A}] }}"#));
    let before = std::fs::read(&path).expect("read registry");

    let mut opts = add_options("spacey", "jirathip-k/corral");
    opts.models = Some(Models {
        orch: "claude opus 5".to_string(),
        impl_: "i".to_string(),
        review: "r".to_string(),
        impl_alt: None,
        impl_alt2: None,
    });
    let err = corrald::fleet::ops::add(
        &path,
        &opts,
        &FakeResolver {
            ok: vec!["jirathip-k/corral".to_string()],
        },
    )
    .expect_err("whitespace in models.orch must be refused");
    assert!(
        matches!(err, ConfigError::Whitespace { ref field, .. } if field == "models.orch"),
        "kind: {err:?}"
    );
    assert_eq!(err.exit_code(), 2, "validation failure exits 2");
    assert_eq!(
        std::fs::read(&path).expect("read registry"),
        before,
        "file byte-identical"
    );
}

#[test]
fn exit_code_contract_separates_refusals_from_usage_errors() {
    // 1 = operational refusal / write failure; 2 = usage/parse/validation.
    use corrald::fleet::config::ConfigError::*;
    assert_eq!(DuplicateFleet { name: "x".into() }.exit_code(), 1);
    assert_eq!(
        AddRepoUnresolved {
            repo: "o/r".into(),
            detail: None
        }
        .exit_code(),
        1
    );
    assert_eq!(AddNeedsModels.exit_code(), 1);
    assert_eq!(RemoveNotFound { name: "x".into() }.exit_code(), 1);
    assert_eq!(
        Write {
            path: "p".into(),
            source: std::io::Error::other("x")
        }
        .exit_code(),
        1
    );
    assert_eq!(
        Empty {
            fleet: "f".into(),
            field: "name".into()
        }
        .exit_code(),
        2
    );
    assert_eq!(
        BadTilde {
            fleet: "f".into(),
            value: "~".into()
        }
        .exit_code(),
        2
    );
}

#[test]
fn add_on_empty_registry_without_models_reports_the_bootstrap_remedy() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = write_registry(dir.path(), r#"{"fleets": []}"#);
    let before = std::fs::read(&path).expect("read registry");

    let err = corrald::fleet::ops::add(
        &path,
        &add_options("first", "jirathip-k/corral"),
        &FakeResolver {
            ok: vec!["jirathip-k/corral".to_string()],
        },
    )
    .expect_err("no models to inherit and no --models must be refused");
    assert!(matches!(err, ConfigError::AddNeedsModels), "kind: {err:?}");
    let message = err.to_string();
    assert!(
        message.contains("--models orch="),
        "the error names the remedy: {message}"
    );
    assert_eq!(
        std::fs::read(&path).expect("read registry"),
        before,
        "file byte-identical"
    );
}

#[test]
fn remove_last_fleet_writes_an_empty_registry_that_reloads() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = write_registry(dir.path(), &format!(r#"{{ "fleets": [{VALID_A}] }}"#));
    let remaining = corrald::fleet::ops::remove(&path, "corral").expect("remove succeeds");
    assert_eq!(remaining, 0, "registry is now empty");

    // The written empty registry must still be `{"fleets": []}`-shaped —
    // a serializer that skipped the empty vec would brick the file.
    let registry = load(&path).expect("empty registry reloads through load()");
    assert!(registry.fleets.is_empty());
    let text = std::fs::read_to_string(&path).expect("read written registry");
    assert!(text.contains("\"fleets\""), "fleets key survives: {text}");
}

#[test]
fn models_inheritance_takes_the_first_fleet_in_array_order() {
    // VALID_A (impl opencode-go/deepseek-v4-flash) is first, VALID_B (impl
    // sonnet) second — the documented policy is FIRST fleet wins.
    let dir = tempfile::tempdir().expect("temp dir");
    let path = write_registry(
        dir.path(),
        &format!(r#"{{ "fleets": [{VALID_A}, {VALID_B}] }}"#),
    );
    let added = corrald::fleet::ops::add(
        &path,
        &add_options("inheritor", "jirathip-k/corral"),
        &FakeResolver {
            ok: vec!["jirathip-k/corral".to_string()],
        },
    )
    .expect("add succeeds");
    assert_eq!(
        added.models.impl_, "opencode-go/deepseek-v4-flash",
        "inherits from the FIRST fleet, not the last"
    );
}

#[test]
fn fleet_add_accepts_the_legacy_positional_name_shape() {
    // The legacy fleet CLI is `fleet add <name> --gh o/r` (positional name)
    // — #35 design principle 2 requires the same shape to work. With no
    // --gh the parse must fail as usage (2) AFTER accepting the positional,
    // and the error must name --gh, proving the positional was consumed as
    // the name.
    let bin = env!("CARGO_BIN_EXE_corrald");
    let out = Command::new(bin)
        .args(["fleet", "add", "somefleet"])
        .output()
        .expect("run");
    assert_eq!(out.status.code(), Some(2), "missing --gh is usage");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("--gh"), "asks for --gh: {stderr}");

    let two_names = Command::new(bin)
        .args(["fleet", "add", "a", "b", "--gh", "x/y"])
        .output()
        .expect("run");
    assert_eq!(
        two_names.status.code(),
        Some(2),
        "two positional names is usage"
    );
}

// ---------------------------------------------------------------------------
// #60 follow-ups from the #58/#67 reviews: binary-level exit-1 pin,
// symlink/permission regression tests, --models alt-slot refusal e2e.
// ---------------------------------------------------------------------------

/// R1: the end-to-end exit-code contract for a refusal — automation branches
/// on this, and the unit-level `exit_code()` test cannot prove main.rs calls
/// it. `RemoveNotFound` needs no gh, no network, no write.
#[test]
fn fleet_remove_unknown_name_exits_1_not_2() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = write_registry(dir.path(), &format!(r#"{{ "fleets": [{VALID_A}] }}"#));
    let bin = env!("CARGO_BIN_EXE_corrald");
    let out = Command::new(bin)
        .args([
            "fleet",
            "remove",
            "no-such-fleet",
            "--registry",
            path.to_str().expect("utf8 path"),
        ])
        .output()
        .expect("run");
    assert_eq!(
        out.status.code(),
        Some(1),
        "unknown name is a refusal (1), not a usage error (2): {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("no fleet named"),
        "names the refusal: {stderr}"
    );
}

/// N1 (#67 round 2): the alt-slot `--models` refusal is a user-facing string
/// with a dedicated match arm — pin both the exit code and the message so a
/// refactor cannot let `impl_alt` fall through to the generic unknown-key arm.
#[test]
fn fleet_add_models_alt_slot_refusal_exits_2_with_the_dedicated_message() {
    let bin = env!("CARGO_BIN_EXE_corrald");
    let out = Command::new(bin)
        .args([
            "fleet",
            "add",
            "x",
            "--gh",
            "o/r",
            "--models",
            "orch=a,impl=b,review=c,impl_alt=d",
        ])
        .output()
        .expect("run");
    assert_eq!(out.status.code(), Some(2), "parse refusal is usage (2)");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("not settable here"),
        "the dedicated alt-slot message, not the unknown-key one: {stderr}"
    );
}

/// R2a: a symlinked registry is followed — the write replaces the TARGET
/// file and the link survives (the dotfiles-checkout arrangement).
#[cfg(unix)]
#[test]
fn write_through_symlinked_registry_preserves_the_link() {
    let dir = tempfile::tempdir().expect("temp dir");
    let target_dir = dir.path().join("dotfiles");
    std::fs::create_dir(&target_dir).expect("create target dir");
    let target = write_registry(&target_dir, &format!(r#"{{ "fleets": [{VALID_B}] }}"#));
    let link = dir.path().join("fleets.json");
    std::os::unix::fs::symlink(&target, &link).expect("symlink registry");

    let before = std::fs::read(&target).expect("target before");
    let registry = load(&link).expect("loads through the link");
    write_atomic(&link, &registry).expect("writes through the link");

    let meta = std::fs::symlink_metadata(&link).expect("stat link");
    assert!(meta.is_symlink(), "the link survives the rewrite");
    // Non-vacuous: write_atomic normalises formatting, so the target's
    // bytes MUST change if (and only where) the write landed on it.
    let written = std::fs::read(&target).expect("target after");
    assert_ne!(
        written, before,
        "the TARGET file received the (re-formatted) write"
    );
    let text = String::from_utf8(written).expect("utf8");
    assert!(text.contains("\"board\""), "content survives: {text}");
}

/// R2b: the registry's file mode survives the temp-file+rename replace —
/// a `chmod 600` registry must not silently widen to 0644.
#[cfg(unix)]
#[test]
fn write_preserves_the_registry_file_mode() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().expect("temp dir");
    let path = write_registry(dir.path(), &format!(r#"{{ "fleets": [{VALID_A}] }}"#));
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).expect("chmod 600");

    let registry = load(&path).expect("loads");
    write_atomic(&path, &registry).expect("writes");

    let mode = std::fs::metadata(&path).expect("stat").permissions().mode() & 0o777;
    assert_eq!(mode, 0o600, "mode preserved through the replace");
}

// ---------------------------------------------------------------------------
// Write path (slice 2): `fleet pause` / `resume` / `models` — idempotent
// pauses, per-slot model updates, `all` semantics, alt-slot clearing, and
// byte-identical-on-refusal.
// ---------------------------------------------------------------------------

/// A registry fixture with the two fleets from VALID_A/VALID_B plus the
/// optional alt slots populated on `corral`, so the "preserves the others"
/// assertions have something real to preserve.
fn two_fleet_registry_body() -> String {
    r#"{"fleets": [
        {
            "name": "corral",
            "gh_repo": "jirathip-k/corral",
            "local": "~/Projects/corral",
            "worktree_dir": "corral",
            "orch": "orch-corral",
            "workers": ["p4-w1"],
            "models": { "orch": "fable", "impl": "opencode-go/deepseek-v4-flash", "impl_alt": "alt-a", "impl_alt2": "alt-b", "review": "opus" }
        },
        {
            "name": "board",
            "gh_repo": "jirathip-k/herdr-board",
            "local": "/opt/board",
            "worktree_dir": "board",
            "orch": "orch-board",
            "workers": [],
            "models": { "orch": "fable", "impl": "sonnet", "review": "opus" }
        }
    ]}"#
    .to_string()
}

#[test]
fn pause_sets_paused_true_in_written_file() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = write_registry(dir.path(), &two_fleet_registry_body());

    let changed = corrald::fleet::ops::pause(&path, "corral").expect("pause succeeds");
    assert!(changed, "pause changed the registry");

    let registry = load(&path).expect("written file re-parses");
    let corral = registry
        .fleets
        .iter()
        .find(|f| f.name == "corral")
        .expect("corral present");
    assert!(corral.paused, "paused: true set");
    let board = registry
        .fleets
        .iter()
        .find(|f| f.name == "board")
        .expect("board present");
    assert!(!board.paused, "other fleet untouched");
    let text = std::fs::read_to_string(&path).expect("read written registry");
    assert!(
        text.contains("\"paused\": true"),
        "written explicitly: {text}"
    );
}

#[test]
fn resume_clears_paused_and_omits_the_false_per_skip_rule() {
    let dir = tempfile::tempdir().expect("temp dir");
    // Start paused.
    let body = r#"{"fleets": [
            { "name": "corral", "gh_repo": "jirathip-k/corral", "local": "~/p",
               "worktree_dir": "corral", "orch": "o", "workers": [],
               "paused": true,
               "models": {"orch": "f", "impl": "i", "impl_alt": "a", "impl_alt2": "b", "review": "r"} }
        ]}"#;
    let path = write_registry(dir.path(), body);

    let changed = corrald::fleet::ops::resume(&path, "corral").expect("resume succeeds");
    assert!(changed, "resume changed the registry");

    let registry = load(&path).expect("written file re-parses");
    assert!(!registry.fleets[0].paused, "paused cleared");
    let text = std::fs::read_to_string(&path).expect("read written registry");
    assert!(
        !text.contains("\"paused\""),
        "paused false is OMITTED per the skip rule: {text}"
    );
    // The alt slots survive the rewrite untouched.
    assert_eq!(
        registry.fleets[0].models.impl_alt.as_deref(),
        Some("a"),
        "impl_alt survives resume"
    );
    assert_eq!(
        registry.fleets[0].models.impl_alt2.as_deref(),
        Some("b"),
        "impl_alt2 survives resume"
    );
}

#[test]
fn pause_and_resume_are_idempotent_no_ops_leaving_file_byte_identical() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = write_registry(dir.path(), &two_fleet_registry_body());
    // Normalise the fixture through a write first so the comparison baseline
    // is the canonical serialisation.
    let registry = load(&path).expect("load");
    write_atomic(&path, &registry).expect("normalise write");
    let before = std::fs::read(&path).expect("read registry");

    // Pausing an unpaused fleet writes; the second pause is a no-op.
    assert!(corrald::fleet::ops::pause(&path, "corral").expect("pause"));
    let after_first = std::fs::read(&path).expect("read registry");
    assert_ne!(before, after_first, "first pause changes the file");
    assert!(!corrald::fleet::ops::pause(&path, "corral").expect("second pause no-op"));
    assert_eq!(
        std::fs::read(&path).expect("read registry"),
        after_first,
        "re-pause leaves the file byte-identical"
    );

    // Resuming a paused fleet writes; resuming the now-unpaused fleet is a no-op.
    assert!(corrald::fleet::ops::resume(&path, "corral").expect("resume"));
    let after_resume = std::fs::read(&path).expect("read registry");
    assert_ne!(after_first, after_resume, "resume changes the file");
    assert!(!corrald::fleet::ops::resume(&path, "corral").expect("re-resume no-op"));
    assert_eq!(
        std::fs::read(&path).expect("read registry"),
        after_resume,
        "re-resume leaves the file byte-identical"
    );
}

#[test]
fn models_updates_only_the_named_slots_and_preserves_others() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = write_registry(dir.path(), &two_fleet_registry_body());

    let update = ModelUpdate {
        orch: Some("new-orch".to_string()),
        impl_: None,
        impl_alt: None,
        impl_alt2: None,
        review: None,
    };
    let changes = corrald::fleet::ops::models(&path, "corral", &update).expect("models succeeds");
    assert_eq!(changes.len(), 1, "exactly one fleet updated");
    assert_eq!(changes[0].name, "corral");

    let registry = load(&path).expect("written file re-parses");
    let corral = registry
        .fleets
        .iter()
        .find(|f| f.name == "corral")
        .expect("corral present");
    assert_eq!(corral.models.orch, "new-orch", "orch updated");
    assert_eq!(
        corral.models.impl_, "opencode-go/deepseek-v4-flash",
        "impl preserved"
    );
    assert_eq!(
        corral.models.impl_alt.as_deref(),
        Some("alt-a"),
        "impl_alt preserved"
    );
    assert_eq!(
        corral.models.impl_alt2.as_deref(),
        Some("alt-b"),
        "impl_alt2 preserved"
    );
    assert_eq!(corral.models.review, "opus", "review preserved");

    let board = registry
        .fleets
        .iter()
        .find(|f| f.name == "board")
        .expect("board present");
    assert_eq!(board.models.orch, "fable", "other fleet's orch untouched");
}

#[test]
fn models_with_empty_required_value_is_usage_exit_2() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = write_registry(dir.path(), &two_fleet_registry_body());
    let before = std::fs::read(&path).expect("read registry");

    // At the ops layer: Empty error, exit code 2.
    let update = ModelUpdate {
        orch: Some(String::new()),
        impl_: None,
        impl_alt: None,
        impl_alt2: None,
        review: None,
    };
    let err =
        corrald::fleet::ops::models(&path, "corral", &update).expect_err("empty orch refused");
    assert!(
        matches!(err, ConfigError::ModelsRequest { ref field } if field == "models.orch"),
        "kind: {err:?}"
    );
    assert_eq!(err.exit_code(), 2, "usage error exits 2");
    assert_eq!(
        std::fs::read(&path).expect("read registry"),
        before,
        "file byte-identical"
    );

    // Binary-level: --orch '' exits 2 too.
    let out = Command::new(env!("CARGO_BIN_EXE_corrald"))
        .args(["fleet", "models", "corral", "--orch", "", "--registry"])
        .arg(&path)
        .output()
        .expect("run corrald fleet models");
    assert_eq!(out.status.code(), Some(2), "empty required value exits 2");
}

#[test]
fn impl_alt_empty_value_clears_the_slot_from_written_json() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = write_registry(dir.path(), &two_fleet_registry_body());

    let update = ModelUpdate {
        orch: None,
        impl_: None,
        impl_alt: Some(String::new()),
        impl_alt2: None,
        review: None,
    };
    let changes = corrald::fleet::ops::models(&path, "corral", &update).expect("models succeeds");
    assert_eq!(changes.len(), 1);
    assert_eq!(
        changes[0].before.impl_alt.as_deref(),
        Some("alt-a"),
        "before had impl_alt"
    );
    assert_eq!(changes[0].after.impl_alt, None, "impl_alt cleared");

    let text = std::fs::read_to_string(&path).expect("read written registry");
    assert!(
        !text.contains("\"impl_alt\""),
        "cleared slot omitted from written JSON: {text}"
    );
    // impl_alt2 is untouched and still present.
    assert!(text.contains("impl_alt2"), "impl_alt2 preserved: {text}");

    let registry = load(&path).expect("written file re-parses");
    let corral = registry
        .fleets
        .iter()
        .find(|f| f.name == "corral")
        .expect("corral present");
    assert_eq!(corral.models.impl_alt, None, "slot gone");
    assert_eq!(
        corral.models.impl_alt2.as_deref(),
        Some("alt-b"),
        "impl_alt2 intact"
    );
    assert_eq!(corral.models.orch, "fable", "orch untouched");
    assert_eq!(
        corral.models.impl_, "opencode-go/deepseek-v4-flash",
        "impl untouched"
    );
}

#[test]
fn models_all_hits_every_fleet() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = write_registry(dir.path(), &two_fleet_registry_body());

    let update = ModelUpdate {
        orch: None,
        impl_: Some("shared-impl".to_string()),
        impl_alt: None,
        impl_alt2: None,
        review: None,
    };
    let changes = corrald::fleet::ops::models(&path, "all", &update).expect("models all succeeds");
    assert_eq!(changes.len(), 2, "both fleets updated");
    assert_eq!(changes[0].name, "corral");
    assert_eq!(changes[1].name, "board");

    let registry = load(&path).expect("written file re-parses");
    for fleet in &registry.fleets {
        assert_eq!(
            fleet.models.impl_, "shared-impl",
            "{} impl updated",
            fleet.name
        );
    }
    // Binary-level: `models all` prints both fleets.
    let out = Command::new(env!("CARGO_BIN_EXE_corrald"))
        .args(["fleet", "models", "all", "--impl", "bin-impl", "--registry"])
        .arg(&path)
        .output()
        .expect("run corrald fleet models all");
    assert!(out.status.success(), "models all exits 0: {:?}", out.status);
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("corral"), "corral line: {text}");
    assert!(text.contains("board"), "board line: {text}");
}

#[test]
fn models_unknown_name_is_refusal_exit_1_leaving_file_byte_identical() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = write_registry(dir.path(), &two_fleet_registry_body());
    let before = std::fs::read(&path).expect("read registry");

    let update = ModelUpdate {
        orch: Some("x".to_string()),
        impl_: None,
        impl_alt: None,
        impl_alt2: None,
        review: None,
    };
    let err =
        corrald::fleet::ops::models(&path, "no-such", &update).expect_err("unknown name refused");
    assert!(
        matches!(err, ConfigError::FleetNotFound { ref name } if name == "no-such"),
        "kind: {err:?}"
    );
    assert_eq!(err.exit_code(), 1, "refusal exits 1");
    assert_eq!(
        std::fs::read(&path).expect("read registry"),
        before,
        "file byte-identical"
    );

    // Binary-level, like fleet_remove_unknown_name_exits_1_not_2.
    let out = Command::new(env!("CARGO_BIN_EXE_corrald"))
        .args(["fleet", "models", "ghost", "--orch", "x", "--registry"])
        .arg(&path)
        .output()
        .expect("run corrald fleet models");
    assert_eq!(out.status.code(), Some(1), "unknown name exits 1, not 2");
}

#[test]
fn pause_and_resume_unknown_name_exit_1_leaving_file_byte_identical() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = write_registry(dir.path(), &two_fleet_registry_body());
    let before = std::fs::read(&path).expect("read registry");

    for sub in ["pause", "resume"] {
        let out = Command::new(env!("CARGO_BIN_EXE_corrald"))
            .args(["fleet", sub, "no-such", "--registry"])
            .arg(&path)
            .output()
            .expect("run corrald fleet {sub}");
        assert_eq!(out.status.code(), Some(1), "{sub} unknown name exits 1");
        assert_eq!(
            std::fs::read(&path).expect("read registry"),
            before,
            "{sub} refusal leaves the file byte-identical"
        );
    }
}

#[test]
fn pause_resume_models_usage_errors_exit_2_and_help_documents_them() {
    let bin = env!("CARGO_BIN_EXE_corrald");

    let no_name = Command::new(bin)
        .args(["fleet", "pause"])
        .env("CORRAL_FLEETS_PATH", "/nonexistent/never-touched.json")
        .output()
        .expect("run");
    assert_eq!(no_name.status.code(), Some(2), "pause without name exits 2");

    let two_names = Command::new(bin)
        .args(["fleet", "resume", "a", "b"])
        .env("CORRAL_FLEETS_PATH", "/nonexistent/never-touched.json")
        .output()
        .expect("run");
    assert_eq!(
        two_names.status.code(),
        Some(2),
        "resume with two names exits 2"
    );

    let models_no_flags = Command::new(bin)
        .args(["fleet", "models", "corral"])
        .env("CORRAL_FLEETS_PATH", "/nonexistent/never-touched.json")
        .output()
        .expect("run");
    assert_eq!(
        models_no_flags.status.code(),
        Some(2),
        "models with no flags exits 2"
    );

    let models_no_name = Command::new(bin)
        .args(["fleet", "models", "--orch", "x"])
        .env("CORRAL_FLEETS_PATH", "/nonexistent/never-touched.json")
        .output()
        .expect("run");
    assert_eq!(
        models_no_name.status.code(),
        Some(2),
        "models without name exits 2"
    );

    let help = Command::new(bin)
        .args(["fleet", "--help"])
        .env("CORRAL_FLEETS_PATH", "/nonexistent/never-touched.json")
        .output()
        .expect("run");
    assert!(help.status.success(), "help exits 0");
    let text = String::from_utf8_lossy(&help.stdout);
    assert!(text.contains("fleet pause"), "documents pause: {text}");
    assert!(text.contains("fleet resume"), "documents resume: {text}");
    assert!(text.contains("fleet models"), "documents models: {text}");
    assert!(text.contains("--impl-alt"), "documents --impl-alt: {text}");
    assert!(
        text.contains("--impl-alt2"),
        "documents --impl-alt2: {text}"
    );
}

#[test]
fn models_binary_round_trip_preserves_alt_slots_and_prints_old_to_new() {
    // e2e through the real binary: set both alt slots, then a later models
    // run on an unrelated slot must keep them, and stdout must show old -> new.
    let dir = tempfile::tempdir().expect("temp dir");
    let path = write_registry(dir.path(), &two_fleet_registry_body());

    let out = Command::new(env!("CARGO_BIN_EXE_corrald"))
        .args([
            "fleet",
            "models",
            "board",
            "--impl",
            "gpt-5.6-luna",
            "--registry",
        ])
        .arg(&path)
        .output()
        .expect("run corrald fleet models");
    assert!(out.status.success(), "models exits 0: {:?}", out.status);
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.contains("board models changed:") && text.contains("sonnet -> gpt-5.6-luna"),
        "prints old -> new: {text}"
    );

    let registry = load(&path).expect("written file re-parses");
    let board = registry
        .fleets
        .iter()
        .find(|f| f.name == "board")
        .expect("board present");
    assert_eq!(board.models.impl_, "gpt-5.6-luna");
    assert_eq!(board.models.review, "opus", "review preserved");
}

#[test]
fn pause_exit_0_with_already_paused_message() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = write_registry(dir.path(), &two_fleet_registry_body());
    let registry = load(&path).expect("load");
    write_atomic(&path, &registry).expect("normalise write");

    corrald::fleet::ops::pause(&path, "corral").expect("pause");
    let out = Command::new(env!("CARGO_BIN_EXE_corrald"))
        .args(["fleet", "pause", "corral", "--registry"])
        .arg(&path)
        .output()
        .expect("run corrald fleet pause");
    assert!(
        out.status.success(),
        "already-paused exits 0: {:?}",
        out.status
    );
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("already paused"), "says so: {text}");

    let resume_out = Command::new(env!("CARGO_BIN_EXE_corrald"))
        .args(["fleet", "resume", "corral", "--registry"])
        .arg(&path)
        .output()
        .expect("run corrald fleet resume");
    assert!(
        resume_out.status.success(),
        "resume exits 0: {:?}",
        resume_out.status
    );
    let rtext = String::from_utf8_lossy(&resume_out.stdout);
    assert!(
        rtext.contains("resumed fleet corral"),
        "resume message: {rtext}"
    );
}

// ---------------------------------------------------------------------------
// Slice-2 review round: `all` reservation, empty-registry refusal, models
// idempotence, impl_alt2 clearing (findings 1, 2, 7, 10a/10c).
// ---------------------------------------------------------------------------

#[test]
fn fleet_named_all_is_refused_by_validate() {
    // `all` is the `fleet models` wildcard — a fleet by that name would be
    // unreachable (models) and misleading (pause/resume), so it is reserved.
    let dir = tempfile::tempdir().expect("temp dir");
    let path = write_registry(
        dir.path(),
        &fleet_body("all", "o/r", "/tmp/x", "[]", MODELS),
    );
    let err = load(&path).expect_err("reserved name must fail");
    assert!(
        matches!(err, ConfigError::ReservedFleetName { ref name } if name == "all"),
        "kind: {err:?}"
    );
    assert_eq!(err.exit_code(), 2, "validation failure class");

    // And end-to-end: `fleet add all` is refused before any write.
    let empty = write_registry(&dir.path().join_and_create("e"), r#"{"fleets": []}"#);
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_corrald"))
        .args([
            "fleet",
            "add",
            "all",
            "--gh",
            "o/r",
            "--models",
            "orch=a,impl=b,review=c",
        ])
        .args(["--registry", empty.to_str().expect("utf8")])
        .output()
        .expect("run");
    assert_eq!(
        out.status.code(),
        Some(2),
        "reserved name is a validation refusal"
    );
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("reserved"),
        "names the reservation: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Helper: create a subdirectory and return it joined (tempdir fixtures
/// need distinct dirs for distinct registries).
trait JoinAndCreate {
    fn join_and_create(&self, sub: &str) -> PathBuf;
}
impl JoinAndCreate for std::path::Path {
    fn join_and_create(&self, sub: &str) -> PathBuf {
        let dir = self.join(sub);
        std::fs::create_dir_all(&dir).expect("create fixture dir");
        dir
    }
}

#[test]
fn models_all_on_empty_registry_is_a_refusal_not_silent_success() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = write_registry(dir.path(), r#"{"fleets": []}"#);
    let before = std::fs::read(&path).expect("read registry");

    let update = corrald::fleet::ops::ModelUpdate {
        orch: None,
        impl_: Some("x".to_string()),
        impl_alt: None,
        impl_alt2: None,
        review: None,
    };
    let err = corrald::fleet::ops::models(&path, "all", &update)
        .expect_err("zero-fleet bulk update must refuse");
    assert!(matches!(err, ConfigError::NoFleets), "kind: {err:?}");
    assert_eq!(err.exit_code(), 1, "operational refusal");
    assert_eq!(
        std::fs::read(&path).expect("read registry"),
        before,
        "file byte-identical — no pointless rewrite"
    );

    // Binary-level: exit 1, and stderr says what happened.
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_corrald"))
        .args(["fleet", "models", "all", "--impl", "x"])
        .args(["--registry", path.to_str().expect("utf8")])
        .output()
        .expect("run");
    assert_eq!(out.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("no fleets"),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn models_with_unchanged_values_is_an_idempotent_no_op() {
    // Same discipline as pause/resume (review finding 7): nothing moved →
    // nothing written, and the binary says so instead of printing x -> x.
    let dir = tempfile::tempdir().expect("temp dir");
    // Two fleets (g78 review F6): a regression ignoring `name` and
    // stamping every fleet must fail the len==1 + name pins below.
    let path = write_registry(
        dir.path(),
        &format!(r#"{{ "fleets": [{VALID_A}, {VALID_B}] }}"#),
    );

    // First run is a REAL write (g78 review F4): the fixture holds
    // review "opus", the update sets a different value — so the
    // byte-identical assertion below guards the write→no-op transition
    // instead of comparing the untouched fixture to itself.
    let update = corrald::fleet::ops::ModelUpdate {
        orch: None,
        impl_: None,
        impl_alt: None,
        impl_alt2: None,
        review: Some("gpt-5.6-luna".to_string()),
    };
    let first = corrald::fleet::ops::models(&path, "corral", &update).expect("first run");
    assert_eq!(first.len(), 1, "only the targeted fleet");
    assert_eq!(first[0].name, "corral");
    assert_eq!(first[0].before.review, "opus");
    assert_eq!(first[0].after.review, "gpt-5.6-luna");
    let after_first = std::fs::read(&path).expect("read");
    let mtime_first = std::fs::metadata(&path)
        .expect("stat")
        .modified()
        .expect("mtime");

    let changes = corrald::fleet::ops::models(&path, "corral", &update).expect("second run");
    // R2-1 (#78): pin the shape, not just the all() predicate — an empty
    // change list (a regression skipping the fleet) would pass `all()`
    // vacuously.
    assert_eq!(changes.len(), 1, "exactly the one targeted fleet reported");
    assert_eq!(changes[0].name, "corral");
    assert_eq!(changes[0].before.review, "gpt-5.6-luna");
    assert_eq!(changes[0].after.review, "gpt-5.6-luna");
    assert!(
        changes.iter().all(|c| c.before == c.after),
        "second run is a no-op"
    );
    assert_eq!(
        std::fs::read(&path).expect("read"),
        after_first,
        "no-op writes nothing — byte-identical"
    );
    // g78 review R2-1: byte-identical alone cannot see an always-rewrite
    // regression (a rewrite of canonical JSON is byte-identical too) —
    // the mtime pin proves the no-op never touched the file at all.
    assert_eq!(
        std::fs::metadata(&path)
            .expect("stat")
            .modified()
            .expect("mtime"),
        mtime_first,
        "no-op does not rewrite the file at all"
    );

    let out = std::process::Command::new(env!("CARGO_BIN_EXE_corrald"))
        .args(["fleet", "models", "corral", "--review", "gpt-5.6-luna"])
        .args(["--registry", path.to_str().expect("utf8")])
        .output()
        .expect("run");
    assert!(out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("nothing to do"),
        "stdout: {}",
        String::from_utf8_lossy(&out.stdout)
    );
}

#[test]
fn impl_alt2_empty_value_clears_the_slot_from_written_json() {
    // 10a: the two alt slots are separate match arms — pin the second one
    // so a copy-paste slip clearing the WRONG slot cannot pass.
    let dir = tempfile::tempdir().expect("temp dir");
    let path = write_registry(dir.path(), &format!(r#"{{ "fleets": [{VALID_ALT}] }}"#));

    let update = corrald::fleet::ops::ModelUpdate {
        orch: None,
        impl_: None,
        impl_alt: None,
        impl_alt2: Some(String::new()),
        review: None,
    };
    corrald::fleet::ops::models(&path, "corral", &update).expect("clear impl_alt2");
    let text = std::fs::read_to_string(&path).expect("read");
    assert!(!text.contains("impl_alt2"), "impl_alt2 cleared: {text}");
    assert!(
        text.contains("\"impl_alt\""),
        "impl_alt (the OTHER slot) survives: {text}"
    );
}

// ---------------------------------------------------------------------------
// #66: registry default path — corral-owned, legacy fallback.
// ---------------------------------------------------------------------------

/// End-to-end (#66): with no env override, a fresh HOME resolves to
/// `~/.config/corral/fleets.json` (the error names it), and a HOME carrying
/// only the legacy `~/.hermes/scripts/fleets.json` still works.
#[test]
fn default_registry_path_is_corral_owned_with_legacy_fallback() {
    let bin = env!("CARGO_BIN_EXE_corrald");

    // Fresh HOME: the missing-file error must point at the corral path.
    let fresh = tempfile::tempdir().expect("temp home");
    let out = Command::new(bin)
        .args(["fleet", "list"])
        .env("HOME", fresh.path())
        .env_remove("CORRAL_FLEETS_PATH")
        .env_remove("CORRAL_CONFIG_DIR")
        .output()
        .expect("run");
    assert_eq!(out.status.code(), Some(2), "missing registry is exit 2");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains(".config/corral/fleets.json"),
        "fresh machines get the corral-owned default: {stderr}"
    );
    assert!(
        !stderr.contains(".hermes"),
        "no legacy path leaks on a fresh machine: {stderr}"
    );

    // Legacy HOME: only ~/.hermes/scripts/fleets.json exists — list works.
    let legacy_home = tempfile::tempdir().expect("temp home");
    let legacy_dir = legacy_home.path().join(".hermes/scripts");
    std::fs::create_dir_all(&legacy_dir).expect("mkdir legacy");
    std::fs::write(
        legacy_dir.join("fleets.json"),
        format!(r#"{{ "fleets": [{VALID_B}] }}"#),
    )
    .expect("write legacy registry");
    let out = Command::new(bin)
        .args(["fleet", "list"])
        .env("HOME", legacy_home.path())
        .env_remove("CORRAL_FLEETS_PATH")
        .env_remove("CORRAL_CONFIG_DIR")
        .output()
        .expect("run");
    assert!(
        out.status.success(),
        "legacy fallback still works: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("board"),
        "lists the legacy registry's fleet"
    );
    // The fallback is LOUD (#66 review F5): a stderr note announces the
    // legacy path so a split-brain migration cannot stay silent.
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("legacy registry"),
        "the fallback announces itself: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

// ---------------------------------------------------------------------------
// #35 watchdog CLI (review F12): the hermetic paths — everything that
// returns before any shell-out.
// ---------------------------------------------------------------------------

#[test]
fn fleet_watch_usage_and_help_are_hermetic() {
    let bin = env!("CARGO_BIN_EXE_corrald");
    let bogus = Command::new(bin)
        .args(["fleet", "watch", "--bogus"])
        .env("CORRAL_FLEETS_PATH", "/nonexistent/never-touched.json")
        .output()
        .expect("run");
    assert_eq!(bogus.status.code(), Some(2), "unknown flag is usage");

    let help = Command::new(bin)
        .args(["fleet", "watch", "--help"])
        .output()
        .expect("run");
    assert!(help.status.success());
    assert!(
        String::from_utf8_lossy(&help.stdout).contains("watch"),
        "help documents watch"
    );
}

/// Review F1/F2: a corrupt or missing registry is a PROBLEM line on
/// STDOUT with exit 1 — the monitor must alert on the failure that stops
/// it watching, never die silently to stderr with an ambiguous code.
#[test]
fn fleet_watch_bad_registry_alerts_on_stdout_exit_1() {
    let bin = env!("CARGO_BIN_EXE_corrald");
    let dir = tempfile::tempdir().expect("temp dir");

    // Missing file.
    let missing = Command::new(bin)
        .args(["fleet", "watch", "--registry"])
        .arg(dir.path().join("nope.json"))
        .output()
        .expect("run");
    assert_eq!(missing.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&missing.stdout).contains("PROBLEM: fleet registry"),
        "stdout: {}",
        String::from_utf8_lossy(&missing.stdout)
    );

    // Validation failure that maps to exit 1 at load (duplicate name) —
    // watch must still be 1-with-PROBLEM, not a bare refusal.
    let dup = write_registry(
        dir.path(),
        &format!(r#"{{ "fleets": [{VALID_A}, {VALID_A}] }}"#),
    );
    let out = Command::new(bin)
        .args(["fleet", "watch", "--registry"])
        .arg(&dup)
        .output()
        .expect("run");
    assert_eq!(out.status.code(), Some(1));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("PROBLEM: fleet registry") && stdout.contains("duplicate"),
        "names the corruption: {stdout}"
    );
}

/// Fresh-review N1: a herdr child that answers COMPLETELY and exits 0,
/// but leaves a background process holding the stdout write end, must
/// NOT read as "server NOT reachable" — the reader publishes as it
/// goes, the grace expiry returns the buffer, and the parse decides.
/// (The old EOF-only reader threw the complete listing away and
/// suppressed every per-fleet check behind a false server-down line.)
#[test]
fn watch_survives_a_grandchild_holding_stdout() {
    let dir = tempfile::tempdir().expect("temp dir");
    // Round-2 N9: an UNPAUSED fleet, so the per-fleet checks actually
    // consume the listing content — the healthy verdict below then
    // depends on name/status/cwd surviving the pipe intact, which also
    // pins round-2 N8 (the listing is padded past 8KiB with a non-ASCII
    // char parked at the old chunk boundary; a per-chunk lossy decode
    // corrupted it and produced a false MISSING).
    // Round-3 N9-residual: the boundary-straddling char lives in the
    // orchestrator NAME — the one field the healthy verdict actually
    // reads (registry lookup) — so a per-chunk decode regression turns
    // this test red (false MISSING), not just the comment.
    let unpaused = VALID_A
        .replace(r#""paused": true,"#, "")
        .replace(r#""orch": "orch-corral""#, r#""orch": "orch-corralé""#);
    let path = write_registry(dir.path(), &format!(r#"{{ "fleets": [{unpaused}] }}"#));

    // Hermetic herdr shim: valid listing, exit 0, fd-holder outliving
    // the 5s EOF grace (but not the test runner).
    let shim_dir = dir.path().join("bin");
    std::fs::create_dir(&shim_dir).expect("mkdir");
    let shim = shim_dir.join("herdr");
    // Pad so a two-byte UTF-8 char (é) straddles the reader's 8KiB
    // boundary: everything before the agents array is ASCII filler
    // sized so the é inside the first cwd lands at bytes 8191/8192.
    let mut listing = String::from("{\"result\":{\"pad\":\"");
    let prefix_after_pad = "\",\"agents\":[{\"name\":\"orch-corralé";
    // Place é's FIRST byte exactly at offset 8191 so 0xC3|0xA9 straddle
    // the reader's 8KiB chunk boundary (the N8 repro geometry) — INSIDE
    // the name the registry lookup reads (round-3 N9-residual).
    let e_off = prefix_after_pad.find('é').expect("é in prefix");
    let pad = 8191usize.saturating_sub(listing.len() + e_off);
    listing.push_str(&"a".repeat(pad));
    listing.push_str(prefix_after_pad);
    listing.push_str("\",\"agent_status\":\"working\",\"cwd\":\"/repos/corral\"},{\"name\":\"p4-w1\",\"agent_status\":\"idle\",\"cwd\":\"/repos/corral\"},{\"name\":\"p4-w1-reviewer\",\"agent_status\":\"idle\",\"cwd\":\"/repos/corral\"}]}}");
    std::fs::write(
        &shim,
        format!("#!/bin/sh\n( sleep 8 ) &\necho '{listing}'\nexit 0\n"),
    )
    .expect("write shim");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&shim, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    }
    // Stub gh too so a stall arm could never hang on the network (the
    // fleet is healthy here, so gh is never called — N7 — but belt and
    // braces for a hermetic test).
    std::fs::write(shim_dir.join("gh"), "#!/bin/sh\necho 0\n").expect("write gh");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(shim_dir.join("gh"), std::fs::Permissions::from_mode(0o755))
            .expect("chmod gh");
    }

    let out = std::process::Command::new(env!("CARGO_BIN_EXE_corrald"))
        .args(["fleet", "watch", "--registry", path.to_str().expect("utf8")])
        .env(
            "PATH",
            format!(
                "{}:{}",
                shim_dir.display(),
                std::env::var("PATH").unwrap_or_default()
            ),
        )
        .env("HOME", dir.path().to_str().expect("utf8"))
        .output()
        .expect("watch runs");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        !stdout.contains("NOT reachable"),
        "a complete answer with a held fd must not be a false server-down: {stdout}"
    );
    // Unpaused fleet + intact listing content (orch working, both
    // workers present, non-ASCII cwd surviving the chunk boundary — N8)
    // = genuinely healthy.
    assert!(
        !stdout.contains("MISSING"),
        "listing content must survive the pipe byte-for-byte (N8): {stdout}"
    );
    assert!(stdout.contains("ALL HEALTHY"), "{stdout}");
    assert!(out.status.success(), "exit 0 on healthy: {stdout}");
}
