#![allow(dead_code)]
//! FUSE mount/unmount lifecycle.
//!
//! Port of C tup's fuse_server.c server_init()/server_quit()/tup_unmount().
//! Creates .tup/mnt and .tup/tmp directories, mounts the FUSE filesystem,
//! and cleans up on shutdown.
//!
//! C reference: fuse_server.c (955 LOC)

use std::path::{Path, PathBuf};

use crate::tup_fuse::TupFuseFs;

/// Mount point relative to project root.
const TUP_MNT: &str = ".tup/mnt";
/// Temporary file directory relative to project root.
const TUP_TMP: &str = ".tup/tmp";

/// Handle for a mounted FUSE filesystem.
///
/// When dropped, the filesystem is unmounted.
pub struct FuseMount {
    /// Background FUSE session (fuser handles the thread).
    _session: fuser::BackgroundSession,
    /// Mount point path.
    mount_point: PathBuf,
    /// Project root.
    tup_top: PathBuf,
}

impl FuseMount {
    /// Mount the FUSE filesystem.
    ///
    /// Port of C tup's server_init() (fuse_server.c:174-348).
    /// Creates .tup/mnt and .tup/tmp, mounts FUSE, cleans old tmp files.
    pub fn mount(tup_top: &Path) -> anyhow::Result<Self> {
        let mount_point = tup_top.join(TUP_MNT);
        let tmp_dir = tup_top.join(TUP_TMP);

        // Create mount point directory
        // C: mkdir(TUP_MNT, 0777) with stale mount detection
        if let Err(e) = std::fs::create_dir_all(&mount_point) {
            if e.kind() != std::io::ErrorKind::AlreadyExists {
                anyhow::bail!("tup error: Unable to create FUSE mountpoint: {e}");
            }
        }

        // macOS: Create .metadata_never_index to prevent Spotlight
        #[cfg(target_os = "macos")]
        {
            let never_index = mount_point.join(".metadata_never_index");
            let _ = std::fs::File::create(&never_index);
        }

        // Create tmp directory
        // C: mkdir(TUP_TMP, 0777)
        if let Err(e) = std::fs::create_dir_all(&tmp_dir) {
            if e.kind() != std::io::ErrorKind::AlreadyExists {
                anyhow::bail!("tup error: Unable to create temporary working directory: {e}");
            }
        }

        // Clean old tmp files
        // C: flist_foreach(&f, ".") { unlink(f.filename) }
        if let Ok(entries) = std::fs::read_dir(&tmp_dir) {
            for entry in entries.flatten() {
                let name = entry.file_name();
                let name_str = name.to_string_lossy();
                if !name_str.starts_with('.') {
                    let _ = std::fs::remove_file(entry.path());
                }
            }
        }

        // Create the FUSE filesystem and mount it
        let fs = TupFuseFs::new(tup_top);

        // fuser mount options matching C tup:
        // -s (single-threaded), -f (foreground), -onobrowse (macOS)
        let mut options = vec![
            fuser::MountOption::FSName("tup".to_string()),
            fuser::MountOption::AutoUnmount,
        ];
        #[cfg(target_os = "macos")]
        {
            options.push(fuser::MountOption::CUSTOM("nobrowse".to_string()));
            options.push(fuser::MountOption::CUSTOM("noappledouble".to_string()));
            options.push(fuser::MountOption::CUSTOM("noapplexattr".to_string()));
        }

        let session = fuser::spawn_mount2(fs, &mount_point, &options).map_err(|e| {
            anyhow::anyhow!(
                "tup error: Unable to mount FUSE on {}: {e}",
                mount_point.display()
            )
        })?;

        // macOS/FreeBSD: Poll for mount readiness
        // C: for(x=0; x<5000; x++) { access(filename, R_OK) }
        #[cfg(any(target_os = "macos", target_os = "freebsd"))]
        {
            let check_path = mount_point.join(".");
            for _ in 0..5000 {
                if check_path.exists() {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
        }

        Ok(FuseMount {
            _session: session,
            mount_point,
            tup_top: tup_top.to_path_buf(),
        })
    }

    /// Get the mount point path.
    pub fn mount_point(&self) -> &Path {
        &self.mount_point
    }

    /// Get the project root.
    pub fn tup_top(&self) -> &Path {
        &self.tup_top
    }
}

// Unmount happens automatically when FuseMount is dropped
// (fuser::BackgroundSession handles this).

/// Unmount a FUSE filesystem at the given mount point.
///
/// Port of C tup's tup_unmount() (fuse_server.c:157-172).
/// Called as a fallback if automatic unmount doesn't work.
pub fn force_unmount(mount_point: &Path) -> anyhow::Result<()> {
    #[cfg(target_os = "linux")]
    {
        let status = std::process::Command::new("fusermount")
            .args(["-u", "-z"])
            .arg(mount_point)
            .status()?;
        if !status.success() {
            // Try fusermount3
            let status = std::process::Command::new("fusermount3")
                .args(["-u", "-z"])
                .arg(mount_point)
                .status()?;
            if !status.success() {
                anyhow::bail!(
                    "tup error: Unable to unmount FUSE at {}",
                    mount_point.display()
                );
            }
        }
    }

    #[cfg(target_os = "macos")]
    {
        use std::ffi::CString;
        let path_c = CString::new(mount_point.to_string_lossy().as_bytes())?;
        unsafe {
            if libc::unmount(path_c.as_ptr(), libc::MNT_FORCE) < 0 {
                let err = std::io::Error::last_os_error();
                if err.raw_os_error() == Some(libc::EBUSY) {
                    eprintln!("tup warning: FUSE filesystem busy, retrying...");
                    std::thread::sleep(std::time::Duration::from_millis(500));
                    if libc::unmount(path_c.as_ptr(), libc::MNT_FORCE) < 0 {
                        anyhow::bail!("tup error: Unable to unmount FUSE: {}", err);
                    }
                } else {
                    anyhow::bail!("tup error: Unable to unmount FUSE: {}", err);
                }
            }
        }
    }

    #[cfg(target_os = "freebsd")]
    {
        let status = std::process::Command::new("umount")
            .args(["-f"])
            .arg(mount_point)
            .status()?;
        if !status.success() {
            anyhow::bail!(
                "tup error: Unable to unmount FUSE at {}",
                mount_point.display()
            );
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_constants() {
        assert_eq!(TUP_MNT, ".tup/mnt");
        assert_eq!(TUP_TMP, ".tup/tmp");
    }

    // Integration test: actually mounting requires macFUSE + privileges.
    // Manual test: create a temp dir, mount, verify, unmount.
    // This would need to be an integration test, not a unit test.
}
