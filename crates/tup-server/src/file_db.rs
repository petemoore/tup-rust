#![allow(dead_code)]
//! File access → DB integration.
//!
//! Port of C tup's file.c write_files()/update_write_info()/update_read_info().
//! After a command executes, this module processes the file access lists
//! (reads, writes, unlinks) and creates/updates dependency links in the DB.
//!
//! C reference: file.c (876 LOC), specifically:
//! - write_files() (232-313) — orchestrator
//! - update_write_info() (693-841) — process write list
//! - update_read_info() (842-876+) — process read list

use std::path::Path;

use tup_types::{LinkType, NodeType, TupId};

use crate::tup_fuse::FileInfo;

/// Result of processing file accesses.
pub struct WriteFilesResult {
    /// Number of read dependencies created.
    pub reads_processed: usize,
    /// Number of write outputs verified.
    pub writes_processed: usize,
    /// Warnings generated during processing.
    pub warnings: Vec<String>,
    /// Whether the check failed (undeclared outputs, etc.)
    pub failed: bool,
}

/// Process file accesses from a completed command and update the database.
///
/// Port of C tup's write_files() (file.c:232-313).
/// Takes the FileInfo from FUSE tracking and creates/updates links in the DB.
///
/// This is the core integration point between the FUSE filesystem
/// (which records file accesses) and the tup database (which stores
/// the dependency graph).
pub fn write_files(
    db: &tup_db::TupDb,
    cmd_id: TupId,
    dir_id: TupId,
    finfo: &mut FileInfo,
    tup_top: &Path,
) -> anyhow::Result<WriteFilesResult> {
    let mut result = WriteFilesResult {
        reads_processed: 0,
        writes_processed: 0,
        warnings: Vec::new(),
        failed: false,
    };

    // Process unlinks first
    // C: handle_unlink(info)
    finfo.handle_unlink();

    // C: Process remaining tmpdirs — error if not removed during execution
    // (file.c:244-270)
    for tmpdir in &finfo.tmpdirs {
        eprintln!(
            "tup error: Directory '{}' was created, but not subsequently removed. \
             Only temporary directories can be created by commands.",
            tmpdir
        );
        result.failed = true;
    }
    finfo.tmpdirs.clear();
    if result.failed {
        return Ok(result);
    }

    // Process writes (outputs)
    // C: update_write_info(f, cmdid, info, warnings, check_only)
    result.writes_processed = update_write_info(db, cmd_id, dir_id, finfo, tup_top, &mut result)?;

    // C: Rename temporary files to their real destinations (file.c:814-833)
    finalize_mappings(finfo, tup_top)?;

    // Process reads (inputs)
    // C: update_read_info(f, cmdid, info, full_deps, vardt, important_link_removed)
    result.reads_processed = update_read_info(db, cmd_id, dir_id, finfo, &mut result)?;

    Ok(result)
}

/// Process the write list: verify outputs match declarations.
///
/// Port of C tup's update_write_info() (file.c:693-841).
/// For each file written during execution:
/// 1. Check if it's a declared output
/// 2. If not, report as undeclared output (error)
/// 3. If yes, create the CMD→output normal link and update mtime
fn update_write_info(
    db: &tup_db::TupDb,
    cmd_id: TupId,
    dir_id: TupId,
    finfo: &FileInfo,
    tup_top: &Path,
    result: &mut WriteFilesResult,
) -> anyhow::Result<usize> {
    let mut count = 0;

    for written_file in &finfo.write_list {
        // Skip system paths
        if written_file.starts_with("/dev")
            || written_file.starts_with("/proc")
            || written_file.starts_with("/sys")
        {
            continue;
        }

        // Skip hidden files (C: get_path_elements → PG_HIDDEN check)
        if crate::tup_fuse::TupFuseFs::is_hidden(written_file) {
            result.warnings.push(format!(
                "tup warning: Writing to hidden file '{written_file}'"
            ));
            continue;
        }

        // Look up the output node in the DB
        let file_name = Path::new(written_file)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();

        if let Ok(Some(node)) = db.node_select(dir_id, &file_name) {
            if node.node_type == NodeType::Generated {
                // This is a declared output — will be finalized in finalize_mappings()
                // For now, create the CMD→output link
                let _ = db.link_insert(cmd_id, node.id, LinkType::Normal);
                count += 1;
            }
        } else {
            // C: File not in DB → undeclared output error (file.c:760-764)
            eprintln!(
                "tup error: File '{}' was written to, but is not in .tup/db. \
                 You probably should specify it as an output",
                written_file
            );
            result.failed = true;
            // C: Delete the undeclared output (do_unlink)
            let undeclared_path = tup_top.join(written_file);
            let _ = std::fs::remove_file(&undeclared_path);
            // Also remove its mapping if present
            if let Some(mapping) = finfo.mappings.get(written_file) {
                let _ = std::fs::remove_file(&mapping.tmpname);
            }
        }
    }

    Ok(count)
}

/// Rename temporary files to their real destinations.
///
/// Port of C tup's mapping finalization (file.c:814-833).
/// After write_files validates outputs, move each .tup/tmp/hex file
/// to its real output path.
fn finalize_mappings(finfo: &mut FileInfo, tup_top: &Path) -> anyhow::Result<()> {
    let mappings: Vec<_> = std::mem::take(&mut finfo.mappings).into_iter().collect();
    for (_realname, mapping) in mappings {
        let dest = tup_top.join(&mapping.realname);

        // Ensure destination directory exists
        if let Some(parent) = dest.parent() {
            let _ = std::fs::create_dir_all(parent);
        }

        // C: renameat(tup_top_fd(), map->tmpname, tup_top_fd(), map->realname)
        if mapping.tmpname != dest {
            if let Err(e) = std::fs::rename(&mapping.tmpname, &dest) {
                eprintln!(
                    "tup error: Unable to rename temporary file '{}' to destination '{}': {}",
                    mapping.tmpname.display(),
                    dest.display(),
                    e
                );
            }
        }
    }
    Ok(())
}

/// Process the read list: create input dependencies.
///
/// Port of C tup's update_read_info() (file.c:842+).
/// For each file read during execution:
/// 1. Look up or create the node in the DB
/// 2. Create a normal link from the file to the command
fn update_read_info(
    db: &tup_db::TupDb,
    cmd_id: TupId,
    dir_id: TupId,
    finfo: &FileInfo,
    _result: &mut WriteFilesResult,
) -> anyhow::Result<usize> {
    let mut count = 0;

    for read_file in &finfo.read_list {
        // Skip system paths and hidden files
        if read_file.starts_with("/dev")
            || read_file.starts_with("/proc")
            || read_file.starts_with("/sys")
            || read_file.contains("/.git")
            || read_file.contains("/.tup")
        {
            continue;
        }

        // Look up the node
        let file_name = Path::new(read_file)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();

        if let Ok(Some(node)) = db.node_select(dir_id, &file_name) {
            // Create normal link: file → CMD
            // C: This creates the auto-detected dependency
            let _ = db.link_insert(node.id, cmd_id, LinkType::Normal);
            count += 1;
        }
        // If file not in DB: C would create a ghost node here.
        // Ghost creation will be implemented when ghost lifecycle is complete.
    }

    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_write_files_result_default() {
        let result = WriteFilesResult {
            reads_processed: 0,
            writes_processed: 0,
            warnings: Vec::new(),
            failed: false,
        };
        assert!(!result.failed);
        assert!(result.warnings.is_empty());
    }
}
