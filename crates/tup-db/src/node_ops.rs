use tup_types::{NodeType, TupFlags, TupId};

use crate::entry::{EntryCache, TupEntry};
use crate::error::{DbError, DbResult};
use crate::schema::TupDb;

/// Result of a node creation attempt.
#[derive(Debug)]
pub enum CreateResult {
    /// A new node was created.
    Created(TupId),
    /// An existing node was found (possibly upgraded from ghost).
    Existing(TupId),
}

impl CreateResult {
    /// Get the TupId regardless of whether it was created or existing.
    pub fn id(&self) -> TupId {
        match self {
            CreateResult::Created(id) | CreateResult::Existing(id) => *id,
        }
    }
}

/// High-level node operations that combine database mutations with
/// cache updates and flag management.
///
/// These correspond to create_name_file.c and delete_name_file.c in C.
impl TupDb {
    /// Create a file node in the database.
    ///
    /// Corresponds to `create_name_file()` in C:
    /// 1. Insert the node (or upgrade ghost)
    /// 2. Add parent directory to create_list
    /// 3. Convert any GeneratedDir parents to Dir
    ///
    /// Returns the TupId of the new or upgraded node.
    pub fn create_name_file(
        &self,
        cache: &mut EntryCache,
        dir: TupId,
        name: &str,
        mtime: i64,
        mtime_ns: i64,
    ) -> DbResult<CreateResult> {
        let result = self.create_node(cache, dir, name, NodeType::File, -1, mtime, mtime_ns)?;

        // Add parent directory to create_list
        self.flag_add(dir, TupFlags::Create)?;

        // Convert GeneratedDir parents to Dir
        self.make_dirs_normal(cache, dir)?;

        Ok(result)
    }

    /// Create a command node in the database.
    ///
    /// Corresponds to `create_command_file()` in C.
    pub fn create_command_file(
        &self,
        cache: &mut EntryCache,
        dir: TupId,
        cmd: &str,
        display: Option<&str>,
        flags: Option<&str>,
    ) -> DbResult<TupId> {
        let result = self.create_node_with_display(
            cache, dir, cmd, NodeType::Cmd, -1, -1, 0, display, flags,
        )?;
        Ok(result.id())
    }

    /// Create a node, handling ghost upgrades and type conflicts.
    ///
    /// This corresponds to `tup_db_create_node_part_display()` in C.
    ///
    /// Behavior:
    /// 1. If no existing node: insert new node
    /// 2. If existing ghost: upgrade to requested type
    /// 3. If existing node with same type: return existing
    /// 4. If existing node with different type: error
    #[allow(clippy::too_many_arguments)]
    pub fn create_node(
        &self,
        cache: &mut EntryCache,
        dir: TupId,
        name: &str,
        node_type: NodeType,
        srcid: i64,
        mtime: i64,
        mtime_ns: i64,
    ) -> DbResult<CreateResult> {
        self.create_node_with_display(cache, dir, name, node_type, srcid, mtime, mtime_ns, None, None)
    }

