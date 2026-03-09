use crate::bang::BangDb;
use crate::errors::ParseError;
use crate::rule::Rule;
use crate::vardb::ParseVarDb;

/// A parsed line from a Tupfile.
#[derive(Debug, Clone)]
pub enum TupfileLine {
    /// Empty line or comment.
    Empty,
    /// Variable assignment: `VAR = value`
    VarAssign { name: String, value: String },
    /// Variable append: `VAR += value`
    VarAppend { name: String, value: String },
    /// Rule: `: [foreach] inputs |> command |> outputs`
    Rule(Rule),
    /// Bang macro definition: `!name = ...`
    BangDef { name: String, definition: String },
    /// Include directive: `include FILE`
    Include(String),
    /// Include rules: `include_rules`
    IncludeRules,
    /// Export environment variable: `export VAR`
    Export(String),
    /// Import environment variable: `import VAR[=default]`
    Import(String),
    /// Preload directive: `preload DIR`
    Preload(String),
    /// Run script: `run SCRIPT`
    Run(String),
    /// Error directive: `error MESSAGE`
    Error(String),
    /// Gitignore request: `.gitignore`
    GitIgnore,
    /// Ifdef: `ifdef VAR`
    Ifdef(String),
    /// Ifndef: `ifndef VAR`
    Ifndef(String),
    /// Ifeq: `ifeq (A,B)`
    Ifeq(String, String),
    /// Ifneq: `ifneq (A,B)`
    Ifneq(String, String),
    /// Else
    Else,
    /// Endif
    Endif,
    /// Unknown/unrecognized line (error if in active branch, ignored in inactive)
    Unknown(String),
}

/// Reader that parses a Tupfile into lines.
pub struct TupfileReader {
    vars: ParseVarDb,
    bangs: BangDb,
    lines: Vec<ParsedLine>,
    /// Whether `.gitignore` directive was found during evaluation.
    gitignore_requested: bool,
}

#[derive(Clone)]
struct ParsedLine {
    content: TupfileLine,
    line_number: usize,
}

impl TupfileReader {
    /// Parse a Tupfile from its content string.
    pub fn parse(content: &str, filename: &str) -> Result<Self, ParseError> {
        let mut reader = TupfileReader {
            vars: ParseVarDb::new(),
            gitignore_requested: false,
            bangs: BangDb::new(),
            lines: Vec::new(),
        };

        let joined = join_continuation_lines(content);
        let mut line_number = 0;

        for raw_line in joined.lines() {
            line_number += 1;
            let line = raw_line.trim();

            // Skip empty lines and comments
            if line.is_empty() || line.starts_with('#') {
                continue;
            }

            let parsed = parse_line(line, line_number, filename)?;
            reader.lines.push(ParsedLine {
                content: parsed,
                line_number,
            });
        }

        Ok(reader)
    }

    /// Get the parsed variable database.
    pub fn vars(&self) -> &ParseVarDb {
        &self.vars
    }

    /// Get a mutable reference to the variable database.
    pub fn vars_mut(&mut self) -> &mut ParseVarDb {
        &mut self.vars
    }

    /// Get the bang macro database.
    pub fn bangs(&self) -> &BangDb {
        &self.bangs
    }

    /// Process all parsed lines, evaluating variables and conditionals.
    ///
    /// Returns the list of rules found in the Tupfile.
    /// If `base_dir` is provided, include directives are resolved relative to it.
    pub fn evaluate(&mut self) -> Result<Vec<Rule>, ParseError> {
        self.evaluate_with_dir(None)
    }

    /// Process all parsed lines with a base directory for include resolution.
    ///
    /// `base_dir` is the directory containing the Tupfile (for include resolution).
    pub fn evaluate_with_dir(
        &mut self,
        base_dir: Option<&std::path::Path>,
    ) -> Result<Vec<Rule>, ParseError> {
        self.evaluate_with_dirs(base_dir, None, None)
    }

