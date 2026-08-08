//! Hive-Scope-Tabelle.
//!
//! Hives are **not** actors — no task, no mailbox, no `cell.db`, no
//! `ActorHandle` entry. This struct is pure data. Anyone about to bolt a sender or
//! task reference onto it has walked into anti-pattern no. 1 (see CLAUDE.md).

use meclaw_core::Path;
use std::collections::HashMap;

/// A registered hive-scope marker. Pure data — no actor handle, no mailbox.
#[derive(Debug, Clone)]
pub struct HiveScope {
    /// Absolute path of the hive-scope directory.
    pub path: Path,
}

/// Lookup table for registered hive-scope markers, keyed by their absolute path.
#[derive(Debug, Default)]
pub struct HiveScopeTable {
    by_path: HashMap<Path, HiveScope>,
}

impl HiveScopeTable {
    /// Create an empty table.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a hive scope under its path.
    pub fn register(&mut self, scope: HiveScope) {
        self.by_path.insert(scope.path.clone(), scope);
    }

    /// Look up a hive scope by exact path match.
    pub fn get(&self, path: &Path) -> Option<&HiveScope> {
        self.by_path.get(path)
    }

    /// Iterate over the absolute paths of all registered hive scopes.
    ///
    /// Used by the mutation callsite to collect hive short-names (last path
    /// segment) so that `add_edges` endpoints may reference hives symmetrically
    /// to cells.
    pub fn paths(&self) -> impl Iterator<Item = &Path> {
        self.by_path.keys()
    }

    /// Number of registered scopes.
    pub fn len(&self) -> usize {
        self.by_path.len()
    }

    /// True if no scopes are registered.
    pub fn is_empty(&self) -> bool {
        self.by_path.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_and_lookup() {
        let mut table = HiveScopeTable::new();
        table.register(HiveScope {
            path: Path::new("/"),
        });
        assert!(table.get(&Path::new("/")).is_some());
        assert!(table.get(&Path::new("/missing")).is_none());
        assert_eq!(table.len(), 1);
    }

    #[test]
    fn paths_yields_every_registered_scope() {
        let mut table = HiveScopeTable::new();
        table.register(HiveScope {
            path: Path::new("/"),
        });
        table.register(HiveScope {
            path: Path::new("/pool"),
        });
        let mut got: Vec<String> = table.paths().map(|p| p.as_str().to_string()).collect();
        got.sort();
        assert_eq!(got, vec!["/".to_string(), "/pool".to_string()]);
    }
}
