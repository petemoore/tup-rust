use rusqlite::params;
use tup_types::{LinkType, NodeType, TupId};

use crate::error::DbResult;
use crate::schema::{NodeRow, TupDb};

/// High-level link/dependency operations.
///
/// These correspond to the link management functions in db.c that handle
/// dependency queries, flag propagation, and input/output tracking.
impl TupDb {
    /// Create a link with a specific cmdid (for group links).
    pub fn link_insert_group(&self, from_id: TupId, to_id: TupId, cmdid: TupId) -> DbResult<()> {
        self.conn().execute(
            "INSERT OR IGNORE INTO group_link VALUES(?1, ?2, ?3)",
            params![from_id.raw(), to_id.raw(), cmdid.raw()],
        )?;
        Ok(())
    }

    /// Remove a specific link.
    pub fn link_remove(&self, from_id: TupId, to_id: TupId, link_type: LinkType) -> DbResult<bool> {
        let table = link_type.table_name();
        let count = self.conn().execute(
            &format!("DELETE FROM {table} WHERE from_id=?1 AND to_id=?2"),
            params![from_id.raw(), to_id.raw()],
        )?;
        Ok(count > 0)
    }

    /// Remove all links (both normal and sticky) involving a node.
    ///
    /// This deletes links where the node is either the source or destination.
    /// Used when cleaning up stale command nodes.
    pub fn link_delete_all(&self, id: TupId) -> DbResult<()> {
        for table in &["normal_link", "sticky_link"] {
            self.conn().execute(
                &format!("DELETE FROM {table} WHERE from_id=?1 OR to_id=?1"),
                params![id.raw()],
            )?;
        }
        Ok(())
    }

    /// Get all input node IDs that link TO a given node via normal_link AND sticky_link.
    ///
    /// For CMD nodes, this returns the input files that the command depends on.
    /// In C tup, declared inputs use STICKY links (from input file to CMD).
    /// Normal links go from CMD to output file.
    pub fn get_input_ids(&self, to_id: TupId) -> DbResult<Vec<TupId>> {
        // Inputs to commands come via sticky_link (declared deps from Tupfile rules)
        let mut stmt = self
            .conn()
            .prepare("SELECT from_id FROM sticky_link WHERE to_id=?1")?;
        let rows = stmt.query_map(params![to_id.raw()], |row| {
            let id: i64 = row.get(0)?;
            Ok(TupId::new(id))
        })?;
        let mut result: Vec<TupId> = rows.collect::<Result<Vec<_>, _>>()?;

        // Also check normal_link for any additional dependencies
        let mut stmt2 = self
            .conn()
            .prepare("SELECT from_id FROM normal_link WHERE to_id=?1")?;
        let rows2 = stmt2.query_map(params![to_id.raw()], |row| {
            let id: i64 = row.get(0)?;
            Ok(TupId::new(id))
        })?;
        for id in rows2 {
            let id = id?;
            if !result.contains(&id) {
                result.push(id);
            }
        }

        Ok(result)
    }

