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

    /// Keep going after errors (shorthand for `tup upd -k`)
    #[arg(short, long, global = true)]
    keep_going: bool,

    /// Number of parallel jobs (shorthand for `tup upd -j N`)
    #[arg(short, long, global = true)]
    jobs: Option<usize>,

    /// Skip filesystem scan (shorthand for `tup upd --no-scan`)
    #[arg(long, global = true)]
    no_scan: bool,
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
    /// Display database configuration
    #[command(name = "dbconfig")]
    DbConfig,
    /// Check if any nodes have pending flags
    #[command(name = "flags_exists")]
    FlagsExists,
    /// Check if any nodes have create flags set
    #[command(name = "create_flags_exists")]
    CreateFlagsExists,
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
    /// Replace @VAR@ patterns with values from the tup vardict
    Varsed {
        /// Use binary mode (y→1, n→0 for single-char values)
        #[arg(long)]
        binary: bool,
        /// Input file (use - for stdin)
        input: String,
        /// Output file (use - for stdout)
        output: String,
    },
    /// Read tup.config and store config variables in the database
    Read,
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
        Some(Commands::Init {
            directory,
            no_sync,
            force,
        }) => cmd_init(directory, no_sync, force),
        Some(Commands::Upd {
            keep_going,
            jobs,
            no_scan,
        }) => cmd_upd(
            keep_going || cli.keep_going,
            jobs.or(cli.jobs),
            no_scan || cli.no_scan,
        ),
        None => cmd_upd(cli.keep_going, cli.jobs, cli.no_scan),
        Some(Commands::Parse) => cmd_parse(),
        Some(Commands::Version) => {
            cmd_version();
            Ok(())
        }
        Some(Commands::Options { jobs }) => {
            cmd_options(jobs);
            Ok(())
        }
        Some(Commands::Graph {
            directory: _,
            dirs: _,
        }) => cmd_graph(),
        Some(Commands::Scan) => cmd_scan(),
        Some(Commands::Touch { files }) => cmd_touch(files),
        Some(Commands::NodeExists { dir, name }) => cmd_node_exists(&dir, &name),
        Some(Commands::DbConfig) => cmd_dbconfig(),
        Some(Commands::FlagsExists) => cmd_flags_exists(),
        Some(Commands::CreateFlagsExists) => cmd_create_flags_exists(),
        Some(Commands::NormalExists {
            dir1,
            name1,
            dir2,
            name2,
        }) => cmd_link_exists(&dir1, &name1, &dir2, &name2, tup_types::LinkType::Normal),
        Some(Commands::StickyExists {
            dir1,
            name1,
            dir2,
            name2,
        }) => cmd_link_exists(&dir1, &name1, &dir2, &name2, tup_types::LinkType::Sticky),
        Some(Commands::Server) => {
            cmd_server();
            Ok(())
        }
        Some(Commands::Varsed {
            binary,
            input,
            output,
        }) => cmd_varsed_cli(&input, &output, binary),
        Some(Commands::Read) => cmd_read(),
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
            println!(
                ".tup repository initialized: {}",
                dir.join(".tup/db").display()
            );
            Ok(())
        }
        Err(tup_platform::init::InitError::AlreadyInitialized(path)) => {
            eprintln!(
                "tup warning: database already exists in directory: {}",
                path.display()
            );
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

        if sync_result.files_added > 0
            || sync_result.files_modified > 0
            || sync_result.files_deleted > 0
        {
            println!(
                "[ tup ] Scan: {} new, {} modified, {} deleted",
                sync_result.files_added, sync_result.files_modified, sync_result.files_deleted,
            );
        }
    }

    // Load config variables from tup.config and detect changes
    let config = load_config_vars(&tup_root);
    write_vardict(&tup_root, &config);

    // Detect tup.config changes by comparing content hash
    let config_content = std::fs::read_to_string(tup_root.join("tup.config")).unwrap_or_default();
    let config_hash = simple_hash(&config_content);
    let old_config_hash = db.config_get_string("config_hash", "")?;
    let config_changed = config_hash != old_config_hash;
    if config_changed && !old_config_hash.is_empty() {
        println!("[ tup ] Configuration changed, re-parsing all Tupfiles.");
    }
    db.config_set_string("config_hash", &config_hash)?;

    // Phase 2: Parse Tupfiles and store commands in database
    let tupfiles =
        tup_platform::scanner::find_tupfiles(&tup_root).map_err(|e| anyhow::anyhow!("{e}"))?;

    // Track directories with .gitignore enabled and their outputs
    let mut gitignore_dirs: std::collections::HashMap<PathBuf, Vec<String>> =
        std::collections::HashMap::new();

    {
        db.begin()?;

        let mut total_stored = 0;

        if !tupfiles.is_empty() {
            // Parse, store rules, and delete stale outputs
            let parse_and_store =
                |tupfiles: &[PathBuf],
                 db: &tup_db::TupDb,
                 cache: &mut tup_db::EntryCache,
                 tup_root: &Path,
                 config: &std::collections::BTreeMap<String, String>,
                 gitignore_dirs: &mut std::collections::HashMap<PathBuf, Vec<String>>|
                 -> anyhow::Result<(usize, bool)> {
                    let mut total = 0;
                    let mut any_stale = false;
                    for tupfile_rel in tupfiles {
                        let tupfile_path = tup_root.join(tupfile_rel);
                        let tupfile_dir = tupfile_path.parent().unwrap_or(tup_root);
                        let filename = tupfile_rel.to_string_lossy();

                        let dir_rel = tupfile_dir.strip_prefix(tup_root).unwrap_or(Path::new(""));
                        let dir_id = resolve_dir_id(db, dir_rel)?;
                        let parser_db = tup_db::TupParserDb::new(db, dir_id);

                        let parse_result = parse_tupfile_any(
                            &tupfile_path,
                            tupfile_dir,
                            tup_root,
                            &filename,
                            config,
                            &parser_db,
                        )?;
                        let rules = parse_result.rules;
                        let expanded =
                            expand_rules_for_dir(&rules, tupfile_dir, &parse_result.vars)?;

                        if parse_result.gitignore {
                            let outputs: Vec<String> = expanded
                                .iter()
                                .flat_map(|r| r.outputs.iter().cloned())
                                .collect();
                            gitignore_dirs.insert(tupfile_dir.to_path_buf(), outputs);
                        }

                        let rules_to_store: Vec<tup_db::RuleToStore> = expanded
                            .iter()
                            .map(|r| tup_db::RuleToStore {
                                command: r.command.command.clone(),
                                inputs: r.inputs.clone(),
                                order_only_inputs: r.order_only_inputs.clone(),
                                outputs: r.outputs.clone(),
                                extra_outputs: r.extra_outputs.clone(),
                                display: r.command.display.clone(),
                                flags: r.command.flags.clone(),
                            })
                            .collect();

                        let store_result = tup_db::store_rules(db, cache, dir_id, &rules_to_store)?;
                        total += store_result.commands.len();
                        for stale in &store_result.stale_outputs {
                            let stale_path = tupfile_dir.join(stale);
                            let _ = std::fs::remove_file(&stale_path);
                            any_stale = true;
                        }
                    }
                    Ok((total, any_stale))
                };

            let (stored, stale) = parse_and_store(
                &tupfiles,
                &db,
                &mut cache,
                &tup_root,
                &config,
                &mut gitignore_dirs,
            )?;
            total_stored = stored;
            let had_stale_outputs = stale;

            // If stale outputs were deleted, re-parse to get correct glob results
            if had_stale_outputs {
                let (stored2, _) = parse_and_store(
                    &tupfiles,
                    &db,
                    &mut cache,
                    &tup_root,
                    &config,
                    &mut gitignore_dirs,
                )?;
                total_stored = stored2;
            }
        } // end if !tupfiles.is_empty()

        // Clean up directories that had Tupfiles deleted.
        // Port of C tup's process_create_nodes(): when a Tupfile is deleted,
        // all CMD nodes and their Generated outputs in that directory are removed.
        // We detect this by finding directories with CMD nodes that aren't in
        // the current set of parsed Tupfile directories.
        let parsed_dirs: std::collections::HashSet<PathBuf> = tupfiles
            .iter()
            .filter_map(|t| tup_root.join(t).parent().map(|p| p.to_path_buf()))
            .collect();
        // Collect all directories that might have CMD nodes: root + subdirs
        let mut dirs_to_check = vec![(DOT_DT, tup_root.clone())];
        let all_dir_nodes = db.node_select_dir(DOT_DT)?;
        for dir_node in &all_dir_nodes {
            if dir_node.node_type == NodeType::Dir {
                if let Ok(dir_path) = resolve_dir_path(&db, dir_node.id, &tup_root) {
                    dirs_to_check.push((dir_node.id, dir_path));
                }
            }
        }
        for (dir_id, dir_path) in &dirs_to_check {
            if !parsed_dirs.contains(dir_path) {
                // This directory wasn't parsed — check if it has stale CMDs
                let nodes = db.node_select_dir(*dir_id)?;
                let has_cmds = nodes.iter().any(|n| n.node_type == NodeType::Cmd);
                if has_cmds {
                    // Clean up: pass empty rules to store_rules, which will
                    // remove all existing CMDs and their outputs
                    let result = tup_db::store_rules(&db, &mut cache, *dir_id, &[])?;
                    for stale in &result.stale_outputs {
                        let stale_path = dir_path.join(stale);
                        let _ = std::fs::remove_file(&stale_path);
                    }
                }
            }
        }

        db.commit()?;

        if total_stored > 0 {
            println!(
                "[ tup ] Stored {} command(s) from {} Tupfile(s).",
                total_stored,
                tupfiles.len()
            );
        }
    }

    // Phase 3: Execute modified commands
    //
    // Port of C tup's process_update_nodes() from updater.c:1638-1692.
    // Reads commands from the modify_list in the DB and executes them.
    // Commands are already stored with their full command strings from Phase 2.
    //
    // Transitive propagation: if command A is in modify_list and produces
    // output X, and command B depends on X, then B must also be in the
    // modify_list. C tup handles this via the graph builder; we do it here
    // by following output→consumer chains until no new commands are added.
    {
        db.begin()?;
        let mut added = true;
        while added {
            added = false;
            let current_cmds = tup_db::get_modified_commands(&db)?;
            for cmd_id in &current_cmds {
                // Find outputs of this command (Generated nodes with matching srcid)
                if let Ok(Some(row)) = db.node_select_by_id(*cmd_id) {
                    let output_nodes = db.node_select_dir(row.dir)?;
                    for output in &output_nodes {
                        if output.node_type == tup_types::NodeType::Generated
                            && output.srcid == cmd_id.raw()
                        {
                            // Flag commands that depend on this output
                            let before = tup_db::get_modified_commands(&db)?.len();
                            db.modify_cmds_by_input(output.id)?;
                            let after = tup_db::get_modified_commands(&db)?.len();
                            if after > before {
                                added = true;
                            }
                        }
                    }
                }
            }
        }
        db.commit()?;
    }
    let modified_cmds = tup_db::get_modified_commands(&db)?;

    if modified_cmds.is_empty() {
        // Clear any remaining scan/parse flags
        db.begin()?;
        db.flags_clear_all()?;
        db.commit()?;
        // Still need to handle gitignore even with no commands
        if !gitignore_dirs.is_empty() {
            generate_gitignore_files(&gitignore_dirs, &tup_root);
        }
        remove_stale_gitignore_files(&tup_root, &gitignore_dirs);
        println!("[ tup ] No commands to execute. Everything is up-to-date.");
        return Ok(());
    }

    let num_jobs = jobs.unwrap_or_else(|| {
        // Check .tup/options for updater.num_jobs (set by single_threaded in tests)
        let options_path = tup_root.join(".tup/options");
        if let Ok(content) = std::fs::read_to_string(&options_path) {
            for line in content.lines() {
                let trimmed = line.trim();
                if let Some(val) = trimmed.strip_prefix("num_jobs=") {
                    if let Ok(n) = val.trim().parse::<usize>() {
                        if n > 0 {
                            return n;
                        }
                    }
                }
            }
        }
        std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1)
    });

    println!("[ tup ] {} command(s) to execute.", modified_cmds.len());

    // Build list of commands to execute from the DB modify_list.
    // Each command is already stored with its full command string.
    // Group by directory for proper working directory during execution.
    struct CmdToRun {
        cmd_id: TupId,
        dir_id: TupId,
        command: String,
        display: Option<String>,
        inputs: Vec<String>,
        outputs: Vec<String>,
    }

    let mut commands_to_run: Vec<CmdToRun> = Vec::new();
    for cmd_id in &modified_cmds {
        if let Some(row) = db.node_select_by_id(*cmd_id)? {
            if row.node_type != tup_types::NodeType::Cmd {
                continue;
            }
            // Get outputs for this command from the DB (Generated nodes with matching srcid)
            let output_nodes = db.node_select_dir(row.dir)?;
            let outputs: Vec<String> = output_nodes
                .iter()
                .filter(|n| {
                    n.node_type == tup_types::NodeType::Generated && n.srcid == cmd_id.raw()
                })
                .map(|n| n.name.clone())
                .collect();

            // Get inputs for this command from normal_link + sticky_link
            // This is needed for dependency ordering during execution
            let mut inputs = Vec::new();
            if let Ok(input_ids) = db.get_input_ids(*cmd_id) {
                for input_id in input_ids {
                    if let Ok(Some(input_row)) = db.node_select_by_id(input_id) {
                        if input_row.node_type == tup_types::NodeType::Generated
                            || input_row.node_type == tup_types::NodeType::File
                        {
                            inputs.push(input_row.name.clone());
                        }
                    }
                }
            }

            commands_to_run.push(CmdToRun {
                cmd_id: *cmd_id,
                dir_id: row.dir,
                command: row.name.clone(),
                display: row.display.clone(),
                inputs,
                outputs,
            });
        }
    }

    // Resolve directory paths for each command
    let mut dir_paths: std::collections::HashMap<TupId, PathBuf> = std::collections::HashMap::new();
    for cmd in &commands_to_run {
        if let std::collections::hash_map::Entry::Vacant(e) = dir_paths.entry(cmd.dir_id) {
            // Resolve dir_id to a filesystem path
            let path = resolve_dir_path(&db, cmd.dir_id, &tup_root)?;
            e.insert(path);
        }
    }

    // Group commands by directory, build Rules for the executor
    let mut dir_rule_groups: std::collections::BTreeMap<PathBuf, Vec<tup_parser::Rule>> =
        std::collections::BTreeMap::new();
    let mut cmd_id_map: Vec<(PathBuf, TupId)> = Vec::new();

    for cmd in &commands_to_run {
        let dir_path = dir_paths.get(&cmd.dir_id).unwrap().clone();
        let rule = tup_parser::Rule {
            foreach: false,
            inputs: cmd.inputs.clone(),
            order_only_inputs: vec![],
            command: tup_parser::RuleCommand {
                command: cmd.command.clone(),
                display: cmd.display.clone(),
                flags: None,
            },
            outputs: cmd.outputs.clone(),
            extra_outputs: vec![],
            line_number: 0,
            had_inputs: !cmd.inputs.is_empty(),
            vars_snapshot: None,
            bin: None,
        };
        dir_rule_groups
            .entry(dir_path.clone())
            .or_default()
            .push(rule);
        cmd_id_map.push((dir_path, cmd.cmd_id));
    }

    // Execute commands grouped by directory
    let mut total_run = 0usize;
    let mut total_failed = 0usize;

    for (dir_path, rules) in &dir_rule_groups {
        let (run, failed) = execute_dir_rules(dir_path, rules, keep_going, num_jobs)?;
        total_run += run;
        total_failed += failed;
    }

    // Post-execution: clear modify flags and track outputs
    // Port of C tup's update_work() post-success handling (updater.c:2170-2185)
    db.begin()?;
    if total_failed == 0 {
        for cmd in &commands_to_run {
            let dir_path = dir_paths.get(&cmd.dir_id).unwrap();
            // Track output mtimes
            if !cmd.outputs.is_empty() {
                let track_result =
                    tup_db::track_outputs(&db, cmd.cmd_id, cmd.dir_id, &cmd.outputs, dir_path)?;
                for missing in &track_result.missing {
                    eprintln!("tup warning: expected output '{}' was not created", missing);
                }
            }
            // Clear modify flag for this command (C: tup_db_unflag_modify)
            tup_db::mark_command_done(&db, cmd.cmd_id)?;
        }

        // Clear all remaining flag lists (scan/parse flags)
        db.flags_clear_all()?;
    }
    db.commit()?;

    // Generate .gitignore files for directories that requested it
    if !gitignore_dirs.is_empty() {
        generate_gitignore_files(&gitignore_dirs, &tup_root);
    }
    // Remove stale .gitignore files from directories that no longer request it
    remove_stale_gitignore_files(&tup_root, &gitignore_dirs);

    // Summary
    if total_failed > 0 {
        println!("[ tup ] {total_failed} command(s) failed out of {total_run}.");
        process::exit(1);
    } else {
        println!("[ tup ] Updated. {total_run} command(s) ran successfully.");
    }

    Ok(())
}

