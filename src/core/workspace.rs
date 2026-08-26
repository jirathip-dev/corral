//! Canonical repo/worktree attribution shared by the read-side planes.
//!
//! Repo names come from explicit Corral roots (or the fleet registry's
//! `gh_repo` slug). Linked worktrees retain Herdr's addressable layout,
//! `<worktrees_root>/<worktree_dir>/<label>`; the registry maps
//! `worktree_dir` to the canonical `gh_repo` basename before any path-derived
//! fallback. Branches are facts recorded by the git plane, never inferred
//! from a pane label, display name, or path suffix.

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

/// A registry mapping from a Herdr worktree root component to the canonical
/// repo basename. `worktree_dir` is an on-disk location, not repo identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeAlias {
    pub worktree_dir: String,
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
    worktree_aliases: Arc<RwLock<BTreeMap<String, String>>>,
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
        Self::from_roots_with_aliases(roots, std::iter::empty(), worktrees_root)
    }

    /// Build an attribution map from explicit repo roots plus registry
    /// worktree aliases. Alias resolution happens before a linked worktree's
    /// path component falls back to its directory name, so a stale Herdr
    /// folder never splits one repo into two groups.
    pub fn from_roots_with_aliases<I, A>(roots: I, aliases: A, worktrees_root: PathBuf) -> Self
    where
        I: IntoIterator<Item = RepoRoot>,
        A: IntoIterator<Item = WorktreeAlias>,
    {
        let attribution = Self {
            roots: Arc::new(RwLock::new(BTreeMap::new())),
            worktree_aliases: Arc::new(RwLock::new(BTreeMap::new())),
            worktrees_root: canonicalize_existing_prefix(&worktrees_root),
            branches: Arc::new(RwLock::new(HashMap::new())),
        };
        for root in roots {
            attribution.add_root(root);
        }
        for alias in aliases {
            attribution.add_worktree_alias(alias);
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

    /// Replace the live primary-root and registry-alias snapshot. The daemon
    /// uses this at the same slow cadence as git-plane source discovery so a
    /// fleet added to `fleets.json` is attributed immediately and a removed
    /// fleet cannot leave a stale primary-root branch fact behind.
    pub fn replace_roots_with_aliases<I, A>(&self, roots: I, aliases: A)
    where
        I: IntoIterator<Item = RepoRoot>,
        A: IntoIterator<Item = WorktreeAlias>,
    {
        let mut next_roots = BTreeMap::new();
        for root in roots {
            if root.repo.is_empty() {
                continue;
            }
            let path = canonicalize_existing_prefix(&root.path);
            next_roots.entry(path).or_insert(root.repo);
        }
        let mut next_aliases = BTreeMap::new();
        for alias in aliases {
            if alias.worktree_dir.is_empty() || alias.repo.is_empty() {
                continue;
            }
            next_aliases.entry(alias.worktree_dir).or_insert(alias.repo);
        }
        let root_paths: std::collections::HashSet<PathBuf> = next_roots.keys().cloned().collect();
        *self.roots.write().unwrap() = next_roots;
        *self.worktree_aliases.write().unwrap() = next_aliases;
        self.branches
            .write()
            .unwrap()
            .retain(|path, _| root_paths.contains(path) || path.starts_with(&self.worktrees_root));
    }

    fn add_worktree_alias(&self, alias: WorktreeAlias) -> bool {
        if alias.worktree_dir.is_empty() || alias.repo.is_empty() {
            return false;
        }
        let mut aliases = self.worktree_aliases.write().unwrap();
        match aliases.get(&alias.worktree_dir) {
            Some(_) => false,
            None => {
                aliases.insert(alias.worktree_dir, alias.repo);
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
        let worktree_dir = first.as_os_str().to_string_lossy();
        if let Some(repo) = self
            .worktree_aliases
            .read()
            .unwrap()
            .get(worktree_dir.as_ref())
        {
            return Some(repo.clone());
        }
        (!worktree_dir.is_empty()).then(|| worktree_dir.into_owned())
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
    fn registry_alias_attributes_stale_worktree_dir_to_canonical_repo() {
        // #182: the on-disk worktree component is a location, not identity.
        // A stale `worktree_dir` must resolve through the registry's
        // `gh_repo` basename, never create a second repo group.
        let temp = tempfile::tempdir().expect("tempdir");
        let primary = temp.path().join("stale-checkout");
        let worktrees = temp.path().join("worktrees");
        let linked = worktrees.join("stale-checkout/g182-fix");
        let unregistered = worktrees.join("another-repo/g182-fix");
        fs::create_dir_all(&primary).unwrap();
        fs::create_dir_all(&linked).unwrap();
        fs::create_dir_all(&unregistered).unwrap();

        let attribution = WorkspaceAttribution::from_roots_with_aliases(
            [RepoRoot {
                path: primary.clone(),
                repo: "canonical-repo".to_string(),
            }],
            [WorktreeAlias {
                worktree_dir: "stale-checkout".to_string(),
                repo: "canonical-repo".to_string(),
            }],
            worktrees,
        );

        assert_eq!(
            attribution
                .facts_for(&primary)
                .expect("primary facts")
                .repo
                .as_deref(),
            Some("canonical-repo")
        );
        assert_eq!(
            attribution
                .facts_for(&linked)
                .expect("linked facts")
                .repo
                .as_deref(),
            Some("canonical-repo"),
            "stale directory maps to the registry's canonical repo"
        );
        assert_eq!(
            attribution.repo_for(&linked),
            Some("canonical-repo".to_string()),
            "no phantom stale-name group"
        );
        assert_eq!(
            attribution.repo_for(&unregistered).as_deref(),
            Some("another-repo"),
            "unregistered worktree dirs keep the path-derived fallback"
        );
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
