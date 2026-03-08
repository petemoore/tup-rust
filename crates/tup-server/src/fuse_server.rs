use std::path::{Path, PathBuf};

/// Status of FUSE availability on this system.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FuseStatus {
    /// FUSE is available and can be used.
    Available,
    /// FUSE is not installed on this system.
    NotInstalled,
    /// FUSE requires elevated privileges (SUID).
    NeedsPrivileges,
    /// FUSE is not supported on this platform.
    Unsupported,
}

/// Configuration for the FUSE server.
#[derive(Debug)]
pub struct FuseConfig {
    /// Mount point for the FUSE filesystem.
    pub mount_point: PathBuf,
    /// Whether to run FUSE in foreground (for debugging).
    pub foreground: bool,
    /// Whether to use single-threaded mode.
    pub single_threaded: bool,
}

impl FuseConfig {
    /// Create a default config for a tup project.
    pub fn new(tup_dir: &Path) -> Self {
        FuseConfig {
            mount_point: tup_dir.join(".tup").join("mnt"),
            foreground: false,
            single_threaded: true,
        }
    }
}

/// Check if FUSE is available on the current system.
pub fn check_fuse_available() -> FuseStatus {
    if cfg!(target_os = "linux") {
        check_fuse_linux()
    } else if cfg!(target_os = "macos") {
        check_fuse_macos()
    } else {
        FuseStatus::Unsupported
    }
}

#[cfg(target_os = "linux")]
fn check_fuse_linux() -> FuseStatus {
    // Check for /dev/fuse
    if std::path::Path::new("/dev/fuse").exists() {
        // Check if fusermount3 is available
        if std::process::Command::new("fusermount3")
            .arg("--version")
            .output()
            .is_ok()
        {
            return FuseStatus::Available;
        }
        // Try fusermount (FUSE 2)
        if std::process::Command::new("fusermount")
            .arg("--version")
            .output()
            .is_ok()
        {
            return FuseStatus::Available;
        }
        FuseStatus::NeedsPrivileges
    } else {
        FuseStatus::NotInstalled
    }
}

#[cfg(not(target_os = "linux"))]
fn check_fuse_linux() -> FuseStatus {
    FuseStatus::Unsupported
}

#[cfg(target_os = "macos")]
fn check_fuse_macos() -> FuseStatus {
    // Check for macFUSE
    if std::path::Path::new("/Library/Frameworks/macFUSE.framework").exists() {
        return FuseStatus::Available;
    }
    // Check for osxfuse (older name)
    if std::path::Path::new("/Library/Frameworks/OSXFUSE.framework").exists() {
        return FuseStatus::Available;
    }
    FuseStatus::NotInstalled
}

#[cfg(not(target_os = "macos"))]
fn check_fuse_macos() -> FuseStatus {
    FuseStatus::Unsupported
}

/// Trait for FUSE filesystem operations.
///
/// This abstracts the platform-specific FUSE implementation.
/// Each method corresponds to a FUSE callback.
pub trait TupFuseOps: Send + Sync {
    /// Look up a directory entry by name.
    fn lookup(&self, parent: u64, name: &str) -> Result<u64, i32>;

    /// Get file attributes.
    fn getattr(&self, ino: u64) -> Result<FileAttr, i32>;

    /// Read directory entries.
    fn readdir(&self, ino: u64) -> Result<Vec<DirEntry>, i32>;

    /// Open a file.
    fn open(&self, ino: u64, flags: i32) -> Result<u64, i32>;

    /// Read from a file.
    fn read(&self, ino: u64, offset: i64, size: u32) -> Result<Vec<u8>, i32>;

    /// Write to a file.
    fn write(&self, ino: u64, offset: i64, data: &[u8]) -> Result<u32, i32>;

    /// Release (close) a file.
    fn release(&self, ino: u64, fh: u64) -> Result<(), i32>;

    /// Create a file.
    fn create(&self, parent: u64, name: &str, mode: u32) -> Result<(u64, u64), i32>;

    /// Remove a file.
    fn unlink(&self, parent: u64, name: &str) -> Result<(), i32>;

    /// Rename a file.
    fn rename(&self, parent: u64, name: &str, newparent: u64, newname: &str) -> Result<(), i32>;

    /// Create a directory.
    fn mkdir(&self, parent: u64, name: &str, mode: u32) -> Result<u64, i32>;

    /// Remove a directory.
    fn rmdir(&self, parent: u64, name: &str) -> Result<(), i32>;
}

/// File attributes returned by getattr.
#[derive(Debug, Clone)]
pub struct FileAttr {
    pub ino: u64,
    pub size: u64,
    pub mode: u32,
    pub nlink: u32,
    pub uid: u32,
    pub gid: u32,
    pub mtime: i64,
}

/// Directory entry returned by readdir.
#[derive(Debug, Clone)]
pub struct DirEntry {
    pub ino: u64,
    pub name: String,
    pub file_type: u8, // DT_REG, DT_DIR, etc.
}

/// A passthrough FUSE implementation that delegates to the real filesystem
/// while recording file accesses.
///
/// This is the core of tup's dependency tracking via FUSE.
/// It intercepts all file operations, records reads/writes, and
/// passes them through to the underlying filesystem.
pub struct PassthroughFuse {
    /// Root directory being served.
    root: PathBuf,
    /// Job ID for this FUSE session.
    job_id: i32,
}

impl PassthroughFuse {
    /// Create a new passthrough FUSE filesystem.
    pub fn new(root: &Path, job_id: i32) -> Self {
        PassthroughFuse {
            root: root.to_path_buf(),
            job_id,
        }
    }

    /// Get the real path for a FUSE path.
    pub fn real_path(&self, fuse_path: &str) -> PathBuf {
        if fuse_path == "/" {
            self.root.clone()
        } else {
            self.root.join(fuse_path.trim_start_matches('/'))
        }
    }

    /// Get the job ID.
    pub fn job_id(&self) -> i32 {
        self.job_id
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fuse_config() {
        let config = FuseConfig::new(Path::new("/home/user/project"));
        assert_eq!(config.mount_point, PathBuf::from("/home/user/project/.tup/mnt"));
        assert!(config.single_threaded);
    }

    #[test]
    fn test_fuse_status_check() {
        let status = check_fuse_available();
        // Just verify it doesn't panic
        match status {
            FuseStatus::Available => println!("FUSE available"),
            FuseStatus::NotInstalled => println!("FUSE not installed"),
            FuseStatus::NeedsPrivileges => println!("FUSE needs privileges"),
            FuseStatus::Unsupported => println!("FUSE unsupported on this platform"),
        }
    }

    #[test]
    fn test_passthrough_real_path() {
        let fuse = PassthroughFuse::new(Path::new("/home/user/project"), 1);
        assert_eq!(fuse.real_path("/"), PathBuf::from("/home/user/project"));
        assert_eq!(fuse.real_path("/src/main.c"), PathBuf::from("/home/user/project/src/main.c"));
    }

    #[test]
    fn test_passthrough_job_id() {
        let fuse = PassthroughFuse::new(Path::new("/tmp"), 42);
        assert_eq!(fuse.job_id(), 42);
    }
}
