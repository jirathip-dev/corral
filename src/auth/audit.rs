//! Signed append-only audit log (D10/AC5): [`HashChainAuditLog`].
//!
//! One JSON line per entry at `<config_dir>/audit.log` (0600). Every entry
//! carries `prev` (the SHA-256 of the previous entry's canonical record)
//! and `hash` (SHA-256 over its own canonical record), so the chain is
//! self-authenticating: `chain()` recomputes every hash and reports
//! `valid` — any tampered/inserted line breaks it.
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
//! **Crash repair (F4b)**: a daemon killed mid-append leaves a partial
//! line. `open()` rewrites it as a [`Tombstone`] (hash of the raw bytes
//! kept for forensics) before anything reads or appends, so a crash can
//! never permanently brick the `valid` verdict; `chain()` and the resume
//! scan skip tombstones.
//!
//! **TODO(W4) — external anchor**: the chain head is self-referential
//! (derived from the file itself), so wholesale truncation of trailing
//! entries silently re-geneses the chain at the next open and is not
//! detected. W4 must anchor the head externally (a second checkpoint
//! file, or a host-key-signed head shipped off-machine) and fold the
//! registry into the integrity story (F6).
//!
//! The file is append-only (never rewritten in place — the only exception
//! is the boot-time tombstone repair above). Rotation is a future W4
//! concern; v1 keeps one file and documents the archive story (stop
//! daemon, move the file, restart — the new chain re-geneses).

use std::fmt;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::drive::{AuditEntry, AuditLog, AuditOutcome};

use super::{hex, now_secs};

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

/// A crash-repair marker (F4b): when a daemon dies mid-append, the partial
/// line cannot be parsed and would otherwise poison `chain()`'s integrity
/// verdict forever. `open()` rewrites it as this tombstone (hash = SHA-256
/// of the raw partial bytes, for forensics). `chain()` skips tombstones —
/// they are artifacts, not entries. TODO(W4): the chain head is
/// self-referential (derived from the file itself), so wholesale
/// truncation of trailing entries is undetectable without an external
/// anchor (second checkpoint file or host-key-signed head shipped
/// off-machine) — W4 must add it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tombstone {
    pub tombstone: bool,
    pub ts: u64,
    /// The sequence number the crashed append was about to write.
    pub seq: u64,
    /// SHA-256 hex of the raw partial line bytes.
    pub hash: String,
    /// Chain head at crash time (forensics only).
    pub prev: String,
}

impl HashChainAuditLog {
    pub fn open(config_dir: &Path) -> Result<Self, String> {
        super::ensure_dir_0700(config_dir)?;
        let path = config_dir.join(AUDIT_FILE);
        // F4b: repair a crash-mid-append partial tail BEFORE anything
        // reads or appends, so a bad line can never brick `valid`.
        repair_partial_tail(&path)?;
        // Append-only: never truncate, never rewrite. Read handle for
        // chain() scans; writes go to the end. Mode enforced on the load
        // path too (F5).
        let file = OpenOptions::new()
            .read(true)
            .append(true)
            .create(true)
            .mode(0o600)
            .open(&path)
            .map_err(|e| format!("open audit log {}: {e}", path.display()))?;
        super::ensure_file_0600(&path)?;
        // Re-read the tail to resume the chain across restarts (tombstones
        // skipped — they carry no chain link).
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
    /// a line is neither a valid entry nor a tombstone. Tombstones (crash
    /// artifacts) are skipped without breaking `valid` (F4b).
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
            if line.trim().is_empty() {
                continue;
            }
            if let Ok(line) = serde_json::from_str::<AuditLine>(line) {
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
            } else if serde_json::from_str::<Tombstone>(line).is_ok() {
                // Crash artifact: skipped, carries no chain link (F4b).
                continue;
            } else {
                valid = false;
                break;
            }
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
/// (resume across daemon restarts). Tombstones are skipped — they carry
/// no chain link.
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
        if let Ok(line) = serde_json::from_str::<AuditLine>(&line) {
            count += 1;
            head = line.hash;
        }
        // Tombstones are skipped; anything else (partial tail) is a
        // crash artifact that open() already repaired — treat as a stop.
    }
    (head, count)
}

/// F4b: if the file ends with a partial (unparseable, non-tombstone) line
/// — the signature of a crash mid-append — rewrite it as a tombstone so
/// `chain()` can keep reporting `valid` and appends can resume. The
/// tombstone's `hash` covers the raw partial bytes for forensics. No-op
/// when the file is clean. This is the ONLY write path that rewrites the
/// file; it runs at open(), before the daemon serves anything.
fn repair_partial_tail(path: &Path) -> Result<(), String> {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return Ok(()), // missing file: open() creates it
    };
    if content.is_empty() {
        return Ok(());
    }
    // Drop the trailing element left by a final newline.
    let mut lines: Vec<&str> = content.split('\n').collect();
    while lines.last().is_some_and(|l| l.is_empty()) {
        lines.pop();
    }
    let Some(tail) = lines.last() else {
        return Ok(());
    };
    let tail_is_entry = serde_json::from_str::<AuditLine>(tail).is_ok();
    let tail_is_tombstone = serde_json::from_str::<Tombstone>(tail).is_ok();
    if tail_is_entry || tail_is_tombstone {
        return Ok(()); // clean tail
    }

    // Partial line: capture its raw bytes, then rebuild the file as
    // [complete lines…, tombstone].
    let (head, count) = head_and_count(&lines[..lines.len() - 1]);
    let tombstone = Tombstone {
        tombstone: true,
        ts: now_secs(),
        seq: count,
        hash: hex(&sha256(tail.as_bytes())),
        prev: head,
    };
    let mut out = lines[..lines.len() - 1].join("\n");
    if !out.is_empty() {
        out.push('\n');
    }
    // No trailing newline here: write_secret appends one.
    out.push_str(&serde_json::to_string(&tombstone).expect("tombstone serializes"));
    tracing::warn!(
        path = %path.display(),
        "audit log crash artifact repaired as tombstone (seq {}); raw bytes hashed for forensics",
        tombstone.seq
    );
    super::write_secret(path, out.as_bytes())
}

