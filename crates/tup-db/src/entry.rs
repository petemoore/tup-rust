use std::collections::BTreeMap;
use std::sync::atomic::{AtomicI32, Ordering};

use tup_types::{NodeType, TupId};

use crate::error::{DbError, DbResult};
use crate::schema::{NodeRow, TupDb};

/// In-memory cache of a node from the database.
///
/// This corresponds to `struct tup_entry` in the C implementation.
/// It mirrors the `node` table row plus cached relationships.
#[derive(Debug)]
pub struct TupEntry {
    /// Unique node ID (primary key in node table).
    pub id: TupId,
    /// Parent directory tupid.
    pub dt: TupId,
    /// Node type.
    pub node_type: NodeType,
    /// Modification time (seconds). -1 means invalid/unknown.
    pub mtime: i64,
    /// Modification time (nanoseconds).
    pub mtime_ns: i64,
    /// Source variant ID. -1 for normal entries.
    pub srcid: i64,
    /// Entry name in parent directory.
    pub name: String,
    /// Display name (for commands).
    pub display: Option<String>,
    /// Flags string ('t' for transient, 'j' for compiledb).
    pub flags: Option<String>,
    /// Reference count for tracking active users.
    refcount: AtomicI32,
}

impl TupEntry {
    /// Create a TupEntry from a database NodeRow.
    pub fn from_node_row(row: &NodeRow) -> Self {
        TupEntry {
            id: row.id,
            dt: row.dir,
            node_type: row.node_type,
            mtime: row.mtime,
            mtime_ns: row.mtime_ns,
            srcid: row.srcid,
            name: row.name.clone(),
            display: row.display.clone(),
            flags: row.flags.clone(),
            refcount: AtomicI32::new(0),
        }
    }

    /// Increment the reference count.
    pub fn add_ref(&self) {
        self.refcount.fetch_add(1, Ordering::Relaxed);
    }

    /// Decrement the reference count.
    pub fn del_ref(&self) {
        self.refcount.fetch_sub(1, Ordering::Relaxed);
    }

    /// Get the current reference count.
    pub fn ref_count(&self) -> i32 {
        self.refcount.load(Ordering::Relaxed)
    }

    /// Returns true if this entry has the transient flag set.
    ///
    /// In C: `is_transient_tent()` checks if flags contains 't'.
    pub fn is_transient(&self) -> bool {
        self.flags.as_ref().is_some_and(|f| f.contains('t'))
    }

    /// Returns true if this entry should appear in compile_commands.json.
    ///
    /// In C: `is_compiledb_tent()` checks if flags contains 'j'.
    pub fn is_compiledb(&self) -> bool {
        self.flags.as_ref().is_some_and(|f| f.contains('j'))
    }

    /// Get the full path of this entry by walking the parent chain.
    ///
    /// Requires access to the entry cache to resolve parents.
    pub fn full_path(&self, cache: &EntryCache) -> String {
        let mut parts = vec![self.name.as_str()];
        let mut current_dt = self.dt;

        while current_dt.raw() > 0 {
            if let Some(parent) = cache.find(current_dt) {
                if parent.name == "." {
                    break;
                }
                parts.push(&parent.name);
                current_dt = parent.dt;
            } else {
                break;
            }
        }

        parts.reverse();
        parts.join("/")
    }
}

/// In-memory cache of `tup_entry` nodes.
///
/// This corresponds to the global `tup_root` RB-tree in the C implementation.
/// The cache provides fast lookup by TupId and maintains parent-child
/// relationships.
pub struct EntryCache {
    entries: BTreeMap<TupId, TupEntry>,
    /// Maps (parent_id, child_name) → child TupId for directory lookups.
    name_index: BTreeMap<(TupId, String), TupId>,
}

impl EntryCache {
    /// Create a new empty entry cache.
    pub fn new() -> Self {
        EntryCache {
            entries: BTreeMap::new(),
            name_index: BTreeMap::new(),
        }
    }

    /// Find an entry by its TupId. Returns None if not cached.
    pub fn find(&self, id: TupId) -> Option<&TupEntry> {
        self.entries.get(&id)
    }

