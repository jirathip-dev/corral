//! The configless, allowlisted sidecar plugin engine.
//!
//! Plugins are discovered only below `~/.config/corral/plugins`; `fleet-ops` is
//! the sole accepted id. Commands are parsed as argv arrays and are never
//! assembled from client input.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use serde::{Deserialize, Serialize};

const ALLOWED_ID: &str = "fleet-ops";
const DEFAULT_INTERVAL: u64 = 60;
const OUTPUT_LIMIT: usize = 256 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginManifest {
    pub id: String,
    pub name: String,
    pub version: String,
    pub platforms: Vec<String>,
    pub plugin_schema: String,
    pub cards: Vec<CardSpec>,
    pub actions: Vec<ActionSpec>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CardSpec {
    pub id: String,
    pub title: String,
    pub command: Vec<String>,
    pub interval_sec: u64,
    pub json: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActionSpec {
    pub id: String,
    pub title: String,
    pub command: Vec<String>,
    pub confirm_message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CardResult {
    pub id: String,
    pub title: String,
    pub value: serde_json::Value,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ActionResult {
    pub action_id: String,
    pub argv: Vec<String>,
    pub stdout: String,
    pub stderr: String,
    pub exit_code: Option<i32>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PluginView {
    pub name: String,
    pub version: String,
    pub cards: Vec<CardResult>,
    pub actions: Vec<ActionSpec>,
}

pub fn plugin_dir(config_dir: &Path) -> PathBuf {
    config_dir.join("plugins")
}

/// Load exactly one allowlisted manifest. Unknown plugin directories are
/// ignored, including malformed manifests, so a third-party crash cannot
/// prevent corrald from starting.
pub fn discover(config_dir: &Path) -> Result<Option<PluginManifest>, String> {
    let root = plugin_dir(config_dir);
    let entries = match std::fs::read_dir(root) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.to_string()),
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if entry.file_name() != ALLOWED_ID || !path.is_dir() {
            tracing::warn!(plugin = ?entry.file_name(), "ignoring non-allowlisted plugin");
            continue;
        }
        let file = path.join("plugin.toml");
        let text = std::fs::read_to_string(file).map_err(|e| e.to_string())?;
        return parse_manifest(&text).map(Some);
    }
    Ok(None)
}

/// Small deterministic TOML v1 reader for the fixed plugin schema. It accepts
/// scalar fields and `[[cards]]`/`[[actions]]` tables, and rejects everything
/// else rather than guessing at command semantics.
pub fn parse_manifest(text: &str) -> Result<PluginManifest, String> {
    let mut id = None;
    let mut name = None;
    let mut version = None;
    let mut platforms = None;
    let mut schema = None;
    let mut cards = Vec::new();
    let mut actions = Vec::new();
    let mut section = "root";
    let mut fields = std::collections::BTreeMap::<String, String>::new();
    let flush = |section: &str,
                 f: &mut std::collections::BTreeMap<String, String>,
                 cards: &mut Vec<CardSpec>,
                 actions: &mut Vec<ActionSpec>|
     -> Result<(), String> {
        if section == "cards" {
            cards.push(CardSpec {
                id: take(f, "id")?,
                title: take(f, "title")?,
                command: take_array(f, "command")?,
                interval_sec: f
                    .remove("interval_sec")
                    .map(|v| {
                        v.parse()
                            .map_err(|_| "interval_sec must be integer".to_string())
                    })
                    .transpose()?
                    .unwrap_or(DEFAULT_INTERVAL),
                json: f
                    .remove("json")
                    .map(|v| parse_bool(&v))
                    .transpose()?
                    .unwrap_or(false),
            });
        }
        if section == "actions" {
            actions.push(ActionSpec {
                id: take(f, "id")?,
                title: take(f, "title")?,
                command: take_array(f, "command")?,
                confirm_message: take(f, "confirm_message")?,
            });
        }
        if !f.is_empty() {
            return Err(format!(
                "unknown plugin fields: {}",
                f.keys().cloned().collect::<Vec<_>>().join(", ")
            ));
        }
        Ok(())
    };
    for raw in text.lines() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        if line == "[[cards]]" || line == "[[actions]]" {
            flush(section, &mut fields, &mut cards, &mut actions)?;
            section = if line.contains("cards") {
                "cards"
            } else {
                "actions"
            };
            continue;
        }
        let (key, val) = line
            .split_once('=')
            .ok_or_else(|| format!("invalid manifest line: {line}"))?;
        if section == "root" {
            match key.trim() {
                "id" => id = Some(string(val)?),
                "name" => name = Some(string(val)?),
                "version" => version = Some(string(val)?),
                "platforms" => platforms = Some(array(val)?),
                "plugin_schema" => schema = Some(string(val)?),
                other => return Err(format!("unknown plugin field: {other}")),
            }
        } else {
            fields.insert(key.trim().to_string(), val.trim().to_string());
        }
    }
    flush(section, &mut fields, &mut cards, &mut actions)?;
    let manifest = PluginManifest {
        id: id.ok_or("missing id")?,
        name: name.ok_or("missing name")?,
        version: version.ok_or("missing version")?,
        platforms: platforms.ok_or("missing platforms")?,
        plugin_schema: schema.ok_or("missing plugin_schema")?,
        cards,
        actions,
    };
    if manifest.id != ALLOWED_ID {
        return Err("plugin id is not allowlisted".into());
    }
    if manifest.plugin_schema != "1" {
        return Err("unsupported plugin_schema".into());
    }
    Ok(manifest)
}

fn take(f: &mut std::collections::BTreeMap<String, String>, key: &str) -> Result<String, String> {
    f.remove(key)
        .ok_or_else(|| format!("missing {key}"))
        .and_then(|v| string(&v))
}
fn take_array(
    f: &mut std::collections::BTreeMap<String, String>,
    key: &str,
) -> Result<Vec<String>, String> {
    f.remove(key)
        .ok_or_else(|| format!("missing {key}"))
        .and_then(|v| array(&v))
}
fn string(v: &str) -> Result<String, String> {
    let v = v.trim();
    if v.len() >= 2 && v.starts_with('"') && v.ends_with('"') {
        Ok(v[1..v.len() - 1].replace("\\\"", "\""))
    } else {
        Err(format!("expected quoted string: {v}"))
    }
}
fn array(v: &str) -> Result<Vec<String>, String> {
    let v = v.trim();
    if !(v.starts_with('[') && v.ends_with(']')) {
        return Err("expected array".into());
    }
    let inner = &v[1..v.len() - 1];
    if inner.trim().is_empty() {
        return Ok(Vec::new());
    }
    inner.split(',').map(string).collect()
}
fn parse_bool(v: &str) -> Result<bool, String> {
    match v.trim() {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err("expected boolean".into()),
    }
}

struct CardSlot {
    started: std::sync::atomic::AtomicBool,
    result: tokio::sync::Mutex<Option<CardResult>>,
}

static CARD_SLOTS: OnceLock<std::sync::Mutex<std::collections::HashMap<String, Arc<CardSlot>>>> =
    OnceLock::new();
static CARD_LIMIT: OnceLock<Arc<tokio::sync::Semaphore>> = OnceLock::new();

fn slot_key(card: &CardSpec) -> String {
    format!(
        "{}|{}|{}|{}",
        card.id,
        card.interval_sec,
        card.json,
        card.command
            .iter()
            .map(|v| format!("{v:?}"))
            .collect::<Vec<_>>()
            .join(",")
    )
}

/// Returns cached card values and starts one bounded, non-overlapping timer per
/// manifest card. The key includes interval and argv, so an edited manifest
/// automatically gets a fresh schedule on the next request.
pub async fn scheduled_cards(manifest: &PluginManifest) -> Vec<CardResult> {
    let slots = CARD_SLOTS.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()));
    let semaphore = CARD_LIMIT
        .get_or_init(|| Arc::new(tokio::sync::Semaphore::new(4)))
        .clone();
    let mut output = Vec::with_capacity(manifest.cards.len());
    for card in &manifest.cards {
        let key = slot_key(card);
        let slot = {
            let mut all = slots.lock().expect("card slots lock");
            all.retain(|known, _| {
                manifest
                    .cards
                    .iter()
                    .any(|candidate| slot_key(candidate) == *known)
            });
            all.entry(key)
                .or_insert_with(|| {
                    Arc::new(CardSlot {
                        started: std::sync::atomic::AtomicBool::new(false),
                        result: tokio::sync::Mutex::new(None),
                    })
                })
                .clone()
        };
        if !slot.started.swap(true, std::sync::atomic::Ordering::AcqRel) {
            let first = run_card_limited(card, &semaphore).await;
            *slot.result.lock().await = Some(first);
            let slot_clone = slot.clone();
            let card_clone = card.clone();
            let semaphore_clone = semaphore.clone();
            tokio::spawn(async move {
                let interval = Duration::from_secs(card_clone.interval_sec.max(1));
                loop {
                    tokio::time::sleep(interval).await;
                    let result = run_card_limited(&card_clone, &semaphore_clone).await;
                    *slot_clone.result.lock().await = Some(result);
                }
            });
        }
        if let Some(result) = slot.result.lock().await.clone() {
            output.push(result);
        }
    }
    output
}