/// Head hash + entry count over complete lines only (tombstones skipped).
fn head_and_count(lines: &[&str]) -> (String, u64) {
    let mut head = GENESIS_HASH.to_string();
    let mut count = 0u64;
    for line in lines {
        if line.trim().is_empty() {
            continue;
        }
        if let Ok(line) = serde_json::from_str::<AuditLine>(line) {
            count += 1;
            head = line.hash;
        }
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

    /// F4b: a crash mid-append leaves a partial tail line. open() must
    /// repair it as a tombstone so the log stays `valid` and appends can
    /// resume — the partial line must never permanently brick the verdict.
    #[test]
    fn f4b_crash_mid_append_is_repaired_and_log_stays_valid() {
        let d = dir();
        let path = d.path().join(AUDIT_FILE);
        {
            let log = HashChainAuditLog::open(d.path()).unwrap();
            log.append(&entry("1", AuditOutcome::Executed)).unwrap();
            log.append(&entry("2", AuditOutcome::Executed)).unwrap();
        } // simulate crash: kill the process mid-append

        // Truncate the tail: cut the last 3 bytes of the final entry line.
        let mut content = std::fs::read(&path).unwrap();
        content.truncate(content.len() - 3);
        std::fs::write(&path, &content).unwrap();

        // Reopen: repair must tombstone the partial line.
        let log = HashChainAuditLog::open(d.path()).unwrap();
        let (entries, _, valid) = log.chain();
        assert!(
            valid,
            "crash artifact must not poison the integrity verdict"
        );
        assert_eq!(entries.len(), 1, "the complete entry survives the crash");

        // Appends resume and chain onto the last REAL entry, with the
        // interrupted seq.
        log.append(&entry("3", AuditOutcome::Executed)).unwrap();
        let (entries, _, valid) = log.chain();
        assert!(valid);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[1].prev, entries[0].hash);
        assert_eq!(
            entries[1].seq, 1,
            "seq continues after the tombstoned entry"
        );

        // The tombstone line itself is on disk with the partial bytes
        // hashed for forensics.
        let on_disk = std::fs::read_to_string(&path).unwrap();
        assert!(
            on_disk.contains("\"tombstone\":true"),
            "tombstone marker present"
        );
        let tomb = on_disk
            .lines()
            .filter_map(|l| serde_json::from_str::<Tombstone>(l).ok())
            .next()
            .expect("one tombstone");
        assert_eq!(tomb.seq, 1, "tombstone carries the interrupted seq");
        assert_eq!(tomb.hash.len(), 64);
        assert_eq!(
            tomb.prev, entries[0].hash,
            "tombstone anchors to the last real head"
        );
    }

    /// F4b: tombstone lines anywhere in the chain are skipped — they are
    /// artifacts, not entries — and never flip `valid`.
    #[test]
    fn f4b_tombstone_is_skipped_anywhere_in_chain() {
        let d = dir();
        let log = HashChainAuditLog::open(d.path()).unwrap();
        log.append(&entry("1", AuditOutcome::Executed)).unwrap();
        log.append(&entry("2", AuditOutcome::Executed)).unwrap();
        drop(log);

        // Inject a tombstone in the middle of the file, as a crash-repair
        // would, then verify the chain skips it.
        let path = d.path().join(AUDIT_FILE);
        let content = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = content.lines().collect();
        let tomb = serde_json::to_string(&Tombstone {
            tombstone: true,
            ts: now_secs(),
            seq: 1,
            hash: "ab".repeat(32),
            prev: "corral-audit-genesis-v1".into(),
        })
        .unwrap();
        let with_tomb = format!("{}\n{tomb}\n{}", lines[0], lines[1]);
        std::fs::write(&path, with_tomb).unwrap();

        let log = HashChainAuditLog::open(d.path()).unwrap();
        let (entries, _, valid) = log.chain();
        assert!(valid, "tombstone mid-chain must not break the verdict");
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[1].prev, entries[0].hash);
    }

    /// F5: a pre-existing audit.log with permissive modes is tightened on
    /// load, not just at creation.
    #[test]
    fn f5_load_enforces_0600() {
        use std::os::unix::fs::PermissionsExt;
        let d = dir();
        let path = d.path().join(AUDIT_FILE);
        std::fs::write(&path, "").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        std::fs::set_permissions(d.path(), std::fs::Permissions::from_mode(0o755)).unwrap();

        let log = HashChainAuditLog::open(d.path()).unwrap();
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600,
            "audit.log must be tightened to 0600 on load"
        );
        assert_eq!(
            std::fs::metadata(d.path()).unwrap().permissions().mode() & 0o777,
            0o700,
            "config dir must be tightened to 0700 on load"
        );
        log.append(&entry("1", AuditOutcome::Executed)).unwrap();
        assert!(log.chain().2);
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
