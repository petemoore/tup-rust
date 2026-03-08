use std::path::Path;

/// Expand glob patterns in a list of input strings.
///
/// Non-glob strings are passed through unchanged.
/// Glob patterns (containing `*`, `?`, or `[`) are expanded against
/// the filesystem relative to `base_dir`.
///
/// Returns the expanded list of file paths.
pub fn expand_globs(patterns: &[String], base_dir: &Path) -> Result<Vec<String>, String> {
    let mut result = Vec::new();

    for pattern in patterns {
        if is_glob(pattern) {
            let matches = glob_match(pattern, base_dir)?;
            if matches.is_empty() {
                // In tup, unmatched globs are silently ignored
                // (they may match after generated files are created)
            }
            result.extend(matches);
        } else {
            result.push(pattern.clone());
        }
    }

    Ok(result)
}

/// Check if a string contains glob metacharacters.
pub fn is_glob(s: &str) -> bool {
    s.contains('*') || s.contains('?') || s.contains('[')
}

/// Match a glob pattern against files in a directory.
fn glob_match(pattern: &str, base_dir: &Path) -> Result<Vec<String>, String> {
    // Split pattern into directory and filename parts
    let (dir_part, file_pattern) = if let Some(pos) = pattern.rfind('/') {
        (&pattern[..pos], &pattern[pos + 1..])
    } else {
        ("", pattern)
    };

    let search_dir = if dir_part.is_empty() {
        base_dir.to_path_buf()
    } else {
        base_dir.join(dir_part)
    };

    if !search_dir.is_dir() {
        return Ok(vec![]);
    }

    let entries = std::fs::read_dir(&search_dir)
        .map_err(|e| format!("failed to read directory {}: {e}", search_dir.display()))?;

    let mut matches = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| format!("failed to read directory entry: {e}"))?;
        let name = entry.file_name().to_string_lossy().to_string();

        // Skip hidden files (starting with .)
        if name.starts_with('.') {
            continue;
        }

        if glob_pattern_match(file_pattern, &name) {
            let path = if dir_part.is_empty() {
                name
            } else {
                format!("{dir_part}/{name}")
            };
            matches.push(path);
        }
    }

    matches.sort();
    Ok(matches)
}

/// Match a simple glob pattern against a string.
///
/// Supports `*` (match any sequence), `?` (match single char),
/// and `[abc]` (match character class).
fn glob_pattern_match(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();

    glob_match_recursive(&p, 0, &t, 0)
}

