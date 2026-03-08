/// Flags indicating the state/status of a node in the build system.
///
/// These are stored as a bitmask in the C implementation, with separate
/// database tables (config_list, create_list, etc.) tracking membership.
/// Values must match the C enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(i32)]
pub enum TupFlags {
    /// No flags set.
    None = 0,
    /// Node needs modification/re-execution.
    Modify = 1,
    /// Node needs creation/parsing (directory Tupfiles).
    Create = 2,
    /// Config-related node.
    Config = 4,
    /// Variant-related node.
    Variant = 8,
    /// Transient output (deleted after downstream consumers finish).
    Transient = 16,
}

impl TupFlags {
    /// Convert from an i32 database value.
    pub fn from_i32(value: i32) -> Option<Self> {
        match value {
            0 => Some(Self::None),
            1 => Some(Self::Modify),
            2 => Some(Self::Create),
            4 => Some(Self::Config),
            8 => Some(Self::Variant),
            16 => Some(Self::Transient),
            _ => None,
        }
    }

    /// Convert to an i32 for database storage.
    pub fn as_i32(self) -> i32 {
        self as i32
    }

    /// Get the database table name for this flag type.
    pub fn table_name(self) -> Option<&'static str> {
        match self {
            Self::None => None,
            Self::Modify => Some("modify_list"),
            Self::Create => Some("create_list"),
            Self::Config => Some("config_list"),
            Self::Variant => Some("variant_list"),
            Self::Transient => Some("transient_list"),
        }
    }
}

/// A bitmask of multiple TupFlags.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FlagSet(i32);

impl FlagSet {
    /// Create an empty flag set.
    pub fn empty() -> Self {
        FlagSet(0)
    }

    /// Create a flag set from a raw bitmask.
    pub fn from_raw(bits: i32) -> Self {
        FlagSet(bits)
    }

    /// Get the raw bitmask value.
    pub fn raw(self) -> i32 {
        self.0
    }

    /// Check if a specific flag is set.
    pub fn contains(self, flag: TupFlags) -> bool {
        self.0 & flag.as_i32() != 0
    }

    /// Set a flag.
    pub fn insert(&mut self, flag: TupFlags) {
        self.0 |= flag.as_i32();
    }

    /// Clear a flag.
    pub fn remove(&mut self, flag: TupFlags) {
        self.0 &= !flag.as_i32();
    }

    /// Check if no flags are set.
    pub fn is_empty(self) -> bool {
        self.0 == 0
    }
}

impl From<TupFlags> for FlagSet {
    fn from(flag: TupFlags) -> Self {
        FlagSet(flag.as_i32())
    }
}

impl std::fmt::Display for TupFlags {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::None => f.write_str("none"),
            Self::Modify => f.write_str("modify"),
            Self::Create => f.write_str("create"),
            Self::Config => f.write_str("config"),
            Self::Variant => f.write_str("variant"),
            Self::Transient => f.write_str("transient"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_flag_values() {
        assert_eq!(TupFlags::None.as_i32(), 0);
        assert_eq!(TupFlags::Modify.as_i32(), 1);
        assert_eq!(TupFlags::Create.as_i32(), 2);
        assert_eq!(TupFlags::Config.as_i32(), 4);
        assert_eq!(TupFlags::Variant.as_i32(), 8);
        assert_eq!(TupFlags::Transient.as_i32(), 16);
    }

    #[test]
    fn test_flag_roundtrip() {
        for &val in &[0, 1, 2, 4, 8, 16] {
            let flag = TupFlags::from_i32(val).unwrap();
            assert_eq!(flag.as_i32(), val);
        }
    }

    #[test]
    fn test_flag_invalid() {
        assert!(TupFlags::from_i32(3).is_none());
        assert!(TupFlags::from_i32(5).is_none());
        assert!(TupFlags::from_i32(32).is_none());
    }

    #[test]
    fn test_flag_table_name() {
        assert_eq!(TupFlags::Modify.table_name(), Some("modify_list"));
        assert_eq!(TupFlags::Create.table_name(), Some("create_list"));
        assert_eq!(TupFlags::Config.table_name(), Some("config_list"));
        assert_eq!(TupFlags::Variant.table_name(), Some("variant_list"));
        assert_eq!(TupFlags::Transient.table_name(), Some("transient_list"));
        assert_eq!(TupFlags::None.table_name(), None);
    }

    #[test]
    fn test_flagset_operations() {
        let mut flags = FlagSet::empty();
        assert!(flags.is_empty());

        flags.insert(TupFlags::Modify);
        assert!(flags.contains(TupFlags::Modify));
        assert!(!flags.contains(TupFlags::Create));

        flags.insert(TupFlags::Create);
        assert!(flags.contains(TupFlags::Modify));
        assert!(flags.contains(TupFlags::Create));
        assert_eq!(flags.raw(), 3);

        flags.remove(TupFlags::Modify);
        assert!(!flags.contains(TupFlags::Modify));
        assert!(flags.contains(TupFlags::Create));
    }

    #[test]
    fn test_flagset_from_flag() {
        let flags: FlagSet = TupFlags::Config.into();
        assert!(flags.contains(TupFlags::Config));
        assert!(!flags.contains(TupFlags::Modify));
    }
}