    /// Find an entry by its TupId. Panics if not found.
    ///
    /// Matches the behavior of `tup_entry_get()` in C.
    pub fn get(&self, id: TupId) -> &TupEntry {
        self.entries.get(&id).unwrap_or_else(|| {
            panic!("tup internal error: Unable to find tup entry {id} in get()");
        })
    }

    /// Find a child entry by parent directory and name.
    ///
    /// Matches `tup_entry_find_name_in_dir()` in C.
    pub fn find_name_in_dir(&self, parent_id: TupId, name: &str) -> Option<&TupEntry> {
        self.name_index
            .get(&(parent_id, name.to_string()))
            .and_then(|id| self.entries.get(id))
    }

    /// Add an entry to the cache.
    ///
    /// If the entry already exists, returns a reference to the existing entry.
    pub fn add(&mut self, entry: TupEntry) -> &TupEntry {
        let id = entry.id;
        let parent = entry.dt;
        let name = entry.name.clone();

        self.entries.entry(id).or_insert(entry);
        self.name_index.insert((parent, name), id);

        self.entries.get(&id).unwrap()
    }

    /// Load an entry from the database into the cache, along with its
    /// ancestor chain.
    ///
    /// Matches `tup_entry_add()` in C, which recursively loads parents.
    pub fn load(&mut self, db: &TupDb, id: TupId) -> DbResult<&TupEntry> {
        if self.entries.contains_key(&id) {
            return Ok(self.entries.get(&id).unwrap());
        }

        let row = db.node_select_by_id(id)?.ok_or(DbError::NodeNotFound(id))?;

        let entry = TupEntry::from_node_row(&row);
        let parent_dt = entry.dt;

        self.add(entry);

        // Recursively load parent (if it has a valid parent)
        if parent_dt.raw() > 0 && !self.entries.contains_key(&parent_dt) {
            self.load(db, parent_dt)?;
        }

        Ok(self.entries.get(&id).unwrap())
    }

    /// Load all entries from the database for a given directory.
    pub fn load_dir(&mut self, db: &TupDb, dir: TupId) -> DbResult<Vec<TupId>> {
        let rows = db.node_select_dir(dir)?;
        let mut ids = Vec::with_capacity(rows.len());
        for row in &rows {
            let entry = TupEntry::from_node_row(row);
            ids.push(entry.id);
            self.add(entry);
        }
        Ok(ids)
    }

    /// Remove an entry from the cache.
    ///
    /// Returns an error if the entry still has children or a non-zero refcount.
    pub fn remove(&mut self, id: TupId) -> DbResult<()> {
        let entry = match self.entries.get(&id) {
            Some(e) => e,
            None => return Ok(()), // Not cached, nothing to do
        };

        // Check refcount
        if entry.ref_count() != 0 {
            return Err(DbError::Other(format!(
                "cannot remove entry {id}: refcount is {}",
                entry.ref_count()
            )));
        }

        // Check no children reference this as parent
        let has_children = self.entries.values().any(|e| e.dt == id && e.id != id);
        if has_children {
            return Err(DbError::Other(format!(
                "cannot remove entry {id}: still has children"
            )));
        }

        // Remove from name index
        let parent = entry.dt;
        let name = entry.name.clone();
        self.name_index.remove(&(parent, name));

        // Remove from entries
        self.entries.remove(&id);

        Ok(())
    }

    /// Update the name and parent of a cached entry.
    pub fn change_name(&mut self, id: TupId, new_name: &str, new_dt: TupId) -> DbResult<()> {
        // Remove old name index entry
        if let Some(entry) = self.entries.get(&id) {
            let old_parent = entry.dt;
            let old_name = entry.name.clone();
            self.name_index.remove(&(old_parent, old_name));
        }

        // Update the entry
        if let Some(entry) = self.entries.get_mut(&id) {
            entry.name = new_name.to_string();
            entry.dt = new_dt;
        }

        // Add new name index entry
        self.name_index.insert((new_dt, new_name.to_string()), id);

        Ok(())
    }

