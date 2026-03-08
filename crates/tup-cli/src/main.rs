use std::path::PathBuf;
use std::process;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "tup")]
#[command(about = "A file-based build system")]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Initialize a tup project
    Init {
        /// Directory to initialize (defaults to current directory)
        directory: Option<PathBuf>,

        /// Disable database sync for faster operations
        #[arg(long)]
        no_sync: bool,

        /// Force initialization even if .tup already exists
        #[arg(long)]
        force: bool,
    },
    /// Scan for file changes and update the build graph
    Scan,
    /// Parse Tupfiles and update the DAG
    Parse,
    /// Update out-of-date build targets
    Upd,
    /// Start the file monitor daemon
    Monitor,
    /// Stop the file monitor daemon
    Stop,
    /// Display the dependency graph
    Graph,
    /// Display tup configuration options
    Options,
    /// Manage variants
    Variant,
    /// Display version information
    Version,
}

fn main() {
    env_logger::init();
    let cli = Cli::parse();

    let result = match cli.command {
        Some(Commands::Init { directory, no_sync, force }) => {
            cmd_init(directory, no_sync, force)
        }
        Some(Commands::Version) => {
            cmd_version();
            Ok(())
        }
        Some(Commands::Options) => {
            cmd_options();
            Ok(())
        }
        Some(Commands::Upd) | None => {
            // Default behavior: scan + parse + update
            eprintln!("tup upd: not yet implemented");
            Ok(())
        }
        Some(_) => {
            eprintln!("Command not yet implemented");
            Ok(())
        }
    };

    if let Err(e) = result {
        eprintln!("tup error: {e}");
        process::exit(1);
    }
}

fn cmd_init(directory: Option<PathBuf>, no_sync: bool, force: bool) -> anyhow::Result<()> {
    let dir = directory.unwrap_or_else(|| PathBuf::from("."));

    // Create the directory if it doesn't exist
    if !dir.exists() {
        std::fs::create_dir_all(&dir)?;
    }

    let db_sync = !no_sync;

    match tup_platform::init::init_command(&dir, db_sync, force) {
        Ok(()) => {
            println!(".tup repository initialized: {}", dir.join(".tup/db").display());
            Ok(())
        }
        Err(tup_platform::init::InitError::AlreadyInitialized(path)) => {
            eprintln!("tup warning: database already exists in directory: {}", path.display());
            process::exit(1);
        }
        Err(e) => Err(e.into()),
    }
}

fn cmd_version() {
    println!("tup-rust v{}", env!("CARGO_PKG_VERSION"));
    println!("Platform: {} ({})", tup_platform::platform::platform_name(), tup_platform::platform::arch_name());
}

fn cmd_options() {
    let opts = tup_platform::options::TupOptions::new();
    for (name, value) in opts.show() {
        println!("{name} = {value}");
    }
}
