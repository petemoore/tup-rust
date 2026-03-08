/// Errors specific to database operations.
#[derive(Debug, thiserror::Error)]
pub enum DbError {
    #[error("SQLite error: {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error("database already exists at {path}")]
    AlreadyExists { path: String },

    #[error("database not found at {path}")]
    NotFound { path: String },

    #[error("schema version mismatch: expected {expected}, found {found}")]
    VersionMismatch { expected: i32, found: i32 },

    #[error("parser version mismatch: expected {expected}, found {found}")]
    ParserVersionMismatch { expected: i32, found: i32 },

    #[error("invalid node type: {0}")]
    InvalidNodeType(i32),

    #[error("invalid link type: {0}")]
    InvalidLinkType(i32),

    #[error("node not found: {0}")]
    NodeNotFound(tup_types::TupId),

    #[error("{0}")]
    Other(String),
}

pub type DbResult<T> = Result<T, DbError>;
