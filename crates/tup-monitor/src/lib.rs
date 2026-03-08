// tup-monitor: File system monitor for the tup build system
//
// Watches the project directory for file changes and records them
// for incremental builds.

mod watcher;

pub use watcher::{deduplicate_events, FileEvent, FileEventKind, Monitor};
