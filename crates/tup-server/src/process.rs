use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Instant;

use crate::depfile::{read_depfile, FileAccess, FileAccessSummary};

/// Server mode for dependency tracking.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServerMode {
    /// No dependency tracking — rely on Tupfile declarations.
    None,
    /// Use LD_PRELOAD for dependency tracking (Linux).
    LdPreload,
    /// Use FUSE filesystem for dependency tracking.
    Fuse,
}

/// Result of executing a command through the process server.
#[derive(Debug)]
pub struct ServerResult {
    /// Whether the command succeeded.
    pub success: bool,
    /// Exit code.
    pub exit_code: Option<i32>,
    /// Whether the process was killed by signal.
    pub signalled: bool,
    /// Standard output.
    pub stdout: String,
    /// Standard error.
    pub stderr: String,
    /// Execution time in milliseconds.
    pub duration_ms: u64,
    /// File accesses recorded (if dependency tracking was active).
    pub file_accesses: Vec<FileAccess>,
    /// Summary of file accesses.
    pub summary: FileAccessSummary,
}

/// Process server that executes commands with optional dependency tracking.
pub struct ProcessServer {
    /// Working directory for command execution.
    work_dir: PathBuf,
    /// Server mode.
    mode: ServerMode,
    /// Path to the LD_PRELOAD library (if mode is LdPreload).
    ldpreload_path: Option<PathBuf>,
}

impl ProcessServer {
    /// Create a new process server.
    pub fn new(work_dir: &Path, mode: ServerMode) -> Self {
        ProcessServer {
            work_dir: work_dir.to_path_buf(),
            mode,
            ldpreload_path: None,
        }
    }

    /// Set the path to the LD_PRELOAD shared library.
    pub fn set_ldpreload_path(&mut self, path: PathBuf) {
        self.ldpreload_path = Some(path);
    }

    /// Get the server mode.
    pub fn mode(&self) -> ServerMode {
        self.mode
    }

    /// Execute a command with dependency tracking.
    pub fn exec(&self, cmd: &str, env_vars: &[(String, String)]) -> Result<ServerResult, String> {
        let start = Instant::now();

        let shell = if cfg!(target_os = "windows") {
            "cmd"
        } else {
            "sh"
        };
        let flag = if cfg!(target_os = "windows") {
            "/C"
        } else {
            "-c"
        };

        let mut command = Command::new(shell);
        command.arg(flag).arg(cmd);
        command.current_dir(&self.work_dir);
        command.stdout(Stdio::piped());
        command.stderr(Stdio::piped());

        // Set up environment
        for (key, value) in env_vars {
            command.env(key, value);
        }

        // Set up dependency tracking based on mode
        let depfile_path = match self.mode {
            ServerMode::LdPreload => {
                let depfile = self.work_dir.join(".tup_depfile");
                command.env("TUP_DEPFILE", depfile.to_str().unwrap_or(""));

                if let Some(ref ldpreload) = self.ldpreload_path {
                    // Prepend to existing LD_PRELOAD
                    let existing = std::env::var("LD_PRELOAD").unwrap_or_default();
                    let new_val = if existing.is_empty() {
                        ldpreload.to_string_lossy().to_string()
                    } else {
                        format!("{}:{}", ldpreload.display(), existing)
                    };
                    command.env("LD_PRELOAD", new_val);
                }

                Some(depfile)
            }
            ServerMode::Fuse => {
                // FUSE tracking is handled at the filesystem level,
                // not via environment variables
                None
            }
            ServerMode::None => None,
        };

        // Execute
        let output = command
            .output()
            .map_err(|e| format!("failed to execute command: {e}"))?;

        let duration = start.elapsed();

        // Read depfile if it was created
        let file_accesses = if let Some(ref depfile) = depfile_path {
            if depfile.exists() {
                let events =
                    read_depfile(depfile).map_err(|e| format!("failed to read depfile: {e}"))?;
                // Clean up depfile
                let _ = std::fs::remove_file(depfile);
                events
            } else {
                Vec::new()
            }
        } else {
            Vec::new()
        };

        let summary = FileAccessSummary::from_events(&file_accesses);
        let signalled = output.status.code().is_none() && !output.status.success();

        Ok(ServerResult {
            success: output.status.success(),
            exit_code: output.status.code(),
            signalled,
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
            duration_ms: duration.as_millis() as u64,
            file_accesses,
            summary,
        })
    }

