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
//! - add_node_to_tree() (334-397) — resolve path, create ghosts

use std::os::unix::fs::MetadataExt;
use std::path::Path;

use tup_db::{resolve_path, EntryCache};
use tup_types::{LinkType, NodeType, TupId, DOT_DT};

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
pub fn write_files(
    db: &tup_db::TupDb,
    cmd_id: TupId,
    _dir_id: TupId,
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
    // C: handle_unlink(info) (file.c:242)
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
    result.writes_processed = update_write_info(db, cmd_id, finfo, tup_top, &mut result)?;

    // C: Rename temporary files to their real destinations (file.c:814-833)
    finalize_mappings(db, finfo, tup_top)?;

    // Process reads (inputs)
    // C: update_read_info(f, cmdid, info, full_deps, vardt, important_link_removed)
    result.reads_processed = update_read_info(db, cmd_id, _dir_id, finfo, &mut result)?;

    Ok(result)
}

/// Process the write list: verify outputs match declarations.
///
/// Port of C tup's update_write_info() (file.c:693-841).
/// For each file written during execution:
/// 1. Resolve the file path to find its directory in the DB
/// 2. Look up the node — must be a declared output (Generated type)
/// 3. If not found, report as undeclared output (error)
fn update_write_info(
    db: &tup_db::TupDb,
    cmd_id: TupId,
    finfo: &FileInfo,
    tup_top: &Path,
    result: &mut WriteFilesResult,
) -> anyhow::Result<usize> {
    let mut count = 0;
    let mut cache = EntryCache::new();
    cache.load(db, DOT_DT)?;

    // C: Remove duplicate write entries (file.c:734-739)
    let mut seen = std::collections::HashSet::new();
    let write_list: Vec<String> = finfo
        .write_list
        .iter()
        .filter(|w| seen.insert((*w).clone()))
        .cloned()
        .collect();

    for written_file in &write_list {
        // Skip system paths
        if written_file.starts_with("/dev")
            || written_file.starts_with("/proc")
            || written_file.starts_with("/sys")
        {
            continue;
        }

        // Skip hidden files (C: get_path_elements → PG_HIDDEN check, file.c:713-720)
        if crate::tup_fuse::TupFuseFs::is_hidden(written_file) {
            result.warnings.push(format!(
                "tup warning: Writing to hidden file '{written_file}'"
            ));
            continue;
        }

        // C: Resolve path from DOT_DT to find the directory and filename
        // (file.c:741-756: find_dir_tupid_dt_pg → tup_db_select_tent_part)
        let node = match resolve_path(db, &mut cache, DOT_DT, written_file) {
            Ok((dir_id, Some(file_name))) => db.node_select(dir_id, &file_name).ok().flatten(),
            _ => None,
        };

        if let Some(node) = node {
            if node.node_type == NodeType::Generated {
                // Declared output — create CMD→output link
                let _ = db.link_insert(cmd_id, node.id, LinkType::Normal);
                count += 1;
            }
        } else {
            // C: File not in DB → undeclared output error (file.c:757-772)
            eprintln!(
                "tup error: File '{}' was written to, but is not in .tup/db. \
                 You probably should specify it as an output",
                written_file
            );
            result.failed = true;
            // C: Delete the undeclared output and its mapping (do_unlink)
            let undeclared_path = tup_top.join(written_file);
            let _ = std::fs::remove_file(&undeclared_path);
            if let Some(mapping) = finfo.mappings.get(written_file.as_str()) {
                let _ = std::fs::remove_file(&mapping.tmpname);
            }
        }
    }

    Ok(count)
}

/// Rename temporary files to their real destinations and update mtimes.
///
/// Port of C tup's mapping finalization (file.c:814-833).
/// After write_files validates outputs, move each .tup/tmp/hex file
/// to its real output path and update the mtime in the DB.
fn finalize_mappings(
    db: &tup_db::TupDb,
    finfo: &mut FileInfo,
    tup_top: &Path,
) -> anyhow::Result<()> {
    let mut cache = EntryCache::new();
    cache.load(db, DOT_DT)?;

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

        // C: file_set_mtime(map->tent, map->realname) (file.c:827-831)
        // Update the DB node's mtime from the actual file
        if let Ok((dir_id, Some(file_name))) =
            resolve_path(db, &mut cache, DOT_DT, &mapping.realname)
        {
            if let Ok(Some(node)) = db.node_select(dir_id, &file_name) {
                if let Ok(meta) = std::fs::symlink_metadata(&dest) {
                    let _ = db.node_set_mtime(node.id, meta.mtime(), meta.mtime_nsec());
                }
            }
        }
    }
    Ok(())
}

/// Process the read list: create input dependencies.
///
/// Port of C tup's update_read_info() (file.c:842-876).
/// For each file read during execution:
/// 1. Resolve the path from DOT_DT (handles cross-directory reads)
/// 2. If the file exists in DB, create a normal link from file → CMD
/// 3. If the file doesn't exist, create a GHOST node and link it
///    (C: add_node_to_tree with SOTGV_CREATE_GHOSTS, file.c:346)
fn update_read_info(
    db: &tup_db::TupDb,
    cmd_id: TupId,
    _dir_id: TupId,
    finfo: &FileInfo,
    _result: &mut WriteFilesResult,
) -> anyhow::Result<usize> {
    let mut count = 0;
    let mut cache = EntryCache::new();
    cache.load(db, DOT_DT)?;

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

        // C: Resolve path from DOT_DT (file.c:856: add_node_to_tree(DOT_DT, ...))
        let (dir_id, file_name) = match resolve_path(db, &mut cache, DOT_DT, read_file) {
            Ok((dir_id, Some(name))) => (dir_id, name),
            Ok((_, None)) => continue, // Directory entry only (e.g. ".")
            Err(_) => continue,        // Can't resolve path — skip
        };

        // C: tup_db_select_tent_part(dtent, pel->path, pel->len, &tent)
        let node_id = match db.node_select(dir_id, &file_name) {
            Ok(Some(node)) => {
                // C: Skip directory nodes (file.c:382-391)
                if node.node_type == NodeType::Dir || node.node_type == NodeType::GeneratedDir {
                    continue;
                }
                node.id
            }
            Ok(None) => {
                // C: Create a ghost node (file.c:363-378)
                // Ghost nodes represent files that were read but don't exist in the DB yet.
                // When the file is later created, the ghost gets upgraded to a real node.
                match db.node_insert(
                    dir_id,
                    &file_name,
                    NodeType::Ghost,
                    -1, // INVALID_MTIME
                    0,
                    -1,
                    None,
                    None,
                ) {
                    Ok(ghost_id) => ghost_id,
                    Err(_) => continue,
                }
            }
            Err(_) => continue,
        };

        // Create normal link: file → CMD
        // C: tent_tree_add_dup(&root, tent) then tup_db_check_actual_inputs()
        let _ = db.link_insert(node_id, cmd_id, LinkType::Normal);
        count += 1;
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
