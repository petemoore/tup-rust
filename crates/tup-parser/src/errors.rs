/// Errors that can occur during Tupfile parsing.
#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    #[error("{file}:{line}: {message}")]
    Syntax {
        file: String,
        line: usize,
        message: String,
    },

    #[error("{file}: missing endif before EOF")]
    MissingEndif { file: String },

    #[error("{file}:{line}: {message}")]
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
