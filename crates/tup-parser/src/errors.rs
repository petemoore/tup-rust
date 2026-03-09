/// Errors that can occur during Tupfile parsing.
#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    #[error("Syntax error parsing {file} line {line}\n  {message}")]
    Syntax {
        file: String,
        line: usize,
        message: String,
    },

    #[error("Error parsing {file} line {line}\n  {message}")]
    RuleParse {
        file: String,
        line: usize,
        message: String,
    },

    #[error("Error parsing {file}: missing endif before EOF.")]
    MissingEndif { file: String },

    #[error("Error parsing {file} line {line}\n  {message}")]
    ErrorDirective {
        file: String,
        line: usize,
        message: String,
    },

    #[error("I/O error reading {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },

    #[error("{0}")]
    Other(String),
}