    /// Process all parsed lines with base directory and tup root for TUP_CWD.
    ///
    /// `tupfile_dir` is the directory of the original Tupfile being parsed (stays
    /// constant through includes). TUP_CWD is computed as the relative path from
    /// `tupfile_dir` to `base_dir`, matching C tup behavior.
    pub fn evaluate_with_dirs(
        &mut self,
        base_dir: Option<&std::path::Path>,
        tup_root: Option<&std::path::Path>,
        tupfile_dir: Option<&std::path::Path>,
    ) -> Result<Vec<Rule>, ParseError> {
        // The original Tupfile directory — defaults to base_dir for the initial call
        let tf_dir = tupfile_dir.or(base_dir);

        // Set TUP_CWD to relative path from Tupfile's directory to current file's
        // directory. In C tup, TUP_CWD = "." for the Tupfile itself, and the
        // relative path (e.g. "../bar") for included files.
        if let (Some(dir), Some(origin)) = (base_dir, tf_dir) {
            let cwd = compute_relative_path(origin, dir);
            self.vars.set("TUP_CWD", &cwd);
        }
        let mut rules = Vec::new();
        let mut if_stack: Vec<bool> = Vec::new(); // true = active branch

        // Clone lines to allow mutable access to self during iteration
        let lines = self.lines.clone();

        const MAX_IF_DEPTH: usize = 8;

        for parsed in &lines {
            match &parsed.content {
                TupfileLine::Ifdef(var) => {
                    if if_stack.len() >= MAX_IF_DEPTH {
                        return Err(ParseError::Syntax {
                            file: String::new(),
                            line: parsed.line_number,
                            message: "too many nested if statements".to_string(),
                        });
                    }
                    let is_active = if_stack.last().copied().unwrap_or(true);
                    let defined = is_active && self.vars.get(var).is_some();
                    if_stack.push(defined);
                }
                TupfileLine::Ifndef(var) => {
                    if if_stack.len() >= MAX_IF_DEPTH {
                        return Err(ParseError::Syntax {
                            file: String::new(),
                            line: parsed.line_number,
                            message: "too many nested if statements".to_string(),
                        });
                    }
                    let is_active = if_stack.last().copied().unwrap_or(true);
                    let defined = is_active && self.vars.get(var).is_some();
                    if_stack.push(!defined);
                }
                TupfileLine::Ifeq(a, b) => {
                    if if_stack.len() >= MAX_IF_DEPTH {
                        return Err(ParseError::Syntax {
                            file: String::new(),
                            line: parsed.line_number,
                            message: "too many nested if statements".to_string(),
                        });
                    }
                    let is_active = if_stack.last().copied().unwrap_or(true);
                    let eq = if is_active {
                        let ea = self.vars.expand(a);
                        let eb = self.vars.expand(b);
                        ea == eb
                    } else {
                        false
                    };
                    if_stack.push(eq);
                }
                TupfileLine::Ifneq(a, b) => {
                    if if_stack.len() >= MAX_IF_DEPTH {
                        return Err(ParseError::Syntax {
                            file: String::new(),
                            line: parsed.line_number,
                            message: "too many nested if statements".to_string(),
                        });
                    }
                    let is_active = if_stack.last().copied().unwrap_or(true);
                    let eq = if is_active {
                        let ea = self.vars.expand(a);
                        let eb = self.vars.expand(b);
                        ea != eb
                    } else {
                        false
                    };
                    if_stack.push(eq);
                }
                TupfileLine::Else => {
                    if let Some(last) = if_stack.last_mut() {
                        *last = !*last;
                    } else {
                        return Err(ParseError::Syntax {
                            file: String::new(),
                            line: parsed.line_number,
                            message: "else statement outside of an if block".to_string(),
                        });
                    }
                }
                TupfileLine::Endif => {
                    if if_stack.pop().is_none() {
                        return Err(ParseError::Syntax {
                            file: String::new(),
                            line: parsed.line_number,
                            message: "endif statement outside of an if block".to_string(),
                        });
                    }
                }
                _ => {
                    // Only process if we're in an active branch
                    let active = if_stack.last().copied().unwrap_or(true);
                    if !active {
                        continue;
                    }

                    match &parsed.content {
                        TupfileLine::VarAssign { name, value } => {
                            let expanded_name = self.vars.expand(name);
                            let expanded = self.vars.expand(value);
                            self.vars.set(&expanded_name, &expanded);
                        }
                        TupfileLine::VarAppend { name, value } => {
                            let expanded_name = self.vars.expand(name);
                            let expanded = self.vars.expand(value);
                            self.vars.append(&expanded_name, &expanded);
                        }
                        TupfileLine::BangDef { name, definition } => {
                            let expanded_def = self.vars.expand(definition);
                            if let Err(e) = self.bangs.define(name, &expanded_def) {
                                return Err(ParseError::Syntax {
                                    file: String::new(),
                                    line: parsed.line_number,
                                    message: e,
                                });
                            }
                        }
                        TupfileLine::Rule(rule) => {
                            // Expand variables in rule components
                            let expanded_rule = self.expand_rule_vars(rule);

                            // Check if command is a bang macro invocation
                            if expanded_rule.command.command.starts_with('!') {
                                let macro_name = &expanded_rule.command.command;
                                match self.bangs.expand_rule(
                                    macro_name,
                                    &expanded_rule.inputs,
                                    &expanded_rule.outputs,
                                    expanded_rule.line_number,
                                ) {
                                    Ok(bang_expanded) => rules.push(bang_expanded),
                                    Err(e) => {
                                        return Err(ParseError::Syntax {
                                            file: String::new(),
                                            line: rule.line_number,
                                            message: e,
                                        })
                                    }
                                }
                            } else {
                                rules.push(expanded_rule);
                            }
                        }
                        TupfileLine::IncludeRules => {
                            // Search up directory tree for Tuprules.tup files.
                            // Matches C tup's parser_include_rules() (parser.c:791-843):
                            // walks from current dir up to tup root, including each
                            // Tuprules.tup found.
                            if let Some(dir) = base_dir {
                                if let Some(root) = tup_root {
                                    let mut search_dir = dir.to_path_buf();
                                    let mut tuprules_files = Vec::new();
                                    loop {
                                        let candidate = search_dir.join("Tuprules.tup");
                                        if candidate.exists() {
                                            tuprules_files.push(candidate);
                                        }
                                        if search_dir == root {
                                            break;
                                        }
                                        if !search_dir.pop() {
                                            break;
                                        }
                                    }
                                    // Process in reverse order (root first, then down)
                                    tuprules_files.reverse();
                                    for tuprules_path in tuprules_files {
                                        let content = std::fs::read_to_string(&tuprules_path)
                                            .map_err(|e| ParseError::Syntax {
                                                file: tuprules_path.display().to_string(),
                                                line: parsed.line_number,
                                                message: format!(
                                                    "failed to read Tuprules.tup: {e}"
                                                ),
                                            })?;
                                        let tuprules_name = tuprules_path.display().to_string();
                                        let mut sub_reader =
                                            TupfileReader::parse(&content, &tuprules_name)?;
                                        sub_reader.vars = self.vars.clone();
                                        sub_reader.bangs = self.bangs.clone();
                                        let tuprules_dir = tuprules_path.parent().unwrap_or(dir);
                                        let sub_rules = sub_reader.evaluate_with_dirs(
                                            Some(tuprules_dir),
                                            Some(root),
                                            tf_dir,
                                        )?;
                                        let saved_cwd = self.vars.get("TUP_CWD").map(String::from);
                                        self.vars = sub_reader.vars;
                                        if let Some(cwd) = saved_cwd {
                                            self.vars.set("TUP_CWD", &cwd);
                                        }
                                        self.bangs = sub_reader.bangs;
                                        self.gitignore_requested |= sub_reader.gitignore_requested;
                                        rules.extend(sub_rules);
                                    }
                                }
                            }
                        }
                        TupfileLine::Include(path) => {
                            if let Some(dir) = base_dir {
                                let include_path = dir.join(self.vars.expand(path));
                                if include_path.exists() {
                                    let content =
                                        std::fs::read_to_string(&include_path).map_err(|e| {
                                            ParseError::Syntax {
                                                file: include_path.display().to_string(),
                                                line: parsed.line_number,
                                                message: format!(
                                                    "failed to read include file: {e}"
                                                ),
                                            }
                                        })?;
                                    let include_name = include_path.display().to_string();
                                    let mut sub_reader =
                                        TupfileReader::parse(&content, &include_name)?;
                                    // Share our variable and bang databases
                                    sub_reader.vars = self.vars.clone();
                                    sub_reader.bangs = self.bangs.clone();
                                    let include_dir = include_path.parent().unwrap_or(dir);
                                    let sub_rules = sub_reader.evaluate_with_dirs(
                                        Some(include_dir),
                                        tup_root,
                                        tf_dir,
                                    )?;
                                    // Merge back any variable changes, but restore TUP_CWD
                                    // to this file's value (it was overwritten by the include)
                                    let saved_cwd = self.vars.get("TUP_CWD").map(String::from);
                                    self.vars = sub_reader.vars;
                                    if let Some(cwd) = saved_cwd {
                                        self.vars.set("TUP_CWD", &cwd);
                                    }
                                    self.bangs = sub_reader.bangs;
                                    self.gitignore_requested |= sub_reader.gitignore_requested;
                                    rules.extend(sub_rules);
                                }
                            }
                        }
                        TupfileLine::GitIgnore => {
                            self.gitignore_requested = true;
                        }
                        TupfileLine::Run(script) => {
                            if let Some(dir) = base_dir {
                                let expanded_script = self.vars.expand(script);
                                let script_path = dir.join(&expanded_script);
                                let output = std::process::Command::new("sh")
                                    .arg("-e")
                                    .arg(&script_path)
                                    .current_dir(dir)
                                    .stdout(std::process::Stdio::piped())
                                    .stderr(std::process::Stdio::piped())
                                    .output()
                                    .map_err(|e| ParseError::Syntax {
                                        file: String::new(),
                                        line: parsed.line_number,
                                        message: format!(
                                            "failed to run script '{}': {e}",
                                            expanded_script
                                        ),
                                    })?;

                                if !output.status.success() {
                                    let stderr = String::from_utf8_lossy(&output.stderr);
                                    return Err(ParseError::Syntax {
                                        file: String::new(),
                                        line: parsed.line_number,
                                        message: format!(
                                            "run script '{}' failed: {}",
                                            expanded_script,
                                            stderr.trim()
                                        ),
                                    });
                                }

                                let stdout = String::from_utf8_lossy(&output.stdout);
                                if !stdout.is_empty() {
                                    let run_name = format!("run:{}", expanded_script);
                                    let mut sub_reader = TupfileReader::parse(&stdout, &run_name)?;
                                    sub_reader.vars = self.vars.clone();
                                    sub_reader.bangs = self.bangs.clone();
                                    sub_reader.gitignore_requested = self.gitignore_requested;
                                    let sub_rules = sub_reader
                                        .evaluate_with_dirs(base_dir, tup_root, tf_dir)?;
                                    self.vars = sub_reader.vars;
                                    self.bangs = sub_reader.bangs;
                                    self.gitignore_requested |= sub_reader.gitignore_requested;
                                    rules.extend(sub_rules);
                                }
                            }
                        }
                        TupfileLine::Error(msg) => {
                            let expanded = self.vars.expand(msg);
                            return Err(ParseError::Syntax {
                                file: String::new(),
                                line: parsed.line_number,
                                message: format!(
                                    "Found 'error' command parsing Tupfile: {expanded}"
                                ),
                            });
                        }
                        TupfileLine::Unknown(text) => {
                            return Err(ParseError::Syntax {
                                file: String::new(),
                                line: parsed.line_number,
                                message: format!("unrecognized line: '{text}'"),
                            });
                        }
                        _ => {
                            // Export, Import, etc. — handled later
                        }
                    }
                }
            }
        }

        // Check for unclosed if blocks
        if !if_stack.is_empty() {
            return Err(ParseError::Syntax {
                file: String::new(),
                line: 0,
                message: "missing endif before EOF".to_string(),
            });
        }

        Ok(rules)
    }

