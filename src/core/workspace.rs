//! Canonical repo/worktree attribution shared by the read-side planes.
//!
//! Repo names come from explicit Corral roots (or the fleet registry's
//! `gh_repo` slug). Linked worktrees retain Herdr's canonical layout,
//! `<worktrees_root>/<repo>/<label>`. Branches are facts recorded by the git
//! plane, never inferred from a pane label, display name, or path suffix.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use crate::core::util::canonicalize_existing_prefix;

/// A repo root that Corral is explicitly allowed to attribute.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoRoot {
    pub path: PathBuf,
    pub repo: String,
}

/// The branch portion of a git fact. `branch_known` distinguishes a cached
/// detached HEAD (`branch == None`) from a path that has not been probed yet.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorkspaceFacts {
    pub repo: Option<String>,
    pub branch: Option<String>,
    pub branch_known: bool,
}

/// Shared attribution state. The Herdr adapter reads it while constructing a
/// fresh agent record; the integrator records git facts into it. Keeping this
/// state separate from the Store means an agent that arrives after a git
/// probe still gets the same canonical repo/branch facts immediately.
#[derive(Debug, Clone)]
pub struct WorkspaceAttribution {
    roots: Arc<RwLock<BTreeMap<PathBuf, String>>>,
    worktrees_root: PathBuf,
    branches: Arc<RwLock<HashMap<PathBuf, Option<String>>>>,
}

impl WorkspaceAttribution {
    /// Build an attribution map from explicit repo roots. Paths are
    /// canonicalized best-effort so a symlinked HOME or an APFS firmlink does
    /// not create a second identity for the same checkout. When canonical
    /// roots collide, the first root wins; callers must order roots by their
    /// source precedence.
    pub fn from_roots<I>(roots: I, worktrees_root: PathBuf) -> Self
    where
        I: IntoIterator<Item = RepoRoot>,
    {
        let attribution = Self {
            roots: Arc::new(RwLock::new(BTreeMap::new())),
            worktrees_root: canonicalize_existing_prefix(&worktrees_root),
            branches: Arc::new(RwLock::new(HashMap::new())),
        };
        for root in roots {
            attribution.add_root(root);
        }
        attribution
    }

    /// Convenience for the daemon's configured primary checkout. The repo
    /// name is the known root's final component, matching the existing
    /// Corral workspace contract.
    pub fn new(repo_root: PathBuf, worktrees_root: PathBuf) -> Self {
        let roots = repo_root.file_name().map(|name| RepoRoot {
            path: repo_root.clone(),
            repo: name.to_string_lossy().into_owned(),
        });
        Self::from_roots(roots, worktrees_root)
    }

    /// Add a known primary checkout. A conflicting duplicate canonical path
    /// is rejected, leaving the first source's identity in place. The daemon
    /// orders fleet-registry roots before the configured fallback so the
    /// registry's canonical `gh_repo` identity wins that collision.
    pub fn add_root(&self, root: RepoRoot) -> bool {
        if root.repo.is_empty() {
            return false;
        }
        let path = canonicalize_existing_prefix(&root.path);
        let mut roots = self.roots.write().unwrap();
        match roots.get(&path) {
            Some(existing) if existing != &root.repo => false,
            Some(_) => true,
            None => {
                roots.insert(path, root.repo);
                true
            }
        }
    }

    /// The canonical primary roots, in path order, for the git plane.
    pub fn repo_roots(&self) -> Vec<PathBuf> {
        self.roots.read().unwrap().keys().cloned().collect()
    }

    pub fn worktrees_root(&self) -> PathBuf {
        self.worktrees_root.clone()
    }

    /// Resolve the repo portion of an agent's worktree path. Only exact
    /// matches to explicit primary roots or paths under the known Herdr
    /// worktrees root are accepted. Prefixes are component-aware.
    pub fn repo_for(&self, path: &Path) -> Option<String> {
        let key = canonicalize_existing_prefix(path);
        if let Some(repo) = self.roots.read().unwrap().get(&key) {
            return Some(repo.clone());
        }
        let relative = key.strip_prefix(&self.worktrees_root).ok()?;
        let first = relative.components().next()?;
        let repo = first.as_os_str().to_string_lossy();
        (!repo.is_empty()).then(|| repo.into_owned())
    }

    /// Return the currently known facts for a path, or `None` for an
    /// unrecognized path. A known path without a git probe has a repo but an
    /// unknown branch; a detached HEAD has a known `None` branch.
    pub fn facts_for(&self, path: &Path) -> Option<WorkspaceFacts> {
        let key = canonicalize_existing_prefix(path);
        let repo = self.repo_for(&key)?;
        let branches = self.branches.read().unwrap();
        let (branch, branch_known) = match branches.get(&key) {
            Some(branch) => (branch.clone(), true),
            None => (None, false),
        };
        Some(WorkspaceFacts {
            repo: Some(repo),
            branch,
            branch_known,
        })
    }