    /// Create a node with display and flags, handling ghost upgrades.
    #[allow(clippy::too_many_arguments)]
    pub fn create_node_with_display(
        &self,
        cache: &mut EntryCache,
        dir: TupId,
        name: &str,
        node_type: NodeType,
        srcid: i64,
        mtime: i64,
        mtime_ns: i64,
        display: Option<&str>,
        flags: Option<&str>,
    ) -> DbResult<CreateResult> {
        // Check if a node with this name already exists in the directory
        let existing = self.node_select(dir, name)?;

        match existing {
            None => {
                // No existing node — create new
                let id = self.node_insert(dir, name, node_type, mtime, mtime_ns, srcid, display, flags)?;

                // Add to cache
                let row = self.node_select_by_id(id)?.unwrap();
                cache.add(TupEntry::from_node_row(&row));

                // New commands go to modify_list
                if node_type == NodeType::Cmd {
                    self.flag_add(id, TupFlags::Modify)?;
                }
                // New directories go to create_list
                if node_type == NodeType::Dir {
                    self.flag_add(id, TupFlags::Create)?;
                }

                Ok(CreateResult::Created(id))
            }
            Some(row) if row.node_type == NodeType::Ghost => {
                // Ghost exists — upgrade it
                let id = row.id;
                self.ghost_to_type(cache, id, node_type)?;

                // Update mtime if provided
                if mtime != -1 {
                    self.node_set_mtime(id, mtime, mtime_ns)?;
                }
                // Update srcid if provided
                if srcid != -1 {
                    self.node_set_srcid(id, srcid)?;
                }
                // Update display and flags
                if display.is_some() {
                    self.node_set_display(id, display)?;
                }
                if flags.is_some() {
                    self.node_set_flags(id, flags)?;
                }

                // Refresh cache
                if let Some(fresh) = self.node_select_by_id(id)? {
                    // Remove old entry and add fresh one
                    let _ = cache.remove(id);
                    cache.add(TupEntry::from_node_row(&fresh));
                }

                Ok(CreateResult::Existing(id))
            }
            Some(row) if row.node_type == node_type => {
                // Same type exists — return existing (update metadata if needed)
                let id = row.id;

                if srcid != -1 && row.srcid != srcid {
                    self.node_set_srcid(id, srcid)?;
                }
                if display.is_some() {
                    self.node_set_display(id, display)?;
                }
                if flags.is_some() {
                    self.node_set_flags(id, flags)?;
                }

                // Refresh cache entry if it was updated
                if let Some(fresh) = self.node_select_by_id(id)? {
                    let _ = cache.remove(id);
                    cache.add(TupEntry::from_node_row(&fresh));
                }

                Ok(CreateResult::Existing(id))
            }
            Some(row) => {
                // Different non-ghost type exists — this is a conflict
                Err(DbError::Other(format!(
                    "cannot create {} '{}' in dir {}: already exists as {}",
                    node_type, name, dir, row.node_type
                )))
            }
        }
    }

    /// Delete a node and clean up all references.
    ///
    /// Corresponds to `delete_name_file()` in C:
    /// 1. Remove from all flag lists
    /// 2. Delete all links
    /// 3. Delete the node itself
    pub fn delete_name_file(
        &self,
        cache: &mut EntryCache,
        id: TupId,
    ) -> DbResult<()> {
        // Remove from all flag lists
        self.flag_remove(id, TupFlags::Config)?;
        self.flag_remove(id, TupFlags::Create)?;
        self.flag_remove(id, TupFlags::Modify)?;
        self.flag_remove(id, TupFlags::Variant)?;
        self.flag_remove(id, TupFlags::Transient)?;

        // Delete all links
        self.links_delete(id)?;

        // Delete the node
        self.node_delete(id)?;

        // Remove from cache
        let _ = cache.remove(id);

        Ok(())
    }

    /// Convert a ghost node to a file node.
    ///
    /// Corresponds to `ghost_to_file()` in C.
    fn ghost_to_type(
        &self,
        cache: &mut EntryCache,
        id: TupId,
        target_type: NodeType,
    ) -> DbResult<()> {
        self.node_set_type(id, target_type)?;
        cache.change_type(id, target_type);

        // Add to modify list
        self.flag_add(id, TupFlags::Modify)?;

        // If parent is a regular Dir, add it to create list
        if let Some(entry) = cache.find(id) {
            let parent_id = entry.dt;
            if let Some(parent) = cache.find(parent_id) {
                if parent.node_type == NodeType::Dir {
                    self.flag_add(parent_id, TupFlags::Create)?;
                }
            }
        }

        Ok(())
    }

    /// Convert GeneratedDir parents to regular Dir.
    ///
    /// Corresponds to `make_dirs_normal()` in C.
    /// Walks up the parent chain converting any GeneratedDir to Dir.
    fn make_dirs_normal(
        &self,
        cache: &mut EntryCache,
        dir: TupId,
    ) -> DbResult<()> {
        let mut current = dir;

        while let Some(entry) = cache.find(current) {
            if entry.node_type != NodeType::GeneratedDir {
                break;
            }

            let parent = entry.dt;
            self.node_set_type(current, NodeType::Dir)?;
            cache.change_type(current, NodeType::Dir);
            current = parent;
        }

        Ok(())
    }