    /// Expand $(VAR) references in all components of a rule.
    ///
    /// After expansion, inputs and outputs are re-split on whitespace.
    /// This matches C tup's eval_path_list() which splits expanded
    /// variables with strcspn(p, " \t") (parser.c:2452-2471).
    fn expand_rule_vars(&self, rule: &Rule) -> Rule {
        Rule {
            foreach: rule.foreach,
            inputs: expand_and_split(&self.vars, &rule.inputs),
            order_only_inputs: expand_and_split(&self.vars, &rule.order_only_inputs),
            command: crate::rule::RuleCommand {
                display: rule.command.display.as_ref().map(|s| self.vars.expand(s)),
                flags: rule.command.flags.clone(),
                command: self.vars.expand(&rule.command.command),
            },
            outputs: expand_and_split(&self.vars, &rule.outputs),
            extra_outputs: expand_and_split(&self.vars, &rule.extra_outputs),
            line_number: rule.line_number,
            had_inputs: rule.had_inputs,
        }
    }

    /// Get all lines (for inspection).
    pub fn parsed_lines(&self) -> impl Iterator<Item = (usize, &TupfileLine)> {
        self.lines.iter().map(|pl| (pl.line_number, &pl.content))
    }

    /// Set a config variable for @(VAR) expansion.
    pub fn set_config(&mut self, name: &str, value: &str) {
        self.vars.set_config(name, value);
    }

