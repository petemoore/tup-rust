#![allow(dead_code, unused_imports)]
//! TUP FUSE filesystem implementation.
//!
//! Port of C tup's fuse_fs.c — intercepts file operations for dependency tracking.
//! Each command runs in a virtual path like `@tupjob-N/path/to/file` under `.tup/mnt`.
//! File writes are redirected to `.tup/tmp/` and reads are tracked as dependencies.
//!
//! C reference: src/tup/server/fuse_fs.c (1550 LOC)

use fuser::{
    FileAttr, FileType, Filesystem, ReplyAttr, ReplyCreate, ReplyData, ReplyDirectory, ReplyEntry,
    ReplyOpen, ReplyWrite, Request,
};
use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tup_types::AccessType;

/// Prefix for job paths in the FUSE mount.
/// C: #define TUP_JOB "@tupjob-"
const TUP_JOB: &str = "@tupjob-";

/// Directory for temporary output files.
/// C: #define TUP_TMP ".tup/tmp"
const TUP_TMP: &str = ".tup/tmp";

/// TTL for FUSE attribute caching.
const TTL: Duration = Duration::from_secs(1);

/// Per-job file tracking information.
///
/// C: struct file_info (file.h:57-76)
/// One instance per executing command, keyed by job ID.
pub struct FileInfo {
    /// Files read during execution.
    pub read_list: Vec<String>,
    /// Files written during execution.
    pub write_list: Vec<String>,
    /// Files deleted during execution.
    pub unlink_list: Vec<String>,
    /// Variables accessed (@tup@ virtual dir).
    pub var_list: Vec<String>,
    /// Output file mappings: realname → tmpname.
    /// C: mapping_list (TAILQ of struct mapping)
    pub mappings: BTreeMap<String, Mapping>,
    /// Virtual directories created by mkdir.
    /// C: tmpdir_list (TAILQ of struct tmpdir)
    pub tmpdirs: Vec<String>,
    /// Count of open file descriptors.
    pub open_count: i32,
    /// Error flag.
    pub server_fail: bool,
}

/// Mapping from virtual output path to temporary file.
/// C: struct mapping (file.h:37-42)
pub struct Mapping {
    /// Real name (virtual path relative to project root).
    pub realname: String,
    /// Temporary file path under .tup/tmp/.
    pub tmpname: PathBuf,
}

impl Default for FileInfo {
    fn default() -> Self {
        Self::new()
    }
}

impl FileInfo {
    pub fn new() -> Self {
        FileInfo {
            read_list: Vec::new(),
            write_list: Vec::new(),
            unlink_list: Vec::new(),
            var_list: Vec::new(),
            mappings: BTreeMap::new(),
            tmpdirs: Vec::new(),
            open_count: 0,
            server_fail: false,
        }
    }
}

/// Global counter for temporary file names.
/// C: static int filenum = 0 (in add_mapping_internal)
static FILENUM: AtomicI32 = AtomicI32::new(0);

/// The TUP FUSE filesystem.
///
/// Port of C tup's fuse_fs.c static globals + fuse_operations.
/// Manages per-job file tracking and intercepts all file operations.
pub struct TupFuseFs {
    /// Project root directory (where .tup/ lives).
    tup_top: PathBuf,
    /// Registry of active jobs: job_id → FileInfo.
    /// C: static struct thread_root troot
    jobs: Arc<RwLock<BTreeMap<i64, Arc<Mutex<FileInfo>>>>>,
    /// Our process group ID for context checking.
    /// C: static pid_t ourpgid
    ourpgid: u32,
}

impl TupFuseFs {
    pub fn new(tup_top: &Path) -> Self {
        let pgid = unsafe { libc::getpgid(0) } as u32;
        TupFuseFs {
            tup_top: tup_top.to_path_buf(),
            jobs: Arc::new(RwLock::new(BTreeMap::new())),
            ourpgid: pgid,
        }
    }

    /// Register a job for tracking.
    /// C: tup_fuse_add_group(id, finfo)
    pub fn add_job(&self, job_id: i64, finfo: Arc<Mutex<FileInfo>>) {
        self.jobs.write().unwrap().insert(job_id, finfo);
    }

    /// Unregister a job.
    /// C: tup_fuse_rm_group(finfo)
    pub fn remove_job(&self, job_id: i64) {
        self.jobs.write().unwrap().remove(&job_id);
    }

    /// Get the shared job registry.
    pub fn jobs(&self) -> Arc<RwLock<BTreeMap<i64, Arc<Mutex<FileInfo>>>>> {
        self.jobs.clone()
    }

    /// Extract job ID from a FUSE path.
    /// C: get_finfo() — extracts job number from @tupjob-N prefix.
    fn get_job_id(path: &str) -> Option<i64> {
        let path = path.strip_prefix('/')?;
        let rest = path.strip_prefix(TUP_JOB)?;
        let end = rest.find('/').unwrap_or(rest.len());
        rest[..end].parse().ok()
    }

