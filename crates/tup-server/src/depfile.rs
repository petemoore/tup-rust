use std::io::{Read, Write};
use std::path::Path;

use tup_types::AccessType;

/// A file access event recorded during command execution.
///
/// This is the Rust equivalent of `struct access_event` in C,
/// plus the associated path data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileAccess {
    /// Type of access.
    pub access_type: AccessType,
    /// Primary path (the file accessed).
    pub path: String,
    /// Secondary path (for renames: the destination).
    pub path2: Option<String>,
}

impl FileAccess {
    pub fn read(path: impl Into<String>) -> Self {
        FileAccess {
            access_type: AccessType::Read,
            path: path.into(),
            path2: None,
        }
    }

    pub fn write(path: impl Into<String>) -> Self {
        FileAccess {
            access_type: AccessType::Write,
            path: path.into(),
            path2: None,
        }
    }

    pub fn unlink(path: impl Into<String>) -> Self {
        FileAccess {
            access_type: AccessType::Unlink,
            path: path.into(),
            path2: None,
        }
    }

    pub fn rename(from: impl Into<String>, to: impl Into<String>) -> Self {
        FileAccess {
            access_type: AccessType::Rename,
            path: from.into(),
            path2: Some(to.into()),
        }
    }

    pub fn var(name: impl Into<String>) -> Self {
        FileAccess {
            access_type: AccessType::Var,
            path: name.into(),
            path2: None,
        }
    }
}

/// Write access events to a depfile.
///
/// The depfile format matches the C implementation:
/// - `access_type` as i32 (4 bytes)
/// - `len` as i32 (4 bytes) — length of path
/// - `len2` as i32 (4 bytes) — length of path2 (0 if none)
/// - `path` bytes (len bytes)
/// - `path2` bytes (len2 bytes, if any)
pub fn write_depfile(path: &Path, events: &[FileAccess]) -> std::io::Result<()> {
    let mut file = std::fs::File::create(path)?;

    for event in events {
        let at = event.access_type.as_i32();
        let path_bytes = event.path.as_bytes();
        let path2_bytes = event.path2.as_deref().unwrap_or("").as_bytes();

        file.write_all(&at.to_le_bytes())?;
        file.write_all(&(path_bytes.len() as i32).to_le_bytes())?;
        file.write_all(&(path2_bytes.len() as i32).to_le_bytes())?;
        file.write_all(path_bytes)?;
        if !path2_bytes.is_empty() {
            file.write_all(path2_bytes)?;
        }
    }

    Ok(())
}

/// Read access events from a depfile.
pub fn read_depfile(path: &Path) -> std::io::Result<Vec<FileAccess>> {
    let mut file = std::fs::File::open(path)?;
    let mut events = Vec::new();

    loop {
        // Read access_type (4 bytes)
        let mut at_buf = [0u8; 4];
        match file.read_exact(&mut at_buf) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(e) => return Err(e),
        }
        let at = i32::from_le_bytes(at_buf);

        // Read len (4 bytes)
        let mut len_buf = [0u8; 4];
        file.read_exact(&mut len_buf)?;
        let len = i32::from_le_bytes(len_buf) as usize;

        // Read len2 (4 bytes)
        let mut len2_buf = [0u8; 4];
        file.read_exact(&mut len2_buf)?;
        let len2 = i32::from_le_bytes(len2_buf) as usize;

        // Read path
        let mut path_buf = vec![0u8; len];
        file.read_exact(&mut path_buf)?;
        let path_str = String::from_utf8_lossy(&path_buf).to_string();

        // Read path2
        let path2 = if len2 > 0 {
            let mut path2_buf = vec![0u8; len2];
            file.read_exact(&mut path2_buf)?;
            Some(String::from_utf8_lossy(&path2_buf).to_string())
        } else {
            None
        };

        let access_type = AccessType::from_i32(at).unwrap_or(AccessType::Read);

        events.push(FileAccess {
            access_type,
            path: path_str,
            path2,
        });
    }

    Ok(events)
}

/// Categorize file accesses into read/write/unlink/var lists.
#[derive(Debug, Default)]
pub struct FileAccessSummary {
    pub reads: Vec<String>,
    pub writes: Vec<String>,
    pub unlinks: Vec<String>,
    pub renames: Vec<(String, String)>,
    pub vars: Vec<String>,
}

impl FileAccessSummary {
    /// Build a summary from a list of access events.
    pub fn from_events(events: &[FileAccess]) -> Self {
        let mut summary = Self::default();

        for event in events {
            match event.access_type {
                AccessType::Read => summary.reads.push(event.path.clone()),
                AccessType::Write => summary.writes.push(event.path.clone()),
                AccessType::Unlink => summary.unlinks.push(event.path.clone()),
                AccessType::Rename => {
                    let to = event.path2.clone().unwrap_or_default();
                    summary.renames.push((event.path.clone(), to));
                }
                AccessType::Var => summary.vars.push(event.path.clone()),
            }
        }

        // Deduplicate
        summary.reads.sort();
        summary.reads.dedup();
        summary.writes.sort();
        summary.writes.dedup();
        summary.unlinks.sort();
        summary.unlinks.dedup();
        summary.vars.sort();
        summary.vars.dedup();

        summary
    }

