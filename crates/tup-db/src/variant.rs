use std::collections::BTreeMap;
use std::path::PathBuf;

use tup_types::TupId;

use crate::vardb::VarDb;

/// Sentinel value indicating a variant's source directory was removed.
#[allow(dead_code)]
pub const VARIANT_SRCDIR_REMOVED: i64 = -2;

/// A build variant (e.g., debug, release).
///
/// Variants allow building the same source tree with different configurations.
/// Each variant has its own output directory and tup.config.
#[derive(Debug)]
pub struct Variant {
    /// TupId of the variant's directory node.
    pub dir_id: TupId,
    /// TupId of the variant's tup.config node.
    pub config_id: Option<TupId>,
    /// Associated tup_entry for the variant directory.
    pub tent_id: TupId,
    /// Variable database for this variant's configuration.
    pub vdb: VarDb,
    /// Whether this variant is enabled.
    pub enabled: bool,
    /// Whether this is the root (in-tree) variant.
    pub root_variant: bool,
    /// Path to the variant directory.
    pub variant_dir: PathBuf,
    /// Path to the variant's vardict file.
    pub vardict_file: PathBuf,
}

impl Variant {
    /// Create a new root variant (in-tree build, no separate config).
    pub fn new_root(dir_id: TupId) -> Self {
        Variant {
            dir_id,
            config_id: None,
            tent_id: dir_id,
            vdb: VarDb::new(),
            enabled: true,
            root_variant: true,
            variant_dir: PathBuf::from("."),
            vardict_file: PathBuf::from(".tup/vardict"),
        }
    }

    /// Create a new non-root variant.
    pub fn new(dir_id: TupId, tent_id: TupId, variant_dir: &str) -> Self {
        let vardict_file = format!("{variant_dir}/.tup/vardict");
        Variant {
            dir_id,
            config_id: None,
            tent_id,
            vdb: VarDb::new(),
            enabled: true,
            root_variant: false,
            variant_dir: PathBuf::from(variant_dir),
            vardict_file: PathBuf::from(vardict_file),
        }
    }

    /// Check if a config variable is set.
    pub fn get_config(&self, name: &str) -> Option<&str> {
        self.vdb.get(name).map(|e| e.value.as_str())
    }

    /// Set a config variable.
    pub fn set_config(&mut self, name: &str, value: &str) {
        self.vdb.set(name, value, None);
    }
}

/// Registry of all build variants.
#[derive(Debug)]
pub struct VariantRegistry {
    variants: BTreeMap<TupId, Variant>,
    root_id: Option<TupId>,
}

impl VariantRegistry {
    /// Create a new empty registry.
    pub fn new() -> Self {
        VariantRegistry {
            variants: BTreeMap::new(),
            root_id: None,
        }
    }

    /// Add the root variant.
    pub fn add_root(&mut self, dir_id: TupId) -> &Variant {
        let variant = Variant::new_root(dir_id);
        self.root_id = Some(dir_id);
        self.variants.entry(dir_id).or_insert(variant)
    }

    /// Add a non-root variant.
    pub fn add(&mut self, dir_id: TupId, tent_id: TupId, variant_dir: &str) -> &Variant {
        let variant = Variant::new(dir_id, tent_id, variant_dir);
        self.variants.entry(dir_id).or_insert(variant)
    }

    /// Remove a variant.
    pub fn remove(&mut self, dir_id: TupId) -> Option<Variant> {
        let removed = self.variants.remove(&dir_id);
        if self.root_id == Some(dir_id) {
            self.root_id = None;
        }
        removed
    }

    /// Look up a variant by directory TupId.
    pub fn search(&self, dir_id: TupId) -> Option<&Variant> {
        self.variants.get(&dir_id)
    }

    /// Look up a variant mutably.
    pub fn search_mut(&mut self, dir_id: TupId) -> Option<&mut Variant> {
        self.variants.get_mut(&dir_id)
    }

    /// Get the root variant.
    pub fn root(&self) -> Option<&Variant> {
        self.root_id.and_then(|id| self.variants.get(&id))
    }

    /// Check if the list is empty.
    pub fn is_empty(&self) -> bool {
        self.variants.is_empty()
    }

    /// Get the number of variants.
    pub fn len(&self) -> usize {
        self.variants.len()
    }

    /// Iterate over all variants.
    pub fn iter(&self) -> impl Iterator<Item = (&TupId, &Variant)> {
        self.variants.iter()
    }

    /// Get all non-root variants.
    pub fn non_root_variants(&self) -> Vec<&Variant> {
        self.variants.values()
            .filter(|v| !v.root_variant)
            .collect()
    }
}

impl Default for VariantRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Parse a tup.config file into a variable map.
///
/// Format: `CONFIG_FOO=bar` (one per line, # comments).
pub fn parse_tup_config(content: &str) -> BTreeMap<String, String> {
    let mut vars = BTreeMap::new();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            let key = key.trim();
            let value = value.trim();
            // Config vars are typically CONFIG_FOO
            vars.insert(key.to_string(), value.to_string());
        }
    }
    vars
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_variant_root() {
        let v = Variant::new_root(TupId::new(1));
        assert!(v.root_variant);
        assert!(v.enabled);
    }

    #[test]
    fn test_variant_non_root() {
        let v = Variant::new(TupId::new(10), TupId::new(10), "build-debug");
        assert!(!v.root_variant);
        assert_eq!(v.variant_dir, PathBuf::from("build-debug"));
    }

    #[test]
    fn test_variant_config() {
        let mut v = Variant::new_root(TupId::new(1));
        v.set_config("CC", "gcc");
        assert_eq!(v.get_config("CC"), Some("gcc"));
        assert_eq!(v.get_config("CXX"), None);
    }

    #[test]
    fn test_registry_add_search() {
        let mut reg = VariantRegistry::new();
        reg.add_root(TupId::new(1));
        reg.add(TupId::new(10), TupId::new(10), "debug");
        reg.add(TupId::new(20), TupId::new(20), "release");

        assert_eq!(reg.len(), 3);
        assert!(reg.root().unwrap().root_variant);
        assert!(reg.search(TupId::new(10)).is_some());
        assert!(reg.search(TupId::new(99)).is_none());
    }

    #[test]
    fn test_registry_non_root() {
        let mut reg = VariantRegistry::new();
        reg.add_root(TupId::new(1));
        reg.add(TupId::new(10), TupId::new(10), "debug");

        let non_root = reg.non_root_variants();
        assert_eq!(non_root.len(), 1);
        assert!(!non_root[0].root_variant);
    }

    #[test]
    fn test_registry_remove() {
        let mut reg = VariantRegistry::new();
        reg.add_root(TupId::new(1));
        reg.add(TupId::new(10), TupId::new(10), "debug");

        reg.remove(TupId::new(10));
        assert!(reg.search(TupId::new(10)).is_none());
        assert_eq!(reg.len(), 1);
    }

    #[test]
    fn test_parse_tup_config() {
        let content = "# Build configuration\nCONFIG_CC=gcc\nCONFIG_DEBUG=y\n\n# Empty\n";
        let vars = parse_tup_config(content);
        assert_eq!(vars.get("CONFIG_CC").unwrap(), "gcc");
        assert_eq!(vars.get("CONFIG_DEBUG").unwrap(), "y");
        assert_eq!(vars.len(), 2);
    }

    #[test]
    fn test_parse_tup_config_empty() {
        let vars = parse_tup_config("");
        assert!(vars.is_empty());
    }

    #[test]
    fn test_parse_tup_config_comments_only() {
        let vars = parse_tup_config("# comment\n# another\n");
        assert!(vars.is_empty());
    }
}
