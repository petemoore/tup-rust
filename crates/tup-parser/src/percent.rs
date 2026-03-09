use std::path::Path;

/// Input file metadata used for percent substitution.
#[derive(Debug, Clone)]
pub struct InputFile {
    /// Full path relative to the rule directory.
    pub path: String,
    /// Basename (filename without directory).
    pub base: String,
    /// Basename without extension.
    pub base_no_ext: String,
    /// File extension (without dot), if any.
    pub ext: String,
    /// Directory portion of the path.
    pub dir: String,
}

impl InputFile {
    /// Create an InputFile from a path string.
    pub fn new(path: &str) -> Self {
        let p = Path::new(path);
        let base = p
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        let base_no_ext = p
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        let ext = p
            .extension()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        let dir = p
            .parent()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();

        InputFile {
            path: path.to_string(),
            base,
            base_no_ext,
            ext,
            dir,
        }
    }
}

/// Expand percent substitutions in a command string.
///
/// Corresponds to `tup_printf()` in C's parser.c.
///
/// Supported substitutions:
/// - `%f` — all input file paths (space-separated)
/// - `%b` — basenames of all inputs
/// - `%B` — basenames without extensions
/// - `%e` — file extension (in foreach rules, from the current input)
/// - `%d` — directory name (last path component of the rule directory)
/// - `%o` — all output file paths (space-separated)
/// - `%O` — first output without extension
/// - `%i` — order-only input paths
/// - `%%` — literal `%`
pub fn expand_percent(
    command: &str,
    inputs: &[InputFile],
    outputs: &[String],
    order_only: &[String],
    dir_name: &str,
) -> String {
    let mut result = String::with_capacity(command.len() * 2);
    let mut chars = command.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '%' {
            match chars.peek() {
                Some('%') => {
                    chars.next();
                    result.push('%');
                }
                Some('f') => {
                    chars.next();
                    let paths: Vec<&str> = inputs.iter().map(|i| i.path.as_str()).collect();
                    result.push_str(&paths.join(" "));
                }
                Some('b') => {
                    chars.next();
                    let bases: Vec<&str> = inputs.iter().map(|i| i.base.as_str()).collect();
                    result.push_str(&bases.join(" "));
                }
                Some('B') => {
                    chars.next();
                    let stems: Vec<&str> = inputs.iter().map(|i| i.base_no_ext.as_str()).collect();
                    result.push_str(&stems.join(" "));
                }
                Some('e') => {
                    chars.next();
                    // Extension of the first input (for foreach rules)
                    if let Some(first) = inputs.first() {
                        result.push_str(&first.ext);
                    }
                }
                Some('d') => {
                    chars.next();
                    result.push_str(dir_name);
                }
                Some('o') => {
                    chars.next();
                    result.push_str(&outputs.join(" "));
                }
                Some('O') => {
                    chars.next();
                    // First output without extension
                    if let Some(first) = outputs.first() {
                        let p = Path::new(first.as_str());
                        if let Some(stem) = p.file_stem() {
                            if let Some(parent) = p.parent() {
                                if parent.as_os_str().is_empty() {
                                    result.push_str(&stem.to_string_lossy());
                                } else {
                                    result.push_str(&parent.to_string_lossy());
                                    result.push('/');
                                    result.push_str(&stem.to_string_lossy());
                                }
                            } else {
                                result.push_str(&stem.to_string_lossy());
                            }
                        }
                    }
                }
                Some('i') => {
                    chars.next();
                    result.push_str(&order_only.join(" "));
                }
                Some(&c) => {
                    // Unknown percent code — keep as-is
                    result.push('%');
                    result.push(c);
                    chars.next();
                }
                None => {
                    result.push('%');
                }
            }
        } else {
            result.push(ch);
        }
    }

    result
}

/// Expand percent substitutions in output patterns.
///
/// Output patterns like `%B.o` are expanded based on the current input.
/// This is used for foreach rules where each input produces a different output.
pub fn expand_output_pattern(pattern: &str, input: &InputFile) -> String {
    let mut result = String::with_capacity(pattern.len() * 2);
    let mut chars = pattern.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '%' {
            match chars.peek() {
                Some('%') => {
                    chars.next();
                    result.push('%');
                }
                Some('B') => {
                    chars.next();
                    result.push_str(&input.base_no_ext);
                }
                Some('b') => {
                    chars.next();
                    result.push_str(&input.base);
                }
                Some('e') => {
                    chars.next();
                    result.push_str(&input.ext);
                }
                Some('d') => {
                    chars.next();
                    result.push_str(&input.dir);
                }
                Some('f') => {
                    chars.next();
                    result.push_str(&input.path);
                }
                Some(&c) => {
                    result.push('%');
                    result.push(c);
                    chars.next();
                }
                None => {
                    result.push('%');
                }
            }
        } else {
            result.push(ch);
        }
    }

    result
}

#[cfg(test)]
#[allow(non_snake_case)]
mod tests {
    use super::*;

    fn make_inputs(paths: &[&str]) -> Vec<InputFile> {
        paths.iter().map(|p| InputFile::new(p)).collect()
    }