    /// Update the display string of a cached entry.
    pub fn change_display(&mut self, id: TupId, display: Option<&str>) {
        if let Some(entry) = self.entries.get_mut(&id) {
            entry.display = display.map(String::from);
        }
    }

    /// Update the flags string of a cached entry.
    pub fn change_flags(&mut self, id: TupId, flags: Option<&str>) {
        if let Some(entry) = self.entries.get_mut(&id) {
            entry.flags = flags.map(String::from);
        }
    }

    /// Update the node type of a cached entry.
    pub fn change_type(&mut self, id: TupId, node_type: NodeType) {
        if let Some(entry) = self.entries.get_mut(&id) {
            entry.node_type = node_type;
        }
    }

    /// Get the number of cached entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Check if the cache is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Clear all cached entries.
    pub fn clear(&mut self) {
        self.entries.clear();
        self.name_index.clear();
    }

    /// Iterate over all cached entries.
    pub fn iter(&self) -> impl Iterator<Item = (&TupId, &TupEntry)> {
        self.entries.iter()
    }

    /// Get the full path of an entry by walking the parent chain.
    pub fn full_path(&self, id: TupId) -> Option<String> {
        self.entries.get(&id).map(|e| e.full_path(self))
    }

    /// Compute the relative path from `start` to `end`.
    ///
    /// Both entries must be in the cache. Returns None if either is missing
    /// or no common ancestor exists.
    pub fn relative_path(&self, start: TupId, end: TupId) -> Option<String> {
        let start_path = self.ancestor_chain(start)?;
        let end_path = self.ancestor_chain(end)?;

        // Find common prefix length
        let common = start_path
            .iter()
            .zip(end_path.iter())
            .take_while(|(a, b)| a == b)
            .count();

        let up_count = start_path.len() - common;
        let mut parts: Vec<&str> = (0..up_count).map(|_| "..").collect();
        for &id in &end_path[common..] {
            if let Some(entry) = self.find(id) {
                parts.push(&entry.name);
            }
        }

        if parts.is_empty() {
            Some(".".to_string())
        } else {
            Some(parts.join("/"))
        }
    }

    /// Get the chain of ancestor TupIds from root to this entry (inclusive).
    fn ancestor_chain(&self, id: TupId) -> Option<Vec<TupId>> {
        let mut chain = Vec::new();
        let mut current = id;

        loop {
            let entry = self.entries.get(&current)?;
            chain.push(current);
            if entry.dt.raw() <= 0 || entry.dt == current {
                break;
            }
            current = entry.dt;
        }

        chain.reverse();
        Some(chain)
    }
}

impl Default for EntryCache {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tup_types::DOT_DT;

    fn make_entry(id: i64, dt: i64, name: &str, node_type: NodeType) -> TupEntry {
        TupEntry {
            id: TupId::new(id),
            dt: TupId::new(dt),
            node_type,
            mtime: -1,
            mtime_ns: 0,
            srcid: -1,
            name: name.to_string(),
            display: None,
            flags: None,
            refcount: AtomicI32::new(0),
        }
    }

    #[test]
    fn test_entry_cache_add_and_find() {
        let mut cache = EntryCache::new();
        let entry = make_entry(1, 0, ".", NodeType::Dir);
        cache.add(entry);

        assert!(cache.find(TupId::new(1)).is_some());
        assert!(cache.find(TupId::new(999)).is_none());
    }

    #[test]
    fn test_entry_cache_get_panics() {
        let mut cache = EntryCache::new();
        let entry = make_entry(1, 0, ".", NodeType::Dir);
        cache.add(entry);

        let _ = cache.get(TupId::new(1)); // Should not panic
    }

    #[test]
    #[should_panic(expected = "Unable to find tup entry")]
    fn test_entry_cache_get_missing_panics() {
        let cache = EntryCache::new();
        let _ = cache.get(TupId::new(999));
    }