fn glob_match_recursive(pattern: &[char], pi: usize, text: &[char], ti: usize) -> bool {
    let mut pi = pi;
    let mut ti = ti;

    while pi < pattern.len() {
        match pattern[pi] {
            '*' => {
                pi += 1;
                // Try matching * with 0 to N characters
                for skip in 0..=(text.len() - ti) {
                    if glob_match_recursive(pattern, pi, text, ti + skip) {
                        return true;
                    }
                }
                return false;
            }
            '?' => {
                if ti >= text.len() {
                    return false;
                }
                pi += 1;
                ti += 1;
            }
            '[' => {
                if ti >= text.len() {
                    return false;
                }
                pi += 1;
                let negate = pi < pattern.len() && pattern[pi] == '^';
                if negate {
                    pi += 1;
                }

                let mut matched = false;
                let ch = text[ti];

                while pi < pattern.len() && pattern[pi] != ']' {
                    if pi + 2 < pattern.len() && pattern[pi + 1] == '-' {
                        // Range: [a-z]
                        let lo = pattern[pi];
                        let hi = pattern[pi + 2];
                        if ch >= lo && ch <= hi {
                            matched = true;
                        }
                        pi += 3;
                    } else {
                        if pattern[pi] == ch {
                            matched = true;
                        }
                        pi += 1;
                    }
                }

                if pi < pattern.len() {
                    pi += 1; // skip ']'
                }

                if matched == negate {
                    return false;
                }
                ti += 1;
            }
            c => {
                if ti >= text.len() || text[ti] != c {
                    return false;
                }
                pi += 1;
                ti += 1;
            }
        }
    }

    ti == text.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_glob_pattern_match_star() {
        assert!(glob_pattern_match("*.c", "foo.c"));
        assert!(glob_pattern_match("*.c", "bar.c"));
        assert!(!glob_pattern_match("*.c", "foo.h"));
        assert!(glob_pattern_match("*", "anything"));
        assert!(glob_pattern_match("foo*", "foobar"));
        assert!(glob_pattern_match("*bar", "foobar"));
        assert!(glob_pattern_match("f*r", "foobar"));
    }

    #[test]
    fn test_glob_pattern_match_question() {
        assert!(glob_pattern_match("?.c", "a.c"));
        assert!(!glob_pattern_match("?.c", "ab.c"));
        assert!(glob_pattern_match("??.c", "ab.c"));
    }

    #[test]
    fn test_glob_pattern_match_bracket() {
        assert!(glob_pattern_match("[abc].c", "a.c"));
        assert!(glob_pattern_match("[abc].c", "b.c"));
        assert!(!glob_pattern_match("[abc].c", "d.c"));
    }

    #[test]
    fn test_glob_pattern_match_range() {
        assert!(glob_pattern_match("[a-z].c", "m.c"));
        assert!(!glob_pattern_match("[a-z].c", "M.c"));
    }

    #[test]
    fn test_glob_pattern_match_negate() {
        assert!(!glob_pattern_match("[^abc].c", "a.c"));
        assert!(glob_pattern_match("[^abc].c", "d.c"));
    }

    #[test]
    fn test_glob_pattern_exact() {
        assert!(glob_pattern_match("foo.c", "foo.c"));
        assert!(!glob_pattern_match("foo.c", "bar.c"));
    }

    #[test]
    fn test_glob_pattern_complex() {
        assert!(glob_pattern_match("test_*.c", "test_main.c"));
        assert!(glob_pattern_match("test_*.c", "test_utils.c"));
        assert!(!glob_pattern_match("test_*.c", "main.c"));
    }

    #[test]
    fn test_is_glob() {
        assert!(is_glob("*.c"));
        assert!(is_glob("test?.c"));
        assert!(is_glob("[abc].c"));
        assert!(!is_glob("foo.c"));
        assert!(!is_glob("src/main.c"));
    }

    #[test]
    fn test_expand_globs_filesystem() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("a.c"), "").unwrap();
        std::fs::write(tmp.path().join("b.c"), "").unwrap();
        std::fs::write(tmp.path().join("c.h"), "").unwrap();
        std::fs::write(tmp.path().join(".hidden"), "").unwrap();

        let patterns = vec!["*.c".to_string()];
        let result = expand_globs(&patterns, tmp.path()).unwrap();
        assert_eq!(result, vec!["a.c", "b.c"]);
    }

    #[test]
    fn test_expand_globs_no_match() {
        let tmp = tempfile::tempdir().unwrap();

        let patterns = vec!["*.xyz".to_string()];
        let result = expand_globs(&patterns, tmp.path()).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn test_expand_globs_mixed() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("a.c"), "").unwrap();
        std::fs::write(tmp.path().join("b.c"), "").unwrap();

        let patterns = vec!["explicit.c".to_string(), "*.c".to_string()];
        let result = expand_globs(&patterns, tmp.path()).unwrap();
        assert_eq!(result, vec!["explicit.c", "a.c", "b.c"]);
    }

    #[test]
    fn test_expand_globs_hidden_excluded() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join(".gitignore"), "").unwrap();
        std::fs::write(tmp.path().join("visible.c"), "").unwrap();

        let patterns = vec!["*".to_string()];
        let result = expand_globs(&patterns, tmp.path()).unwrap();
        assert_eq!(result, vec!["visible.c"]);
    }

    #[test]
    fn test_expand_globs_subdir() {
        let tmp = tempfile::tempdir().unwrap();
        let sub = tmp.path().join("src");
        std::fs::create_dir(&sub).unwrap();
        std::fs::write(sub.join("main.c"), "").unwrap();
        std::fs::write(sub.join("util.c"), "").unwrap();

        let patterns = vec!["src/*.c".to_string()];
        let result = expand_globs(&patterns, tmp.path()).unwrap();
        assert_eq!(result, vec!["src/main.c", "src/util.c"]);
    }
}
