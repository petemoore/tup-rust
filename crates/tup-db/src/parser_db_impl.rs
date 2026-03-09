use tup_types::{NodeType, ParserDb, ParserNode, TupId};

use crate::schema::TupDb;

/// Implementation of ParserDb backed by a real TupDb and a specific directory.
///
/// This provides the database operations that C tup's parser has access to
/// via `struct tupfile *tf`. The `dir_id` corresponds to `tf->curtent->tnode.tupid`.
pub struct TupParserDb<'a> {
    db: &'a TupDb,
    dir_id: TupId,
}

impl<'a> TupParserDb<'a> {
    pub fn new(db: &'a TupDb, dir_id: TupId) -> Self {
        Self { db, dir_id }
    }
}

impl<'a> ParserDb for TupParserDb<'a> {
    fn node_lookup(&self, dir_id: TupId, name: &str) -> Option<ParserNode> {
        self.db
            .node_select(dir_id, name)
            .ok()
            .flatten()
            .map(|row| ParserNode {
                id: row.id,
                name: row.name,
                node_type: row.node_type,
                dir: row.dir,
            })
    }

    fn node_lookup_in_dir(&self, name: &str) -> Option<ParserNode> {
        self.node_lookup(self.dir_id, name)
    }

    fn get_relative_path(&self, _node_id: TupId) -> Option<String> {
        // TODO: implement get_relative_dir() matching C tup's algorithm
        // For now, just return the node's name
        None
    }

    fn list_dir_files(&self) -> Vec<String> {
        self.db
            .node_select_dir(self.dir_id)
            .unwrap_or_default()
            .into_iter()
            .filter(|n| n.node_type == NodeType::File || n.node_type == NodeType::Generated)
            .map(|n| n.name)
            .collect()
    }

    fn current_dir_id(&self) -> TupId {
        self.dir_id
    }
}
