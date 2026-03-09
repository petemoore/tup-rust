// tup-server: FUSE/LD_PRELOAD server for dependency tracking
//
// Manages command execution with automatic dependency detection
// via file system interception.

pub mod depfile;
pub mod fuse_server;
pub mod ldpreload;
pub mod process;

pub use depfile::{read_depfile, write_depfile, FileAccess, FileAccessSummary};
pub use fuse_server::{check_fuse_available, FuseConfig, FuseStatus, PassthroughFuse};
pub use ldpreload::LdPreloadLib;
pub use process::{ProcessServer, ServerMode, ServerResult};
