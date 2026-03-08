use std::path::{Path, PathBuf};
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

        /// Number of parallel jobs
        #[arg(short, long)]
        jobs: Option<usize>,
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
        Some(Commands::Upd { keep_going, jobs }) => cmd_upd(keep_going, jobs),
        None => cmd_upd(false, None),
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

fn cmd_upd(keep_going: bool, jobs: Option<usize>) -> anyhow::Result<()> {
    let cwd = std::env::current_dir()?;

    // Find the tup root
    let tup_root = tup_platform::init::find_tup_dir(&cwd)
        .ok_or_else(|| anyhow::anyhow!("No .tup directory found. Run 'tup init' first."))?;

    // Find all Tupfiles in the project
    let tupfiles = tup_platform::scanner::find_tupfiles(&tup_root)
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    if tupfiles.is_empty() {
        println!("[ tup ] No Tupfiles found.");
        return Ok(());
    }

    // Parse all Tupfiles and collect rules
    let mut all_rules: Vec<(std::path::PathBuf, tup_parser::Rule)> = Vec::new();

    for tupfile_rel in &tupfiles {
        let tupfile_path = tup_root.join(tupfile_rel);
        let tupfile_dir = tupfile_path.parent().unwrap_or(&tup_root);

        let content = std::fs::read_to_string(&tupfile_path)?;
        let filename = tupfile_rel.to_string_lossy();
        let mut reader = tup_parser::TupfileReader::parse(&content, &filename)?;
        let rules = reader.evaluate()?;

        for rule in rules {
            all_rules.push((tupfile_dir.to_path_buf(), rule));
        }
    }

    if all_rules.is_empty() {
        println!("[ tup ] No commands to execute.");
        return Ok(());
    }

    let num_jobs = jobs.unwrap_or_else(|| {
        std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1)
    });

    // Group rules by directory and execute
    let mut total_run = 0usize;
    let mut total_failed = 0usize;
    let total_rules = all_rules.len();

    println!("[ tup ] Parsing {} Tupfile(s), {} command(s) to execute.",
        tupfiles.len(), total_rules);

    // Execute rules grouped by directory
    let mut current_dir: Option<std::path::PathBuf> = None;
    let mut dir_rules: Vec<tup_parser::Rule> = Vec::new();

    // Sort by directory for grouping
    all_rules.sort_by(|a, b| a.0.cmp(&b.0));

    for (dir, rule) in all_rules {
        if current_dir.as_ref() != Some(&dir) {
            // Execute accumulated rules for previous directory
            if !dir_rules.is_empty() {
                let work_dir = current_dir.as_ref().unwrap();
                let (run, failed) = execute_dir_rules(
                    work_dir, &dir_rules, keep_going, num_jobs,
                )?;
                total_run += run;
                total_failed += failed;
                dir_rules.clear();
            }
            current_dir = Some(dir);
        }
        dir_rules.push(rule);
    }

    // Execute remaining rules
    if !dir_rules.is_empty() {
        if let Some(ref work_dir) = current_dir {
            let (run, failed) = execute_dir_rules(
                work_dir, &dir_rules, keep_going, num_jobs,
            )?;
            total_run += run;
            total_failed += failed;
        }
    }

    // Summary
    if total_failed > 0 {
        eprintln!("[ tup ] {total_failed} command(s) failed out of {total_run}.");
        process::exit(1);
    } else {
        println!("[ tup ] Updated. {total_run} command(s) ran successfully.");
    }

    Ok(())
}

fn execute_dir_rules(
    work_dir: &Path,
    rules: &[tup_parser::Rule],
    keep_going: bool,
    num_jobs: usize,
) -> anyhow::Result<(usize, usize)> {
    let mut updater = tup_updater::Updater::new(work_dir);
    updater.set_keep_going(keep_going);

    let results = if num_jobs > 1 {
        updater.execute_rules_parallel(rules, num_jobs)
    } else {
        updater.execute_rules(rules)
    }.map_err(|e| anyhow::anyhow!("{e}"))?;

    // Check for missing outputs
    let missing = updater.verify_outputs(&results);
    for msg in &missing {
        eprintln!("tup warning: {msg}");
    }

    Ok((updater.commands_run(), updater.commands_failed()))
}

fn cmd_parse() -> anyhow::Result<()> {
    let cwd = std::env::current_dir()?;

    let tup_root = tup_platform::init::find_tup_dir(&cwd)
        .ok_or_else(|| anyhow::anyhow!("No .tup directory found. Run 'tup init' first."))?;

    let tupfiles = tup_platform::scanner::find_tupfiles(&tup_root)
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    if tupfiles.is_empty() {
        println!("No Tupfiles found.");
        return Ok(());
    }

    let mut total_rules = 0;
    for tupfile_rel in &tupfiles {
        let tupfile_path = tup_root.join(tupfile_rel);
        let content = std::fs::read_to_string(&tupfile_path)?;
        let filename = tupfile_rel.to_string_lossy();
        let mut reader = tup_parser::TupfileReader::parse(&content, &filename)?;
        let rules = reader.evaluate()?;

        if !rules.is_empty() {
            println!("{}:", tupfile_rel.display());
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
            total_rules += rules.len();
        }
    }

    println!("\n{} Tupfile(s), {} rule(s) total.", tupfiles.len(), total_rules);
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
