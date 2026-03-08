use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use tup_parser::{expand_globs, expand_output_pattern, expand_percent, InputFile, Rule};

/// Result of executing a single command.
#[derive(Debug)]
pub struct CommandResult {
    /// The command that was executed.
    pub command: String,
    /// Display string (if set in the rule).
    pub display: Option<String>,
    /// Whether the command succeeded.
    pub success: bool,
    /// Exit code (if available).
    pub exit_code: Option<i32>,
    /// Standard output.
    pub stdout: String,
    /// Standard error.
    pub stderr: String,
    /// Execution time in milliseconds.
    pub duration_ms: u64,
    /// Output files that were expected.
    pub expected_outputs: Vec<String>,
}

/// The build updater that executes commands.
pub struct Updater {
    /// Working directory for command execution.
    work_dir: PathBuf,
    /// Whether to keep going after errors.
    keep_going: bool,
    /// Number of commands executed.
    commands_run: usize,
    /// Number of commands that failed.
    commands_failed: usize,
    /// Total expected commands (set before execution for progress).
    total_expected: usize,
    /// Whether to show error output inline.
    show_errors: bool,
}

impl Updater {
    /// Create a new updater for the given working directory.
    pub fn new(work_dir: &Path) -> Self {
        Updater {
            work_dir: work_dir.to_path_buf(),
            keep_going: false,
            commands_run: 0,
            commands_failed: 0,
            total_expected: 0,
            show_errors: true,
        }
    }

    /// Set whether to continue after command failures.
    pub fn set_keep_going(&mut self, keep_going: bool) {
        self.keep_going = keep_going;
    }

    /// Get the number of commands executed.
    pub fn commands_run(&self) -> usize {
        self.commands_run
    }

    /// Get the number of commands that failed.
    pub fn commands_failed(&self) -> usize {
        self.commands_failed
    }

    /// Execute a single rule.
    ///
    /// For foreach rules, executes once per input file.
    /// For non-foreach rules, executes once with all inputs.
    pub fn execute_rule(&mut self, rule: &Rule) -> Result<Vec<CommandResult>, String> {
        let mut results = Vec::new();

        // Expand glob patterns in inputs
        let expanded_inputs = expand_globs(&rule.inputs, &self.work_dir)
            .map_err(|e| format!("glob expansion failed: {e}"))?;

        if rule.foreach {
            // Execute once per input
            for input_str in &expanded_inputs {
                let input = InputFile::new(input_str);

                // Expand output patterns for this input
                let outputs: Vec<String> = rule.outputs.iter()
                    .map(|pat| expand_output_pattern(pat, &input))
                    .collect();

                let cmd = expand_percent(
                    &rule.command.command,
                    &[input],
                    &outputs,
                    &rule.order_only_inputs,
                    &self.dir_name(),
                );

                let result = self.execute_command(&cmd, rule.command.display.as_deref(), &outputs)?;
                let failed = !result.success;
                results.push(result);

                if failed && !self.keep_going {
                    return Ok(results);
                }
            }
        } else {
            // Execute once with all inputs
            let inputs: Vec<InputFile> = expanded_inputs.iter()
                .map(|s| InputFile::new(s))
                .collect();

            let outputs: Vec<String> = if !inputs.is_empty() {
                rule.outputs.iter()
                    .map(|pat| {
                        if pat.contains('%') {
                            if let Some(first) = inputs.first() {
                                expand_output_pattern(pat, first)
                            } else {
                                pat.clone()
                            }
                        } else {
                            pat.clone()
                        }
                    })
                    .collect()
            } else {
                rule.outputs.clone()
            };

            let cmd = expand_percent(
                &rule.command.command,
                &inputs,
                &outputs,
                &rule.order_only_inputs,
                &self.dir_name(),
            );

            let result = self.execute_command(&cmd, rule.command.display.as_deref(), &outputs)?;
            results.push(result);
        }

        Ok(results)
    }

