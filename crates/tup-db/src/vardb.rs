use std::collections::BTreeMap;

use tup_types::TupId;

/// In-memory variable database.
///
/// Corresponds to `struct vardb` in C's vardb.h. This is a fast in-memory
/// store for @-variables and environment variables used during Tupfile parsing.
/// Variables are keyed by name and may optionally reference a database node.
#[derive(Debug)]
pub struct VarDb {
    vars: BTreeMap<String, VarEntry>,
}

/// A single variable entry.
#[derive(Debug, Clone)]
pub struct VarEntry {
    /// Variable name.
    pub name: String,
    /// Variable value.
    pub value: String,
    /// Optional reference to a node in the database (for @-variables).
    pub tent_id: Option<TupId>,
}

impl VarDb {
    /// Create a new empty variable database.
    pub fn new() -> Self {
        VarDb {
            vars: BTreeMap::new(),
        }
    }

    /// Set a variable value, creating or replacing it.
    pub fn set(&mut self, name: &str, value: &str, tent_id: Option<TupId>) {
        self.vars.insert(
            name.to_string(),
            VarEntry {
                name: name.to_string(),
                value: value.to_string(),
                tent_id,
            },
        );
    }

    /// Append to a variable's value (space-separated).
    ///
    /// If the variable doesn't exist, creates it with the given value.
    pub fn append(&mut self, name: &str, value: &str) {
        match self.vars.get_mut(name) {
            Some(entry) => {
                if !entry.value.is_empty() {
                    entry.value.push(' ');
                }
                entry.value.push_str(value);
            }
            None => {
                self.set(name, value, None);
            }
        }
    }

    /// Get a variable entry by name.
    pub fn get(&self, name: &str) -> Option<&VarEntry> {
        self.vars.get(name)
    }

    /// Get a variable's value, returning empty string if not found.
    pub fn get_value(&self, name: &str) -> &str {
        self.vars.get(name).map(|e| e.value.as_str()).unwrap_or("")
    }

    /// Copy a variable's value into a string.
    ///
    /// Returns true if the variable was found.
    pub fn copy_value(&self, name: &str, dest: &mut String) -> bool {
        match self.vars.get(name) {
            Some(entry) => {
                dest.push_str(&entry.value);
                true
            }
            None => false,
        }
    }

    /// Remove a variable.
    pub fn remove(&mut self, name: &str) -> bool {
        self.vars.remove(name).is_some()
    }

    /// Get the number of variables.
    pub fn len(&self) -> usize {
        self.vars.len()
    }

    /// Check if the database is empty.
    pub fn is_empty(&self) -> bool {
        self.vars.is_empty()
    }

    /// Clear all variables.
    pub fn clear(&mut self) {
        self.vars.clear();
    }

    /// Iterate over all variables in sorted order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &VarEntry)> {
        self.vars.iter().map(|(k, v)| (k.as_str(), v))
    }

    /// Compare two variable databases, calling callbacks for differences.
    ///
    /// Corresponds to `vardb_compare()` in C.
    /// - `only_in_a`: called for variables only in `self`
    /// - `only_in_b`: called for variables only in `other`
    /// - `in_both`: called for variables in both (may have different values)
    pub fn compare<FA, FB, FS>(
        &self,
        other: &VarDb,
        mut only_in_a: FA,
        mut only_in_b: FB,
        mut in_both: FS,
    ) where
        FA: FnMut(&VarEntry),
        FB: FnMut(&VarEntry),
        FS: FnMut(&VarEntry, &VarEntry),
    {
        let mut b_iter = other.vars.iter().peekable();

        for (a_name, a_entry) in &self.vars {
            // Advance b past any entries that come before a
            while let Some((b_name, b_entry)) = b_iter.peek() {
                match b_name.as_str().cmp(a_name.as_str()) {
                    std::cmp::Ordering::Less => {
                        only_in_b(b_entry);
                        b_iter.next();
                    }
                    std::cmp::Ordering::Equal => {
                        in_both(a_entry, b_entry);
                        b_iter.next();
                        break;
                    }
                    std::cmp::Ordering::Greater => {
                        break;
                    }
                }
            }

            // If b is exhausted or past a, then a is only_in_a
            // (unless we matched in the Equal case above)
            if !other.vars.contains_key(a_name) {
                only_in_a(a_entry);
            }
        }

        // Remaining entries in b are only_in_b
        for (_, b_entry) in b_iter {
            only_in_b(b_entry);
        }
    }
}

