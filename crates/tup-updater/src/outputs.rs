use std::collections::BTreeSet;
use std::path::Path;

/// Result of verifying command outputs.
#[derive(Debug)]
pub struct OutputVerification {
    /// Output files that were expected but not created.
    pub missing: Vec<MissingOutput>,
    /// Files created in the working directory that weren't declared as outputs.
    pub unexpected: Vec<String>,
}

/// A missing output file.
#[derive(Debug)]
pub struct MissingOutput {
    /// The expected output path.
    pub path: String,
    /// The command that was supposed to create it.
    pub command: String,
}

impl OutputVerification {
    /// Check if all outputs are accounted for.
    pub fn is_clean(&self) -> bool {
        self.missing.is_empty()
    }

    /// Get a human-readable report.
    pub fn report(&self) -> Vec<String> {
        let mut lines = Vec::new();
        for m in &self.missing {
            lines.push(format!(
                "Expected output '{}' was not created by: {}",
                m.path, m.command,
            ));
        }
        for u in &self.unexpected {
            lines.push(format!(
                "Unexpected file created: {u}",
            ));
        }
        lines
    }
}

/// Verify that all expected outputs were created and detect unexpected files.
///
/// `before_files`: set of files that existed before the build.
/// `expected_outputs`: list of (path, command) pairs.
/// `work_dir`: the directory to check in.
pub fn verify_outputs(
    work_dir: &Path,
    expected_outputs: &[(String, String)],
    before_files: Option<&BTreeSet<String>>,
) -> OutputVerification {
    let mut missing = Vec::new();

    for (path, command) in expected_outputs {
        let full_path = work_dir.join(path);
        if !full_path.exists() {
            missing.push(MissingOutput {
                path: path.clone(),
                command: command.clone(),
            });
        }
    }

    // Detect unexpected files (if we have a before snapshot)
    let unexpected = if let Some(before) = before_files {
        let expected_set: BTreeSet<String> = expected_outputs.iter()
            .map(|(p, _)| p.clone())
            .collect();

        let mut unexpected = Vec::new();
        if let Ok(entries) = std::fs::read_dir(work_dir) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if name.starts_with('.') {
                    continue;
                }
                if entry.file_type().map(|t| t.is_file()).unwrap_or(false)
                    && !before.contains(&name)
                    && !expected_set.contains(&name)
                {
                    unexpected.push(name);
                }
            }
        }
        unexpected
    } else {
        Vec::new()
    };

    OutputVerification { missing, unexpected }
}

/// Snapshot the current files in a directory (for before/after comparison).
pub fn snapshot_files(dir: &Path) -> BTreeSet<String> {
    let mut files = BTreeSet::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            if entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
                let name = entry.file_name().to_string_lossy().to_string();
                if !name.starts_with('.') {
                    files.insert(name);
                }
            }
        }
    }
    files
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_verify_all_present() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("a.o"), "").unwrap();
        std::fs::write(tmp.path().join("b.o"), "").unwrap();

        let expected = vec![
            ("a.o".to_string(), "gcc -c a.c".to_string()),
            ("b.o".to_string(), "gcc -c b.c".to_string()),
        ];

        let result = verify_outputs(tmp.path(), &expected, None);
        assert!(result.is_clean());
        assert!(result.missing.is_empty());
    }

    #[test]
    fn test_verify_missing() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("a.o"), "").unwrap();

        let expected = vec![
            ("a.o".to_string(), "gcc -c a.c".to_string()),
            ("b.o".to_string(), "gcc -c b.c".to_string()),
        ];

        let result = verify_outputs(tmp.path(), &expected, None);
        assert!(!result.is_clean());
        assert_eq!(result.missing.len(), 1);
        assert_eq!(result.missing[0].path, "b.o");
    }

    #[test]
    fn test_verify_unexpected() {
        let tmp = tempfile::tempdir().unwrap();
        let before = snapshot_files(tmp.path());

        std::fs::write(tmp.path().join("expected.o"), "").unwrap();
        std::fs::write(tmp.path().join("surprise.tmp"), "").unwrap();

        let expected = vec![
            ("expected.o".to_string(), "gcc".to_string()),
        ];

        let result = verify_outputs(tmp.path(), &expected, Some(&before));
        assert!(result.is_clean()); // No missing outputs
        assert_eq!(result.unexpected.len(), 1);
        assert_eq!(result.unexpected[0], "surprise.tmp");
    }

    #[test]
    fn test_snapshot_files() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("a.c"), "").unwrap();
        std::fs::write(tmp.path().join("b.c"), "").unwrap();
        std::fs::write(tmp.path().join(".hidden"), "").unwrap();
        std::fs::create_dir(tmp.path().join("subdir")).unwrap();

        let snap = snapshot_files(tmp.path());
        assert!(snap.contains("a.c"));
        assert!(snap.contains("b.c"));
        assert!(!snap.contains(".hidden"));
        assert!(!snap.contains("subdir"));
    }

    #[test]
    fn test_report() {
        let v = OutputVerification {
            missing: vec![MissingOutput {
                path: "foo.o".to_string(),
                command: "gcc -c foo.c".to_string(),
            }],
            unexpected: vec!["bar.tmp".to_string()],
        };

        let lines = v.report();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("foo.o"));
        assert!(lines[1].contains("bar.tmp"));
    }
}
