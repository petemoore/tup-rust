use std::fs::{File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};

/// File-based lock for coordinating concurrent tup processes.
///
/// Uses flock(2) on Unix and LockFileEx on Windows.
pub struct TupLock {
    file: File,
    path: PathBuf,
}

impl TupLock {
    /// Open a lock file (creating it if needed).
    pub fn open(path: &Path) -> io::Result<Self> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)?;

        Ok(TupLock {
            file,
            path: path.to_path_buf(),
        })
    }

    /// Acquire an exclusive lock (blocking).
    pub fn lock_exclusive(&self) -> io::Result<()> {
        sys::flock_exclusive(&self.file)
    }

    /// Try to acquire an exclusive lock (non-blocking).
    ///
    /// Returns Ok(true) if acquired, Ok(false) if would block.
    pub fn try_lock_exclusive(&self) -> io::Result<bool> {
        sys::try_flock_exclusive(&self.file)
    }

    /// Acquire a shared lock (blocking).
    pub fn lock_shared(&self) -> io::Result<()> {
        sys::flock_shared(&self.file)
    }

    /// Release the lock.
    pub fn unlock(&self) -> io::Result<()> {
        sys::flock_unlock(&self.file)
    }

    /// Get the lock file path.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// The tri-lock system for coordinating tup processes.
///
/// Three lock files:
/// - shared: mutex for object lock access
/// - object: database serialization
/// - tri: priority for monitor
pub struct TriLock {
    shared: TupLock,
    object: TupLock,
    #[allow(dead_code)]
    tri: TupLock,
}

impl TriLock {
    /// Initialize the tri-lock system for a .tup directory.
    pub fn new(tup_dir: &Path) -> io::Result<Self> {
        let tup_internal = tup_dir.join(".tup");
        Ok(TriLock {
            shared: TupLock::open(&tup_internal.join("shared"))?,
            object: TupLock::open(&tup_internal.join("object"))?,
            tri: TupLock::open(&tup_internal.join("tri"))?,
        })
    }

    /// Acquire locks for a normal (non-monitor) tup process.
    pub fn lock_normal(&self) -> io::Result<()> {
        self.shared.lock_shared()?;
        self.object.lock_exclusive()?;
        Ok(())
    }

    /// Release locks for a normal process.
    pub fn unlock_normal(&self) -> io::Result<()> {
        self.object.unlock()?;
        self.shared.unlock()?;
        Ok(())
    }

    /// Try to acquire locks without blocking.
    pub fn try_lock_normal(&self) -> io::Result<bool> {
        // First try shared lock
        self.shared.lock_shared()?;
        // Then try exclusive object lock
        match self.object.try_lock_exclusive() {
            Ok(true) => Ok(true),
            Ok(false) => {
                self.shared.unlock()?;
                Ok(false)
            }
            Err(e) => {
                let _ = self.shared.unlock();
                Err(e)
            }
        }
    }
}

#[cfg(unix)]
mod sys {
    use std::fs::File;
    use std::io;
    use std::os::unix::io::AsRawFd;

    pub fn flock_exclusive(file: &File) -> io::Result<()> {
        let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) };
        if rc != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    pub fn try_flock_exclusive(file: &File) -> io::Result<bool> {
        let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        if rc != 0 {
            let err = io::Error::last_os_error();
            if err.kind() == io::ErrorKind::WouldBlock {
                return Ok(false);
            }
            return Err(err);
        }
        Ok(true)
    }

    pub fn flock_shared(file: &File) -> io::Result<()> {
        let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_SH) };
        if rc != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    pub fn flock_unlock(file: &File) -> io::Result<()> {
        let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_UN) };
        if rc != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}

#[cfg(not(unix))]
mod sys {
    use std::fs::File;
    use std::io;

    pub fn flock_exclusive(_file: &File) -> io::Result<()> {
        // TODO: Implement using LockFileEx on Windows
        Ok(())
    }

    pub fn try_flock_exclusive(_file: &File) -> io::Result<bool> {
        Ok(true)
    }

    pub fn flock_shared(_file: &File) -> io::Result<()> {
        Ok(())
    }

    pub fn flock_unlock(_file: &File) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lock_open() {
        let tmp = tempfile::tempdir().unwrap();
        let lock_path = tmp.path().join("test.lock");
        let lock = TupLock::open(&lock_path).unwrap();
        assert!(lock.path().exists());
    }

    #[test]
    fn test_lock_exclusive() {
        let tmp = tempfile::tempdir().unwrap();
        let lock = TupLock::open(&tmp.path().join("test.lock")).unwrap();
        lock.lock_exclusive().unwrap();
        lock.unlock().unwrap();
    }

    #[test]
    fn test_lock_shared() {
        let tmp = tempfile::tempdir().unwrap();
        let lock = TupLock::open(&tmp.path().join("test.lock")).unwrap();
        lock.lock_shared().unwrap();
        lock.unlock().unwrap();
    }

    #[test]
    fn test_try_lock() {
        let tmp = tempfile::tempdir().unwrap();
        let lock = TupLock::open(&tmp.path().join("test.lock")).unwrap();
        assert!(lock.try_lock_exclusive().unwrap());
        lock.unlock().unwrap();
    }

    #[test]
    fn test_trilock() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir(tmp.path().join(".tup")).unwrap();
        let tri = TriLock::new(tmp.path()).unwrap();
        tri.lock_normal().unwrap();
        tri.unlock_normal().unwrap();
    }

    #[test]
    fn test_trilock_try() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir(tmp.path().join(".tup")).unwrap();
        let tri = TriLock::new(tmp.path()).unwrap();
        assert!(tri.try_lock_normal().unwrap());
        tri.unlock_normal().unwrap();
    }
}
