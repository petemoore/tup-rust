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

    /// Record a file access event.
    ///
    /// Port of C tup's handle_open_file() (file.c:197-230).
    /// Adds the filename to the appropriate list based on access type.
    /// For writes, also removes matching entries from the unlink list
    /// (C: check_unlink_list).
    pub fn handle_open_file(&mut self, at: AccessType, filename: &str) {
        match at {
            AccessType::Read => {
                self.read_list.push(filename.to_string());
            }
            AccessType::Write => {
                // C: check_unlink_list(filename, &info->unlink_list)
                // If a file was unlinked then written, remove the unlink entry
                self.unlink_list.retain(|u| u != filename);
                self.write_list.push(filename.to_string());
            }
            AccessType::Unlink => {
                self.unlink_list.push(filename.to_string());
            }
            AccessType::Var => {
                self.var_list.push(filename.to_string());
            }
            AccessType::Rename => {
                // Rename is handled separately by handle_rename()
                eprintln!("tup error: Invalid event type: rename in handle_open_file");
            }
        }
    }

    /// Record a file access with full path handling.
    ///
    /// Port of C tup's handle_file() (file.c:174-195).
    /// Dispatches to handle_open_file or handle_rename based on access type.
    /// Also follows symlinks for read accesses (C: add_symlinks).
    pub fn handle_file(&mut self, at: AccessType, filename: &str, file2: &str) {
        if at == AccessType::Rename {
            self.handle_rename(filename, file2);
        } else {
            self.handle_open_file(at, filename);
            // C: add_symlinks(filename, info) for reads
            // We skip symlink following for now — requires readlinkat()
        }
    }

    /// Record a rename event.
    ///
    /// Port of C tup's handle_rename() (file.c:573-602).
    /// A rename is treated as: unlink(old) + write(new).
    pub fn handle_rename(&mut self, from: &str, to: &str) {
        // C: treat rename as unlink(from) + write(to)
        self.handle_open_file(AccessType::Unlink, from);
        self.handle_open_file(AccessType::Write, to);
    }

    /// Process the unlink list after command execution.
    ///
    /// Port of C tup's handle_unlink() (file.c:623-643).
    /// For each unlinked file, if it was also written to during this
    /// command, the unlink is a no-op (the file was recreated).
    /// Otherwise, the unlink stands.
    pub fn handle_unlink(&mut self) {
        // C: For each entry in unlink_list, check if it appears in write_list.
        // If so, remove it from unlink_list (it was recreated).
        let write_set: std::collections::HashSet<&str> =
            self.write_list.iter().map(|s| s.as_str()).collect();
        self.unlink_list.retain(|u| !write_set.contains(u.as_str()));
    }

    /// Add a file mapping (output file → temporary file).
    ///
    /// Port of C tup's add_mapping_internal() (fuse_fs.c:165-214).
    /// Creates a mapping from a virtual output path to a temporary
    /// file in .tup/tmp/. Records the write access.
    pub fn add_mapping(&mut self, realname: &str, tup_top: &Path) -> PathBuf {
        let peeled = realname.trim_start_matches('/');

        // Record the write access (C: handle_open_file(ACCESS_WRITE, peeled, finfo))
        if !TupFuseFs::is_hidden(realname) {
            self.handle_open_file(AccessType::Write, peeled);
        }

        // Generate unique temporary filename
        let num = FILENUM.fetch_add(1, Ordering::SeqCst);
        let tmpname = tup_top.join(format!("{TUP_TMP}/{num:x}"));

        self.mappings.insert(
            peeled.to_string(),
            Mapping {
                realname: peeled.to_string(),
                tmpname: tmpname.clone(),
            },
        );

        tmpname
    }

    /// Find a mapping by its real name.
    ///
    /// Port of C tup's find_mapping() (fuse_fs.c:229-241).
    pub fn find_mapping(&self, realname: &str) -> Option<&Mapping> {
        let peeled = realname.trim_start_matches('/');
        self.mappings.get(peeled)
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

    // WP1: File access tracking tests (port of file.c behavior)

    #[test]
    fn test_handle_open_file_read() {
        // C: handle_open_file(ACCESS_READ, "foo.c", info) → adds to read_list
        let mut finfo = FileInfo::new();
        finfo.handle_open_file(AccessType::Read, "foo.c");
        assert_eq!(finfo.read_list, vec!["foo.c"]);
        assert!(finfo.write_list.is_empty());
    }

    #[test]
    fn test_handle_open_file_write() {
        // C: handle_open_file(ACCESS_WRITE, "foo.o", info) → adds to write_list
        let mut finfo = FileInfo::new();
        finfo.handle_open_file(AccessType::Write, "foo.o");
        assert_eq!(finfo.write_list, vec!["foo.o"]);
        assert!(finfo.read_list.is_empty());
    }

    #[test]
    fn test_handle_open_file_unlink() {
        // C: handle_open_file(ACCESS_UNLINK, "old.o", info) → adds to unlink_list
        let mut finfo = FileInfo::new();
        finfo.handle_open_file(AccessType::Unlink, "old.o");
        assert_eq!(finfo.unlink_list, vec!["old.o"]);
    }

    #[test]
    fn test_handle_open_file_var() {
        // C: handle_open_file(ACCESS_VAR, "CONFIG_FOO", info) → adds to var_list
        let mut finfo = FileInfo::new();
        finfo.handle_open_file(AccessType::Var, "CONFIG_FOO");
        assert_eq!(finfo.var_list, vec!["CONFIG_FOO"]);
    }

    #[test]
    fn test_write_clears_unlink() {
        // C: check_unlink_list(filename, &info->unlink_list) removes matching unlink
        // When a file is unlinked then written, the unlink is canceled
        let mut finfo = FileInfo::new();
        finfo.handle_open_file(AccessType::Unlink, "foo.o");
        assert_eq!(finfo.unlink_list.len(), 1);
        finfo.handle_open_file(AccessType::Write, "foo.o");
        assert!(finfo.unlink_list.is_empty());
        assert_eq!(finfo.write_list, vec!["foo.o"]);
    }

    #[test]
    fn test_handle_rename() {
        // C: handle_rename(from, to, info) → unlink(from) + write(to)
        let mut finfo = FileInfo::new();
        finfo.handle_rename("old.txt", "new.txt");
        assert_eq!(finfo.unlink_list, vec!["old.txt"]);
        assert_eq!(finfo.write_list, vec!["new.txt"]);
    }

    #[test]
    fn test_handle_file_dispatches() {
        // C: handle_file() dispatches to handle_open_file or handle_rename
        let mut finfo = FileInfo::new();
        finfo.handle_file(AccessType::Read, "input.c", "");
        finfo.handle_file(AccessType::Write, "output.o", "");
        finfo.handle_file(AccessType::Rename, "old.txt", "new.txt");
        assert_eq!(finfo.read_list, vec!["input.c"]);
        assert_eq!(finfo.write_list, vec!["output.o", "new.txt"]);
        assert_eq!(finfo.unlink_list, vec!["old.txt"]);
    }

    #[test]
    fn test_handle_unlink_processing() {
        // C: handle_unlink() removes unlinks that were also written
        let mut finfo = FileInfo::new();
        finfo.handle_open_file(AccessType::Unlink, "keep.txt");
        finfo.handle_open_file(AccessType::Unlink, "recreated.txt");
        finfo.handle_open_file(AccessType::Write, "recreated.txt");
        // Note: write already clears matching unlinks via check_unlink_list
        // But handle_unlink() does a final pass
        finfo.handle_unlink();
        assert_eq!(finfo.unlink_list, vec!["keep.txt"]);
    }

    #[test]
    fn test_add_mapping() {
        let tmp = tempfile::tempdir().unwrap();
        let mut finfo = FileInfo::new();
        let tmpname = finfo.add_mapping("/src/output.o", tmp.path());
        assert!(tmpname.to_string_lossy().contains(".tup/tmp/"));
        assert!(finfo.write_list.contains(&"src/output.o".to_string()));
        assert!(finfo.find_mapping("src/output.o").is_some());
    }

    #[test]
    fn test_find_mapping() {
        let tmp = tempfile::tempdir().unwrap();
        let mut finfo = FileInfo::new();
        finfo.add_mapping("/build/foo.o", tmp.path());
        assert!(finfo.find_mapping("build/foo.o").is_some());
        assert!(finfo.find_mapping("nonexistent").is_none());
    }

    #[test]
    fn test_hidden_not_tracked() {
        // C: is_hidden() paths should not be added to write_list
        let tmp = tempfile::tempdir().unwrap();
        let mut finfo = FileInfo::new();
        finfo.add_mapping("/.git/config", tmp.path());
        assert!(finfo.write_list.is_empty()); // .git is hidden
    }
}
