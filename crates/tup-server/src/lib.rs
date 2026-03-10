// tup-server: FUSE/LD_PRELOAD server for dependency tracking
//
// Manages command execution with automatic dependency detection
// via file system interception.

pub mod depfile;
#[cfg(feature = "fuse")]
pub mod file_db;
#[cfg(feature = "fuse")]
pub mod fuse_mount;
pub mod fuse_server;
pub mod ldpreload;
#[cfg(unix)]
pub mod master_fork;
pub mod process;
#[cfg(feature = "fuse")]
pub mod tup_fuse;

pub use depfile::{read_depfile, write_depfile, FileAccess, FileAccessSummary};
pub use fuse_server::{check_fuse_available, FuseConfig, FuseStatus, PassthroughFuse};
pub use ldpreload::LdPreloadLib;
#[cfg(unix)]
pub use master_fork::MasterFork;
pub use process::{ProcessServer, ServerMode, ServerResult};
