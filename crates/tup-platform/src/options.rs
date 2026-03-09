use std::collections::BTreeMap;
use std::path::Path;

/// Tup option definitions with defaults.
///
/// Corresponds to the `options[]` array in C's option.c.
const OPTION_DEFAULTS: &[(&str, &str)] = &[
    ("updater.num_jobs", ""), // Empty = auto-detect CPU count
    ("updater.keep_going", "0"),
    ("updater.full_deps", "0"),
    ("updater.warnings", "1"),
    ("display.color", "auto"),
    ("display.width", ""),    // Empty = auto-detect terminal width
    ("display.progress", ""), // Empty = auto-detect if stdout is a tty
    ("display.job_numbers", "1"),
    ("display.job_time", "1"),
    ("display.quiet", "0"),
    ("monitor.autoupdate", "0"),
    ("monitor.autoparse", "0"),
    ("monitor.foreground", "0"),
    ("db.sync", "1"),
    ("graph.dirs", "0"),
    ("graph.ghosts", "0"),
    ("graph.environment", "0"),
    ("graph.combine", "0"),
];

/// The tup options file path within the .tup directory.
pub const TUP_OPTIONS_FILE: &str = ".tup/options";

/// Manages tup runtime options loaded from INI files and command line.
///
/// Options are loaded from (highest to lowest priority):
/// 1. Command line overrides
/// 2. .tup/options (project-local)
/// 3. ~/.config/tup/options (user)
/// 4. /etc/tup/options (system, Unix only)
pub struct TupOptions {
    values: BTreeMap<String, String>,
}

impl TupOptions {
    /// Create options with default values.
    pub fn new() -> Self {
        let mut values = BTreeMap::new();
        for &(name, default) in OPTION_DEFAULTS {
            if !default.is_empty() {
                values.insert(name.to_string(), default.to_string());
            }
        }

        // Auto-detect dynamic defaults
        if !values.contains_key("updater.num_jobs") {
            let cpus = std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(1);
            values.insert("updater.num_jobs".to_string(), cpus.to_string());
        }

        TupOptions { values }
    }

    /// Load options from an INI-style file.
    ///
    /// Format: `section.key = value` (one per line, # comments).
    pub fn load_file(&mut self, path: &Path) -> Result<(), String> {
        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(format!("failed to read {}: {e}", path.display())),
        };

        self.parse_ini(&content);
        Ok(())
    }

    /// Parse INI-style content and merge into options.
    pub fn parse_ini(&mut self, content: &str) {
        let mut current_section = String::new();

        for line in content.lines() {
            let line = line.trim();

            // Skip empty lines and comments
            if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
                continue;
            }

            // Section header [section]
            if line.starts_with('[') && line.ends_with(']') {
                current_section = line[1..line.len() - 1].to_string();
                continue;
            }

            // Key = value
            if let Some((key, value)) = line.split_once('=') {
                let key = key.trim();
                let value = value.trim();

                let full_key = if current_section.is_empty() {
                    key.to_string()
                } else {
                    format!("{}.{}", current_section, key)
                };

                // Only accept known options
                if is_valid_option(&full_key) {
                    self.values.insert(full_key, value.to_string());
                }
            }
        }
    }

    /// Set an option value.
    pub fn set(&mut self, name: &str, value: &str) {
        self.values.insert(name.to_string(), value.to_string());
    }

    /// Get an option value as a string.
    pub fn get_string(&self, name: &str) -> Option<&str> {
        self.values.get(name).map(|s| s.as_str())
    }

    /// Get an option value as an integer.
    pub fn get_int(&self, name: &str) -> Option<i32> {
        self.values.get(name)?.parse().ok()
    }

    /// Get an option value as a boolean flag (0 = false, non-zero = true).
    pub fn get_flag(&self, name: &str) -> bool {
        self.get_int(name).unwrap_or(0) != 0
    }

    /// Display all options and their values.
    pub fn show(&self) -> Vec<(String, String)> {
        let mut result = Vec::new();
        for &(name, _) in OPTION_DEFAULTS {
            let value = self
                .values
                .get(name)
                .map(|s| s.as_str())
                .unwrap_or("(unset)");
            result.push((name.to_string(), value.to_string()));
        }
        result
    }
}

impl Default for TupOptions {
    fn default() -> Self {
        Self::new()
    }
}

/// Check if an option name is valid.
fn is_valid_option(name: &str) -> bool {
    OPTION_DEFAULTS.iter().any(|&(n, _)| n == name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_defaults() {
        let opts = TupOptions::new();
        assert_eq!(opts.get_string("updater.keep_going"), Some("0"));
        assert_eq!(opts.get_string("db.sync"), Some("1"));
        assert_eq!(opts.get_string("display.color"), Some("auto"));

        // num_jobs should be auto-detected
        let num_jobs = opts.get_int("updater.num_jobs").unwrap();
        assert!(num_jobs >= 1);
    }

    #[test]
    fn test_get_flag() {
        let opts = TupOptions::new();
        assert!(!opts.get_flag("updater.keep_going"));
        assert!(opts.get_flag("db.sync"));
    }

    #[test]
    fn test_set_override() {
        let mut opts = TupOptions::new();
        opts.set("updater.keep_going", "1");
        assert!(opts.get_flag("updater.keep_going"));
    }

    #[test]
    fn test_parse_ini() {
        let mut opts = TupOptions::new();
        opts.parse_ini(
            "[updater]\n\
             num_jobs = 4\n\
             keep_going = 1\n\
             \n\
             # Comment\n\
             [display]\n\
             color = never\n",
        );

        assert_eq!(opts.get_int("updater.num_jobs"), Some(4));
        assert!(opts.get_flag("updater.keep_going"));
        assert_eq!(opts.get_string("display.color"), Some("never"));
    }

    #[test]
    fn test_parse_ini_ignores_unknown() {
        let mut opts = TupOptions::new();
        opts.parse_ini("unknown.option = value\n");
        assert!(opts.get_string("unknown.option").is_none());
    }

    #[test]
    fn test_parse_ini_comments_and_blanks() {
        let mut opts = TupOptions::new();
        opts.parse_ini(
            "# Full line comment\n\
             ; Semicolon comment\n\
             \n\
             [db]\n\
             sync = 0\n",
        );
        assert!(!opts.get_flag("db.sync"));
    }

    #[test]
    fn test_show() {
        let opts = TupOptions::new();
        let items = opts.show();
        assert!(!items.is_empty());

        // All defined options should appear
        let names: Vec<&str> = items.iter().map(|(n, _)| n.as_str()).collect();
        assert!(names.contains(&"updater.num_jobs"));
        assert!(names.contains(&"db.sync"));
    }

    #[test]
    fn test_load_nonexistent_file() {
        let mut opts = TupOptions::new();
        let result = opts.load_file(Path::new("/nonexistent/path/options"));
        assert!(result.is_ok()); // Missing file is OK
    }

    #[test]
    fn test_is_valid_option() {
        assert!(is_valid_option("updater.num_jobs"));
        assert!(is_valid_option("db.sync"));
        assert!(!is_valid_option("foo.bar"));
    }
}
