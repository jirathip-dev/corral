//! Shared repo-category read model for the credential-free board surfaces
//! (#237: CONFIGLESS).
//!
//! Repo categories have exactly ONE source: the live canonical
//! `workspace.repo` values already present on agent records in the Herdr
//! snapshot. Corral does not own, read, or write the fleet registry, so no
//! registry-derived basenames can appear. This module never derives a
//! category from an agent id, pane label, or arbitrary path, and it never
//! turns a live-only category into a fleet-operation target — actionable
//! identities come exclusively from the fleet-ops CLI validated identity
//! path ([`crate::fleet::cli`]).

use std::collections::BTreeSet;

use crate::core::store::Store;

/// Collect nonempty, trimmed repo identities from the current agent records.
///
/// `Store::matching` is intentional: the store owns the live agent map, and
/// this read must not flush pending deltas or fabricate an SSE revision.
pub(crate) async fn live_workspace_repos(store: &Store) -> BTreeSet<String> {
    store
        .matching(|agent| {
            agent
                .workspace
                .repo
                .as_deref()
                .is_some_and(|repo| !repo.trim().is_empty())
        })
        .await
        .into_iter()
        .filter_map(|agent| {
            agent.workspace.repo.and_then(|repo| {
                let repo = repo.trim();
                (!repo.is_empty()).then(|| repo.to_string())
            })
        })
        .collect()
}

/// Sorted, deduplicated live workspace repo categories. Trimming is
/// deliberate: this read model is also fed by hand-built fixtures and live
/// records from older peers.
pub(crate) fn normalize_categories(live_repos: BTreeSet<String>) -> BTreeSet<String> {
    live_repos
        .into_iter()
        .filter_map(|repo| {
            let repo = repo.trim();
            (!repo.is_empty()).then(|| repo.to_string())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_trims_and_drops_empty() {
        let categories = normalize_categories(BTreeSet::from([
            "  corral  ".to_string(),
            "".to_string(),
            "  ".to_string(),
            "sendmeter".to_string(),
        ]));
        assert_eq!(
            categories,
            BTreeSet::from(["corral".to_string(), "sendmeter".to_string()])
        );
    }
}
