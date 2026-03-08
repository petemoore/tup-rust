// tup-updater: Build updater/executor for the tup build system
//
// Executes build commands from parsed Tupfiles, handling process
// spawning, output capture, and error reporting.

mod executor;

pub use executor::{CommandResult, Updater};
