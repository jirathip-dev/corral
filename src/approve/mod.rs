//! Claim-based approvals (P3 D8, W2) — the load-bearing seam between the
//! read model's `waiting_on` and the drive path's `approve` capability.
//!
//! Model: when an agent is blocked on a prompt (its record carries
//! `waiting_on`), the host exposes a **live approval claim**:
//!
//! ```text
//! { approval_id, prompt_hash, choices[] }
//! ```
//!
//! - `approval_id` is a stable identity for the waiting approval, derived
//!   from the agent and the exact prompt it is waiting on
//!   ([`approval_id_for`]) — so the drive path can re-derive it from the
//!   store with no extra state.
//! - `prompt_hash` is the source adapter's hash of the EXACT prompt text
//!   (untrimmed — P3 brief: never trim).
//! - `choices[]` passes through from `waiting_on`; the kind
//!   (`ApproveTool`/`AnswerQuestion`/`Menu`/`Crash`) stays distinct and is
//!   never collapsed.
//!
//! A client replies with `DrivePayload::Approve { approval_id, prompt_hash,
//! choice }`. [`check_approval_claim`] performs the CLAIM CHECK before any
//! dispatch and returns a typed refusal (never a 500):
//!
//! 1. `approval_id` must match the agent's CURRENT live approval — otherwise
//!    [`ApprovalError::NoWaitingApproval`] (nothing is pending) or
//!    [`ApprovalError::StaleApproval`] (the claim refers to an approval that
//!    is no longer live).
//! 2. `prompt_hash` must EXACTLY match the current prompt's hash — otherwise
//!    [`ApprovalError::HashMismatch`]: the wrong-question race kill. A client
//!    answering an earlier prompt while the agent has moved on is refused
//!    here.
//! 3. Per kind: a `Menu` (or `ApproveTool` with known choices) choice must be
//!    within `choices[]` — otherwise [`ApprovalError::ChoiceNotInMenu`];
//!    `AnswerQuestion` is free-form; `Crash` is never approvable.
//!
//! W1 wiring contract (the drive endpoint calls this): fetch the agent's
//! current record from the store, then
//!
//! ```rust,ignore
//! let approved = approve::check_approval_claim(
//!     &agent.agent_id,
//!     agent.waiting_on.as_ref(),
//!     &payload.approval_id,
//!     &payload.prompt_hash,
//!     &payload.choice,
//! )?;                                    // typed ApprovalError -> typed HTTP error
//! adapter.drive(&agent.agent_id, DriveCommand::Approve { choice: approved.choice })?;
//! ```

use serde::Serialize;

use crate::core::model::{WaitingOn, WaitingOnKind};

/// The live approval claim exposed for one waiting approval. `prompt_hash`
/// is the adapter's hash of the EXACT (untrimmed) prompt text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ApprovalClaim {
    pub approval_id: String,
    pub prompt_hash: String,
    pub choices: Vec<String>,
    pub kind: WaitingOnKind,
}

/// The validated approval: only a passed claim yields one. The adapter
/// dispatches `choice`; `kind` distinguishes how it must be sent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ApprovedApproval {
    pub choice: String,
    pub kind: WaitingOnKind,
}

/// Typed refusal for an approval that does not match the live claim. Every
/// variant maps onto a typed HTTP error in W1 — never a 500.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApprovalError {
    /// The agent has no live approval (not blocked, or no waiting_on).
    NoWaitingApproval,
    /// `approval_id` does not match the agent's current live approval
    /// (an approval the agent moved past, or a fabricated id).
    StaleApproval,
    /// `prompt_hash` does not exactly match the current prompt's hash —
    /// the client is answering the wrong question.
    HashMismatch,
    /// The kind is a menu (or an approve-tool with known choices) and
    /// `choice` is not one of `choices[]`.
    ChoiceNotInMenu,
    /// The waiting kind can never be approved (e.g. a crash).
    CannotApproveKind(WaitingOnKind),
}

impl std::fmt::Display for ApprovalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoWaitingApproval => {
                write!(f, "no waiting approval for this agent")
            }
            Self::StaleApproval => write!(
                f,
                "stale approval: approval_id does not match the agent's live approval"
            ),
            Self::HashMismatch => write!(
                f,
                "hash mismatch: prompt_hash does not match the current prompt (wrong question)"
            ),
            Self::ChoiceNotInMenu => {
                write!(f, "choice not in the prompt's menu choices")
            }
            Self::CannotApproveKind(kind) => write!(f, "cannot approve a {kind:?} waiting state"),
        }
    }
}