    /// Execute a shell command.
    fn execute_command(
        &mut self,
        cmd: &str,
        display: Option<&str>,
        expected_outputs: &[String],
    ) -> Result<CommandResult, String> {
        self.commands_run += 1;

        // Show what we're doing
        let total = if self.total_expected > 0 { self.total_expected } else { self.commands_run };
        if let Some(disp) = display {
            eprintln!(" [{}/{}] {}", self.commands_run, total, disp);
        } else {
            eprintln!(" [{}/{}] {}", self.commands_run, total, cmd);
        }

        let start = Instant::now();

        let shell = if cfg!(target_os = "windows") {
            "cmd"
        } else {
            "sh"
        };
        let shell_flag = if cfg!(target_os = "windows") {
            "/C"
        } else {
            "-c"
        };

        let output = Command::new(shell)
            .arg(shell_flag)
            .arg(cmd)
            .current_dir(&self.work_dir)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .map_err(|e| format!("failed to execute command: {e}"))?;

        let duration = start.elapsed();
        let success = output.status.success();

        if !success {
            self.commands_failed += 1;
            if self.show_errors {
                let stderr_text = String::from_utf8_lossy(&output.stderr);
                let stdout_text = String::from_utf8_lossy(&output.stdout);
                if !stderr_text.trim().is_empty() {
                    eprintln!("{}", stderr_text.trim());
                }
                if !stdout_text.trim().is_empty() {
                    eprintln!("{}", stdout_text.trim());
                }
                if let Some(code) = output.status.code() {
                    eprintln!(" *** Command failed with exit code {code}");
                } else {
                    eprintln!(" *** Command was killed by signal");
                }
            }
        }

        Ok(CommandResult {
            command: cmd.to_string(),
            display: display.map(String::from),
            success,
            exit_code: output.status.code(),
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
            duration_ms: duration.as_millis() as u64,
            expected_outputs: expected_outputs.to_vec(),
        })
    }

