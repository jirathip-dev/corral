//! Shared repo-category read model for the credential-free board surfaces.
//!
//! Repo categories have two deliberately separate sources:
//!
//! - the live canonical `workspace.repo` values already present on agent
//!   records; and
//! - the validated fleet registry's `gh_repo` basenames.
//!
//! This module only reads those sources. It never derives a category from an
//! agent id, pane label, or arbitrary path, and it never turns a live-only
//! category into a fleet-operation target.

use std::collections::BTreeSet;

use crate::core::store::Store;
use crate::fleet::config::Registry;

/// Collect nonempty repo identities from the current agent records.
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
        .filter_map(|agent| agent.workspace.repo)
        .collect()
}

/// Add the canonical repository basenames from a validated registry.
///
/// The registry loader validates `gh_repo` as a single `owner/repo` slug. The
/// defensive shape check here keeps this read model safe for manually-built
/// test registries too, without widening the registry schema or trusting an
/// invalid value.
pub(crate) fn add_registry_repos(categories: &mut BTreeSet<String>, registry: &Registry) {
    for fleet in &registry.fleets {
        let Some((owner, repo)) = fleet.gh_repo.split_once('/') else {
            continue;
        };
        if !owner.is_empty() && !repo.is_empty() && !repo.contains('/') {
            categories.insert(repo.to_string());
        }
    }
}

/// Return the sorted union used by `/fleet-registry` and by the category
/// placeholders in `/issues`.
pub(crate) fn union_with_registry(
    mut live_repos: BTreeSet<String>,
    registry: Option<&Registry>,
) -> BTreeSet<String> {
    if let Some(registry) = registry {
        add_registry_repos(&mut live_repos, registry);
    }
    live_repos
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fleet::config::{Fleet, Models};

    fn fleet(gh_repo: &str) -> Fleet {
        Fleet {
            name: "fleet-name".to_string(),
            gh_repo: gh_repo.to_string(),
            local: "/tmp/fleet".to_string(),
            worktree_dir: "fleet".to_string(),
            orch: "orch".to_string(),
            workers: Vec::new(),
            paused: false,
            models: Models {
                orch: "orch-model".to_string(),
                impl_: "impl-model".to_string(),
                review: "review-model".to_string(),
                impl_alt: None,
                impl_alt2: None,
            },
        }
    }

    #[test]
    fn registry_union_uses_canonical_basename_and_keeps_live_only_repo() {
        let live = BTreeSet::from(["live-only".to_string(), "shared".to_string()]);
        let registry = Registry::new(vec![fleet("owner/registry-only"), fleet("owner/shared")]);

        assert_eq!(
            union_with_registry(live, Some(&registry)),
            BTreeSet::from([
                "live-only".to_string(),
                "registry-only".to_string(),
                "shared".to_string(),
            ])
        );
    }

    #[test]
    fn invalid_manual_registry_slug_is_not_a_category() {
        let mut categories = BTreeSet::new();
        add_registry_repos(&mut categories, &Registry::new(vec![fleet("owner/a/b")]));
        assert!(categories.is_empty());
    }
}
