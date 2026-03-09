use tup_types::{LinkType, NodeType, TupFlags, TupId};

use crate::entry::{EntryCache, TupEntry};
use crate::error::DbResult;
use crate::schema::TupDb;

/// A command stored in the database, with its inputs and outputs.
#[derive(Debug)]
pub struct StoredCommand {
    /// TupId of the CMD node.
    pub cmd_id: TupId,
    /// The command string (stored as the node name).
    pub command: String,
    /// Display string (optional).
    pub display: Option<String>,
    /// Flags string (optional).
    pub flags: Option<String>,
    /// Directory this command runs in.
    pub dir_id: TupId,
}

/// A parsed rule ready to be stored in the database.
#[derive(Debug, Clone)]
pub struct RuleToStore {
    pub command: String,
    pub inputs: Vec<String>,
    /// Order-only inputs (after `|` in input section).
    /// FILE-type order-only inputs get NORMAL links (not STICKY).
    /// GENERATED-type order-only inputs get STICKY links.
    /// Matches C tup's add_input() with force_normal_file parameter.
    pub order_only_inputs: Vec<String>,
    pub outputs: Vec<String>,
    /// Extra outputs (after `|` in output section).
    /// These are order-only outputs that get CMD→output NORMAL links
    /// but are NOT the primary outputs of the command.
    pub extra_outputs: Vec<String>,
    pub display: Option<String>,
    pub flags: Option<String>,
}

