//! Signed append-only audit log (D10/AC5): [`HashChainAuditLog`].
//!
//! One JSON line per entry at `<config_dir>/audit.log` (0600). Every entry
//! carries `prev` (the SHA-256 of the previous entry's canonical record)
//! and `hash` (SHA-256 over its own canonical record), so the chain is
//! self-authenticating: `chain()` recomputes every hash and reports
//! `valid` — any tampered/inserted/deleted line breaks it.
//!
//! Canonical record order is fixed by the [`AuditRecord`] struct field
//! order (same discipline as the drive envelope), so hashes are
//! deterministic across processes.
//!
//! **Growth policy (AC5) is enforced by the CALLER**: append is called
//! only for drive executions and typed refusals at dispatch — never for
//! GETs and never for authentication failures. The drive seam in
//! `http.rs` follows this; the caller-stub test proves it.
//!
//! The file is append-only (never rewritten in place). Rotation is a
//! future W4 concern; v1 keeps one file and documents the archive story
//! (stop daemon, move the file, restart — the new chain re-geneses).

use std::fmt;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::drive::{AuditEntry, AuditLog, AuditOutcome};

use super::hex;

/// File name of the audit log inside the config dir.
pub const AUDIT_FILE: &str = "audit.log";
/// Fixed first-chain-link predecessor (no prior entry exists).
pub const GENESIS_HASH: &str = "corral-audit-genesis-v1";

/// One hash-chained entry as served by `GET /audit`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChainEntry {
    pub seq: u64,
    pub ts: u64,
    pub key_id: String,
    pub request_id: String,
    pub capability: String,
    pub target: String,
    pub outcome: AuditOutcome,
    pub prev: String,
    pub hash: String,
}

impl Serialize for ChainEntry {
    fn serialize<S>(&self, s: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut st = s.serialize_struct("ChainEntry", 9)?;
        st.serialize_field("seq", &self.seq)?;
        st.serialize_field("ts", &self.ts)?;
        st.serialize_field("key_id", &self.key_id)?;
        st.serialize_field("request_id", &self.request_id)?;
        st.serialize_field("capability", &self.capability)?;
        st.serialize_field("target", &self.target)?;
        st.serialize_field("outcome", &OutcomeJson::from(&self.outcome))?;
        st.serialize_field("prev", &self.prev)?;
        st.serialize_field("hash", &self.hash)?;
        st.end()
    }
}

/// Canonical (hash-covered) record; field order is part of the format.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct AuditRecord {
    seq: u64,
    ts: u64,
    key_id: String,
    request_id: String,
    capability: String,
    target: String,
    outcome: OutcomeJson,
    prev: String,
}

/// Outcome as stored in the canonical record and served over HTTP.
/// External tagging: `Executed` -> `"executed"`, `Refused(d)` ->
/// `{"refused": d}`, `Failed(d)` -> `{"failed": d}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum OutcomeJson {
    Executed,
    Refused(String),
    Failed(String),
}

impl From<&AuditOutcome> for OutcomeJson {
    fn from(o: &AuditOutcome) -> Self {
        match o {
            AuditOutcome::Executed => Self::Executed,
            AuditOutcome::Refused(detail) => Self::Refused(detail.clone()),
            AuditOutcome::Failed(detail) => Self::Failed(detail.clone()),
        }
    }
}

impl From<OutcomeJson> for AuditOutcome {
    fn from(o: OutcomeJson) -> Self {
        match o {
            OutcomeJson::Executed => Self::Executed,
            OutcomeJson::Refused(d) => Self::Refused(d),
            OutcomeJson::Failed(d) => Self::Failed(d),
        }
    }
}

/// Full line as written: the canonical record plus its hash.
#[derive(Debug, Serialize, Deserialize)]
struct AuditLine {
    #[serde(flatten)]
    record: AuditRecord,
    hash: String,
}

struct AuditState {
    file: File,
    head: String,
    next_seq: u64,
}

pub struct HashChainAuditLog {
    path: PathBuf,
    state: Mutex<AuditState>,
}

