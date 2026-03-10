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
use std::sync::atomic::{AtomicI32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tup_types::AccessType;

/// Prefix for job paths in the FUSE mount.
/// C: #define TUP_JOB "@tupjob-"
const TUP_JOB: &str = "@tupjob-";

/// Directory for temporary output files.
/// C: #define TUP_TMP ".tup/tmp"
const TUP_TMP: &str = ".tup/tmp";

/// Virtual directory for @-variable access.
/// C: #define TUP_VAR_VIRTUAL_DIR "@tup@"
const TUP_VAR_VIRTUAL_DIR: &str = "@tup@";

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
            // C: add_symlinks(filename, info) for reads (file.c:186-188)
            // If this file is a symlink, also track its target as a read dependency.
            self.add_symlinks(filename);
        }
    }

    /// Follow symlinks and record their targets as read dependencies.
    /// Port of C tup's add_symlinks() + get_symlink() (file.c:93-144).
    fn add_symlinks(&mut self, path: &str) {
        if let Ok(target) = std::fs::read_link(path) {
            let link_path = if target.is_absolute() {
                target.to_string_lossy().to_string()
            } else {
                // Relative symlink — resolve relative to the file's directory
                if let Some(last_slash) = path.rfind('/') {
                    format!("{}/{}", &path[..last_slash + 1], target.display())
                } else {
                    target.to_string_lossy().to_string()
                }
            };
            self.handle_open_file(AccessType::Read, &link_path);
        }
    }

    /// Record a rename event.
    ///
    /// Port of C tup's handle_rename() (file.c:573-602).
    /// Renames existing write_list/read_list entries in-place from old→new,
    /// then removes any unlink entry for the destination.
    pub fn handle_rename(&mut self, from: &str, to: &str) {
        // C: Walk write_list, rename entries matching `from` to `to`
        for entry in &mut self.write_list {
            if entry == from {
                *entry = to.to_string();
            }
        }
        // C: Walk read_list, rename entries matching `from` to `to`
        for entry in &mut self.read_list {
            if entry == from {
                *entry = to.to_string();
            }
        }
        // C: check_unlink_list(to, &info->unlink_list)
        self.unlink_list.retain(|u| u != to);
    }

    /// Process the unlink list after command execution.
    ///
    /// Port of C tup's handle_unlink() (file.c:623-643).
    /// For each unlinked file, remove matching entries from BOTH
    /// write_list AND read_list, then discard the unlink entry.
    pub fn handle_unlink(&mut self) {
        // C: For each entry in unlink_list:
        //   - remove matching entries from write_list
        //   - remove matching entries from read_list
        //   - then remove the unlink entry itself
        let unlinks: Vec<String> = self.unlink_list.drain(..).collect();
        for u in &unlinks {
            self.write_list.retain(|w| w != u);
            self.read_list.retain(|r| r != u);
        }
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
    ourpgid: i32,
    /// Maximum open files before falling back to on-the-fly opens.
    /// C: static int max_open_files = 128
    max_open_files: i32,
    /// Inode→path mapping.
    /// fuser uses inodes (low-level API); C tup uses paths (high-level API).
    /// We bridge the gap with this table. Inode 1 = root.
    inodes: RwLock<BTreeMap<u64, PathBuf>>,
    /// Path→inode reverse mapping.
    path_to_inode: RwLock<BTreeMap<PathBuf, u64>>,
    /// Next inode number.
    next_inode: AtomicU64,
}

impl TupFuseFs {
    /// Port of C tup's tup_fuse_fs_init() (fuse_fs.c:50-69).
    /// Detects max open files via RLIMIT_NOFILE and stores our process group ID.
    pub fn new(tup_top: &Path) -> Self {
        let pgid = unsafe { libc::getpgid(0) };

        // C: tup_fuse_fs_init() — detect max open files
        // Keep doubling rlim_cur until we hit the real limit (macOS sets rlim_max
        // to -1, so we probe). Then use half that as our max_open_files.
        let mut max_open_files: i32 = 128;
        unsafe {
            let mut rlim: libc::rlimit = std::mem::zeroed();
            if libc::getrlimit(libc::RLIMIT_NOFILE, &mut rlim) == 0 {
                for _ in 0..10 {
                    rlim.rlim_cur *= 2;
                    if libc::setrlimit(libc::RLIMIT_NOFILE, &rlim) != 0 {
                        break;
                    }
                }
                if libc::getrlimit(libc::RLIMIT_NOFILE, &mut rlim) == 0 {
                    max_open_files = (rlim.rlim_cur / 2) as i32;
                }
            }
        }

        let mut inodes = BTreeMap::new();
        let mut path_to_inode = BTreeMap::new();
        // Inode 1 = FUSE root (= tup_top directory)
        inodes.insert(1, tup_top.to_path_buf());
        path_to_inode.insert(tup_top.to_path_buf(), 1);
        TupFuseFs {
            tup_top: tup_top.to_path_buf(),
            jobs: Arc::new(RwLock::new(BTreeMap::new())),
            ourpgid: pgid,
            max_open_files,
            inodes: RwLock::new(inodes),
            path_to_inode: RwLock::new(path_to_inode),
            next_inode: AtomicU64::new(2), // 1 is root
        }
    }

    /// Get or assign an inode for a path.
    fn get_or_create_inode(&self, path: &Path) -> u64 {
        let p2i = self.path_to_inode.read().unwrap();
        if let Some(&ino) = p2i.get(path) {
            return ino;
        }
        drop(p2i);

        let ino = self.next_inode.fetch_add(1, Ordering::SeqCst);
        self.inodes.write().unwrap().insert(ino, path.to_path_buf());
        self.path_to_inode
            .write()
            .unwrap()
            .insert(path.to_path_buf(), ino);
        ino
    }

