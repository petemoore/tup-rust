// tup-updater: Build updater/executor for the tup build system
//
// Executes build commands from parsed Tupfiles, handling process
// spawning, output capture, and error reporting.

mod executor;
mod incremental;
mod outputs;
mod progress;

pub use executor::{CommandResult, Updater};
pub use incremental::{BuildState, compute_rule_hash, rule_key};
pub use outputs::{snapshot_files, verify_outputs, OutputVerification};
pub use progress::Progress;
