use rusqlite::params;
use tup_types::{NodeType, TupId};

use crate::error::DbResult;
use crate::schema::TupDb;

/// Ghost reclamation — delete ghost nodes that are no longer referenced.
///
/// Verified against C source (db.c:7080-7177):
/// - Called once per commit in tup_db_commit()
/// - Multi-pass: deleting a child ghost may make parent reclaimable
/// - tup.config ghosts are NEVER reclaimed
/// - Reclaimable conditions:
///   - No nodes have this as their dir (no children)
///   - No nodes have this as their srcid (no source refs)
///   - No normal_links originate from this node
///   - For groups: also no sticky_links and no links to/from
impl TupDb {
    /// Reclaim all reclaimable ghost nodes.
    ///
    /// Called at commit time. Returns the number of ghosts reclaimed.
    pub fn reclaim_ghosts(&self, ghost_candidates: &mut Vec<TupId>) -> DbResult<usize> {
        let mut total_reclaimed = 0;

        // Multi-pass: keep going until no more ghosts can be reclaimed
        loop {
            let mut reclaimed_this_pass = 0;
            let mut next_candidates = Vec::new();

            for &id in ghost_candidates.iter() {
                if let Some(row) = self.node_select_by_id(id)? {
                    // Only reclaim GHOST, GROUP, or GENERATED_DIR
                    match row.node_type {
                        NodeType::Ghost | NodeType::Group | NodeType::GeneratedDir => {}
                        _ => continue,
                    }

                    // Special case: tup.config ghosts are NEVER reclaimed
                    if row.name == "tup.config" {
                        continue;
                    }

                    if self.is_ghost_reclaimable(id, row.node_type)? {
                        // Record parent for re-check in next pass
                        let parent_id = row.dir;
                        if parent_id != TupId::new(0) {
                            next_candidates.push(parent_id);
                        }

                        // Delete the ghost
                        self.delete_ghost_node(id)?;
                        reclaimed_this_pass += 1;
                    }
                }
            }

            total_reclaimed += reclaimed_this_pass;

            if reclaimed_this_pass == 0 || next_candidates.is_empty() {
                break;
            }

            // Next pass: check parents that may now be reclaimable
            *ghost_candidates = next_candidates;
        }

        Ok(total_reclaimed)
    }

    /// Check if a ghost node can be safely reclaimed.
    ///
    /// Verified against C source (db.c:7150-7177, ghost_reclaimable1/2):
    /// - No children (dir column) or srcid references
    /// - No normal_link from this node
    /// - For groups: also no sticky_link and no links to/from
    fn is_ghost_reclaimable(&self, id: TupId, node_type: NodeType) -> DbResult<bool> {
        // Condition 1: No nodes reference this via dir or srcid
        let has_refs: bool = self.conn().query_row(
            "SELECT EXISTS(SELECT 1 FROM node WHERE dir=?1 OR srcid=?1)",
            params![id.raw()],
            |row| row.get(0),
        )?;

        if has_refs {
            return Ok(false);
        }

        // Condition 2: No normal_links originate from this node
        let has_normal_links: bool = self.conn().query_row(
            "SELECT EXISTS(SELECT 1 FROM normal_link WHERE from_id=?1)",
            params![id.raw()],
            |row| row.get(0),
        )?;

        if has_normal_links {
            return Ok(false);
        }

        // For groups: additional checks
        if node_type == NodeType::Group {
            // No normal links to/from
            let has_group_links: bool = self.conn().query_row(
                "SELECT EXISTS(SELECT 1 FROM normal_link WHERE from_id=?1 OR to_id=?1)",
                params![id.raw()],
                |row| row.get(0),
            )?;
            if has_group_links {
                return Ok(false);
            }

            // No sticky links from
            let has_sticky: bool = self.conn().query_row(
                "SELECT EXISTS(SELECT 1 FROM sticky_link WHERE from_id=?1)",
                params![id.raw()],
                |row| row.get(0),
            )?;
            if has_sticky {
                return Ok(false);
            }
        }

        Ok(true)
    }

    /// Delete a ghost node — remove from all flag lists, links, and node table.
    fn delete_ghost_node(&self, id: TupId) -> DbResult<()> {
        // Remove from all flag lists
        for table in &[
            "config_list",
            "create_list",
            "modify_list",
            "variant_list",
            "transient_list",
        ] {
            self.conn().execute(
                &format!("DELETE FROM {table} WHERE id=?1"),
                params![id.raw()],
            )?;
        }

        // Remove all links
        self.links_delete(id)?;

        // Delete the node
        self.node_delete(id)?;

        Ok(())
    }

