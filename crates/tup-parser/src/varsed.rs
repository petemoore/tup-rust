use std::collections::BTreeMap;
use std::io::Read;

/// Perform @-variable substitution on file contents.
///
/// Replaces all occurrences of `@VARIABLE@` with the corresponding
/// value from the variables map. Unknown variables are replaced with
/// empty strings.
///
/// This implements the `varsed` command used in tup Tupfiles.
pub fn varsed(input: &str, variables: &BTreeMap<String, String>) -> String {
    varsed_impl(input, variables, false)
}

/// Perform @-variable substitution with optional binary mode.
///
/// In binary mode, single-character values 'y' are replaced with '1'
/// and 'n' with '0'. This matches the C tup `--binary` flag behavior.
pub fn varsed_binary(input: &str, variables: &BTreeMap<String, String>, binmode: bool) -> String {
    varsed_impl(input, variables, binmode)
}

/// Core implementation of varsed, ported from C varsed.c:var_replace().
///
/// Algorithm: scan for '@', then scan for alphanumeric/underscore chars
/// until another '@'. If found, look up the variable and substitute.
/// If not found (non-alnum char before closing '@'), write the literal text.
fn varsed_impl(input: &str, variables: &BTreeMap<String, String>, binmode: bool) -> String {
    let bytes = input.as_bytes();
    let len = bytes.len();
    let mut result = String::with_capacity(len);
    let mut pos = 0;

    while pos < len {
        // Find next '@'
        let at_pos = match bytes[pos..].iter().position(|&b| b == b'@') {
            Some(offset) => pos + offset,
            None => {
                // No more '@' - copy rest and done
                result.push_str(&input[pos..]);
                break;
            }
        };

        // Write everything before the '@'
        result.push_str(&input[pos..at_pos]);

        // Scan for variable name (alphanumeric or underscore)
        let var_start = at_pos + 1;
        let mut var_end = var_start;
        while var_end < len && (bytes[var_end].is_ascii_alphanumeric() || bytes[var_end] == b'_') {
            var_end += 1;
        }

        if var_end < len && bytes[var_end] == b'@' {
            // Found closing '@' - look up variable
            let var_name = &input[var_start..var_end];
            if let Some(value) = variables.get(var_name) {
                if binmode && value.len() == 1 {
                    match value.as_str() {
                        "y" => result.push('1'),
                        "n" => result.push('0'),
                        _ => result.push_str(value),
                    }
                } else {
                    result.push_str(value);
                }
            }
            // Unknown variables expand to empty string (matching C behavior)
            pos = var_end + 1;
        } else {
            // No closing '@' with valid var chars - write literal text
            result.push_str(&input[at_pos..var_end]);
            pos = var_end;
        }
    }

    result
}

/// Parse a binary vardict file (as written by C tup's save_vardict_file).
///
/// Format:
/// - 4 bytes: num_entries (u32 LE)
/// - num_entries * 4 bytes: offsets (u32 LE array)
/// - entries data: sorted KEY=VALUE strings (null-terminated or delimited by offsets)
pub fn parse_vardict_binary(data: &[u8]) -> BTreeMap<String, String> {
    let mut vars = BTreeMap::new();

    if data.len() < 4 {
        return vars;
    }

    let num_entries = u32::from_ne_bytes([data[0], data[1], data[2], data[3]]) as usize;
    let header_size = 4 + num_entries * 4;

    if data.len() < header_size {
        return vars;
    }

    // Read offsets
    let offsets: Vec<usize> = (0..num_entries)
        .map(|i| {
            let off = 4 + i * 4;
            u32::from_ne_bytes([data[off], data[off + 1], data[off + 2], data[off + 3]]) as usize
        })
        .collect();

    let entries_base = header_size;

    for (i, &offset) in offsets.iter().enumerate() {
        let start = entries_base + offset;
        if start >= data.len() {
            continue;
        }
        // Find end of this entry: either next entry's offset or end of data
        let end = if i + 1 < num_entries {
            let next_start = entries_base + offsets[i + 1];
            next_start.min(data.len())
        } else {
            data.len()
        };

        // Entry is KEY=VALUE (may have trailing null)
        let entry_bytes = &data[start..end];
        let entry_str = match std::str::from_utf8(entry_bytes) {
            Ok(s) => s.trim_end_matches('\0'),
            Err(_) => continue,
        };

        if let Some(eq_pos) = entry_str.find('=') {
            let key = &entry_str[..eq_pos];
            let value = &entry_str[eq_pos + 1..];
            vars.insert(key.to_string(), value.to_string());
        }
    }

    vars
}