async fn run_card_limited(card: &CardSpec, semaphore: &tokio::sync::Semaphore) -> CardResult {
    let _permit = semaphore.acquire().await.expect("card semaphore");
    run_card(card).await
}
pub async fn run_card(card: &CardSpec) -> CardResult {
    let output = run_argv(&card.command).await;
    match output {
        Ok((out, _err, _)) => {
            let value = if card.json {
                serde_json::from_slice(&out)
                    .unwrap_or_else(|_| serde_json::json!({"error":"invalid JSON"}))
            } else {
                serde_json::Value::String(String::from_utf8_lossy(&out).into_owned())
            };
            CardResult {
                id: card.id.clone(),
                title: card.title.clone(),
                value,
                error: None,
            }
        }
        Err(e) => CardResult {
            id: card.id.clone(),
            title: card.title.clone(),
            value: serde_json::Value::Null,
            error: Some(e),
        },
    }
}

/// Execute a manifest action only after an explicit confirmation. The action
/// id is resolved against the manifest; callers cannot provide an argv.
pub async fn run_action(
    config_dir: &Path,
    action_id: &str,
    confirmed: bool,
) -> Result<ActionResult, String> {
    let manifest = discover(config_dir)?.ok_or("fleet-ops plugin is not installed")?;
    let action = manifest
        .actions
        .iter()
        .find(|a| a.id == action_id)
        .ok_or("unknown plugin action")?;
    if !confirmed {
        return Err("action not confirmed".into());
    }
    let argv = action.command.clone();
    let (stdout, stderr, exit_code) = run_argv(&argv).await?;
    let audit = config_dir.join("../../.hermes/logs/corral-plugin-audit.log");
    if let Some(parent) = audit.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&audit)
        .map_err(|e| e.to_string())?;
    writeln!(
        file,
        "ts={} action_id={} argv={} exit_code={:?}",
        crate::core::util::now_millis(),
        action_id,
        serde_json::to_string(&argv).unwrap_or_default(),
        exit_code
    )
    .map_err(|e| e.to_string())?;
    Ok(ActionResult {
        action_id: action_id.into(),
        argv,
        stdout: String::from_utf8_lossy(&stdout).into_owned(),
        stderr: String::from_utf8_lossy(&stderr).into_owned(),
        exit_code,
        error: None,
    })
}

