use std::collections::BTreeMap;

use crate::rule::{Rule, RuleCommand};

/// A bang macro definition.
///
/// Bang macros are rule templates defined as `!name = |> COMMAND |> OUTPUT`.
/// They can be invoked in rules as `: inputs |> !name |> extra_outputs`.
#[derive(Debug, Clone)]
pub struct BangMacro {
    /// Macro name (without the `!` prefix).
    pub name: String,
    /// Whether this is a foreach macro.
    pub foreach: bool,
    /// Input pattern (optional, for macros with built-in inputs).
    pub input: Option<String>,
    /// Command template.
    pub command: String,
    /// Output pattern(s).
    pub outputs: Vec<String>,
    /// Extra output pattern(s).
    pub extra_outputs: Vec<String>,
}

/// Storage for bang macro definitions.
#[derive(Debug, Default, Clone)]
pub struct BangDb {
    macros: BTreeMap<String, BangMacro>,
}

impl BangDb {
    pub fn new() -> Self {
        Self::default()
    }

    /// Define a bang macro from its definition string.
    ///
    /// Format: `!name = [foreach] [INPUT] |> COMMAND |> OUTPUT [| EXTRA]`
    /// Or alias: `!name = !other`
    pub fn define(&mut self, name: &str, definition: &str) -> Result<(), String> {
        let name = name.trim().trim_start_matches('!');
        let definition = definition.trim();

        // Check for alias: !name = !other
        if definition.starts_with('!') {
            let other_name = definition.trim_start_matches('!');
            let other = self
                .macros
                .get(other_name)
                .ok_or_else(|| format!("unknown !-macro '!{other_name}'"))?
                .clone();
            self.macros.insert(
                name.to_string(),
                BangMacro {
                    name: name.to_string(),
                    ..other
                },
            );
            return Ok(());
        }

        // Parse: [foreach] [INPUT] |> COMMAND |> OUTPUT [| EXTRA]
        let parts: Vec<&str> = definition.split("|>").collect();
        if parts.len() < 3 {
            return Err("bang macro must have at least 3 sections separated by |>".to_string());
        }

        let input_section = parts[0].trim();
        let (foreach, input) = if input_section == "foreach" {
            (true, None)
        } else if let Some(rest) = input_section.strip_prefix("foreach ") {
            (
                true,
                if rest.trim().is_empty() {
                    None
                } else {
                    Some(rest.trim().to_string())
                },
            )
        } else {
            (
                false,
                if input_section.is_empty() {
                    None
                } else {
                    Some(input_section.to_string())
                },
            )
        };

        let command = parts[1].trim().to_string();

        let output_text = parts[2..].join("|>");
        let (outputs, extra_outputs) = if let Some(pipe_pos) = output_text.find(" | ") {
            let outs: Vec<String> = output_text[..pipe_pos]
                .split_whitespace()
                .map(|s| s.to_string())
                .collect();
            let extras: Vec<String> = output_text[pipe_pos + 3..]
                .split_whitespace()
                .map(|s| s.to_string())
                .collect();
            (outs, extras)
        } else {
            (
                output_text
                    .split_whitespace()
                    .map(|s| s.to_string())
                    .collect(),
                vec![],
            )
        };

        self.macros.insert(
            name.to_string(),
            BangMacro {
                name: name.to_string(),
                foreach,
                input,
                command,
                outputs,
                extra_outputs,
            },
        );

        Ok(())
    }

    /// Look up a bang macro by name.
    pub fn get(&self, name: &str) -> Option<&BangMacro> {
        let name = name.trim_start_matches('!');
        self.macros.get(name)
    }

    /// Expand a bang macro invocation into a Rule.
    ///
    /// A rule like `: inputs |> !cc |> extra_outputs` expands the `!cc`
    /// macro, merging the rule's inputs/outputs with the macro's template.
    pub fn expand_rule(
        &self,
        macro_name: &str,
        rule_inputs: &[String],
        rule_outputs: &[String],
        rule_foreach: bool,
        line_number: usize,
    ) -> Result<Rule, String> {
        let mac = self
            .get(macro_name)
            .ok_or_else(|| format!("unknown !-macro '!{macro_name}'"))?;

        // Merge inputs: rule inputs override macro inputs
        let inputs = if rule_inputs.is_empty() {
            mac.input
                .as_ref()
                .map(|i| i.split_whitespace().map(|s| s.to_string()).collect())
                .unwrap_or_default()
        } else {
            rule_inputs.to_vec()
        };

        // Merge outputs: rule outputs override macro outputs
        let outputs = if rule_outputs.is_empty() {
            mac.outputs.clone()
        } else {
            rule_outputs.to_vec()
        };

        let had_inputs = !inputs.is_empty();
        Ok(Rule {
            foreach: rule_foreach || mac.foreach,
            inputs,
            order_only_inputs: vec![],
            command: RuleCommand {
                display: None,
                flags: None,
                command: mac.command.clone(),
            },
            outputs,
            extra_outputs: mac.extra_outputs.clone(),
            line_number,
            had_inputs,
            vars_snapshot: None,
        })
    }

