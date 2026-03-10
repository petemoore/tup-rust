//! Master fork pattern for FUSE command execution.
//!
//! Port of C tup's master_fork.c (887 LOC).
//!
//! On macOS, commands need to be forked from a process that was created BEFORE
//! the FUSE mount, so that chdir() into the FUSE mount works correctly for
//! dependency tracking. The kernel resolves CWD at exec time; if the process
//! was created after the FUSE mount, the kernel may bypass FUSE for path
//! resolution.
//!
//! Architecture (matching C tup):
//!
//! 1. `MasterFork::pre_init()` — called before FUSE mount:
//!    - Creates a Unix socketpair for communication
//!    - fork()s a persistent child process
//!    - Child enters `master_fork_loop()`, parent returns handle
//!
//! 2. `master_fork_loop()` — persistent child process:
//!    - Reads ExecMsg structs from socket (job path, dir path, command, env)
//!    - fork()s a grandchild for each command
//!    - Grandchild does chdir(job) + chdir(dir) + exec (via setup_subprocess)
//!    - Waits for grandchild, sends exit status back over socket
//!
//! 3. `MasterFork::exec()` — parent sends command over socket:
//!    - Serializes ExecMsg + payload to socket
//!    - Reads back exit status
//!
//! C reference: master_fork.c, master_fork.h

use std::collections::BTreeMap;
use std::ffi::CString;
use std::io::{self, Read, Write};
use std::os::unix::net::UnixStream;
use std::sync::Mutex;

/// Message sent from parent to master_fork child to request command execution.
///
/// Port of C tup's `struct execmsg` (master_fork.h:27-39).
/// We use a simpler serialization than the C struct — lengths are sent as
/// little-endian i32, followed by the variable-length payload.
#[derive(Debug, Clone)]
struct ExecMsg {
    /// Session ID (used to correlate request/response). C: em.sid
    sid: i64,
    /// Length of the job path string. C: em.joblen
    job_len: i32,
    /// Length of the dir path string. C: em.dirlen
    dir_len: i32,
    /// Length of the command string. C: em.cmdlen
    cmd_len: i32,
    /// Length of the environment string. C: em.envlen
    env_len: i32,
    /// Whether to run the command in bash instead of sh. C: em.run_in_bash
    run_in_bash: bool,
}

/// Return message from master_fork child to parent with exit status.
///
/// Port of C tup's `struct rcmsg` (master_fork.c:43-46).
#[derive(Debug, Clone, Copy)]
struct ReturnMsg {
    /// Session ID matching the request. C: rcm.sid
    sid: i64,
    /// Exit status from waitpid(). C: rcm.status
    status: i32,
}

/// Handle to the master fork child process.
///
/// Created by `MasterFork::pre_init()` before the FUSE mount. Provides
/// `exec()` to run commands through the pre-fork child, ensuring correct
/// CWD handling for FUSE.
pub struct MasterFork {
    /// Our end of the socketpair (msd[1] in C).
    socket: Mutex<UnixStream>,
    /// PID of the master_fork child process.
    child_pid: i32,
}