    /// Handle file modification (new or changed).
    ///
    /// Simplified version of `tup_file_mod_mtime()` in C.
    /// - If node doesn't exist: create it
    /// - If node is a ghost: upgrade to file
    /// - If mtime changed: mark as modified and update dependents
    pub fn file_mod(
        &self,
        cache: &mut EntryCache,
        dir: TupId,
        name: &str,
        mtime: i64,
        mtime_ns: i64,
    ) -> DbResult<(TupId, bool)> {
        let existing = self.node_select(dir, name)?;
        let mut modified = false;

        match existing {
            None => {
                // New file
                let result = self.create_name_file(cache, dir, name, mtime, mtime_ns)?;
                modified = true;
                Ok((result.id(), modified))
            }
            Some(row) if row.node_type == NodeType::Ghost => {
                // Ghost → file
                self.ghost_to_type(cache, row.id, NodeType::File)?;
                self.node_set_mtime(row.id, mtime, mtime_ns)?;
                modified = true;
                Ok((row.id, modified))
            }
            Some(row) => {
                // Existing file — check if mtime changed
                if row.mtime != mtime || row.mtime_ns != mtime_ns {
                    self.node_set_mtime(row.id, mtime, mtime_ns)?;
                    self.flag_add(row.id, TupFlags::Modify)?;
                    modified = true;
                }
                Ok((row.id, modified))
            }
        }
    }

