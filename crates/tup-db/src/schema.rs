use rusqlite::{params, Connection};
use std::path::{Path, PathBuf};
use tup_types::{NodeType, TupId, DB_VERSION, DOT_DT, PARSER_VERSION};

use crate::error::{DbError, DbResult};

/// SQL statements to create the database schema.
///
/// These match the C implementation exactly (db.c:312-329).
const SCHEMA_SQL: &[&str] = &[
    "CREATE TABLE node (id INTEGER PRIMARY KEY NOT NULL, dir INTEGER NOT NULL, type INTEGER NOT NULL, mtime INTEGER NOT NULL, mtime_ns INTEGER NOT NULL, srcid INTEGER NOT NULL, name VARCHAR(4096), display VARCHAR(4096), flags VARCHAR(256), UNIQUE(dir, name))",
    "CREATE TABLE normal_link (from_id INTEGER, to_id INTEGER, UNIQUE(from_id, to_id))",
    "CREATE TABLE sticky_link (from_id INTEGER, to_id INTEGER, UNIQUE(from_id, to_id))",
    "CREATE TABLE group_link (from_id INTEGER, to_id INTEGER, cmdid INTEGER, UNIQUE(from_id, to_id, cmdid))",
    "CREATE TABLE var (id INTEGER PRIMARY KEY NOT NULL, value VARCHAR(4096))",
    "CREATE TABLE config (lval VARCHAR(256) UNIQUE, rval VARCHAR(256))",
    "CREATE TABLE config_list (id INTEGER PRIMARY KEY NOT NULL)",
    "CREATE TABLE create_list (id INTEGER PRIMARY KEY NOT NULL)",
    "CREATE TABLE modify_list (id INTEGER PRIMARY KEY NOT NULL)",
    "CREATE TABLE variant_list (id INTEGER PRIMARY KEY NOT NULL)",
    "CREATE TABLE transient_list (id INTEGER PRIMARY KEY NOT NULL)",
    "CREATE INDEX normal_index2 ON normal_link(to_id)",
    "CREATE INDEX sticky_index2 ON sticky_link(to_id)",
    "CREATE INDEX group_index2 ON group_link(cmdid)",
    "CREATE INDEX srcid_index ON node(srcid)",
];

/// The root node inserted at database creation.
/// Node id=1, dir=0 (virtual root parent), type=DIR, mtime=-1, mtime_ns=0, srcid=-1, name="."
const ROOT_NODE_SQL: &str = "INSERT INTO node VALUES(1, 0, 2, -1, 0, -1, '.', NULL, NULL)";

/// Initial config entry for db_version (set to 0, then updated).
const INITIAL_CONFIG_SQL: &str = "INSERT INTO config VALUES('db_version', '0')";

/// Handle to the tup SQLite database.
pub struct TupDb {
    conn: Connection,
    #[allow(dead_code)]
    db_path: Option<PathBuf>,
}

impl TupDb {
    /// Create a new tup database at the standard location (.tup/db).
    ///
    /// This creates the .tup directory if needed, initializes the schema,
    /// inserts the root node, sets up virtual directories, and configures
    /// db_version and parser_version.
    ///
    /// `db_sync`: if false, sets PRAGMA synchronous=OFF for speed.
    pub fn create(tup_dir: &Path, db_sync: bool) -> DbResult<Self> {
        let tup_internal = tup_dir.join(".tup");
        std::fs::create_dir_all(&tup_internal)
            .map_err(|e| DbError::Other(format!("failed to create .tup directory: {e}")))?;

        let db_path = tup_internal.join("db");
        if db_path.exists() {
            return Err(DbError::AlreadyExists {
                path: db_path.display().to_string(),
            });
        }

        let conn = Connection::open(&db_path)?;
        let mut db = TupDb {
            conn,
            db_path: Some(db_path),
        };

        if !db_sync {
            db.no_sync()?;
        }

        db.init_schema()?;
        Ok(db)
    }

    /// Create a new in-memory database (for testing).
    pub fn create_in_memory() -> DbResult<Self> {
        let conn = Connection::open_in_memory()?;
        let mut db = TupDb {
            conn,
            db_path: None,
        };
        db.no_sync()?;
        db.init_schema()?;
        Ok(db)
    }

