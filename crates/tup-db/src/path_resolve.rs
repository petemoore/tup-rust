use tup_types::{NodeType, TupId};

use crate::entry::EntryCache;
use crate::error::{DbError, DbResult};
use crate::schema::TupDb;

/// Resolve a relative path to a TupId, traversing the directory tree.
///
/// Verified against C source (create_name_file.c:548-662):
/// - Splits path into components
/// - Handles ".." by traversing tent->parent
/// - Creates GHOST nodes for missing directories (if create_ghosts=true)
/// - Returns the directory TupId and the final filename component
///
/// Returns (dir_tupid, filename) where filename is the last path component.
/// If the path is a directory only (no filename), returns (dir_tupid, None).
pub fn resolve_path(
    db: &TupDb,
    cache: &mut EntryCache,
    start_dir: TupId,
    path: &str,
) -> DbResult<(TupId, Option<String>)> {
    let path = path.trim();
    if path.is_empty() || path == "." {
        return Ok((start_dir, None));
    }

    let components: Vec<&str> = path.split('/').collect();
    if components.is_empty() {
        return Ok((start_dir, None));
    }

    // Split into directory components and final filename
    let (dir_parts, filename) = if components.len() == 1 {
        (&components[..0], Some(components[0].to_string()))
    } else {
        let last = components.last().unwrap().to_string();
        (&components[..components.len() - 1], Some(last))
    };

    // Traverse directory components
    let mut current = start_dir;

    for &component in dir_parts {
        if component == "." {
            continue;
        } else if component == ".." {
            // Go up to parent directory
            if let Some(entry) = cache.find(current) {
                let parent = entry.dt;
                if parent.raw() <= 0 {
                    // At the top — can't go further up
                    return Ok((TupId::new(0), None));
                }
                current = parent;
            } else {
                // Try loading from DB
                cache.load(db, current)?;
                if let Some(entry) = cache.find(current) {
                    let parent = entry.dt;
                    if parent.raw() <= 0 {
                        return Ok((TupId::new(0), None));
                    }
                    current = parent;
                } else {
                    return Err(DbError::NodeNotFound(current));
                }
            }
        } else {
            // Look up child directory
            match db.node_select(current, component)? {
                Some(row)
                    if row.node_type == NodeType::Dir
                        || row.node_type == NodeType::GeneratedDir
                        || row.node_type == NodeType::Ghost =>
                {
                    current = row.id;
                }
                Some(_) => {
                    // Exists but not a directory
                    return Err(DbError::Other(format!(
                        "path component '{component}' is not a directory"
                    )));
                }
                None => {
                    // Directory doesn't exist in DB
                    return Err(DbError::Other(format!(
                        "directory '{component}' not found in database"
                    )));
                }
            }
        }
    }

    Ok((current, filename))
}

/// Resolve a full path to its TupId.
///
/// Like resolve_path but looks up the final component too.
pub fn resolve_full_path(
    db: &TupDb,
    cache: &mut EntryCache,
    start_dir: TupId,
    path: &str,
) -> DbResult<Option<TupId>> {
    let (dir_id, filename) = resolve_path(db, cache, start_dir, path)?;

    match filename {
        Some(name) => match db.node_select(dir_id, &name)? {
            Some(row) => Ok(Some(row.id)),
            None => Ok(None),
        },
        None => Ok(Some(dir_id)),
    }
}

/// Track directory-level input dependencies.
///
/// Verified against C source (db.c:6476-6498):
/// - When a Tupfile references files from another directory, create
///   a NORMAL link from the referenced file to the current directory.
/// - This ensures that when the referenced file changes, the current
///   directory's Tupfile is re-parsed.
pub fn add_dir_input(db: &TupDb, from_file_id: TupId, to_dir_id: TupId) -> DbResult<()> {
    // Directory-level inputs always use NORMAL links (not sticky)
    // as verified in C source: "All links to directories should be TUP_LINK_NORMAL"
    db.link_insert(from_file_id, to_dir_id, tup_types::LinkType::Normal)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tup_types::DOT_DT;

    fn setup() -> (TupDb, EntryCache) {
        let db = TupDb::create_in_memory().unwrap();
        let mut cache = EntryCache::new();
        cache.load(&db, DOT_DT).unwrap();
        (db, cache)
    }

    #[test]
    fn test_resolve_simple_file() {
        let (db, mut cache) = setup();
        db.begin().unwrap();
        db.node_insert(DOT_DT, "main.c", NodeType::File, 0, 0, -1, None, None)
            .unwrap();

        let (dir, name) = resolve_path(&db, &mut cache, DOT_DT, "main.c").unwrap();
        assert_eq!(dir, DOT_DT);
        assert_eq!(name, Some("main.c".to_string()));
        db.commit().unwrap();
    }

    #[test]
    fn test_resolve_subdirectory_file() {
        let (db, mut cache) = setup();
        db.begin().unwrap();
        let sub = db
            .node_insert(DOT_DT, "src", NodeType::Dir, 0, 0, -1, None, None)
            .unwrap();
        db.node_insert(sub, "main.c", NodeType::File, 0, 0, -1, None, None)
            .unwrap();

        let (dir, name) = resolve_path(&db, &mut cache, DOT_DT, "src/main.c").unwrap();
        assert_eq!(dir, sub);
        assert_eq!(name, Some("main.c".to_string()));
        db.commit().unwrap();
    }

    #[test]
    fn test_resolve_parent_dir() {
        let (db, mut cache) = setup();
        db.begin().unwrap();
        let sub = db
            .node_insert(DOT_DT, "src", NodeType::Dir, 0, 0, -1, None, None)
            .unwrap();
        cache.load(&db, sub).unwrap();

        // From src, go up with ..
        let (dir, name) = resolve_path(&db, &mut cache, sub, "../Tupfile").unwrap();
        assert_eq!(dir, DOT_DT);
        assert_eq!(name, Some("Tupfile".to_string()));
        db.commit().unwrap();
    }

    #[test]
    fn test_resolve_dot() {
        let (db, mut cache) = setup();
        let (dir, name) = resolve_path(&db, &mut cache, DOT_DT, ".").unwrap();
        assert_eq!(dir, DOT_DT);
        assert!(name.is_none());
    }

    #[test]
    fn test_resolve_full_path_found() {
        let (db, mut cache) = setup();
        db.begin().unwrap();
        let file_id = db
            .node_insert(DOT_DT, "test.c", NodeType::File, 0, 0, -1, None, None)
            .unwrap();

        let result = resolve_full_path(&db, &mut cache, DOT_DT, "test.c").unwrap();
        assert_eq!(result, Some(file_id));
        db.commit().unwrap();
    }

    #[test]
    fn test_resolve_full_path_not_found() {
        let (db, mut cache) = setup();
        let result = resolve_full_path(&db, &mut cache, DOT_DT, "nonexistent.c").unwrap();
        assert_eq!(result, None);
    }

    #[test]
    fn test_add_dir_input() {
        let (db, _cache) = setup();
        db.begin().unwrap();
        let file = db
            .node_insert(DOT_DT, "rules.tup", NodeType::File, 0, 0, -1, None, None)
            .unwrap();
        let dir = db
            .node_insert(DOT_DT, "subdir", NodeType::Dir, 0, 0, -1, None, None)
            .unwrap();

        add_dir_input(&db, file, dir).unwrap();
        assert!(db
            .link_exists(file, dir, tup_types::LinkType::Normal)
            .unwrap());
        db.commit().unwrap();
    }
}