/// Store parsed rules into the database as CMD nodes with links.
///
/// For each rule:
/// 1. Create or find a CMD node (keyed by command hash)
/// 2. Create or find output FILE/GENERATED nodes
/// 3. Create normal links: input → CMD, CMD → output
/// 4. Flag the CMD as MODIFY if it's new or changed
///
/// Returns the list of stored commands.
pub fn store_rules(
    db: &TupDb,
    cache: &mut EntryCache,
    dir_id: TupId,
    rules: &[RuleToStore],
) -> DbResult<StoreResult> {
    let mut stored = Vec::new();
    let mut stale_outputs = Vec::new();

    for rule in rules {
        let RuleToStore {
            command,
            inputs,
            order_only_inputs: _,
            outputs,
            extra_outputs: _,
            display,
            flags,
        } = rule;
        // In C tup, the command name IS the full command string.
        // UNIQUE(dir, name) deduplicates same command in same directory.
        let cmd_name = command_node_name(command);

        // Create or find the CMD node
        let existing = db.node_select(dir_id, &cmd_name)?;
        let was_existing = existing.is_some();
        let cmd_id = match existing {
            Some(row) if row.node_type == NodeType::Cmd => {
                // Command exists — check if it changed
                let old_display = row.display.as_deref();
                let old_flags = row.flags.as_deref();
                let new_display = display.as_deref();
                let new_flags = flags.as_deref();

                if old_display != new_display || old_flags != new_flags {
                    db.node_set_display(row.id, new_display)?;
                    db.node_set_flags(row.id, new_flags)?;
                    db.flag_add(row.id, TupFlags::Modify)?;
                }
                row.id
            }
            Some(row) if row.node_type == NodeType::Ghost => {
                // Ghost → CMD
                db.node_set_type(row.id, NodeType::Cmd)?;
                db.node_set_display(row.id, display.as_deref())?;
                db.node_set_flags(row.id, flags.as_deref())?;
                db.flag_add(row.id, TupFlags::Modify)?;
                cache.change_type(row.id, NodeType::Cmd);
                row.id
            }
            Some(_) => {
                // Type conflict — skip
                continue;
            }
            None => {
                // New command
                let id = db.node_insert(
                    dir_id,
                    &cmd_name,
                    NodeType::Cmd,
                    -1,
                    0,
                    -1,
                    display.as_deref(),
                    flags.as_deref(),
                )?;
                db.flag_add(id, TupFlags::Modify)?;

                if let Some(row) = db.node_select_by_id(id)? {
                    cache.add(TupEntry::from_node_row(&row));
                }
                id
            }
        };

        // Clean up old links for existing CMDs before creating new ones.
        // This matches C tup's tup_db_write_outputs()/tup_db_write_inputs()
        // which reconcile old links with new ones on each parse.
        if was_existing {
            // Remove old CMD → output normal links
            let old_output_ids = db.get_normal_outputs(cmd_id)?;
            for old_out_id in &old_output_ids {
                db.link_remove(cmd_id, *old_out_id, LinkType::Normal)?;
            }
            // Remove old input → CMD sticky links
            let old_sticky_ids = db.get_sticky_inputs(cmd_id)?;
            for old_in_id in &old_sticky_ids {
                db.link_remove(*old_in_id, cmd_id, LinkType::Sticky)?;
            }
            // Remove old input → CMD normal links
            let old_normal_ids = db.get_normal_inputs(cmd_id)?;
            for old_in_id in &old_normal_ids {
                db.link_remove(*old_in_id, cmd_id, LinkType::Normal)?;
            }
        }

        // Create links for regular inputs: input_node → CMD (STICKY)
        // C tup uses STICKY links for declared inputs (from Tupfile rules).
        for input_name in inputs {
            if let Some(input_node) = db.node_select(dir_id, input_name)? {
                db.link_insert(input_node.id, cmd_id, LinkType::Sticky)?;
            }
            // If input doesn't exist in DB, it might be from another
            // directory or not yet scanned — will be resolved later
        }

        // Create links for order-only inputs:
        // - Generated files → STICKY link (matches C tup: generated order-only
        //   inputs still track changes)
        // - Regular files → NORMAL link only (C tup parser.c:3153 skips FILE
        //   types for order-only unless they're generated)
        for oo_name in &rule.order_only_inputs {
            if let Some(oo_node) = db.node_select(dir_id, oo_name)? {
                let link_type = if oo_node.node_type == NodeType::Generated {
                    LinkType::Sticky
                } else {
                    LinkType::Normal
                };
                db.link_insert(oo_node.id, cmd_id, link_type)?;
            }
        }

        // Create output nodes and links: CMD → output_node
        for output_name in outputs {
            let output_id = match db.node_select(dir_id, output_name)? {
                Some(row) => {
                    // Update to Generated type if needed
                    if row.node_type != NodeType::Generated && row.node_type != NodeType::Ghost {
                        // Already exists as a different type — may be a source
                        // file being overwritten. For now, skip the link.
                        continue;
                    }
                    if row.node_type == NodeType::Ghost {
                        db.node_set_type(row.id, NodeType::Generated)?;
                    }
                    // Update srcid to point to the current command.
                    // This handles the case where the same output is produced
                    // by a different command string (e.g., link command changes
                    // when source files are added/removed).
                    if row.srcid != cmd_id.raw() {
                        db.node_set_srcid(row.id, cmd_id.raw())?;
                    }
                    row.id
                }
                None => {
                    // Create new Generated node
                    db.node_insert(
                        dir_id,
                        output_name,
                        NodeType::Generated,
                        -1,
                        0,
                        cmd_id.raw(),
                        None,
                        None,
                    )?
                }
            };
            db.link_insert(cmd_id, output_id, LinkType::Normal)?;
        }

        // Create extra output nodes and links: CMD → extra_output_node
        // Extra outputs (after | in output section) are order-only outputs.
        // They get the same treatment as regular outputs in the DB.
        for extra_name in &rule.extra_outputs {
            let extra_id = match db.node_select(dir_id, extra_name)? {
                Some(row) => {
                    if row.node_type != NodeType::Generated && row.node_type != NodeType::Ghost {
                        continue;
                    }
                    if row.node_type == NodeType::Ghost {
                        db.node_set_type(row.id, NodeType::Generated)?;
                    }
                    row.id
                }
                None => db.node_insert(
                    dir_id,
                    extra_name,
                    NodeType::Generated,
                    -1,
                    0,
                    cmd_id.raw(),
                    None,
                    None,
                )?,
            };
            db.link_insert(cmd_id, extra_id, LinkType::Normal)?;
        }

        stored.push(StoredCommand {
            cmd_id,
            command: command.clone(),
            display: display.clone(),
            flags: flags.clone(),
            dir_id,
        });
    }

    // Clean up stale commands: remove CMD nodes in this directory that
    // were not produced by the current parse. This handles the case where
    // a Tupfile changes and old commands should be deleted.
    // Also collects stale output files for disk deletion by the caller.
    let active_cmd_ids: std::collections::HashSet<TupId> =
        stored.iter().map(|s| s.cmd_id).collect();
    // Collect all active output names for orphan detection
    let active_outputs: std::collections::HashSet<&str> = rules
        .iter()
        .flat_map(|r| r.outputs.iter().chain(r.extra_outputs.iter()))
        .map(|s| s.as_str())
        .collect();
    let existing_nodes = db.node_select_dir(dir_id)?;
    for row in &existing_nodes {
        if row.node_type == NodeType::Cmd && !active_cmd_ids.contains(&row.id) {
            // Stale command — find and remove its generated outputs too
            for output in &existing_nodes {
                if output.node_type == NodeType::Generated && output.srcid == row.id.raw() {
                    // Track for disk deletion
                    stale_outputs.push(output.name.clone());
                    db.link_delete_all(output.id)?;
                    db.node_delete(output.id)?;
                }
            }
            // Remove the command node and its links
            db.link_delete_all(row.id)?;
            db.node_delete(row.id)?;
        }
    }
    // Clean up orphaned Generated nodes: nodes that were outputs of a
    // still-active CMD but are no longer in its output list (CMD changed outputs).
    for row in &existing_nodes {
        if row.node_type == NodeType::Generated
            && active_cmd_ids.contains(&TupId::new(row.srcid))
            && !active_outputs.contains(row.name.as_str())
        {
            stale_outputs.push(row.name.clone());
            db.link_delete_all(row.id)?;
            db.node_delete(row.id)?;
        }
    }

    Ok(StoreResult {
        commands: stored,
        stale_outputs,
    })
}

