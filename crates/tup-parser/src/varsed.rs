use std::collections::BTreeMap;

/// Perform @-variable substitution on file contents.
///
/// Replaces all occurrences of `@VARIABLE@` with the corresponding
/// value from the variables map. Unknown variables are replaced with
/// empty strings.
///
/// This implements the `varsed` command used in tup Tupfiles.
pub fn varsed(input: &str, variables: &BTreeMap<String, String>) -> String {
    let mut result = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '@' {
            // Try to read a variable name
            let mut var_name = String::new();
            let mut found_end = false;

            for c in chars.by_ref() {
                if c == '@' {
                    found_end = true;
                    break;
                }
                if !c.is_alphanumeric() && c != '_' {
                    // Not a valid variable character — put everything back
                    result.push('@');
                    result.push_str(&var_name);
                    result.push(c);
                    found_end = false;
                    break;
                }
                var_name.push(c);
            }

            if found_end {
                // Look up the variable
                if let Some(value) = variables.get(&var_name) {
                    result.push_str(value);
                }
                // Unknown variables expand to empty string
            } else if var_name.is_empty() {
                // Just a lone @ at end of input
                result.push('@');
            } else {
                // Hit end of input without closing @
                // Put back the @ and accumulated chars
                result.push('@');
                result.push_str(&var_name);
            }
        } else {
            result.push(ch);
        }
    }

    result
}

/// Perform varsed substitution reading from a file and writing to another.
pub fn varsed_file(
    input_path: &std::path::Path,
    output_path: &std::path::Path,
    variables: &BTreeMap<String, String>,
) -> Result<(), String> {
    let input = std::fs::read_to_string(input_path)
        .map_err(|e| format!("failed to read {}: {e}", input_path.display()))?;

    let output = varsed(&input, variables);

    std::fs::write(output_path, &output)
        .map_err(|e| format!("failed to write {}: {e}", output_path.display()))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_vars(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn test_varsed_simple() {
        let vars = make_vars(&[("NAME", "world")]);
        assert_eq!(varsed("Hello @NAME@!", &vars), "Hello world!");
    }

    #[test]
    fn test_varsed_multiple() {
        let vars = make_vars(&[("CC", "gcc"), ("FLAGS", "-O2")]);
        assert_eq!(varsed("@CC@ @FLAGS@", &vars), "gcc -O2");
    }

    #[test]
    fn test_varsed_unknown() {
        let vars = make_vars(&[]);
        assert_eq!(varsed("@UNKNOWN@", &vars), "");
    }

    #[test]
    fn test_varsed_no_vars() {
        let vars = make_vars(&[]);
        assert_eq!(varsed("hello world", &vars), "hello world");
    }

    #[test]
    fn test_varsed_at_in_text() {
        let vars = make_vars(&[]);
        // Single @ without matching end
        assert_eq!(varsed("email@host", &vars), "email@host");
    }

    #[test]
    fn test_varsed_empty_var() {
        let vars = make_vars(&[]);
        // @@ is an empty variable name — expands to empty
        assert_eq!(varsed("@@", &vars), "");
    }

    #[test]
    fn test_varsed_multiline() {
        let vars = make_vars(&[("VERSION", "1.0"), ("NAME", "myapp")]);
        let input = "Name: @NAME@\nVersion: @VERSION@\n";
        let expected = "Name: myapp\nVersion: 1.0\n";
        assert_eq!(varsed(input, &vars), expected);
    }

    #[test]
    fn test_varsed_adjacent() {
        let vars = make_vars(&[("A", "x"), ("B", "y")]);
        assert_eq!(varsed("@A@@B@", &vars), "xy");
    }

    #[test]
    fn test_varsed_underscore_in_name() {
        let vars = make_vars(&[("MY_VAR", "value")]);
        assert_eq!(varsed("@MY_VAR@", &vars), "value");
    }

    #[test]
    fn test_varsed_file_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let input_path = tmp.path().join("input.txt");
        let output_path = tmp.path().join("output.txt");

        std::fs::write(&input_path, "Hello @NAME@!").unwrap();
        let vars = make_vars(&[("NAME", "tup")]);

        varsed_file(&input_path, &output_path, &vars).unwrap();
        assert_eq!(std::fs::read_to_string(&output_path).unwrap(), "Hello tup!");
    }
}
