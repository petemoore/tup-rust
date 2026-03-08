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
    /// Update out-of-date build targets
    Upd {
        /// Keep going after errors
        #[arg(short, long)]
        keep_going: bool,
    },
    /// Display tup configuration options
    Options,
    /// Display version information
    Version,
    /// Parse Tupfiles (without executing commands)
    Parse,
    /// Scan for file changes
    Scan,
    /// Start the file monitor daemon
    Monitor,
    /// Stop the file monitor daemon
    Stop,
    /// Display the dependency graph
    Graph,
    /// Manage variants
    Variant,
}

fn main() {
    env_logger::init();
    let cli = Cli::parse();

    let result = match cli.command {
        Some(Commands::Init { directory, no_sync, force }) => {
            cmd_init(directory, no_sync, force)
        }
        Some(Commands::Upd { keep_going }) => cmd_upd(keep_going),
        None => cmd_upd(false),
        Some(Commands::Parse) => cmd_parse(),
        Some(Commands::Version) => {
            cmd_version();
            Ok(())
        }
        Some(Commands::Options) => {
            cmd_options();
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

fn cmd_upd(keep_going: bool) -> anyhow::Result<()> {
    let cwd = std::env::current_dir()?;

    // Find the tup root
    let _tup_root = tup_platform::init::find_tup_dir(&cwd)
        .ok_or_else(|| anyhow::anyhow!("No .tup directory found. Run 'tup init' first."))?;

    // Look for a Tupfile in the current directory
    let tupfile_path = cwd.join("Tupfile");
    if !tupfile_path.exists() {
        eprintln!("No Tupfile found in current directory.");
        return Ok(());
    }

    // Read and parse the Tupfile
    let content = std::fs::read_to_string(&tupfile_path)?;
    let mut reader = tup_parser::TupfileReader::parse(&content, "Tupfile")?;
    let rules = reader.evaluate()?;

    if rules.is_empty() {
        println!("[ tup ] No commands to execute.");
        return Ok(());
    }

    println!("[ tup ] Executing {} command(s).", rules.len());

    // Execute rules
    let mut updater = tup_updater::Updater::new(&cwd);
    updater.set_keep_going(keep_going);

    let results = updater.execute_rules(&rules)
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    // Check for missing outputs
    let missing = updater.verify_outputs(&results);
    for msg in &missing {
        eprintln!("tup warning: {msg}");
    }

    // Summary
    let failed = updater.commands_failed();
    let total = updater.commands_run();
    if failed > 0 {
        eprintln!("[ tup ] {failed} command(s) failed out of {total}.");
        process::exit(1);
    } else {
        println!("[ tup ] Updated. {total} command(s) ran successfully.");
    }

    Ok(())
}

fn cmd_parse() -> anyhow::Result<()> {
    let cwd = std::env::current_dir()?;

    let tupfile_path = cwd.join("Tupfile");
    if !tupfile_path.exists() {
        eprintln!("No Tupfile found in current directory.");
        return Ok(());
    }

    let content = std::fs::read_to_string(&tupfile_path)?;
    let mut reader = tup_parser::TupfileReader::parse(&content, "Tupfile")?;
    let rules = reader.evaluate()?;

    println!("Parsed {} rule(s) from Tupfile:", rules.len());
    for (i, rule) in rules.iter().enumerate() {
        let inputs = rule.inputs.join(" ");
        let outputs = rule.outputs.join(" ");
        let foreach_str = if rule.foreach { "foreach " } else { "" };
        println!(
            "  [{}] : {foreach_str}{inputs} |> {} |> {outputs}",
            i + 1,
            rule.command.command,
        );
    }

    Ok(())
}

fn cmd_version() {
    println!("tup-rust v{}", env!("CARGO_PKG_VERSION"));
    println!("Platform: {} ({})",
        tup_platform::platform::platform_name(),
        tup_platform::platform::arch_name(),
    );
}

fn cmd_options() {
    let opts = tup_platform::options::TupOptions::new();
    for (name, value) in opts.show() {
        println!("{name} = {value}");
    }
}