    /// Get the single incoming normal link for a node.
    ///
    /// For output files, there should be exactly one command that produces them.
    /// Returns None if no incoming link exists.
    ///
    /// Corresponds to `tup_db_get_incoming_link()` in C.
    pub fn get_incoming_link(&self, id: TupId) -> DbResult<Option<TupId>> {
        let result = self.conn().query_row(
            "SELECT from_id FROM normal_link WHERE to_id=?1",
            params![id.raw()],
            |row| {
                let from_id: i64 = row.get(0)?;
                Ok(TupId::new(from_id))
            },
        );

        match result {
            Ok(id) => Ok(Some(id)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Get all nodes linked FROM the given node via normal links.
    ///
    /// i.e., "what does this node point to?" (outputs/dependents)
    ///
    /// Corresponds to `tup_db_select_node_by_link()` in C.
    pub fn get_normal_outputs(&self, from_id: TupId) -> DbResult<Vec<TupId>> {
        let mut stmt = self
            .conn()
            .prepare("SELECT to_id FROM normal_link WHERE from_id=?1")?;
        let rows = stmt.query_map(params![from_id.raw()], |row| {
            let id: i64 = row.get(0)?;
            Ok(TupId::new(id))
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Get all nodes linked TO the given node via normal links.
    ///
    /// i.e., "what points to this node?" (inputs/dependencies)
    pub fn get_normal_inputs(&self, to_id: TupId) -> DbResult<Vec<TupId>> {
        let mut stmt = self
            .conn()
            .prepare("SELECT from_id FROM normal_link WHERE to_id=?1")?;
        let rows = stmt.query_map(params![to_id.raw()], |row| {
            let id: i64 = row.get(0)?;
            Ok(TupId::new(id))
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Get all sticky inputs for a node (typically a command).
    ///
    /// Corresponds to sticky_link queries in `tup_db_get_inputs()`.
    pub fn get_sticky_inputs(&self, to_id: TupId) -> DbResult<Vec<TupId>> {
        let mut stmt = self
            .conn()
            .prepare("SELECT from_id FROM sticky_link WHERE to_id=?1")?;
        let rows = stmt.query_map(params![to_id.raw()], |row| {
            let id: i64 = row.get(0)?;
            Ok(TupId::new(id))
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Get all nodes linked FROM the given node via sticky links.
    pub fn get_sticky_outputs(&self, from_id: TupId) -> DbResult<Vec<TupId>> {
        let mut stmt = self
            .conn()
            .prepare("SELECT to_id FROM sticky_link WHERE from_id=?1")?;
        let rows = stmt.query_map(params![from_id.raw()], |row| {
            let id: i64 = row.get(0)?;
            Ok(TupId::new(id))
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Get all group links for a node (as group member).
    ///
    /// Returns (to_id, cmdid) pairs.
    pub fn get_group_links(&self, from_id: TupId) -> DbResult<Vec<(TupId, TupId)>> {
        let mut stmt = self
            .conn()
            .prepare("SELECT to_id, cmdid FROM group_link WHERE from_id=?1")?;
        let rows = stmt.query_map(params![from_id.raw()], |row| {
            let to_id: i64 = row.get(0)?;
            let cmdid: i64 = row.get(1)?;
            Ok((TupId::new(to_id), TupId::new(cmdid)))
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Get all outputs for a command.
    ///
    /// Returns all nodes that this command produces (normal_link from_id=cmdid).
    ///
    /// Corresponds to `tup_db_get_outputs()` in C.
    pub fn get_cmd_outputs(&self, cmdid: TupId) -> DbResult<Vec<NodeRow>> {
        let mut stmt = self.conn().prepare(
            "SELECT n.id, n.dir, n.type, n.mtime, n.mtime_ns, n.srcid, n.name, n.display, n.flags \
             FROM node n JOIN normal_link l ON n.id = l.to_id \
             WHERE l.from_id=?1",
        )?;
        let rows = stmt.query_map(params![cmdid.raw()], crate::schema::NodeRow::from_row)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Get all inputs for a command (both normal and sticky).
    ///
    /// Returns (normal_inputs, sticky_inputs).
    ///
    /// Corresponds to `tup_db_get_inputs()` in C.
    pub fn get_cmd_inputs(&self, cmdid: TupId) -> DbResult<(Vec<NodeRow>, Vec<NodeRow>)> {
        // Normal inputs: nodes that point TO this command
        let mut normal_stmt = self.conn().prepare(
            "SELECT n.id, n.dir, n.type, n.mtime, n.mtime_ns, n.srcid, n.name, n.display, n.flags \
             FROM node n JOIN normal_link l ON n.id = l.from_id \
             WHERE l.to_id=?1",
        )?;
        let normal = normal_stmt
            .query_map(params![cmdid.raw()], crate::schema::NodeRow::from_row)?
            .collect::<Result<Vec<_>, _>>()?;

        // Sticky inputs
        let mut sticky_stmt = self.conn().prepare(
            "SELECT n.id, n.dir, n.type, n.mtime, n.mtime_ns, n.srcid, n.name, n.display, n.flags \
             FROM node n JOIN sticky_link l ON n.id = l.from_id \
             WHERE l.to_id=?1",
        )?;
        let sticky = sticky_stmt
            .query_map(params![cmdid.raw()], crate::schema::NodeRow::from_row)?
            .collect::<Result<Vec<_>, _>>()?;

        Ok((normal, sticky))
    }

    /// Get the output group for a command.
    ///
    /// Finds the GROUP node linked from the command via normal_link.
    /// Returns None if no output group is defined.
    pub fn get_output_group(&self, cmdid: TupId) -> DbResult<Option<TupId>> {
        let result = self.conn().query_row(
            "SELECT l.to_id FROM normal_link l \
             JOIN node n ON l.to_id = n.id \
             WHERE l.from_id=?1 AND n.type=?2",
            params![cmdid.raw(), NodeType::Group.as_i32()],
            |row| {
                let id: i64 = row.get(0)?;
                Ok(TupId::new(id))
            },
        );

        match result {
            Ok(id) => Ok(Some(id)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Mark commands that produce a given output as modified.
    ///
    /// Adds the from_id of any normal_link pointing to this output
    /// to the modify_list.
    ///
    /// Corresponds to `tup_db_modify_cmds_by_output()` in C.
    /// Returns the number of commands flagged.
    pub fn modify_cmds_by_output(&self, output_id: TupId) -> DbResult<u64> {
        self.conn().execute(
            "INSERT OR IGNORE INTO modify_list \
             SELECT from_id FROM normal_link WHERE to_id=?1",
            params![output_id.raw()],
        )?;
        Ok(self.changes())
    }

    /// Mark commands that use a given input as modified.
    ///
    /// Adds the to_id of any normal_link from this input (where to_id
    /// is a CMD or GROUP) to the modify_list.
    ///
    /// Corresponds to `tup_db_modify_cmds_by_input()` in C.
    pub fn modify_cmds_by_input(&self, input_id: TupId) -> DbResult<()> {
        // C tup only checks normal_link (auto-detected via LD_PRELOAD/FUSE).
        // We also check sticky_link since we store declared inputs as sticky
        // links and don't yet have LD_PRELOAD/FUSE auto-detection.
        self.conn().execute(
            "INSERT OR IGNORE INTO modify_list \
             SELECT l.to_id FROM normal_link l \
             JOIN node n ON l.to_id = n.id \
             WHERE l.from_id=?1 AND (n.type=?2 OR n.type=?3)",
            params![
                input_id.raw(),
                NodeType::Cmd.as_i32(),
                NodeType::Group.as_i32(),
            ],
        )?;
        self.conn().execute(
            "INSERT OR IGNORE INTO modify_list \
             SELECT l.to_id FROM sticky_link l \
             JOIN node n ON l.to_id = n.id \
             WHERE l.from_id=?1 AND (n.type=?2 OR n.type=?3)",
            params![
                input_id.raw(),
                NodeType::Cmd.as_i32(),
                NodeType::Group.as_i32(),
            ],
        )?;
        Ok(())
    }

    /// Flag dependent directories for re-parsing.
    ///
    /// When a file changes, any directory that includes it (via normal_link)
    /// needs to be re-parsed.
    ///
    /// Corresponds to `tup_db_set_dependent_dir_flags()` in C.
    pub fn set_dependent_dir_flags(&self, id: TupId) -> DbResult<()> {
        self.conn().execute(
            "INSERT OR IGNORE INTO create_list \
             SELECT l.to_id FROM normal_link l \
             JOIN node n ON l.to_id = n.id \
             WHERE l.from_id=?1 AND n.type=?2",
            params![id.raw(), NodeType::Dir.as_i32()],
        )?;
        Ok(())
    }

    /// Flag dependent config nodes.
    ///
    /// Corresponds to `tup_db_set_dependent_config_flags()` in C.
    pub fn set_dependent_config_flags(&self, id: TupId) -> DbResult<()> {
        // Find directories that have a tup.config dependency on this node
        self.conn().execute(
            "INSERT OR IGNORE INTO config_list \
             SELECT l.to_id FROM sticky_link l \
             JOIN node n ON l.to_id = n.id \
             WHERE l.from_id=?1 AND n.type=?2",
            params![id.raw(), NodeType::Dir.as_i32()],
        )?;
        Ok(())
    }

    /// Set all dependent flags for a changed file.
    ///
    /// Combines `set_dependent_dir_flags` and `set_dependent_config_flags`.
    ///
    /// Corresponds to `tup_db_set_dependent_flags()` in C.
    pub fn set_dependent_flags(&self, id: TupId) -> DbResult<()> {
        self.set_dependent_dir_flags(id)?;
        self.set_dependent_config_flags(id)?;
        Ok(())
    }

    /// Flag all directories with srcid matching the given tupid.
    ///
    /// Used when a source directory changes to re-parse variant directories.
    ///
    /// Corresponds to `tup_db_set_srcid_dir_flags()` in C.
    pub fn set_srcid_dir_flags(&self, srcid: TupId) -> DbResult<()> {
        self.conn().execute(
            "INSERT OR IGNORE INTO create_list \
             SELECT id FROM node WHERE srcid=?1 AND type=?2",
            params![srcid.raw(), NodeType::Dir.as_i32()],
        )?;
        Ok(())
    }

    /// Get all nodes of a specific type in a directory.
    ///
    /// Corresponds to `tup_db_dirtype()` in C.
    pub fn dir_nodes_by_type(&self, dir: TupId, node_type: NodeType) -> DbResult<Vec<NodeRow>> {
        let mut stmt = self.conn().prepare(
            "SELECT id, dir, type, mtime, mtime_ns, srcid, name, display, flags \
             FROM node WHERE dir=?1 AND type=?2",
        )?;
        let rows = stmt.query_map(
            params![dir.raw(), node_type.as_i32()],
            crate::schema::NodeRow::from_row,
        )?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Get all nodes with a specific srcid.
    ///
    /// Corresponds to `tup_db_srcid_to_tree()` in C.
    pub fn nodes_by_srcid(&self, srcid: TupId, node_type: NodeType) -> DbResult<Vec<NodeRow>> {
        let mut stmt = self.conn().prepare(
            "SELECT id, dir, type, mtime, mtime_ns, srcid, name, display, flags \
             FROM node WHERE srcid=?1 AND type=?2",
        )?;
        let rows = stmt.query_map(
            params![srcid.raw(), node_type.as_i32()],
            crate::schema::NodeRow::from_row,
        )?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Mark all commands for re-execution.
    ///
    /// Corresponds to `tup_db_rebuild_all()` in C.
    pub fn rebuild_all(&self) -> DbResult<()> {
        self.conn().execute(
            "INSERT OR IGNORE INTO modify_list \
             SELECT id FROM node WHERE type=?1",
            params![NodeType::Cmd.as_i32()],
        )?;
        Ok(())
    }

    /// Mark all Tupfiles for re-parsing.
    ///
    /// Corresponds to `tup_db_reparse_all()` in C.
    pub fn reparse_all(&self) -> DbResult<()> {
        self.conn().execute(
            "INSERT OR IGNORE INTO create_list \
             SELECT id FROM node WHERE type=?1",
            params![NodeType::Dir.as_i32()],
        )?;
        Ok(())
    }

    /// Check if a directory is a "generated" directory.
    ///
    /// A directory is considered generated if it contains no regular files
    /// and has at least one generated file or command.
    ///
    /// Corresponds to `tup_db_is_generated_dir()` in C.
    pub fn is_generated_dir(&self, dir: TupId) -> DbResult<bool> {
        // Check for any regular files
        let has_files: bool = self.conn().query_row(
            "SELECT EXISTS(SELECT 1 FROM node WHERE dir=?1 AND type=?2)",
            params![dir.raw(), NodeType::File.as_i32()],
            |row| row.get(0),
        )?;

        if has_files {
            return Ok(false);
        }

        // Check for generated content
        let has_generated: bool = self.conn().query_row(
            "SELECT EXISTS(SELECT 1 FROM node WHERE dir=?1 AND (type=?2 OR type=?3))",
            params![
                dir.raw(),
                NodeType::Generated.as_i32(),
                NodeType::Cmd.as_i32(),
            ],
            |row| row.get(0),
        )?;

        Ok(has_generated)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tup_types::{TupFlags, DOT_DT};

    fn setup() -> TupDb {
        TupDb::create_in_memory().unwrap()
    }

    fn insert_file(db: &TupDb, dir: TupId, name: &str) -> TupId {
        db.node_insert(dir, name, NodeType::File, 0, 0, -1, None, None)
            .unwrap()
    }

    fn insert_cmd(db: &TupDb, dir: TupId, name: &str) -> TupId {
        db.node_insert(dir, name, NodeType::Cmd, -1, 0, -1, None, None)
            .unwrap()
    }

    #[test]
    fn test_get_normal_outputs() {
        let db = setup();
        db.begin().unwrap();

        let file = insert_file(&db, DOT_DT, "input.c");
        let cmd = insert_cmd(&db, DOT_DT, "gcc input.c");
        db.link_insert(file, cmd, LinkType::Normal).unwrap();

        let outputs = db.get_normal_outputs(file).unwrap();
        assert_eq!(outputs.len(), 1);
        assert_eq!(outputs[0], cmd);

        db.commit().unwrap();
    }

    #[test]
    fn test_get_normal_inputs() {
        let db = setup();
        db.begin().unwrap();

        let file1 = insert_file(&db, DOT_DT, "a.c");
        let file2 = insert_file(&db, DOT_DT, "b.c");
        let cmd = insert_cmd(&db, DOT_DT, "gcc a.c b.c");
        db.link_insert(file1, cmd, LinkType::Normal).unwrap();
        db.link_insert(file2, cmd, LinkType::Normal).unwrap();

        let inputs = db.get_normal_inputs(cmd).unwrap();
        assert_eq!(inputs.len(), 2);
        assert!(inputs.contains(&file1));
        assert!(inputs.contains(&file2));

        db.commit().unwrap();
    }

    #[test]
    fn test_get_sticky_inputs() {
        let db = setup();
        db.begin().unwrap();

        let header = insert_file(&db, DOT_DT, "config.h");
        let cmd = insert_cmd(&db, DOT_DT, "gcc main.c");
        db.link_insert(header, cmd, LinkType::Sticky).unwrap();

        let sticky = db.get_sticky_inputs(cmd).unwrap();
        assert_eq!(sticky.len(), 1);
        assert_eq!(sticky[0], header);

        // Normal inputs should be empty
        let normal = db.get_normal_inputs(cmd).unwrap();
        assert!(normal.is_empty());

        db.commit().unwrap();
    }

    #[test]
    fn test_link_remove() {
        let db = setup();
        db.begin().unwrap();

        let a = insert_file(&db, DOT_DT, "a");
        let b = insert_cmd(&db, DOT_DT, "cmd");
        db.link_insert(a, b, LinkType::Normal).unwrap();
        assert!(db.link_exists(a, b, LinkType::Normal).unwrap());

        let removed = db.link_remove(a, b, LinkType::Normal).unwrap();
        assert!(removed);
        assert!(!db.link_exists(a, b, LinkType::Normal).unwrap());

        // Removing non-existent link returns false
        let removed = db.link_remove(a, b, LinkType::Normal).unwrap();
        assert!(!removed);

        db.commit().unwrap();
    }

    #[test]
    fn test_get_incoming_link() {
        let db = setup();
        db.begin().unwrap();

        let cmd = insert_cmd(&db, DOT_DT, "gcc");
        let output = insert_file(&db, DOT_DT, "output.o");
        db.link_insert(cmd, output, LinkType::Normal).unwrap();

        let incoming = db.get_incoming_link(output).unwrap();
        assert_eq!(incoming, Some(cmd));

        // No incoming for cmd
        let incoming = db.get_incoming_link(cmd).unwrap();
        assert!(incoming.is_none());

        db.commit().unwrap();
    }

    #[test]
    fn test_link_insert_group() {
        let db = setup();
        db.begin().unwrap();

        let output = insert_file(&db, DOT_DT, "out.o");
        let group = db
            .node_insert(DOT_DT, "<objects>", NodeType::Group, -1, 0, -1, None, None)
            .unwrap();
        let cmd = insert_cmd(&db, DOT_DT, "gcc");

        db.link_insert_group(output, group, cmd).unwrap();

        let links = db.get_group_links(output).unwrap();
        assert_eq!(links.len(), 1);
        assert_eq!(links[0], (group, cmd));

        db.commit().unwrap();
    }

    #[test]
    fn test_get_cmd_outputs() {
        let db = setup();
        db.begin().unwrap();

        let cmd = insert_cmd(&db, DOT_DT, "gcc -c main.c");
        let out1 = db
            .node_insert(DOT_DT, "main.o", NodeType::Generated, 0, 0, -1, None, None)
            .unwrap();
        let out2 = db
            .node_insert(DOT_DT, "main.d", NodeType::Generated, 0, 0, -1, None, None)
            .unwrap();
        db.link_insert(cmd, out1, LinkType::Normal).unwrap();
        db.link_insert(cmd, out2, LinkType::Normal).unwrap();

        let outputs = db.get_cmd_outputs(cmd).unwrap();
        assert_eq!(outputs.len(), 2);
        let names: Vec<&str> = outputs.iter().map(|n| n.name.as_str()).collect();
        assert!(names.contains(&"main.o"));
        assert!(names.contains(&"main.d"));

        db.commit().unwrap();
    }

    #[test]
    fn test_get_cmd_inputs() {
        let db = setup();
        db.begin().unwrap();

        let src = insert_file(&db, DOT_DT, "main.c");
        let hdr = insert_file(&db, DOT_DT, "config.h");
        let cmd = insert_cmd(&db, DOT_DT, "gcc -c main.c");
        db.link_insert(src, cmd, LinkType::Normal).unwrap();
        db.link_insert(hdr, cmd, LinkType::Sticky).unwrap();

        let (normal, sticky) = db.get_cmd_inputs(cmd).unwrap();
        assert_eq!(normal.len(), 1);
        assert_eq!(normal[0].name, "main.c");
        assert_eq!(sticky.len(), 1);
        assert_eq!(sticky[0].name, "config.h");

        db.commit().unwrap();
    }

    #[test]
    fn test_get_output_group() {
        let db = setup();
        db.begin().unwrap();

        let cmd = insert_cmd(&db, DOT_DT, "gcc");
        let group = db
            .node_insert(DOT_DT, "<objs>", NodeType::Group, -1, 0, -1, None, None)
            .unwrap();
        db.link_insert(cmd, group, LinkType::Normal).unwrap();

        let result = db.get_output_group(cmd).unwrap();
        assert_eq!(result, Some(group));

        // Command without a group
        let cmd2 = insert_cmd(&db, DOT_DT, "other");
        assert!(db.get_output_group(cmd2).unwrap().is_none());

        db.commit().unwrap();
    }

    #[test]
    fn test_modify_cmds_by_output() {
        let db = setup();
        db.begin().unwrap();

        let cmd = insert_cmd(&db, DOT_DT, "gcc main.c");
        let output = db
            .node_insert(DOT_DT, "main.o", NodeType::Generated, 0, 0, -1, None, None)
            .unwrap();
        db.link_insert(cmd, output, LinkType::Normal).unwrap();

        let count = db.modify_cmds_by_output(output).unwrap();
        assert_eq!(count, 1);

        // Command should now be in modify_list
        assert!(db.flag_check(cmd, TupFlags::Modify).unwrap());

        db.commit().unwrap();
    }

    #[test]
    fn test_modify_cmds_by_input() {
        let db = setup();
        db.begin().unwrap();

        let input = insert_file(&db, DOT_DT, "input.c");
        let cmd = insert_cmd(&db, DOT_DT, "gcc input.c");
        db.link_insert(input, cmd, LinkType::Normal).unwrap();

        db.modify_cmds_by_input(input).unwrap();
        assert!(db.flag_check(cmd, TupFlags::Modify).unwrap());

        db.commit().unwrap();
    }

    #[test]
    fn test_set_dependent_dir_flags() {
        let db = setup();
        db.begin().unwrap();

        // Create a file that's included by a directory (via normal_link)
        let file = insert_file(&db, DOT_DT, "rules.tup");
        let subdir = db
            .node_insert(DOT_DT, "subdir", NodeType::Dir, 0, 0, -1, None, None)
            .unwrap();
        db.link_insert(file, subdir, LinkType::Normal).unwrap();

        db.set_dependent_dir_flags(file).unwrap();
        assert!(db.flag_check(subdir, TupFlags::Create).unwrap());

        db.commit().unwrap();
    }

    #[test]
    fn test_dir_nodes_by_type() {
        let db = setup();
        db.begin().unwrap();

        insert_file(&db, DOT_DT, "a.c");
        insert_file(&db, DOT_DT, "b.c");
        insert_cmd(&db, DOT_DT, "gcc");

        let files = db.dir_nodes_by_type(DOT_DT, NodeType::File).unwrap();
        assert_eq!(files.len(), 2);

        let cmds = db.dir_nodes_by_type(DOT_DT, NodeType::Cmd).unwrap();
        assert_eq!(cmds.len(), 1);

        db.commit().unwrap();
    }

    #[test]
    fn test_rebuild_all() {
        let db = setup();
        db.begin().unwrap();

        let cmd1 = insert_cmd(&db, DOT_DT, "gcc a.c");
        let cmd2 = insert_cmd(&db, DOT_DT, "gcc b.c");
        insert_file(&db, DOT_DT, "a.c"); // Not a command

        db.rebuild_all().unwrap();
        assert!(db.flag_check(cmd1, TupFlags::Modify).unwrap());
        assert!(db.flag_check(cmd2, TupFlags::Modify).unwrap());

        db.commit().unwrap();
    }

    #[test]
    fn test_reparse_all() {
        let db = setup();
        db.begin().unwrap();

        let dir1 = db
            .node_insert(DOT_DT, "dir1", NodeType::Dir, 0, 0, -1, None, None)
            .unwrap();
        let dir2 = db
            .node_insert(DOT_DT, "dir2", NodeType::Dir, 0, 0, -1, None, None)
            .unwrap();

        db.reparse_all().unwrap();
        assert!(db.flag_check(dir1, TupFlags::Create).unwrap());
        assert!(db.flag_check(dir2, TupFlags::Create).unwrap());
        // DOT_DT is also a Dir, so it should be flagged too
        assert!(db.flag_check(DOT_DT, TupFlags::Create).unwrap());

        db.commit().unwrap();
    }

    #[test]
    fn test_is_generated_dir() {
        let db = setup();
        db.begin().unwrap();

        let dir = db
            .node_insert(DOT_DT, "build", NodeType::Dir, 0, 0, -1, None, None)
            .unwrap();

        // Empty dir is not generated
        assert!(!db.is_generated_dir(dir).unwrap());

        // Dir with a generated file is generated
        db.node_insert(dir, "output.o", NodeType::Generated, 0, 0, -1, None, None)
            .unwrap();
        assert!(db.is_generated_dir(dir).unwrap());

        // Dir with a regular file is NOT generated
        insert_file(&db, dir, "source.c");
        assert!(!db.is_generated_dir(dir).unwrap());

        db.commit().unwrap();
    }
}