impl Default for VarDb {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_set_and_get() {
        let mut vdb = VarDb::new();
        vdb.set("CC", "gcc", None);

        assert_eq!(vdb.get_value("CC"), "gcc");
        assert!(vdb.get("CC").is_some());
        assert!(vdb.get("CXX").is_none());
    }

    #[test]
    fn test_set_with_tent_id() {
        let mut vdb = VarDb::new();
        vdb.set("MY_VAR", "value", Some(TupId::new(42)));

        let entry = vdb.get("MY_VAR").unwrap();
        assert_eq!(entry.tent_id, Some(TupId::new(42)));
    }

    #[test]
    fn test_append() {
        let mut vdb = VarDb::new();
        vdb.set("CFLAGS", "-Wall", None);
        vdb.append("CFLAGS", "-O2");

        assert_eq!(vdb.get_value("CFLAGS"), "-Wall -O2");
    }

    #[test]
    fn test_append_new_var() {
        let mut vdb = VarDb::new();
        vdb.append("NEW", "value");
        assert_eq!(vdb.get_value("NEW"), "value");
    }

    #[test]
    fn test_copy_value() {
        let mut vdb = VarDb::new();
        vdb.set("X", "hello", None);

        let mut dest = String::new();
        assert!(vdb.copy_value("X", &mut dest));
        assert_eq!(dest, "hello");

        let mut dest2 = String::new();
        assert!(!vdb.copy_value("Y", &mut dest2));
        assert!(dest2.is_empty());
    }

    #[test]
    fn test_remove() {
        let mut vdb = VarDb::new();
        vdb.set("X", "1", None);
        assert!(vdb.remove("X"));
        assert!(!vdb.remove("X"));
        assert!(vdb.get("X").is_none());
    }

    #[test]
    fn test_len_and_empty() {
        let mut vdb = VarDb::new();
        assert!(vdb.is_empty());
        assert_eq!(vdb.len(), 0);

        vdb.set("A", "1", None);
        vdb.set("B", "2", None);
        assert_eq!(vdb.len(), 2);
        assert!(!vdb.is_empty());
    }

    #[test]
    fn test_overwrite() {
        let mut vdb = VarDb::new();
        vdb.set("X", "old", None);
        vdb.set("X", "new", None);
        assert_eq!(vdb.get_value("X"), "new");
        assert_eq!(vdb.len(), 1);
    }

    #[test]
    fn test_iter_sorted() {
        let mut vdb = VarDb::new();
        vdb.set("C", "3", None);
        vdb.set("A", "1", None);
        vdb.set("B", "2", None);

        let names: Vec<&str> = vdb.iter().map(|(k, _)| k).collect();
        assert_eq!(names, vec!["A", "B", "C"]);
    }

    #[test]
    fn test_compare() {
        let mut a = VarDb::new();
        a.set("X", "1", None);
        a.set("Y", "2", None);
        a.set("Z", "3", None);

        let mut b = VarDb::new();
        b.set("Y", "2", None);
        b.set("Z", "changed", None);
        b.set("W", "4", None);

        let mut only_a = Vec::new();
        let mut only_b = Vec::new();
        let mut both = Vec::new();

        a.compare(
            &b,
            |e| only_a.push(e.name.clone()),
            |e| only_b.push(e.name.clone()),
            |ea, eb| both.push((ea.name.clone(), ea.value == eb.value)),
        );

        assert_eq!(only_a, vec!["X"]);
        assert_eq!(only_b, vec!["W"]);
        assert_eq!(
            both,
            vec![("Y".to_string(), true), ("Z".to_string(), false)]
        );
    }

    #[test]
    fn test_get_value_default() {
        let vdb = VarDb::new();
        assert_eq!(vdb.get_value("MISSING"), "");
    }
}
