/// The type of dependency link between two nodes.
///
/// Values must match the C enum for database compatibility.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(i32)]
pub enum LinkType {
    /// Regular dependency link (input was read by command).
    Normal = 1,
    /// Sticky link (explicitly declared in Tupfile, always in input list).
    Sticky = 2,
    /// Group link (associates output with an output group via a command).
    Group = 3,
}

impl LinkType {
    /// Convert from an i32 database value.
    pub fn from_i32(value: i32) -> Option<Self> {
        match value {
            1 => Some(Self::Normal),
            2 => Some(Self::Sticky),
            3 => Some(Self::Group),
            _ => None,
        }
    }

    /// Convert to an i32 for database storage.
    pub fn as_i32(self) -> i32 {
        self as i32
    }

    /// Get the database table name for this link type.
    pub fn table_name(self) -> &'static str {
        match self {
            Self::Normal => "normal_link",
            Self::Sticky => "sticky_link",
            Self::Group => "group_link",
        }
    }
}

impl std::fmt::Display for LinkType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Normal => f.write_str("normal"),
            Self::Sticky => f.write_str("sticky"),
            Self::Group => f.write_str("group"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_link_type_values() {
        assert_eq!(LinkType::Normal.as_i32(), 1);
        assert_eq!(LinkType::Sticky.as_i32(), 2);
        assert_eq!(LinkType::Group.as_i32(), 3);
    }

    #[test]
    fn test_link_type_roundtrip() {
        for i in 1..=3 {
            let lt = LinkType::from_i32(i).unwrap();
            assert_eq!(lt.as_i32(), i);
        }
    }

    #[test]
    fn test_link_type_invalid() {
        assert!(LinkType::from_i32(0).is_none());
        assert!(LinkType::from_i32(4).is_none());
        assert!(LinkType::from_i32(-1).is_none());
    }

    #[test]
    fn test_link_type_table_name() {
        assert_eq!(LinkType::Normal.table_name(), "normal_link");
        assert_eq!(LinkType::Sticky.table_name(), "sticky_link");
        assert_eq!(LinkType::Group.table_name(), "group_link");
    }
}
