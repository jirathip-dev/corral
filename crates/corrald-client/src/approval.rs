//! Approval-claim helpers (D8, load-bearing).
//!
//! - `approval_id = "<agent_id>:<prompt_hash>"`
//! - `prompt_hash` = `sha256:` + hex of SHA-256 over the EXACT untrimmed,
//!   redacted `waiting_on.prompt` string served in the snapshot. Clients
//!   must hash the snapshot string byte-for-byte — never raw pane text,
//!   never trimmed.
//!
//! The daemon re-derives the claim and refuses mismatches with typed 409s
//! (`no_waiting_approval` / `stale_approval` / `hash_mismatch`), so this
//! module is pure client-side bookkeeping — the refusal logic lives on the
//! host.

use crate::model::Agent;

/// `"sha256:" + hex(sha256(prompt))` over the EXACT bytes of the prompt
/// string as served by the snapshot. Do not trim, do not normalize.
pub fn prompt_hash_of(prompt: &str) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(prompt.as_bytes());
    let mut out = String::with_capacity(7 + 64);
    out.push_str("sha256:");
    for byte in &digest {
        use std::fmt::Write;
        write!(&mut out, "{byte:02x}").expect("hex write to String cannot fail");
    }
    out
}

/// The claim identity echoed in an approve reply:
/// `"<agent_id>:<prompt_hash>"`.
pub fn approval_id_for(agent_id: &str, prompt_hash: &str) -> String {
    format!("{agent_id}:{prompt_hash}")
}

/// The live approval claim of an agent record, if it is waiting on one.
/// Derives `approval_id` the same way the daemon does (`agent_id` +
/// `waiting_on.prompt_hash`) rather than trusting the stored copy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalClaim {
    pub approval_id: String,
    pub prompt_hash: String,
    pub choices: Vec<String>,
}

/// Extract the live claim from a snapshot agent (None when the agent is not
/// blocked on a prompt).
pub fn claim_from(agent: &Agent) -> Option<ApprovalClaim> {
    let waiting_on = agent.waiting_on.as_ref()?;
    Some(ApprovalClaim {
        approval_id: approval_id_for(&agent.agent_id, &waiting_on.prompt_hash),
        prompt_hash: waiting_on.prompt_hash.clone(),
        choices: waiting_on.choices.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_hash_is_sha256_hex_prefixed() {
        let hash = prompt_hash_of("Approve this change? [y/n]");
        assert!(hash.starts_with("sha256:"));
        assert_eq!(hash.len(), "sha256:".len() + 64);
    }

    #[test]
    fn prompt_hash_covers_exact_untrimmed_bytes() {
        let trimmed = prompt_hash_of("  approve?  ");
        let untrimmed = prompt_hash_of("  approve?  ".trim());
        assert_ne!(trimmed, untrimmed, "trimming must change the hash");
        // The claim must hash the SNAPSHOT string byte-for-byte; a client
        // that trims the prompt signs a different claim.
        assert_ne!(prompt_hash_of("Approve?"), prompt_hash_of("Approve? "));
    }

    #[test]
    fn approval_id_is_agent_colon_hash() {
        let agent_id = "herdr:pane:wQ:p1";
        let hash = prompt_hash_of("continue?");
        assert_eq!(
            approval_id_for(agent_id, &hash),
            format!("{agent_id}:{hash}")
        );
    }
}