    /// Get number of defined macros.
    pub fn len(&self) -> usize {
        self.macros.len()
    }

    /// Check if no macros are defined.
    pub fn is_empty(&self) -> bool {
        self.macros.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_define_simple_macro() {
        let mut db = BangDb::new();
        db.define("cc", "|> gcc -c %f -o %o |> %B.o").unwrap();

        let mac = db.get("cc").unwrap();
        assert_eq!(mac.command, "gcc -c %f -o %o");
        assert_eq!(mac.outputs, vec!["%B.o"]);
        assert!(!mac.foreach);
    }

    #[test]
    fn test_define_foreach_macro() {
        let mut db = BangDb::new();
        db.define("cc", "foreach |> gcc -c %f -o %o |> %B.o")
            .unwrap();

        let mac = db.get("cc").unwrap();
        assert!(mac.foreach);
    }

    #[test]
    fn test_define_macro_with_input() {
        let mut db = BangDb::new();
        db.define("link", "*.o |> gcc %f -o %o |> myapp").unwrap();

        let mac = db.get("link").unwrap();
        assert_eq!(mac.input, Some("*.o".to_string()));
    }

    #[test]
    fn test_define_macro_with_extras() {
        let mut db = BangDb::new();
        db.define("cc", "|> gcc -c %f -o %o -MD -MF %O.d |> %B.o | %B.o.d")
            .unwrap();

        let mac = db.get("cc").unwrap();
        assert_eq!(mac.outputs, vec!["%B.o"]);
        assert_eq!(mac.extra_outputs, vec!["%B.o.d"]);
    }

    #[test]
    fn test_define_alias() {
        let mut db = BangDb::new();
        db.define("cc", "|> gcc -c %f -o %o |> %B.o").unwrap();
        db.define("compile", "!cc").unwrap();

        let mac = db.get("compile").unwrap();
        assert_eq!(mac.command, "gcc -c %f -o %o");
    }

    #[test]
    fn test_alias_unknown() {
        let mut db = BangDb::new();
        let result = db.define("foo", "!unknown");
        assert!(result.is_err());
    }

    #[test]
    fn test_expand_rule_basic() {
        let mut db = BangDb::new();
        db.define("cc", "|> gcc -c %f -o %o |> %B.o").unwrap();

        let rule = db
            .expand_rule("cc", &["main.c".to_string()], &[], false, 1)
            .unwrap();

        assert_eq!(rule.inputs, vec!["main.c"]);
        assert_eq!(rule.command.command, "gcc -c %f -o %o");
        assert_eq!(rule.outputs, vec!["%B.o"]);
    }

    #[test]
    fn test_expand_rule_override_outputs() {
        let mut db = BangDb::new();
        db.define("cc", "|> gcc -c %f -o %o |> %B.o").unwrap();

        let rule = db
            .expand_rule(
                "cc",
                &["main.c".to_string()],
                &["custom.o".to_string()],
                false,
                1,
            )
            .unwrap();

        assert_eq!(rule.outputs, vec!["custom.o"]);
    }

    #[test]
    fn test_expand_rule_macro_inputs() {
        let mut db = BangDb::new();
        db.define("link", "*.o |> gcc %f -o %o |> myapp").unwrap();

        let rule = db.expand_rule("link", &[], &[], false, 1).unwrap();
        assert_eq!(rule.inputs, vec!["*.o"]);
    }

    #[test]
    fn test_lookup_with_bang_prefix() {
        let mut db = BangDb::new();
        db.define("cc", "|> gcc -c %f -o %o |> %B.o").unwrap();

        assert!(db.get("!cc").is_some());
        assert!(db.get("cc").is_some());
    }

    #[test]
    fn test_len() {
        let mut db = BangDb::new();
        assert!(db.is_empty());
        db.define("cc", "|> gcc |> out").unwrap();
        db.define("ld", "|> ld |> out").unwrap();
        assert_eq!(db.len(), 2);
    }
}
