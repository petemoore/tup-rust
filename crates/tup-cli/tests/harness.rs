use std::path::{Path, PathBuf};
use std::process::Command;

/// Test harness for tup integration tests.
///
/// Mirrors the functionality of the C test suite's tup.sh.
#[allow(dead_code)]
pub struct TupTestEnv {
    /// Temporary directory for this test.
    pub dir: tempfile::TempDir,
    /// Path to the tup binary.
    pub tup_bin: PathBuf,
}

#[allow(dead_code, clippy::new_without_default)]
impl TupTestEnv {
    /// Create a new test environment with `tup init` already run.
    pub fn new() -> Self {
        let dir = tempfile::tempdir().expect("failed to create temp dir");

        // Find the tup binary
        let tup_bin = cargo_bin("tup");

        // Run tup init
        let output = Command::new(&tup_bin)
            .args(["init", "--no-sync", "--force"])
            .current_dir(dir.path())
            .output()
            .expect("failed to run tup init");

        assert!(output.status.success(), "tup init failed: {}",
            String::from_utf8_lossy(&output.stderr));

        TupTestEnv { dir, tup_bin }
    }

    /// Get the test directory path.
    pub fn path(&self) -> &Path {
        self.dir.path()
    }

    /// Write a file relative to the test directory.
    pub fn write_file(&self, name: &str, content: &str) {
        let path = self.dir.path().join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("failed to create parent dirs");
        }
        std::fs::write(&path, content).expect("failed to write file");
    }

    /// Write a Tupfile.
    pub fn write_tupfile(&self, content: &str) {
        self.write_file("Tupfile", content);
    }

    /// Write a Tupfile in a subdirectory.
    pub fn write_tupfile_in(&self, dir: &str, content: &str) {
        self.write_file(&format!("{dir}/Tupfile"), content);
    }

    /// Run `tup upd` and return the result.
    pub fn update(&self) -> TupResult {
        self.run_tup(&["upd", "-j", "1"])
    }

    /// Run `tup upd` with parallel jobs.
    pub fn update_parallel(&self, jobs: usize) -> TupResult {
        self.run_tup(&["upd", "-j", &jobs.to_string()])
    }

    /// Run `tup upd --keep-going` and expect it to fail.
    pub fn update_keep_going(&self) -> TupResult {
        self.run_tup(&["upd", "-j", "1", "--keep-going"])
    }

    /// Run `tup parse` and return the result.
    pub fn parse(&self) -> TupResult {
        self.run_tup(&["parse"])
    }

    /// Run `tup graph` and return the result.
    pub fn graph(&self) -> TupResult {
        self.run_tup(&["graph"])
    }

    /// Run a tup command.
    pub fn run_tup(&self, args: &[&str]) -> TupResult {
        let output = Command::new(&self.tup_bin)
            .args(args)
            .current_dir(self.dir.path())
            .output()
            .expect("failed to run tup");

        TupResult {
            success: output.status.success(),
            exit_code: output.status.code(),
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        }
    }

    /// Check that a file exists.
    pub fn check_exist(&self, path: &str) {
        let full = self.dir.path().join(path);
        assert!(full.exists(), "expected file to exist: {path}");
    }

    /// Check that a file does not exist.
    pub fn check_not_exist(&self, path: &str) {
        let full = self.dir.path().join(path);
        assert!(!full.exists(), "expected file to not exist: {path}");
    }

    /// Read a file's contents.
    pub fn read_file(&self, path: &str) -> String {
        let full = self.dir.path().join(path);
        std::fs::read_to_string(&full)
            .unwrap_or_else(|e| panic!("failed to read {path}: {e}"))
    }
}

/// Result of running a tup command.
#[derive(Debug)]
#[allow(dead_code)]
pub struct TupResult {
    pub success: bool,
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

#[allow(dead_code)]
impl TupResult {
    /// Assert the command succeeded.
    pub fn assert_success(&self) {
        assert!(self.success, "tup command failed:\nstdout: {}\nstderr: {}",
            self.stdout, self.stderr);
    }

    /// Assert the command failed.
    pub fn assert_failure(&self) {
        assert!(!self.success, "expected tup command to fail but it succeeded:\nstdout: {}",
            self.stdout);
    }

    /// Assert stdout contains a string.
    pub fn assert_stdout_contains(&self, s: &str) {
        assert!(self.stdout.contains(s),
            "expected stdout to contain '{s}'\nstdout: {}", self.stdout);
    }

    /// Assert stderr contains a string.
    pub fn assert_stderr_contains(&self, s: &str) {
        assert!(self.stderr.contains(s),
            "expected stderr to contain '{s}'\nstderr: {}", self.stderr);
    }
}

/// Find a binary built by cargo.
fn cargo_bin(name: &str) -> PathBuf {
    let mut path = std::env::current_exe()
        .expect("failed to get current exe path");
    // Navigate from test binary to target/debug/
    path.pop(); // remove test binary name
    if path.ends_with("deps") {
        path.pop(); // remove deps/
    }
    path.push(name);

    if !path.exists() {
        // Try with .exe on Windows
        path.set_extension("exe");
    }

    assert!(path.exists(), "tup binary not found at: {}", path.display());
    path
}
