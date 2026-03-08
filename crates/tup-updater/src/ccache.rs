use std::path::PathBuf;

/// Detect and manage ccache integration.
///
/// When ccache is available, tup can wrap compiler commands to
/// benefit from compilation caching across builds.
pub struct CcacheConfig {
    /// Path to the ccache binary, if found.
    pub ccache_path: Option<PathBuf>,
    /// Whether ccache is enabled.
    pub enabled: bool,
}

impl CcacheConfig {
    /// Detect ccache on the system.
    pub fn detect() -> Self {
        let ccache_path = find_ccache();
        let enabled = ccache_path.is_some();
        CcacheConfig {
            ccache_path,
            enabled,
        }
    }

    /// Create a disabled config (for testing or when not wanted).
    pub fn disabled() -> Self {
        CcacheConfig {
            ccache_path: None,
            enabled: false,
        }
    }

    /// Check if a command looks like a compiler invocation that ccache can wrap.
    pub fn is_cacheable_command(command: &str) -> bool {
        let first_word = command.split_whitespace().next().unwrap_or("");
        let basename = std::path::Path::new(first_word)
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();

        matches!(
            basename.as_str(),
            "gcc" | "g++" | "cc" | "c++"
                | "clang" | "clang++"
                | "arm-none-eabi-gcc" | "arm-none-eabi-g++"
        ) || basename.ends_with("-gcc")
            || basename.ends_with("-g++")
            || basename.ends_with("-cc")
    }

    /// Wrap a command with ccache if applicable.
    ///
    /// Returns the original command if ccache is disabled or the command
    /// isn't a compiler invocation.
    pub fn wrap_command(&self, command: &str) -> String {
        if !self.enabled {
            return command.to_string();
        }

        if !Self::is_cacheable_command(command) {
            return command.to_string();
        }

        if let Some(ref path) = self.ccache_path {
            format!("{} {}", path.display(), command)
        } else {
            command.to_string()
        }
    }

    /// Check if a path is a ccache-related path (for filtering in dependency tracking).
    pub fn is_ccache_path(path: &str) -> bool {
        path.contains(".ccache") || path.contains("ccache")
    }
}

/// Find the ccache binary on the system PATH.
fn find_ccache() -> Option<PathBuf> {
    let output = std::process::Command::new("which")
        .arg("ccache")
        .output()
        .ok()?;

    if output.status.success() {
        let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !path.is_empty() {
            return Some(PathBuf::from(path));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_cacheable_gcc() {
        assert!(CcacheConfig::is_cacheable_command("gcc -c foo.c -o foo.o"));
        assert!(CcacheConfig::is_cacheable_command("g++ -c foo.cpp -o foo.o"));
        assert!(CcacheConfig::is_cacheable_command("cc -c foo.c"));
        assert!(CcacheConfig::is_cacheable_command("clang -c foo.c"));
        assert!(CcacheConfig::is_cacheable_command("clang++ -c foo.cpp"));
    }

    #[test]
    fn test_is_cacheable_cross() {
        assert!(CcacheConfig::is_cacheable_command("arm-none-eabi-gcc -c foo.c"));
        assert!(CcacheConfig::is_cacheable_command("/usr/bin/gcc -c foo.c"));
    }

    #[test]
    fn test_not_cacheable() {
        assert!(!CcacheConfig::is_cacheable_command("ld -o foo foo.o"));
        assert!(!CcacheConfig::is_cacheable_command("ar rcs lib.a foo.o"));
        assert!(!CcacheConfig::is_cacheable_command("echo hello"));
        assert!(!CcacheConfig::is_cacheable_command("cp foo bar"));
        assert!(!CcacheConfig::is_cacheable_command("python build.py"));
    }

    #[test]
    fn test_disabled_no_wrap() {
        let cc = CcacheConfig::disabled();
        assert_eq!(cc.wrap_command("gcc -c foo.c"), "gcc -c foo.c");
    }

    #[test]
    fn test_enabled_wraps_gcc() {
        let cc = CcacheConfig {
            ccache_path: Some(PathBuf::from("/usr/bin/ccache")),
            enabled: true,
        };
        assert_eq!(
            cc.wrap_command("gcc -c foo.c -o foo.o"),
            "/usr/bin/ccache gcc -c foo.c -o foo.o"
        );
    }

    #[test]
    fn test_enabled_skips_non_compiler() {
        let cc = CcacheConfig {
            ccache_path: Some(PathBuf::from("/usr/bin/ccache")),
            enabled: true,
        };
        assert_eq!(cc.wrap_command("echo hello"), "echo hello");
    }

    #[test]
    fn test_is_ccache_path() {
        assert!(CcacheConfig::is_ccache_path("/home/user/.ccache/tmp/foo"));
        assert!(CcacheConfig::is_ccache_path("/usr/lib/ccache/gcc"));
        assert!(!CcacheConfig::is_ccache_path("/usr/bin/gcc"));
    }

    #[test]
    fn test_detect() {
        // Just verify it doesn't panic — ccache may or may not be installed
        let _cc = CcacheConfig::detect();
    }
}