/// Parse a text vardict file (as written by our Rust write_vardict).
///
/// Format: one KEY=VALUE per line.
pub fn parse_vardict_text(data: &str) -> BTreeMap<String, String> {
    let mut vars = BTreeMap::new();
    for line in data.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(eq_pos) = line.find('=') {
            let key = &line[..eq_pos];
            let value = &line[eq_pos + 1..];
            vars.insert(key.to_string(), value.to_string());
        }
    }
    vars
}

/// Load variables from the vardict, checking `tup_vardict` env var first,
/// then falling back to `.tup/vardict` in the given tup root.
///
/// The env var path may point to either a binary vardict (C tup) or a text
/// vardict (our Rust code). We detect binary format by checking if the file
/// starts with a valid u32 entry count.
pub fn load_vardict(tup_root: Option<&std::path::Path>) -> BTreeMap<String, String> {
    // Check for tup_vardict env var (set during command execution)
    if let Ok(path) = std::env::var("tup_vardict") {
        if let Ok(data) = std::fs::read(&path) {
            if is_binary_vardict(&data) {
                return parse_vardict_binary(&data);
            }
            // Try as text
            if let Ok(text) = String::from_utf8(data) {
                return parse_vardict_text(&text);
            }
        }
        // If env var set but file missing/empty, return empty (matching C)
        return BTreeMap::new();
    }

    // Fall back to .tup/vardict in tup root
    if let Some(root) = tup_root {
        let vardict_path = root.join(".tup").join("vardict");
        if let Ok(text) = std::fs::read_to_string(&vardict_path) {
            return parse_vardict_text(&text);
        }
    }

    BTreeMap::new()
}

/// Heuristic to detect binary vardict format.
/// Binary format starts with a u32 entry count, then u32 offsets.
/// Text format starts with printable ASCII (variable names).
fn is_binary_vardict(data: &[u8]) -> bool {
    if data.len() < 4 {
        return false;
    }
    // If first byte is a printable ASCII char that could start a variable name,
    // it's probably text format
    if data[0].is_ascii_alphanumeric() || data[0] == b'_' || data[0] == b'#' {
        return false;
    }
    // Otherwise assume binary
    true
}