impl MasterFork {
    /// Create the master fork child process.
    ///
    /// Port of C tup's `server_pre_init()` (master_fork.c:150-261).
    /// Must be called BEFORE the FUSE filesystem is mounted.
    ///
    /// # Safety
    ///
    /// This function calls `fork()`. The caller must ensure no other threads
    /// are running at the time of the call (standard fork safety requirement).
    pub fn pre_init() -> io::Result<Self> {
        // C: setpgid(0, 0) to set own process group (master_fork.c:155).
        // This ensures child processes inherit our pgid for FUSE context_check().
        // Must be called before fork so the child inherits our pgid.
        unsafe {
            if libc::setpgid(0, 0) < 0 {
                // Non-fatal on macOS if we're already the group leader
                let _ = io::Error::last_os_error();
            }
        }

        // C: socketpair(AF_LOCAL, SOCK_STREAM, 0, msd)
        // Temporarily open /dev/null to prevent socketpair from using fd 0.
        // C: int tmpfd = open("/dev/null", O_RDONLY);
        let (parent_sock, child_sock) = UnixStream::pair()?;

        // C: fork()
        let pid = unsafe { libc::fork() };
        if pid < 0 {
            return Err(io::Error::last_os_error());
        }

        if pid == 0 {
            // === Child process ===
            // C: close(msd[1]) — close parent's socket end
            drop(parent_sock);

            // Signal parent that we're ready.
            // C: write(msd[0], "1", 1)
            let mut sock = child_sock;
            if sock.write_all(b"1").is_err() {
                std::process::exit(1);
            }

            // Enter the command execution loop.
            // C: exit(master_fork_loop())
            let rc = master_fork_loop(sock);
            std::process::exit(rc);
        }

        // === Parent process ===
        // C: close(msd[0]) — close child's socket end
        drop(child_sock);

        // C: read(msd[1], &c, 1) — wait for child to be ready
        let mut parent_sock = parent_sock;
        let mut buf = [0u8; 1];
        loop {
            match parent_sock.read_exact(&mut buf) {
                Ok(()) => break,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            }
        }

        if buf[0] != b'1' {
            return Err(io::Error::other(
                "tup error: master_fork server did not start up correctly.",
            ));
        }

        Ok(MasterFork {
            socket: Mutex::new(parent_sock),
            child_pid: pid,
        })
    }

    /// Execute a command through the master_fork child process.
    ///
    /// Port of C tup's `master_fork_exec()` (master_fork.c:306-349).
    ///
    /// The child process will:
    /// 1. fork() a grandchild
    /// 2. Grandchild does chdir(job) + chdir(dir) + exec(cmd)
    /// 3. Wait for grandchild, return exit status
    ///
    /// # Arguments
    ///
    /// * `sid` - Session ID (unique per command, used for correlation)
    /// * `job` - Job path (e.g., ".tup/mnt/@tupjob-1"), chdir target #1
    /// * `dir` - Dir path (e.g., "Users/.../project/src"), chdir target #2
    /// * `cmd` - Shell command to execute
    /// * `env` - Environment variables as "KEY=VALUE\0KEY=VALUE\0\0"
    /// * `run_in_bash` - If true, use `bash -e -o pipefail -c` instead of `sh -e -c`
    ///
    /// Returns the exit status of the command.
    pub fn exec(
        &self,
        sid: i64,
        job: &str,
        dir: &str,
        cmd: &str,
        env: &[u8],
        run_in_bash: bool,
    ) -> io::Result<i32> {
        let em = ExecMsg {
            sid,
            job_len: job.len() as i32 + 1, // +1 for NUL
            dir_len: dir.len() as i32 + 1,
            cmd_len: cmd.len() as i32 + 1,
            env_len: env.len() as i32,
            run_in_bash,
        };

        // Serialize: send command, read result (serial execution).
        // C tup uses a notifier thread for parallel support; we do
        // inline read for now, matching serial execution behavior.
        let mut socket = self.socket.lock().unwrap();

        // Write the header
        write_exec_msg(&mut *socket, &em)?;

        // Write the payloads (NUL-terminated strings, env is already formatted)
        socket.write_all(job.as_bytes())?;
        socket.write_all(b"\0")?;
        socket.write_all(dir.as_bytes())?;
        socket.write_all(b"\0")?;
        socket.write_all(cmd.as_bytes())?;
        socket.write_all(b"\0")?;
        socket.write_all(env)?;

        // Read the return status directly (C: notifier thread + condvar)
        let rcm = read_return_msg(&mut *socket)?;

        // Decode wait status
        let exit_code = if libc::WIFEXITED(rcm.status) {
            libc::WEXITSTATUS(rcm.status)
        } else if libc::WIFSIGNALED(rcm.status) {
            128 + libc::WTERMSIG(rcm.status)
        } else {
            1
        };

        Ok(exit_code)
    }

