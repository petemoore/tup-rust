// tup-updater: Build updater/executor for the tup build system
//
// Executes build commands from parsed Tupfiles, handling process
// spawning, output capture, and error reporting.

mod ccache;
mod executor;
mod incremental;
mod outputs;
mod progress;

pub use ccache::CcacheConfig;
pub use executor::{CommandResult, Updater};
pub use incremental::{compute_rule_hash, rule_key, BuildState};
pub use outputs::{snapshot_files, verify_outputs, OutputVerification};
pub use progress::Progress;
