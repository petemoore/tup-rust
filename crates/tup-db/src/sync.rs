use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use tup_types::{NodeType, TupFlags, TupId, DOT_DT};

use crate::entry::{EntryCache, TupEntry};
use crate::error::DbResult;
use crate::schema::{NodeRow, TupDb};

/// Result of syncing the filesystem to the database.
#[derive(Debug, Default)]
pub struct SyncResult {
    /// Number of new files added to the database.
    pub files_added: usize,
    /// Number of files with updated mtimes.
    pub files_modified: usize,
    /// Number of files removed from the database.
    pub files_deleted: usize,
    /// Number of new directories added.
    pub dirs_added: usize,
    /// Number of directories flagged for re-parsing.
    pub dirs_flagged: usize,
}

/// Sync the filesystem state into the database.
///
/// This is the first step of `tup upd`: scan the filesystem and update
/// the node table to reflect current reality. After syncing:
/// - New files have FILE nodes
/// - Modified files have MODIFY flag set
/// - Deleted files are removed (or become ghosts)
/// - Directories with changed Tupfiles have CREATE flag set
///
/// Corresponds to `tup_scan()` in C's path.c.
pub fn sync_filesystem(
    db: &TupDb,
    cache: &mut EntryCache,
    root: &Path,
) -> DbResult<SyncResult> {
    let mut result = SyncResult::default();

    db.begin()?;

    // Load root node into cache
    cache.load(db, DOT_DT)?;

    // Sync the root directory recursively
    sync_dir(db, cache, root, root, DOT_DT, &mut result)?;

    db.commit()?;

    Ok(result)
}