    /// Handle file deletion.
    ///
    /// Simplified version of `tup_file_del()` in C.
    /// If the file isn't in the database, that's OK (may have been
    /// created and deleted before the monitor could track it).
    pub fn file_del(
        &self,
        cache: &mut EntryCache,
        dir: TupId,
        name: &str,
    ) -> DbResult<bool> {
        let existing = self.node_select(dir, name)?;

        match existing {
            None => Ok(false), // Not tracked, nothing to do
            Some(row) => {
                let id = row.id;
                let node_type = row.node_type;

                match node_type {
                    NodeType::Ghost | NodeType::Group => {
                        // Don't delete ghosts or groups
                        Ok(false)
                    }
                    NodeType::File | NodeType::Generated => {
                        // Mark dependent commands
                        // (In C, this calls tup_db_set_dependent_flags and
                        // tup_db_modify_cmds_by_input — simplified here)
                        self.flag_add(dir, TupFlags::Create)?;
                        self.delete_name_file(cache, id)?;
                        Ok(true)
                    }
                    NodeType::Dir | NodeType::GeneratedDir => {
                        // Directory deletion — just delete
                        self.delete_name_file(cache, id)?;
                        Ok(true)
                    }
                    _ => {
                        self.delete_name_file(cache, id)?;
                        Ok(true)
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tup_types::DOT_DT;

    fn setup() -> (TupDb, EntryCache) {
        let db = TupDb::create_in_memory().unwrap();
        let mut cache = EntryCache::new();
        // Load root node into cache
        cache.load(&db, DOT_DT).unwrap();
        (db, cache)
    }

    #[test]
    fn test_create_name_file() {
        let (db, mut cache) = setup();
        db.begin().unwrap();

        let result = db.create_name_file(&mut cache, DOT_DT, "hello.c", 1000, 500).unwrap();
        let id = result.id();

        // Verify in database
        let node = db.node_select(DOT_DT, "hello.c").unwrap().unwrap();
        assert_eq!(node.id, id);
        assert_eq!(node.node_type, NodeType::File);
        assert_eq!(node.mtime, 1000);
        assert_eq!(node.mtime_ns, 500);

        // Verify in cache
        assert!(cache.find(id).is_some());
        assert_eq!(cache.find(id).unwrap().name, "hello.c");

        // Verify parent was added to create_list
        assert!(db.flag_check(DOT_DT, TupFlags::Create).unwrap());

        db.commit().unwrap();
    }

    #[test]
    fn test_create_name_file_ghost_upgrade() {
        let (db, mut cache) = setup();
        db.begin().unwrap();

        // Create a ghost first
        let ghost_id = db.node_insert(DOT_DT, "phantom.c", NodeType::Ghost, -1, 0, -1, None, None).unwrap();
        let row = db.node_select_by_id(ghost_id).unwrap().unwrap();
        cache.add(TupEntry::from_node_row(&row));

        // Now create_name_file should upgrade the ghost
        let result = db.create_name_file(&mut cache, DOT_DT, "phantom.c", 2000, 0).unwrap();
        assert!(matches!(result, CreateResult::Existing(_)));
        assert_eq!(result.id(), ghost_id);

        // Verify it's now a file
        let node = db.node_select_by_id(ghost_id).unwrap().unwrap();
        assert_eq!(node.node_type, NodeType::File);

        // Verify in cache
        assert_eq!(cache.find(ghost_id).unwrap().node_type, NodeType::File);

        db.commit().unwrap();
    }

    #[test]
    fn test_create_command_file() {
        let (db, mut cache) = setup();
        db.begin().unwrap();

        let id = db.create_command_file(
            &mut cache, DOT_DT, "gcc -c foo.c -o foo.o",
            Some("CC foo.c"), Some("j"),
        ).unwrap();

        let node = db.node_select_by_id(id).unwrap().unwrap();
        assert_eq!(node.node_type, NodeType::Cmd);
        assert_eq!(node.display, Some("CC foo.c".to_string()));
        assert_eq!(node.flags, Some("j".to_string()));

        // Commands should be in modify_list
        assert!(db.flag_check(id, TupFlags::Modify).unwrap());

        db.commit().unwrap();
    }

    #[test]
    fn test_create_node_same_type_returns_existing() {
        let (db, mut cache) = setup();
        db.begin().unwrap();

        let r1 = db.create_node(&mut cache, DOT_DT, "file.c", NodeType::File, -1, 100, 0).unwrap();
        let r2 = db.create_node(&mut cache, DOT_DT, "file.c", NodeType::File, -1, 200, 0).unwrap();

        assert_eq!(r1.id(), r2.id());
        assert!(matches!(r1, CreateResult::Created(_)));
        assert!(matches!(r2, CreateResult::Existing(_)));

        db.commit().unwrap();
    }

    #[test]
    fn test_create_node_different_type_errors() {
        let (db, mut cache) = setup();
        db.begin().unwrap();

        db.create_node(&mut cache, DOT_DT, "thing", NodeType::File, -1, 0, 0).unwrap();
        let result = db.create_node(&mut cache, DOT_DT, "thing", NodeType::Dir, -1, 0, 0);
        assert!(result.is_err());

        db.rollback().unwrap();
    }

    #[test]
    fn test_create_node_dir_gets_create_flag() {
        let (db, mut cache) = setup();
        db.begin().unwrap();

        let result = db.create_node(&mut cache, DOT_DT, "subdir", NodeType::Dir, -1, 0, 0).unwrap();
        assert!(db.flag_check(result.id(), TupFlags::Create).unwrap());

        db.commit().unwrap();
    }

    #[test]
    fn test_delete_name_file() {
        let (db, mut cache) = setup();
        db.begin().unwrap();

        // Create a file with links and flags
        let file_id = db.create_name_file(&mut cache, DOT_DT, "test.c", 0, 0).unwrap().id();
        let cmd_id = db.create_command_file(&mut cache, DOT_DT, "gcc test.c", None, None).unwrap();
        db.link_insert(file_id, cmd_id, tup_types::LinkType::Normal).unwrap();
        db.flag_add(file_id, TupFlags::Modify).unwrap();

        // Delete the file
        db.delete_name_file(&mut cache, file_id).unwrap();

        // Verify it's gone from DB
        assert!(db.node_select_by_id(file_id).unwrap().is_none());

        // Verify links are gone
        assert!(!db.link_exists(file_id, cmd_id, tup_types::LinkType::Normal).unwrap());

        // Verify flags are gone
        assert!(!db.flag_check(file_id, TupFlags::Modify).unwrap());

        // Verify gone from cache
        assert!(cache.find(file_id).is_none());

        db.commit().unwrap();
    }

    #[test]
    fn test_file_mod_new_file() {
        let (db, mut cache) = setup();
        db.begin().unwrap();

        let (id, modified) = db.file_mod(&mut cache, DOT_DT, "new.c", 5000, 0).unwrap();
        assert!(modified);

        let node = db.node_select_by_id(id).unwrap().unwrap();
        assert_eq!(node.name, "new.c");
        assert_eq!(node.mtime, 5000);

        db.commit().unwrap();
    }

    #[test]
    fn test_file_mod_existing_changed() {
        let (db, mut cache) = setup();
        db.begin().unwrap();

        let (id, _) = db.file_mod(&mut cache, DOT_DT, "file.c", 1000, 0).unwrap();

        // Modify with new mtime
        let (id2, modified) = db.file_mod(&mut cache, DOT_DT, "file.c", 2000, 0).unwrap();
        assert_eq!(id, id2);
        assert!(modified);
        assert!(db.flag_check(id, TupFlags::Modify).unwrap());

        db.commit().unwrap();
    }

    #[test]
    fn test_file_mod_existing_unchanged() {
        let (db, mut cache) = setup();
        db.begin().unwrap();

        let (id, _) = db.file_mod(&mut cache, DOT_DT, "file.c", 1000, 0).unwrap();

        // Same mtime — no modification
        let (id2, modified) = db.file_mod(&mut cache, DOT_DT, "file.c", 1000, 0).unwrap();
        assert_eq!(id, id2);
        assert!(!modified);

        db.commit().unwrap();
    }

    #[test]
    fn test_file_mod_ghost_upgrade() {
        let (db, mut cache) = setup();
        db.begin().unwrap();

        // Create a ghost
        let ghost_id = db.node_insert(DOT_DT, "ghost.c", NodeType::Ghost, -1, 0, -1, None, None).unwrap();
        let row = db.node_select_by_id(ghost_id).unwrap().unwrap();
        cache.add(TupEntry::from_node_row(&row));

        // file_mod should upgrade the ghost
        let (id, modified) = db.file_mod(&mut cache, DOT_DT, "ghost.c", 3000, 0).unwrap();
        assert_eq!(id, ghost_id);
        assert!(modified);

        let node = db.node_select_by_id(id).unwrap().unwrap();
        assert_eq!(node.node_type, NodeType::File);

        db.commit().unwrap();
    }

    #[test]
    fn test_file_del_existing() {
        let (db, mut cache) = setup();
        db.begin().unwrap();

        db.create_name_file(&mut cache, DOT_DT, "delete_me.c", 0, 0).unwrap();
        let deleted = db.file_del(&mut cache, DOT_DT, "delete_me.c").unwrap();
        assert!(deleted);

        assert!(db.node_select(DOT_DT, "delete_me.c").unwrap().is_none());

        db.commit().unwrap();
    }

    #[test]
    fn test_file_del_nonexistent() {
        let (db, mut cache) = setup();
        db.begin().unwrap();

        let deleted = db.file_del(&mut cache, DOT_DT, "nonexistent.c").unwrap();
        assert!(!deleted);

        db.commit().unwrap();
    }

    #[test]
    fn test_file_del_ghost_not_deleted() {
        let (db, mut cache) = setup();
        db.begin().unwrap();

        let ghost_id = db.node_insert(DOT_DT, "ghost.c", NodeType::Ghost, -1, 0, -1, None, None).unwrap();
        let row = db.node_select_by_id(ghost_id).unwrap().unwrap();
        cache.add(TupEntry::from_node_row(&row));

        let deleted = db.file_del(&mut cache, DOT_DT, "ghost.c").unwrap();
        assert!(!deleted); // Ghosts should not be deleted

        // Ghost should still exist
        assert!(db.node_select_by_id(ghost_id).unwrap().is_some());

        db.commit().unwrap();
    }

    #[test]
    fn test_make_dirs_normal() {
        let (db, mut cache) = setup();
        db.begin().unwrap();

        // Create a GeneratedDir
        let gendir_id = db.node_insert(DOT_DT, "gendir", NodeType::GeneratedDir, -1, 0, -1, None, None).unwrap();
        let row = db.node_select_by_id(gendir_id).unwrap().unwrap();
        cache.add(TupEntry::from_node_row(&row));

        // Create a file inside it — should convert parent to Dir
        db.create_name_file(&mut cache, gendir_id, "output.o", 0, 0).unwrap();

        let node = db.node_select_by_id(gendir_id).unwrap().unwrap();
        assert_eq!(node.node_type, NodeType::Dir);
        assert_eq!(cache.find(gendir_id).unwrap().node_type, NodeType::Dir);

        db.commit().unwrap();
    }
}