    /// Whether the `.gitignore` directive was found during evaluation.
    pub fn gitignore_requested(&self) -> bool {
        self.gitignore_requested
    }
}

/// Expand variables in a list of strings, then re-split on whitespace.
///
/// Matches C tup's eval_path_list() (parser.c:2452-2471) which splits
/// expanded variables with strcspn(p, " \t").
fn expand_and_split(vars: &ParseVarDb, items: &[String]) -> Vec<String> {
    let mut result = Vec::new();
    for s in items {
        let expanded = vars.expand(s);
        // If the expanded result contains whitespace, split into words
        if expanded.contains(char::is_whitespace) {
            for word in expanded.split_whitespace() {
                if !word.is_empty() {
                    result.push(word.to_string());
                }
            }
        } else if !expanded.is_empty() {
            result.push(expanded);
        }
    }
    result
}

/// Join lines ending with `\` (line continuation).
fn join_continuation_lines(content: &str) -> String {
    let mut result = String::with_capacity(content.len());
    let mut continuation = false;

    for line in content.lines() {
        if continuation {
            result.push(' ');
            let trimmed = line.trim_start();
            if let Some(stripped) = trimmed.strip_suffix('\\') {
                result.push_str(stripped);
            } else {
                result.push_str(trimmed);
                result.push('\n');
                continuation = false;
            }
        } else if let Some(stripped) = line.strip_suffix('\\') {
            result.push_str(stripped);
            continuation = true;
        } else {
            result.push_str(line);
            result.push('\n');
        }
    }

    result
}

