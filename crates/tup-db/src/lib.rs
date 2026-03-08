// tup-db: SQLite database layer for the tup build system
//
// This crate manages the .tup/db SQLite database that stores the
// dependency graph, node metadata, variables, and build state.

mod commands;
mod entry;
mod error;
mod ghost;
mod link_ops;
mod node_ops;
mod output_tracking;
mod path_resolve;
mod schema;
mod sync;
mod vardb;
mod variant;

pub use commands::{get_modified_commands, mark_command_done, store_rules, RuleToStore, StoredCommand};
pub use entry::{EntryCache, TupEntry};
pub use error::DbError;
pub use node_ops::CreateResult;
pub use output_tracking::{track_outputs, OutputTrackResult};
pub use path_resolve::{add_dir_input, resolve_full_path, resolve_path};
pub use schema::{NodeRow, TupDb};
pub use sync::{sync_filesystem, SyncResult};
pub use vardb::{VarDb, VarEntry};
pub use variant::{parse_tup_config, Variant, VariantRegistry};
