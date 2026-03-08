use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Result of scanning the filesystem.
#[derive(Debug)]
pub struct ScanResult {
    /// Files that are new (not in the database).
    pub new_files: Vec<ScannedFile>,
    /// Files that have been modified (mtime changed).
    pub modified_files: Vec<ScannedFile>,
    /// Files that have been deleted (in database but not on disk).
    pub deleted_files: Vec<String>,
    /// Directories found.
    pub directories: Vec<String>,
}

/// A file discovered during scanning.
#[derive(Debug, Clone)]
pub struct ScannedFile {
    /// Path relative to the project root.
    pub path: String,
    /// Modification time in seconds since epoch.
    pub mtime: i64,
    /// Modification time nanoseconds.
    pub mtime_ns: i64,
}

/// Directories to skip during scanning.
const SKIP_DIRS: &[&str] = &[".tup", ".git", ".hg", ".svn", ".bzr", ".ccache"];

/// Files to skip during scanning.
const SKIP_FILES: &[&str] = &[];

/// Scan a directory tree for files.
///
/// Returns all files found, excluding hidden directories and
/// tup-internal directories.
pub fn scan_directory(root: &Path) -> Result<ScanResult, String> {
    let mut result = ScanResult {
        new_files: Vec::new(),
        modified_files: Vec::new(),
        deleted_files: Vec::new(),
        directories: Vec::new(),
    };

    scan_recursive(root, root, &mut result)?;
    Ok(result)
}

fn scan_recursive(
    root: &Path,
    dir: &Path,
    result: &mut ScanResult,
) -> Result<(), String> {
    let entries = std::fs::read_dir(dir)
        .map_err(|e| format!("failed to read directory {}: {e}", dir.display()))?;

    for entry in entries {
        let entry = entry.map_err(|e| format!("failed to read entry: {e}"))?;
        let name = entry.file_name().to_string_lossy().to_string();

        // Skip hidden files/dirs
        if name.starts_with('.') {
            // But check if it's a skipped directory specifically
            if SKIP_DIRS.contains(&name.as_str()) {
                continue;
            }
            // Skip other hidden files too
            continue;
        }

        // Skip any files in SKIP_FILES
        if SKIP_FILES.contains(&name.as_str()) {
            continue;
        }

        let path = entry.path();
        let rel_path = path.strip_prefix(root)
            .map_err(|_| "failed to compute relative path".to_string())?
            .to_string_lossy()
            .to_string();

        let file_type = entry.file_type()
            .map_err(|e| format!("failed to get file type for {}: {e}", path.display()))?;

        if file_type.is_dir() {
            result.directories.push(rel_path.clone());
            scan_recursive(root, &path, result)?;
        } else if file_type.is_file() {
            let metadata = entry.metadata()
                .map_err(|e| format!("failed to get metadata for {}: {e}", path.display()))?;

            let (mtime, mtime_ns) = get_mtime(&metadata);

            result.new_files.push(ScannedFile {
                path: rel_path,
                mtime,
                mtime_ns,
            });
        }
        // Skip symlinks and other special files for now
    }

    Ok(())
}

/// Extract mtime from file metadata.
fn get_mtime(metadata: &std::fs::Metadata) -> (i64, i64) {
    match metadata.modified() {
        Ok(time) => {
            match time.duration_since(SystemTime::UNIX_EPOCH) {
                Ok(duration) => (duration.as_secs() as i64, duration.subsec_nanos() as i64),
                Err(_) => (-1, 0),
            }
        }
        Err(_) => (-1, 0),
    }
}

/// Compare scan results against a set of known files.
///
/// `known_files` is a set of relative paths that are already tracked.
/// Returns (new, modified, deleted) file lists.
pub fn diff_scan(
    scanned: &[ScannedFile],
    known_files: &BTreeSet<String>,
    known_mtimes: &std::collections::BTreeMap<String, (i64, i64)>,
) -> (Vec<ScannedFile>, Vec<ScannedFile>, Vec<String>) {
    let mut new_files = Vec::new();
    let mut modified_files = Vec::new();

    let scanned_paths: BTreeSet<String> = scanned.iter()
        .map(|f| f.path.clone())
        .collect();

    for file in scanned {
        if !known_files.contains(&file.path) {
            new_files.push(file.clone());
        } else if let Some(&(known_mtime, known_mtime_ns)) = known_mtimes.get(&file.path) {
            if file.mtime != known_mtime || file.mtime_ns != known_mtime_ns {
                modified_files.push(file.clone());
            }
        }
    }

    let deleted_files: Vec<String> = known_files.iter()
        .filter(|f| !scanned_paths.contains(f.as_str()))
        .cloned()
        .collect();

    (new_files, modified_files, deleted_files)
}

/// Find all Tupfiles in a directory tree.
pub fn find_tupfiles(root: &Path) -> Result<Vec<PathBuf>, String> {
    let mut tupfiles = Vec::new();
    find_tupfiles_recursive(root, root, &mut tupfiles)?;
    tupfiles.sort();
    Ok(tupfiles)
}

