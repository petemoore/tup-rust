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
    pub fn new(tup_top: &Path) -> Self {
        let pgid = unsafe { libc::getpgid(0) } as u32;
        let mut inodes = BTreeMap::new();
        let mut path_to_inode = BTreeMap::new();
        // Inode 1 = FUSE root (= tup_top directory)
        inodes.insert(1, tup_top.to_path_buf());
        path_to_inode.insert(tup_top.to_path_buf(), 1);
        TupFuseFs {
            tup_top: tup_top.to_path_buf(),
            jobs: Arc::new(RwLock::new(BTreeMap::new())),
            ourpgid: pgid,
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
    fn lookup(&mut self, _req: &Request<'_>, parent: u64, name: &OsStr, reply: ReplyEntry) {
        let parent_path = match self.inode_path(parent) {
            Some(p) => p,
            None => {
                reply.error(libc::ENOENT);
                return;
            }
        };

        let child_path = parent_path.join(name);
        match std::fs::symlink_metadata(&child_path) {
            Ok(meta) => {
                let ino = self.get_or_create_inode(&child_path);
                let attr = metadata_to_attr(ino, &meta);
                reply.entry(&TTL, &attr, 0);
            }
            Err(_) => {
                reply.error(libc::ENOENT);
            }
        }
    }

    /// Get file attributes.
    /// Port of C tup's tup_fs_getattr() (fuse_fs.c:344-435).
    fn getattr(&mut self, _req: &Request<'_>, ino: u64, reply: ReplyAttr) {
        let path = match self.inode_path(ino) {
            Some(p) => p,
            None => {
                reply.error(libc::ENOENT);
                return;
            }
        };

        match std::fs::symlink_metadata(&path) {
            Ok(meta) => {
                let attr = metadata_to_attr(ino, &meta);
                reply.attr(&TTL, &attr);
            }
            Err(_) => {
                reply.error(libc::ENOENT);
            }
        }
    }

    /// Read directory entries.
    /// Port of C tup's tup_fs_readdir() (fuse_fs.c:588-724).
    fn readdir(
        &mut self,
        _req: &Request<'_>,
        ino: u64,
        _fh: u64,
        offset: i64,
        mut reply: ReplyDirectory,
    ) {
        let dir_path = match self.inode_path(ino) {
            Some(p) => p,
            None => {
                reply.error(libc::ENOENT);
                return;
            }
        };

        let entries = match std::fs::read_dir(&dir_path) {
            Ok(rd) => rd,
            Err(_) => {
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

        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
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

        for (i, (entry_ino, ft, name)) in full_entries.iter().enumerate().skip(offset as usize) {
            if reply.add(*entry_ino, (i + 1) as i64, *ft, name) {
                break; // buffer full
            }
        }
        reply.ok();
    }

    /// Open a file.
    /// Port of C tup's tup_fs_open() (fuse_fs.c).
    fn open(&mut self, _req: &Request<'_>, ino: u64, _flags: i32, reply: ReplyOpen) {
        let path = match self.inode_path(ino) {
            Some(p) => p,
            None => {
                reply.error(libc::ENOENT);
                return;
            }
        };

        // Track the read access
        let rel_path = path
            .strip_prefix(&self.tup_top)
            .unwrap_or(&path)
            .to_string_lossy()
            .to_string();
        if !Self::is_hidden(&rel_path) && !Self::should_ignore(&rel_path) {
            // TODO: Record access in the job's FileInfo when FUSE is
            // wired into the updater execution path (WP6)
        }

        // Return FH=0, let kernel handle the actual FD
        reply.opened(0, 0);
    }

    /// Read data from a file.
    /// Port of C tup's tup_fs_read() (fuse_fs.c).
    fn read(
        &mut self,
        _req: &Request<'_>,
        ino: u64,
        _fh: u64,
        offset: i64,
        size: u32,
        _flags: i32,
        _lock_owner: Option<u64>,
        reply: ReplyData,
    ) {
        let path = match self.inode_path(ino) {
            Some(p) => p,
            None => {
                reply.error(libc::ENOENT);
                return;
            }
        };

        match std::fs::read(&path) {
            Ok(data) => {
                let start = offset as usize;
                let end = std::cmp::min(start + size as usize, data.len());
                if start < data.len() {
                    reply.data(&data[start..end]);
                } else {
                    reply.data(&[]);
                }
            }
            Err(_) => {
                reply.error(libc::EIO);
            }
        }
    }

    /// Release (close) a file.
    /// Port of C tup's tup_fs_release() (fuse_fs.c).
    fn release(
        &mut self,
        _req: &Request<'_>,
        _ino: u64,
        _fh: u64,
        _flags: i32,
        _lock_owner: Option<u64>,
        _flush: bool,
        reply: fuser::ReplyEmpty,
    ) {
        reply.ok();
    }

    /// Write data to a file.
    /// Port of C tup's tup_fs_write() (fuse_fs.c).
    /// Writes go to the mapped temporary file, not the real output.
    fn write(
        &mut self,
        _req: &Request<'_>,
        ino: u64,
        _fh: u64,
        offset: i64,
        data: &[u8],
        _write_flags: u32,
        _flags: i32,
        _lock_owner: Option<u64>,
        reply: ReplyWrite,
    ) {
        let path = match self.inode_path(ino) {
            Some(p) => p,
            None => {
                reply.error(libc::ENOENT);
                return;
            }
        };

        // TODO: When wired to updater (WP6), write to mapped tmpname
        // For now, write to the real file
        use std::io::Write;
        match std::fs::OpenOptions::new().write(true).open(&path) {
            Ok(mut f) => {
                use std::io::Seek;
                if f.seek(std::io::SeekFrom::Start(offset as u64)).is_err() {
                    reply.error(libc::EIO);
                    return;
                }
                match f.write(data) {
                    Ok(n) => reply.written(n as u32),
                    Err(_) => reply.error(libc::EIO),
                }
            }
            Err(_) => reply.error(libc::EIO),
        }
    }

    /// Create and open a file.
    /// Port of C tup's tup_fs_create() (fuse_fs.c:1239-1258).
    /// Creates the file (via mknod_internal logic) then opens it.
    fn create(
        &mut self,
        _req: &Request<'_>,
        parent: u64,
        name: &OsStr,
        _mode: u32,
        _umask: u32,
        _flags: i32,
        reply: ReplyCreate,
    ) {
        let parent_path = match self.inode_path(parent) {
            Some(p) => p,
            None => {
                reply.error(libc::ENOENT);
                return;
            }
        };

        let child_path = parent_path.join(name);

        // Create the file (C: mknod_internal with O_CREAT | O_WRONLY | O_TRUNC)
        match std::fs::File::create(&child_path) {
            Ok(_) => {
                let ino = self.get_or_create_inode(&child_path);
                match std::fs::symlink_metadata(&child_path) {
                    Ok(meta) => {
                        let attr = metadata_to_attr(ino, &meta);
                        reply.created(&TTL, &attr, 0, 0, 0);
                    }
                    Err(_) => reply.error(libc::EIO),
                }
            }
            Err(e) => reply.error(e.raw_os_error().unwrap_or(libc::EIO)),
        }
    }

    /// Create a file node.
    /// Port of C tup's mknod_internal() (fuse_fs.c:725-800).
    fn mknod(
        &mut self,
        _req: &Request<'_>,
        parent: u64,
        name: &OsStr,
        mode: u32,
        _umask: u32,
        _rdev: u32,
        reply: ReplyEntry,
    ) {
        let parent_path = match self.inode_path(parent) {
            Some(p) => p,
            None => {
                reply.error(libc::ENOENT);
                return;
            }
        };

        let child_path = parent_path.join(name);

        // C: Only regular files, FIFOs, and sockets are allowed.
        // Device nodes are rejected with EPERM.
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

        match std::fs::File::create(&child_path) {
            Ok(_) => {
                let ino = self.get_or_create_inode(&child_path);
                match std::fs::symlink_metadata(&child_path) {
                    Ok(meta) => {
                        let attr = metadata_to_attr(ino, &meta);
                        reply.entry(&TTL, &attr, 0);
                    }
                    Err(_) => reply.error(libc::EIO),
                }
            }
            Err(e) => reply.error(e.raw_os_error().unwrap_or(libc::EIO)),
        }
    }

    /// Create a directory.
    /// Port of C tup's tup_fs_mkdir() (fuse_fs.c:808-854).
    /// In tup's FUSE, directories are virtual — tracked in tmpdir_list,
    /// not actually created on the real filesystem.
    fn mkdir(
        &mut self,
        _req: &Request<'_>,
        parent: u64,
        name: &OsStr,
        mode: u32,
        _umask: u32,
        reply: ReplyEntry,
    ) {
        let parent_path = match self.inode_path(parent) {
            Some(p) => p,
            None => {
                reply.error(libc::ENOENT);
                return;
            }
        };

        let child_path = parent_path.join(name);

        // C: For ignored paths (ccache etc), do real mkdir
        let rel = child_path
            .strip_prefix(&self.tup_top)
            .unwrap_or(&child_path)
            .to_string_lossy();
        if Self::should_ignore(&format!("/{rel}")) {
            match std::fs::create_dir_all(&child_path) {
                Ok(_) => {}
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(e) => {
                    reply.error(e.raw_os_error().unwrap_or(libc::EIO));
                    return;
                }
            }
        }
        // else: C tracks in tmpdir_list (virtual dir)
        // TODO: Wire to FileInfo tmpdir_list when integrated with updater (WP6)

        // Create real dir for now (will be virtual when FUSE is fully wired)
        let _ = std::fs::create_dir_all(&child_path);
        let ino = self.get_or_create_inode(&child_path);
        let attr = FileAttr {
            ino,
            size: 0,
            blocks: 0,
            atime: SystemTime::now(),
            mtime: SystemTime::now(),
            ctime: SystemTime::now(),
            crtime: SystemTime::now(),
            kind: FileType::Directory,
            perm: (mode & 0o7777) as u16,
            nlink: 2,
            uid: unsafe { libc::getuid() },
            gid: unsafe { libc::getgid() },
            rdev: 0,
            blksize: 4096,
            flags: 0,
        };
        reply.entry(&TTL, &attr, 0);
    }

    /// Remove a file.
    /// Port of C tup's tup_fs_unlink() (fuse_fs.c:856-890).
    fn unlink(&mut self, _req: &Request<'_>, parent: u64, name: &OsStr, reply: fuser::ReplyEmpty) {
        let parent_path = match self.inode_path(parent) {
            Some(p) => p,
            None => {
                reply.error(libc::ENOENT);
                return;
            }
        };

        let child_path = parent_path.join(name);

        match std::fs::remove_file(&child_path) {
            Ok(_) => reply.ok(),
            Err(e) => reply.error(e.raw_os_error().unwrap_or(libc::EIO)),
        }
    }

    /// Remove a directory.
    /// Port of C tup's tup_fs_rmdir() (fuse_fs.c:892-935).
    fn rmdir(&mut self, _req: &Request<'_>, parent: u64, name: &OsStr, reply: fuser::ReplyEmpty) {
        let parent_path = match self.inode_path(parent) {
            Some(p) => p,
            None => {
                reply.error(libc::ENOENT);
                return;
            }
        };

        let child_path = parent_path.join(name);

        match std::fs::remove_dir(&child_path) {
            Ok(_) => reply.ok(),
            Err(e) => reply.error(e.raw_os_error().unwrap_or(libc::EIO)),
        }
    }

    /// Create a symbolic link.
    /// Port of C tup's tup_fs_symlink() (fuse_fs.c:937-955).
    fn symlink(
        &mut self,
        _req: &Request<'_>,
        parent: u64,
        link_name: &OsStr,
        target: &Path,
        reply: ReplyEntry,
    ) {
        let parent_path = match self.inode_path(parent) {
            Some(p) => p,
            None => {
                reply.error(libc::ENOENT);
                return;
            }
        };

        let link_path = parent_path.join(link_name);

        #[cfg(unix)]
        match std::os::unix::fs::symlink(target, &link_path) {
            Ok(_) => {
                let ino = self.get_or_create_inode(&link_path);
                match std::fs::symlink_metadata(&link_path) {
                    Ok(meta) => {
                        let attr = metadata_to_attr(ino, &meta);
                        reply.entry(&TTL, &attr, 0);
                    }
                    Err(_) => reply.error(libc::EIO),
                }
            }
            Err(e) => reply.error(e.raw_os_error().unwrap_or(libc::EIO)),
        }
    }

    /// Rename a file.
    /// Port of C tup's tup_fs_rename() (fuse_fs.c:957-1039).
    fn rename(
        &mut self,
        _req: &Request<'_>,
        parent: u64,
        name: &OsStr,
        newparent: u64,
        newname: &OsStr,
        _flags: u32,
        reply: fuser::ReplyEmpty,
    ) {
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

        match std::fs::rename(&old_path, &new_path) {
            Ok(_) => {
                // Update inode mapping
                if let Some(ino) = self.path_to_inode.write().unwrap().remove(&old_path) {
                    self.inodes.write().unwrap().insert(ino, new_path.clone());
                    self.path_to_inode.write().unwrap().insert(new_path, ino);
                }
                reply.ok();
            }
            Err(e) => reply.error(e.raw_os_error().unwrap_or(libc::EIO)),
        }
    }

    /// Hard links are not supported.
    /// Port of C tup's tup_fs_link() (fuse_fs.c:1041-1047).
    fn link(
        &mut self,
        _req: &Request<'_>,
        _ino: u64,
        _newparent: u64,
        _newname: &OsStr,
        reply: ReplyEntry,
    ) {
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