    /// Open an existing tup database.
    ///
    /// Verifies the schema version matches.
    pub fn open(tup_dir: &Path, db_sync: bool) -> DbResult<Self> {
        let db_path = tup_dir.join(".tup").join("db");
        if !db_path.exists() {
            return Err(DbError::NotFound {
                path: db_path.display().to_string(),
            });
        }

        let conn =
            Connection::open_with_flags(&db_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE)?;

        let db = TupDb {
            conn,
            db_path: Some(db_path),
        };

        if !db_sync {
            db.no_sync()?;
        }

        db.version_check()?;
        Ok(db)
    }

    /// Get a reference to the underlying SQLite connection.
    pub fn conn(&self) -> &Connection {
        &self.conn
    }

    /// Get a mutable reference to the underlying SQLite connection.
    pub fn conn_mut(&mut self) -> &mut Connection {
        &mut self.conn
    }

    /// Begin a transaction.
    pub fn begin(&self) -> DbResult<()> {
        self.conn.execute_batch("BEGIN")?;
        Ok(())
    }

    /// Commit the current transaction.
    pub fn commit(&self) -> DbResult<()> {
        self.conn.execute_batch("COMMIT")?;
        Ok(())
    }

    /// Rollback the current transaction.
    pub fn rollback(&self) -> DbResult<()> {
        self.conn.execute_batch("ROLLBACK")?;
        Ok(())
    }

    /// Get the number of changes made by the last statement.
    pub fn changes(&self) -> u64 {
        self.conn.changes()
    }

    // -- Config operations --