/// Result of storing rules, including stale outputs to delete.
pub struct StoreResult {
    pub commands: Vec<StoredCommand>,
    /// Output files from removed commands that should be deleted from disk.
    pub stale_outputs: Vec<String>,
}

/// Get all commands in the modify list (need re-execution).
pub fn get_modified_commands(db: &TupDb) -> DbResult<Vec<TupId>> {
    let modify_ids = db.flag_list(TupFlags::Modify)?;
    let mut cmd_ids = Vec::new();

    for id in modify_ids {
        if let Some(row) = db.node_select_by_id(id)? {
            if row.node_type == NodeType::Cmd {
                cmd_ids.push(id);
            }
        }
    }

    Ok(cmd_ids)
}

/// Clear the modify flag for a command after successful execution.
pub fn mark_command_done(db: &TupDb, cmd_id: TupId) -> DbResult<()> {
    db.flag_remove(cmd_id, TupFlags::Modify)?;
    Ok(())
}

/// Get the node name for a command.
///
/// In C tup, the command name IS the full command string.
/// Commands in the same directory with the same string are deduplicated
/// by the UNIQUE(dir, name) constraint.
fn command_node_name(command: &str) -> String {
    command.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> (TupDb, EntryCache) {
        let db = TupDb::create_in_memory().unwrap();
        let mut cache = EntryCache::new();
        cache.load(&db, tup_types::DOT_DT).unwrap();
        (db, cache)
    }

    #[test]
    fn test_store_simple_rule() {
        let (db, mut cache) = setup();
        db.begin().unwrap();

        // Create an input file first
        db.node_insert(
            tup_types::DOT_DT,
            "main.c",
            NodeType::File,
            1000,
            0,
            -1,
            None,
            None,
        )
        .unwrap();

        let rules = vec![RuleToStore {
            command: "gcc -c main.c -o main.o".to_string(),
            inputs: vec!["main.c".to_string()],
            order_only_inputs: vec![],
            outputs: vec!["main.o".to_string()],
            extra_outputs: vec![],
            display: Some("CC main.c".to_string()),
            flags: None,
        }];

        let stored = store_rules(&db, &mut cache, tup_types::DOT_DT, &rules).unwrap();
        assert_eq!(stored.commands.len(), 1);

        // CMD should exist in DB
        let cmd = db
            .node_select_by_id(stored.commands[0].cmd_id)
            .unwrap()
            .unwrap();
        assert_eq!(cmd.node_type, NodeType::Cmd);
        assert_eq!(cmd.display, Some("CC main.c".to_string()));

        // CMD should be in modify list
        assert!(db
            .flag_check(stored.commands[0].cmd_id, TupFlags::Modify)
            .unwrap());

        // Output should exist as Generated
        let output = db
            .node_select(tup_types::DOT_DT, "main.o")
            .unwrap()
            .unwrap();
        assert_eq!(output.node_type, NodeType::Generated);

        // Input links should be STICKY (C tup uses sticky for declared inputs)
        assert!(db
            .link_exists(
                db.node_select(tup_types::DOT_DT, "main.c")
                    .unwrap()
                    .unwrap()
                    .id,
                stored.commands[0].cmd_id,
                LinkType::Sticky,
            )
            .unwrap());

        db.commit().unwrap();
    }

    #[test]
    fn test_store_same_rule_twice() {
        let (db, mut cache) = setup();
        db.begin().unwrap();

        let rules = vec![RuleToStore {
            command: "echo hello".to_string(),
            inputs: vec![],
            order_only_inputs: vec![],
            outputs: vec!["out.txt".to_string()],
            extra_outputs: vec![],
            display: None,
            flags: None,
        }];

        let stored1 = store_rules(&db, &mut cache, tup_types::DOT_DT, &rules).unwrap();
        // Clear modify flag
        mark_command_done(&db, stored1.commands[0].cmd_id).unwrap();

        // Store same rule again — should not re-flag
        let stored2 = store_rules(&db, &mut cache, tup_types::DOT_DT, &rules).unwrap();
        assert_eq!(stored1.commands[0].cmd_id, stored2.commands[0].cmd_id);
        assert!(!db
            .flag_check(stored2.commands[0].cmd_id, TupFlags::Modify)
            .unwrap());

        db.commit().unwrap();
    }

    #[test]
    fn test_store_changed_display() {
        let (db, mut cache) = setup();
        db.begin().unwrap();

        let rules1 = vec![RuleToStore {
            command: "gcc -c foo.c".to_string(),
            inputs: vec![],
            order_only_inputs: vec![],
            outputs: vec![],
            extra_outputs: vec![],
            display: Some("CC foo.c".to_string()),
            flags: None,
        }];
        let stored1 = store_rules(&db, &mut cache, tup_types::DOT_DT, &rules1).unwrap();
        mark_command_done(&db, stored1.commands[0].cmd_id).unwrap();

        // Change display string
        let rules2 = vec![RuleToStore {
            command: "gcc -c foo.c".to_string(),
            inputs: vec![],
            order_only_inputs: vec![],
            outputs: vec![],
            extra_outputs: vec![],
            display: Some("COMPILE foo.c".to_string()),
            flags: None,
        }];
        let stored2 = store_rules(&db, &mut cache, tup_types::DOT_DT, &rules2).unwrap();

        // Should be re-flagged due to display change
        assert!(db
            .flag_check(stored2.commands[0].cmd_id, TupFlags::Modify)
            .unwrap());

        db.commit().unwrap();
    }

    #[test]
    fn test_get_modified_commands() {
        let (db, mut cache) = setup();
        db.begin().unwrap();

        let rules = vec![
            RuleToStore {
                command: "cmd1".to_string(),
                inputs: vec![],
                order_only_inputs: vec![],
                outputs: vec![],
                extra_outputs: vec![],
                display: None,
                flags: None,
            },
            RuleToStore {
                command: "cmd2".to_string(),
                inputs: vec![],
                order_only_inputs: vec![],
                outputs: vec![],
                extra_outputs: vec![],
                display: None,
                flags: None,
            },
        ];
        store_rules(&db, &mut cache, tup_types::DOT_DT, &rules).unwrap();

        let modified = get_modified_commands(&db).unwrap();
        assert_eq!(modified.len(), 2);

        db.commit().unwrap();
    }

    #[test]
    fn test_mark_command_done() {
        let (db, mut cache) = setup();
        db.begin().unwrap();

        let rules = vec![RuleToStore {
            command: "test cmd".to_string(),
            inputs: vec![],
            order_only_inputs: vec![],
            outputs: vec![],
            extra_outputs: vec![],
            display: None,
            flags: None,
        }];
        let stored = store_rules(&db, &mut cache, tup_types::DOT_DT, &rules).unwrap();

        assert!(db
            .flag_check(stored.commands[0].cmd_id, TupFlags::Modify)
            .unwrap());
        mark_command_done(&db, stored.commands[0].cmd_id).unwrap();
        assert!(!db
            .flag_check(stored.commands[0].cmd_id, TupFlags::Modify)
            .unwrap());

        db.commit().unwrap();
    }

    #[test]
    fn test_command_node_name_is_full_string() {
        // C tup stores the full command string as the node name
        let name = command_node_name("gcc -c foo.c -o foo.o");
        assert_eq!(name, "gcc -c foo.c -o foo.o");
    }

    #[test]
    fn test_command_node_name_different() {
        let n1 = command_node_name("gcc -c foo.c");
        let n2 = command_node_name("gcc -c bar.c");
        assert_ne!(n1, n2);
    }
}