    /// Record a git-plane branch fact. Unknown paths are intentionally
    /// ignored so synthetic or untrusted paths cannot manufacture repo
    /// attribution in the read model.
    pub fn record_branch(&self, path: &Path, branch: &str) {
        let key = canonicalize_existing_prefix(path);
        if self.repo_for(&key).is_none() {
            return;
        }
        let branch = (branch != "HEAD").then(|| branch.to_string());
        self.branches.write().unwrap().insert(key, branch);
    }

    /// Start a new git-plane generation. Repo roots and the linked-worktree
    /// layout remain valid, but branch values are only valid when the current
    /// generation has re-observed them. This prevents a replacement plane
    /// from handing a vanished worktree's old branch to a fresh Herdr record
    /// when the previous generation lost its removal event.
    pub fn reset_branch_facts(&self) {
        self.branches.write().unwrap().clear();
    }

    pub fn clear_branch(&self, path: &Path) {
        self.branches
            .write()
            .unwrap()
            .remove(&canonicalize_existing_prefix(path));
    }
}

/// Raw equality first, then canonical equality. This is the identity rule
/// used when a Herdr cwd and a git-plane path use different symlink spellings.
pub fn paths_match(a: &Path, b: &Path) -> bool {
    a == b || canonicalize_existing_prefix(a) == canonicalize_existing_prefix(b)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[cfg(unix)]
    #[test]
    fn primary_linked_alias_and_unknown_paths_are_deterministic() {
        let temp = tempfile::tempdir().expect("tempdir");
        let primary = temp.path().join("primary-repo");
        let worktrees = temp.path().join("worktrees");
        fs::create_dir_all(&primary).unwrap();
        fs::create_dir_all(&worktrees).unwrap();
        let linked = worktrees.join("linked-repo/feature");
        fs::create_dir_all(&linked).unwrap();
        let alias = temp.path().join("primary-alias");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&primary, &alias).unwrap();

        let attribution = WorkspaceAttribution::from_roots(
            [RepoRoot {
                path: primary.clone(),
                repo: "primary-repo".to_string(),
            }],
            worktrees.clone(),
        );
        attribution.record_branch(&primary, "main");
        attribution.record_branch(&linked, "feature/x");

        let primary_facts = attribution.facts_for(&primary).expect("primary facts");
        assert_eq!(primary_facts.repo.as_deref(), Some("primary-repo"));
        assert_eq!(primary_facts.branch.as_deref(), Some("main"));
        assert_eq!(attribution.facts_for(&alias), Some(primary_facts));

        let linked_facts = attribution.facts_for(&linked).expect("linked facts");
        assert_eq!(linked_facts.repo.as_deref(), Some("linked-repo"));
        assert_eq!(linked_facts.branch.as_deref(), Some("feature/x"));

        let unknown = temp.path().join("unknown");
        assert!(attribution.facts_for(&unknown).is_none());
        attribution.record_branch(&unknown, "do-not-infer");
        assert!(attribution.facts_for(&unknown).is_none());
    }

    #[test]
    fn generation_reset_clears_vanished_branch_and_reaccepts_present_fact() {
        let temp = tempfile::tempdir().expect("tempdir");
        let primary = temp.path().join("primary-repo");
        let worktrees = temp.path().join("worktrees");
        let present = worktrees.join("linked-repo/present");
        let vanished = worktrees.join("linked-repo/vanished");
        fs::create_dir_all(&primary).unwrap();
        fs::create_dir_all(&present).unwrap();
        fs::create_dir_all(&vanished).unwrap();

        let attribution = WorkspaceAttribution::from_roots(
            [RepoRoot {
                path: primary,
                repo: "primary-repo".to_string(),
            }],
            worktrees,
        );
        attribution.record_branch(&present, "old-present");
        attribution.record_branch(&vanished, "stale-vanished");
        fs::remove_dir_all(&vanished).unwrap();

        // A replacement GitPlane starts with no path registry. Clearing only
        // branch facts keeps repo/layout attribution while requiring the new
        // generation to re-observe every present worktree.
        attribution.reset_branch_facts();

        let vanished_facts = attribution.facts_for(&vanished).expect("known layout");
        assert_eq!(vanished_facts.repo.as_deref(), Some("linked-repo"));
        assert_eq!(vanished_facts.branch, None);
        assert!(!vanished_facts.branch_known);
        let present_facts = attribution.facts_for(&present).expect("present path");
        assert_eq!(present_facts.branch, None);
        assert!(!present_facts.branch_known);

        attribution.record_branch(&present, "current-present");
        let present_facts = attribution.facts_for(&present).expect("reconciled path");
        assert_eq!(present_facts.branch.as_deref(), Some("current-present"));
        assert!(present_facts.branch_known);
    }
}
