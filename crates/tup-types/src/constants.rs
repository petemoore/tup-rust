use crate::TupId;

/// The `.tup` directory name where tup stores its database and state.
pub const TUP_DIR: &str = ".tup";

/// The path to the SQLite database file.
pub const TUP_DB_FILE: &str = ".tup/db";

/// The tupid of the root directory (`.`).
///
/// In the C implementation this is `DOT_DT = 1`.
pub const DOT_DT: TupId = TupId::new_const(1);

/// Current database schema version.
pub const DB_VERSION: i32 = 19;

/// Current Tupfile parser version.
pub const PARSER_VERSION: i32 = 16;

/// Environment variable name for the dependency file path.
pub const TUP_DEPFILE: &str = "TUP_DEPFILE";

/// Name of the variable dictionary.
pub const TUP_VARDICT_NAME: &str = "tup_vardict";

/// Virtual directory for @-variable dependencies.
pub const TUP_VAR_VIRTUAL_DIR: &str = "@tup@";

/// Maximum path length for Windows wide paths.
pub const WIDE_PATH_MAX: usize = 32767;
