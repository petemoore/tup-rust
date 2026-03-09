use std::path::{Path, PathBuf};
use std::process;

use clap::{Parser, Subcommand};
use tup_types::{NodeType, TupId, DOT_DT};

#[derive(Parser)]
#[command(name = "tup")]
#[command(about = "A file-based build system")]
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

        /// Skip filesystem scan
        #[arg(long)]
        no_scan: bool,
    },
    /// Display tup configuration options
    Options {
        /// Number of parallel jobs (override for display)
        #[arg(short, long)]
        jobs: Option<usize>,
    },
    /// Display version information
    Version,
    /// Parse Tupfiles (without executing commands)
    Parse,
    /// Scan for file changes and sync to database
    Scan,
    /// Start the file monitor daemon
    Monitor,
    /// Stop the file monitor daemon
    Stop,
    /// Display the dependency graph in Graphviz DOT format
    Graph {
        /// Directory to graph (defaults to project root)
        directory: Option<String>,

        /// Show directory nodes
        #[arg(long)]
        dirs: bool,
    },
    /// Manage variants
    Variant,

    // -- Testing/debugging commands used by C test suite --

    /// Register files with the database
    Touch {
        /// File paths to register
        files: Vec<String>,
    },
    /// Check if a node exists in the database
    #[command(name = "node_exists")]
    NodeExists {
        /// Directory containing the node (relative to tup root)
        dir: String,
        /// Node name to look for
        name: String,
    },
    /// Check if any nodes have pending flags
    #[command(name = "flags_exists")]
    FlagsExists,
    /// Check if a normal dependency link exists
    #[command(name = "normal_exists")]
    NormalExists {
        /// Source directory
        dir1: String,
        /// Source node name
        name1: String,
        /// Destination directory
        dir2: String,
        /// Destination node name
        name2: String,
    },
    /// Check if a sticky dependency link exists
    #[command(name = "sticky_exists")]
    StickyExists {
        /// Source directory
        dir1: String,
        /// Source node name
        name1: String,
        /// Destination directory
        dir2: String,
        /// Destination node name
        name2: String,
    },
    /// Display the server mode
    Server,
}