/// Parse a single line into a TupfileLine.
fn parse_line(line: &str, line_number: usize, filename: &str) -> Result<TupfileLine, ParseError> {
    // Conditionals (must be checked before variable assignment)
    if line == "else" {
        return Ok(TupfileLine::Else);
    }
    if line == "endif" {
        return Ok(TupfileLine::Endif);
    }
    if let Some(rest) = line.strip_prefix("ifdef ") {
        return Ok(TupfileLine::Ifdef(rest.trim().to_string()));
    }
    if let Some(rest) = line.strip_prefix("ifndef ") {
        return Ok(TupfileLine::Ifndef(rest.trim().to_string()));
    }
    if let Some(rest) = line.strip_prefix("ifeq ") {
        let (a, b) = parse_eq_args(rest).map_err(|msg| ParseError::Syntax {
            file: filename.to_string(),
            line: line_number,
            message: msg,
        })?;
        return Ok(TupfileLine::Ifeq(a, b));
    }
    if let Some(rest) = line.strip_prefix("ifneq ") {
        let (a, b) = parse_eq_args(rest).map_err(|msg| ParseError::Syntax {
            file: filename.to_string(),
            line: line_number,
            message: msg,
        })?;
        return Ok(TupfileLine::Ifneq(a, b));
    }

    // Directives
    if let Some(rest) = line.strip_prefix("include ") {
        return Ok(TupfileLine::Include(rest.trim().to_string()));
    }
    if line == "include_rules" {
        return Ok(TupfileLine::IncludeRules);
    }
    if let Some(rest) = line.strip_prefix("export ") {
        return Ok(TupfileLine::Export(rest.trim().to_string()));
    }
    if let Some(rest) = line.strip_prefix("import ") {
        return Ok(TupfileLine::Import(rest.trim().to_string()));
    }
    if let Some(rest) = line.strip_prefix("preload ") {
        return Ok(TupfileLine::Preload(rest.trim().to_string()));
    }
    if let Some(rest) = line.strip_prefix("run ") {
        return Ok(TupfileLine::Run(rest.trim().to_string()));
    }
    if let Some(rest) = line.strip_prefix("error ") {
        return Ok(TupfileLine::Error(rest.trim().to_string()));
    }
    if line == ".gitignore" {
        return Ok(TupfileLine::GitIgnore);
    }

    // Rule (starts with `:`)
    if let Some(rest) = line.strip_prefix(':') {
        let rule = Rule::parse(rest, line_number).map_err(|msg| ParseError::Syntax {
            file: filename.to_string(),
            line: line_number,
            message: msg,
        })?;
        return Ok(TupfileLine::Rule(rule));
    }

    // Bang macro definition (starts with `!`)
    if line.starts_with('!') {
        if let Some(eq_pos) = line.find(" = ") {
            let name = line[1..eq_pos].to_string();
            let def = line[eq_pos + 3..].to_string();
            return Ok(TupfileLine::BangDef {
                name,
                definition: def,
            });
        }
    }

    // Variable assignment or append
    if let Some(pos) = line.find(" += ") {
        let name = line[..pos].trim().to_string();
        let value = line[pos + 4..].to_string();
        return Ok(TupfileLine::VarAppend { name, value });
    }
    if let Some(pos) = line.find(" = ") {
        let name = line[..pos].trim().to_string();
        let value = line[pos + 3..].to_string();
        return Ok(TupfileLine::VarAssign { name, value });
    }
    if let Some(pos) = line.find(" := ") {
        let name = line[..pos].trim().to_string();
        let value = line[pos + 4..].to_string();
        return Ok(TupfileLine::VarAssign { name, value });
    }

    // Return as unknown line — will be rejected during evaluation if in an active branch,
    // but silently ignored in inactive if/else branches (matching C tup behavior)
    Ok(TupfileLine::Unknown(line.to_string()))
}