    /// Execute a command and verify dependencies against declarations.
    ///
    /// Returns warnings about undeclared reads/writes.
    pub fn exec_and_verify(
        &self,
        cmd: &str,
        declared_inputs: &[String],
        declared_outputs: &[String],
        env_vars: &[(String, String)],
    ) -> Result<(ServerResult, Vec<String>), String> {
        let result = self.exec(cmd, env_vars)?;

        let mut warnings = Vec::new();

        if !result.file_accesses.is_empty() {
            let undeclared_reads = result.summary.undeclared_reads(declared_inputs);
            for path in &undeclared_reads {
                warnings.push(format!("Undeclared read: {path}"));
            }

            let undeclared_writes = result.summary.undeclared_writes(declared_outputs);
            for path in &undeclared_writes {
                warnings.push(format!("Undeclared write: {path}"));
            }
        }

        Ok((result, warnings))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_server_exec_none_mode() {
        let tmp = tempfile::tempdir().unwrap();
        let server = ProcessServer::new(tmp.path(), ServerMode::None);

        let result = server.exec("echo hello > out.txt", &[]).unwrap();
        assert!(result.success);
        assert!(tmp.path().join("out.txt").exists());
        assert!(result.file_accesses.is_empty()); // No tracking in None mode
    }

    #[test]
    fn test_server_exec_failure() {
        let tmp = tempfile::tempdir().unwrap();
        let server = ProcessServer::new(tmp.path(), ServerMode::None);

        let result = server.exec("false", &[]).unwrap();
        assert!(!result.success);
    }

    #[test]
    fn test_server_exec_captures_output() {
        let tmp = tempfile::tempdir().unwrap();
        let server = ProcessServer::new(tmp.path(), ServerMode::None);

        let result = server.exec("echo test_output", &[]).unwrap();
        assert!(result.stdout.contains("test_output"));
    }

    #[test]
    fn test_server_exec_with_env() {
        let tmp = tempfile::tempdir().unwrap();
        let server = ProcessServer::new(tmp.path(), ServerMode::None);

        // Use platform-appropriate env var expansion
        #[cfg(unix)]
        let cmd = "echo $MY_VAR > out.txt";
        #[cfg(windows)]
        let cmd = "echo %MY_VAR% > out.txt";

        let result = server
            .exec(cmd, &[("MY_VAR".to_string(), "env_value".to_string())])
            .unwrap();

        assert!(result.success);
        let content = std::fs::read_to_string(tmp.path().join("out.txt")).unwrap();
        assert!(content.contains("env_value"));
    }

    #[test]
    fn test_server_exec_timing() {
        let tmp = tempfile::tempdir().unwrap();
        let server = ProcessServer::new(tmp.path(), ServerMode::None);

        let result = server.exec("true", &[]).unwrap();
        // Should complete quickly
        assert!(result.duration_ms < 5000);
    }

    #[test]
    fn test_server_verify_no_tracking() {
        let tmp = tempfile::tempdir().unwrap();
        let server = ProcessServer::new(tmp.path(), ServerMode::None);

        let (result, warnings) = server
            .exec_and_verify(
                "echo ok",
                &["input.c".to_string()],
                &["output.o".to_string()],
                &[],
            )
            .unwrap();

        assert!(result.success);
        assert!(warnings.is_empty()); // No tracking = no warnings
    }

    #[test]
    fn test_server_mode() {
        let tmp = tempfile::tempdir().unwrap();
        let server = ProcessServer::new(tmp.path(), ServerMode::LdPreload);
        assert_eq!(server.mode(), ServerMode::LdPreload);
    }
}
