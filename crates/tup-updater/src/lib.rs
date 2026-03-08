// tup-updater: Build updater/executor for the tup build system
//
// Executes build commands from parsed Tupfiles, handling process
// spawning, output capture, and error reporting.

mod executor;
mod outputs;
mod progress;

pub use executor::{CommandResult, Updater};
pub use outputs::{snapshot_files, verify_outputs, OutputVerification};
pub use progress::Progress;
