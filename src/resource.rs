//! Resources: the units a change reads and writes.

use serde::{Deserialize, Serialize};
use std::fmt;

/// A unit of program state. Text form: `path` (whole file), `path#Name`
/// (top-level symbol), `path#` (module-level code outside any symbol).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(into = "String", from = "String")]
pub enum Resource {
    File(String),
    Symbol(String, String),
    Top(String),
}

impl Resource {
    /// Parse the text form. A literal `#` or `%` in a path is written `%23` or `%25`.
    pub fn parse(s: &str) -> Resource {
        let unescape = |path: &str| path.replace("%23", "#").replace("%25", "%");
        match s.split_once('#') {
            Some((path, "")) => Resource::Top(unescape(path)),
            Some((path, name)) => Resource::Symbol(unescape(path), name.into()),
            None => Resource::File(unescape(s)),
        }
    }

    pub fn path(&self) -> &str {
        match self {
            Resource::File(p) | Resource::Symbol(p, _) | Resource::Top(p) => p,
        }
    }

    /// Two resources overlap when they are equal, or one is a whole file
    /// containing the other.
    pub fn overlaps(&self, other: &Resource) -> bool {
        if self == other {
            return true;
        }
        let whole = matches!(self, Resource::File(_)) || matches!(other, Resource::File(_));
        whole && self.path() == other.path()
    }

    /// For claims: as `overlaps`, and a type also holds its methods (`Type` and
    /// `Type::method`). Writes do not use this: a field and a method of one
    /// type can be changed at the same time.
    pub fn held_with(&self, other: &Resource) -> bool {
        let member = |a: &Resource, b: &Resource| match (a, b) {
            (Resource::Symbol(p, owner), Resource::Symbol(q, name)) => {
                p == q && name.strip_prefix(owner.as_str()).is_some_and(|rest| rest.starts_with("::"))
            }
            _ => false,
        };
        self.overlaps(other) || member(self, other) || member(other, self)
    }
}

/// Ordered by path first, so everything about one file sorts together.
impl Ord for Resource {
    fn cmp(&self, other: &Resource) -> std::cmp::Ordering {
        fn key(r: &Resource) -> (u8, &str) {
            match r {
                Resource::File(_) => (0, ""),
                Resource::Top(_) => (1, ""),
                Resource::Symbol(_, name) => (2, name),
            }
        }
        self.path().cmp(other.path()).then_with(|| key(self).cmp(&key(other)))
    }
}

impl PartialOrd for Resource {
    fn partial_cmp(&self, other: &Resource) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl fmt::Display for Resource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let path = self.path().replace('%', "%25").replace('#', "%23");
        match self {
            Resource::File(_) => write!(f, "{path}"),
            Resource::Symbol(_, name) => write!(f, "{path}#{name}"),
            Resource::Top(_) => write!(f, "{path}#"),
        }
    }
}

impl From<Resource> for String {
    fn from(r: Resource) -> String {
        r.to_string()
    }
}

impl From<String> for Resource {
    fn from(s: String) -> Resource {
        Resource::parse(&s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_form_round_trips() {
        for s in ["src/a.rs", "src/a.rs#Foo", "src/a.rs#"] {
            assert_eq!(Resource::parse(s).to_string(), s);
        }
        assert_eq!(Resource::parse("a.rs#Foo"), Resource::Symbol("a.rs".into(), "Foo".into()));
        assert_eq!(Resource::parse("a.rs#"), Resource::Top("a.rs".into()));
        assert_eq!(Resource::parse("a.rs"), Resource::File("a.rs".into()));
    }

    #[test]
    fn hashes_in_paths_and_names_round_trip() {
        for r in [
            Resource::File("C#/a.cs".into()),
            Resource::File("notes#1.txt".into()),
            Resource::File("100%#.md".into()),
            Resource::Top("C#/a.cs".into()),
            Resource::Symbol("C#/a.cs".into(), "Foo".into()),
            Resource::Symbol("a.rs".into(), "r#try".into()),
        ] {
            assert_eq!(Resource::parse(&r.to_string()), r, "{r}");
        }
        assert_eq!(Resource::File("C#/a.cs".into()).to_string(), "C%23/a.cs");
    }

    #[test]
    fn distinct_symbols_in_one_file_do_not_overlap() {
        let foo = Resource::parse("a.rs#foo");
        let bar = Resource::parse("a.rs#bar");
        assert!(!foo.overlaps(&bar));
        assert!(foo.overlaps(&foo));
        assert!(!foo.overlaps(&Resource::parse("a.rs#")));
    }

    #[test]
    fn whole_file_overlaps_everything_inside_it() {
        let file = Resource::parse("a.rs");
        assert!(file.overlaps(&Resource::parse("a.rs#foo")));
        assert!(Resource::parse("a.rs#").overlaps(&file));
        assert!(!file.overlaps(&Resource::parse("b.rs#foo")));
        assert!(!file.overlaps(&Resource::parse("b.rs")));
    }
}