/// Generate .gitignore files for directories that requested them.
///
/// Each .gitignore contains the generated output files for that directory,
/// plus the .gitignore file itself. Matches C tup behavior.
fn generate_gitignore_files(
    gitignore_dirs: &std::collections::HashMap<PathBuf, Vec<String>>,
    tup_root: &Path,
) {
    for (dir, outputs) in gitignore_dirs {
        let gitignore_path = dir.join(".gitignore");
        let mut lines: Vec<String> = outputs.clone();
        // The .gitignore file itself should be ignored
        lines.push(".gitignore".to_string());
        // In the root directory, also ignore .tup
        if dir == tup_root {
            lines.push(".tup".to_string());
            // If .git exists, also add /.gitignore (root-anchored)
            if dir.join(".git").exists() {
                lines.push("/.gitignore".to_string());
            }
        }
        lines.sort();
        lines.dedup();
        let content = lines.join("\n") + "\n";
        if let Err(e) = std::fs::write(&gitignore_path, content) {
            eprintln!(
                "tup warning: failed to write {}: {e}",
                gitignore_path.display()
            );
        }
    }
}

/// Remove .gitignore files from directories that no longer request them.
/// Walks the project tree looking for .gitignore files to remove.
fn remove_stale_gitignore_files(
    tup_root: &Path,
    active_dirs: &std::collections::HashMap<PathBuf, Vec<String>>,
) {
    fn walk_and_clean(dir: &Path, active: &std::collections::HashMap<PathBuf, Vec<String>>) {
        let gitignore = dir.join(".gitignore");
        if gitignore.exists() && !active.contains_key(dir) {
            let _ = std::fs::remove_file(&gitignore);
        }
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    let name = entry.file_name();
                    let name_str = name.to_string_lossy();
                    if !name_str.starts_with('.') {
                        walk_and_clean(&path, active);
                    }
                }
            }
        }
    }
    walk_and_clean(tup_root, active_dirs);
}