    /// Look up the path for an inode.
    fn inode_path(&self, ino: u64) -> Option<PathBuf> {
        self.inodes.read().unwrap().get(&ino).cloned()
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

    /// Verify that the requesting process is in our process group.
    /// Port of C tup's context_check() (fuse_fs.c:243-281).
    /// Returns true if OK, false if the process is not allowed.
    fn context_check(&self, req: &Request<'_>) -> bool {
        let pid = req.pid() as i32;
        let pgid = unsafe { libc::getpgid(pid) };
        // C: OSX/container: pgid == -1 && errno == ESRCH → allow (zombie or container)
        if pgid == -1 {
            let err = std::io::Error::last_os_error();
            if err.raw_os_error() == Some(libc::ESRCH) {
                return true;
            }
            return false;
        }
        if self.ourpgid != pgid {
            return false;
        }
        true
    }

    /// Check if a path should be hidden from dependency tracking.
    /// Port of C tup's is_hidden() (fuse_fs.c:92-107).
    pub fn is_hidden(path: &str) -> bool {
        path.contains("/.git")
            || path.contains("/.tup")
            || path.contains("/.hg")
            || path.contains("/.svn")
            || path.contains("/.bzr")
            || Self::is_ccache_path(path)
    }

    /// Check if a path should be ignored entirely.
    /// Port of C tup's ignore_file() (fuse_fs.c:283-294).
    fn should_ignore(path: &str) -> bool {
        path.starts_with("/dev")
            || path.starts_with("/sys")
            || path.starts_with("/proc")
            || Self::is_ccache_path(path)
    }

    /// Check if a path is a ccache/icecream path.
    /// Port of C tup's is_ccache_path() (ccache.c:24-44).
    fn is_ccache_path(path: &str) -> bool {
        path.contains("/.ccache")
            || path.contains("/.cache/ccache")
            || path.contains("/ccache-tmp/")
            || path.starts_with("/tmp/.icecream-")
    }

    /// Check if a peeled path refers to the @tup@ virtual variable directory.
    /// Port of C tup's get_virtual_var() (fuse_fs.c:319-340).
    /// Returns Some("") for the @tup@ directory itself, Some("VARNAME") for a variable,
    /// or None if not a @tup@ path.
    fn get_virtual_var<'a>(&self, peeled: &'a str) -> Option<&'a str> {
        // C: The peeled path must start with tup_top, then contain @tup@
        // In our case, peeled is already relative to tup_top, so just look for @tup@
        if let Some(idx) = peeled.find(TUP_VAR_VIRTUAL_DIR) {
            let after = &peeled[idx + TUP_VAR_VIRTUAL_DIR.len()..];
            if after.is_empty() {
                return Some(""); // Just the @tup@ directory
            }
            if let Some(rest) = after.strip_prefix('/') {
                return Some(rest); // Variable name
            }
        }
        None
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
    /// Resolve a path that might be inside a @tupjob-N virtual dir
    /// to the real filesystem path. Virtual @tupjob-N paths map to tup_top.
    fn resolve_real_path(&self, virtual_path: &Path) -> PathBuf {
        let path_str = virtual_path.to_string_lossy();
        if let Some(idx) = path_str.find(TUP_JOB) {
            // Find the end of @tupjob-N/ and get the rest
            let after_prefix = &path_str[idx + TUP_JOB.len()..];
            if let Some(slash_pos) = after_prefix.find('/') {
                let relative = &after_prefix[slash_pos + 1..];
                if relative.is_empty() {
                    return self.tup_top.clone();
                }
                return self.tup_top.join(relative);
            }
            return self.tup_top.clone();
        }
        virtual_path.to_path_buf()
    }

    /// C: add_mapping_internal()
    fn create_tmp_path(&self) -> PathBuf {
        let num = FILENUM.fetch_add(1, Ordering::SeqCst);
        self.tup_top.join(format!("{TUP_TMP}/{num:x}"))
    }

    /// Get the FileInfo for a path, if it belongs to a registered job.
    /// Returns (job_id, finfo_arc, peeled_path).
    /// Equivalent to C's get_finfo() + peel() combined.
    fn get_finfo_and_peeled(&self, path: &Path) -> Option<(i64, Arc<Mutex<FileInfo>>, String)> {
        let path_str = path.to_string_lossy();
        let full_path = format!("/{}", path_str.trim_start_matches('/'));
        let job_id = Self::get_job_id(&full_path)?;
        let peeled = Self::peel(&full_path).trim_start_matches('/').to_string();
        let jobs = self.jobs.read().unwrap();
        let finfo = jobs.get(&job_id)?.clone();
        Some((job_id, finfo, peeled))
    }