/// Recursively sync a directory and its contents.
#[allow(clippy::only_used_in_recursion)]
fn sync_dir(
    db: &TupDb,
    cache: &mut EntryCache,
    project_root: &Path,
    dir_path: &Path,
    dir_id: TupId,
    result: &mut SyncResult,
) -> DbResult<()> {
    // Read current filesystem contents
    let fs_entries = read_dir_entries(dir_path);

    // Load existing DB nodes for this directory
    let db_nodes = db.node_select_dir(dir_id)?;
    let mut db_map: BTreeMap<String, NodeRow> = BTreeMap::new();
    for node in &db_nodes {
        // Skip virtual directories ($, /, ^)
        if node.name == "$" || node.name == "/" || node.name == "^" {
            continue;
        }
        db_map.insert(node.name.clone(), node.clone());
    }

    let mut seen_names: BTreeSet<String> = BTreeSet::new();

    // Process filesystem entries
    for entry in &fs_entries {
        seen_names.insert(entry.name.clone());

        if entry.is_dir {
            // Handle directory
            let sub_id = match db_map.get(&entry.name) {
                Some(row) if row.node_type == NodeType::Dir || row.node_type == NodeType::GeneratedDir => {
                    row.id
                }
                Some(row) if row.node_type == NodeType::Ghost => {
                    // Ghost → directory
                    db.node_set_type(row.id, NodeType::Dir)?;
                    cache.change_type(row.id, NodeType::Dir);
                    row.id
                }
                Some(_) => continue, // Type conflict, skip
                None => {
                    // New directory
                    let id = db.node_insert(
                        dir_id, &entry.name, NodeType::Dir,
                        -1, 0, -1, None, None,
                    )?;
                    let row = db.node_select_by_id(id)?.unwrap();
                    cache.add(TupEntry::from_node_row(&row));
                    result.dirs_added += 1;
                    id
                }
            };

            // Check if Tupfile exists/changed in this directory
            let sub_path = dir_path.join(&entry.name);
            if has_tupfile(&sub_path) {
                // Flag for parsing if Tupfile is new or modified
                let tupfile_changed = check_tupfile_changed(db, sub_id, &sub_path)?;
                if tupfile_changed {
                    db.flag_add(sub_id, TupFlags::Create)?;
                    result.dirs_flagged += 1;
                }
            }

            // Recurse into subdirectory
            let sub_path = dir_path.join(&entry.name);
            sync_dir(db, cache, project_root, &sub_path, sub_id, result)?;

        } else {
            // Handle file
            match db_map.get(&entry.name) {
                Some(row) => {
                    // Existing file — check mtime
                    if row.node_type == NodeType::Ghost {
                        // Ghost → file
                        db.node_set_type(row.id, NodeType::File)?;
                        db.node_set_mtime(row.id, entry.mtime, entry.mtime_ns)?;
                        db.flag_add(row.id, TupFlags::Modify)?;
                        cache.change_type(row.id, NodeType::File);
                        result.files_modified += 1;
                    } else if row.mtime != entry.mtime || row.mtime_ns != entry.mtime_ns {
                        // Mtime changed
                        db.node_set_mtime(row.id, entry.mtime, entry.mtime_ns)?;
                        db.flag_add(row.id, TupFlags::Modify)?;
                        result.files_modified += 1;

                        // Propagate changes to dependent commands and directories.
                        // In C tup, set_dependent_flags() flags directories for
                        // re-parsing via normal_link. Additionally, we need to flag
                        // commands that use this file as an input (via sticky_link)
                        // so they get re-executed.
                        db.set_dependent_flags(row.id)?;

                        // Flag commands that have this file as a sticky input
                        // (declared dependency from Tupfile rules)
                        let sticky_outputs = db.get_sticky_outputs(row.id)?;
                        for cmd_id in &sticky_outputs {
                            db.flag_add(*cmd_id, TupFlags::Modify)?;
                        }

                        // If this is a Tupfile, flag the directory
                        if is_tupfile_name(&entry.name) {
                            db.flag_add(dir_id, TupFlags::Create)?;
                            result.dirs_flagged += 1;
                        }
                    }
                }
                None => {
                    // New file
                    db.node_insert(
                        dir_id, &entry.name, NodeType::File,
                        entry.mtime, entry.mtime_ns, -1, None, None,
                    )?;
                    result.files_added += 1;

                    // If this is a new Tupfile, flag the directory
                    if is_tupfile_name(&entry.name) {
                        db.flag_add(dir_id, TupFlags::Create)?;
                        result.dirs_flagged += 1;
                    }

                    // Flag parent for re-parsing (new input file)
                    db.flag_add(dir_id, TupFlags::Create)?;
                }
            }
        }
    }

    // Detect deleted entries
    for (name, row) in &db_map {
        if !seen_names.contains(name) {
            // File/dir exists in DB but not on filesystem
            match row.node_type {
                NodeType::File => {
                    db.flag_add(row.id, TupFlags::Modify)?;
                    // Don't delete yet — let the updater handle it
                    // after re-parsing affected Tupfiles
                    result.files_deleted += 1;
                }
                NodeType::Dir => {
                    // Directory deleted — flag for cleanup
                    db.flag_add(row.id, TupFlags::Modify)?;
                    result.files_deleted += 1;
                }
                NodeType::Generated | NodeType::Cmd | NodeType::Group => {
                    // Generated files, commands, and groups are managed
                    // by the parser, not the scanner
                }
                NodeType::Ghost => {
                    // Already a ghost, nothing to do
                }
                _ => {}
            }
        }
    }

    Ok(())
}

/// A filesystem directory entry.
struct DirEntry {
    name: String,
    is_dir: bool,
    mtime: i64,
    mtime_ns: i64,
}

/// Read entries from a directory, filtering hidden files and tup internals.
fn read_dir_entries(dir: &Path) -> Vec<DirEntry> {
    let mut entries = Vec::new();

    let read_dir = match std::fs::read_dir(dir) {
        Ok(rd) => rd,
        Err(_) => return entries,
    };

    for entry in read_dir.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();

        // Skip hidden files and tup-internal directories
        if name.starts_with('.') {
            continue;
        }

        let file_type = match entry.file_type() {
            Ok(ft) => ft,
            Err(_) => continue,
        };

        if file_type.is_dir() {
            entries.push(DirEntry {
                name,
                is_dir: true,
                mtime: -1,
                mtime_ns: 0,
            });
        } else if file_type.is_file() {
            let (mtime, mtime_ns) = entry.metadata()
                .ok()
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| (d.as_secs() as i64, d.subsec_nanos() as i64))
                .unwrap_or((-1, 0));

            entries.push(DirEntry {
                name,
                is_dir: false,
                mtime,
                mtime_ns,
            });
        }
    }

    entries.sort_by(|a, b| a.name.cmp(&b.name));
    entries
}