/// Resolve a relative directory path to its TupId in the database.
fn resolve_dir_id(db: &tup_db::TupDb, rel_path: &Path) -> anyhow::Result<TupId> {
    let path_str = rel_path.to_string_lossy();
    if path_str.is_empty() || path_str == "." {
        return Ok(DOT_DT);
    }
    // Handle numeric directory IDs (C tup convention: "0" = virtual root)
    if let Ok(id) = path_str.parse::<i64>() {
        return Ok(TupId::new(id));
    }

    let mut current = DOT_DT;
    for component in rel_path.components() {
        let name = component.as_os_str().to_string_lossy();
        match db.node_select(current, &name)? {
            Some(row) => current = row.id,
            None => {
                return Err(anyhow::anyhow!(
                    "directory not found in DB: {}",
                    rel_path.display()
                ))
            }
        }
    }

    Ok(current)
}

/// Resolve a TupId back to a filesystem path relative to tup root.
fn resolve_dir_path(db: &tup_db::TupDb, dir_id: TupId, tup_root: &Path) -> anyhow::Result<PathBuf> {
    if dir_id == DOT_DT {
        return Ok(tup_root.to_path_buf());
    }

    let mut parts = Vec::new();
    let mut current = dir_id;
    while current != DOT_DT {
        let row = db
            .node_select_by_id(current)?
            .ok_or_else(|| anyhow::anyhow!("node {} not found", current.raw()))?;
        parts.push(row.name.clone());
        current = row.dir;
    }
    parts.reverse();

    let mut path = tup_root.to_path_buf();
    for part in parts {
        path = path.join(part);
    }
    Ok(path)
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
                let id = db.node_insert(current, &name, NodeType::Dir, -1, 0, -1, None, None)?;
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
                if components
                    .last()
                    .is_some_and(|c| matches!(c, std::path::Component::Normal(_)))
                {
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
    if let (Ok(canon_path), Ok(canon_root)) =
        (try_canonicalize(&normalized), try_canonicalize(tup_root))
    {
        if let Ok(rel) = canon_path.strip_prefix(&canon_root) {
            return Ok(rel.to_path_buf());
        }
    }

    Err(anyhow::anyhow!(
        "path '{}' is outside tup root '{}'",
        path,
        tup_root.display()
    ))
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
    Err(std::io::Error::new(
        std::io::ErrorKind::NotFound,
        "cannot canonicalize",
    ))
}

fn execute_dir_rules(
    work_dir: &Path,
    rules: &[tup_parser::Rule],
    keep_going: bool,
    num_jobs: usize,
) -> anyhow::Result<(usize, usize)> {
    let mut updater = tup_updater::Updater::new(work_dir);
    updater.set_keep_going(keep_going);

    // Use pre-expanded execution since expand_rules_for_dir already
    // handled all % substitutions. Avoids double-expansion of %%.
    let results = if num_jobs > 1 {
        updater.execute_expanded_rules_parallel(rules, num_jobs)
    } else {
        updater.execute_expanded_rules(rules)
    }
    .map_err(|e| anyhow::anyhow!("{e}"))?;

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

    let config = load_config_vars(&tup_root);
    write_vardict(&tup_root, &config);

    // Phase 1: Scan filesystem and sync to database
    let db = tup_db::TupDb::open(&tup_root, false)?;
    let mut cache = tup_db::EntryCache::new();
    let sync_result = tup_db::sync_filesystem(&db, &mut cache, &tup_root)?;

    if sync_result.files_added > 0
        || sync_result.files_modified > 0
        || sync_result.files_deleted > 0
    {
        println!(
            "[ tup ] Scan: {} new, {} modified, {} deleted",
            sync_result.files_added, sync_result.files_modified, sync_result.files_deleted,
        );
    }

    // Phase 2: Parse Tupfiles and store commands in database
    let tupfiles =
        tup_platform::scanner::find_tupfiles(&tup_root).map_err(|e| anyhow::anyhow!("{e}"))?;

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

        // Find the dir_id for this directory
        let dir_rel = tupfile_dir.strip_prefix(&tup_root).unwrap_or(Path::new(""));
        let dir_id = resolve_dir_id(&db, dir_rel).or_else(|_| ensure_dir_nodes(&db, dir_rel))?;
        let parser_db = tup_db::TupParserDb::new(&db, dir_id);

        let parse_result = parse_tupfile_any(
            &tupfile_path,
            tupfile_dir,
            &tup_root,
            &filename,
            &config,
            &parser_db,
        )?;
        let rules = parse_result.rules;
        let expanded = expand_rules_for_dir(&rules, tupfile_dir, &parse_result.vars)?;

        let rules_to_store: Vec<tup_db::RuleToStore> = expanded
            .iter()
            .map(|r| tup_db::RuleToStore {
                command: r.command.command.clone(),
                inputs: r.inputs.clone(),
                order_only_inputs: r.order_only_inputs.clone(),
                outputs: r.outputs.clone(),
                extra_outputs: r.extra_outputs.clone(),
                display: r.command.display.clone(),
                flags: r.command.flags.clone(),
            })
            .collect();

        let store_result = tup_db::store_rules(&db, &mut cache, dir_id, &rules_to_store)?;
        total_stored += store_result.commands.len();
        // Delete stale output files from disk
        for stale in &store_result.stale_outputs {
            let stale_path = tupfile_dir.join(stale);
            let _ = std::fs::remove_file(&stale_path);
        }
    }

    // Clear all remaining flag lists
    db.flags_clear_all()?;
    db.commit()?;

    if total_stored > 0 {
        println!(
            "[ tup ] Stored {} command(s) from {} Tupfile(s).",
            total_stored,
            tupfiles.len()
        );
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

    println!(
        "[ tup ] Scan: {} new, {} modified, {} deleted",
        sync_result.files_added, sync_result.files_modified, sync_result.files_deleted,
    );

    Ok(())
}

fn cmd_graph() -> anyhow::Result<()> {
    let cwd = std::env::current_dir()?;
    let tup_root = tup_platform::init::find_tup_dir(&cwd)
        .ok_or_else(|| anyhow::anyhow!("No .tup directory found."))?;

    let config = load_config_vars(&tup_root);

    let tupfiles =
        tup_platform::scanner::find_tupfiles(&tup_root).map_err(|e| anyhow::anyhow!("{e}"))?;

    let mut all_rules = Vec::new();
    for tupfile_rel in &tupfiles {
        let tupfile_path = tup_root.join(tupfile_rel);
        let tupfile_dir = tupfile_path.parent().unwrap_or(&tup_root);
        let dir_rel = tupfile_dir
            .strip_prefix(&tup_root)
            .unwrap_or(Path::new(""))
            .to_string_lossy()
            .to_string();

        let filename = tupfile_rel.to_string_lossy();
        let noop_db = tup_types::NoopParserDb;
        let parse_result = parse_tupfile_any(
            &tupfile_path,
            tupfile_dir,
            &tup_root,
            &filename,
            &config,
            &noop_db,
        )?;
        let rules = parse_result.rules;

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
    vars: &std::collections::BTreeMap<String, String>,
) -> anyhow::Result<Vec<tup_parser::Rule>> {
    // Build a variable database for second-pass expansion of $(var_%B) etc.
    // In C tup, do_rule() does: tup_printf() (%-flags) → eval() ($-vars).
    // Our parser defers $(var_%B) during first pass since it contains %.
    // After %-expansion here, we do a second $-expansion pass.
    let _global_vars = vars;
    let mut expanded = Vec::new();
    // Track declared outputs from prior rules for glob resolution
    let mut declared_outputs: Vec<String> = Vec::new();
    // Track bins: bin_name → accumulated output files.
    // Bins are C tup's runtime collection mechanism — outputs tagged with
    // {bin} are accumulated and can be used as inputs via {bin} syntax.
    let mut bins: std::collections::BTreeMap<String, Vec<String>> =
        std::collections::BTreeMap::new();
    let dir_name = work_dir
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| ".".to_string());

    for rule in rules {
        // Skip rules where inputs came from variables that expanded to empty.
        // Matches C tup: `: $(empty_var) |> cmd |>` produces no command,
        // but `: |> cmd |>` (explicitly empty) does.
        if rule.had_inputs && rule.inputs.is_empty() {
            continue;
        }

        // Build per-rule vardb for second-pass expansion.
        // Uses the rule's vars_snapshot (captured at parse time) if available,
        // otherwise falls back to global vars. This ensures $(var_%B) uses
        // the variable state at the time the rule was parsed, not the final state.
        let vardb = {
            let mut vdb = tup_parser::ParseVarDb::new();
            let source = rule.vars_snapshot.as_ref().unwrap_or(_global_vars);
            for (k, v) in source {
                vdb.set(k, v);
            }
            vdb
        };

        // Expand {bin} references in inputs before glob expansion.
        // In C tup, bins are runtime collections accumulated during parsing.
        let expanded_inputs: Vec<String> = rule
            .inputs
            .iter()
            .flat_map(|input| {
                if input.starts_with('{') && input.ends_with('}') && input.len() > 2 {
                    let bin_name = &input[1..input.len() - 1];
                    bins.get(bin_name).cloned().unwrap_or_default()
                } else {
                    vec![input.clone()]
                }
            })
            .collect();

        // Expand input globs against filesystem + declared outputs
        let raw_inputs = expand_globs_with_declared(&expanded_inputs, work_dir, &declared_outputs)?;
        // Deduplicate inputs (C tup: bins + explicit may overlap, prune dups)
        let mut seen = std::collections::HashSet::new();
        let matched_inputs: Vec<String> = raw_inputs
            .into_iter()
            .filter(|s| seen.insert(s.clone()))
            .collect();

        // Check for missing explicit inputs (non-glob, non-declared)
        // Matches C tup parser.c:2746-2757: "Explicitly named file not found"
        // Skip cross-directory paths (containing ..) as they reference other dirs
        for input in &expanded_inputs {
            // Skip bin refs, group refs, globs, empty, and cross-directory paths
            if is_explicit_file_ref(input) {
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

        // Check order-only inputs too (skip cross-directory, groups, bins)
        for oo_input in &rule.order_only_inputs {
            if is_explicit_file_ref(oo_input) {
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
        // Skip escaped %% (which produces literal %)
        if matched_inputs.is_empty() && !rule.had_inputs {
            let cmd = &rule.command.command;
            for (pat, desc) in [("%f", "%f"), ("%b", "%b"), ("%B", "%B")] {
                if has_unescaped_percent(cmd, pat) {
                    return Err(anyhow::anyhow!(
                        "tup error: {} used in rule pattern and no input files were specified",
                        desc
                    ));
                }
            }
        }
        if rule.order_only_inputs.is_empty() && has_unescaped_percent(&rule.command.command, "%i") {
            return Err(anyhow::anyhow!(
                "tup error: %i used in rule pattern and no order-only input files were specified"
            ));
        }

        if rule.foreach {
            // Expand foreach: one rule per input file
            for input_path in &matched_inputs {
                let input = tup_parser::InputFile::new(input_path);

                // Expand output patterns for this input
                let outputs: Vec<String> = rule
                    .outputs
                    .iter()
                    .map(|pat| tup_parser::expand_output_pattern(pat, &input))
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(anyhow::Error::msg)?;

                // Validate output paths (reject hidden files like .git)
                for out in &outputs {
                    tup_parser::validate_output_path(out).map_err(anyhow::Error::msg)?;
                }

                // Expand % in command
                let cmd_percent = tup_parser::expand_percent(
                    &rule.command.command,
                    std::slice::from_ref(&input),
                    &outputs,
                    &rule.order_only_inputs,
                    &dir_name,
                )
                .map_err(anyhow::Error::msg)?;
                // Second pass: expand $(var) after %-flags resolved (C: tup_printf → eval)
                // Only run when rule had deferred variables (names containing %)
                let cmd = if rule.vars_snapshot.is_some() {
                    vardb.expand_no_defer(&cmd_percent)
                } else {
                    cmd_percent
                };

                // Expand % in display string if present
                let display = rule
                    .command
                    .display
                    .as_ref()
                    .map(|d| {
                        tup_parser::expand_percent(
                            d,
                            std::slice::from_ref(&input),
                            &outputs,
                            &rule.order_only_inputs,
                            &dir_name,
                        )
                    })
                    .transpose()
                    .map_err(anyhow::Error::msg)?
                    .map(|d| {
                        if rule.vars_snapshot.is_some() {
                            vardb.expand_no_defer(&d)
                        } else {
                            d
                        }
                    });

                // Track these outputs for later rules
                declared_outputs.extend(outputs.clone());
                // Add outputs to bin if rule has one
                if let Some(ref bin_name) = rule.bin {
                    bins.entry(bin_name.clone())
                        .or_default()
                        .extend(outputs.clone());
                }

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
                    vars_snapshot: None,
                    bin: rule.bin.clone(),
                });
            }
        } else {
            // Non-foreach: expand globs and % substitutions
            let inputs: Vec<tup_parser::InputFile> = matched_inputs
                .iter()
                .map(|p| tup_parser::InputFile::new(p))
                .collect();

            // Expand output patterns with % substitutions
            let outputs: Vec<String> = if let Some(first_input) = inputs.first() {
                rule.outputs
                    .iter()
                    .map(|pat| {
                        if pat.contains('%') {
                            tup_parser::expand_output_pattern(pat, first_input)
                        } else {
                            Ok(pat.clone())
                        }
                    })
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(anyhow::Error::msg)?
            } else {
                rule.outputs.clone()
            };

            // Validate output paths (reject hidden files like .git)
            for out in &outputs {
                tup_parser::validate_output_path(out).map_err(anyhow::Error::msg)?;
            }

            let cmd_percent = tup_parser::expand_percent(
                &rule.command.command,
                &inputs,
                &outputs,
                &rule.order_only_inputs,
                &dir_name,
            )
            .map_err(anyhow::Error::msg)?;
            // Second pass: expand $(var) after %-flags resolved (C: tup_printf → eval)
            let cmd = if rule.vars_snapshot.is_some() {
                vardb.expand_no_defer(&cmd_percent)
            } else {
                cmd_percent
            };

            let display = rule
                .command
                .display
                .as_ref()
                .map(|d| {
                    tup_parser::expand_percent(
                        d,
                        &inputs,
                        &outputs,
                        &rule.order_only_inputs,
                        &dir_name,
                    )
                })
                .transpose()
                .map_err(anyhow::Error::msg)?
                .map(|d| {
                    if rule.vars_snapshot.is_some() {
                        vardb.expand_no_defer(&d)
                    } else {
                        d
                    }
                });

            // Track these outputs for later rules
            declared_outputs.extend(outputs.clone());
            // Add outputs to bin if rule has one
            if let Some(ref bin_name) = rule.bin {
                bins.entry(bin_name.clone())
                    .or_default()
                    .extend(outputs.clone());
            }

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
                vars_snapshot: None,
                bin: rule.bin.clone(),
            });
        }
    }

    // Check for duplicate outputs within each rule first
    // C tup: "The output file 'X' is listed multiple times in a command"
    for rule in &expanded {
        let mut rule_outputs = std::collections::HashSet::new();
        for output in &rule.outputs {
            if !rule_outputs.insert(output.clone()) {
                return Err(anyhow::anyhow!(
                    "tup error: The output file '{}' is listed multiple times in a command",
                    output,
                ));
            }
        }
    }

    // Check for duplicate outputs across different rules
    // C tup: "Unable to create output file because it is already owned by command"
    let mut seen_outputs: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    for rule in &expanded {
        for output in &rule.outputs {
            if let Some(prev_cmd) = seen_outputs.get(output) {
                if prev_cmd != &rule.command.command {
                    return Err(anyhow::anyhow!(
                        "tup error: Unable to create output file '{}' in '{}' because it is already owned by '{}'",
                        output, rule.command.command, prev_cmd,
                    ));
                }
            }
            seen_outputs.insert(output.clone(), rule.command.command.clone());
        }
    }

    Ok(expanded)
}

/// Check if an input name refers to an explicit local file (not a glob, bin, group, or cross-dir path).
fn is_explicit_file_ref(name: &str) -> bool {
    !name.is_empty()
        && !tup_parser::is_glob(name)
        && !name.contains("..")
        && !name.contains('/')
        && !is_group_or_bin(name)
}

/// Check if a name is a group (<name>) or bin ({name}) reference.
fn is_group_or_bin(name: &str) -> bool {
    (name.starts_with('<') && name.ends_with('>'))
        || (name.starts_with('{') && name.ends_with('}'))
        || name.contains('<')
}

/// Check if a command string contains an unescaped percent pattern.
/// Returns false for `%%f` (escaped percent), true for `%f`.
fn has_unescaped_percent(cmd: &str, pattern: &str) -> bool {
    let suffix = &pattern[1..]; // e.g., "f" from "%f"
    let mut chars = cmd.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '%' {
            if chars.peek() == Some(&'%') {
                chars.next(); // skip escaped %%
            } else {
                let rest: String = chars.clone().take(suffix.len()).collect();
                if rest == suffix {
                    return true;
                }
            }
        }
    }
    false
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
                if ti >= text.len() {
                    return false;
                }
                pi += 1;
                ti += 1;
            }
            c => {
                if ti >= text.len() || text[ti] != c {
                    return false;
                }
                pi += 1;
                ti += 1;
            }
        }
    }

    ti == text.len()
}