    /// Set a config value (integer).
    pub fn config_set_int(&self, key: &str, value: i32) -> DbResult<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO config VALUES(?1, ?2)",
            params![key, value.to_string()],
        )?;
        Ok(())
    }

    /// Get a config value (integer), returning the default if not found.
    pub fn config_get_int(&self, key: &str, default: i32) -> DbResult<i32> {
        let result = self.conn.query_row(
            "SELECT rval FROM config WHERE lval=?1",
            params![key],
            |row| {
                let val: String = row.get(0)?;
                Ok(val)
            },
        );

        match result {
            Ok(val) => val
                .parse::<i32>()
                .map_err(|e| DbError::Other(format!("invalid config value for '{key}': {e}"))),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(default),
            Err(e) => Err(e.into()),
        }
    }

    /// Set a config value (string).
    pub fn config_set_string(&self, key: &str, value: &str) -> DbResult<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO config VALUES(?1, ?2)",
            params![key, value],
        )?;
        Ok(())
    }

    /// Get a config value (string), returning default if not found.
    pub fn config_get_string(&self, key: &str, default: &str) -> DbResult<String> {
        let result = self.conn.query_row(
            "SELECT rval FROM config WHERE lval=?1",
            params![key],
            |row| row.get::<_, String>(0),
        );
        match result {
            Ok(val) => Ok(val),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(default.to_string()),
            Err(e) => Err(e.into()),
        }
    }

    // -- Node operations --

    /// Insert a node into the database.
    ///
    /// Returns the TupId of the newly inserted node.
    #[allow(clippy::too_many_arguments)]
    pub fn node_insert(
        &self,
        dir: TupId,
        name: &str,
        node_type: NodeType,
        mtime: i64,
        mtime_ns: i64,
        srcid: i64,
        display: Option<&str>,
        flags: Option<&str>,
    ) -> DbResult<TupId> {
        self.conn.execute(
            "INSERT INTO node (dir, type, mtime, mtime_ns, srcid, name, display, flags) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                dir.raw(),
                node_type.as_i32(),
                mtime,
                mtime_ns,
                srcid,
                name,
                display,
                flags,
            ],
        )?;
        Ok(TupId::new(self.conn.last_insert_rowid()))
    }

    /// Look up a node by parent directory and name.
    ///
    /// Returns None if not found.
    pub fn node_select(&self, dir: TupId, name: &str) -> DbResult<Option<NodeRow>> {
        let result = self.conn.query_row(
            "SELECT id, dir, type, mtime, mtime_ns, srcid, name, display, flags FROM node WHERE dir=?1 AND name=?2",
            params![dir.raw(), name],
            NodeRow::from_row,
        );

        match result {
            Ok(node) => Ok(Some(node)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Look up a node by its tupid.
    pub fn node_select_by_id(&self, id: TupId) -> DbResult<Option<NodeRow>> {
        let result = self.conn.query_row(
            "SELECT id, dir, type, mtime, mtime_ns, srcid, name, display, flags FROM node WHERE id=?1",
            params![id.raw()],
            NodeRow::from_row,
        );

        match result {
            Ok(node) => Ok(Some(node)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Delete a node by its tupid.
    pub fn node_delete(&self, id: TupId) -> DbResult<bool> {
        let count = self
            .conn
            .execute("DELETE FROM node WHERE id=?1", params![id.raw()])?;
        Ok(count > 0)
    }

    /// Update a node's type.
    pub fn node_set_type(&self, id: TupId, node_type: NodeType) -> DbResult<()> {
        self.conn.execute(
            "UPDATE node SET type=?1 WHERE id=?2",
            params![node_type.as_i32(), id.raw()],
        )?;
        Ok(())
    }

    /// Update a node's name and/or parent directory.
    pub fn node_set_name(&self, id: TupId, name: &str, dir: TupId) -> DbResult<()> {
        self.conn.execute(
            "UPDATE node SET name=?1, dir=?2 WHERE id=?3",
            params![name, dir.raw(), id.raw()],
        )?;
        Ok(())
    }

    /// Update a node's mtime.
    pub fn node_set_mtime(&self, id: TupId, mtime: i64, mtime_ns: i64) -> DbResult<()> {
        self.conn.execute(
            "UPDATE node SET mtime=?1, mtime_ns=?2 WHERE id=?3",
            params![mtime, mtime_ns, id.raw()],
        )?;
        Ok(())
    }

    /// Update a node's srcid.
    pub fn node_set_srcid(&self, id: TupId, srcid: i64) -> DbResult<()> {
        self.conn.execute(
            "UPDATE node SET srcid=?1 WHERE id=?2",
            params![srcid, id.raw()],
        )?;
        Ok(())
    }

    /// Update a node's display string.
    pub fn node_set_display(&self, id: TupId, display: Option<&str>) -> DbResult<()> {
        self.conn.execute(
            "UPDATE node SET display=?1 WHERE id=?2",
            params![display, id.raw()],
        )?;
        Ok(())
    }

    /// Update a node's flags string.
    pub fn node_set_flags(&self, id: TupId, flags: Option<&str>) -> DbResult<()> {
        self.conn.execute(
            "UPDATE node SET flags=?1 WHERE id=?2",
            params![flags, id.raw()],
        )?;
        Ok(())
    }

    /// List all nodes in a directory.
    pub fn node_select_dir(&self, dir: TupId) -> DbResult<Vec<NodeRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, dir, type, mtime, mtime_ns, srcid, name, display, flags FROM node WHERE dir=?1",
        )?;
        let rows = stmt.query_map(params![dir.raw()], NodeRow::from_row)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    // -- Link operations --

    /// Create a link between two nodes.
    ///
    /// Uses INSERT OR IGNORE for idempotency.
    pub fn link_insert(
        &self,
        from_id: TupId,
        to_id: TupId,
        link_type: tup_types::LinkType,
    ) -> DbResult<()> {
        let sql = match link_type {
            tup_types::LinkType::Normal => "INSERT OR IGNORE INTO normal_link VALUES(?1, ?2)",
            tup_types::LinkType::Sticky => "INSERT OR IGNORE INTO sticky_link VALUES(?1, ?2)",
            tup_types::LinkType::Group => {
                // Group links need a cmdid, but this simple version uses 0.
                // The full version with cmdid will come in a later PR.
                "INSERT OR IGNORE INTO group_link VALUES(?1, ?2, 0)"
            }
        };
        self.conn
            .execute(sql, params![from_id.raw(), to_id.raw()])?;
        Ok(())
    }

    /// Check if a link exists.
    pub fn link_exists(
        &self,
        from_id: TupId,
        to_id: TupId,
        link_type: tup_types::LinkType,
    ) -> DbResult<bool> {
        let table = link_type.table_name();
        let sql = format!("SELECT 1 FROM {table} WHERE from_id=?1 AND to_id=?2");
        let result = self
            .conn
            .query_row(&sql, params![from_id.raw(), to_id.raw()], |_| Ok(()));
        match result {
            Ok(()) => Ok(true),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(false),
            Err(e) => Err(e.into()),
        }
    }

    /// Delete all links involving a node (both directions).
    pub fn links_delete(&self, id: TupId) -> DbResult<()> {
        for table in &["normal_link", "sticky_link"] {
            self.conn.execute(
                &format!("DELETE FROM {table} WHERE from_id=?1 OR to_id=?1"),
                params![id.raw()],
            )?;
        }
        self.conn.execute(
            "DELETE FROM group_link WHERE from_id=?1 OR to_id=?1 OR cmdid=?1",
            params![id.raw()],
        )?;
        Ok(())
    }

    // -- Flag list operations --

    /// Add a node to a flag list (idempotent).
    pub fn flag_add(&self, id: TupId, flag: tup_types::TupFlags) -> DbResult<()> {
        if let Some(table) = flag.table_name() {
            self.conn.execute(
                &format!("INSERT OR IGNORE INTO {table} VALUES(?1)"),
                params![id.raw()],
            )?;
        }
        Ok(())
    }

    /// Remove a node from a flag list.
    pub fn flag_remove(&self, id: TupId, flag: tup_types::TupFlags) -> DbResult<()> {
        if let Some(table) = flag.table_name() {
            self.conn.execute(
                &format!("DELETE FROM {table} WHERE id=?1"),
                params![id.raw()],
            )?;
        }
        Ok(())
    }

    /// Check if a node is in a flag list.
    pub fn flag_check(&self, id: TupId, flag: tup_types::TupFlags) -> DbResult<bool> {
        let table = match flag.table_name() {
            Some(t) => t,
            None => return Ok(false),
        };
        let result = self.conn.query_row(
            &format!("SELECT 1 FROM {table} WHERE id=?1"),
            params![id.raw()],
            |_| Ok(()),
        );
        match result {
            Ok(()) => Ok(true),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(false),
            Err(e) => Err(e.into()),
        }
    }

    /// Check if all flag lists are empty (no pending work).
    ///
    /// Returns true when create_list, modify_list, config_list, variant_list,
    /// and transient_list are all empty. Used by `tup flags_exists`.
    pub fn flags_empty(&self) -> DbResult<bool> {
        for table in &[
            "create_list",
            "modify_list",
            "config_list",
            "variant_list",
            "transient_list",
        ] {
            let count: i64 =
                self.conn
                    .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                        row.get(0)
                    })?;
            if count > 0 {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// Clear all flag lists (after a successful update cycle).
    pub fn flags_clear_all(&self) -> DbResult<()> {
        for table in &[
            "create_list",
            "modify_list",
            "config_list",
            "variant_list",
            "transient_list",
        ] {
            self.conn.execute(&format!("DELETE FROM {table}"), [])?;
        }
        Ok(())
    }

    /// Get all node IDs in a flag list.
    pub fn flag_list(&self, flag: tup_types::TupFlags) -> DbResult<Vec<TupId>> {
        let table = match flag.table_name() {
            Some(t) => t,
            None => return Ok(vec![]),
        };
        let mut stmt = self.conn.prepare(&format!("SELECT id FROM {table}"))?;
        let rows = stmt.query_map([], |row| {
            let id: i64 = row.get(0)?;
            Ok(TupId::new(id))
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    // -- Variable operations --

    /// Set a variable value.
    pub fn var_set(&self, id: TupId, value: &str) -> DbResult<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO var VALUES(?1, ?2)",
            params![id.raw(), value],
        )?;
        Ok(())
    }

    /// Get a variable value.
    pub fn var_get(&self, id: TupId) -> DbResult<Option<String>> {
        let result = self.conn.query_row(
            "SELECT value FROM var WHERE id=?1",
            params![id.raw()],
            |row| row.get(0),
        );
        match result {
            Ok(val) => Ok(Some(val)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Delete a variable.
    pub fn var_delete(&self, id: TupId) -> DbResult<bool> {
        let count = self
            .conn
            .execute("DELETE FROM var WHERE id=?1", params![id.raw()])?;
        Ok(count > 0)
    }

    // -- Internal helpers --

    /// Set PRAGMA synchronous=OFF for speed.
    fn no_sync(&self) -> DbResult<()> {
        self.conn.execute_batch("PRAGMA synchronous=OFF")?;
        Ok(())
    }

    /// Initialize the database schema, root node, and config.
    fn init_schema(&mut self) -> DbResult<()> {
        self.begin()?;

        // Create all tables and indexes
        for sql in SCHEMA_SQL {
            self.conn.execute_batch(sql)?;
        }

        // Insert initial config and root node
        self.conn.execute_batch(INITIAL_CONFIG_SQL)?;
        self.conn.execute_batch(ROOT_NODE_SQL)?;

        // Set version numbers
        self.config_set_int("db_version", DB_VERSION)?;
        self.config_set_int("parser_version", PARSER_VERSION)?;

        // Create virtual directories ($, /, ^)
        self.init_virtual_dirs()?;

        self.commit()?;
        Ok(())
    }

    /// Create the three virtual directories under the root node.
    ///
    /// These are: "$" (env), "/" (slash), "^" (exclusion).
    fn init_virtual_dirs(&self) -> DbResult<()> {
        let dir_type = NodeType::Dir.as_i32();

        // "$" directory for environment variables
        self.conn.execute(
            "INSERT OR IGNORE INTO node (dir, type, mtime, mtime_ns, srcid, name) VALUES (?1, ?2, -1, 0, -1, '$')",
            params![DOT_DT.raw(), dir_type],
        )?;

        // "/" directory for full filesystem dependency tracking
        self.conn.execute(
            "INSERT OR IGNORE INTO node (dir, type, mtime, mtime_ns, srcid, name) VALUES (?1, ?2, -1, 0, -1, '/')",
            params![DOT_DT.raw(), dir_type],
        )?;

        // "^" directory for exclusion tracking
        self.conn.execute(
            "INSERT OR IGNORE INTO node (dir, type, mtime, mtime_ns, srcid, name) VALUES (?1, ?2, -1, 0, -1, '^')",
            params![DOT_DT.raw(), dir_type],
        )?;

        Ok(())
    }

    /// Look up the tupid of a virtual directory by name.
    pub fn virtual_dir_id(&self, name: &str) -> DbResult<Option<TupId>> {
        self.node_select(DOT_DT, name).map(|opt| opt.map(|n| n.id))
    }

    /// Get the tupid of the "$" (environment) virtual directory.
    pub fn env_dt(&self) -> DbResult<TupId> {
        self.virtual_dir_id("$")?
            .ok_or_else(|| DbError::Other("virtual '$' directory not found".to_string()))
    }

    /// Get the tupid of the "/" (slash) virtual directory.
    pub fn slash_dt(&self) -> DbResult<TupId> {
        self.virtual_dir_id("/")?
            .ok_or_else(|| DbError::Other("virtual '/' directory not found".to_string()))
    }

    /// Get the tupid of the "^" (exclusion) virtual directory.
    pub fn exclusion_dt(&self) -> DbResult<TupId> {
        self.virtual_dir_id("^")?
            .ok_or_else(|| DbError::Other("virtual '^' directory not found".to_string()))
    }

    /// Verify the database schema version matches expectations.
    fn version_check(&self) -> DbResult<()> {
        let db_ver = self.config_get_int("db_version", -1)?;
        if db_ver != DB_VERSION {
            return Err(DbError::VersionMismatch {
                expected: DB_VERSION,
                found: db_ver,
            });
        }

        let parser_ver = self.config_get_int("parser_version", -1)?;
        if parser_ver != PARSER_VERSION {
            return Err(DbError::ParserVersionMismatch {
                expected: PARSER_VERSION,
                found: parser_ver,
            });
        }

        Ok(())
    }
}

/// A row from the `node` table.
#[derive(Debug, Clone)]
pub struct NodeRow {
    pub id: TupId,
    pub dir: TupId,
    pub node_type: NodeType,
    pub mtime: i64,
    pub mtime_ns: i64,
    pub srcid: i64,
    pub name: String,
    pub display: Option<String>,
    pub flags: Option<String>,
}

impl NodeRow {
    pub(crate) fn from_row(row: &rusqlite::Row) -> rusqlite::Result<Self> {
        let type_val: i32 = row.get(2)?;
        Ok(NodeRow {
            id: TupId::new(row.get(0)?),
            dir: TupId::new(row.get(1)?),
            node_type: NodeType::from_i32(type_val).unwrap_or(NodeType::File),
            mtime: row.get(3)?,
            mtime_ns: row.get(4)?,
            srcid: row.get(5)?,
            name: row.get(6)?,
            display: row.get(7)?,
            flags: row.get(8)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tup_types::{LinkType, TupFlags};

    fn test_db() -> TupDb {
        TupDb::create_in_memory().expect("failed to create test database")
    }

    #[test]
    fn test_create_in_memory() {
        let db = test_db();
        let ver = db.config_get_int("db_version", -1).unwrap();
        assert_eq!(ver, DB_VERSION);
        let pver = db.config_get_int("parser_version", -1).unwrap();
        assert_eq!(pver, PARSER_VERSION);
    }

    #[test]
    fn test_root_node_exists() {
        let db = test_db();
        let root = db.node_select_by_id(DOT_DT).unwrap();
        assert!(root.is_some());
        let root = root.unwrap();
        assert_eq!(root.id, DOT_DT);
        assert_eq!(root.name, ".");
        assert_eq!(root.node_type, NodeType::Dir);
        assert_eq!(root.dir, TupId::new(0));
        assert_eq!(root.mtime, -1);
        assert_eq!(root.srcid, -1);
    }

    #[test]
    fn test_virtual_dirs_created() {
        let db = test_db();
        let env = db.env_dt().unwrap();
        let slash = db.slash_dt().unwrap();
        let excl = db.exclusion_dt().unwrap();

        // All should be different
        assert_ne!(env, slash);
        assert_ne!(env, excl);
        assert_ne!(slash, excl);

        // All should be children of DOT_DT
        let env_node = db.node_select(DOT_DT, "$").unwrap().unwrap();
        assert_eq!(env_node.id, env);
        assert_eq!(env_node.node_type, NodeType::Dir);

        let slash_node = db.node_select(DOT_DT, "/").unwrap().unwrap();
        assert_eq!(slash_node.id, slash);

        let excl_node = db.node_select(DOT_DT, "^").unwrap().unwrap();
        assert_eq!(excl_node.id, excl);
    }

    #[test]
    fn test_config_set_get() {
        let db = test_db();
        db.begin().unwrap();
        db.config_set_int("test_key", 42).unwrap();
        let val = db.config_get_int("test_key", -1).unwrap();
        assert_eq!(val, 42);
        db.commit().unwrap();
    }

    #[test]
    fn test_config_get_default() {
        let db = test_db();
        let val = db.config_get_int("nonexistent", 99).unwrap();
        assert_eq!(val, 99);
    }

    #[test]
    fn test_node_insert_and_select() {
        let db = test_db();
        db.begin().unwrap();

        let id = db
            .node_insert(DOT_DT, "hello.c", NodeType::File, 1000, 0, -1, None, None)
            .unwrap();

        let node = db.node_select(DOT_DT, "hello.c").unwrap().unwrap();
        assert_eq!(node.id, id);
        assert_eq!(node.name, "hello.c");
        assert_eq!(node.node_type, NodeType::File);
        assert_eq!(node.mtime, 1000);
        assert_eq!(node.srcid, -1);
        assert!(node.display.is_none());
        assert!(node.flags.is_none());

        db.commit().unwrap();
    }

    #[test]
    fn test_node_insert_with_display() {
        let db = test_db();
        db.begin().unwrap();

        let id = db
            .node_insert(
                DOT_DT,
                "gcc -c foo.c",
                NodeType::Cmd,
                -1,
                0,
                -1,
                Some("CC foo.c"),
                Some("t"),
            )
            .unwrap();

        let node = db.node_select_by_id(id).unwrap().unwrap();
        assert_eq!(node.display, Some("CC foo.c".to_string()));
        assert_eq!(node.flags, Some("t".to_string()));

        db.commit().unwrap();
    }

    #[test]
    fn test_node_unique_constraint() {
        let db = test_db();
        db.begin().unwrap();

        db.node_insert(DOT_DT, "file.c", NodeType::File, 0, 0, -1, None, None)
            .unwrap();
        let result = db.node_insert(DOT_DT, "file.c", NodeType::File, 0, 0, -1, None, None);
        assert!(result.is_err());

        db.rollback().unwrap();
    }

    #[test]
    fn test_node_select_not_found() {
        let db = test_db();
        let result = db.node_select(DOT_DT, "nonexistent").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_node_delete() {
        let db = test_db();
        db.begin().unwrap();

        let id = db
            .node_insert(DOT_DT, "temp.c", NodeType::File, 0, 0, -1, None, None)
            .unwrap();
        assert!(db.node_select_by_id(id).unwrap().is_some());

        let deleted = db.node_delete(id).unwrap();
        assert!(deleted);
        assert!(db.node_select_by_id(id).unwrap().is_none());

        db.commit().unwrap();
    }

    #[test]
    fn test_node_set_type() {
        let db = test_db();
        db.begin().unwrap();

        let id = db
            .node_insert(DOT_DT, "ghost.c", NodeType::Ghost, -1, 0, -1, None, None)
            .unwrap();
        db.node_set_type(id, NodeType::File).unwrap();

        let node = db.node_select_by_id(id).unwrap().unwrap();
        assert_eq!(node.node_type, NodeType::File);

        db.commit().unwrap();
    }

    #[test]
    fn test_node_set_mtime() {
        let db = test_db();
        db.begin().unwrap();

        let id = db
            .node_insert(DOT_DT, "file.c", NodeType::File, 0, 0, -1, None, None)
            .unwrap();
        db.node_set_mtime(id, 12345, 67890).unwrap();

        let node = db.node_select_by_id(id).unwrap().unwrap();
        assert_eq!(node.mtime, 12345);
        assert_eq!(node.mtime_ns, 67890);

        db.commit().unwrap();
    }

    #[test]
    fn test_node_select_dir() {
        let db = test_db();
        db.begin().unwrap();

        db.node_insert(DOT_DT, "a.c", NodeType::File, 0, 0, -1, None, None)
            .unwrap();
        db.node_insert(DOT_DT, "b.c", NodeType::File, 0, 0, -1, None, None)
            .unwrap();
        db.node_insert(DOT_DT, "c.c", NodeType::File, 0, 0, -1, None, None)
            .unwrap();

        let children = db.node_select_dir(DOT_DT).unwrap();
        // DOT_DT has 3 files + 3 virtual dirs ($, /, ^)
        let file_children: Vec<_> = children
            .iter()
            .filter(|n| n.node_type == NodeType::File)
            .collect();
        assert_eq!(file_children.len(), 3);

        db.commit().unwrap();
    }

    #[test]
    fn test_link_insert_and_exists() {
        let db = test_db();
        db.begin().unwrap();

        let file_id = db
            .node_insert(DOT_DT, "input.c", NodeType::File, 0, 0, -1, None, None)
            .unwrap();
        let cmd_id = db
            .node_insert(DOT_DT, "gcc input.c", NodeType::Cmd, -1, 0, -1, None, None)
            .unwrap();

        db.link_insert(file_id, cmd_id, LinkType::Normal).unwrap();
        assert!(db.link_exists(file_id, cmd_id, LinkType::Normal).unwrap());
        assert!(!db.link_exists(file_id, cmd_id, LinkType::Sticky).unwrap());

        db.link_insert(file_id, cmd_id, LinkType::Sticky).unwrap();
        assert!(db.link_exists(file_id, cmd_id, LinkType::Sticky).unwrap());

        db.commit().unwrap();
    }

    #[test]
    fn test_link_idempotent() {
        let db = test_db();
        db.begin().unwrap();

        let a = db
            .node_insert(DOT_DT, "a", NodeType::File, 0, 0, -1, None, None)
            .unwrap();
        let b = db
            .node_insert(DOT_DT, "b", NodeType::Cmd, -1, 0, -1, None, None)
            .unwrap();

        db.link_insert(a, b, LinkType::Normal).unwrap();
        db.link_insert(a, b, LinkType::Normal).unwrap(); // Should not error

        db.commit().unwrap();
    }

    #[test]
    fn test_links_delete() {
        let db = test_db();
        db.begin().unwrap();

        let a = db
            .node_insert(DOT_DT, "a", NodeType::File, 0, 0, -1, None, None)
            .unwrap();
        let b = db
            .node_insert(DOT_DT, "b", NodeType::Cmd, -1, 0, -1, None, None)
            .unwrap();
        db.link_insert(a, b, LinkType::Normal).unwrap();
        db.link_insert(a, b, LinkType::Sticky).unwrap();

        db.links_delete(a).unwrap();
        assert!(!db.link_exists(a, b, LinkType::Normal).unwrap());
        assert!(!db.link_exists(a, b, LinkType::Sticky).unwrap());

        db.commit().unwrap();
    }

    #[test]
    fn test_flag_operations() {
        let db = test_db();
        db.begin().unwrap();

        let id = db
            .node_insert(DOT_DT, "dir", NodeType::Dir, 0, 0, -1, None, None)
            .unwrap();

        assert!(!db.flag_check(id, TupFlags::Create).unwrap());
        db.flag_add(id, TupFlags::Create).unwrap();
        assert!(db.flag_check(id, TupFlags::Create).unwrap());

        db.flag_remove(id, TupFlags::Create).unwrap();
        assert!(!db.flag_check(id, TupFlags::Create).unwrap());

        db.commit().unwrap();
    }

    #[test]
    fn test_flag_idempotent() {
        let db = test_db();
        db.begin().unwrap();

        let id = db
            .node_insert(DOT_DT, "dir", NodeType::Dir, 0, 0, -1, None, None)
            .unwrap();
        db.flag_add(id, TupFlags::Modify).unwrap();
        db.flag_add(id, TupFlags::Modify).unwrap(); // Should not error
        assert!(db.flag_check(id, TupFlags::Modify).unwrap());

        db.commit().unwrap();
    }

    #[test]
    fn test_flag_list() {
        let db = test_db();
        db.begin().unwrap();

        let a = db
            .node_insert(DOT_DT, "a", NodeType::Dir, 0, 0, -1, None, None)
            .unwrap();
        let b = db
            .node_insert(DOT_DT, "b", NodeType::Dir, 0, 0, -1, None, None)
            .unwrap();

        db.flag_add(a, TupFlags::Create).unwrap();
        db.flag_add(b, TupFlags::Create).unwrap();

        let list = db.flag_list(TupFlags::Create).unwrap();
        assert_eq!(list.len(), 2);
        assert!(list.contains(&a));
        assert!(list.contains(&b));

        db.commit().unwrap();
    }

    #[test]
    fn test_var_set_get() {
        let db = test_db();
        db.begin().unwrap();

        let id = db
            .node_insert(DOT_DT, "MY_VAR", NodeType::Var, 0, 0, -1, None, None)
            .unwrap();
        db.var_set(id, "hello world").unwrap();

        let val = db.var_get(id).unwrap();
        assert_eq!(val, Some("hello world".to_string()));

        db.commit().unwrap();
    }

    #[test]
    fn test_var_delete() {
        let db = test_db();
        db.begin().unwrap();

        let id = db
            .node_insert(DOT_DT, "MY_VAR", NodeType::Var, 0, 0, -1, None, None)
            .unwrap();
        db.var_set(id, "value").unwrap();
        assert!(db.var_get(id).unwrap().is_some());

        db.var_delete(id).unwrap();
        assert!(db.var_get(id).unwrap().is_none());

        db.commit().unwrap();
    }

    #[test]
    fn test_create_on_disk() {
        let tmp = tempfile::tempdir().unwrap();
        let db = TupDb::create(tmp.path(), false).unwrap();

        // Verify the database file was created
        assert!(tmp.path().join(".tup").join("db").exists());

        // Verify contents
        let ver = db.config_get_int("db_version", -1).unwrap();
        assert_eq!(ver, DB_VERSION);

        // Verify root node
        let root = db.node_select_by_id(DOT_DT).unwrap().unwrap();
        assert_eq!(root.name, ".");
    }

    #[test]
    fn test_create_already_exists() {
        let tmp = tempfile::tempdir().unwrap();
        TupDb::create(tmp.path(), false).unwrap();

        let result = TupDb::create(tmp.path(), false);
        assert!(matches!(result, Err(DbError::AlreadyExists { .. })));
    }

    #[test]
    fn test_open_existing() {
        let tmp = tempfile::tempdir().unwrap();
        drop(TupDb::create(tmp.path(), false).unwrap());

        let db = TupDb::open(tmp.path(), false).unwrap();
        let ver = db.config_get_int("db_version", -1).unwrap();
        assert_eq!(ver, DB_VERSION);
    }

    #[test]
    fn test_open_not_found() {
        let tmp = tempfile::tempdir().unwrap();
        let result = TupDb::open(tmp.path(), false);
        assert!(matches!(result, Err(DbError::NotFound { .. })));
    }

    #[test]
    fn test_transaction_commit() {
        let db = test_db();
        db.begin().unwrap();
        db.node_insert(DOT_DT, "file.c", NodeType::File, 0, 0, -1, None, None)
            .unwrap();
        db.commit().unwrap();

        // Should persist
        let node = db.node_select(DOT_DT, "file.c").unwrap();
        assert!(node.is_some());
    }

    #[test]
    fn test_transaction_rollback() {
        let db = test_db();
        db.begin().unwrap();
        db.node_insert(DOT_DT, "file.c", NodeType::File, 0, 0, -1, None, None)
            .unwrap();
        db.rollback().unwrap();

        // Should not persist
        let node = db.node_select(DOT_DT, "file.c").unwrap();
        assert!(node.is_none());
    }
}