    /// Check if a file was read but not declared as an input.
    pub fn undeclared_reads(&self, declared_inputs: &[String]) -> Vec<String> {
        self.reads
            .iter()
            .filter(|r| !declared_inputs.contains(r))
            .filter(|r| !self.is_ignorable(r))
            .cloned()
            .collect()
    }

    /// Check if a file was written but not declared as an output.
    pub fn undeclared_writes(&self, declared_outputs: &[String]) -> Vec<String> {
        self.writes
            .iter()
            .filter(|w| !declared_outputs.contains(w))
            .filter(|w| !self.is_ignorable(w))
            .cloned()
            .collect()
    }

    /// Check if a path should be ignored (system files, /dev, /proc, etc.)
    fn is_ignorable(&self, path: &str) -> bool {
        path.starts_with("/dev/")
            || path.starts_with("/proc/")
            || path.starts_with("/sys/")
            || path.starts_with("/tmp/")
            || path.starts_with("/etc/")
            || path.starts_with("/usr/")
            || path.starts_with("/lib")
            || path.contains(".tup/")
            || path.contains(".git/")
            || path.contains(".ccache")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_file_access_constructors() {
        let r = FileAccess::read("/tmp/foo");
        assert_eq!(r.access_type, AccessType::Read);
        assert_eq!(r.path, "/tmp/foo");

        let w = FileAccess::write("/tmp/bar");
        assert_eq!(w.access_type, AccessType::Write);

        let rn = FileAccess::rename("/tmp/a", "/tmp/b");
        assert_eq!(rn.path2, Some("/tmp/b".to_string()));
    }

    #[test]
    fn test_depfile_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let depfile = tmp.path().join("depfile");

        let events = vec![
            FileAccess::read("input.c"),
            FileAccess::read("header.h"),
            FileAccess::write("output.o"),
            FileAccess::rename("tmp.o", "output.o"),
            FileAccess::var("CONFIG_DEBUG"),
        ];

        write_depfile(&depfile, &events).unwrap();
        let loaded = read_depfile(&depfile).unwrap();

        assert_eq!(events.len(), loaded.len());
        for (a, b) in events.iter().zip(loaded.iter()) {
            assert_eq!(a.access_type, b.access_type);
            assert_eq!(a.path, b.path);
            assert_eq!(a.path2, b.path2);
        }
    }

    #[test]
    fn test_depfile_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let depfile = tmp.path().join("depfile");
        write_depfile(&depfile, &[]).unwrap();
        let loaded = read_depfile(&depfile).unwrap();
        assert!(loaded.is_empty());
    }

    #[test]
    fn test_summary_from_events() {
        let events = vec![
            FileAccess::read("a.c"),
            FileAccess::read("b.h"),
            FileAccess::read("a.c"), // duplicate
            FileAccess::write("a.o"),
            FileAccess::unlink("tmp.o"),
        ];

        let summary = FileAccessSummary::from_events(&events);
        assert_eq!(summary.reads, vec!["a.c", "b.h"]); // deduped
        assert_eq!(summary.writes, vec!["a.o"]);
        assert_eq!(summary.unlinks, vec!["tmp.o"]);
    }

    #[test]
    fn test_undeclared_reads() {
        let summary = FileAccessSummary {
            reads: vec!["a.c".to_string(), "b.h".to_string(), "c.h".to_string()],
            ..Default::default()
        };

        let declared = vec!["a.c".to_string()];
        let undeclared = summary.undeclared_reads(&declared);
        assert_eq!(undeclared, vec!["b.h", "c.h"]);
    }

    #[test]
    fn test_undeclared_writes() {
        let summary = FileAccessSummary {
            writes: vec!["a.o".to_string(), "extra.tmp".to_string()],
            ..Default::default()
        };

        let declared = vec!["a.o".to_string()];
        let undeclared = summary.undeclared_writes(&declared);
        assert_eq!(undeclared, vec!["extra.tmp"]);
    }

    #[test]
    fn test_ignorable_paths() {
        let summary = FileAccessSummary {
            reads: vec![
                "/usr/include/stdio.h".to_string(),
                "/dev/null".to_string(),
                "local_file.c".to_string(),
            ],
            ..Default::default()
        };

        let undeclared = summary.undeclared_reads(&[]);
        assert_eq!(undeclared, vec!["local_file.c"]);
    }
}