/// Result of parsing a Tupfile, including rules and metadata.
struct ParseResult {
    rules: Vec<tup_parser::Rule>,
    gitignore: bool,
    /// Variables defined in the Tupfile (for multi-pass expansion of $(var_%B) etc.)
    vars: std::collections::BTreeMap<String, String>,
}

/// Simple string hash for change detection.
fn simple_hash(s: &str) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    s.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

/// Load config variables from tup.config file.
fn load_config_vars(tup_root: &Path) -> std::collections::BTreeMap<String, String> {
    let config_path = tup_root.join("tup.config");
    let mut vars = std::collections::BTreeMap::new();

    if let Ok(content) = std::fs::read_to_string(&config_path) {
        for line in content.lines() {
            let line = line.trim();
            if line.starts_with("CONFIG_") {
                // CONFIG_VAR=value
                if let Some((key, value)) = line.split_once('=') {
                    let var_name = key.strip_prefix("CONFIG_").unwrap_or(key);
                    vars.insert(var_name.to_string(), value.to_string());
                }
            } else if line.starts_with("# CONFIG_") && line.ends_with(" is not set") {
                // # CONFIG_VAR is not set → var exists with value "n"
                let rest = line.strip_prefix("# CONFIG_").unwrap();
                let var_name = rest.strip_suffix(" is not set").unwrap_or(rest);
                vars.insert(var_name.to_string(), "n".to_string());
            }
        }
    }

    vars
}