    #[test]
    fn test_find_name_in_dir() {
        let mut cache = EntryCache::new();
        cache.add(make_entry(1, 0, ".", NodeType::Dir));
        cache.add(make_entry(2, 1, "src", NodeType::Dir));
        cache.add(make_entry(3, 2, "main.c", NodeType::File));

        let found = cache.find_name_in_dir(TupId::new(1), "src");
        assert!(found.is_some());
        assert_eq!(found.unwrap().id, TupId::new(2));

        let found = cache.find_name_in_dir(TupId::new(2), "main.c");
        assert!(found.is_some());
        assert_eq!(found.unwrap().id, TupId::new(3));

        assert!(cache
            .find_name_in_dir(TupId::new(1), "nonexistent")
            .is_none());
    }

    #[test]
    fn test_entry_cache_remove() {
        let mut cache = EntryCache::new();
        cache.add(make_entry(1, 0, ".", NodeType::Dir));
        cache.add(make_entry(5, 1, "file.c", NodeType::File));

        cache.remove(TupId::new(5)).unwrap();
        assert!(cache.find(TupId::new(5)).is_none());
        assert!(cache.find_name_in_dir(TupId::new(1), "file.c").is_none());
    }

    #[test]
    fn test_entry_cache_remove_with_refcount() {
        let mut cache = EntryCache::new();
        let entry = make_entry(5, 1, "file.c", NodeType::File);
        cache.add(entry);
        cache.find(TupId::new(5)).unwrap().add_ref();

        let result = cache.remove(TupId::new(5));
        assert!(result.is_err());
    }

    #[test]
    fn test_entry_cache_remove_with_children() {
        let mut cache = EntryCache::new();
        cache.add(make_entry(1, 0, ".", NodeType::Dir));
        cache.add(make_entry(5, 1, "file.c", NodeType::File));

        let result = cache.remove(TupId::new(1));
        assert!(result.is_err()); // Has child entry 5
    }

    #[test]
    fn test_change_name() {
        let mut cache = EntryCache::new();
        cache.add(make_entry(1, 0, ".", NodeType::Dir));
        cache.add(make_entry(2, 1, "old.c", NodeType::File));

        cache
            .change_name(TupId::new(2), "new.c", TupId::new(1))
            .unwrap();

        assert!(cache.find_name_in_dir(TupId::new(1), "old.c").is_none());
        assert!(cache.find_name_in_dir(TupId::new(1), "new.c").is_some());
        assert_eq!(cache.get(TupId::new(2)).name, "new.c");
    }

    #[test]
    fn test_change_display() {
        let mut cache = EntryCache::new();
        cache.add(make_entry(1, 0, "cmd", NodeType::Cmd));

        cache.change_display(TupId::new(1), Some("CC main.c"));
        assert_eq!(
            cache.get(TupId::new(1)).display,
            Some("CC main.c".to_string())
        );
    }

    #[test]
    fn test_change_type() {
        let mut cache = EntryCache::new();
        cache.add(make_entry(1, 0, "ghost", NodeType::Ghost));

        cache.change_type(TupId::new(1), NodeType::File);
        assert_eq!(cache.get(TupId::new(1)).node_type, NodeType::File);
    }

    #[test]
    fn test_refcount() {
        let entry = make_entry(1, 0, ".", NodeType::Dir);
        assert_eq!(entry.ref_count(), 0);

        entry.add_ref();
        entry.add_ref();
        assert_eq!(entry.ref_count(), 2);

        entry.del_ref();
        assert_eq!(entry.ref_count(), 1);
    }

    #[test]
    fn test_is_transient() {
        let mut entry = make_entry(1, 0, "output", NodeType::Generated);
        assert!(!entry.is_transient());

        entry.flags = Some("t".to_string());
        assert!(entry.is_transient());

        entry.flags = Some("tj".to_string());
        assert!(entry.is_transient());
    }

    #[test]
    fn test_is_compiledb() {
        let mut entry = make_entry(1, 0, "cmd", NodeType::Cmd);
        assert!(!entry.is_compiledb());

        entry.flags = Some("j".to_string());
        assert!(entry.is_compiledb());
    }