/// Check if a directory contains a Tupfile or Tupfile.lua.
fn has_tupfile(dir: &Path) -> bool {
    dir.join("Tupfile").exists() || dir.join("Tupfile.lua").exists()
}

/// Check if a filename is a Tupfile.
fn is_tupfile_name(name: &str) -> bool {
    name == "Tupfile" || name == "Tupfile.lua" || name == "Tuprules.tup"
}

/// Check if a Tupfile in a directory has changed since last parse.
fn check_tupfile_changed(db: &TupDb, dir_id: TupId, dir_path: &Path) -> DbResult<bool> {
    for name in &["Tupfile", "Tupfile.lua"] {
        let file_path = dir_path.join(name);
        if !file_path.exists() {
            continue;
        }

        let (mtime, mtime_ns) = std::fs::metadata(&file_path)
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| (d.as_secs() as i64, d.subsec_nanos() as i64))
            .unwrap_or((-1, 0));

        match db.node_select(dir_id, name)? {
            Some(row) => {
                if row.mtime != mtime || row.mtime_ns != mtime_ns {
                    // Tupfile changed — update mtime and flag
                    db.node_set_mtime(row.id, mtime, mtime_ns)?;
                    return Ok(true);
                }
            }
            None => {
                // New Tupfile — create node
                db.node_insert(dir_id, name, NodeType::File, mtime, mtime_ns, -1, None, None)?;
                return Ok(true);
            }
        }
    }

    // Also check if the directory was never parsed (no CREATE flag history)
    let in_create = db.flag_check(dir_id, TupFlags::Create)?;
    if in_create {
        return Ok(true);
    }

    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup_project(tmp: &Path) {
        // Initialize tup
        tup_platform::init::init_command(tmp, false, false).unwrap();
    }

    #[test]
    fn test_sync_empty_project() {
        let tmp = tempfile::tempdir().unwrap();
        setup_project(tmp.path());

        let db = TupDb::open(tmp.path(), false).unwrap();
        let mut cache = EntryCache::new();

        let result = sync_filesystem(&db, &mut cache, tmp.path()).unwrap();
        // Empty project — no files to sync
        assert_eq!(result.files_added, 0);
        assert_eq!(result.files_modified, 0);
    }

    #[test]
    fn test_sync_new_files() {
        let tmp = tempfile::tempdir().unwrap();
        setup_project(tmp.path());
        std::fs::write(tmp.path().join("hello.c"), "int main() {}").unwrap();
        std::fs::write(tmp.path().join("hello.h"), "#pragma once").unwrap();

        let db = TupDb::open(tmp.path(), false).unwrap();
        let mut cache = EntryCache::new();

        let result = sync_filesystem(&db, &mut cache, tmp.path()).unwrap();
        assert_eq!(result.files_added, 2);
    }

    #[test]
    fn test_sync_modified_file() {
        let tmp = tempfile::tempdir().unwrap();
        setup_project(tmp.path());
        std::fs::write(tmp.path().join("file.c"), "v1").unwrap();

        let db = TupDb::open(tmp.path(), false).unwrap();
        let mut cache = EntryCache::new();

        // First sync
        sync_filesystem(&db, &mut cache, tmp.path()).unwrap();

        // Modify the file (change mtime)
        std::thread::sleep(std::time::Duration::from_millis(50));
        std::fs::write(tmp.path().join("file.c"), "v2").unwrap();

        // Second sync should detect the change
        let result = sync_filesystem(&db, &mut cache, tmp.path()).unwrap();
        assert!(result.files_modified > 0);
    }

    #[test]
    fn test_sync_deleted_file() {
        let tmp = tempfile::tempdir().unwrap();
        setup_project(tmp.path());
        std::fs::write(tmp.path().join("temp.c"), "delete me").unwrap();

        let db = TupDb::open(tmp.path(), false).unwrap();
        let mut cache = EntryCache::new();

        // First sync — file exists
        sync_filesystem(&db, &mut cache, tmp.path()).unwrap();

        // Delete the file
        std::fs::remove_file(tmp.path().join("temp.c")).unwrap();

        // Second sync should detect deletion
        let result = sync_filesystem(&db, &mut cache, tmp.path()).unwrap();
        assert_eq!(result.files_deleted, 1);
    }

    #[test]
    fn test_sync_new_directory() {
        let tmp = tempfile::tempdir().unwrap();
        setup_project(tmp.path());
        std::fs::create_dir(tmp.path().join("src")).unwrap();
        std::fs::write(tmp.path().join("src/main.c"), "").unwrap();

        let db = TupDb::open(tmp.path(), false).unwrap();
        let mut cache = EntryCache::new();

        let result = sync_filesystem(&db, &mut cache, tmp.path()).unwrap();
        assert_eq!(result.dirs_added, 1);
        assert_eq!(result.files_added, 1);
    }

    #[test]
    fn test_sync_tupfile_flags_directory() {
        let tmp = tempfile::tempdir().unwrap();
        setup_project(tmp.path());
        std::fs::write(tmp.path().join("Tupfile"), ": |> echo hi |>\n").unwrap();

        let db = TupDb::open(tmp.path(), false).unwrap();
        let mut cache = EntryCache::new();

        let result = sync_filesystem(&db, &mut cache, tmp.path()).unwrap();
        assert!(result.dirs_flagged > 0);
    }

    #[test]
    fn test_sync_subdirectory_tupfile() {
        let tmp = tempfile::tempdir().unwrap();
        setup_project(tmp.path());
        std::fs::create_dir(tmp.path().join("lib")).unwrap();
        std::fs::write(tmp.path().join("lib/Tupfile"), ": |> echo lib |>\n").unwrap();
        std::fs::write(tmp.path().join("lib/helper.c"), "").unwrap();

        let db = TupDb::open(tmp.path(), false).unwrap();
        let mut cache = EntryCache::new();

        let result = sync_filesystem(&db, &mut cache, tmp.path()).unwrap();
        assert!(result.dirs_added >= 1);
        assert!(result.dirs_flagged >= 1);
        assert!(result.files_added >= 1);
    }

    #[test]
    fn test_sync_no_change_second_run() {
        let tmp = tempfile::tempdir().unwrap();
        setup_project(tmp.path());
        std::fs::write(tmp.path().join("stable.c"), "no change").unwrap();

        let db = TupDb::open(tmp.path(), false).unwrap();
        let mut cache = EntryCache::new();

        // First sync
        sync_filesystem(&db, &mut cache, tmp.path()).unwrap();

        // Second sync — nothing changed
        let result = sync_filesystem(&db, &mut cache, tmp.path()).unwrap();
        assert_eq!(result.files_added, 0);
        assert_eq!(result.files_modified, 0);
        assert_eq!(result.files_deleted, 0);
    }

    #[test]
    fn test_sync_skips_hidden_files() {
        let tmp = tempfile::tempdir().unwrap();
        setup_project(tmp.path());
        std::fs::write(tmp.path().join(".hidden"), "secret").unwrap();
        std::fs::write(tmp.path().join("visible.c"), "public").unwrap();

        let db = TupDb::open(tmp.path(), false).unwrap();
        let mut cache = EntryCache::new();

        let result = sync_filesystem(&db, &mut cache, tmp.path()).unwrap();
        assert_eq!(result.files_added, 1); // Only visible.c
    }
}