/// Write vardict file (parsed config variables for test framework).
fn write_vardict(tup_root: &Path, config: &std::collections::BTreeMap<String, String>) {
    let vardict_path = tup_root.join(".tup").join("vardict");
    let mut lines = Vec::new();
    for (k, v) in config {
        lines.push(format!("{k}={v}"));
    }
    let _ = std::fs::write(vardict_path, lines.join("\n") + "\n");
}

/// CLI handler for `tup varsed [--binary] <input> <output>`.
///
/// Reads variables from the vardict (via tup_vardict env var or .tup/vardict),
/// replaces @VAR@ patterns in the input file, and writes the result.
fn cmd_varsed_cli(input: &str, output: &str, binmode: bool) -> anyhow::Result<()> {
    // Try to find the tup root for fallback vardict loading
    let tup_root = tup_platform::init::find_tup_dir(&std::env::current_dir()?);
    tup_parser::cmd_varsed(input, output, binmode, tup_root.as_deref())
        .map_err(|e| anyhow::anyhow!("{e}"))
}

/// Parse a Tupfile (either standard or Lua) and return rules + metadata.
fn parse_tupfile_any(
    tupfile_path: &Path,
    tupfile_dir: &Path,
    tup_root: &Path,
    filename: &str,
    config: &std::collections::BTreeMap<String, String>,
    db: &dyn tup_types::ParserDb,
) -> anyhow::Result<ParseResult> {
    let content = std::fs::read_to_string(tupfile_path)?;

    if filename.ends_with(".lua") {
        let rules = tup_parser::parse_lua_tupfile(&content, filename, tupfile_dir, db)
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        Ok(ParseResult {
            rules,
            gitignore: false,
            vars: std::collections::BTreeMap::new(),
        })
    } else {
        let mut reader = tup_parser::TupfileReader::parse(&content, filename)?;
        // Load config variables for @(VAR) and $(CONFIG_VAR) expansion
        for (k, v) in config {
            reader.set_config(k, v);
            // Also make config vars available as $(CONFIG_VAR)
            // Matches C tup behavior
            reader.set_var(&format!("CONFIG_{k}"), v);
        }
        let rules = reader.evaluate_with_dirs(Some(tupfile_dir), Some(tup_root), None)?;
        let gitignore = reader.gitignore_requested();
        let vars = reader.all_vars().clone();
        Ok(ParseResult {
            rules,
            gitignore,
            vars,
        })
    }
}

