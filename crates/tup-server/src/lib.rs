// tup-server: FUSE/LD_PRELOAD server for dependency tracking
//
// Manages command execution with automatic dependency detection
// via file system interception.

pub mod depfile;
pub mod process;

pub use depfile::{FileAccess, FileAccessSummary, read_depfile, write_depfile};
pub use process::{ProcessServer, ServerMode, ServerResult};