    /// Record a file access for a given inode.
    ///
    /// Port of C tup's tup_fuse_handle_file() (fuse_fs.c:296-317).
    /// Determines the job ID from the inode's path, looks up the FileInfo,
    /// and records the access.
    fn record_access(&self, ino: u64, at: tup_types::AccessType) {
        let path = match self.inode_path(ino) {
            Some(p) => p,
            None => return,
        };

        let path_str = path.to_string_lossy();

        // Extract job ID from the path (if it contains @tupjob-N)
        let job_id = if let Some(idx) = path_str.find(TUP_JOB) {
            let after = &path_str[idx + TUP_JOB.len()..];
            let end = after.find('/').unwrap_or(after.len());
            after[..end].parse::<i64>().ok()
        } else {
            None
        };

        let job_id = match job_id {
            Some(id) => id,
            None => return, // Not a job path, skip
        };

        // Get the peeled path (relative to project root)
        let full_path = format!("/{}", path_str.trim_start_matches('/'));
        let peeled = Self::peel(&full_path);
        let peeled = peeled.trim_start_matches('/');

        // Skip hidden and system paths
        if Self::is_hidden(peeled) || Self::should_ignore(&format!("/{peeled}")) {
            return;
        }

        // Look up the job's FileInfo and record the access
        if let Some(finfo) = self.jobs.read().unwrap().get(&job_id) {
            if let Ok(mut finfo) = finfo.lock() {
                finfo.handle_open_file(at, peeled);
            }
        }
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

/// Helper: create a synthetic directory FileAttr.
fn synthetic_dir_attr(ino: u64) -> FileAttr {
    FileAttr {
        ino,
        size: 0,
        blocks: 0,
        atime: SystemTime::now(),
        mtime: SystemTime::now(),
        ctime: SystemTime::now(),
        crtime: SystemTime::now(),
        kind: FileType::Directory,
        perm: 0o755,
        nlink: 2,
        uid: unsafe { libc::getuid() },
        gid: unsafe { libc::getgid() },
        rdev: 0,
        blksize: 4096,
        flags: 0,
    }
}

/// Helper: open a file by CString path, returning fd or errno.
fn libc_open(path: &Path, flags: i32, mode: libc::mode_t) -> Result<i32, i32> {
    let c_path =
        std::ffi::CString::new(path.to_string_lossy().as_bytes()).map_err(|_| libc::EINVAL)?;
    let fd = unsafe { libc::open(c_path.as_ptr(), flags, mode as libc::c_uint) };
    if fd < 0 {
        Err(std::io::Error::last_os_error()
            .raw_os_error()
            .unwrap_or(libc::EIO))
    } else {
        Ok(fd)
    }
}

/// Implement the fuser Filesystem trait for tup's passthrough FUSE.
///
/// C tup uses the high-level (path-based) libfuse API. The fuser crate
/// uses the low-level (inode-based) API, so we maintain an inode→path
/// mapping to bridge the gap.
///
/// Port of C tup's tup_fs_* callbacks from fuse_fs.c.
impl Filesystem for TupFuseFs {
    /// Look up a directory entry by name.
    /// C: implicit in getattr/readdir path-based operations.
    fn lookup(&mut self, req: &Request<'_>, parent: u64, name: &OsStr, reply: ReplyEntry) {
        if !self.context_check(req) {
            reply.error(libc::EPERM);
            return;
        }
        let parent_path = match self.inode_path(parent) {
            Some(p) => p,
            None => {
                reply.error(libc::ENOENT);
                return;
            }
        };

        let name_str = name.to_string_lossy();

        // Handle @tupjob-N virtual directories.
        if name_str.starts_with(TUP_JOB) {
            if let Some(job_id_str) = name_str.strip_prefix(TUP_JOB) {
                if job_id_str.parse::<i64>().is_ok() {
                    let ino = self.get_or_create_inode(&parent_path.join(name));
                    reply.entry(&TTL, &synthetic_dir_attr(ino), 0);
                    return;
                }
            }
        }

        let virtual_child = parent_path.join(name);

        // Check if this matches a tmpdir or mapping in any job's FileInfo
        if let Some((_job_id, finfo_arc, peeled)) = self.get_finfo_and_peeled(&virtual_child) {
            if let Ok(finfo) = finfo_arc.lock() {
                // Check tmpdirs
                if finfo.tmpdirs.iter().any(|td| td == &peeled) {
                    let ino = self.get_or_create_inode(&virtual_child);
                    reply.entry(&TTL, &synthetic_dir_attr(ino), 0);
                    return;
                }
                // Check mappings — stat the tmpname instead
                if let Some(mapping) = finfo.find_mapping(&peeled) {
                    let tmpname = mapping.tmpname.clone();
                    drop(finfo);
                    if let Ok(meta) = std::fs::symlink_metadata(&tmpname) {
                        let ino = self.get_or_create_inode(&virtual_child);
                        reply.entry(&TTL, &metadata_to_attr(ino, &meta), 0);
                        return;
                    }
                }
            }
        }

        // Fall through to real filesystem
        let real_child = self.resolve_real_path(&virtual_child);
        match std::fs::symlink_metadata(&real_child) {
            Ok(meta) => {
                let ino = self.get_or_create_inode(&virtual_child);
                reply.entry(&TTL, &metadata_to_attr(ino, &meta), 0);
            }
            Err(_) => {
                reply.error(libc::ENOENT);
            }
        }
    }

    /// Get file attributes.
    /// Port of C tup's tup_fs_getattr() (fuse_fs.c:344-435).
    fn getattr(&mut self, req: &Request<'_>, ino: u64, reply: ReplyAttr) {
        if !self.context_check(req) {
            reply.error(libc::EPERM);
            return;
        }
        let path = match self.inode_path(ino) {
            Some(p) => p,
            None => {
                reply.error(libc::ENOENT);
                return;
            }
        };

        // Check if this is the @tupjob-N directory itself
        let path_str = path.to_string_lossy();
        let is_job_dir_itself = if let Some(idx) = path_str.find(TUP_JOB) {
            let after = &path_str[idx + TUP_JOB.len()..];
            let after_num = after.find('/').map(|p| &after[p + 1..]).unwrap_or("");
            after_num.is_empty()
        } else {
            false
        };

        if is_job_dir_itself {
            reply.attr(&TTL, &synthetic_dir_attr(ino));
            return;
        }

        // Check job-specific resources: tmpdirs, mappings, @tup@ vars
        if let Some((_job_id, finfo_arc, peeled)) = self.get_finfo_and_peeled(&path) {
            // C: Protect .tup/mnt from sub-processes (t6056)
            if peeled == ".tup/mnt" || peeled.contains("/.tup/mnt") {
                reply.error(libc::EPERM);
                return;
            }

            if let Ok(mut finfo) = finfo_arc.lock() {
                // Check tmpdirs — return tup_top's stat (C: fstat(tup_top_fd(), stbuf))
                if finfo.tmpdirs.iter().any(|td| td == &peeled) {
                    if let Ok(meta) = std::fs::symlink_metadata(&self.tup_top) {
                        reply.attr(&TTL, &metadata_to_attr(ino, &meta));
                        return;
                    }
                }
                // Check mappings — stat the tmpname
                if let Some(mapping) = finfo.find_mapping(&peeled) {
                    let tmpname = mapping.tmpname.clone();
                    drop(finfo);
                    if let Ok(meta) = std::fs::symlink_metadata(&tmpname) {
                        reply.attr(&TTL, &metadata_to_attr(ino, &meta));
                        return;
                    }
                } else {
                    // C: Check @tup@ virtual variable directory (fuse_fs.c:401-423)
                    if let Some(var) = self.get_virtual_var(&peeled) {
                        if var.is_empty() {
                            // @tup@ directory itself — return as directory
                            reply.attr(&TTL, &synthetic_dir_attr(ino));
                            return;
                        } else {
                            // @tup@/VARNAME — record var access and return error
                            finfo.handle_open_file(AccessType::Var, var);
                            reply.error(libc::ENOENT);
                            return;
                        }
                    }
                    drop(finfo);
                }
            }
        }

        // Fall through to real filesystem
        let real_path = self.resolve_real_path(&path);
        match std::fs::symlink_metadata(&real_path) {
            Ok(meta) => {
                self.record_access(ino, tup_types::AccessType::Read);
                reply.attr(&TTL, &metadata_to_attr(ino, &meta));
            }
            Err(_) => {
                reply.error(libc::ENOENT);
            }
        }
    }

    /// Check file access permissions.
    /// Port of C tup's tup_fs_access() (fuse_fs.c:437-495).
    fn access(&mut self, req: &Request<'_>, ino: u64, mask: i32, reply: fuser::ReplyEmpty) {
        if !self.context_check(req) {
            reply.error(libc::EPERM);
            return;
        }
        let path = match self.inode_path(ino) {
            Some(p) => p,
            None => {
                reply.error(libc::ENOENT);
                return;
            }
        };

        // Check job-specific: mappings and tmpdirs
        if let Some((_job_id, finfo_arc, peeled)) = self.get_finfo_and_peeled(&path) {
            if let Ok(finfo) = finfo_arc.lock() {
                // Mapped file — check access on tmpname
                if let Some(mapping) = finfo.find_mapping(&peeled) {
                    let c_path = match std::ffi::CString::new(
                        mapping.tmpname.to_string_lossy().as_bytes(),
                    ) {
                        Ok(p) => p,
                        Err(_) => {
                            reply.error(libc::EINVAL);
                            return;
                        }
                    };
                    let rc = unsafe { libc::access(c_path.as_ptr(), mask) };
                    if rc == 0 {
                        reply.ok();
                    } else {
                        reply.error(
                            std::io::Error::last_os_error()
                                .raw_os_error()
                                .unwrap_or(libc::EACCES),
                        );
                    }
                    return;
                }
                // C: Check @tup@ virtual directory in access() (fuse_fs.c:483-487)
                if let Some(var) = self.get_virtual_var(&peeled) {
                    if var.is_empty() {
                        reply.ok();
                        return;
                    }
                }
                // Tmpdir — use tup_top access (C: faccessat(tup_top_fd(), ".", mask))
                if finfo.tmpdirs.iter().any(|td| td == &peeled) {
                    let c_path =
                        match std::ffi::CString::new(self.tup_top.to_string_lossy().as_bytes()) {
                            Ok(p) => p,
                            Err(_) => {
                                reply.error(libc::EINVAL);
                                return;
                            }
                        };
                    let rc = unsafe { libc::access(c_path.as_ptr(), mask) };
                    if rc == 0 {
                        reply.ok();
                    } else {
                        reply.error(
                            std::io::Error::last_os_error()
                                .raw_os_error()
                                .unwrap_or(libc::EACCES),
                        );
                    }
                    return;
                }
            }
        }

        // Fall through to real filesystem
        let real_path = self.resolve_real_path(&path);
        let c_path = match std::ffi::CString::new(real_path.to_string_lossy().as_bytes()) {
            Ok(p) => p,
            Err(_) => {
                reply.error(libc::EINVAL);
                return;
            }
        };
        let rc = unsafe { libc::access(c_path.as_ptr(), mask) };
        if rc == 0 {
            reply.ok();
        } else {
            reply.error(
                std::io::Error::last_os_error()
                    .raw_os_error()
                    .unwrap_or(libc::EACCES),
            );
        }
    }

    /// Set file attributes (chmod, truncate, utimes).
    /// Port of C tup's tup_fs_chmod/truncate/utimens (fuse_fs.c:1049-1237).
    /// Only operates on mapped files and tmpdirs; returns EPERM otherwise.
    fn setattr(
        &mut self,
        req: &Request<'_>,
        ino: u64,
        mode: Option<u32>,
        uid: Option<u32>,
        gid: Option<u32>,
        size: Option<u64>,
        _atime: Option<fuser::TimeOrNow>,
        _mtime: Option<fuser::TimeOrNow>,
        _ctime: Option<SystemTime>,
        _fh: Option<u64>,
        _crtime: Option<SystemTime>,
        _chgtime: Option<SystemTime>,
        _bkuptime: Option<SystemTime>,
        _flags: Option<u32>,
        reply: ReplyAttr,
    ) {
        if !self.context_check(req) {
            reply.error(libc::EPERM);
            return;
        }
        let path = match self.inode_path(ino) {
            Some(p) => p,
            None => {
                reply.error(libc::ENOENT);
                return;
            }
        };

        // Determine the target path: mapped tmpname, tmpdir, or real file
        let target_path =
            if let Some((_job_id, finfo_arc, peeled)) = self.get_finfo_and_peeled(&path) {
                if let Ok(finfo) = finfo_arc.lock() {
                    if let Some(mapping) = finfo.find_mapping(&peeled) {
                        // Mapped file — operate on tmpname
                        mapping.tmpname.clone()
                    } else if finfo.tmpdirs.iter().any(|td| td == &peeled) {
                        // Tmpdir — chmod/utimens are no-ops
                        if let Ok(meta) = std::fs::symlink_metadata(&self.tup_top) {
                            reply.attr(&TTL, &metadata_to_attr(ino, &meta));
                        } else {
                            reply.error(libc::EIO);
                        }
                        return;
                    } else if Self::is_hidden(&peeled) {
                        // Hidden files can be modified
                        self.resolve_real_path(&path)
                    } else {
                        // Not a mapped file, not a tmpdir, not hidden → EPERM
                        let peeled_str = peeled.clone();
                        drop(finfo);
                        eprintln!(
                        "tup error: Unable to modify files not created by this job: {peeled_str}"
                    );
                        reply.error(libc::EPERM);
                        return;
                    }
                } else {
                    self.resolve_real_path(&path)
                }
            } else {
                self.resolve_real_path(&path)
            };

        // Handle truncate
        if let Some(new_size) = size {
            if let Ok(f) = std::fs::OpenOptions::new().write(true).open(&target_path) {
                let _ = f.set_len(new_size);
            }
        }

        // Handle chmod (C: tup_fs_chmod, fuse_fs.c:1050-1087)
        if let Some(new_mode) = mode {
            use std::os::unix::fs::PermissionsExt;
            let perms = std::fs::Permissions::from_mode(new_mode);
            let _ = std::fs::set_permissions(&target_path, perms);
        }

        // Handle chown (C: tup_fs_chown, fuse_fs.c:1090-1127)
        if uid.is_some() || gid.is_some() {
            let c_path = std::ffi::CString::new(target_path.to_string_lossy().as_bytes());
            if let Ok(c_path) = c_path {
                let new_uid = uid.map(|u| u as libc::uid_t).unwrap_or(u32::MAX);
                let new_gid = gid.map(|g| g as libc::gid_t).unwrap_or(u32::MAX);
                unsafe {
                    libc::lchown(c_path.as_ptr(), new_uid, new_gid);
                }
            }
        }

        match std::fs::symlink_metadata(&target_path) {
            Ok(meta) => reply.attr(&TTL, &metadata_to_attr(ino, &meta)),
            Err(_) => reply.error(libc::EIO),
        }
    }

    /// Get filesystem statistics.
    /// Port of C tup's tup_fs_statfs() (fuse_fs.c:1396-1437).
    fn statfs(&mut self, req: &Request<'_>, ino: u64, reply: fuser::ReplyStatfs) {
        if !self.context_check(req) {
            reply.error(libc::EPERM);
            return;
        }
        let path = self.inode_path(ino).unwrap_or_else(|| self.tup_top.clone());
        let real_path = self.resolve_real_path(&path);
        let c_path = match std::ffi::CString::new(real_path.to_string_lossy().as_bytes()) {
            Ok(p) => p,
            Err(_) => {
                reply.statfs(0, 0, 0, 0, 0, 512, 255, 0);
                return;
            }
        };
        let fd = unsafe { libc::open(c_path.as_ptr(), libc::O_RDONLY) };
        if fd < 0 {
            reply.statfs(0, 0, 0, 0, 0, 512, 255, 0);
            return;
        }
        let mut stbuf: libc::statvfs = unsafe { std::mem::zeroed() };
        let rc = unsafe { libc::fstatvfs(fd, &mut stbuf) };
        unsafe { libc::close(fd) };
        if rc == 0 {
            reply.statfs(
                stbuf.f_blocks as u64,
                stbuf.f_bfree as u64,
                stbuf.f_bavail as u64,
                stbuf.f_files as u64,
                stbuf.f_ffree as u64,
                stbuf.f_bsize as u32,
                stbuf.f_namemax as u32,
                stbuf.f_frsize as u32,
            );
        } else {
            reply.statfs(0, 0, 0, 0, 0, 512, 255, 0);
        }
    }

    /// Flush is called on close — just return OK.
    /// C: tup_fs_flush() (fuse_fs.c:1439-1454) — ensures data written before process exits.
    fn flush(
        &mut self,
        _req: &Request<'_>,
        _ino: u64,
        _fh: u64,
        _lock_owner: u64,
        reply: fuser::ReplyEmpty,
    ) {
        reply.ok();
    }

    /// Read directory entries.
    /// Port of C tup's tup_fs_readdir() (fuse_fs.c:588-724).
    fn readdir(
        &mut self,
        req: &Request<'_>,
        ino: u64,
        _fh: u64,
        offset: i64,
        mut reply: ReplyDirectory,
    ) {
        if !self.context_check(req) {
            reply.error(libc::EPERM);
            return;
        }
        let dir_path = match self.inode_path(ino) {
            Some(p) => p,
            None => {
                reply.error(libc::ENOENT);
                return;
            }
        };

        let mut full_entries: Vec<(u64, FileType, String)> = Vec::new();
        // Add . and ..
        full_entries.push((ino, FileType::Directory, ".".to_string()));
        if let Some(parent) = dir_path.parent() {
            let parent_ino = self.get_or_create_inode(parent);
            full_entries.push((parent_ino, FileType::Directory, "..".to_string()));
        }

        // For the FUSE root, also add @tupjob-N entries for registered jobs
        if ino == 1 {
            let jobs = self.jobs.read().unwrap();
            for &job_id in jobs.keys() {
                let job_name = format!("{TUP_JOB}{job_id}");
                let job_path = dir_path.join(&job_name);
                let job_ino = self.get_or_create_inode(&job_path);
                full_entries.push((job_ino, FileType::Directory, job_name));
            }
        }

        // Check if this is a tmpdir (if so, only show mapped files/sub-tmpdirs, not real dir)
        let mut is_tmpdir = false;
        let peeled = if let Some((_job_id, finfo_arc, peeled)) =
            self.get_finfo_and_peeled(&dir_path)
        {
            if let Ok(finfo) = finfo_arc.lock() {
                if finfo.tmpdirs.iter().any(|td| td == &peeled) {
                    is_tmpdir = true;
                }

                // C: Add mapped files whose realname is in this directory
                for (realname, mapping) in &finfo.mappings {
                    let in_dir = if peeled == "." {
                        !realname.contains('/')
                    } else if let Some(rest) = realname.strip_prefix(&peeled) {
                        rest.starts_with('/') && !rest[1..].contains('/')
                    } else {
                        false
                    };
                    if in_dir {
                        let basename = realname.rsplit('/').next().unwrap_or(realname);
                        // Stat the tmpname to get the file type
                        if let Ok(meta) = std::fs::symlink_metadata(&mapping.tmpname) {
                            let child_path = dir_path.join(basename);
                            let child_ino = self.get_or_create_inode(&child_path);
                            let ft = if meta.is_dir() {
                                FileType::Directory
                            } else if meta.is_symlink() {
                                FileType::Symlink
                            } else {
                                FileType::RegularFile
                            };
                            full_entries.push((child_ino, ft, basename.to_string()));
                        }
                    }
                }

                // C: Add tmpdir subdirectories that are children of this directory
                for tmpdir in &finfo.tmpdirs {
                    let in_dir = if peeled == "." {
                        !tmpdir.contains('/')
                    } else if let Some(rest) = tmpdir.strip_prefix(&peeled) {
                        rest.starts_with('/') && !rest[1..].contains('/')
                    } else {
                        false
                    };
                    if in_dir {
                        let basename = tmpdir.rsplit('/').next().unwrap_or(tmpdir);
                        let child_path = dir_path.join(basename);
                        let child_ino = self.get_or_create_inode(&child_path);
                        full_entries.push((child_ino, FileType::Directory, basename.to_string()));
                    }
                }
            }
            Some(peeled)
        } else {
            None
        };

        // If this IS a tmpdir, don't read the real directory (C: return 0 if is_tmpdir)
        if !is_tmpdir {
            // Record read access for dependency tracking
            if peeled.is_some() {
                self.record_access(ino, tup_types::AccessType::Read);
            }

            // Read real directory
            let real_dir = self.resolve_real_path(&dir_path);
            if let Ok(entries) = std::fs::read_dir(&real_dir) {
                let is_inside_job = peeled.is_some();
                for entry in entries.flatten() {
                    let name = entry.file_name().to_string_lossy().to_string();
                    // C: filter out .tup if inside a job
                    if is_inside_job && name == ".tup" {
                        continue;
                    }
                    let child_path = dir_path.join(&name);
                    let child_ino = self.get_or_create_inode(&child_path);
                    if let Ok(ft) = entry.file_type() {
                        let fuse_ft = if ft.is_dir() {
                            FileType::Directory
                        } else if ft.is_symlink() {
                            FileType::Symlink
                        } else {
                            FileType::RegularFile
                        };
                        full_entries.push((child_ino, fuse_ft, name));
                    }
                }
            }
        }

        for (i, (entry_ino, ft, name)) in full_entries.iter().enumerate().skip(offset as usize) {
            if reply.add(*entry_ino, (i + 1) as i64, *ft, name) {
                break;
            }
        }
        reply.ok();
    }

    /// Open a file.
    /// Port of C tup's tup_fs_open() (fuse_fs.c:1260-1317).
    fn open(&mut self, req: &Request<'_>, ino: u64, flags: i32, reply: ReplyOpen) {
        if !self.context_check(req) {
            reply.error(libc::EPERM);
            return;
        }
        let path = match self.inode_path(ino) {
            Some(p) => p,
            None => {
                reply.error(libc::ENOENT);
                return;
            }
        };

        // Determine access type from flags
        let at = if (flags & libc::O_RDWR != 0) || (flags & libc::O_WRONLY != 0) {
            tup_types::AccessType::Write
        } else {
            tup_types::AccessType::Read
        };

        // Check job mappings — open the tmpname if mapped
        let open_path = if let Some((_job_id, finfo_arc, peeled)) = self.get_finfo_and_peeled(&path)
        {
            if let Ok(mut finfo) = finfo_arc.lock() {
                let mapped_path = if let Some(mapping) = finfo.find_mapping(&peeled) {
                    Some(mapping.tmpname.clone())
                } else if at == tup_types::AccessType::Write && !Self::is_hidden(&peeled) {
                    // Write to unmapped file — create a mapping (C: FUSE3 path, lines 1285-1295)
                    let tmpname = finfo.add_mapping(&format!("/{peeled}"), &self.tup_top);
                    Some(tmpname)
                } else {
                    None
                };

                finfo.open_count += 1;

                if let Some(p) = mapped_path {
                    p
                } else {
                    self.resolve_real_path(&path)
                }
            } else {
                reply.error(libc::EPERM);
                return;
            }
        } else {
            reply.error(libc::EPERM);
            return;
        };

        self.record_access(ino, at);

        // C: fd = openat(tup_top_fd(), openfile, fi->flags)
        match libc_open(&open_path, flags & !libc::O_NOFOLLOW, 0) {
            Ok(fd) => {
                // C: If open_count >= max_open_files, close fd and set fh=0
                // (triggers fallback open in read/write) (fuse_fs.c:1302-1307)
                if let Some((_jid, finfo_arc, _p)) = self.get_finfo_and_peeled(&path) {
                    if let Ok(finfo) = finfo_arc.lock() {
                        if finfo.open_count >= self.max_open_files {
                            unsafe { libc::close(fd) };
                            reply.opened(0, 0);
                            return;
                        }
                    }
                }
                reply.opened(fd as u64, 0);
            }
            Err(e) => reply.error(e),
        }
    }

    /// Read data from a file.
    /// Port of C tup's tup_fs_read() (fuse_fs.c:1319-1356).
    fn read(
        &mut self,
        _req: &Request<'_>,
        ino: u64,
        fh: u64,
        offset: i64,
        size: u32,
        _flags: i32,
        _lock_owner: Option<u64>,
        reply: ReplyData,
    ) {
        let fd = if fh > 0 {
            fh as i32
        } else {
            // Fallback: fh==0 means we ran out of FDs — open on the fly (C: lines 1325-1345)
            let path = match self.inode_path(ino) {
                Some(p) => p,
                None => {
                    reply.error(libc::EBADF);
                    return;
                }
            };
            let open_path =
                if let Some((_jid, finfo_arc, peeled)) = self.get_finfo_and_peeled(&path) {
                    if let Ok(finfo) = finfo_arc.lock() {
                        if let Some(mapping) = finfo.find_mapping(&peeled) {
                            mapping.tmpname.clone()
                        } else {
                            self.resolve_real_path(&path)
                        }
                    } else {
                        self.resolve_real_path(&path)
                    }
                } else {
                    self.resolve_real_path(&path)
                };
            match libc_open(&open_path, libc::O_RDONLY, 0) {
                Ok(fd) => {
                    // Read and close immediately
                    let mut buf = vec![0u8; size as usize];
                    let n = unsafe {
                        libc::pread(
                            fd,
                            buf.as_mut_ptr() as *mut libc::c_void,
                            size as usize,
                            offset,
                        )
                    };
                    unsafe { libc::close(fd) };
                    if n < 0 {
                        reply.error(
                            std::io::Error::last_os_error()
                                .raw_os_error()
                                .unwrap_or(libc::EIO),
                        );
                    } else {
                        reply.data(&buf[..n as usize]);
                    }
                    return;
                }
                Err(e) => {
                    reply.error(e);
                    return;
                }
            }
        };

        let mut buf = vec![0u8; size as usize];
        let n = unsafe {
            libc::pread(
                fd,
                buf.as_mut_ptr() as *mut libc::c_void,
                size as usize,
                offset,
            )
        };
        if n < 0 {
            reply.error(
                std::io::Error::last_os_error()
                    .raw_os_error()
                    .unwrap_or(libc::EIO),
            );
        } else {
            reply.data(&buf[..n as usize]);
        }
    }

    /// Release (close) a file.
    /// Port of C tup's tup_fs_release() (fuse_fs.c:1456-1471).
    fn release(
        &mut self,
        _req: &Request<'_>,
        ino: u64,
        fh: u64,
        _flags: i32,
        _lock_owner: Option<u64>,
        _flush: bool,
        reply: fuser::ReplyEmpty,
    ) {
        if fh > 0 {
            unsafe { libc::close(fh as i32) };
        }

        // C: finfo->open_count--; pthread_cond_signal(&finfo->cond);
        let path = self.inode_path(ino);
        if let Some(path) = path {
            if let Some((_job_id, finfo_arc, _peeled)) = self.get_finfo_and_peeled(&path) {
                if let Ok(mut finfo) = finfo_arc.lock() {
                    if finfo.open_count > 0 {
                        finfo.open_count -= 1;
                    }
                }
            }
        }
        reply.ok();
    }

    /// Write data to a file.
    /// Port of C tup's tup_fs_write() (fuse_fs.c:1358-1394).
    fn write(
        &mut self,
        _req: &Request<'_>,
        ino: u64,
        fh: u64,
        offset: i64,
        data: &[u8],
        _write_flags: u32,
        _flags: i32,
        _lock_owner: Option<u64>,
        reply: ReplyWrite,
    ) {
        let fd = if fh > 0 {
            fh as i32
        } else {
            // Fallback: fh==0 — open mapped tmpname on the fly (C: lines 1364-1393)
            let path = match self.inode_path(ino) {
                Some(p) => p,
                None => {
                    reply.error(libc::EBADF);
                    return;
                }
            };
            if let Some((_jid, finfo_arc, peeled)) = self.get_finfo_and_peeled(&path) {
                if let Ok(finfo) = finfo_arc.lock() {
                    if let Some(mapping) = finfo.find_mapping(&peeled) {
                        match libc_open(&mapping.tmpname, libc::O_WRONLY, 0) {
                            Ok(fd) => {
                                let n = unsafe {
                                    libc::pwrite(
                                        fd,
                                        data.as_ptr() as *const libc::c_void,
                                        data.len(),
                                        offset,
                                    )
                                };
                                unsafe { libc::close(fd) };
                                if n < 0 {
                                    reply.error(
                                        std::io::Error::last_os_error()
                                            .raw_os_error()
                                            .unwrap_or(libc::EIO),
                                    );
                                } else {
                                    reply.written(n as u32);
                                }
                                return;
                            }
                            Err(e) => {
                                reply.error(e);
                                return;
                            }
                        }
                    }
                }
            }
            reply.error(libc::EPERM);
            return;
        };

        let n =
            unsafe { libc::pwrite(fd, data.as_ptr() as *const libc::c_void, data.len(), offset) };
        if n < 0 {
            reply.error(
                std::io::Error::last_os_error()
                    .raw_os_error()
                    .unwrap_or(libc::EIO),
            );
        } else {
            reply.written(n as u32);
        }
    }

    /// Create and open a file.
    /// Port of C tup's tup_fs_create() (fuse_fs.c:1239-1258).
    /// Creates a mapping to .tup/tmp/ and opens the temp file.
    fn create(
        &mut self,
        req: &Request<'_>,
        parent: u64,
        name: &OsStr,
        mode: u32,
        _umask: u32,
        flags: i32,
        reply: ReplyCreate,
    ) {
        if !self.context_check(req) {
            reply.error(libc::EPERM);
            return;
        }
        let parent_path = match self.inode_path(parent) {
            Some(p) => p,
            None => {
                reply.error(libc::ENOENT);
                return;
            }
        };

        let child_path = parent_path.join(name);

        // Create mapping and open the tmpfile
        if let Some((_job_id, finfo_arc, peeled)) = self.get_finfo_and_peeled(&child_path) {
            let tmpname = {
                let mut finfo = match finfo_arc.lock() {
                    Ok(f) => f,
                    Err(_) => {
                        reply.error(libc::EIO);
                        return;
                    }
                };
                let tmpname = finfo.add_mapping(&format!("/{peeled}"), &self.tup_top);
                finfo.open_count += 1;
                tmpname
            };

            // C: mknod_internal → openat(tup_top_fd(), map->tmpname, flags, mode)
            match libc_open(
                &tmpname,
                flags | libc::O_CREAT | libc::O_TRUNC,
                mode as libc::mode_t,
            ) {
                Ok(fd) => {
                    let ino = self.get_or_create_inode(&child_path);
                    if let Ok(meta) = std::fs::symlink_metadata(&tmpname) {
                        let attr = metadata_to_attr(ino, &meta);
                        // C: If open_count >= max_open_files, close fd, set fh=0
                        // (fuse_fs.c:1248-1253)
                        let fh = if let Ok(finfo) = finfo_arc.lock() {
                            if finfo.open_count >= self.max_open_files {
                                unsafe { libc::close(fd) };
                                0u64
                            } else {
                                fd as u64
                            }
                        } else {
                            fd as u64
                        };
                        reply.created(&TTL, &attr, 0, fh, 0);
                    } else {
                        unsafe { libc::close(fd) };
                        reply.error(libc::EIO);
                    }
                }
                Err(e) => reply.error(e),
            }
        } else {
            reply.error(libc::EPERM);
        }
    }

    /// Create a file node.
    /// Port of C tup's mknod_internal() (fuse_fs.c:725-800).
    /// Creates a mapping to .tup/tmp/ and creates the temp file.
    fn mknod(
        &mut self,
        req: &Request<'_>,
        parent: u64,
        name: &OsStr,
        mode: u32,
        _umask: u32,
        _rdev: u32,
        reply: ReplyEntry,
    ) {
        if !self.context_check(req) {
            reply.error(libc::EPERM);
            return;
        }
        let parent_path = match self.inode_path(parent) {
            Some(p) => p,
            None => {
                reply.error(libc::ENOENT);
                return;
            }
        };

        let child_path = parent_path.join(name);

        // C: Only regular files, FIFOs, and sockets are allowed.
        let file_mode = mode & libc::S_IFMT as u32;
        if file_mode != libc::S_IFREG as u32
            && file_mode != libc::S_IFIFO as u32
            && file_mode != libc::S_IFSOCK as u32
        {
            eprintln!(
                "tup error: mknod() with mode 0x{:x} is not permitted.",
                mode
            );
            reply.error(libc::EPERM);
            return;
        }

        if let Some((_job_id, finfo_arc, peeled)) = self.get_finfo_and_peeled(&child_path) {
            let tmpname = {
                let mut finfo = match finfo_arc.lock() {
                    Ok(f) => f,
                    Err(_) => {
                        reply.error(libc::EIO);
                        return;
                    }
                };
                finfo.add_mapping(&format!("/{peeled}"), &self.tup_top)
            };

            // Create the temp file and close it (mknod doesn't keep it open)
            match libc_open(
                &tmpname,
                libc::O_CREAT | libc::O_EXCL | libc::O_WRONLY,
                mode as libc::mode_t,
            ) {
                Ok(fd) => {
                    unsafe { libc::close(fd) };
                    let ino = self.get_or_create_inode(&child_path);
                    if let Ok(meta) = std::fs::symlink_metadata(&tmpname) {
                        reply.entry(&TTL, &metadata_to_attr(ino, &meta), 0);
                    } else {
                        reply.error(libc::EIO);
                    }
                }
                Err(e) => reply.error(e),
            }
        } else {
            reply.error(libc::EPERM);
        }
    }

    /// Create a directory.
    /// Port of C tup's tup_fs_mkdir() (fuse_fs.c:808-854).
    /// Directories are virtual — tracked in FileInfo::tmpdirs, not created on real filesystem.
    fn mkdir(
        &mut self,
        req: &Request<'_>,
        parent: u64,
        name: &OsStr,
        _mode: u32,
        _umask: u32,
        reply: ReplyEntry,
    ) {
        if !self.context_check(req) {
            reply.error(libc::EPERM);
            return;
        }
        let parent_path = match self.inode_path(parent) {
            Some(p) => p,
            None => {
                reply.error(libc::ENOENT);
                return;
            }
        };

        let child_path = parent_path.join(name);

        if let Some((_job_id, finfo_arc, peeled)) = self.get_finfo_and_peeled(&child_path) {
            // C: For ignored paths (ccache, /dev, /proc, /sys), do real mkdir
            if Self::should_ignore(&format!("/{peeled}")) {
                let real_child = self.resolve_real_path(&child_path);
                let _ = std::fs::create_dir_all(&real_child);
            } else {
                // Virtual directory — add to tmpdir_list (C: fuse_fs.c:833-851)
                if let Ok(mut finfo) = finfo_arc.lock() {
                    finfo.tmpdirs.push(peeled);
                }
            }
            let ino = self.get_or_create_inode(&child_path);
            reply.entry(&TTL, &synthetic_dir_attr(ino), 0);
        } else {
            reply.error(libc::EPERM);
        }
    }

    /// Remove a file.
    /// Port of C tup's tup_fs_unlink() (fuse_fs.c:856-890).
    /// Only allows unlinking mapped files (created during this job).
    fn unlink(&mut self, req: &Request<'_>, parent: u64, name: &OsStr, reply: fuser::ReplyEmpty) {
        if !self.context_check(req) {
            reply.error(libc::EPERM);
            return;
        }
        let parent_path = match self.inode_path(parent) {
            Some(p) => p,
            None => {
                reply.error(libc::ENOENT);
                return;
            }
        };

        let child_path = parent_path.join(name);

        if let Some((_job_id, finfo_arc, peeled)) = self.get_finfo_and_peeled(&child_path) {
            if let Ok(mut finfo) = finfo_arc.lock() {
                // Check if this file has a mapping (was created during this job)
                if let Some(mapping) = finfo.mappings.remove(&peeled) {
                    // Delete the tmpfile
                    let _ = std::fs::remove_file(&mapping.tmpname);
                    drop(finfo);
                    // Record unlink access
                    let ino = self.get_or_create_inode(&child_path);
                    self.record_access(ino, tup_types::AccessType::Unlink);
                    reply.ok();
                    return;
                }
            }

            // C: .fuse_hidden workaround (fuse_fs.c:876-887)
            let name_str = name.to_string_lossy();
            if name_str.contains(".fuse_hidden") {
                let real_child = self.resolve_real_path(&child_path);
                let _ = std::fs::remove_file(&real_child);
                reply.ok();
                return;
            }

            eprintln!(
                "tup error: Unable to unlink files not created during this job: {}",
                Self::peel(&child_path.to_string_lossy())
            );
            reply.error(libc::EPERM);
        } else {
            reply.error(libc::EPERM);
        }
    }

    /// Remove a directory.
    /// Port of C tup's tup_fs_rmdir() (fuse_fs.c:892-935).
    /// Only allows removing virtual tmpdirs.
    fn rmdir(&mut self, req: &Request<'_>, parent: u64, name: &OsStr, reply: fuser::ReplyEmpty) {
        if !self.context_check(req) {
            reply.error(libc::EPERM);
            return;
        }
        let parent_path = match self.inode_path(parent) {
            Some(p) => p,
            None => {
                reply.error(libc::ENOENT);
                return;
            }
        };

        let child_path = parent_path.join(name);

        if let Some((_job_id, finfo_arc, peeled)) = self.get_finfo_and_peeled(&child_path) {
            if let Ok(mut finfo) = finfo_arc.lock() {
                // C: Check for subdirectories (ENOTEMPTY)
                let has_subdir = finfo.tmpdirs.iter().any(|td| {
                    td.starts_with(&peeled)
                        && td.len() > peeled.len()
                        && td.as_bytes()[peeled.len()] == b'/'
                });
                if has_subdir {
                    reply.error(libc::ENOTEMPTY);
                    return;
                }
                // C: Check for files in the directory (ENOTEMPTY)
                let has_files = finfo.mappings.keys().any(|rn| {
                    rn.starts_with(&peeled)
                        && rn.len() > peeled.len()
                        && rn.as_bytes()[peeled.len()] == b'/'
                });
                if has_files {
                    reply.error(libc::ENOTEMPTY);
                    return;
                }

                // Remove the tmpdir entry
                if let Some(pos) = finfo.tmpdirs.iter().position(|td| td == &peeled) {
                    finfo.tmpdirs.remove(pos);
                    reply.ok();
                    return;
                }
            }
            eprintln!(
                "tup error: Unable to rmdir a directory not created during this job: {}",
                Self::peel(&child_path.to_string_lossy())
            );
            reply.error(libc::EPERM);
        } else {
            reply.error(libc::EPERM);
        }
    }

    /// Create a symbolic link.
    /// Port of C tup's tup_fs_symlink() (fuse_fs.c:937-955).
    /// Creates a mapping and the symlink at the tmpname.
    fn symlink(
        &mut self,
        req: &Request<'_>,
        parent: u64,
        link_name: &OsStr,
        target: &Path,
        reply: ReplyEntry,
    ) {
        if !self.context_check(req) {
            reply.error(libc::EPERM);
            return;
        }
        let parent_path = match self.inode_path(parent) {
            Some(p) => p,
            None => {
                reply.error(libc::ENOENT);
                return;
            }
        };

        let link_path = parent_path.join(link_name);

        if let Some((_job_id, finfo_arc, peeled)) = self.get_finfo_and_peeled(&link_path) {
            let tmpname = {
                let mut finfo = match finfo_arc.lock() {
                    Ok(f) => f,
                    Err(_) => {
                        reply.error(libc::EIO);
                        return;
                    }
                };
                finfo.add_mapping(&format!("/{peeled}"), &self.tup_top)
            };

            // C: symlinkat(from, tup_top_fd(), tomap->tmpname)
            #[cfg(unix)]
            match std::os::unix::fs::symlink(target, &tmpname) {
                Ok(_) => {
                    let ino = self.get_or_create_inode(&link_path);
                    if let Ok(meta) = std::fs::symlink_metadata(&tmpname) {
                        reply.entry(&TTL, &metadata_to_attr(ino, &meta), 0);
                    } else {
                        reply.error(libc::EIO);
                    }
                }
                Err(e) => reply.error(e.raw_os_error().unwrap_or(libc::EIO)),
            }
        } else {
            reply.error(libc::EPERM);
        }
    }

    /// Read a symbolic link target.
    /// Port of C tup's tup_fs_readlink() (fuse_fs.c:497-539).
    fn readlink(&mut self, req: &Request<'_>, ino: u64, reply: fuser::ReplyData) {
        if !self.context_check(req) {
            reply.error(libc::EPERM);
            return;
        }
        let path = match self.inode_path(ino) {
            Some(p) => p,
            None => {
                reply.error(libc::ENOENT);
                return;
            }
        };

        // Check mappings — readlink the tmpname if mapped
        let read_path = if let Some((_jid, finfo_arc, peeled)) = self.get_finfo_and_peeled(&path) {
            if let Ok(finfo) = finfo_arc.lock() {
                if let Some(mapping) = finfo.find_mapping(&peeled) {
                    mapping.tmpname.clone()
                } else {
                    self.resolve_real_path(&path)
                }
            } else {
                self.resolve_real_path(&path)
            }
        } else {
            self.resolve_real_path(&path)
        };

        // Record read access
        self.record_access(ino, tup_types::AccessType::Read);

        match std::fs::read_link(&read_path) {
            Ok(target) => {
                reply.data(target.to_string_lossy().as_bytes());
            }
            Err(e) => reply.error(e.raw_os_error().unwrap_or(libc::EIO)),
        }
    }

    /// Rename a file.
    /// Port of C tup's tup_fs_rename() (fuse_fs.c:957-1039).
    /// Works with mappings: renames the mapping's realname.
    fn rename(
        &mut self,
        req: &Request<'_>,
        parent: u64,
        name: &OsStr,
        newparent: u64,
        newname: &OsStr,
        _flags: u32,
        reply: fuser::ReplyEmpty,
    ) {
        if !self.context_check(req) {
            reply.error(libc::EPERM);
            return;
        }
        let old_parent = match self.inode_path(parent) {
            Some(p) => p,
            None => {
                reply.error(libc::ENOENT);
                return;
            }
        };
        let new_parent = match self.inode_path(newparent) {
            Some(p) => p,
            None => {
                reply.error(libc::ENOENT);
                return;
            }
        };

        let old_path = old_parent.join(name);
        let new_path = new_parent.join(newname);

        if let Some((_job_id, finfo_arc, peeled_from)) = self.get_finfo_and_peeled(&old_path) {
            let full_new = format!("/{}", new_path.to_string_lossy().trim_start_matches('/'));
            let peeled_to = Self::peel(&full_new).trim_start_matches('/').to_string();

            if let Ok(mut finfo) = finfo_arc.lock() {
                // C: Check if renaming a tmpdir — just update the dirname
                if let Some(pos) = finfo.tmpdirs.iter().position(|td| td == &peeled_from) {
                    finfo.tmpdirs[pos] = peeled_to;
                    reply.ok();
                    return;
                }

                // C: If destination already has a mapping, delete the old tmpfile
                if let Some(old_dest_mapping) = finfo.mappings.remove(&peeled_to) {
                    let _ = std::fs::remove_file(&old_dest_mapping.tmpname);
                }

                // C: Look up source mapping
                if let Some(mut src_mapping) = finfo.mappings.remove(&peeled_from) {
                    let newname_str = newname.to_string_lossy();
                    if newname_str.contains(".fuse_hidden") {
                        // C: Treat as unlink (fuse_fs.c:1013-1023)
                        let _ = std::fs::remove_file(&src_mapping.tmpname);
                        drop(finfo);
                        let ino = self.get_or_create_inode(&old_path);
                        self.record_access(ino, tup_types::AccessType::Unlink);
                    } else {
                        // Update the mapping's realname
                        src_mapping.realname = peeled_to.clone();
                        finfo.handle_rename(&peeled_from, &peeled_to);
                        finfo.mappings.insert(peeled_to, src_mapping);
                    }
                    reply.ok();
                    return;
                }

                // Not mapped — error
                reply.error(libc::ENOENT);
                return;
            }
        }

        // Update inode mapping
        if let Some(ino) = self.path_to_inode.write().unwrap().remove(&old_path) {
            self.inodes.write().unwrap().insert(ino, new_path.clone());
            self.path_to_inode.write().unwrap().insert(new_path, ino);
        }
        reply.ok();
    }

    /// Hard links are not supported.
    /// Port of C tup's tup_fs_link() (fuse_fs.c:1041-1047).
    fn link(
        &mut self,
        req: &Request<'_>,
        _ino: u64,
        _newparent: u64,
        _newname: &OsStr,
        reply: ReplyEntry,
    ) {
        if !self.context_check(req) {
            reply.error(libc::EPERM);
            return;
        }
        eprintln!("tup error: hard links are not supported.");
        reply.error(libc::EPERM);
    }
}

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
        // C: handle_rename() renames write_list/read_list entries in-place
        let mut finfo = FileInfo::new();
        finfo.handle_open_file(AccessType::Write, "old.txt");
        finfo.handle_open_file(AccessType::Read, "old.txt");
        finfo.handle_rename("old.txt", "new.txt");
        // write_list and read_list entries should be renamed
        assert_eq!(finfo.write_list, vec!["new.txt"]);
        assert_eq!(finfo.read_list, vec!["new.txt"]);
        // unlink_list should have "new.txt" removed (check_unlink_list)
        assert!(finfo.unlink_list.is_empty());
    }