    /// Get the directory name (last component of work_dir).
    fn dir_name(&self) -> String {
        self.work_dir
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| ".".to_string())
    }

    /// Execute all rules from a parsed Tupfile.
    ///
    /// Returns all command results in execution order.
    pub fn execute_rules(&mut self, rules: &[Rule]) -> Result<Vec<CommandResult>, String> {
        self.total_expected = rules.len();
        let mut all_results = Vec::new();

        for rule in rules {
            let results = self.execute_rule(rule)?;
            let had_failure = results.iter().any(|r| !r.success);
            all_results.extend(results);

            if had_failure && !self.keep_going {
                break;
            }
        }

        Ok(all_results)
    }

    /// Check that all expected output files exist.
    pub fn verify_outputs(&self, results: &[CommandResult]) -> Vec<String> {
        let mut missing = Vec::new();
        for result in results {
            if !result.success {
                continue;
            }
            for output in &result.expected_outputs {
                let path = self.work_dir.join(output);
                if !path.exists() {
                    missing.push(format!(
                        "expected output '{}' was not created by: {}",
                        output, result.command
                    ));
                }
            }
        }
        missing
    }

    /// Execute independent commands in parallel using a thread pool.
    ///
    /// `num_jobs` controls the maximum number of concurrent commands.
    /// Each command result is collected in completion order.
    pub fn execute_rules_parallel(
        &mut self,
        rules: &[Rule],
        num_jobs: usize,
    ) -> Result<Vec<CommandResult>, String> {
        let num_jobs = num_jobs.max(1);

        // For single job, just use sequential execution
        if num_jobs == 1 {
            return self.execute_rules(rules);
        }

        // Expand all rules into individual commands first
        let mut commands: Vec<(String, Option<String>, Vec<String>)> = Vec::new();

        for rule in rules {
            let expanded_inputs = expand_globs(&rule.inputs, &self.work_dir)
                .map_err(|e| format!("glob expansion failed: {e}"))?;

            if rule.foreach {
                for input_str in &expanded_inputs {
                    let input = InputFile::new(input_str);
                    let outputs: Vec<String> = rule.outputs.iter()
                        .map(|pat| expand_output_pattern(pat, &input))
                        .collect();
                    let cmd = expand_percent(
                        &rule.command.command, &[input], &outputs,
                        &rule.order_only_inputs, &self.dir_name(),
                    );
                    commands.push((cmd, rule.command.display.clone(), outputs));
                }
            } else {
                let inputs: Vec<InputFile> = expanded_inputs.iter()
                    .map(|s| InputFile::new(s))
                    .collect();
                let outputs: Vec<String> = if !inputs.is_empty() {
                    rule.outputs.iter()
                        .map(|pat| {
                            if pat.contains('%') {
                                inputs.first().map(|f| expand_output_pattern(pat, f))
                                    .unwrap_or_else(|| pat.clone())
                            } else { pat.clone() }
                        }).collect()
                } else { rule.outputs.clone() };
                let cmd = expand_percent(
                    &rule.command.command, &inputs, &outputs,
                    &rule.order_only_inputs, &self.dir_name(),
                );
                commands.push((cmd, rule.command.display.clone(), outputs));
            }
        }

        let total = commands.len();
        let results = Arc::new(Mutex::new(Vec::with_capacity(total)));
        let work_dir = self.work_dir.clone();
        let counter = Arc::new(Mutex::new(0usize));

        std::thread::scope(|s| {
            let mut handles = Vec::new();
            let chunks: Vec<_> = commands.chunks(
                commands.len().div_ceil(num_jobs)
            ).collect();

            for chunk in chunks {
                let results = Arc::clone(&results);
                let counter = Arc::clone(&counter);
                let work_dir = work_dir.clone();
                let chunk: Vec<_> = chunk.to_vec();

                handles.push(s.spawn(move || {
                    for (cmd, display, outputs) in chunk {
                        let mut num = counter.lock().unwrap();
                        *num += 1;
                        let n = *num;
                        drop(num);

                        if let Some(ref d) = display {
                            eprintln!(" [{n}/{total}] {d}");
                        } else {
                            eprintln!(" [{n}/{total}] {cmd}");
                        }

                        let shell = if cfg!(target_os = "windows") { "cmd" } else { "sh" };
                        let flag = if cfg!(target_os = "windows") { "/C" } else { "-c" };
                        let start = Instant::now();

                        let output = Command::new(shell)
                            .arg(flag).arg(&cmd)
                            .current_dir(&work_dir)
                            .stdout(Stdio::piped())
                            .stderr(Stdio::piped())
                            .output();

                        let duration = start.elapsed();

                        let result = match output {
                            Ok(o) => CommandResult {
                                command: cmd,
                                display,
                                success: o.status.success(),
                                exit_code: o.status.code(),
                                stdout: String::from_utf8_lossy(&o.stdout).to_string(),
                                stderr: String::from_utf8_lossy(&o.stderr).to_string(),
                                duration_ms: duration.as_millis() as u64,
                                expected_outputs: outputs,
                            },
                            Err(e) => CommandResult {
                                command: cmd,
                                display,
                                success: false,
                                exit_code: None,
                                stdout: String::new(),
                                stderr: e.to_string(),
                                duration_ms: duration.as_millis() as u64,
                                expected_outputs: outputs,
                            },
                        };

                        results.lock().unwrap().push(result);
                    }
                }));
            }

            for h in handles {
                h.join().unwrap();
            }
        });

        let final_results = Arc::try_unwrap(results).unwrap().into_inner().unwrap();
        self.commands_run = final_results.len();
        self.commands_failed = final_results.iter().filter(|r| !r.success).count();
        Ok(final_results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_rule(inputs: &[&str], cmd: &str, outputs: &[&str]) -> Rule {
        Rule {
            foreach: false,
            inputs: inputs.iter().map(|s| s.to_string()).collect(),
            order_only_inputs: vec![],
            command: tup_parser::RuleCommand {
                display: None,
                flags: None,
                command: cmd.to_string(),
            },
            outputs: outputs.iter().map(|s| s.to_string()).collect(),
            extra_outputs: vec![],
            line_number: 1,
        }
    }

    fn make_foreach_rule(inputs: &[&str], cmd: &str, output_pattern: &str) -> Rule {
        Rule {
            foreach: true,
            inputs: inputs.iter().map(|s| s.to_string()).collect(),
            order_only_inputs: vec![],
            command: tup_parser::RuleCommand {
                display: None,
                flags: None,
                command: cmd.to_string(),
            },
            outputs: vec![output_pattern.to_string()],
            extra_outputs: vec![],
            line_number: 1,
        }
    }

    #[test]
    fn test_execute_simple_command() {
        let tmp = tempfile::tempdir().unwrap();
        let mut updater = Updater::new(tmp.path());

        let rule = make_rule(&[], "echo hello > output.txt", &["output.txt"]);
        let results = updater.execute_rule(&rule).unwrap();

        assert_eq!(results.len(), 1);
        assert!(results[0].success);
        assert!(tmp.path().join("output.txt").exists());
    }

    #[test]
    fn test_execute_with_inputs() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("input.txt"), "hello world").unwrap();

        let mut updater = Updater::new(tmp.path());
        let rule = make_rule(&["input.txt"], "cp %f %o", &["output.txt"]);
        let results = updater.execute_rule(&rule).unwrap();

        assert!(results[0].success);
        assert!(tmp.path().join("output.txt").exists());
        let content = std::fs::read_to_string(tmp.path().join("output.txt")).unwrap();
        assert_eq!(content, "hello world");
    }

    #[test]
    fn test_execute_failing_command() {
        let tmp = tempfile::tempdir().unwrap();
        let mut updater = Updater::new(tmp.path());

        let rule = make_rule(&[], "false", &[]);
        let results = updater.execute_rule(&rule).unwrap();

        assert!(!results[0].success);
        assert_eq!(updater.commands_failed(), 1);
    }

    #[test]
    fn test_execute_foreach() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("a.txt"), "aaa").unwrap();
        std::fs::write(tmp.path().join("b.txt"), "bbb").unwrap();

        let mut updater = Updater::new(tmp.path());
        let rule = make_foreach_rule(&["a.txt", "b.txt"], "cp %f %o", "%B.out");
        let results = updater.execute_rule(&rule).unwrap();

        assert_eq!(results.len(), 2);
        assert!(results[0].success);
        assert!(results[1].success);
        assert!(tmp.path().join("a.out").exists());
        assert!(tmp.path().join("b.out").exists());
    }

    #[test]
    fn test_execute_captures_output() {
        let tmp = tempfile::tempdir().unwrap();
        let mut updater = Updater::new(tmp.path());

        let rule = make_rule(&[], "echo hello_stdout", &[]);
        let results = updater.execute_rule(&rule).unwrap();

        assert!(results[0].stdout.contains("hello_stdout"));
    }

    #[test]
    fn test_execute_captures_stderr() {
        let tmp = tempfile::tempdir().unwrap();
        let mut updater = Updater::new(tmp.path());

        let rule = make_rule(&[], "echo hello_stderr >&2", &[]);
        let results = updater.execute_rule(&rule).unwrap();

        assert!(results[0].stderr.contains("hello_stderr"));
    }

    #[test]
    fn test_execute_rules_stops_on_failure() {
        let tmp = tempfile::tempdir().unwrap();
        let mut updater = Updater::new(tmp.path());

        let rules = vec![
            make_rule(&[], "echo first > first.txt", &["first.txt"]),
            make_rule(&[], "false", &[]),
            make_rule(&[], "echo third > third.txt", &["third.txt"]),
        ];

        let results = updater.execute_rules(&rules).unwrap();
        assert_eq!(results.len(), 2); // Stopped after failure
        assert!(results[0].success);
        assert!(!results[1].success);
        assert!(!tmp.path().join("third.txt").exists());
    }

    #[test]
    fn test_execute_rules_keep_going() {
        let tmp = tempfile::tempdir().unwrap();
        let mut updater = Updater::new(tmp.path());
        updater.set_keep_going(true);

        let rules = vec![
            make_rule(&[], "echo first > first.txt", &["first.txt"]),
            make_rule(&[], "false", &[]),
            make_rule(&[], "echo third > third.txt", &["third.txt"]),
        ];

        let results = updater.execute_rules(&rules).unwrap();
        assert_eq!(results.len(), 3); // Continued after failure
        assert!(results[0].success);
        assert!(!results[1].success);
        assert!(results[2].success);
        assert!(tmp.path().join("third.txt").exists());
    }

    #[test]
    fn test_verify_outputs_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let updater = Updater::new(tmp.path());

        let results = vec![CommandResult {
            command: "gcc -c foo.c -o foo.o".to_string(),
            display: None,
            success: true,
            exit_code: Some(0),
            stdout: String::new(),
            stderr: String::new(),
            duration_ms: 10,
            expected_outputs: vec!["foo.o".to_string()],
        }];

        let missing = updater.verify_outputs(&results);
        assert_eq!(missing.len(), 1);
        assert!(missing[0].contains("foo.o"));
    }

    #[test]
    fn test_verify_outputs_present() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("foo.o"), "").unwrap();
        let updater = Updater::new(tmp.path());

        let results = vec![CommandResult {
            command: "gcc -c foo.c -o foo.o".to_string(),
            display: None,
            success: true,
            exit_code: Some(0),
            stdout: String::new(),
            stderr: String::new(),
            duration_ms: 10,
            expected_outputs: vec!["foo.o".to_string()],
        }];

        let missing = updater.verify_outputs(&results);
        assert!(missing.is_empty());
    }

    #[test]
    fn test_command_result_duration() {
        let tmp = tempfile::tempdir().unwrap();
        let mut updater = Updater::new(tmp.path());

        let rule = make_rule(&[], "true", &[]);
        let results = updater.execute_rule(&rule).unwrap();

        // Duration should be reasonable (less than 5 seconds)
        assert!(results[0].duration_ms < 5000);
    }

    #[test]
    fn test_display_string() {
        let tmp = tempfile::tempdir().unwrap();
        let mut updater = Updater::new(tmp.path());

        let rule = Rule {
            foreach: false,
            inputs: vec![],
            order_only_inputs: vec![],
            command: tup_parser::RuleCommand {
                display: Some("CC foo.c".to_string()),
                flags: None,
                command: "echo compiling > /dev/null".to_string(),
            },
            outputs: vec![],
            extra_outputs: vec![],
            line_number: 1,
        };

        let results = updater.execute_rule(&rule).unwrap();
        assert_eq!(results[0].display, Some("CC foo.c".to_string()));
    }

    #[test]
    fn test_commands_run_counter() {
        let tmp = tempfile::tempdir().unwrap();
        let mut updater = Updater::new(tmp.path());

        assert_eq!(updater.commands_run(), 0);
        let rules = vec![
            make_rule(&[], "true", &[]),
            make_rule(&[], "true", &[]),
        ];
        updater.execute_rules(&rules).unwrap();
        assert_eq!(updater.commands_run(), 2);
        assert_eq!(updater.commands_failed(), 0);
    }

    #[test]
    fn test_glob_expansion_foreach() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("a.txt"), "aaa").unwrap();
        std::fs::write(tmp.path().join("b.txt"), "bbb").unwrap();
        std::fs::write(tmp.path().join("c.dat"), "ccc").unwrap();

        let mut updater = Updater::new(tmp.path());
        let rule = make_foreach_rule(&["*.txt"], "cp %f %o", "%B.out");
        let results = updater.execute_rule(&rule).unwrap();

        // Should only match .txt files, not .dat
        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|r| r.success));
        assert!(tmp.path().join("a.out").exists());
        assert!(tmp.path().join("b.out").exists());
        assert!(!tmp.path().join("c.out").exists());
    }

    #[test]
    fn test_glob_expansion_non_foreach() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("x.c"), "").unwrap();
        std::fs::write(tmp.path().join("y.c"), "").unwrap();

        let mut updater = Updater::new(tmp.path());
        let rule = make_rule(&["*.c"], "echo %f > %o", &["files.txt"]);
        let results = updater.execute_rule(&rule).unwrap();

        assert_eq!(results.len(), 1);
        assert!(results[0].success);
        let content = std::fs::read_to_string(tmp.path().join("files.txt")).unwrap();
        assert!(content.contains("x.c"));
        assert!(content.contains("y.c"));
    }

    #[test]
    fn test_parallel_execution() {
        let tmp = tempfile::tempdir().unwrap();
        let mut updater = Updater::new(tmp.path());

        let rules = vec![
            make_rule(&[], "echo 1 > one.txt", &["one.txt"]),
            make_rule(&[], "echo 2 > two.txt", &["two.txt"]),
            make_rule(&[], "echo 3 > three.txt", &["three.txt"]),
            make_rule(&[], "echo 4 > four.txt", &["four.txt"]),
        ];

        let results = updater.execute_rules_parallel(&rules, 4).unwrap();
        assert_eq!(results.len(), 4);
        assert!(results.iter().all(|r| r.success));
        assert!(tmp.path().join("one.txt").exists());
        assert!(tmp.path().join("two.txt").exists());
        assert!(tmp.path().join("three.txt").exists());
        assert!(tmp.path().join("four.txt").exists());
    }

    #[test]
    fn test_parallel_single_job_fallback() {
        let tmp = tempfile::tempdir().unwrap();
        let mut updater = Updater::new(tmp.path());

        let rules = vec![
            make_rule(&[], "echo ok > out.txt", &["out.txt"]),
        ];

        let results = updater.execute_rules_parallel(&rules, 1).unwrap();
        assert_eq!(results.len(), 1);
        assert!(results[0].success);
    }

    #[test]
    fn test_parallel_with_failures() {
        let tmp = tempfile::tempdir().unwrap();
        let mut updater = Updater::new(tmp.path());
        updater.set_keep_going(true);

        let rules = vec![
            make_rule(&[], "true", &[]),
            make_rule(&[], "false", &[]),
            make_rule(&[], "true", &[]),
        ];

        let results = updater.execute_rules_parallel(&rules, 2).unwrap();
        assert_eq!(results.len(), 3);
        assert_eq!(updater.commands_failed(), 1);
    }
}
