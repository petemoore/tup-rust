use std::path::Path;
use std::time::SystemTime;

use tup_types::{NodeType, TupId};

use crate::error::DbResult;
use crate::schema::TupDb;

/// Result of post-execution output tracking.
#[derive(Debug, Default)]
pub struct OutputTrackResult {
    /// Outputs successfully updated with new mtimes.
    pub updated: usize,
    /// Declared outputs that were not created.
    pub missing: Vec<String>,
    /// Files written but not declared as outputs.
    pub extra: Vec<String>,
}

/// Update output file mtimes in the database after command execution.
///
/// Verified against C source (file.c:453-468, updater.c:2704-2706):
/// - Stat each declared output file from disk
/// - Store its mtime in the database
/// - Report missing outputs (declared but not created)
///
/// Also updates the command node's mtime to the execution time.
pub fn track_outputs(
    db: &TupDb,
    cmd_id: TupId,
    dir_id: TupId,
    declared_outputs: &[String],
    work_dir: &Path,
) -> DbResult<OutputTrackResult> {
    let mut result = OutputTrackResult::default();

    for output_name in declared_outputs {
        let output_path = work_dir.join(output_name);

        if output_path.exists() {
            // Get mtime from disk
            let (mtime, mtime_ns) = get_file_mtime(&output_path);

            // Find or create the output node
            match db.node_select(dir_id, output_name)? {
                Some(row) => {
                    // Update mtime in database
                    db.node_set_mtime(row.id, mtime, mtime_ns)?;

                    // Ensure it's marked as Generated
                    if row.node_type != NodeType::Generated {
                        db.node_set_type(row.id, NodeType::Generated)?;
                    }

                    result.updated += 1;
                }
                None => {
                    // Output node doesn't exist yet — create it
                    db.node_insert(
                        dir_id,
                        output_name,
                        NodeType::Generated,
                        mtime,
                        mtime_ns,
                        cmd_id.raw(),
                        None,
                        None,
                    )?;
                    result.updated += 1;
                }
            }
        } else {
            result.missing.push(output_name.clone());
        }
    }

    // Update command node mtime to current time
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| (d.as_secs() as i64, d.subsec_nanos() as i64))
        .unwrap_or((-1, 0));
    db.node_set_mtime(cmd_id, now.0, now.1)?;

    Ok(result)
}

/// Get modification time of a file.
fn get_file_mtime(path: &Path) -> (i64, i64) {
    std::fs::metadata(path)
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
        .map(|d| (d.as_secs() as i64, d.subsec_nanos() as i64))
        .unwrap_or((-1, 0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tup_types::DOT_DT;

    #[test]
    fn test_track_outputs_present() {
        let tmp = tempfile::tempdir().unwrap();
        tup_platform::init::init_command(tmp.path(), false, false).unwrap();

        std::fs::write(tmp.path().join("output.o"), "compiled").unwrap();

        let db = TupDb::open(tmp.path(), false).unwrap();
        db.begin().unwrap();

        let cmd_id = db
            .node_insert(DOT_DT, "gcc -c foo.c", NodeType::Cmd, -1, 0, -1, None, None)
            .unwrap();

        let result =
            track_outputs(&db, cmd_id, DOT_DT, &["output.o".to_string()], tmp.path()).unwrap();

        assert_eq!(result.updated, 1);
        assert!(result.missing.is_empty());

        // Output should exist in DB with correct mtime
        let node = db.node_select(DOT_DT, "output.o").unwrap().unwrap();
        assert_eq!(node.node_type, NodeType::Generated);
        assert!(node.mtime > 0);

        db.commit().unwrap();
    }

    #[test]
    fn test_track_outputs_missing() {
        let tmp = tempfile::tempdir().unwrap();
        tup_platform::init::init_command(tmp.path(), false, false).unwrap();

        let db = TupDb::open(tmp.path(), false).unwrap();
        db.begin().unwrap();

        let cmd_id = db
            .node_insert(DOT_DT, "gcc -c foo.c", NodeType::Cmd, -1, 0, -1, None, None)
            .unwrap();

        let result =
            track_outputs(&db, cmd_id, DOT_DT, &["missing.o".to_string()], tmp.path()).unwrap();

        assert_eq!(result.updated, 0);
        assert_eq!(result.missing, vec!["missing.o"]);

        db.commit().unwrap();
    }

    #[test]
    fn test_track_outputs_updates_existing() {
        let tmp = tempfile::tempdir().unwrap();
        tup_platform::init::init_command(tmp.path(), false, false).unwrap();

        let db = TupDb::open(tmp.path(), false).unwrap();
        db.begin().unwrap();

        // Create output node with old mtime
        db.node_insert(
            DOT_DT,
            "result.o",
            NodeType::Generated,
            100,
            0,
            -1,
            None,
            None,
        )
        .unwrap();

        // Write the actual file
        std::fs::write(tmp.path().join("result.o"), "new content").unwrap();

        let cmd_id = db
            .node_insert(DOT_DT, "gcc", NodeType::Cmd, -1, 0, -1, None, None)
            .unwrap();

        let result =
            track_outputs(&db, cmd_id, DOT_DT, &["result.o".to_string()], tmp.path()).unwrap();

        assert_eq!(result.updated, 1);

        // Mtime should be updated from disk (not the old 100)
        let node = db.node_select(DOT_DT, "result.o").unwrap().unwrap();
        assert!(node.mtime > 100);

        db.commit().unwrap();
    }

    #[test]
    fn test_track_outputs_cmd_mtime_updated() {
        let tmp = tempfile::tempdir().unwrap();
        tup_platform::init::init_command(tmp.path(), false, false).unwrap();

        let db = TupDb::open(tmp.path(), false).unwrap();
        db.begin().unwrap();

        let cmd_id = db
            .node_insert(DOT_DT, "echo hi", NodeType::Cmd, -1, 0, -1, None, None)
            .unwrap();

        track_outputs(&db, cmd_id, DOT_DT, &[], tmp.path()).unwrap();

        // Command mtime should be updated to ~now
        let cmd = db.node_select_by_id(cmd_id).unwrap().unwrap();
        assert!(cmd.mtime > 0);

        db.commit().unwrap();
    }
}
