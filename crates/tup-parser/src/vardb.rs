use std::collections::BTreeMap;

/// Variable database for the parser.
///
/// Handles variable assignment (`VAR = value`), append (`VAR += value`),
/// and expansion (`$(VAR)`).
#[derive(Debug, Default, Clone)]
pub struct ParseVarDb {
    vars: BTreeMap<String, String>,
    /// Config variables from tup.config, expanded via @(VAR)
    config_vars: BTreeMap<String, String>,
}

impl ParseVarDb {
    pub fn new() -> Self {
        Self::default()
    }

    /// Set a variable (replaces existing value).
    pub fn set(&mut self, name: &str, value: &str) {
        self.vars.insert(name.to_string(), value.to_string());
    }

    /// Append to a variable (space-separated).
    pub fn append(&mut self, name: &str, value: &str) {
        match self.vars.get_mut(name) {
            Some(existing) => {
                if !existing.is_empty() {
                    existing.push(' ');
                }
                existing.push_str(value);
            }
            None => {
                self.vars.insert(name.to_string(), value.to_string());
            }
        }
    }

    /// Get a variable value.
    pub fn get(&self, name: &str) -> Option<&str> {
        self.vars.get(name).map(|s| s.as_str())
    }

    /// Set a config variable (from tup.config).
    pub fn set_config(&mut self, name: &str, value: &str) {
        self.config_vars.insert(name.to_string(), value.to_string());
    }

    /// Get a config variable value.
    pub fn get_config(&self, name: &str) -> Option<&str> {
        self.config_vars.get(name).map(|s| s.as_str())
    }

    /// Get all config variables (for writing vardict).
    pub fn config_vars(&self) -> &BTreeMap<String, String> {
        &self.config_vars
    }

    /// Expand all `$(VAR)` references in a string.
    ///
    /// Returns the expanded string. Unknown variables expand to empty string.
    pub fn expand(&self, input: &str) -> String {
        let mut result = String::with_capacity(input.len());
        let mut chars = input.chars().peekable();

        while let Some(ch) = chars.next() {
            if ch == '\\' && chars.peek() == Some(&'$') {
                // Check if this is \$( — escaped variable reference.
                // In C tup, \$( prevents variable expansion but the \$ is
                // kept in the output (the command string).
                // Plain \$ (not followed by '(') is also preserved as-is.
                let mut lookahead = chars.clone();
                lookahead.next(); // skip '$'
                if lookahead.peek() == Some(&'(') {
                    // \$( → $( in output (skip the backslash, don't expand)
                    chars.next(); // consume '$'
                    result.push('$');
                } else {
                    // \$ not followed by ( — preserve both characters
                    result.push('\\');
                    // Don't consume the '$' — it will be processed next iteration
                }
            } else if ch == '$' && chars.peek() == Some(&'(') {
                chars.next(); // consume '('
                let mut var_name = String::new();
                let mut depth = 1;
                for c in chars.by_ref() {
                    if c == '(' {
                        depth += 1;
                        var_name.push(c);
                    } else if c == ')' {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                        var_name.push(c);
                    } else {
                        var_name.push(c);
                    }
                }
                // Expand the variable
                if let Some(value) = self.vars.get(&var_name) {
                    result.push_str(value);
                }
                // Unknown variables expand to empty string
            } else if ch == '@' && chars.peek() == Some(&'(') {
                // Config variable: @(VAR)
                chars.next(); // consume '('
                let mut var_name = String::new();
                for c in chars.by_ref() {
                    if c == ')' {
                        break;
                    }
                    var_name.push(c);
                }
                if let Some(value) = self.config_vars.get(&var_name) {
                    result.push_str(value);
                }
                // Unknown config variables expand to empty string
            } else {
                result.push(ch);
            }
        }

        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_set_and_get() {
        let mut vdb = ParseVarDb::new();
        vdb.set("CC", "gcc");
        assert_eq!(vdb.get("CC"), Some("gcc"));
    }

    #[test]
    fn test_append() {
        let mut vdb = ParseVarDb::new();
        vdb.set("CFLAGS", "-Wall");
        vdb.append("CFLAGS", "-O2");
        assert_eq!(vdb.get("CFLAGS"), Some("-Wall -O2"));
    }

    #[test]
    fn test_expand_simple() {
        let mut vdb = ParseVarDb::new();
        vdb.set("CC", "gcc");
        assert_eq!(vdb.expand("$(CC) -c foo.c"), "gcc -c foo.c");
    }

    #[test]
    fn test_expand_multiple() {
        let mut vdb = ParseVarDb::new();
        vdb.set("CC", "gcc");
        vdb.set("CFLAGS", "-Wall -O2");
        assert_eq!(vdb.expand("$(CC) $(CFLAGS) -c foo.c"), "gcc -Wall -O2 -c foo.c");
    }

    #[test]
    fn test_expand_unknown() {
        let vdb = ParseVarDb::new();
        assert_eq!(vdb.expand("$(UNKNOWN)"), "");
    }

    #[test]
    fn test_expand_no_vars() {
        let vdb = ParseVarDb::new();
        assert_eq!(vdb.expand("hello world"), "hello world");
    }

    #[test]
    fn test_expand_dollar_without_paren() {
        let vdb = ParseVarDb::new();
        assert_eq!(vdb.expand("$5 price"), "$5 price");
    }
}