    /// Strip the @tupjob-N prefix from a path, returning the real relative path.
    /// C: peel()
    fn peel(path: &str) -> &str {
        if let Some(rest) = path.strip_prefix('/') {
            if let Some(after_job) = rest.strip_prefix(TUP_JOB) {
                if let Some(slash_pos) = after_job.find('/') {
                    return &after_job[slash_pos..];
                } else {
                    return "/";
                }
            }
        }
        path
    }

    /// Check if a path should be hidden from dependency tracking.
    /// C: is_hidden()
    fn is_hidden(path: &str) -> bool {
        path.contains("/.git")
            || path.contains("/.tup")
            || path.contains("/.hg")
            || path.contains("/.svn")
            || path.contains("/.bzr")
    }

    /// Check if a path should be ignored entirely.
    /// C: ignore_file()
    fn should_ignore(path: &str) -> bool {
        path.starts_with("/dev") || path.starts_with("/sys") || path.starts_with("/proc")
    }

    /// Resolve a FUSE path to a real filesystem path.
    fn resolve_path(&self, fuse_path: &str) -> PathBuf {
        let peeled = Self::peel(fuse_path);
        if peeled == "/" {
            self.tup_top.clone()
        } else {
            self.tup_top.join(peeled.trim_start_matches('/'))
        }
    }

    /// Create a temporary file mapping for an output.
    /// C: add_mapping_internal()
    fn create_tmp_path(&self) -> PathBuf {
        let num = FILENUM.fetch_add(1, Ordering::SeqCst);
        self.tup_top.join(format!("{TUP_TMP}/{num:x}"))
    }
}

/// Convert SystemTime to UNIX timestamp.
fn system_time_to_unix(t: SystemTime) -> i64 {
    t.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs() as i64
}

/// Create a FileAttr from filesystem metadata.
fn metadata_to_attr(ino: u64, meta: &std::fs::Metadata) -> FileAttr {
    let kind = if meta.is_dir() {
        FileType::Directory
    } else if meta.is_symlink() {
        FileType::Symlink
    } else {
        FileType::RegularFile
    };

    FileAttr {
        ino,
        size: meta.len(),
        blocks: meta.blocks(),
        atime: UNIX_EPOCH + Duration::from_secs(meta.atime() as u64),
        mtime: UNIX_EPOCH + Duration::from_secs(meta.mtime() as u64),
        ctime: UNIX_EPOCH + Duration::from_secs(meta.ctime() as u64),
        crtime: UNIX_EPOCH,
        kind,
        perm: (meta.mode() & 0o7777) as u16,
        nlink: meta.nlink() as u32,
        uid: meta.uid(),
        gid: meta.gid(),
        rdev: meta.rdev() as u32,
        blksize: meta.blksize() as u32,
        flags: 0,
    }
}

// The fuser::Filesystem implementation will be added in the next commit
// once the core data structures are verified working.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_job_id() {
        assert_eq!(TupFuseFs::get_job_id("/@tupjob-42/src/foo.c"), Some(42));
        assert_eq!(TupFuseFs::get_job_id("/@tupjob-0/"), Some(0));
        assert_eq!(TupFuseFs::get_job_id("/other/path"), None);
        assert_eq!(TupFuseFs::get_job_id("/@tupjob-123"), Some(123));
    }

    #[test]
    fn test_peel() {
        assert_eq!(TupFuseFs::peel("/@tupjob-42/src/foo.c"), "/src/foo.c");
        assert_eq!(TupFuseFs::peel("/@tupjob-0/"), "/");
        assert_eq!(TupFuseFs::peel("/@tupjob-123"), "/");
        assert_eq!(TupFuseFs::peel("/normal/path"), "/normal/path");
    }

    #[test]
    fn test_is_hidden() {
        assert!(TupFuseFs::is_hidden("/project/.git/config"));
        assert!(TupFuseFs::is_hidden("/project/.tup/db"));
        assert!(!TupFuseFs::is_hidden("/project/src/main.c"));
    }

    #[test]
    fn test_should_ignore() {
        assert!(TupFuseFs::should_ignore("/dev/null"));
        assert!(TupFuseFs::should_ignore("/proc/self/fd"));
        assert!(TupFuseFs::should_ignore("/sys/class"));
        assert!(!TupFuseFs::should_ignore("/usr/include/stdio.h"));
    }

    #[test]
    fn test_file_info() {
        let finfo = FileInfo::new();
        assert!(finfo.read_list.is_empty());
        assert!(finfo.write_list.is_empty());
        assert_eq!(finfo.open_count, 0);
        assert!(!finfo.server_fail);
    }
}
