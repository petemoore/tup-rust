use crate::{NodeType, TupId};

/// Trait providing database operations needed during Tupfile parsing.
///
/// In C tup, the parser has full DB access via `struct tupfile *tf`.
/// This trait abstracts the specific DB operations the parser needs,
/// allowing `tup-parser` to depend on `tup-types` (not `tup-db`),
/// while `tup-cli` provides the real implementation.
///
/// Operations match C tup's parser.c usage of db.c functions.
pub trait ParserDb {
    /// Look up a node by name in a directory.
    /// C: get_tent_dt(dt, name)
    fn node_lookup(&self, dir_id: TupId, name: &str) -> Option<ParserNode>;

    /// Look up a node by name in the current Tupfile's directory.
    /// C: get_tent_dt(tf->curtent->tnode.tupid, name)
    fn node_lookup_in_dir(&self, name: &str) -> Option<ParserNode>;

    /// Get the relative path from the current directory to a node.
    /// C: get_relative_dir(NULL, &e, tf->curtent->tnode.tupid, tid)
    fn get_relative_path(&self, node_id: TupId) -> Option<String>;

    /// List all files in the current directory.
    /// C: gen_dir_list(tf, tf->tent->tnode.tupid)
    fn list_dir_files(&self) -> Vec<String>;

    /// Get the current directory's TupId.
    fn current_dir_id(&self) -> TupId;
}

/// A simplified node representation for the parser.
/// Contains just enough information for parser operations.
#[derive(Debug, Clone)]
pub struct ParserNode {
    pub id: TupId,
    pub name: String,
    pub node_type: NodeType,
    pub dir: TupId,
}

/// A no-op implementation for when DB access is not available.
/// Used for unit tests and standalone parsing.
pub struct NoopParserDb;

impl ParserDb for NoopParserDb {
    fn node_lookup(&self, _dir_id: TupId, _name: &str) -> Option<ParserNode> {
        None
    }
    fn node_lookup_in_dir(&self, _name: &str) -> Option<ParserNode> {
        None
    }
    fn get_relative_path(&self, _node_id: TupId) -> Option<String> {
        None
    }
    fn list_dir_files(&self) -> Vec<String> {
        vec![]
    }
    fn current_dir_id(&self) -> TupId {
        crate::DOT_DT
    }
}