/// Read tup.config and sync config variables to the database.
///
/// Ported from C tup: main.c calls updater(argc, argv, 1) which does
/// scan + process_config_nodes (updater.c:195-198). process_config_nodes
/// calls tup_db_read_vars (db.c:5003) which:
/// 1. Gets existing VAR nodes from DB under the tup.config entry
/// 2. Reads tup.config from filesystem (get_file_var_tree, db.c:7672)
/// 3. Compares and syncs: removes old vars, adds new, updates changed
/// 4. Writes vardict file
fn cmd_read() -> anyhow::Result<()> {
    let cwd = std::env::current_dir()?;
    let tup_root = tup_platform::init::find_tup_dir(&cwd)
        .ok_or_else(|| anyhow::anyhow!("No .tup directory found. Run 'tup init' first."))?;

    let db = tup_db::TupDb::open(&tup_root, false)?;
    let mut cache = tup_db::EntryCache::new();

    // Phase 1: Scan filesystem (same as C tup's run_scan in updater.c:192)
    let sync_result = tup_db::sync_filesystem(&db, &mut cache, &tup_root)?;
    if sync_result.files_added > 0
        || sync_result.files_modified > 0
        || sync_result.files_deleted > 0
    {
        println!(
            "[ tup ] Scan: {} new, {} modified, {} deleted",
            sync_result.files_added, sync_result.files_modified, sync_result.files_deleted,
        );
    }

    // Phase 2: Read config vars from tup.config file
    let config = load_config_vars(&tup_root);

    // Phase 3: Sync config vars to DB as VAR nodes under a "tup.config" directory node.
    //
    // In C tup, the tup.config FILE node's tupid is used as the parent dir
    // for VAR nodes. We replicate this: ensure "tup.config" exists as a node
    // in the root directory, then create VAR children under it.
    db.begin()?;

    // Get or create the tup.config node in the root dir (DOT_DT).
    // In C, this node is the tup.config file entry itself. Its tupid becomes
    // the dir for VAR nodes.
    let vartent_id = match db.node_select(DOT_DT, "tup.config")? {
        Some(row) => row.id,
        None => {
            // Create it as a File node (matches C: tup.config is a file in the fs)
            let result =
                db.create_node(&mut cache, DOT_DT, "tup.config", NodeType::File, -1, 0, 0)?;
            match result {
                tup_db::CreateResult::Created(id) => id,
                tup_db::CreateResult::Existing(id) => id,
            }
        }
    };

    // Get existing VAR nodes from DB (C: tup_db_get_vardb, db.c:7605)
    // Query: select node.id, name, value, type from node, var where dir=vartent_id and node.id=var.id
    let existing_nodes = db.node_select_dir(vartent_id)?;
    let mut db_vars: std::collections::BTreeMap<String, (TupId, Option<String>)> =
        std::collections::BTreeMap::new();
    for node in &existing_nodes {
        let value = db.var_get(node.id)?;
        db_vars.insert(node.name.clone(), (node.id, value));
    }

    // Compare and sync (C: vardb_compare with remove_var, add_var, compare_vars callbacks)

    // Remove vars that are in DB but not in file (C: remove_var, db.c:4966)
    for (name, (id, _value)) in &db_vars {
        if !config.contains_key(name.as_str()) {
            // C: remove_var_tupid → delete from var, delete from node
            db.var_delete(*id)?;
            db.node_delete(*id)?;
        }
    }

    // Add or update vars from file (C: add_var db.c:4973, compare_vars db.c:4985)
    for (name, value) in &config {
        match db_vars.get(name.as_str()) {
            None => {
                // New var — create node + var entry (C: add_var → tup_db_create_node + tup_db_set_var)
                let id = db.node_insert(vartent_id, name, NodeType::Var, 0, 0, -1, None, None)?;
                db.var_set(id, value)?;
            }
            Some((_id, existing_value)) => {
                // Existing var — update if changed (C: compare_vars)
                let needs_update = match existing_value {
                    Some(v) => v != value,
                    None => true,
                };
                if needs_update {
                    db.var_set(*_id, value)?;
                }
            }
        }
    }

    db.commit()?;

    // Write vardict file (C: save_vardict_file)
    write_vardict(&tup_root, &config);

    Ok(())
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
    let name = rel_path
        .file_name()
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

fn cmd_dbconfig() -> anyhow::Result<()> {
    let cwd = std::env::current_dir()?;
    let tup_root = tup_platform::init::find_tup_dir(&cwd)
        .ok_or_else(|| anyhow::anyhow!("No .tup directory found."))?;

    let db = tup_db::TupDb::open(&tup_root, false)?;

    // Match C tup's dbconfig output format
    let db_version = db.config_get_string("db_version", "0")?;
    let parser_version = db.config_get_string("parser_version", "0")?;
    println!("db_version: '{db_version}'");
    println!("parser_version: '{parser_version}'");
    Ok(())
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

fn cmd_create_flags_exists() -> anyhow::Result<()> {
    let cwd = std::env::current_dir()?;
    let tup_root = tup_platform::init::find_tup_dir(&cwd)
        .ok_or_else(|| anyhow::anyhow!("No .tup directory found."))?;

    let db = tup_db::TupDb::open(&tup_root, false)?;

    let create_list = db.flag_list(tup_types::TupFlags::Create)?;
    if create_list.is_empty() {
        // No create flags — exit 0
        Ok(())
    } else {
        // Create flags exist — exit non-zero
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

    let node1 = db
        .node_select(dir1_id, name1)?
        .ok_or_else(|| anyhow::anyhow!("node '{name1}' not found in dir '{dir1}'"))?;
    let node2 = db
        .node_select(dir2_id, name2)?
        .ok_or_else(|| anyhow::anyhow!("node '{name2}' not found in dir '{dir2}'"))?;

    if db.link_exists(node1.id, node2.id, link_type)? {
        // Link exists — exit code 11 (C tup convention)
        process::exit(11);
    } else {
        // Link doesn't exist — exit code 0
        Ok(())
    }
}
