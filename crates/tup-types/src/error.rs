use std::fmt;

/// Errors that can occur in the tup build system.
#[derive(Debug)]
pub enum TupError {
    /// Database operation failed.
    Database(String),
    /// Tupfile parsing error.
    Parse {
        file: String,
        line: usize,
        message: String,
    },
    /// Circular dependency detected in the build graph.
    CircularDependency(String),
    /// File system operation failed.
    Io(std::io::Error),
    /// Node type mismatch or invalid type.
    InvalidNodeType(i32),
    /// Invalid link type.
    InvalidLinkType(i32),
    /// Node not found.
    NodeNotFound(crate::TupId),
    /// Build command failed.
    CommandFailed { command: String, exit_code: i32 },
    /// Generic error with message.
    Other(String),
}

impl fmt::Display for TupError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Database(msg) => write!(f, "database error: {msg}"),
            Self::Parse {
                file,
                line,
                message,
            } => {
                write!(f, "{file}:{line}: {message}")
            }
            Self::CircularDependency(msg) => {
                write!(f, "circular dependency: {msg}")
            }
            Self::Io(err) => write!(f, "I/O error: {err}"),
            Self::InvalidNodeType(val) => write!(f, "invalid node type: {val}"),
            Self::InvalidLinkType(val) => write!(f, "invalid link type: {val}"),
            Self::NodeNotFound(id) => write!(f, "node not found: {id}"),
            Self::CommandFailed { command, exit_code } => {
                write!(f, "command failed (exit {exit_code}): {command}")
            }
            Self::Other(msg) => f.write_str(msg),
        }
    }
}

impl std::error::Error for TupError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(err) => Some(err),
            _ => None,
        }
    }
}

impl From<std::io::Error> for TupError {
    fn from(err: std::io::Error) -> Self {
        Self::Io(err)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_display() {
        let err = TupError::Database("connection failed".to_string());
        assert_eq!(format!("{err}"), "database error: connection failed");

        let err = TupError::Parse {
            file: "Tupfile".to_string(),
            line: 10,
            message: "syntax error".to_string(),
        };
        assert_eq!(format!("{err}"), "Tupfile:10: syntax error");

        let err = TupError::InvalidNodeType(99);
        assert_eq!(format!("{err}"), "invalid node type: 99");
    }

    #[test]
    fn test_error_from_io() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file not found");
        let tup_err: TupError = io_err.into();
        assert!(matches!(tup_err, TupError::Io(_)));
    }
}
