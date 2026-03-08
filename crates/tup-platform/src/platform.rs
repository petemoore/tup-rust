/// Get the platform name string.
///
/// Corresponds to `tup_platform` in C's platform.c.
pub fn platform_name() -> &'static str {
    if cfg!(target_os = "linux") {
        "linux"
    } else if cfg!(target_os = "macos") {
        "macosx"
    } else if cfg!(target_os = "windows") {
        "win32"
    } else if cfg!(target_os = "freebsd") {
        "freebsd"
    } else if cfg!(target_os = "netbsd") {
        "netbsd"
    } else {
        "unknown"
    }
}

/// Get the architecture name string.
///
/// Corresponds to `tup_arch` in C's platform.c.
pub fn arch_name() -> &'static str {
    if cfg!(target_arch = "x86_64") {
        "x86_64"
    } else if cfg!(target_arch = "aarch64") {
        "arm64"
    } else if cfg!(target_arch = "x86") {
        "i386"
    } else if cfg!(target_arch = "arm") {
        "arm"
    } else if cfg!(target_arch = "riscv64") {
        "riscv64"
    } else {
        "unknown"
    }
}

/// Get the path separator for the current platform.
pub fn path_sep() -> char {
    if cfg!(target_os = "windows") {
        '\\'
    } else {
        '/'
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_platform_name() {
        let name = platform_name();
        assert!(!name.is_empty());
        // Should be one of the known platforms
        assert!(
            ["linux", "macosx", "win32", "freebsd", "netbsd", "unknown"].contains(&name),
            "unexpected platform: {name}"
        );
    }

    #[test]
    fn test_arch_name() {
        let name = arch_name();
        assert!(!name.is_empty());
    }

    #[test]
    fn test_path_sep() {
        let sep = path_sep();
        if cfg!(target_os = "windows") {
            assert_eq!(sep, '\\');
        } else {
            assert_eq!(sep, '/');
        }
    }
}