    #[test]
    fn test_input_file_simple() {
        let f = InputFile::new("foo.c");
        assert_eq!(f.path, "foo.c");
        assert_eq!(f.base, "foo.c");
        assert_eq!(f.base_no_ext, "foo");
        assert_eq!(f.ext, "c");
        assert_eq!(f.dir, "");
    }

    #[test]
    fn test_input_file_with_dir() {
        let f = InputFile::new("src/lib/main.c");
        assert_eq!(f.path, "src/lib/main.c");
        assert_eq!(f.base, "main.c");
        assert_eq!(f.base_no_ext, "main");
        assert_eq!(f.ext, "c");
        assert_eq!(f.dir, "src/lib");
    }

    #[test]
    fn test_input_file_no_extension() {
        let f = InputFile::new("Makefile");
        assert_eq!(f.base, "Makefile");
        assert_eq!(f.base_no_ext, "Makefile");
        assert_eq!(f.ext, "");
    }

    #[test]
    fn test_expand_percent_f() {
        let inputs = make_inputs(&["a.c", "b.c", "c.c"]);
        let result = expand_percent("gcc %f -o out", &inputs, &[], &[], ".");
        assert_eq!(result, "gcc a.c b.c c.c -o out");
    }

    #[test]
    fn test_expand_percent_b() {
        let inputs = make_inputs(&["src/a.c", "src/b.c"]);
        let result = expand_percent("gcc %b", &inputs, &[], &[], ".");
        assert_eq!(result, "gcc a.c b.c");
    }

    #[test]
    fn test_expand_percent_B() {
        let inputs = make_inputs(&["foo.c", "bar.c"]);
        let result = expand_percent("%B.o", &inputs, &[], &[], ".");
        assert_eq!(result, "foo bar.o");
    }

    #[test]
    fn test_expand_percent_o() {
        let inputs = make_inputs(&["foo.c"]);
        let outputs = vec!["foo.o".to_string()];
        let result = expand_percent("gcc -c %f -o %o", &inputs, &outputs, &[], ".");
        assert_eq!(result, "gcc -c foo.c -o foo.o");
    }

    #[test]
    fn test_expand_percent_O() {
        let outputs = vec!["build/foo.o".to_string()];
        let result = expand_percent("%O.d", &[], &outputs, &[], ".");
        assert_eq!(result, "build/foo.d");
    }

    #[test]
    fn test_expand_percent_e() {
        let inputs = make_inputs(&["test.cpp"]);
        let result = expand_percent("ext=%e", &inputs, &[], &[], ".");
        assert_eq!(result, "ext=cpp");
    }

    #[test]
    fn test_expand_percent_d() {
        let result = expand_percent("dir=%d", &[], &[], &[], "myproject");
        assert_eq!(result, "dir=myproject");
    }

    #[test]
    fn test_expand_percent_i() {
        let oo = vec!["config.h".to_string(), "types.h".to_string()];
        let result = expand_percent("gcc %f %i", &[], &[], &oo, ".");
        assert_eq!(result, "gcc  config.h types.h");
    }

    #[test]
    fn test_expand_literal_percent() {
        let result = expand_percent("echo 100%%", &[], &[], &[], ".");
        assert_eq!(result, "echo 100%");
    }

    #[test]
    fn test_expand_full_command() {
        let inputs = make_inputs(&["main.c"]);
        let outputs = vec!["main.o".to_string()];
        let result = expand_percent("gcc -c %f -o %o -MD -MF %O.d", &inputs, &outputs, &[], ".");
        assert_eq!(result, "gcc -c main.c -o main.o -MD -MF main.d");
    }

    #[test]
    fn test_expand_output_pattern_B() {
        let input = InputFile::new("src/hello.c");
        let result = expand_output_pattern("%B.o", &input);
        assert_eq!(result, "hello.o");
    }

    #[test]
    fn test_expand_output_pattern_b() {
        let input = InputFile::new("hello.c");
        let result = expand_output_pattern("%b.bak", &input);
        assert_eq!(result, "hello.c.bak");
    }

    #[test]
    fn test_expand_no_inputs() {
        let result = expand_percent("echo hello > %o", &[], &["out.txt".to_string()], &[], ".");
        assert_eq!(result, "echo hello > out.txt");
    }

    #[test]
    fn test_expand_unknown_percent() {
        let result = expand_percent("%z test", &[], &[], &[], ".");
        assert_eq!(result, "%z test");
    }

    #[test]
    fn test_expand_trailing_percent() {
        let result = expand_percent("test%", &[], &[], &[], ".");
        assert_eq!(result, "test%");
    }

    #[test]
    fn test_foreach_output_expansion() {
        // Simulate foreach: for each .c input, produce a .o output
        let inputs = vec![
            InputFile::new("a.c"),
            InputFile::new("b.c"),
            InputFile::new("sub/c.c"),
        ];

        for input in &inputs {
            let output = expand_output_pattern("%B.o", input);
            let cmd = expand_percent(
                "gcc -c %f -o %o",
                std::slice::from_ref(input),
                std::slice::from_ref(&output),
                &[],
                ".",
            );
            assert!(cmd.contains(&input.path));
            assert!(cmd.contains(&output));
        }
    }
}
