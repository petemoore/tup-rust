/// A unique identifier for a node in the tup database.
///
/// In the C implementation, this is `typedef sqlite3_int64 tupid_t`,
/// which is a 64-bit signed integer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TupId(i64);

impl TupId {
    /// Create a new TupId from a raw i64 value.
    pub fn new(id: i64) -> Self {
        TupId(id)
    }

    /// Create a TupId in a const context.
    pub const fn new_const(id: i64) -> Self {
        TupId(id)
    }

    /// Get the raw i64 value.
    pub fn raw(self) -> i64 {
        self.0
    }
}

impl From<i64> for TupId {
    fn from(id: i64) -> Self {
        TupId(id)
    }
}

impl From<TupId> for i64 {
    fn from(id: TupId) -> Self {
        id.0
    }
}

impl std::fmt::Display for TupId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tupid_new_and_raw() {
        let id = TupId::new(42);
        assert_eq!(id.raw(), 42);
    }

    #[test]
    fn test_tupid_from_i64() {
        let id: TupId = 100i64.into();
        assert_eq!(id.raw(), 100);
    }

    #[test]
    fn test_tupid_into_i64() {
        let id = TupId::new(200);
        let raw: i64 = id.into();
        assert_eq!(raw, 200);
    }

    #[test]
    fn test_tupid_ordering() {
        let a = TupId::new(1);
        let b = TupId::new(2);
        let c = TupId::new(1);
        assert!(a < b);
        assert_eq!(a, c);
    }

    #[test]
    fn test_tupid_display() {
        let id = TupId::new(42);
        assert_eq!(format!("{}", id), "42");
    }

    #[test]
    fn test_tupid_hash() {
        use std::collections::HashSet;
        let mut set = HashSet::new();
        set.insert(TupId::new(1));
        set.insert(TupId::new(2));
        set.insert(TupId::new(1));
        assert_eq!(set.len(), 2);
    }
}
