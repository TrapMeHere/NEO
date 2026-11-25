//! External OSM identifiers and object kinds.
//!
//! Convention (matching JOSM): `ObjectId > 0` is an ID that already exists on the server; `ObjectId < 0` is an
//! object created locally and not yet uploaded; `ObjectId == 0` is reserved for "no ID". Internal storage never
//! addresses objects by OSM ID directly; IDs are first mapped to [`crate::Handle`] via `IdIndex`, see `neo-store`.

use core::fmt;

/// External OSM object identifier (shared by Node / Way / Relation / Changeset).
///
/// Uses `i64` instead of `i32`: OSM node IDs already exceed 12 digits and changeset IDs keep growing,
/// so any future 32-bit assumption would be a data-corruption risk.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct ObjectId(pub i64);

impl ObjectId {
    /// Placeholder value for "no ID".
    pub const NONE: Self = Self(0);

    /// Construct.
    #[inline]
    pub const fn new(raw: i64) -> Self {
        Self(raw)
    }

    /// Raw value.
    #[inline]
    pub const fn raw(self) -> i64 {
        self.0
    }

    /// Whether this is a locally created (not yet uploaded) object.
    #[inline]
    pub const fn is_local(self) -> bool {
        self.0 < 0
    }

    /// Whether a server ID already exists.
    #[inline]
    pub const fn is_server(self) -> bool {
        self.0 > 0
    }

    /// Whether this is the empty ID.
    #[inline]
    pub const fn is_none(self) -> bool {
        self.0 == 0
    }
}

impl fmt::Display for ObjectId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl fmt::Debug for ObjectId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ObjectId({})", self.0)
    }
}

#[cfg(feature = "serde")]
impl serde::Serialize for ObjectId {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_i64(self.0)
    }
}

#[cfg(feature = "serde")]
impl<'de> serde::Deserialize<'de> for ObjectId {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        i64::deserialize(d).map(Self)
    }
}

/// OSM object kind.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
#[repr(u8)]
pub enum ObjectKind {
    /// Point.
    Node = 0,
    /// Line / closed area.
    Way = 1,
    /// Relation.
    Relation = 2,
    /// Changeset (API / metadata only, not in the spatial index).
    Changeset = 3,
}

impl ObjectKind {
    /// Editable primitive kinds (excluding changeset).
    pub const EDITABLE: [Self; 3] = [Self::Node, Self::Way, Self::Relation];

    /// Stable lowercase name used in logs, XML/OSC output, and persistence.
    #[inline]
    pub const fn type_name(self) -> &'static str {
        match self {
            Self::Node => "node",
            Self::Way => "way",
            Self::Relation => "relation",
            Self::Changeset => "changeset",
        }
    }

    /// Parse from `node`/`way`/`relation`.
    pub fn from_type_name(name: &str) -> Option<Self> {
        match name {
            "node" => Some(Self::Node),
            "way" => Some(Self::Way),
            "relation" => Some(Self::Relation),
            "changeset" => Some(Self::Changeset),
            _ => None,
        }
    }
}

impl fmt::Display for ObjectKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.type_name())
    }
}
