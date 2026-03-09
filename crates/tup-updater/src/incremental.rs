use std::collections::BTreeMap;
use std::path::Path;

/// Tracks build state for incremental rebuilds.
///
/// Stores a hash of each rule's inputs + command so we can skip
/// rules that haven't changed since the last build.
#[derive(Debug)]
pub struct BuildState {
    /// Map from rule key (command + outputs) to input hash.
    entries: BTreeMap<String, BuildEntry>,
}

#[derive(Debug, Clone)]
struct BuildEntry {
    /// Hash of all inputs + command.
    hash: u64,
    /// Whether this entry was used in the current build.
    touched: bool,
}

impl BuildState {
    /// Create a new empty build state.
    pub fn new() -> Self {
        BuildState {
            entries: BTreeMap::new(),
        }
    }

    /// Load build state from a file.
    pub fn load(path: &Path) -> Self {
        let mut state = BuildState::new();
        if let Ok(content) = std::fs::read_to_string(path) {
            for line in content.lines() {
                if let Some((key, hash_str)) = line.split_once('\t') {
                    if let Ok(hash) = hash_str.parse::<u64>() {
                        state.entries.insert(
                            key.to_string(),
                            BuildEntry {
                                hash,
                                touched: false,
                            },
                        );
                    }
                }
            }
        }
        state
    }

    /// Save build state to a file.
    pub fn save(&self, path: &Path) -> Result<(), String> {
        let mut content = String::new();
        for (key, entry) in &self.entries {
            if entry.touched {
                content.push_str(key);
                content.push('\t');
                content.push_str(&entry.hash.to_string());
                content.push('\n');
            }
        }
        std::fs::write(path, &content).map_err(|e| format!("failed to save build state: {e}"))?;
        Ok(())
    }

    /// Check if a rule needs to be rebuilt.
    ///
    /// Returns true if the rule should be executed (inputs changed or new).
    pub fn needs_rebuild(&mut self, rule_key: &str, input_hash: u64) -> bool {
        match self.entries.get(rule_key) {
            Some(entry) if entry.hash == input_hash => {
                // Mark as touched (still valid)
                if let Some(e) = self.entries.get_mut(rule_key) {
                    e.touched = true;
                }
                false
            }
            _ => {
                // New or changed — will be updated after successful build
                true
            }
        }
    }

    /// Mark a rule as successfully built with the given hash.
    pub fn mark_built(&mut self, rule_key: &str, input_hash: u64) {
        self.entries.insert(
            rule_key.to_string(),
            BuildEntry {
                hash: input_hash,
                touched: true,
            },
        );
    }

    /// Get the number of tracked entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Check if empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

impl Default for BuildState {
    fn default() -> Self {
        Self::new()
    }
}

/// Compute a hash for a rule based on its command and input file mtimes.
pub fn compute_rule_hash(command: &str, input_files: &[String], work_dir: &Path) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let mut hasher = DefaultHasher::new();
    command.hash(&mut hasher);

    for input in input_files {
        input.hash(&mut hasher);
        // Include mtime if the file exists
        let path = work_dir.join(input);
        if let Ok(meta) = std::fs::metadata(&path) {
            if let Ok(mtime) = meta.modified() {
                if let Ok(dur) = mtime.duration_since(std::time::UNIX_EPOCH) {
                    dur.as_nanos().hash(&mut hasher);
                }
            }
        }
    }

    hasher.finish()
}

/// Generate a unique key for a rule (command + outputs).
pub fn rule_key(command: &str, outputs: &[String]) -> String {
    let mut key = command.to_string();
    key.push_str(" -> ");
    key.push_str(&outputs.join(", "));
    key
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_state_new_rule() {
        let mut state = BuildState::new();
        assert!(state.needs_rebuild("cmd -> out", 12345));
    }

    #[test]
    fn test_build_state_unchanged() {
        let mut state = BuildState::new();
        state.mark_built("cmd -> out", 12345);
        assert!(!state.needs_rebuild("cmd -> out", 12345));
    }

    #[test]
    fn test_build_state_changed() {
        let mut state = BuildState::new();
        state.mark_built("cmd -> out", 12345);
        assert!(state.needs_rebuild("cmd -> out", 99999));
    }

    #[test]
    fn test_build_state_save_load() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("build_state");

        let mut state = BuildState::new();
        state.mark_built("gcc -c a.c -> a.o", 111);
        state.mark_built("gcc -c b.c -> b.o", 222);
        state.save(&path).unwrap();

        let mut loaded = BuildState::load(&path);
        assert!(!loaded.needs_rebuild("gcc -c a.c -> a.o", 111));
        assert!(!loaded.needs_rebuild("gcc -c b.c -> b.o", 222));
        assert!(loaded.needs_rebuild("gcc -c c.c -> c.o", 333));
    }

    #[test]
    fn test_build_state_load_missing() {
        let state = BuildState::load(Path::new("/nonexistent/path"));
        assert!(state.is_empty());
    }

    #[test]
    fn test_compute_rule_hash_deterministic() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("a.c"), "int main() {}").unwrap();

        let h1 = compute_rule_hash("gcc -c a.c", &["a.c".to_string()], tmp.path());
        let h2 = compute_rule_hash("gcc -c a.c", &["a.c".to_string()], tmp.path());
        assert_eq!(h1, h2);
    }

    #[test]
    fn test_compute_rule_hash_different_command() {
        let tmp = tempfile::tempdir().unwrap();
        let h1 = compute_rule_hash("gcc -c a.c", &[], tmp.path());
        let h2 = compute_rule_hash("gcc -O2 -c a.c", &[], tmp.path());
        assert_ne!(h1, h2);
    }

    #[test]
    fn test_rule_key() {
        let key = rule_key("gcc -c a.c", &["a.o".to_string()]);
        assert_eq!(key, "gcc -c a.c -> a.o");
    }
}