impl HashChainAuditLog {
    pub fn open(config_dir: &Path) -> Result<Self, String> {
        std::fs::create_dir_all(config_dir).map_err(|e| format!("mkdir: {e}"))?;
        let path = config_dir.join(AUDIT_FILE);
        // Append-only: never truncate, never rewrite. Read handle for
        // chain() scans; writes go to the end.
        let file = OpenOptions::new()
            .read(true)
            .append(true)
            .create(true)
            .mode(0o600)
            .open(&path)
            .map_err(|e| format!("open audit log {}: {e}", path.display()))?;
        // Re-read the tail to resume the chain across restarts.
        let (head, next_seq) = scan_tail(&file);
        Ok(Self {
            path,
            state: Mutex::new(AuditState {
                file,
                head,
                next_seq,
            }),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Full chain: entries, head hash, and integrity verdict. `valid` is
    /// false if any stored hash does not match its recomputed canonical
    /// record, if any `prev` does not match the predecessor's hash, or if
    /// the file has a partial/truncated tail line.
    pub fn chain(&self) -> (Vec<ChainEntry>, String, bool) {
        let state = self.state.lock().expect("audit lock poisoned");
        let mut content = String::new();
        let read_ok = std::fs::File::open(&self.path)
            .and_then(|mut f| f.read_to_string(&mut content))
            .is_ok();
        if !read_ok {
            return (Vec::new(), state.head.clone(), false);
        }
        let mut entries = Vec::new();
        let mut prev = GENESIS_HASH.to_string();
        let mut valid = true;
        let mut last_hash = state.head.clone();
        for line in content.lines() {
            let line: AuditLine = match serde_json::from_str(line) {
                Ok(l) => l,
                Err(_) => {
                    valid = false;
                    last_hash = state.head.clone();
                    break;
                }
            };
            if line.record.prev != prev {
                valid = false;
            }
            let record_bytes = serde_json::to_vec(&line.record).expect("record serializes");
            let recomputed = hex(&sha256(&record_bytes));
            if recomputed != line.hash {
                valid = false;
            }
            let seq = line.record.seq;
            last_hash = line.hash.clone();
            entries.push(ChainEntry {
                seq,
                ts: line.record.ts,
                key_id: line.record.key_id,
                request_id: line.record.request_id,
                capability: line.record.capability,
                target: line.record.target,
                outcome: line.record.outcome.into(),
                prev: line.record.prev,
                hash: line.hash,
            });
            prev = last_hash.clone();
        }
        (entries, last_hash, valid)
    }

    /// The head hash (last appended link).
    pub fn head(&self) -> String {
        self.state.lock().expect("audit lock poisoned").head.clone()
    }

    pub fn len(&self) -> usize {
        let (entries, _, _) = self.chain();
        entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl AuditLog for HashChainAuditLog {
    fn append(&self, entry: &AuditEntry) -> Result<(), String> {
        let mut state = self.state.lock().expect("audit lock poisoned");
        let seq = state.next_seq;
        let record = AuditRecord {
            seq,
            ts: entry.ts,
            key_id: entry.key_id.clone(),
            request_id: entry.request_id.clone(),
            capability: entry.capability.clone(),
            target: entry.target.clone(),
            outcome: OutcomeJson::from(&entry.outcome),
            prev: state.head.clone(),
        };
        let record_bytes = serde_json::to_vec(&record).expect("record serializes");
        let hash = hex(&sha256(&record_bytes));
        let line = AuditLine {
            record,
            hash: hash.clone(),
        };
        let mut bytes = serde_json::to_vec(&line).expect("line serializes");
        bytes.push(b'\n');
        state
            .file
            .write_all(&bytes)
            .and_then(|_| state.file.flush())
            .and_then(|_| state.file.sync_all())
            .map_err(|e| format!("append audit entry: {e}"))?;
        state.head = hash;
        state.next_seq = seq + 1;
        Ok(())
    }
}

/// Recompute chain head + next sequence from the current file contents
/// (resume across daemon restarts). A trailing partial line is ignored for
/// the *tail* scan (it will be flagged invalid by `chain()`).
fn scan_tail(file: &File) -> (String, u64) {
    let mut head = GENESIS_HASH.to_string();
    let mut count = 0u64;
    let reader = BufReader::new(file);
    for line in reader.lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => break,
        };
        if line.trim().is_empty() {
            continue;
        }
        let line: AuditLine = match serde_json::from_str(&line) {
            Ok(l) => l,
            Err(_) => break, // partial tail (crash mid-write) — not counted
        };
        count += 1;
        head = line.hash;
    }
    (head, count)
}

fn sha256(bytes: &[u8]) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes).into()
}

impl fmt::Debug for HashChainAuditLog {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HashChainAuditLog")
            .field("path", &self.path)
            .field("entries", &self.len())
            .field("head", &self.head())
            .finish()
    }
}