fn main() {
    env_logger::init();

    // Handle -v / --version before clap (C tup compatibility)
    let args: Vec<String> = std::env::args().collect();
    if args.len() == 2 && (args[1] == "-v" || args[1] == "--version") {
        cmd_version();
        return;
    }

    let cli = Cli::parse();

    let result = match cli.command {
        Some(Commands::Init { directory, no_sync, force }) => {
            cmd_init(directory, no_sync, force)
        }
        Some(Commands::Upd { keep_going, jobs, no_scan }) => cmd_upd(keep_going, jobs, no_scan),
        None => cmd_upd(false, None, false),
        Some(Commands::Parse) => cmd_parse(),
        Some(Commands::Version) => {
            cmd_version();
            Ok(())
        }
        Some(Commands::Options { jobs }) => {
            cmd_options(jobs);
            Ok(())
        }
        Some(Commands::Graph { directory: _, dirs: _ }) => cmd_graph(),
        Some(Commands::Scan) => cmd_scan(),
        Some(Commands::Touch { files }) => cmd_touch(files),
        Some(Commands::NodeExists { dir, name }) => cmd_node_exists(&dir, &name),
        Some(Commands::FlagsExists) => cmd_flags_exists(),
        Some(Commands::NormalExists { dir1, name1, dir2, name2 }) => {
            cmd_link_exists(&dir1, &name1, &dir2, &name2, tup_types::LinkType::Normal)
        }
        Some(Commands::StickyExists { dir1, name1, dir2, name2 }) => {
            cmd_link_exists(&dir1, &name1, &dir2, &name2, tup_types::LinkType::Sticky)
        }
        Some(Commands::Server) => {
            cmd_server();
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

    // Without --force, check if .tup exists in any parent directory
    if !force {
        let check_dir = if dir.is_absolute() {
            dir.clone()
        } else {
            std::env::current_dir()?.join(&dir)
        };
        if tup_platform::init::find_tup_dir(&check_dir).is_some() {
            eprintln!("tup warning: database already exists in a parent directory");
            process::exit(1);
        }
    }

    // If force, remove existing database so it can be re-created
    if force {
        let db_path = dir.join(".tup").join("db");
        if db_path.exists() {
            std::fs::remove_file(&db_path)?;
        }
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

fn cmd_upd(keep_going: bool, jobs: Option<usize>, no_scan: bool) -> anyhow::Result<()> {
    let cwd = std::env::current_dir()?;

    // Find the tup root
    let tup_root = tup_platform::init::find_tup_dir(&cwd)
        .ok_or_else(|| anyhow::anyhow!("No .tup directory found. Run 'tup init' first."))?;

    // Phase 1: Scan filesystem and sync to database (unless --no-scan)
    let db = tup_db::TupDb::open(&tup_root, false)?;
    let mut cache = tup_db::EntryCache::new();

    if !no_scan {
        let sync_result = tup_db::sync_filesystem(&db, &mut cache, &tup_root)?;

        if sync_result.files_added > 0 || sync_result.files_modified > 0 || sync_result.files_deleted > 0 {
            eprintln!(
                "[ tup ] Scan: {} new, {} modified, {} deleted",
                sync_result.files_added, sync_result.files_modified, sync_result.files_deleted,
            );
        }
    }

    // Phase 2: Parse Tupfiles and store commands in database
    let tupfiles = tup_platform::scanner::find_tupfiles(&tup_root)
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    if !tupfiles.is_empty() {
        db.begin()?;

        let mut total_stored = 0usize;
        for tupfile_rel in &tupfiles {
            let tupfile_path = tup_root.join(tupfile_rel);
            let tupfile_dir = tupfile_path.parent().unwrap_or(&tup_root);
            let filename = tupfile_rel.to_string_lossy();

            let rules = parse_tupfile_any(&tupfile_path, tupfile_dir, &tup_root, &filename)?;
            let expanded = expand_rules_for_dir(&rules, tupfile_dir)?;

            // Find the dir_id for this directory
            let dir_rel = tupfile_dir.strip_prefix(&tup_root).unwrap_or(Path::new(""));
            let dir_id = resolve_dir_id(&db, dir_rel)?;

            // Convert expanded rules to RuleToStore
            let rules_to_store: Vec<tup_db::RuleToStore> = expanded.iter().map(|r| {
                tup_db::RuleToStore {
                    command: r.command.command.clone(),
                    inputs: r.inputs.clone(),
                    order_only_inputs: r.order_only_inputs.clone(),
                    outputs: r.outputs.clone(),
                    display: r.command.display.clone(),
                    flags: r.command.flags.clone(),
                }
            }).collect();

            let stored = tup_db::store_rules(&db, &mut cache, dir_id, &rules_to_store)?;
            total_stored += stored.len();
        }

        db.commit()?;

        if total_stored > 0 {
            eprintln!("[ tup ] Stored {} command(s) from {} Tupfile(s).",
                total_stored, tupfiles.len());
        }
    }

    // Phase 3: Execute only modified commands
    let modified_cmds = tup_db::get_modified_commands(&db)?;

    if modified_cmds.is_empty() {
        // Clear any remaining scan/parse flags
        db.begin()?;
        db.flags_clear_all()?;
        db.commit()?;
        println!("[ tup ] No commands to execute. Everything is up-to-date.");
        return Ok(());
    }

    let num_jobs = jobs.unwrap_or_else(|| {
        std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1)
    });

    eprintln!("[ tup ] {} command(s) to execute.", modified_cmds.len());

    // Look up each command and execute it
    let mut total_run = 0usize;
    let mut total_failed = 0usize;

    // Group commands by directory for execution
    let mut dir_commands: std::collections::BTreeMap<TupId, Vec<(TupId, String, Option<String>)>> =
        std::collections::BTreeMap::new();

    for cmd_id in &modified_cmds {
        if let Some(row) = db.node_select_by_id(*cmd_id)? {
            let dir_id = row.dir;
            let display = row.display.clone();
            let cmd_name = row.name.clone();
            dir_commands.entry(dir_id).or_default().push((*cmd_id, cmd_name, display));
        }
    }

    // Re-parse and expand Tupfiles to get actual command strings for execution
    let mut all_rules: Vec<(PathBuf, tup_parser::Rule)> = Vec::new();
    for tupfile_rel in &tupfiles {
        let tupfile_path = tup_root.join(tupfile_rel);
        let tupfile_dir = tupfile_path.parent().unwrap_or(&tup_root);
        let filename = tupfile_rel.to_string_lossy();
        let rules = parse_tupfile_any(&tupfile_path, tupfile_dir, &tup_root, &filename)?;
        let expanded = expand_rules_for_dir(&rules, tupfile_dir)?;
        for rule in expanded {
            all_rules.push((tupfile_dir.to_path_buf(), rule));
        }
    }

    // Execute all rules
    all_rules.sort_by(|a, b| a.0.cmp(&b.0));

    let mut current_dir: Option<PathBuf> = None;
    let mut dir_rules: Vec<tup_parser::Rule> = Vec::new();

    for (dir, rule) in all_rules {
        if current_dir.as_ref() != Some(&dir) {
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

    if !dir_rules.is_empty() {
        if let Some(ref work_dir) = current_dir {
            let (run, failed) = execute_dir_rules(
                work_dir, &dir_rules, keep_going, num_jobs,
            )?;
            total_run += run;
            total_failed += failed;
        }
    }

    // Post-execution: update output mtimes and clear modify flags
    db.begin()?;
    if total_failed == 0 {
        // Track output mtimes for each stored command
        for tupfile_rel in &tupfiles {
            let tupfile_path = tup_root.join(tupfile_rel);
            let tupfile_dir = tupfile_path.parent().unwrap_or(&tup_root);
            let dir_rel = tupfile_dir.strip_prefix(&tup_root).unwrap_or(Path::new(""));

            if let Ok(dir_id) = resolve_dir_id(&db, dir_rel) {
                let filename = tupfile_rel.to_string_lossy();
                if let Ok(rules) = parse_tupfile_any(&tupfile_path, tupfile_dir, &tup_root, &filename) {
                    let expanded = expand_rules_for_dir(&rules, tupfile_dir).unwrap_or_default();
                    for rule in &expanded {
                        // Find the CMD node for this rule
                        if let Ok(Some(cmd_row)) = db.node_select(dir_id, &rule.command.command) {
                            if cmd_row.node_type == tup_types::NodeType::Cmd {
                                let track_result = tup_db::track_outputs(
                                    &db, cmd_row.id, dir_id,
                                    &rule.outputs, tupfile_dir,
                                )?;
                                for missing in &track_result.missing {
                                    eprintln!("tup warning: expected output '{}' was not created", missing);
                                }
                            }
                        }
                    }
                }
            }
        }

        // Clear modify flags
        for cmd_id in &modified_cmds {
            tup_db::mark_command_done(&db, *cmd_id)?;
        }

        // Clear all remaining flag lists (scan/parse flags)
        db.flags_clear_all()?;
    }
    db.commit()?;

    // Summary
    if total_failed > 0 {
        eprintln!("[ tup ] {total_failed} command(s) failed out of {total_run}.");
        process::exit(1);
    } else {
        println!("[ tup ] Updated. {total_run} command(s) ran successfully.");
    }

    Ok(())
}

/// Resolve a relative directory path to its TupId in the database.
fn resolve_dir_id(db: &tup_db::TupDb, rel_path: &Path) -> anyhow::Result<TupId> {
    if rel_path.as_os_str().is_empty() || rel_path == Path::new(".") {
        return Ok(DOT_DT);
    }

    let mut current = DOT_DT;
    for component in rel_path.components() {
        let name = component.as_os_str().to_string_lossy();
        match db.node_select(current, &name)? {
            Some(row) => current = row.id,
            None => return Err(anyhow::anyhow!("directory not found in DB: {}", rel_path.display())),
        }
    }

    Ok(current)
}

/// Resolve a directory path, creating missing DIR nodes along the way.
fn ensure_dir_nodes(db: &tup_db::TupDb, dir_path: &Path) -> anyhow::Result<TupId> {
    if dir_path.as_os_str().is_empty() || dir_path == Path::new(".") {
        return Ok(DOT_DT);
    }

    let mut current = DOT_DT;
    for component in dir_path.components() {
        let name = component.as_os_str().to_string_lossy();
        match db.node_select(current, &name)? {
            Some(row) => current = row.id,
            None => {
                let id = db.node_insert(
                    current, &name, NodeType::Dir,
                    -1, 0, -1, None, None,
                )?;
                current = id;
            }
        }
    }

    Ok(current)
}

/// Normalize a path by resolving `.` and `..` components.
fn normalize_path(path: &Path) -> PathBuf {
    let mut components = Vec::new();
    for component in path.components() {
        match component {
            std::path::Component::ParentDir => {
                // Only pop if we have a normal component to go back from
                if components.last().is_some_and(|c| matches!(c, std::path::Component::Normal(_))) {
                    components.pop();
                } else {
                    components.push(component);
                }
            }
            std::path::Component::CurDir => {}
            _ => components.push(component),
        }
    }
    components.iter().collect()
}

/// Resolve a file path (from CLI arg) to a path relative to the tup root.
fn resolve_to_tup_relative(cwd: &Path, tup_root: &Path, path: &str) -> anyhow::Result<PathBuf> {
    let full_path = if Path::new(path).is_absolute() {
        PathBuf::from(path)
    } else {
        cwd.join(path)
    };

    let normalized = normalize_path(&full_path);

    // Try direct prefix stripping first
    if let Ok(rel) = normalized.strip_prefix(tup_root) {
        return Ok(rel.to_path_buf());
    }

    // On macOS, /tmp is a symlink to /private/tmp. Try canonicalizing
    // both paths to resolve symlinks before comparing.
    if let (Ok(canon_path), Ok(canon_root)) = (
        try_canonicalize(&normalized),
        try_canonicalize(tup_root),
    ) {
        if let Ok(rel) = canon_path.strip_prefix(&canon_root) {
            return Ok(rel.to_path_buf());
        }
    }

    Err(anyhow::anyhow!("path '{}' is outside tup root '{}'", path, tup_root.display()))
}

/// Try to canonicalize a path, falling back to the original if it doesn't exist.
fn try_canonicalize(path: &Path) -> std::io::Result<PathBuf> {
    // Try the full path first
    if let Ok(p) = path.canonicalize() {
        return Ok(p);
    }
    // If the file doesn't exist, canonicalize the parent and append the filename
    if let (Some(parent), Some(name)) = (path.parent(), path.file_name()) {
        if let Ok(canon_parent) = parent.canonicalize() {
            return Ok(canon_parent.join(name));
        }
    }
    Err(std::io::Error::new(std::io::ErrorKind::NotFound, "cannot canonicalize"))
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

    // Phase 1: Scan filesystem and sync to database
    let db = tup_db::TupDb::open(&tup_root, false)?;
    let mut cache = tup_db::EntryCache::new();
    let sync_result = tup_db::sync_filesystem(&db, &mut cache, &tup_root)?;

    if sync_result.files_added > 0 || sync_result.files_modified > 0 || sync_result.files_deleted > 0 {
        eprintln!(
            "[ tup ] Scan: {} new, {} modified, {} deleted",
            sync_result.files_added, sync_result.files_modified, sync_result.files_deleted,
        );
    }

    // Phase 2: Parse Tupfiles and store commands in database
    let tupfiles = tup_platform::scanner::find_tupfiles(&tup_root)
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    if tupfiles.is_empty() {
        println!("No Tupfiles found.");
        return Ok(());
    }

    db.begin()?;

    let mut total_stored = 0usize;
    for tupfile_rel in &tupfiles {
        let tupfile_path = tup_root.join(tupfile_rel);
        let tupfile_dir = tupfile_path.parent().unwrap_or(&tup_root);
        let filename = tupfile_rel.to_string_lossy();

        let rules = parse_tupfile_any(&tupfile_path, tupfile_dir, &tup_root, &filename)?;
        let expanded = expand_rules_for_dir(&rules, tupfile_dir)?;

        // Find the dir_id for this directory
        let dir_rel = tupfile_dir.strip_prefix(&tup_root).unwrap_or(Path::new(""));
        let dir_id = resolve_dir_id(&db, dir_rel)
            .or_else(|_| ensure_dir_nodes(&db, dir_rel))?;

        let rules_to_store: Vec<tup_db::RuleToStore> = expanded.iter().map(|r| {
            tup_db::RuleToStore {
                command: r.command.command.clone(),
                inputs: r.inputs.clone(),
                order_only_inputs: r.order_only_inputs.clone(),
                outputs: r.outputs.clone(),
                display: r.command.display.clone(),
                flags: r.command.flags.clone(),
            }
        }).collect();

        let stored = tup_db::store_rules(&db, &mut cache, dir_id, &rules_to_store)?;
        total_stored += stored.len();
    }

    // Clear all remaining flag lists
    db.flags_clear_all()?;
    db.commit()?;

    if total_stored > 0 {
        eprintln!("[ tup ] Stored {} command(s) from {} Tupfile(s).",
            total_stored, tupfiles.len());
    }

    Ok(())
}

fn cmd_scan() -> anyhow::Result<()> {
    let cwd = std::env::current_dir()?;
    let tup_root = tup_platform::init::find_tup_dir(&cwd)
        .ok_or_else(|| anyhow::anyhow!("No .tup directory found."))?;

    // Actually sync to database (not just report stats)
    let db = tup_db::TupDb::open(&tup_root, false)?;
    let mut cache = tup_db::EntryCache::new();
    let sync_result = tup_db::sync_filesystem(&db, &mut cache, &tup_root)?;

    eprintln!(
        "[ tup ] Scan: {} new, {} modified, {} deleted",
        sync_result.files_added, sync_result.files_modified, sync_result.files_deleted,
    );

    Ok(())
}

fn cmd_graph() -> anyhow::Result<()> {
    let cwd = std::env::current_dir()?;
    let tup_root = tup_platform::init::find_tup_dir(&cwd)
        .ok_or_else(|| anyhow::anyhow!("No .tup directory found."))?;

    let tupfiles = tup_platform::scanner::find_tupfiles(&tup_root)
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    let mut all_rules = Vec::new();
    for tupfile_rel in &tupfiles {
        let tupfile_path = tup_root.join(tupfile_rel);
        let tupfile_dir = tupfile_path.parent().unwrap_or(&tup_root);
        let dir_rel = tupfile_dir.strip_prefix(&tup_root)
            .unwrap_or(Path::new(""))
            .to_string_lossy()
            .to_string();

        let filename = tupfile_rel.to_string_lossy();
        let rules = parse_tupfile_any(&tupfile_path, tupfile_dir, &tup_root, &filename)?;

        for rule in rules {
            all_rules.push((
                dir_rel.clone(),
                rule.inputs.clone(),
                rule.command.command.clone(),
                rule.outputs.clone(),
            ));
        }
    }

    let dot = tup_graph::rules_to_dot(&all_rules);
    print!("{dot}");
    Ok(())
}

/// Expand foreach rules into individual rules with substituted commands.
///
/// For foreach rules, glob-matches inputs against the filesystem, then
/// creates one rule per matched input with all % substitutions applied.
/// Non-foreach rules get their globs expanded and % substitutions applied.
///
/// Input globs match against both filesystem files AND declared outputs
/// from prior rules (since generated outputs don't exist on disk yet
/// during the parse phase).
fn expand_rules_for_dir(
    rules: &[tup_parser::Rule],
    work_dir: &Path,
) -> anyhow::Result<Vec<tup_parser::Rule>> {
    let mut expanded = Vec::new();
    // Track declared outputs from prior rules for glob resolution
    let mut declared_outputs: Vec<String> = Vec::new();
    let dir_name = work_dir.file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| ".".to_string());

    for rule in rules {
        // Skip rules where inputs came from variables that expanded to empty.
        // Matches C tup: `: $(empty_var) |> cmd |>` produces no command,
        // but `: |> cmd |>` (explicitly empty) does.
        if rule.had_inputs && rule.inputs.is_empty() {
            continue;
        }

        // Expand input globs against filesystem + declared outputs
        let matched_inputs = expand_globs_with_declared(
            &rule.inputs, work_dir, &declared_outputs,
        )?;

        // Check for missing explicit inputs (non-glob, non-declared)
        // Matches C tup parser.c:2746-2757: "Explicitly named file not found"
        for input in &rule.inputs {
            if !tup_parser::is_glob(input) && !input.is_empty() {
                let on_disk = work_dir.join(input).exists();
                let in_declared = declared_outputs.contains(input);
                if !on_disk && !in_declared {
                    return Err(anyhow::anyhow!(
                        "tup error: Explicitly named file '{}' not found",
                        input
                    ));
                }
            }
        }

        // Check order-only inputs too
        for oo_input in &rule.order_only_inputs {
            if !tup_parser::is_glob(oo_input) && !oo_input.is_empty() {
                let on_disk = work_dir.join(oo_input).exists();
                let in_declared = declared_outputs.contains(oo_input);
                if !on_disk && !in_declared {
                    return Err(anyhow::anyhow!(
                        "tup error: Explicitly named file '{}' not found",
                        oo_input
                    ));
                }
            }
        }

        // Check for %f/%b/%B usage with no inputs, and %i with no order-only inputs
        // Matches C tup parser.c error messages
        if matched_inputs.is_empty() && !rule.had_inputs {
            let cmd = &rule.command.command;
            for (pat, desc) in [("%f", "%f"), ("%b", "%b"), ("%B", "%B")] {
                if cmd.contains(pat) {
                    return Err(anyhow::anyhow!(
                        "tup error: {} used in rule pattern and no input files were specified",
                        desc
                    ));
                }
            }
        }
        if rule.order_only_inputs.is_empty() && rule.command.command.contains("%i") {
            return Err(anyhow::anyhow!(
                "tup error: %i used in rule pattern and no order-only input files were specified"
            ));
        }

        if rule.foreach {
            // Expand foreach: one rule per input file
            for input_path in &matched_inputs {
                let input = tup_parser::InputFile::new(input_path);

                // Expand output patterns for this input
                let outputs: Vec<String> = rule.outputs.iter()
                    .map(|pat| tup_parser::expand_output_pattern(pat, &input))
                    .collect();

                // Expand % in command
                let cmd = tup_parser::expand_percent(
                    &rule.command.command,
                    std::slice::from_ref(&input),
                    &outputs,
                    &rule.order_only_inputs,
                    &dir_name,
                );

                // Expand % in display string if present
                let display = rule.command.display.as_ref().map(|d| {
                    tup_parser::expand_percent(
                        d, std::slice::from_ref(&input), &outputs, &rule.order_only_inputs, &dir_name,
                    )
                });

                // Track these outputs for later rules
                declared_outputs.extend(outputs.clone());

                expanded.push(tup_parser::Rule {
                    foreach: false,
                    inputs: vec![input_path.clone()],
                    order_only_inputs: rule.order_only_inputs.clone(),
                    command: tup_parser::RuleCommand {
                        command: cmd,
                        display,
                        flags: rule.command.flags.clone(),
                    },
                    outputs,
                    extra_outputs: rule.extra_outputs.clone(),
                    line_number: rule.line_number,
                    had_inputs: true,
                });
            }
        } else {
            // Non-foreach: expand globs and % substitutions
            let inputs: Vec<tup_parser::InputFile> = matched_inputs.iter()
                .map(|p| tup_parser::InputFile::new(p))
                .collect();

            // Expand output patterns with % substitutions
            let outputs: Vec<String> = if let Some(first_input) = inputs.first() {
                rule.outputs.iter()
                    .map(|pat| {
                        if pat.contains('%') {
                            tup_parser::expand_output_pattern(pat, first_input)
                        } else {
                            pat.clone()
                        }
                    })
                    .collect()
            } else {
                rule.outputs.clone()
            };

            let cmd = tup_parser::expand_percent(
                &rule.command.command,
                &inputs,
                &outputs,
                &rule.order_only_inputs,
                &dir_name,
            );

            let display = rule.command.display.as_ref().map(|d| {
                tup_parser::expand_percent(
                    d, &inputs, &outputs, &rule.order_only_inputs, &dir_name,
                )
            });

            // Track these outputs for later rules
            declared_outputs.extend(outputs.clone());

            expanded.push(tup_parser::Rule {
                foreach: false,
                inputs: matched_inputs,
                order_only_inputs: rule.order_only_inputs.clone(),
                command: tup_parser::RuleCommand {
                    command: cmd,
                    display,
                    flags: rule.command.flags.clone(),
                },
                outputs,
                extra_outputs: rule.extra_outputs.clone(),
                line_number: rule.line_number,
                had_inputs: rule.had_inputs,
            });
        }
    }

    // Check for duplicate outputs across all expanded rules
    // Matches C tup parser.c:3187-3191: "Unable to create output file because
    // it is already owned by command"
    let mut seen_outputs: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    for rule in &expanded {
        for output in &rule.outputs {
            if let Some(prev_cmd) = seen_outputs.get(output) {
                return Err(anyhow::anyhow!(
                    "tup error: Unable to create output file '{}' in '{}' because it is already owned by '{}'",
                    output, rule.command.command, prev_cmd,
                ));
            }
            seen_outputs.insert(output.clone(), rule.command.command.clone());
        }
    }

    Ok(expanded)
}

/// Expand input globs against both the filesystem and declared outputs.
fn expand_globs_with_declared(
    patterns: &[String],
    base_dir: &Path,
    declared_outputs: &[String],
) -> anyhow::Result<Vec<String>> {
    let mut result = Vec::new();

    for pattern in patterns {
        if tup_parser::is_glob(pattern) {
            // First, match against filesystem
            let mut matches = tup_parser::expand_globs(std::slice::from_ref(pattern), base_dir)
                .map_err(|e| anyhow::anyhow!("{e}"))?;

            // Also match against declared outputs from prior rules
            for output in declared_outputs {
                if glob_matches_name(pattern, output) && !matches.contains(output) {
                    matches.push(output.clone());
                }
            }
            matches.sort();
            result.extend(matches);
        } else {
            result.push(pattern.clone());
        }
    }

    Ok(result)
}

/// Check if a glob pattern matches a filename.
fn glob_matches_name(pattern: &str, name: &str) -> bool {
    // Split pattern into dir part and file part
    let (dir_part, file_pattern) = if let Some(pos) = pattern.rfind('/') {
        (&pattern[..pos], &pattern[pos + 1..])
    } else {
        ("", pattern)
    };

    // Split name into dir part and file part
    let (name_dir, name_file) = if let Some(pos) = name.rfind('/') {
        (&name[..pos], &name[pos + 1..])
    } else {
        ("", name)
    };

    // Directory parts must match
    if dir_part != name_dir {
        return false;
    }

    // Match the file pattern
    simple_glob_match(file_pattern, name_file)
}

/// Simple glob matching for * and ? patterns.
fn simple_glob_match(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    simple_glob_recursive(&p, 0, &t, 0)
}

fn simple_glob_recursive(pattern: &[char], pi: usize, text: &[char], ti: usize) -> bool {
    let mut pi = pi;
    let mut ti = ti;

    while pi < pattern.len() {
        match pattern[pi] {
            '*' => {
                pi += 1;
                for skip in 0..=(text.len() - ti) {
                    if simple_glob_recursive(pattern, pi, text, ti + skip) {
                        return true;
                    }
                }
                return false;
            }
            '?' => {
                if ti >= text.len() { return false; }
                pi += 1;
                ti += 1;
            }
            c => {
                if ti >= text.len() || text[ti] != c { return false; }
                pi += 1;
                ti += 1;
            }
        }
    }

    ti == text.len()
}

/// Parse a Tupfile (either standard or Lua) and return rules.
fn parse_tupfile_any(
    tupfile_path: &Path,
    tupfile_dir: &Path,
    tup_root: &Path,
    filename: &str,
) -> anyhow::Result<Vec<tup_parser::Rule>> {
    let content = std::fs::read_to_string(tupfile_path)?;

    if filename.ends_with(".lua") {
        tup_parser::parse_lua_tupfile(&content, filename, tupfile_dir)
            .map_err(|e| anyhow::anyhow!("{e}"))
    } else {
        let mut reader = tup_parser::TupfileReader::parse(&content, filename)?;
        Ok(reader.evaluate_with_dirs(Some(tupfile_dir), Some(tup_root), None)?)
    }
}

fn cmd_version() {
    println!("tup-rust v{}", env!("CARGO_PKG_VERSION"));
}

fn cmd_options(jobs: Option<usize>) {
    let mut opts = tup_platform::options::TupOptions::new();

    // Load ~/.tupoptions (user-level defaults)
    if let Some(home) = std::env::var_os("HOME") {
        let user_opts = PathBuf::from(home).join(".tupoptions");
        let _ = opts.load_file(&user_opts);
    }

    // Load .tup/options (project-level)
    if let Ok(cwd) = std::env::current_dir() {
        if let Some(tup_root) = tup_platform::init::find_tup_dir(&cwd) {
            let project_opts = tup_root.join(".tup").join("options");
            let _ = opts.load_file(&project_opts);
        }
    }

    // Apply -j override
    if let Some(j) = jobs {
        opts.set("updater.num_jobs", &j.to_string());
    }

    for (name, value) in opts.show() {
        println!("{name} = {value}");
    }
}

fn cmd_server() {
    // Report server mode. On macOS there's no LD_PRELOAD or FUSE support,
    // so we report "none". On Linux, we'll report the configured mode.
    #[cfg(target_os = "linux")]
    println!("ldpreload");
    #[cfg(not(target_os = "linux"))]
    println!("none");
}

// -- Testing/debugging commands --

fn cmd_touch(files: Vec<String>) -> anyhow::Result<()> {
    let cwd = std::env::current_dir()?;
    let tup_root = tup_platform::init::find_tup_dir(&cwd)
        .ok_or_else(|| anyhow::anyhow!("No .tup directory found. Run 'tup init' first."))?;

    let db = tup_db::TupDb::open(&tup_root, false)?;
    db.begin()?;

    for file in &files {
        let rel_path = resolve_to_tup_relative(&cwd, &tup_root, file)?;
        touch_path(&db, &tup_root, &rel_path)?;
    }

    db.commit()?;
    Ok(())
}

/// Create or update a node in the database for the given path.
fn touch_path(db: &tup_db::TupDb, tup_root: &Path, rel_path: &Path) -> anyhow::Result<()> {
    let parent = rel_path.parent().unwrap_or(Path::new(""));
    let name = rel_path.file_name()
        .ok_or_else(|| anyhow::anyhow!("no file name in path: {}", rel_path.display()))?
        .to_string_lossy();

    // Ensure parent directory nodes exist
    let dir_id = ensure_dir_nodes(db, parent)?;

    // Determine node type based on what's on disk
    let full_path = tup_root.join(rel_path);
    let node_type = if full_path.is_dir() {
        NodeType::Dir
    } else {
        NodeType::File
    };

    // Create or update the node
    match db.node_select(dir_id, &name)? {
        Some(existing) => {
            // Flag existing node as modified
            db.flag_add(existing.id, tup_types::TupFlags::Modify)?;
        }
        None => {
            let id = db.node_insert(dir_id, &name, node_type, -1, 0, -1, None, None)?;
            db.flag_add(id, tup_types::TupFlags::Modify)?;
            if node_type == NodeType::Dir {
                db.flag_add(id, tup_types::TupFlags::Create)?;
            }
        }
    }

    Ok(())
}

fn cmd_node_exists(dir: &str, name: &str) -> anyhow::Result<()> {
    let cwd = std::env::current_dir()?;
    let tup_root = tup_platform::init::find_tup_dir(&cwd)
        .ok_or_else(|| anyhow::anyhow!("No .tup directory found."))?;

    let db = tup_db::TupDb::open(&tup_root, false)?;

    // Resolve directory path to dir_id
    let dir_id = match resolve_dir_id(&db, Path::new(dir)) {
        Ok(id) => id,
        Err(_) => {
            // Directory doesn't exist in DB — node can't exist
            process::exit(1);
        }
    };

    // Check if the node exists
    match db.node_select(dir_id, name)? {
        Some(_) => {
            // Node exists — exit 0 (success)
            Ok(())
        }
        None => {
            // Node doesn't exist — exit non-zero
            process::exit(1);
        }
    }
}

fn cmd_flags_exists() -> anyhow::Result<()> {
    let cwd = std::env::current_dir()?;
    let tup_root = tup_platform::init::find_tup_dir(&cwd)
        .ok_or_else(|| anyhow::anyhow!("No .tup directory found."))?;

    let db = tup_db::TupDb::open(&tup_root, false)?;

    if db.flags_empty()? {
        // No flags — clean state — exit 0
        Ok(())
    } else {
        // Flags exist — dirty state — exit non-zero
        process::exit(1);
    }
}

fn cmd_link_exists(
    dir1: &str,
    name1: &str,
    dir2: &str,
    name2: &str,
    link_type: tup_types::LinkType,
) -> anyhow::Result<()> {
    let cwd = std::env::current_dir()?;
    let tup_root = tup_platform::init::find_tup_dir(&cwd)
        .ok_or_else(|| anyhow::anyhow!("No .tup directory found."))?;

    let db = tup_db::TupDb::open(&tup_root, false)?;

    let dir1_id = resolve_dir_id(&db, Path::new(dir1))?;
    let dir2_id = resolve_dir_id(&db, Path::new(dir2))?;

    let node1 = db.node_select(dir1_id, name1)?
        .ok_or_else(|| anyhow::anyhow!("node '{name1}' not found in dir '{dir1}'"))?;
    let node2 = db.node_select(dir2_id, name2)?
        .ok_or_else(|| anyhow::anyhow!("node '{name2}' not found in dir '{dir2}'"))?;

    if db.link_exists(node1.id, node2.id, link_type)? {
        // Link exists — exit code 11 (C tup convention)
        process::exit(11);
    } else {
        // Link doesn't exist — exit code 0
        Ok(())
    }
}