impl std::error::Error for ApprovalError {}

/// Stable approval identity: the agent plus the exact prompt being approved.
/// Derivable by the drive path from the store (`agent_id` +
/// `waiting_on.prompt_hash`) — no extra claim state is needed to validate.
pub fn approval_id_for(agent_id: &str, prompt_hash: &str) -> String {
    format!("{agent_id}:{prompt_hash}")
}

/// Derive the live claim from an agent's current `waiting_on` record. The
/// source adapter exposes the same fields on the read-model record
/// (`waiting_on.approval_id`) so clients never derive anything themselves.
pub fn claim_for(agent_id: &str, waiting_on: &WaitingOn) -> ApprovalClaim {
    ApprovalClaim {
        approval_id: approval_id_for(agent_id, &waiting_on.prompt_hash),
        prompt_hash: waiting_on.prompt_hash.clone(),
        choices: waiting_on.choices.clone(),
        kind: waiting_on.kind,
    }
}

/// THE claim check (W1 calls this from the drive handler — see module docs).
///
/// `waiting_on` is the agent's CURRENT `waiting_on` from the store. The
/// current claim is re-derived (never trusted from the stored record) and
/// the client's reply is validated against it in order: live approval ->
/// exact prompt hash -> kind-specific choice. Only a fully matching claim
/// yields an [`ApprovedApproval`] to dispatch.
pub fn check_approval_claim(
    agent_id: &str,
    waiting_on: Option<&WaitingOn>,
    approval_id: &str,
    prompt_hash: &str,
    choice: &str,
) -> Result<ApprovedApproval, ApprovalError> {
    let Some(waiting_on) = waiting_on else {
        return Err(ApprovalError::NoWaitingApproval);
    };
    let live = claim_for(agent_id, waiting_on);
    if approval_id != live.approval_id {
        return Err(ApprovalError::StaleApproval);
    }
    if prompt_hash != live.prompt_hash {
        return Err(ApprovalError::HashMismatch);
    }
    match waiting_on.kind {
        WaitingOnKind::Menu | WaitingOnKind::ApproveTool => {
            if !waiting_on.choices.is_empty() && !waiting_on.choices.iter().any(|c| c == choice) {
                return Err(ApprovalError::ChoiceNotInMenu);
            }
        }
        WaitingOnKind::AnswerQuestion => {}
        WaitingOnKind::Crash => {
            return Err(ApprovalError::CannotApproveKind(WaitingOnKind::Crash));
        }
    }
    Ok(ApprovedApproval {
        choice: choice.to_string(),
        kind: waiting_on.kind,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::model::WaitingOnKind as K;

    const AGENT: &str = "herdr:ses-live";
    const HASH: &str = "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    fn waiting_on(kind: K, prompt: &str, choices: Vec<String>) -> WaitingOn {
        WaitingOn {
            kind,
            prompt: prompt.to_string(),
            prompt_hash: HASH.to_string(),
            approval_id: approval_id_for(AGENT, HASH),
            choices,
        }
    }

    #[test]
    fn approval_id_is_stable_and_derivable() {
        let a = approval_id_for(AGENT, HASH);
        let b = approval_id_for(AGENT, HASH);
        assert_eq!(a, b, "same agent + prompt -> same approval identity");
        assert_ne!(
            approval_id_for(AGENT, "sha256:other"),
            a,
            "different prompt -> different approval"
        );
        assert_ne!(
            approval_id_for("herdr:other", HASH),
            a,
            "different agent -> different approval"
        );
        assert_eq!(
            claim_for(
                AGENT,
                &waiting_on(K::Menu, "proceed? [y/n]", vec!["y".into(), "n".into()])
            )
            .approval_id,
            a,
            "the drive path re-derives the live claim from the store record"
        );
    }

    #[test]
    fn claim_preserves_kind_and_choices() {
        for (kind, prompt, choices) in [
            (
                K::ApproveTool,
                "Approve this change?",
                vec!["y".to_string(), "n".to_string()],
            ),
            (K::AnswerQuestion, "What should I name the branch?", vec![]),
            (
                K::Menu,
                "Select an option [y/n]",
                vec!["y".to_string(), "n".to_string()],
            ),
            (K::Crash, "agent crashed", vec![]),
        ] {
            let w = waiting_on(kind, prompt, choices.clone());
            let claim = claim_for(AGENT, &w);
            assert_eq!(claim.kind, kind, "kinds are never collapsed in the claim");
            assert_eq!(claim.choices, choices, "choices pass through");
            assert_eq!(claim.prompt_hash, HASH);
        }
    }

    #[test]
    fn matching_claim_executes() {
        let w = waiting_on(K::Menu, "proceed? [y/n]", vec!["y".into(), "n".into()]);
        let ok = check_approval_claim(AGENT, Some(&w), &w.approval_id, HASH, "y").unwrap();
        assert_eq!(
            ok,
            ApprovedApproval {
                choice: "y".into(),
                kind: K::Menu
            }
        );
    }

    #[test]
    fn no_waiting_approval_is_typed_refusal() {
        let err = check_approval_claim(AGENT, None, "anything", "anything", "y").unwrap_err();
        assert_eq!(err, ApprovalError::NoWaitingApproval);
    }

    #[test]
    fn stale_approval_id_is_typed_refusal() {
        let w = waiting_on(K::Menu, "proceed? [y/n]", vec!["y".into(), "n".into()]);
        let err = check_approval_claim(
            AGENT,
            Some(&w),
            "herdr:old-agent:sha256:deadbeef",
            HASH,
            "y",
        )
        .unwrap_err();
        assert_eq!(err, ApprovalError::StaleApproval);
    }

    #[test]
    fn hash_mismatch_is_typed_refusal() {
        // The wrong-question race kill: the claim is live (right approval
        // identity) but the client is answering an older/different prompt.
        let w = waiting_on(K::Menu, "proceed? [y/n]", vec!["y".into(), "n".into()]);
        let err = check_approval_claim(
            AGENT,
            Some(&w),
            &w.approval_id,
            "sha256:deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef",
            "y",
        )
        .unwrap_err();
        assert_eq!(err, ApprovalError::HashMismatch);
    }

    #[test]
    fn menu_choice_must_be_in_choices() {
        let w = waiting_on(K::Menu, "select [y/n]", vec!["y".into(), "n".into()]);
        let err = check_approval_claim(AGENT, Some(&w), &w.approval_id, HASH, "maybe").unwrap_err();
        assert_eq!(err, ApprovalError::ChoiceNotInMenu);
        assert!(
            check_approval_claim(AGENT, Some(&w), &w.approval_id, HASH, "n").is_ok(),
            "a member of choices[] executes"
        );
    }

    #[test]
    fn approve_tool_choice_validated_when_choices_known() {
        let w = waiting_on(
            K::ApproveTool,
            "Approve this change?",
            vec!["y".into(), "n".into()],
        );
        let err = check_approval_claim(AGENT, Some(&w), &w.approval_id, HASH, "skip").unwrap_err();
        assert_eq!(err, ApprovalError::ChoiceNotInMenu);
        let ok = check_approval_claim(AGENT, Some(&w), &w.approval_id, HASH, "y").unwrap();
        assert_eq!(ok.kind, K::ApproveTool);
    }

    #[test]
    fn answer_question_is_free_form() {
        let w = waiting_on(K::AnswerQuestion, "What should I name the branch?", vec![]);
        let ok =
            check_approval_claim(AGENT, Some(&w), &w.approval_id, HASH, "feat/anything").unwrap();
        assert_eq!(
            ok,
            ApprovedApproval {
                choice: "feat/anything".into(),
                kind: K::AnswerQuestion
            }
        );
    }

    #[test]
    fn crash_is_never_approvable() {
        let w = waiting_on(K::Crash, "segfault", vec![]);
        let err =
            check_approval_claim(AGENT, Some(&w), &w.approval_id, HASH, "continue").unwrap_err();
        assert_eq!(err, ApprovalError::CannotApproveKind(K::Crash));
    }

    #[test]
    fn empty_choices_menu_is_lenient() {
        // Degenerate: a Menu with no extractable choices cannot validate
        // membership; the claim still executes (the adapter sends the text).
        let w = waiting_on(K::Menu, "odd menu", vec![]);
        assert!(check_approval_claim(AGENT, Some(&w), &w.approval_id, HASH, "anything").is_ok());
    }
}