    #[test]
    fn test_full_path() {
        let mut cache = EntryCache::new();
        cache.add(make_entry(1, 0, ".", NodeType::Dir));
        cache.add(make_entry(2, 1, "src", NodeType::Dir));
        cache.add(make_entry(3, 2, "lib", NodeType::Dir));
        cache.add(make_entry(4, 3, "main.c", NodeType::File));

        assert_eq!(
            cache.full_path(TupId::new(4)),
            Some("src/lib/main.c".to_string())
        );
        assert_eq!(cache.full_path(TupId::new(2)), Some("src".to_string()));
    }

    #[test]
    fn test_relative_path() {
        let mut cache = EntryCache::new();
        cache.add(make_entry(1, 0, ".", NodeType::Dir));
        cache.add(make_entry(2, 1, "src", NodeType::Dir));
        cache.add(make_entry(3, 1, "build", NodeType::Dir));
        cache.add(make_entry(4, 2, "lib", NodeType::Dir));

        let rel = cache.relative_path(TupId::new(4), TupId::new(3));
        assert_eq!(rel, Some("../../build".to_string()));

        let rel = cache.relative_path(TupId::new(2), TupId::new(2));
        assert_eq!(rel, Some(".".to_string()));
    }

    #[test]
    fn test_load_from_db() {
        let db = TupDb::create_in_memory().unwrap();
        let mut cache = EntryCache::new();

        // Load the root node
        cache.load(&db, DOT_DT).unwrap();
        let root = cache.find(DOT_DT).unwrap();
        assert_eq!(root.name, ".");
        assert_eq!(root.node_type, NodeType::Dir);
    }

    #[test]
    fn test_load_with_parent_chain() {
        let db = TupDb::create_in_memory().unwrap();
        db.begin().unwrap();

        // Create a directory under root
        let dir_id = db
            .node_insert(DOT_DT, "mydir", NodeType::Dir, -1, 0, -1, None, None)
            .unwrap();
        // Create a file under that directory
        let file_id = db
            .node_insert(dir_id, "hello.c", NodeType::File, 1000, 0, -1, None, None)
            .unwrap();
        db.commit().unwrap();

        let mut cache = EntryCache::new();
        // Loading the file should also load its parent chain
        cache.load(&db, file_id).unwrap();

        assert!(cache.find(file_id).is_some());
        assert!(cache.find(dir_id).is_some());
        assert!(cache.find(DOT_DT).is_some());

        let file = cache.get(file_id);
        assert_eq!(file.name, "hello.c");
        assert_eq!(file.dt, dir_id);

        // Full path should work
        assert_eq!(cache.full_path(file_id), Some("mydir/hello.c".to_string()));
    }

    #[test]
    fn test_load_dir() {
        let db = TupDb::create_in_memory().unwrap();
        db.begin().unwrap();
        db.node_insert(DOT_DT, "a.c", NodeType::File, 0, 0, -1, None, None)
            .unwrap();
        db.node_insert(DOT_DT, "b.c", NodeType::File, 0, 0, -1, None, None)
            .unwrap();
        db.commit().unwrap();

        let mut cache = EntryCache::new();
        let ids = cache.load_dir(&db, DOT_DT).unwrap();

        // Should have loaded the virtual dirs ($, /, ^) plus our two files
        assert!(ids.len() >= 2);

        // Can find by name
        assert!(cache.find_name_in_dir(DOT_DT, "a.c").is_some());
        assert!(cache.find_name_in_dir(DOT_DT, "b.c").is_some());
    }

    #[test]
    fn test_cache_len_and_clear() {
        let mut cache = EntryCache::new();
        assert!(cache.is_empty());

        cache.add(make_entry(1, 0, ".", NodeType::Dir));
        cache.add(make_entry(2, 1, "file", NodeType::File));
        assert_eq!(cache.len(), 2);

        cache.clear();
        assert!(cache.is_empty());
    }

    #[test]
    fn test_duplicate_add_keeps_first() {
        let mut cache = EntryCache::new();
        let e1 = make_entry(1, 0, ".", NodeType::Dir);
        let e2 = make_entry(1, 0, ".", NodeType::File); // Same ID, different type

        cache.add(e1);
        cache.add(e2);

        // Should keep the first entry (Dir, not File)
        assert_eq!(cache.get(TupId::new(1)).node_type, NodeType::Dir);
    }
}