    /// Convert a node to ghost (instead of deleting) when it still has references.
    ///
    /// Verified against C source (db.c:1528-1580):
    /// - If node has children (dir) or srcid refs → convert to GHOST
    /// - If no refs → delete immediately
    pub fn delete_or_ghost(&self, id: TupId) -> DbResult<bool> {
        // Check if anything references this node
        let has_refs: bool = self.conn().query_row(
            "SELECT EXISTS(SELECT 1 FROM node WHERE dir=?1 OR srcid=?1)",
            params![id.raw()],
            |row| row.get(0),
        )?;

        if has_refs {
            // Convert to ghost instead of deleting
            self.node_set_type(id, NodeType::Ghost)?;
            self.node_set_srcid(id, -1)?;
            Ok(false) // Not fully deleted
        } else {
            // Safe to delete
            self.delete_ghost_node(id)?;
            Ok(true) // Fully deleted
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tup_types::DOT_DT;

    #[test]
    fn test_ghost_reclaimable_no_refs() {
        let db = TupDb::create_in_memory().unwrap();
        db.begin().unwrap();

        let ghost_id = db
            .node_insert(DOT_DT, "phantom", NodeType::Ghost, -1, 0, -1, None, None)
            .unwrap();

        assert!(db.is_ghost_reclaimable(ghost_id, NodeType::Ghost).unwrap());
        db.commit().unwrap();
    }

    #[test]
    fn test_ghost_not_reclaimable_has_children() {
        let db = TupDb::create_in_memory().unwrap();
        db.begin().unwrap();

        let ghost_dir = db
            .node_insert(DOT_DT, "ghost_dir", NodeType::Ghost, -1, 0, -1, None, None)
            .unwrap();
        // Child node has ghost_dir as parent
        db.node_insert(ghost_dir, "child.txt", NodeType::File, 0, 0, -1, None, None)
            .unwrap();

        assert!(!db.is_ghost_reclaimable(ghost_dir, NodeType::Ghost).unwrap());
        db.commit().unwrap();
    }

    #[test]
    fn test_ghost_not_reclaimable_has_links() {
        let db = TupDb::create_in_memory().unwrap();
        db.begin().unwrap();

        let ghost = db
            .node_insert(DOT_DT, "ghost_file", NodeType::Ghost, -1, 0, -1, None, None)
            .unwrap();
        let cmd = db
            .node_insert(DOT_DT, "some_cmd", NodeType::Cmd, -1, 0, -1, None, None)
            .unwrap();
        db.link_insert(ghost, cmd, tup_types::LinkType::Normal)
            .unwrap();

        assert!(!db.is_ghost_reclaimable(ghost, NodeType::Ghost).unwrap());
        db.commit().unwrap();
    }

    #[test]
    fn test_reclaim_ghosts() {
        let db = TupDb::create_in_memory().unwrap();
        db.begin().unwrap();

        let g1 = db
            .node_insert(DOT_DT, "ghost1", NodeType::Ghost, -1, 0, -1, None, None)
            .unwrap();
        let g2 = db
            .node_insert(DOT_DT, "ghost2", NodeType::Ghost, -1, 0, -1, None, None)
            .unwrap();

        let mut candidates = vec![g1, g2];
        let reclaimed = db.reclaim_ghosts(&mut candidates).unwrap();
        assert_eq!(reclaimed, 2);

        // Both should be gone
        assert!(db.node_select_by_id(g1).unwrap().is_none());
        assert!(db.node_select_by_id(g2).unwrap().is_none());
        db.commit().unwrap();
    }

    #[test]
    fn test_reclaim_cascading() {
        let db = TupDb::create_in_memory().unwrap();
        db.begin().unwrap();

        // Ghost parent dir with ghost child
        let parent = db
            .node_insert(
                DOT_DT,
                "ghost_parent",
                NodeType::Ghost,
                -1,
                0,
                -1,
                None,
                None,
            )
            .unwrap();
        let child = db
            .node_insert(
                parent,
                "ghost_child",
                NodeType::Ghost,
                -1,
                0,
                -1,
                None,
                None,
            )
            .unwrap();

        // Start with child — parent should cascade
        let mut candidates = vec![child];
        let reclaimed = db.reclaim_ghosts(&mut candidates).unwrap();
        assert_eq!(reclaimed, 2); // Both child and parent reclaimed

        assert!(db.node_select_by_id(child).unwrap().is_none());
        assert!(db.node_select_by_id(parent).unwrap().is_none());
        db.commit().unwrap();
    }

    #[test]
    fn test_tup_config_never_reclaimed() {
        let db = TupDb::create_in_memory().unwrap();
        db.begin().unwrap();

        let config = db
            .node_insert(DOT_DT, "tup.config", NodeType::Ghost, -1, 0, -1, None, None)
            .unwrap();

        let mut candidates = vec![config];
        let reclaimed = db.reclaim_ghosts(&mut candidates).unwrap();
        assert_eq!(reclaimed, 0); // tup.config never reclaimed

        assert!(db.node_select_by_id(config).unwrap().is_some());
        db.commit().unwrap();
    }

    #[test]
    fn test_delete_or_ghost_no_refs() {
        let db = TupDb::create_in_memory().unwrap();
        db.begin().unwrap();

        let id = db
            .node_insert(DOT_DT, "temp", NodeType::File, 0, 0, -1, None, None)
            .unwrap();

        let deleted = db.delete_or_ghost(id).unwrap();
        assert!(deleted); // Fully deleted
        assert!(db.node_select_by_id(id).unwrap().is_none());
        db.commit().unwrap();
    }

    #[test]
    fn test_delete_or_ghost_has_refs() {
        let db = TupDb::create_in_memory().unwrap();
        db.begin().unwrap();

        let dir = db
            .node_insert(DOT_DT, "mydir", NodeType::Dir, 0, 0, -1, None, None)
            .unwrap();
        db.node_insert(dir, "child", NodeType::File, 0, 0, -1, None, None)
            .unwrap();

        let deleted = db.delete_or_ghost(dir).unwrap();
        assert!(!deleted); // Converted to ghost
        let node = db.node_select_by_id(dir).unwrap().unwrap();
        assert_eq!(node.node_type, NodeType::Ghost);
        db.commit().unwrap();
    }
}
