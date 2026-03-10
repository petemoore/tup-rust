use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use tup_parser::{expand_globs, expand_output_pattern, expand_percent, InputFile, Rule};

/// An expanded command ready for execution, with its inputs and outputs tracked.
struct ExpandedCommand {
    cmd: String,
    display: Option<String>,
    inputs: Vec<String>,
    outputs: Vec<String>,
}

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
    /// FUSE job path (`.tup/mnt/@tupjob-N`) and dir path (relative tup_top + subdir).
    /// When set, child does chdir(job) then chdir(dir) to enter FUSE mount.
    /// Port of C tup's exec_internal (master_fork.c:524-536).
    #[cfg(unix)]
    fuse_paths: Option<(PathBuf, PathBuf)>,
    /// Optional master_fork handle for FUSE command execution.
    /// When set, commands are executed through the master_fork child process
    /// (which was forked before the FUSE mount), ensuring correct CWD handling.
    /// Port of C tup's master_fork_exec() (master_fork.c:306-349).
    #[cfg(unix)]
    master_fork: Option<Arc<tup_server::MasterFork>>,
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
            #[cfg(unix)]
            fuse_paths: None,
            #[cfg(unix)]
            master_fork: None,
        }
    }

    /// Set the FUSE job and dir paths for command execution.
    /// Port of C tup's exec_internal: chdir(job) then chdir(dir).
    #[cfg(unix)]
    pub fn set_fuse_job_dir(&mut self, job: PathBuf, dir: PathBuf) {
        self.fuse_paths = Some((job, dir));
    }

    /// Set the master_fork handle for FUSE command execution.
    /// When set, commands are routed through the pre-forked child process.
    /// Port of C tup's master_fork_exec() pattern.
    #[cfg(unix)]
    pub fn set_master_fork(&mut self, mf: Arc<tup_server::MasterFork>) {
        self.master_fork = Some(mf);
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
                let outputs: Vec<String> = rule
                    .outputs
                    .iter()
                    .map(|pat| expand_output_pattern(pat, &input))
                    .collect::<Result<Vec<_>, _>>()?;

                let cmd = expand_percent(
                    &rule.command.command,
                    &[input],
                    &outputs,
                    &rule.order_only_inputs,
                    &self.dir_name(),
                )?;

                let result =
                    self.execute_command(&cmd, rule.command.display.as_deref(), &outputs)?;
                let failed = !result.success;
                results.push(result);

                if failed && !self.keep_going {
                    return Ok(results);
                }
            }
        } else {
            // Execute once with all inputs
            let inputs: Vec<InputFile> =
                expanded_inputs.iter().map(|s| InputFile::new(s)).collect();

            let outputs: Vec<String> = if !inputs.is_empty() {
                rule.outputs
                    .iter()
                    .map(|pat| {
                        if pat.contains('%') {
                            if let Some(first) = inputs.first() {
                                expand_output_pattern(pat, first)
                            } else {
                                Ok(pat.clone())
                            }
                        } else {
                            Ok(pat.clone())
                        }
                    })
                    .collect::<Result<Vec<_>, _>>()?
            } else {
                rule.outputs.clone()
            };

            let cmd = expand_percent(
                &rule.command.command,
                &inputs,
                &outputs,
                &rule.order_only_inputs,
                &self.dir_name(),
            )?;

            let result = self.execute_command(&cmd, rule.command.display.as_deref(), &outputs)?;
            results.push(result);
        }

        Ok(results)
    }

    /// Execute a shell command.
    ///
    /// When `master_fork` is set and `fuse_paths` are configured, commands
    /// are routed through the pre-forked master_fork child process. This
    /// ensures the child was forked before the FUSE mount, so chdir() into
    /// the FUSE mount works correctly on macOS.
    ///
    /// Port of C tup's master_fork_exec() + exec_internal() flow.
    fn execute_command(
        &mut self,
        cmd: &str,
        display: Option<&str>,
        expected_outputs: &[String],
    ) -> Result<CommandResult, String> {
        self.commands_run += 1;

        // Show what we're doing
        let total = if self.total_expected > 0 {
            self.total_expected
        } else {
            self.commands_run
        };
        if let Some(disp) = display {
            println!(" [{}/{}] {}", self.commands_run, total, disp);
        } else {
            println!(" [{}/{}] {}", self.commands_run, total, cmd);
        }

        let start = Instant::now();

        // Try master_fork path first (preferred for FUSE on macOS).
        // C tup: master_fork_exec() sends command over socket to pre-forked child,
        // which does fork+chdir(job)+chdir(dir)+exec. This is the correct path
        // because the master_fork child was created before the FUSE mount.
        #[cfg(unix)]
        if let (Some(ref mf), Some((ref job, ref dir))) = (&self.master_fork, &self.fuse_paths) {
            let env = tup_server::master_fork::build_env_block();
            let sid = self.commands_run as i64;
            let job_str = job.to_string_lossy();
            let dir_str = dir.to_string_lossy();

            let exit_code = mf
                .exec(sid, &job_str, &dir_str, cmd, &env, false)
                .map_err(|e| format!("master_fork exec failed: {e}"))?;

            let duration = start.elapsed();
            let success = exit_code == 0;

            // Note: with master_fork, stdout/stderr go to the terminal directly
            // (the grandchild inherits the master_fork child's stdout/stderr).
            // C tup redirects to .tup/tmp/output-N files; we'll add that later.
            if !success {
                self.commands_failed += 1;
                eprintln!(" *** Command failed with exit code {exit_code}");
            }

            return Ok(CommandResult {
                command: cmd.to_string(),
                display: display.map(String::from),
                success,
                exit_code: Some(exit_code),
                stdout: String::new(),
                stderr: String::new(),
                duration_ms: duration.as_millis() as u64,
                expected_outputs: expected_outputs.to_vec(),
            });
        }

        // Fallback: use Command::new with optional pre_exec for FUSE paths
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

        let mut command = Command::new(shell);
        command
            .arg(shell_flag)
            .arg(cmd)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        // C tup: child does chdir(job) then chdir(dir) (master_fork.c:524-536).
        // CRITICAL: We must use pre_exec with libc::chdir(), NOT Command::current_dir().
        // Rust's current_dir() resolves the path in the parent before fork, which
        // causes the macOS kernel to see the real path instead of the FUSE path.
        // By doing chdir() after fork (in pre_exec), the child process enters the
        // FUSE mount and all subsequent file operations go through FUSE.
        //
        // We also set the child's process group to match ours so that
        // context_check() in the FUSE filesystem allows access.
        // C tup: master_fork does this implicitly via the persistent forked process.
        #[cfg(unix)]
        if let Some((ref job, ref dir)) = self.fuse_paths {
            use std::os::unix::process::CommandExt;
            let job_path = job.clone();
            let dir_path = dir.clone();
            let parent_pgid = unsafe { libc::getpgid(0) };
            unsafe {
                command.pre_exec(move || {
                    // Set child's process group to match parent's, so FUSE
                    // context_check() allows access (fuse_fs.c:243-281).
                    libc::setpgid(0, parent_pgid);

                    // C: chdir(job) — relative path ".tup/mnt/@tupjob-N"
                    // Must be relative so macOS kernel routes through FUSE.
                    let c_job = std::ffi::CString::new(job_path.to_string_lossy().as_bytes())
                        .map_err(|_| std::io::Error::from_raw_os_error(libc::EINVAL))?;
                    if libc::chdir(c_job.as_ptr()) < 0 {
                        return Err(std::io::Error::last_os_error());
                    }

                    // C: chdir(dir) — tup_top-relative path e.g. "Users/.../project/src"
                    let c_dir = std::ffi::CString::new(dir_path.to_string_lossy().as_bytes())
                        .map_err(|_| std::io::Error::from_raw_os_error(libc::EINVAL))?;
                    if libc::chdir(c_dir.as_ptr()) < 0 {
                        return Err(std::io::Error::last_os_error());
                    }

                    Ok(())
                });
            }
        } else {
            command.current_dir(&self.work_dir);
        }
        #[cfg(not(unix))]
        command.current_dir(&self.work_dir);

        let output = command
            .output()
            .map_err(|e| format!("failed to execute command: {e}"))?;

        let duration = start.elapsed();
        let success = output.status.success();

        // Show command output (matches C tup behavior: output shown inline)
        let stdout_text = String::from_utf8_lossy(&output.stdout);
        let stderr_text = String::from_utf8_lossy(&output.stderr);
        if !stdout_text.trim().is_empty() {
            print!("{}", stdout_text);
        }
        if !stderr_text.trim().is_empty() {
            eprint!("{}", stderr_text);
        }

        if !success {
            self.commands_failed += 1;
            if let Some(code) = output.status.code() {
                eprintln!(" *** Command failed with exit code {code}");
            } else {
                eprintln!(" *** Command was killed by signal");
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

    /// Execute pre-expanded rules where commands are already fully substituted.
    ///
    /// Unlike `execute_rules`, this does NOT call expand_percent on the commands.
    /// Use this when rules have already been expanded by expand_rules_for_dir.
    pub fn execute_expanded_rules(&mut self, rules: &[Rule]) -> Result<Vec<CommandResult>, String> {
        self.total_expected = rules.len();
        let mut all_results = Vec::new();

        for rule in rules {
            let result = self.execute_command(
                &rule.command.command,
                rule.command.display.as_deref(),
                &rule.outputs,
            )?;
            let failed = !result.success;
            all_results.push(result);

            if failed && !self.keep_going {
                break;
            }
        }

        Ok(all_results)
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

    /// Execute commands in parallel with dependency-aware scheduling.
    ///
    /// Commands are organized into waves based on producer→consumer
    /// relationships (output of one rule used as input to another).
    /// Commands within a wave run in parallel, waves execute sequentially.
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

        // Expand all rules into individual commands with their inputs
        let mut commands: Vec<ExpandedCommand> = Vec::new();

        for rule in rules {
            let expanded_inputs = expand_globs(&rule.inputs, &self.work_dir)
                .map_err(|e| format!("glob expansion failed: {e}"))?;

            if rule.foreach {
                for input_str in &expanded_inputs {
                    let input = InputFile::new(input_str);
                    let outputs: Vec<String> = rule
                        .outputs
                        .iter()
                        .map(|pat| expand_output_pattern(pat, &input))
                        .collect::<Result<Vec<_>, _>>()?;
                    let cmd = expand_percent(
                        &rule.command.command,
                        &[input],
                        &outputs,
                        &rule.order_only_inputs,
                        &self.dir_name(),
                    )?;
                    commands.push(ExpandedCommand {
                        cmd,
                        display: rule.command.display.clone(),
                        inputs: vec![input_str.clone()],
                        outputs,
                    });
                }
            } else {
                let inputs: Vec<InputFile> =
                    expanded_inputs.iter().map(|s| InputFile::new(s)).collect();
                let outputs: Vec<String> = if !inputs.is_empty() {
                    rule.outputs
                        .iter()
                        .map(|pat| {
                            if pat.contains('%') {
                                inputs
                                    .first()
                                    .map(|f| expand_output_pattern(pat, f))
                                    .unwrap_or_else(|| Ok(pat.clone()))
                            } else {
                                Ok(pat.clone())
                            }
                        })
                        .collect::<Result<Vec<_>, _>>()?
                } else {
                    rule.outputs.clone()
                };
                let cmd = expand_percent(
                    &rule.command.command,
                    &inputs,
                    &outputs,
                    &rule.order_only_inputs,
                    &self.dir_name(),
                )?;
                commands.push(ExpandedCommand {
                    cmd,
                    display: rule.command.display.clone(),
                    inputs: expanded_inputs,
                    outputs,
                });
            }
        }

        // Build dependency graph: for each command, find which other commands
        // produce its inputs. A command depends on all producers of its inputs.
        let total = commands.len();

        // Map output name → index of command that produces it
        let mut output_producers: std::collections::HashMap<String, usize> =
            std::collections::HashMap::new();
        for (idx, ec) in commands.iter().enumerate() {
            for out in &ec.outputs {
                output_producers.insert(out.clone(), idx);
            }
        }

        // Compute dependencies for each command
        let mut deps: Vec<Vec<usize>> = vec![Vec::new(); total];
        for (idx, ec) in commands.iter().enumerate() {
            for inp in &ec.inputs {
                if let Some(&producer) = output_producers.get(inp) {
                    if producer != idx {
                        deps[idx].push(producer);
                    }
                }
            }
        }

        // Execute in waves: each wave contains commands whose deps are all done
        let mut done = vec![false; total];
        let mut all_results = Vec::with_capacity(total);
        self.total_expected = total;

        while all_results.len() < total {
            // Find ready commands (all deps satisfied)
            let ready: Vec<usize> = (0..total)
                .filter(|&i| !done[i] && deps[i].iter().all(|&d| done[d]))
                .collect();

            if ready.is_empty() {
                return Err("dependency cycle detected in rules".to_string());
            }

            // Execute this wave in parallel
            let wave_commands: Vec<(usize, &ExpandedCommand)> =
                ready.iter().map(|&i| (i, &commands[i])).collect();

            let results = Arc::new(Mutex::new(Vec::new()));
            let work_dir = self.work_dir.clone();
            let counter = Arc::new(Mutex::new(self.commands_run));

            std::thread::scope(|s| {
                let mut handles = Vec::new();
                let chunk_size = wave_commands.len().div_ceil(num_jobs);
                let chunks: Vec<_> = wave_commands.chunks(chunk_size).collect();

                for chunk in chunks {
                    let results = Arc::clone(&results);
                    let counter = Arc::clone(&counter);
                    let work_dir = work_dir.clone();
                    let chunk: Vec<_> = chunk.to_vec();

                    handles.push(s.spawn(move || {
                        for (idx, ec) in chunk {
                            let mut num = counter.lock().unwrap();
                            *num += 1;
                            let n = *num;
                            drop(num);

                            if let Some(ref d) = ec.display {
                                println!(" [{n}/{total}] {d}");
                            } else {
                                println!(" [{n}/{total}] {}", ec.cmd);
                            }

                            let shell = if cfg!(target_os = "windows") {
                                "cmd"
                            } else {
                                "sh"
                            };
                            let flag = if cfg!(target_os = "windows") {
                                "/C"
                            } else {
                                "-c"
                            };
                            let start = Instant::now();

                            let output = Command::new(shell)
                                .arg(flag)
                                .arg(&ec.cmd)
                                .current_dir(&work_dir)
                                .stdout(Stdio::piped())
                                .stderr(Stdio::piped())
                                .output();

                            let duration = start.elapsed();

                            let result = parallel_command_result(&output, ec, duration);
                            results.lock().unwrap().push((idx, result));
                        }
                    }));
                }

                for h in handles {
                    h.join().unwrap();
                }
            });

            let wave_results = Arc::try_unwrap(results).unwrap().into_inner().unwrap();
            self.commands_run += wave_results.len();
            let wave_failed = wave_results.iter().filter(|(_, r)| !r.success).count();
            self.commands_failed += wave_failed;

            for (idx, result) in wave_results {
                done[idx] = true;
                all_results.push(result);
            }

            if wave_failed > 0 && !self.keep_going {
                break;
            }
        }

        Ok(all_results)
    }

    /// Execute pre-expanded rules in parallel.
    ///
    /// Like `execute_rules_parallel` but skips all % expansion and glob
    /// resolution since rules are already fully expanded by the caller.
    pub fn execute_expanded_rules_parallel(
        &mut self,
        rules: &[Rule],
        num_jobs: usize,
    ) -> Result<Vec<CommandResult>, String> {
        let num_jobs = num_jobs.max(1);

        if num_jobs == 1 {
            return self.execute_expanded_rules(rules);
        }

        // Convert rules directly to ExpandedCommands without expansion
        let commands: Vec<ExpandedCommand> = rules
            .iter()
            .map(|rule| ExpandedCommand {
                cmd: rule.command.command.clone(),
                display: rule.command.display.clone(),
                inputs: rule.inputs.clone(),
                outputs: rule.outputs.clone(),
            })
            .collect();

        self.execute_commands_parallel(commands, num_jobs)
    }

    /// Internal: execute a list of pre-expanded commands in parallel with dependency ordering.
    fn execute_commands_parallel(
        &mut self,
        commands: Vec<ExpandedCommand>,
        num_jobs: usize,
    ) -> Result<Vec<CommandResult>, String> {
        let total = commands.len();

        // Map output name → index of command that produces it
        let mut output_producers: std::collections::HashMap<String, usize> =
            std::collections::HashMap::new();
        for (idx, ec) in commands.iter().enumerate() {
            for out in &ec.outputs {
                output_producers.insert(out.clone(), idx);
            }
        }

        // Compute dependencies
        let mut deps: Vec<Vec<usize>> = vec![Vec::new(); total];
        for (idx, ec) in commands.iter().enumerate() {
            for inp in &ec.inputs {
                if let Some(&producer) = output_producers.get(inp) {
                    if producer != idx {
                        deps[idx].push(producer);
                    }
                }
            }
        }

        // Execute in waves
        let mut done = vec![false; total];
        let mut all_results = Vec::with_capacity(total);
        self.total_expected = total;

        while all_results.len() < total {
            let ready: Vec<usize> = (0..total)
                .filter(|&i| !done[i] && deps[i].iter().all(|&d| done[d]))
                .collect();

            if ready.is_empty() {
                return Err("dependency cycle detected in rules".to_string());
            }

            let wave_commands: Vec<(usize, &ExpandedCommand)> =
                ready.iter().map(|&i| (i, &commands[i])).collect();

            let results = Arc::new(Mutex::new(Vec::new()));
            let work_dir = self.work_dir.clone();
            let counter = Arc::new(Mutex::new(self.commands_run));

            std::thread::scope(|s| {
                let mut handles = Vec::new();
                let chunk_size = wave_commands.len().div_ceil(num_jobs);
                let chunks: Vec<_> = wave_commands.chunks(chunk_size).collect();

                for chunk in chunks {
                    let results = Arc::clone(&results);
                    let counter = Arc::clone(&counter);
                    let work_dir = work_dir.clone();
                    let chunk: Vec<_> = chunk.to_vec();

                    handles.push(s.spawn(move || {
                        for (idx, ec) in chunk {
                            let mut num = counter.lock().unwrap();
                            *num += 1;
                            let n = *num;
                            drop(num);

                            if let Some(ref d) = ec.display {
                                println!(" [{n}/{total}] {d}");
                            } else {
                                println!(" [{n}/{total}] {}", ec.cmd);
                            }

                            let shell = if cfg!(target_os = "windows") {
                                "cmd"
                            } else {
                                "sh"
                            };
                            let flag = if cfg!(target_os = "windows") {
                                "/C"
                            } else {
                                "-c"
                            };
                            let start = Instant::now();

                            let output = Command::new(shell)
                                .arg(flag)
                                .arg(&ec.cmd)
                                .current_dir(&work_dir)
                                .stdout(Stdio::piped())
                                .stderr(Stdio::piped())
                                .output();

                            let duration = start.elapsed();

                            let result = parallel_command_result(&output, ec, duration);
                            results.lock().unwrap().push((idx, result));
                        }
                    }));
                }

                for h in handles {
                    h.join().unwrap();
                }
            });

            let wave_results = Arc::try_unwrap(results).unwrap().into_inner().unwrap();
            self.commands_run += wave_results.len();
            let wave_failed = wave_results.iter().filter(|(_, r)| !r.success).count();
            self.commands_failed += wave_failed;

            for (idx, result) in wave_results {
                done[idx] = true;
                all_results.push(result);
            }

            if wave_failed > 0 && !self.keep_going {
                break;
            }
        }

        Ok(all_results)
    }
}

/// Build a CommandResult from a parallel command execution, showing output inline.
fn parallel_command_result(
    output: &Result<std::process::Output, std::io::Error>,
    ec: &ExpandedCommand,
    duration: std::time::Duration,
) -> CommandResult {
    match output {
        Ok(o) => {
            let stdout_text = String::from_utf8_lossy(&o.stdout);
            let stderr_text = String::from_utf8_lossy(&o.stderr);
            if !stdout_text.trim().is_empty() {
                print!("{}", stdout_text);
            }
            if !stderr_text.trim().is_empty() {
                eprint!("{}", stderr_text);
            }
            if !o.status.success() {
                if let Some(code) = o.status.code() {
                    eprintln!(" *** Command failed with exit code {code}");
                }
            }
            CommandResult {
                command: ec.cmd.clone(),
                display: ec.display.clone(),
                success: o.status.success(),
                exit_code: o.status.code(),
                stdout: stdout_text.to_string(),
                stderr: stderr_text.to_string(),
                duration_ms: duration.as_millis() as u64,
                expected_outputs: ec.outputs.clone(),
            }
        }
        Err(e) => CommandResult {
            command: ec.cmd.clone(),
            display: ec.display.clone(),
            success: false,
            exit_code: None,
            stdout: String::new(),
            stderr: e.to_string(),
            duration_ms: duration.as_millis() as u64,
            expected_outputs: ec.outputs.clone(),
        },
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
            had_inputs: !inputs.is_empty(),
            vars_snapshot: None,
            bin: None,
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
            had_inputs: !inputs.is_empty(),
            vars_snapshot: None,
            bin: None,
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
            had_inputs: false,
            vars_snapshot: None,
            bin: None,
        };

        let results = updater.execute_rule(&rule).unwrap();
        assert_eq!(results[0].display, Some("CC foo.c".to_string()));
    }

    #[test]
    fn test_commands_run_counter() {
        let tmp = tempfile::tempdir().unwrap();
        let mut updater = Updater::new(tmp.path());

        assert_eq!(updater.commands_run(), 0);
        let rules = vec![make_rule(&[], "true", &[]), make_rule(&[], "true", &[])];
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

        let rules = vec![make_rule(&[], "echo ok > out.txt", &["out.txt"])];

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
