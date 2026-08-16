//! Branch-name → issue inference, DISPLAY-ONLY (DECISIONS.md D21).
//!
//! A worktree branch like `issue-431-embed-project-management` hints at
//! issue 431, but the hint is NOT authoritative: authoritative linkage is
//! the snapshot's per-agent `issues` (daemon `GhIssueRef`, joined from
//! GitHub's `closingIssuesReferences` — G23). This module parses the
//! branch name, validates the number against that fetched set, and renders
//! a visually distinct marker:
//!
//! - `~#431` — inferred AND present in the fetched issue set;
//! - `~#431?` — inferred but NOT in the fetched set (flagged, never
//!   asserted as real; a daemon without the G23 join validates against an
//!   empty set, so every inference is flagged).
//!
//! HARD RULE: inferred numbers are NEVER action-driving. The only public
//! surface is [`InferredIssue::marker`] (a display string); drive payload
//! builders in `drive.rs` take `agent_id` + waiting-on claims and have no
//! type-level access to an inferred number — pinned by the
//! `inferred_numbers_never_reach_drive_payloads` test.
//!
//! Inference is pure + deterministic (branch + fetched set in → marker
//! out; no time, randomness, or mutable state), so rendering is stable
//! across frames and never flickers.

use std::collections::BTreeSet;
use std::fmt;

/// A branch-name-inferred issue number, validated against the fetched
/// authoritative issue set (the agent's snapshot `issues`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InferredIssue {
    pub number: u64,
    /// True only when `number` is present in the fetched authoritative set.
    pub known: bool,
}

impl InferredIssue {
    /// The distinct display marker: `~#N` (validated) or `~#N?` (flagged).
    /// The `~` prefix keeps it visually distinct from the authoritative
    /// `#N` PR/issue refs.
    pub fn marker(&self) -> String {
        if self.known {
            format!("~#{}", self.number)
        } else {
            format!("~#{}?", self.number)
        }
    }
}

impl fmt::Display for InferredIssue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.marker())
    }
}

/// Pure branch-name inference + validation against the fetched issue set.
/// `None` when the branch name infers no issue. Deterministic: same
/// branch + same set → same result, frame after frame.
pub fn infer(branch: Option<&str>, known: &BTreeSet<u64>) -> Option<InferredIssue> {
    let number = issue_number_from_branch(branch?)?;
    Some(InferredIssue {
        number,
        known: known.contains(&number),
    })
}

/// Parse `issue-<N>…` / `#<N>…` style branch-name forms:
///
/// - `issue-<N>-…`, `issues-<N>…`, `issue/<N>…`, `issues/<N>…` anywhere
///   in the name (e.g. `issue-431-embed-project-management`,
///   `w2/issue-17-read-tail`, `issues/24`);
/// - `#<N>…` anywhere in the name (e.g. `#123`, `feat/#123-fix`).
///
/// Returns `None` for every other shape: no number, number zero, leading
/// non-digit after the marker (`issue-inference`), overflow, or bare
/// numbers with no `issue`/`#` marker (`123`, `fix-24-crash`).
pub fn issue_number_from_branch(branch: &str) -> Option<u64> {
    let name = branch.trim();
    if name.is_empty() {
        return None;
    }
    if let Some(pos) = name.find('#')
        && let Some(n) = leading_number(&name[pos + 1..])
    {
        return Some(n);
    }
    let segment = name;
    for marker in ["issue-", "issues-", "issue/", "issues/"] {
        if let Some(idx) = segment.find(marker)
            && let Some(n) = leading_number(&segment[idx + marker.len()..])
        {
            return Some(n);
        }
    }
    None
}

