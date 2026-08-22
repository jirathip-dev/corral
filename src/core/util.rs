//! Small shared helpers.

use std::fs;
use std::path::{Path, PathBuf};

use std::time::{SystemTime, UNIX_EPOCH};

/// Wall-clock now in epoch millis (display-only; ordering comes from seq/rev).
pub fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Canonicalize the longest existing prefix of `path`, then append any
/// components that do not exist yet.
///
/// A plain [`fs::canonicalize`] fails for a path that has just disappeared or
/// has not been created yet. On macOS that also leaves `/var` and
/// `/private/var` spellings incomparable. Resolving the existing prefix keeps
/// path identity stable for both live and disappearing worktrees without
/// normalizing `..` textually (which would be unsafe in the presence of
/// symlinks).
pub fn canonicalize_existing_prefix(path: &Path) -> PathBuf {
    let mut tail: Vec<std::ffi::OsString> = Vec::new();
    let mut cursor = path;
    loop {
        if let Ok(base) = fs::canonicalize(cursor) {
            let mut out = base;
            for part in tail.iter().rev() {
                out.push(part);
            }
            return out;
        }
        match (cursor.parent(), cursor.file_name()) {
            (Some(parent), Some(name)) => {
                tail.push(name.to_os_string());
                cursor = parent;
            }
            _ => return path.to_path_buf(),
        }
    }
}