/// Audit-log test module.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::now_secs;

    fn dir() -> tempfile::TempDir {
        tempfile::tempdir().expect("tempdir")
    }

    fn entry(seq_hint: &str, outcome: AuditOutcome) -> AuditEntry {
        AuditEntry {
            ts: now_secs(),
            key_id: format!("dev_{seq_hint}"),
            request_id: format!("req-{seq_hint}"),
            capability: "prompt".to_string(),
            target: "herdr:agent-a".to_string(),
            outcome,
        }
    }

    #[test]
    fn append_creates_chained_valid_log() {
        let d = dir();
        let log = HashChainAuditLog::open(d.path()).unwrap();
        log.append(&entry("1", AuditOutcome::Executed)).unwrap();
        log.append(&entry("2", AuditOutcome::Refused("agent busy".into())))
            .unwrap();
        log.append(&entry("3", AuditOutcome::Failed("adapter died".into())))
            .unwrap();

        let (entries, head, valid) = log.chain();
        assert!(valid);
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].seq, 0);
        assert_eq!(entries[0].prev, GENESIS_HASH);
        assert_eq!(entries[1].prev, entries[0].hash);
        assert_eq!(entries[2].prev, entries[1].hash);
        assert_eq!(head, entries[2].hash);
        assert!(entries[0].hash.len() == 64);

        // File on disk, 0600.
        let meta = std::fs::metadata(d.path().join(AUDIT_FILE)).unwrap();
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(meta.permissions().mode() & 0o777, 0o600);
    }

    #[test]
    fn tampered_line_breaks_integrity() {
        let d = dir();
        let log = HashChainAuditLog::open(d.path()).unwrap();
        log.append(&entry("1", AuditOutcome::Executed)).unwrap();
        log.append(&entry("2", AuditOutcome::Executed)).unwrap();

        let (_, _, valid_before) = log.chain();
        assert!(valid_before);

        let path = d.path().join(AUDIT_FILE);
        let content = std::fs::read_to_string(&path).unwrap();
        // Flip one character of entry 2's request_id inside the canonical record.
        let tampered = content.replacen("req-2", "req-9", 1);
        std::fs::write(&path, tampered).unwrap();
        let (entries, _, valid_after) = log.chain();
        assert!(!valid_after, "stored hash must not match tampered record");
        assert_eq!(entries.len(), 2);

        // Inserting a fake line also breaks the chain.
        let fake = content.replacen("req-1", "req-FAKE", 1);
        std::fs::write(&path, fake).unwrap();
        assert!(!log.chain().2);
    }

    #[test]
    fn resumes_chain_across_reopen() {
        let d = dir();
        let first = HashChainAuditLog::open(d.path()).unwrap();
        first.append(&entry("a", AuditOutcome::Executed)).unwrap();
        drop(first);

        let second = HashChainAuditLog::open(d.path()).unwrap();
        second.append(&entry("b", AuditOutcome::Executed)).unwrap();
        let (entries, _, valid) = second.chain();
        assert!(valid);
        assert_eq!(entries.len(), 2);
        assert_eq!(
            entries[1].prev, entries[0].hash,
            "new process chains onto the stored head"
        );
    }

    #[test]
    fn caller_stub_grows_only_on_writes() {
        // The AC5 policy — enforced by the CALLER, proven with a stub:
        // append happens for executions and dispatch refusals only; GETs
        // and authentication failures never touch the log.
        let d = dir();
        let log = HashChainAuditLog::open(d.path()).unwrap();

        enum CallerEvent {
            DriveExecuted,
            DriveRefusedAtDispatch,
            Read,
            AuthFailed,
        }
        let caller = |log: &HashChainAuditLog, ev: CallerEvent| {
            match ev {
                CallerEvent::DriveExecuted => {
                    log.append(&entry("ex", AuditOutcome::Executed)).unwrap()
                }
                CallerEvent::DriveRefusedAtDispatch => log
                    .append(&entry("re", AuditOutcome::Refused("target busy".into())))
                    .unwrap(),
                CallerEvent::Read | CallerEvent::AuthFailed => (), // NO append
            };
        };

        for _ in 0..3 {
            caller(&log, CallerEvent::Read);
        }
        assert_eq!(log.len(), 0, "GETs must not grow the log");
        for _ in 0..2 {
            caller(&log, CallerEvent::AuthFailed);
        }
        assert_eq!(log.len(), 0, "auth failures must not grow the log");

        caller(&log, CallerEvent::DriveExecuted);
        caller(&log, CallerEvent::DriveRefusedAtDispatch);
        caller(&log, CallerEvent::DriveExecuted);
        assert_eq!(log.len(), 3, "only the three writes are logged");

        let (entries, _, valid) = log.chain();
        assert!(valid);
        assert_eq!(
            entries.iter().map(|e| &e.outcome).collect::<Vec<_>>(),
            vec![
                &AuditOutcome::Executed,
                &AuditOutcome::Refused("target busy".into()),
                &AuditOutcome::Executed
            ]
        );
    }
}