/// Leading decimal digits of `text` as a nonzero `u64`.
fn leading_number(text: &str) -> Option<u64> {
    let digits: String = text.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() {
        return None;
    }
    let n: u64 = digits.parse().ok()?;
    (n > 0).then_some(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_issue_and_hash_branch_forms() {
        for (branch, expected) in [
            ("issue-431-embed-project-management", 431),
            ("w2/issue-17-read-tail", 17),
            ("g24/issue-24-issue-inference", 24),
            ("issues/24", 24),
            ("issue/24-foo", 24),
            ("gh-issues-24", 24),
            ("#123", 123),
            ("feat/#123-fix-thing", 123),
            ("issue-007", 7),
        ] {
            assert_eq!(issue_number_from_branch(branch), Some(expected), "{branch}");
        }
    }

    #[test]
    fn rejects_non_issue_branch_forms() {
        for branch in [
            "",
            "main",
            "w21/read-tail-roundtrip",
            "feat/corral-p4",
            "g24/issue-inference",
            "issue-",
            "issue--24",
            "issue-0",
            "issue-000",
            "issues-",
            "#",
            "#x",
            "123",
            "fix-24-crash",
            "issue-99999999999999999999999",
        ] {
            assert_eq!(issue_number_from_branch(branch), None, "{branch}");
        }
    }

    #[test]
    fn inference_validates_against_the_fetched_issue_set() {
        let set: BTreeSet<u64> = [431].into_iter().collect();
        let inf =
            infer(Some("issue-431-embed-project-management"), &set).expect("branch infers #431");
        assert!(inf.known);
        assert_eq!(inf.marker(), "~#431");
        assert_eq!(inf.to_string(), "~#431");

        let inf =
            infer(Some("issue-999-embed-project-management"), &set).expect("branch infers #999");
        assert!(!inf.known);
        assert_eq!(
            inf.marker(),
            "~#999?",
            "absent-in-set is flagged, never asserted"
        );

        // Pre-G23 daemon: empty fetched set → everything stays flagged.
        assert_eq!(
            infer(Some("issue-431-x"), &BTreeSet::new())
                .expect("branch infers #431")
                .marker(),
            "~#431?"
        );
    }

    #[test]
    fn inference_is_deterministic() {
        // Pure: no time/randomness/state, so the marker is stable across
        // frames (no flicker). Pinned by calling twice with equal inputs.
        let set: BTreeSet<u64> = [7, 24].into_iter().collect();
        let branch = Some("issue-24-widget");
        let a = infer(branch, &set).expect("infers");
        let b = infer(branch, &set).expect("infers");
        assert_eq!(a, b);
        assert_eq!(a.marker(), "~#24");
    }

    #[test]
    fn inferred_numbers_never_reach_drive_payloads() {
        // D21 pin: branch-name inference is display-only. Drive intents
        // are built from agent_id + waiting-on claims; the inferred number
        // must never leak into the signed envelope payload.
        use crate::drive::DriveIntent;
        use crate::model::{Agent, AgentState, WaitingOn, WaitingOnKind, Workspace};

        let agent = Agent {
            agent_id: "herdr:a".into(),
            source: "herdr".into(),
            tool: "claude".into(),
            state: AgentState::Blocked,
            reason: None,
            seq: 1,
            ts: 0,
            capabilities: vec![],
            waiting_on: Some(WaitingOn {
                kind: WaitingOnKind::Menu,
                prompt: "choose".into(),
                prompt_hash: "sha256:x".into(),
                approval_id: "herdr:a:sha256:x".into(),
                choices: vec!["yes".into()],
            }),
            cost: None,
            parent_id: None,
            host: None,
            workspace: Workspace {
                branch: Some("issue-24-widget".into()),
                ..Default::default()
            },
            attachment: None,
            display_name: None,
            title: None,
            issues: vec![],
        };

        let inferred =
            infer(agent.workspace.branch.as_deref(), &BTreeSet::new()).expect("branch infers #24");
        assert_eq!(inferred.number, 24);
        assert!(!inferred.known);

        let approve = DriveIntent::approve(&agent, "yes".into(), Some(1));
        let prompt = DriveIntent::prompt(&agent.agent_id, "continue".into(), Some(1));
        let approve_text = serde_json::to_string(&approve.payload).unwrap();
        let prompt_text = serde_json::to_string(&prompt.payload).unwrap();
        assert!(
            !approve_text.contains("24"),
            "approve claim must not carry the inferred number: {approve_text}"
        );
        assert!(
            !prompt_text.contains("24"),
            "prompt payload must not carry the inferred number: {prompt_text}"
        );
        assert!(
            !approve_text.contains("issue"),
            "approve claim must not reference the branch hint: {approve_text}"
        );
        // The ONLY surface for the number is the display marker.
        assert_eq!(inferred.marker(), "~#24?");
    }
}