    /// Shut down the master fork child process.
    ///
    /// Port of C tup's `server_post_exit()` (master_fork.c:263-294).
    /// Sends a shutdown message (sid=-1) and waits for the child to exit.
    pub fn shutdown(self) -> io::Result<()> {
        // Send shutdown message: sid = -1
        // C: struct execmsg em = {-1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0};
        let em = ExecMsg {
            sid: -1,
            job_len: 0,
            dir_len: 0,
            cmd_len: 0,
            env_len: 0,
            run_in_bash: false,
        };

        {
            let mut socket = self.socket.lock().unwrap();
            write_exec_msg(&mut *socket, &em)?;
        }

        // C: waitpid(master_fork_pid, &status, 0)
        let mut status: i32 = 0;
        unsafe {
            if libc::waitpid(self.child_pid, &mut status, 0) < 0 {
                return Err(io::Error::last_os_error());
            }
        }

        // Drop the socket
        drop(self.socket);

        if status != 0 {
            return Err(io::Error::other(format!(
                "tup error: Master fork process returned {status}"
            )));
        }

        Ok(())
    }
}

// ===== Wire format helpers =====

/// Write an ExecMsg to the socket.
///
/// Port of C tup's write_all() calls for the execmsg struct.
/// We serialize field by field as little-endian to avoid struct layout issues.
fn write_exec_msg(w: &mut impl Write, em: &ExecMsg) -> io::Result<()> {
    w.write_all(&em.sid.to_le_bytes())?;
    w.write_all(&em.job_len.to_le_bytes())?;
    w.write_all(&em.dir_len.to_le_bytes())?;
    w.write_all(&em.cmd_len.to_le_bytes())?;
    w.write_all(&em.env_len.to_le_bytes())?;
    w.write_all(&[if em.run_in_bash { 1 } else { 0 }])?;
    Ok(())
}

/// Read an ExecMsg from the socket.
///
/// Port of C tup's read_all() for the execmsg struct.
fn read_exec_msg(r: &mut impl Read) -> io::Result<ExecMsg> {
    let mut buf8 = [0u8; 8];
    let mut buf4 = [0u8; 4];
    let mut buf1 = [0u8; 1];

    read_all(r, &mut buf8)?;
    let sid = i64::from_le_bytes(buf8);

    read_all(r, &mut buf4)?;
    let job_len = i32::from_le_bytes(buf4);

    read_all(r, &mut buf4)?;
    let dir_len = i32::from_le_bytes(buf4);

    read_all(r, &mut buf4)?;
    let cmd_len = i32::from_le_bytes(buf4);

    read_all(r, &mut buf4)?;
    let env_len = i32::from_le_bytes(buf4);

    read_all(r, &mut buf1)?;
    let run_in_bash = buf1[0] != 0;

    Ok(ExecMsg {
        sid,
        job_len,
        dir_len,
        cmd_len,
        env_len,
        run_in_bash,
    })
}

/// Write a ReturnMsg to the socket.
fn write_return_msg(w: &mut impl Write, rm: &ReturnMsg) -> io::Result<()> {
    w.write_all(&rm.sid.to_le_bytes())?;
    w.write_all(&rm.status.to_le_bytes())?;
    Ok(())
}

/// Read a ReturnMsg from the socket.
fn read_return_msg(r: &mut impl Read) -> io::Result<ReturnMsg> {
    let mut buf8 = [0u8; 8];
    let mut buf4 = [0u8; 4];

    read_all(r, &mut buf8)?;
    let sid = i64::from_le_bytes(buf8);

    read_all(r, &mut buf4)?;
    let status = i32::from_le_bytes(buf4);

    Ok(ReturnMsg { sid, status })
}

