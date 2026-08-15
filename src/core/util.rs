//! Small shared helpers.

use std::time::{SystemTime, UNIX_EPOCH};

/// Wall-clock now in epoch millis (display-only; ordering comes from seq/rev).
pub fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}
