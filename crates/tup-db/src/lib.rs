// tup-db: SQLite database layer for the tup build system
//
// This crate manages the .tup/db SQLite database that stores the
// dependency graph, node metadata, variables, and build state.

mod entry;
mod error;
mod link_ops;
mod node_ops;
mod schema;

pub use entry::{EntryCache, TupEntry};
pub use error::DbError;
pub use node_ops::CreateResult;
pub use schema::{NodeRow, TupDb};