/// Parse `(A, B)` arguments for ifeq/ifneq.
fn parse_eq_args(text: &str) -> Result<(String, String), String> {
    let text = text.trim();
    if !text.starts_with('(') || !text.ends_with(')') {
        return Err("expected (value1, value2)".to_string());
    }
    let inner = &text[1..text.len() - 1];
    match inner.split_once(',') {
        Some((a, b)) => Ok((a.trim().to_string(), b.trim().to_string())),
        None => Err("expected (value1, value2)".to_string()),
    }
}

/// Compute a relative path from `from` to `to`.
///
/// Returns "." when both paths are the same. Otherwise returns a relative
/// path using ".." components as needed, matching C tup's TUP_CWD behavior.
fn compute_relative_path(from: &std::path::Path, to: &std::path::Path) -> String {
    use std::path::Component;

    // Normalize both paths to remove . and .. components
    let normalize = |p: &std::path::Path| -> std::path::PathBuf {
        let mut parts = Vec::new();
        for c in p.components() {
            match c {
                Component::ParentDir => {
                    if parts
                        .last()
                        .is_some_and(|l| matches!(l, Component::Normal(_)))
                    {
                        parts.pop();
                    } else {
                        parts.push(c);
                    }
                }
                Component::CurDir => {}
                _ => parts.push(c),
            }
        }
        parts.iter().collect()
    };

    let from_norm = normalize(from);
    let to_norm = normalize(to);

    // Find common prefix length
    let from_parts: Vec<_> = from_norm.components().collect();
    let to_parts: Vec<_> = to_norm.components().collect();
    let common = from_parts
        .iter()
        .zip(&to_parts)
        .take_while(|(a, b)| a == b)
        .count();

    // Build relative path: ".." for each remaining `from` component, then `to` remainder
    let ups = from_parts.len() - common;
    let mut result = std::path::PathBuf::new();
    for _ in 0..ups {
        result.push("..");
    }
    for part in &to_parts[common..] {
        result.push(part.as_os_str());
    }

    if result.as_os_str().is_empty() {
        ".".to_string()
    } else {
        result.to_string_lossy().to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_basic_tupfile() {
        let content = r#"
CC = gcc
CFLAGS = -Wall -O2

: foreach *.c |> $(CC) $(CFLAGS) -c %f -o %o |> %B.o
: *.o |> $(CC) %f -o myapp |> myapp
"#;
        let reader = TupfileReader::parse(content, "Tupfile").unwrap();
        let lines: Vec<_> = reader.parsed_lines().collect();
        assert_eq!(lines.len(), 4); // 2 var assigns + 2 rules
    }

    #[test]
    fn test_parse_comments() {
        let content = "# This is a comment\nCC = gcc\n# Another comment\n";
        let reader = TupfileReader::parse(content, "Tupfile").unwrap();
        let lines: Vec<_> = reader.parsed_lines().collect();
        assert_eq!(lines.len(), 1); // Only the variable assignment
    }

    #[test]
    fn test_parse_empty_lines() {
        let content = "\n\n\nCC = gcc\n\n\n";
        let reader = TupfileReader::parse(content, "Tupfile").unwrap();
        let lines: Vec<_> = reader.parsed_lines().collect();
        assert_eq!(lines.len(), 1);
    }

    #[test]
    fn test_parse_line_continuation() {
        let content = "CFLAGS = -Wall \\\n  -O2 \\\n  -g\n";
        let joined = join_continuation_lines(content);
        assert!(joined.contains("-Wall  -O2  -g"));
    }

    #[test]
    fn test_parse_variable_assign() {
        let content = "CC = gcc\nCFLAGS = -Wall -O2\n";
        let mut reader = TupfileReader::parse(content, "Tupfile").unwrap();
        reader.evaluate().unwrap();
        assert_eq!(reader.vars().get("CC"), Some("gcc"));
        assert_eq!(reader.vars().get("CFLAGS"), Some("-Wall -O2"));
    }

    #[test]
    fn test_parse_variable_append() {
        let content = "CFLAGS = -Wall\nCFLAGS += -O2\n";
        let mut reader = TupfileReader::parse(content, "Tupfile").unwrap();
        reader.evaluate().unwrap();
        assert_eq!(reader.vars().get("CFLAGS"), Some("-Wall -O2"));
    }

    #[test]
    fn test_parse_variable_expansion_in_assign() {
        let content = "CC = gcc\nCMD = $(CC) -c\n";
        let mut reader = TupfileReader::parse(content, "Tupfile").unwrap();
        reader.evaluate().unwrap();
        assert_eq!(reader.vars().get("CMD"), Some("gcc -c"));
    }

    #[test]
    fn test_parse_ifdef_true() {
        let content = "DEBUG = yes\nifdef DEBUG\nCFLAGS = -g\nendif\n";
        let mut reader = TupfileReader::parse(content, "Tupfile").unwrap();
        reader.evaluate().unwrap();
        assert_eq!(reader.vars().get("CFLAGS"), Some("-g"));
    }

    #[test]
    fn test_parse_ifdef_false() {
        let content = "ifdef DEBUG\nCFLAGS = -g\nendif\n";
        let mut reader = TupfileReader::parse(content, "Tupfile").unwrap();
        reader.evaluate().unwrap();
        assert_eq!(reader.vars().get("CFLAGS"), None);
    }

    #[test]
    fn test_parse_ifndef() {
        let content = "ifndef RELEASE\nCFLAGS = -g\nendif\n";
        let mut reader = TupfileReader::parse(content, "Tupfile").unwrap();
        reader.evaluate().unwrap();
        assert_eq!(reader.vars().get("CFLAGS"), Some("-g"));
    }

    #[test]
    fn test_parse_ifdef_else() {
        let content = "ifdef DEBUG\nMODE = debug\nelse\nMODE = release\nendif\n";
        let mut reader = TupfileReader::parse(content, "Tupfile").unwrap();
        reader.evaluate().unwrap();
        assert_eq!(reader.vars().get("MODE"), Some("release"));
    }

    #[test]
    fn test_parse_ifeq() {
        let content = "CC = gcc\nifeq ($(CC), gcc)\nIS_GCC = yes\nendif\n";
        let mut reader = TupfileReader::parse(content, "Tupfile").unwrap();
        reader.evaluate().unwrap();
        assert_eq!(reader.vars().get("IS_GCC"), Some("yes"));
    }

    #[test]
    fn test_parse_ifneq() {
        let content = "CC = clang\nifneq ($(CC), gcc)\nNOT_GCC = yes\nendif\n";
        let mut reader = TupfileReader::parse(content, "Tupfile").unwrap();
        reader.evaluate().unwrap();
        assert_eq!(reader.vars().get("NOT_GCC"), Some("yes"));
    }

    #[test]
    fn test_parse_directives() {
        let content = "export PATH\nimport HOME\ninclude rules.tup\ninclude_rules\n.gitignore\n";
        let reader = TupfileReader::parse(content, "Tupfile").unwrap();
        let lines: Vec<_> = reader.parsed_lines().collect();
        assert_eq!(lines.len(), 5);

        assert!(matches!(lines[0].1, TupfileLine::Export(_)));
        assert!(matches!(lines[1].1, TupfileLine::Import(_)));
        assert!(matches!(lines[2].1, TupfileLine::Include(_)));
        assert!(matches!(lines[3].1, TupfileLine::IncludeRules));
        assert!(matches!(lines[4].1, TupfileLine::GitIgnore));
    }

    #[test]
    fn test_parse_bang_definition() {
        let content = "!cc = |> gcc -c %f -o %o |> %B.o\n";
        let reader = TupfileReader::parse(content, "Tupfile").unwrap();
        let lines: Vec<_> = reader.parsed_lines().collect();
        assert_eq!(lines.len(), 1);
        if let TupfileLine::BangDef { name, definition } = &lines[0].1 {
            assert_eq!(name, "cc");
            assert!(definition.contains("gcc"));
        } else {
            panic!("expected BangDef");
        }
    }

    #[test]
    fn test_parse_rules_extracted() {
        let content = ": foo.c |> gcc -c foo.c -o foo.o |> foo.o\n";
        let mut reader = TupfileReader::parse(content, "Tupfile").unwrap();
        let rules = reader.evaluate().unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].inputs, vec!["foo.c"]);
        assert_eq!(rules[0].command.command, "gcc -c foo.c -o foo.o");
        assert_eq!(rules[0].outputs, vec!["foo.o"]);
    }

    #[test]
    fn test_parse_error_directive() {
        let content = "error Something went wrong\n";
        let reader = TupfileReader::parse(content, "Tupfile").unwrap();
        let lines: Vec<_> = reader.parsed_lines().collect();
        if let TupfileLine::Error(msg) = &lines[0].1 {
            assert_eq!(msg, "Something went wrong");
        } else {
            panic!("expected Error");
        }
    }

    #[test]
    fn test_full_tupfile() {
        let content = r#"
# Build configuration
CC = gcc
CFLAGS = -Wall

ifdef DEBUG
CFLAGS += -g -O0
else
CFLAGS += -O2
endif

# Compile all C files
: foreach *.c |> $(CC) $(CFLAGS) -c %f -o %o |> %B.o

# Link
: *.o |> $(CC) %f -o myapp |> myapp

.gitignore
"#;
        let mut reader = TupfileReader::parse(content, "Tupfile").unwrap();
        let rules = reader.evaluate().unwrap();
        assert_eq!(rules.len(), 2);
        assert_eq!(reader.vars().get("CC"), Some("gcc"));
        assert_eq!(reader.vars().get("CFLAGS"), Some("-Wall -O2"));
    }

    #[test]
    fn test_bang_macro_definition_and_use() {
        let content = r#"
!cc = |> gcc -c %f -o %o |> %B.o
: foreach *.c |> !cc |>
"#;
        let mut reader = TupfileReader::parse(content, "Tupfile").unwrap();
        let rules = reader.evaluate().unwrap();

        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].command.command, "gcc -c %f -o %o");
        assert_eq!(rules[0].outputs, vec!["%B.o"]);
        // foreach comes from the macro
        assert!(!rules[0].foreach); // The rule itself isn't foreach, macro isn't either
    }

    #[test]
    fn test_bang_macro_foreach() {
        let content = r#"
!cc = foreach |> gcc -c %f -o %o |> %B.o
: *.c |> !cc |>
"#;
        let mut reader = TupfileReader::parse(content, "Tupfile").unwrap();
        let rules = reader.evaluate().unwrap();

        assert_eq!(rules.len(), 1);
        assert!(rules[0].foreach);
    }

    #[test]
    fn test_bang_macro_with_variable() {
        let content = r#"
CC = clang
!cc = |> $(CC) -c %f -o %o |> %B.o
: main.c |> !cc |>
"#;
        let mut reader = TupfileReader::parse(content, "Tupfile").unwrap();
        let rules = reader.evaluate().unwrap();

        assert_eq!(rules.len(), 1);
        // Variable should be expanded in the macro definition
        assert_eq!(rules[0].command.command, "clang -c %f -o %o");
    }
}