fn find_tupfiles_recursive(
    root: &Path,
    dir: &Path,
    tupfiles: &mut Vec<PathBuf>,
) -> Result<(), String> {
    let entries = std::fs::read_dir(dir)
        .map_err(|e| format!("failed to read directory {}: {e}", dir.display()))?;

    for entry in entries {
        let entry = entry.map_err(|e| format!("failed to read entry: {e}"))?;
        let name = entry.file_name().to_string_lossy().to_string();

        if name.starts_with('.') {
            continue;
        }

        let path = entry.path();
        let file_type = entry.file_type()
            .map_err(|e| format!("failed to get file type: {e}"))?;

        if file_type.is_dir() {
            find_tupfiles_recursive(root, &path, tupfiles)?;
        } else if name == "Tupfile" || name == "Tupfile.lua" {
            let rel = path.strip_prefix(root)
                .map_err(|_| "failed to strip prefix".to_string())?;
            tupfiles.push(rel.to_path_buf());
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_scan_directory() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("a.c"), "int main() {}").unwrap();
        std::fs::write(tmp.path().join("b.c"), "void foo() {}").unwrap();
        std::fs::create_dir(tmp.path().join("src")).unwrap();
        std::fs::write(tmp.path().join("src/lib.c"), "").unwrap();
        std::fs::create_dir(tmp.path().join(".git")).unwrap();
        std::fs::write(tmp.path().join(".git/config"), "").unwrap();

        let result = scan_directory(tmp.path()).unwrap();

        let paths: BTreeSet<String> = result.new_files.iter()
            .map(|f| f.path.clone()).collect();
        assert!(paths.contains("a.c"));
        assert!(paths.contains("b.c"));
        assert!(paths.contains("src/lib.c"));
        assert!(!paths.contains(".git/config")); // Hidden dir excluded
        assert!(result.directories.contains(&"src".to_string()));
    }

    #[test]
    fn test_scan_skips_tup_dir() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir(tmp.path().join(".tup")).unwrap();
        std::fs::write(tmp.path().join(".tup/db"), "").unwrap();
        std::fs::write(tmp.path().join("Tupfile"), "").unwrap();

        let result = scan_directory(tmp.path()).unwrap();
        let paths: Vec<&str> = result.new_files.iter()
            .map(|f| f.path.as_str()).collect();
        assert!(paths.contains(&"Tupfile"));
        assert!(!paths.iter().any(|p| p.contains(".tup")));
    }

    #[test]
    fn test_scan_mtimes() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("test.c"), "hello").unwrap();

        let result = scan_directory(tmp.path()).unwrap();
        assert_eq!(result.new_files.len(), 1);
        assert!(result.new_files[0].mtime > 0);
    }

    #[test]
    fn test_diff_scan() {
        let scanned = vec![
            ScannedFile { path: "a.c".to_string(), mtime: 100, mtime_ns: 0 },
            ScannedFile { path: "b.c".to_string(), mtime: 200, mtime_ns: 0 },
            ScannedFile { path: "new.c".to_string(), mtime: 300, mtime_ns: 0 },
        ];

        let mut known = BTreeSet::new();
        known.insert("a.c".to_string());
        known.insert("b.c".to_string());
        known.insert("deleted.c".to_string());

        let mut mtimes = std::collections::BTreeMap::new();
        mtimes.insert("a.c".to_string(), (100i64, 0i64));
        mtimes.insert("b.c".to_string(), (150i64, 0i64)); // Different mtime

        let (new, modified, deleted) = diff_scan(&scanned, &known, &mtimes);
        assert_eq!(new.len(), 1);
        assert_eq!(new[0].path, "new.c");
        assert_eq!(modified.len(), 1);
        assert_eq!(modified[0].path, "b.c");
        assert_eq!(deleted, vec!["deleted.c"]);
    }

    #[test]
    fn test_find_tupfiles() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("Tupfile"), ": |> echo |>").unwrap();
        std::fs::create_dir(tmp.path().join("src")).unwrap();
        std::fs::write(tmp.path().join("src/Tupfile"), ": |> echo |>").unwrap();
        std::fs::create_dir(tmp.path().join("src/sub")).unwrap();
        std::fs::write(tmp.path().join("src/sub/Tupfile"), "").unwrap();
        std::fs::write(tmp.path().join("src/notup.txt"), "").unwrap();

        let tupfiles = find_tupfiles(tmp.path()).unwrap();
        assert_eq!(tupfiles.len(), 3);
        assert!(tupfiles.contains(&PathBuf::from("Tupfile")));
        assert!(tupfiles.contains(&PathBuf::from("src/Tupfile")));
        assert!(tupfiles.contains(&PathBuf::from("src/sub/Tupfile")));
    }

    #[test]
    fn test_find_tupfiles_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let tupfiles = find_tupfiles(tmp.path()).unwrap();
        assert!(tupfiles.is_empty());
    }

    #[test]
    fn test_scan_empty_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let result = scan_directory(tmp.path()).unwrap();
        assert!(result.new_files.is_empty());
        assert!(result.directories.is_empty());
    }
}
