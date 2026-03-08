/// The type of file access event tracked by the server/LD_PRELOAD.
///
/// Values must match the C enum for protocol compatibility.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(i32)]
pub enum AccessType {
    /// File was read.
    Read = 0,
    /// File was written.
    Write = 1,
    /// File was renamed (has a second path).
    Rename = 2,
    /// File was deleted.
    Unlink = 3,
    /// Variable was accessed.
    Var = 4,
}

impl AccessType {
    /// Convert from an i32 protocol value.
    pub fn from_i32(value: i32) -> Option<Self> {
        match value {
            0 => Some(Self::Read),
            1 => Some(Self::Write),
            2 => Some(Self::Rename),
            3 => Some(Self::Unlink),
            4 => Some(Self::Var),
            _ => None,
        }
    }

    /// Convert to an i32 for protocol use.
    pub fn as_i32(self) -> i32 {
        self as i32
    }
}

impl std::fmt::Display for AccessType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Read => f.write_str("read"),
            Self::Write => f.write_str("write"),
            Self::Rename => f.write_str("rename"),
            Self::Unlink => f.write_str("unlink"),
            Self::Var => f.write_str("var"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_access_type_values() {
        assert_eq!(AccessType::Read.as_i32(), 0);
        assert_eq!(AccessType::Write.as_i32(), 1);
        assert_eq!(AccessType::Rename.as_i32(), 2);
        assert_eq!(AccessType::Unlink.as_i32(), 3);
        assert_eq!(AccessType::Var.as_i32(), 4);
    }

    #[test]
    fn test_access_type_roundtrip() {
        for i in 0..=4 {
            let at = AccessType::from_i32(i).unwrap();
            assert_eq!(at.as_i32(), i);
        }
    }

    #[test]
    fn test_access_type_invalid() {
        assert!(AccessType::from_i32(-1).is_none());
        assert!(AccessType::from_i32(5).is_none());
    }
}