    #[test]
    fn test_handle_file_dispatches() {
        // C: handle_file() dispatches to handle_open_file or handle_rename
        let mut finfo = FileInfo::new();
        finfo.handle_file(AccessType::Read, "input.c", "");
        finfo.handle_file(AccessType::Write, "output.o", "");
        // For rename: first the file must exist in a list
        finfo.handle_open_file(AccessType::Write, "old.txt");
        finfo.handle_file(AccessType::Rename, "old.txt", "new.txt");
        assert_eq!(finfo.read_list, vec!["input.c"]);
        assert_eq!(finfo.write_list, vec!["output.o", "new.txt"]);
    }

    #[test]
    fn test_handle_unlink_processing() {
        // C: handle_unlink() for each unlinked file, removes from write_list AND read_list
        let mut finfo = FileInfo::new();
        finfo.handle_open_file(AccessType::Write, "keep.o");
        finfo.handle_open_file(AccessType::Read, "deleted.c");
        finfo.handle_open_file(AccessType::Write, "deleted.c");
        finfo.handle_open_file(AccessType::Unlink, "deleted.c");
        finfo.handle_unlink();
        // "deleted.c" should be removed from both write_list and read_list
        assert_eq!(finfo.write_list, vec!["keep.o"]);
        assert!(finfo.read_list.is_empty());
        assert!(finfo.unlink_list.is_empty());
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

    #[test]
    fn test_record_access_with_job() {
        // Verify that record_access routes to the correct job's FileInfo
        let tmp = tempfile::tempdir().unwrap();
        let fs = TupFuseFs::new(tmp.path());

        // Register a job
        let finfo = std::sync::Arc::new(std::sync::Mutex::new(FileInfo::new()));
        fs.add_job(42, finfo.clone());

        // Create a path that looks like @tupjob-42/src/foo.c
        let job_path = tmp.path().join("@tupjob-42").join("src").join("foo.c");
        let ino = fs.get_or_create_inode(&job_path);

        // Record a read access
        fs.record_access(ino, tup_types::AccessType::Read);

        // Verify it was recorded in the job's FileInfo
        let fi = finfo.lock().unwrap();
        assert_eq!(fi.read_list.len(), 1);
        assert!(fi.read_list[0].contains("foo.c"));
    }

    #[test]
    fn test_record_access_hidden_skipped() {
        let tmp = tempfile::tempdir().unwrap();
        let fs = TupFuseFs::new(tmp.path());

        let finfo = std::sync::Arc::new(std::sync::Mutex::new(FileInfo::new()));
        fs.add_job(1, finfo.clone());

        // Access to .git should be skipped
        let git_path = tmp.path().join("@tupjob-1").join(".git").join("config");
        let ino = fs.get_or_create_inode(&git_path);
        fs.record_access(ino, tup_types::AccessType::Read);

        let fi = finfo.lock().unwrap();
        assert!(fi.read_list.is_empty()); // .git is hidden
    }
}