async fn run_argv(argv: &[String]) -> Result<(Vec<u8>, Vec<u8>, Option<i32>), String> {
    let Some((program, args)) = argv.split_first() else {
        return Err("empty argv".into());
    };
    let mut command = tokio::process::Command::new(program);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut paths =
        std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()).collect::<Vec<_>>();
    let home = PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".into()));
    paths.extend([
        home.join(".local/bin"),
        PathBuf::from("/opt/homebrew/bin"),
        PathBuf::from("/usr/local/bin"),
    ]);
    command.env(
        "PATH",
        std::env::join_paths(paths).map_err(|e| e.to_string())?,
    );
    command.env("CORRAL_PLUGIN_ID", ALLOWED_ID);
    command.kill_on_drop(true);
    let child = command.spawn().map_err(|e| e.to_string())?;
    let output = tokio::time::timeout(Duration::from_secs(30), child.wait_with_output())
        .await
        .map_err(|_| "plugin command timed out".to_string())?
        .map_err(|e| e.to_string())?;
    if output.stdout.len() > OUTPUT_LIMIT {
        return Err("plugin stdout exceeded 256KiB".into());
    }
    if !output.status.success() {
        return Err(format!(
            "plugin exited {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok((output.stdout, output.stderr, output.status.code()))
}

#[cfg(test)]
mod tests {
    use super::*;
    const MANIFEST: &str = r#"id = "fleet-ops"
name = "Fleet Ops"
version = "1.0.0"
platforms = ["macos"]
plugin_schema = "1"

[[cards]]
id = "registry"
title = "Registry"
command = ["printf", "{\"fleets\":3}"]
interval_sec = 15
json = true

[[actions]]
id = "refresh"
title = "Refresh"
command = ["true"]
confirm_message = "Refresh Fleet Ops?"
"#;
    #[test]
    fn parses_fixture_manifest_without_registry_file() {
        let m = parse_manifest(MANIFEST).unwrap();
        assert_eq!(m.cards.len(), 1);
        assert_eq!(m.cards[0].command[0], "printf");
        assert_eq!(m.actions[0].command, vec!["true"]);
    }
    #[test]
    fn rejects_non_allowlisted_id() {
        assert!(parse_manifest(&MANIFEST.replace("fleet-ops", "other")).is_err());
    }
    #[tokio::test]
    async fn scheduled_card_runs_again_after_interval() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("ticks");
        let card = CardSpec {
            id: format!("tick-{}", marker.display()),
            title: "Tick".into(),
            command: vec![
                "sh".into(),
                "-c".into(),
                format!("printf x >> '{}'", marker.display()),
            ],
            interval_sec: 1,
            json: false,
        };
        let manifest = PluginManifest {
            id: ALLOWED_ID.into(),
            name: "fixture".into(),
            version: "1".into(),
            platforms: vec!["macos".into()],
            plugin_schema: "1".into(),
            cards: vec![card],
            actions: vec![],
        };
        scheduled_cards(&manifest).await;
        tokio::time::sleep(Duration::from_millis(1_200)).await;
        assert!(std::fs::metadata(marker).unwrap().len() >= 2);
        let mut changed = manifest.cards[0].clone();
        changed.interval_sec = 2;
        assert_ne!(slot_key(&manifest.cards[0]), slot_key(&changed));
    }
    #[tokio::test]
    async fn failed_plugin_is_an_error_result_not_a_daemon_failure() {
        let card = CardSpec {
            id: "x".into(),
            title: "x".into(),
            command: vec!["false".into()],
            interval_sec: 60,
            json: false,
        };
        assert!(run_card(&card).await.error.is_some());
    }
}
