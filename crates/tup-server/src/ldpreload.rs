use std::path::PathBuf;

/// Location and availability of the LD_PRELOAD shared library.
pub struct LdPreloadLib {
    /// Path to libtup_ldpreload.so, if compiled.
    pub path: Option<PathBuf>,
}

impl LdPreloadLib {
    /// Detect the LD_PRELOAD library.
    ///
    /// Checks the build output directory first, then common install paths.
    pub fn detect() -> Self {
        // Check if compiled during build (set by build.rs)
        if let Ok(path) = std::env::var("TUP_LDPRELOAD_PATH") {
            let p = PathBuf::from(&path);
            if p.exists() {
                return LdPreloadLib { path: Some(p) };
            }
        }

        // Check common install locations
        for candidate in &[
            "/usr/lib/tup/libtup_ldpreload.so",
            "/usr/local/lib/tup/libtup_ldpreload.so",
        ] {
            let p = PathBuf::from(candidate);
            if p.exists() {
                return LdPreloadLib { path: Some(p) };
            }
        }

        // Check next to the tup binary
        if let Ok(exe) = std::env::current_exe() {
            if let Some(dir) = exe.parent() {
                let p = dir.join("libtup_ldpreload.so");
                if p.exists() {
                    return LdPreloadLib { path: Some(p) };
                }
            }
        }

        LdPreloadLib { path: None }
    }

    /// Check if LD_PRELOAD is available.
    pub fn is_available(&self) -> bool {
        self.path.is_some()
    }

    /// Check if we're on a platform that supports LD_PRELOAD.
    pub fn platform_supported() -> bool {
        cfg!(target_os = "linux")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_no_crash() {
        let lib = LdPreloadLib::detect();
        // May or may not find the library depending on build
        let _ = lib.is_available();
    }

    #[test]
    fn test_platform_supported() {
        let supported = LdPreloadLib::platform_supported();
        if cfg!(target_os = "linux") {
            assert!(supported);
        } else {
            assert!(!supported);
        }
    }
}
