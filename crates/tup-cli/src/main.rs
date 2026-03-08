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
    /// Initialize a tup project in the current directory
    Init,
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
    /// Display the build status
    Status,
    /// Display tup configuration options
    Options,
    /// Manage variants
    Variant,
    /// Run a privileged server (FUSE)
    Privileged,
}

fn main() -> anyhow::Result<()> {
    env_logger::init();
    let cli = Cli::parse();

    match cli.command {
        Some(Commands::Init) => {
            println!("tup init: not yet implemented");
        }
        Some(Commands::Upd) => {
            println!("tup upd: not yet implemented");
        }
        None => {
            // Default behavior: scan + parse + update
            println!("tup: not yet implemented");
        }
        Some(_) => {
            println!("Command not yet implemented");
        }
    }

    Ok(())
}
