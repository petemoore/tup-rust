/// A parsed rule from a Tupfile.
///
/// Rules have the form: `: [foreach] INPUT |> COMMAND |> OUTPUT`
#[derive(Debug, Clone)]
pub struct Rule {
    /// Whether this is a foreach rule (iterate over each input).
    pub foreach: bool,
    /// Input file patterns (before the first `|>`).
    pub inputs: Vec<String>,
    /// Order-only inputs (after `|` in input section).
    pub order_only_inputs: Vec<String>,
    /// The command to execute.
    pub command: RuleCommand,
    /// Output file patterns (after the second `|>`).
    pub outputs: Vec<String>,
    /// Extra outputs (after `|` in output section).
    pub extra_outputs: Vec<String>,
    /// Line number in the Tupfile.
    pub line_number: usize,
    /// Whether the rule originally had inputs (pre-expansion).
    /// When true and inputs expand to empty, the rule should be skipped.
    /// Matches C tup behavior: `$(empty_var) |> cmd |>` is skipped,
    /// but `: |> cmd |>` runs.
    pub had_inputs: bool,
    /// Snapshot of variables at the time this rule was parsed.
    /// Used for multi-pass expansion: after %-flags are expanded,
    /// these vars are used for the second-pass $(var) expansion.
    /// This matches C tup's do_rule() which calls tup_printf() then eval().
    pub vars_snapshot: Option<std::collections::BTreeMap<String, String>>,
}

/// The command portion of a rule.
#[derive(Debug, Clone)]
pub struct RuleCommand {
    /// Display string (between `^` markers, if present).
    pub display: Option<String>,
    /// Flags string (after `^` before display).
    pub flags: Option<String>,
    /// The actual command to execute.
    pub command: String,
}

impl Rule {
    /// Parse a rule from the text after the leading `:`.
    ///
    /// Format: `[foreach] INPUT... |> [^FLAGS^ DISPLAY^] COMMAND |> OUTPUT... [| EXTRA...]`
    pub fn parse(text: &str, line_number: usize) -> Result<Self, String> {
        let text = text.trim();

        // Check for foreach (accepts space or tab as separator, matching C tup)
        let (foreach, rest) = if let Some(stripped) = text
            .strip_prefix("foreach ")
            .or_else(|| text.strip_prefix("foreach\t"))
        {
            (true, stripped.trim())
        } else {
            (false, text)
        };

        // Split on |> delimiters
        let parts: Vec<&str> = rest.split("|>").collect();
        if parts.len() < 3 {
            return Err("rule must have at least 3 sections separated by |>".to_string());
        }

        // Parse inputs (first section)
        let (inputs, order_only_inputs) = parse_input_section(parts[0].trim());

        // Parse command (second section)
        let command = parse_command_section(parts[1].trim());

        // Parse outputs (third section, and any remaining sections joined)
        let output_text = parts[2..].join("|>");
        let (outputs, extra_outputs) = parse_output_section(output_text.trim());

        let had_inputs = !inputs.is_empty();
        Ok(Rule {
            foreach,
            inputs,
            order_only_inputs,
            command,
            outputs,
            extra_outputs,
            line_number,
            had_inputs,
            vars_snapshot: None,
        })
    }
}

/// Parse the input section of a rule.
///
/// Inputs are space-separated. Order-only inputs follow `|`.
fn parse_input_section(text: &str) -> (Vec<String>, Vec<String>) {
    if let Some(pipe_pos) = text.find(" | ") {
        let inputs = split_words(&text[..pipe_pos]);
        let oo_inputs = split_words(&text[pipe_pos + 3..]);
        (inputs, oo_inputs)
    } else if let Some(rest) = text.strip_prefix("| ") {
        // No regular inputs, only order-only: `| order.h`
        (vec![], split_words(rest))
    } else if text == "|" {
        (vec![], vec![])
    } else if let Some(stripped) = text.strip_suffix(" |") {
        let inputs = split_words(stripped);
        (inputs, vec![])
    } else {
        (split_words(text), vec![])
    }
}

