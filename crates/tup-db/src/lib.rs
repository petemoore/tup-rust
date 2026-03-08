// tup-db: SQLite database layer for the tup build system
//
// This crate manages the .tup/db SQLite database that stores the
// dependency graph, node metadata, variables, and build state.

mod error;
mod schema;

pub use error::DbError;
pub use schema::TupDb;