/// Run the varsed command, matching C tup's varsed() in varsed.c.
///
/// Reads input from `input` (file path or "-" for stdin),
/// writes output to `output` (file path or "-" for stdout),
/// with optional binary mode.
pub fn cmd_varsed(
    input: &str,
    output: &str,
    binmode: bool,
    tup_root: Option<&std::path::Path>,
) -> Result<(), String> {
    let variables = load_vardict(tup_root);

    // Read input
    let input_data = if input == "-" {
        let mut buf = String::new();
        std::io::stdin()
            .read_to_string(&mut buf)
            .map_err(|e| format!("Error reading stdin: {e}"))?;
        buf
    } else {
        std::fs::read_to_string(input).map_err(|e| {
            eprintln!("Error opening input file.");
            format!("{input}: {e}")
        })?
    };

    // Perform substitution
    let output_data = varsed_binary(&input_data, &variables, binmode);

    // Write output
    if output == "-" {
        use std::io::Write;
        std::io::stdout()
            .write_all(output_data.as_bytes())
            .map_err(|e| format!("write: {e}"))?;
    } else {
        std::fs::write(output, &output_data).map_err(|e| {
            eprintln!("Error creating output file.");
            format!("{output}: {e}")
        })?
    };

    Ok(())
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

    #[test]
    fn test_varsed_binary_mode_y() {
        let vars = make_vars(&[("FEATURE", "y")]);
        assert_eq!(
            varsed_binary("#define FEATURE @FEATURE@", &vars, true),
            "#define FEATURE 1"
        );
    }

    #[test]
    fn test_varsed_binary_mode_n() {
        let vars = make_vars(&[("FEATURE", "n")]);
        assert_eq!(
            varsed_binary("#define FEATURE @FEATURE@", &vars, true),
            "#define FEATURE 0"
        );
    }

    #[test]
    fn test_varsed_binary_mode_multi_char_unchanged() {
        let vars = make_vars(&[("VAL", "yes")]);
        // Multi-char values are NOT converted in binary mode
        assert_eq!(varsed_binary("@VAL@", &vars, true), "yes");
    }

    #[test]
    fn test_varsed_binary_mode_off() {
        let vars = make_vars(&[("FEATURE", "y")]);
        // Without binary mode, 'y' stays as 'y'
        assert_eq!(varsed_binary("@FEATURE@", &vars, false), "y");
    }

    #[test]
    fn test_parse_vardict_text() {
        let text = "CC=gcc\nFLAGS=-O2\nNAME=hello world\n";
        let vars = parse_vardict_text(text);
        assert_eq!(vars.get("CC").map(|s| s.as_str()), Some("gcc"));
        assert_eq!(vars.get("FLAGS").map(|s| s.as_str()), Some("-O2"));
        assert_eq!(vars.get("NAME").map(|s| s.as_str()), Some("hello world"));
    }

    #[test]
    fn test_parse_vardict_text_empty() {
        let vars = parse_vardict_text("");
        assert!(vars.is_empty());
    }

    #[test]
    fn test_parse_vardict_text_blank_lines() {
        let text = "\nCC=gcc\n\nFLAGS=-O2\n\n";
        let vars = parse_vardict_text(text);
        assert_eq!(vars.len(), 2);
    }

    #[test]
    fn test_cmd_varsed_file_to_file() {
        let tmp = tempfile::tempdir().unwrap();
        let tup_dir = tmp.path().join(".tup");
        std::fs::create_dir_all(&tup_dir).unwrap();

        // Write a text vardict
        std::fs::write(tup_dir.join("vardict"), "NAME=tup\nVERSION=1.0\n").unwrap();

        let input_path = tmp.path().join("input.txt");
        let output_path = tmp.path().join("output.txt");
        std::fs::write(&input_path, "@NAME@ version @VERSION@").unwrap();

        cmd_varsed(
            input_path.to_str().unwrap(),
            output_path.to_str().unwrap(),
            false,
            Some(tmp.path()),
        )
        .unwrap();

        assert_eq!(
            std::fs::read_to_string(&output_path).unwrap(),
            "tup version 1.0"
        );
    }

    #[test]
    fn test_cmd_varsed_binary_mode() {
        let tmp = tempfile::tempdir().unwrap();
        let tup_dir = tmp.path().join(".tup");
        std::fs::create_dir_all(&tup_dir).unwrap();

        std::fs::write(
            tup_dir.join("vardict"),
            "FEATURE_A=y\nFEATURE_B=n\nFEATURE_C=yes\n",
        )
        .unwrap();

        let input_path = tmp.path().join("input.txt");
        let output_path = tmp.path().join("output.txt");
        std::fs::write(&input_path, "A=@FEATURE_A@ B=@FEATURE_B@ C=@FEATURE_C@").unwrap();

        cmd_varsed(
            input_path.to_str().unwrap(),
            output_path.to_str().unwrap(),
            true,
            Some(tmp.path()),
        )
        .unwrap();

        assert_eq!(
            std::fs::read_to_string(&output_path).unwrap(),
            "A=1 B=0 C=yes"
        );
    }

    #[test]
    fn test_is_binary_vardict() {
        // Text format starts with ASCII
        assert!(!is_binary_vardict(b"CC=gcc\n"));
        assert!(!is_binary_vardict(b"FEATURE=y\n"));

        // Binary format starts with entry count (small number as u32)
        let mut binary_data = Vec::new();
        binary_data.extend_from_slice(&2u32.to_ne_bytes()); // 2 entries
        assert!(is_binary_vardict(&binary_data));

        // Empty or too short
        assert!(!is_binary_vardict(b""));
        assert!(!is_binary_vardict(b"ab"));
    }
}