/// Parse the command section of a rule.
///
/// May start with `^DISPLAY^` before the actual command.
/// Flags are single characters at the start of the display, separated
/// by a space: `^t CC foo.c^` → flags="t", display="CC foo.c"
fn parse_command_section(text: &str) -> RuleCommand {
    if let Some(rest) = text.strip_prefix('^') {
        if let Some(end) = rest.find('^') {
            let display_part = &rest[..end];
            let command = rest[end + 1..].trim();

            // Check if the display starts with short flags (1-2 chars
            // before a space, all non-space characters)
            let (flags, display) = if let Some(space_pos) = display_part.find(' ') {
                let potential_flags = &display_part[..space_pos];
                if potential_flags.len() <= 2
                    && !potential_flags.is_empty()
                    && potential_flags.chars().all(|c| c.is_ascii_lowercase())
                {
                    let d = display_part[space_pos + 1..].trim();
                    (
                        Some(potential_flags.to_string()),
                        if d.is_empty() {
                            None
                        } else {
                            Some(d.to_string())
                        },
                    )
                } else {
                    (None, Some(display_part.to_string()))
                }
            } else {
                // No space — could be just flags or just display
                if display_part.is_empty() {
                    (None, None)
                } else {
                    (None, Some(display_part.to_string()))
                }
            };

            RuleCommand {
                display,
                flags,
                command: command.to_string(),
            }
        } else {
            RuleCommand {
                display: None,
                flags: None,
                command: text.to_string(),
            }
        }
    } else {
        RuleCommand {
            display: None,
            flags: None,
            command: text.to_string(),
        }
    }
}

/// Parse the output section of a rule.
///
/// Extra outputs follow `|`.
fn parse_output_section(text: &str) -> (Vec<String>, Vec<String>) {
    if let Some(pipe_pos) = text.find(" | ") {
        let outputs = split_words(&text[..pipe_pos]);
        let extras = split_words(&text[pipe_pos + 3..]);
        (outputs, extras)
    } else {
        (split_words(text), vec![])
    }
}

/// Split a string into whitespace-separated words.
fn split_words(text: &str) -> Vec<String> {
    text.split_whitespace().map(|s| s.to_string()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_basic_rule() {
        let rule = Rule::parse("foo.c |> gcc -c foo.c -o foo.o |> foo.o", 1).unwrap();
        assert!(!rule.foreach);
        assert_eq!(rule.inputs, vec!["foo.c"]);
        assert_eq!(rule.command.command, "gcc -c foo.c -o foo.o");
        assert_eq!(rule.outputs, vec!["foo.o"]);
        assert!(rule.order_only_inputs.is_empty());
        assert!(rule.extra_outputs.is_empty());
    }

    #[test]
    fn test_parse_foreach_rule() {
        let rule = Rule::parse("foreach *.c |> gcc -c %f -o %o |> %B.o", 1).unwrap();
        assert!(rule.foreach);
        assert_eq!(rule.inputs, vec!["*.c"]);
        assert_eq!(rule.command.command, "gcc -c %f -o %o");
        assert_eq!(rule.outputs, vec!["%B.o"]);
    }

    #[test]
    fn test_parse_multiple_inputs() {
        let rule = Rule::parse("a.c b.c c.c |> gcc %f -o out |> out", 1).unwrap();
        assert_eq!(rule.inputs, vec!["a.c", "b.c", "c.c"]);
    }

    #[test]
    fn test_parse_order_only_inputs() {
        let rule =
            Rule::parse("main.c | config.h |> gcc -c main.c -o main.o |> main.o", 1).unwrap();
        assert_eq!(rule.inputs, vec!["main.c"]);
        assert_eq!(rule.order_only_inputs, vec!["config.h"]);
    }

    #[test]
    fn test_parse_extra_outputs() {
        let rule = Rule::parse(
            "foo.c |> gcc -c %f -o %o -MD -MF %o.d |> foo.o | foo.o.d",
            1,
        )
        .unwrap();
        assert_eq!(rule.outputs, vec!["foo.o"]);
        assert_eq!(rule.extra_outputs, vec!["foo.o.d"]);
    }

    #[test]
    fn test_parse_command_with_display() {
        let rule = Rule::parse("|> ^CC foo.c^ gcc -c foo.c -o foo.o |> foo.o", 1).unwrap();
        assert_eq!(rule.command.display, Some("CC foo.c".to_string()));
        assert_eq!(rule.command.command, "gcc -c foo.c -o foo.o");
    }

    #[test]
    fn test_parse_command_with_flags_and_display() {
        let rule = Rule::parse("|> ^t CC foo.c^ gcc -c foo.c -o foo.o |> foo.o", 1).unwrap();
        assert_eq!(rule.command.flags, Some("t".to_string()));
        assert_eq!(rule.command.display, Some("CC foo.c".to_string()));
        assert_eq!(rule.command.command, "gcc -c foo.c -o foo.o");
    }

    #[test]
    fn test_parse_empty_input() {
        let rule = Rule::parse("|> echo hello > %o |> greeting.txt", 1).unwrap();
        assert!(rule.inputs.is_empty());
        assert_eq!(rule.command.command, "echo hello > %o");
        assert_eq!(rule.outputs, vec!["greeting.txt"]);
    }

    #[test]
    fn test_parse_too_few_sections() {
        let result = Rule::parse("foo.c |> gcc foo.c", 1);
        assert!(result.is_err());
    }
}