/// Read exactly `buf.len()` bytes, retrying on EINTR.
///
/// Port of C tup's `read_all()` (master_fork.c:352-373).
fn read_all(r: &mut impl Read, buf: &mut [u8]) -> io::Result<()> {
    let mut bytes_read = 0;
    while bytes_read < buf.len() {
        match r.read(&mut buf[bytes_read..]) {
            Ok(0) => {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    format!(
                        "tup error: Expected to read {} bytes, but the master fork socket closed after {} bytes.",
                        buf.len(),
                        bytes_read,
                    ),
                ));
            }
            Ok(n) => bytes_read += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

/// Read exactly `len` bytes into a new Vec.
fn read_vec(r: &mut impl Read, len: usize) -> io::Result<Vec<u8>> {
    let mut buf = vec![0u8; len];
    read_all(r, &mut buf)?;
    Ok(buf)
}

// ===== Child process (master_fork_loop) =====

/// The master fork child's main loop.
///
/// Port of C tup's `master_fork_loop()` (master_fork.c:541-777).
///
/// Reads command requests from the socket, fork()s grandchildren to execute
/// them, waits for completion, and sends exit status back.
fn master_fork_loop(mut socket: UnixStream) -> i32 {
    // C: sigaction setup — ignore signals so we stay alive to collect waitpid()
    // On macOS, the master fork process ignores signals; the main tup process
    // handles signal forwarding to the process group.
    unsafe {
        libc::signal(libc::SIGINT, libc::SIG_IGN);
        libc::signal(libc::SIGTERM, libc::SIG_IGN);
        libc::signal(libc::SIGHUP, libc::SIG_IGN);
        libc::signal(libc::SIGUSR1, libc::SIG_IGN);
        libc::signal(libc::SIGUSR2, libc::SIG_IGN);
    }

    // C: dup2(null_fd, STDIN_FILENO) — redirect stdin to /dev/null
    unsafe {
        let null_fd = libc::open(c"/dev/null".as_ptr(), libc::O_RDONLY);
        if null_fd >= 0 {
            libc::dup2(null_fd, libc::STDIN_FILENO);
            libc::close(null_fd);
        }
    }

    // Track active children for waitpid
    let mut children: BTreeMap<i32, i64> = BTreeMap::new(); // pid -> sid

    loop {
        // C: read_all(msd[0], &em, sizeof(em))
        let em = match read_exec_msg(&mut socket) {
            Ok(em) => em,
            Err(_) => return 1,
        };

        // C: if(em.sid == -1) break;
        if em.sid == -1 {
            break;
        }

        // Read variable-length payloads
        // C: read_all(msd[0], job, em.joblen)
        let job_bytes = match read_vec(&mut socket, em.job_len as usize) {
            Ok(v) => v,
            Err(_) => return 1,
        };
        let dir_bytes = match read_vec(&mut socket, em.dir_len as usize) {
            Ok(v) => v,
            Err(_) => return 1,
        };
        let cmd_bytes = match read_vec(&mut socket, em.cmd_len as usize) {
            Ok(v) => v,
            Err(_) => return 1,
        };
        let env_bytes = match read_vec(&mut socket, em.env_len as usize) {
            Ok(v) => v,
            Err(_) => return 1,
        };

        // Strip NUL terminators for string conversion
        let job = String::from_utf8_lossy(&job_bytes[..job_bytes.len().saturating_sub(1)]);
        let dir = String::from_utf8_lossy(&dir_bytes[..dir_bytes.len().saturating_sub(1)]);
        let cmd = String::from_utf8_lossy(&cmd_bytes[..cmd_bytes.len().saturating_sub(1)]);

        // Parse environment: NUL-separated "KEY=VALUE\0KEY=VALUE\0\0"
        let env_strings: Vec<CString> = parse_env_block(&env_bytes);

        // C: fork() grandchild
        let pid = unsafe { libc::fork() };
        if pid < 0 {
            eprintln!("tup error: fork() failed in master_fork_loop");
            std::process::exit(1);
        }

        if pid == 0 {
            // === Grandchild process ===
            // C: close(msd[0])
            drop(socket);

            // C: setup_subprocess(&em, job, dir, ...)
            if setup_subprocess(&job, &dir) != 0 {
                std::process::exit(1);
            }

            // Build envp array for execle
            let mut envp: Vec<*const libc::c_char> =
                env_strings.iter().map(|s| s.as_ptr()).collect();
            envp.push(std::ptr::null());

            // C: execle("/bin/sh", "/bin/sh", "-e", "-c", cmd, NULL, envp)
            // or: execle("/usr/bin/env", "env", "bash", "-e", "-o", "pipefail", "-c", cmd, NULL, envp)
            let c_cmd = CString::new(cmd.as_ref()).unwrap_or_default();

            if em.run_in_bash {
                let c_env = CString::new("/usr/bin/env").unwrap();
                let c_bash = CString::new("bash").unwrap();
                let c_e = CString::new("-e").unwrap();
                let c_pipefail_flag = CString::new("-o").unwrap();
                let c_pipefail = CString::new("pipefail").unwrap();
                let c_c = CString::new("-c").unwrap();
                let argv: [*const libc::c_char; 8] = [
                    c_env.as_ptr(),
                    c_bash.as_ptr(),
                    c_e.as_ptr(),
                    c_pipefail_flag.as_ptr(),
                    c_pipefail.as_ptr(),
                    c_c.as_ptr(),
                    c_cmd.as_ptr(),
                    std::ptr::null(),
                ];
                unsafe {
                    libc::execve(c_env.as_ptr(), argv.as_ptr(), envp.as_ptr());
                }
            } else {
                let c_sh = CString::new("/bin/sh").unwrap();
                let c_e = CString::new("-e").unwrap();
                let c_c = CString::new("-c").unwrap();
                let argv: [*const libc::c_char; 5] = [
                    c_sh.as_ptr(),
                    c_e.as_ptr(),
                    c_c.as_ptr(),
                    c_cmd.as_ptr(),
                    std::ptr::null(),
                ];
                unsafe {
                    libc::execve(c_sh.as_ptr(), argv.as_ptr(), envp.as_ptr());
                }
            }

            // If we get here, exec failed
            eprintln!("tup error: execve failed");
            std::process::exit(1);
        }

        // === Master fork child (not grandchild) ===
        // Track this grandchild.
        // C: waiter->pid = pid; waiter->sid = em.sid;
        children.insert(pid, em.sid);

        // C: In C, there is a separate child_waiter thread that calls wait().
        // For simplicity (and because Rust's UnixStream is not easily shared
        // across fork), we do a synchronous waitpid here for each child.
        // This matches C's behavior for serial execution (one command at a time
        // through the master_fork socket).
        //
        // For parallel execution, the parent sends multiple commands and the
        // child processes them sequentially — each fork+wait before reading
        // the next command. This is correct because parallel commands are sent
        // from different threads in the parent, each waiting on their own sid.
        //
        // However, to support true parallel grandchild execution, we'd need
        // the child_waiter thread pattern from C. For now, we wait inline.
        let mut wait_status: i32 = 0;
        let waited_pid = unsafe { libc::waitpid(pid, &mut wait_status, 0) };
        if waited_pid < 0 {
            eprintln!("tup error: waitpid failed in master_fork_loop");
            return 1;
        }

        children.remove(&pid);

        // Send return message
        // C: write(msd[0], &rcm, sizeof(rcm))
        let rcm = ReturnMsg {
            sid: em.sid,
            status: wait_status,
        };
        if write_return_msg(&mut socket, &rcm).is_err() {
            eprintln!("tup error: Unable to write return status to socket");
            return 1;
        }
    }

    // Shutdown: send sentinel to notifier thread
    // C: rcm.sid = -1, write(msd[0], &rcm, ...)
    let rcm = ReturnMsg { sid: -1, status: 0 };
    let _ = write_return_msg(&mut socket, &rcm);

    0
}

/// Set up the grandchild process before exec.
///
/// Port of C tup's `setup_subprocess()` (master_fork.c:375-539).
/// Simplified: no chroot, no namespacing (macOS only), no output redirection
/// (handled by the parent/executor). Just chdir(job) + chdir(dir).
fn setup_subprocess(job: &str, dir: &str) -> i32 {
    // C: chdir(job) — enter the FUSE job directory
    // e.g., ".tup/mnt/@tupjob-1"
    let c_job = match CString::new(job) {
        Ok(s) => s,
        Err(_) => {
            eprintln!("tup error: Invalid job path");
            return -1;
        }
    };
    unsafe {
        if libc::chdir(c_job.as_ptr()) < 0 {
            let err = io::Error::last_os_error();
            eprintln!("tup error: Unable to chdir to '{}': {}", job, err);
            return -1;
        }
    }

    // C: chdir(dir) — enter the subdirectory within the FUSE mount
    // e.g., "Users/.../project/src"
    let c_dir = match CString::new(dir) {
        Ok(s) => s,
        Err(_) => {
            eprintln!("tup error: Invalid dir path");
            return -1;
        }
    };
    unsafe {
        if libc::chdir(c_dir.as_ptr()) < 0 {
            let err = io::Error::last_os_error();
            eprintln!("tup error: Unable to chdir to '{}': {}", dir, err);
            return -1;
        }
    }

    0
}

/// Parse a NUL-separated environment block into CStrings.
///
/// The environment is formatted as "KEY=VALUE\0KEY=VALUE\0\0" (Windows-style
/// environment block, as used by C tup).
fn parse_env_block(env: &[u8]) -> Vec<CString> {
    let mut result = Vec::new();
    let mut start = 0;
    for (i, &b) in env.iter().enumerate() {
        if b == 0 {
            if i > start {
                if let Ok(s) = CString::new(&env[start..i]) {
                    result.push(s);
                }
            }
            start = i + 1;
        }
    }
    result
}

/// Build a NUL-separated environment block from the current environment.
///
/// Format: "KEY=VALUE\0KEY=VALUE\0\0"
/// This matches the format expected by C tup's master_fork.
pub fn build_env_block() -> Vec<u8> {
    let mut block = Vec::new();
    for (key, value) in std::env::vars() {
        block.extend_from_slice(key.as_bytes());
        block.push(b'=');
        block.extend_from_slice(value.as_bytes());
        block.push(0);
    }
    block.push(0); // Double-NUL terminator
    block
}

/// Build a NUL-separated environment block from specific key-value pairs.
pub fn build_env_block_from(vars: &[(&str, &str)]) -> Vec<u8> {
    let mut block = Vec::new();
    for (key, value) in vars {
        block.extend_from_slice(key.as_bytes());
        block.push(b'=');
        block.extend_from_slice(value.as_bytes());
        block.push(0);
    }
    block.push(0); // Double-NUL terminator
    block
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_env_block_empty() {
        let block = vec![0u8];
        let result = parse_env_block(&block);
        assert!(result.is_empty());
    }

    #[test]
    fn test_parse_env_block_single() {
        let block = b"FOO=bar\0\0";
        let result = parse_env_block(block);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].to_str().unwrap(), "FOO=bar");
    }

    #[test]
    fn test_parse_env_block_multiple() {
        let block = b"FOO=bar\0BAZ=qux\0\0";
        let result = parse_env_block(block);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].to_str().unwrap(), "FOO=bar");
        assert_eq!(result[1].to_str().unwrap(), "BAZ=qux");
    }

    #[test]
    fn test_build_env_block() {
        let block = build_env_block_from(&[("A", "1"), ("B", "2")]);
        assert_eq!(&block, b"A=1\0B=2\0\0");
    }

    #[test]
    fn test_exec_msg_roundtrip() {
        let em = ExecMsg {
            sid: 42,
            job_len: 10,
            dir_len: 20,
            cmd_len: 30,
            env_len: 40,
            run_in_bash: true,
        };

        let mut buf = Vec::new();
        write_exec_msg(&mut buf, &em).unwrap();

        let mut cursor = std::io::Cursor::new(buf);
        let em2 = read_exec_msg(&mut cursor).unwrap();

        assert_eq!(em2.sid, 42);
        assert_eq!(em2.job_len, 10);
        assert_eq!(em2.dir_len, 20);
        assert_eq!(em2.cmd_len, 30);
        assert_eq!(em2.env_len, 40);
        assert!(em2.run_in_bash);
    }

    #[test]
    fn test_return_msg_roundtrip() {
        let rm = ReturnMsg { sid: 7, status: 42 };

        let mut buf = Vec::new();
        write_return_msg(&mut buf, &rm).unwrap();

        let mut cursor = std::io::Cursor::new(buf);
        let rm2 = read_return_msg(&mut cursor).unwrap();

        assert_eq!(rm2.sid, 7);
        assert_eq!(rm2.status, 42);
    }

    #[test]
    fn test_master_fork_exec_simple() {
        // Test the full pre_init -> exec -> shutdown cycle
        let mf = MasterFork::pre_init().expect("pre_init failed");

        let env = build_env_block_from(&[("PATH", "/usr/bin:/bin")]);
        // Use /tmp as both job and dir since we're not testing FUSE
        let status = mf
            .exec(1, "/tmp", ".", "true", &env, false)
            .expect("exec failed");
        assert_eq!(status, 0, "true should exit 0");

        let status = mf
            .exec(2, "/tmp", ".", "false", &env, false)
            .expect("exec failed");
        assert_ne!(status, 0, "false should exit non-zero");

        mf.shutdown().expect("shutdown failed");
    }

    #[test]
    fn test_master_fork_exec_echo() {
        let mf = MasterFork::pre_init().expect("pre_init failed");

        let tmp = tempfile::tempdir().unwrap();
        let output_file = tmp.path().join("output.txt");
        let cmd = format!("echo hello > {}", output_file.display());

        let env = build_env_block_from(&[("PATH", "/usr/bin:/bin")]);
        let status = mf
            .exec(1, "/tmp", ".", &cmd, &env, false)
            .expect("exec failed");
        assert_eq!(status, 0);

        let content = std::fs::read_to_string(&output_file).unwrap();
        assert_eq!(content.trim(), "hello");

        mf.shutdown().expect("shutdown failed");
    }

    #[test]
    fn test_master_fork_exec_chdir() {
        let mf = MasterFork::pre_init().expect("pre_init failed");

        let tmp = tempfile::tempdir().unwrap();
        let subdir = tmp.path().join("sub");
        std::fs::create_dir_all(&subdir).unwrap();

        let output_file = tmp.path().join("pwd.txt");
        let cmd = format!("pwd > {}", output_file.display());

        // chdir to tmp, then chdir to "sub"
        let env = build_env_block_from(&[("PATH", "/usr/bin:/bin")]);
        let status = mf
            .exec(1, &tmp.path().to_string_lossy(), "sub", &cmd, &env, false)
            .expect("exec failed");
        assert_eq!(status, 0);

        let content = std::fs::read_to_string(&output_file).unwrap();
        // Canonicalize both paths — macOS resolves /tmp → /private/tmp
        let actual = std::fs::canonicalize(std::path::Path::new(content.trim()))
            .unwrap_or_else(|_| std::path::PathBuf::from(content.trim()));
        let expected = std::fs::canonicalize(&subdir).unwrap();
        assert_eq!(actual, expected);

        mf.shutdown().expect("shutdown failed");
    }

    #[test]
    fn test_master_fork_multiple_commands() {
        let mf = MasterFork::pre_init().expect("pre_init failed");
        let env = build_env_block_from(&[("PATH", "/usr/bin:/bin")]);

        for i in 1..=5 {
            let status = mf
                .exec(i, "/tmp", ".", "true", &env, false)
                .expect("exec failed");
            assert_eq!(status, 0, "command {} should succeed", i);
        }

        mf.shutdown().expect("shutdown failed");
    }

    #[test]
    fn test_master_fork_bash_mode() {
        let mf = MasterFork::pre_init().expect("pre_init failed");

        let tmp = tempfile::tempdir().unwrap();
        let output_file = tmp.path().join("bash_test.txt");
        // Use bash-specific feature: pipefail (set by -o pipefail flag)
        let cmd = format!("echo bash_ok > {}", output_file.display());

        let env = build_env_block_from(&[("PATH", "/usr/bin:/bin")]);
        let status = mf
            .exec(1, "/tmp", ".", &cmd, &env, true)
            .expect("exec failed");
        assert_eq!(status, 0);

        let content = std::fs::read_to_string(&output_file).unwrap();
        assert_eq!(content.trim(), "bash_ok");

        mf.shutdown().expect("shutdown failed");
    }
}
