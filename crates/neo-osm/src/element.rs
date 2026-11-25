//! OSM primitive DTOs.
//!
//! These structs are **value objects** at IO / API / clipboard / command boundaries, not the storage format.
//! The storage layer (`neo-store`) keeps the same information in SoA form; the two can convert into each other.

use neo_core::{LatLon, Meta, ObjectId, ObjectKind, Sym};
use smallvec::SmallVec;

use crate::tags::Tags;

/// A Node.
#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    /// OSM ID (negative for local creations).
    pub id: ObjectId,
    /// Latitude/longitude (fixed point, 1e-7 degree).
    pub pos: LatLon,
    /// Tags.
    pub tags: Tags,
    /// Metadata.
    pub meta: Meta,
}

/// A Way; the order of `nodes` is the geometric order.
#[derive(Clone, Debug, PartialEq)]
pub struct Way {
    /// OSM ID.
    pub id: ObjectId,
    /// Node sequence (order-sensitive; duplicate nodes are legal but usually invalid data, reported by the validator).
    pub nodes: SmallVec<[ObjectId; 8]>,
    /// Tags.
    pub tags: Tags,
    /// Metadata.
    pub meta: Meta,
}

/// Relation member.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RelationMember {
    /// Target object kind.
    pub kind: ObjectKind,
    /// Target object ID.
    pub id: ObjectId,
    /// Member role (interned symbol, may be empty).
    pub role: Sym,
}

/// A Relation.
///
/// Member order is **part of the data** (order is semantic for route / multipolygon); editing and uploading
/// must both preserve it; see the "easy-to-miss behaviors" in docs/13.
#[derive(Clone, Debug, PartialEq)]
pub struct Relation {
    /// OSM ID.
    pub id: ObjectId,
    /// Member list.
    pub members: SmallVec<[RelationMember; 4]>,
    /// Tags.
    pub tags: Tags,
    /// Metadata.
    pub meta: Meta,
}

/// Any OSM primitive.
#[derive(Clone, Debug, PartialEq)]
pub enum Element {
    /// Point.
    Node(Node),
    /// Line / area.
    Way(Way),
    /// Relation.
    Relation(Relation),
}

impl Element {
    /// Object ID.
    #[inline]
    pub fn id(&self) -> ObjectId {
        match self {
            Self::Node(n) => n.id,
            Self::Way(w) => w.id,
            Self::Relation(r) => r.id,
        }
    }

    /// Object kind.
    #[inline]
    pub fn kind(&self) -> ObjectKind {
        match self {
            Self::Node(_) => ObjectKind::Node,
            Self::Way(_) => ObjectKind::Way,
            Self::Relation(_) => ObjectKind::Relation,
        }
    }

    /// Tags.
    #[inline]
    pub fn tags(&self) -> &Tags {
        match self {
            Self::Node(n) => &n.tags,
            Self::Way(w) => &w.tags,
            Self::Relation(r) => &r.tags,
        }
    }

    /// Metadata.
    #[inline]
    pub fn meta(&self) -> &Meta {
        match self {
            Self::Node(n) => &n.meta,
            Self::Way(w) => &w.meta,
            Self::Relation(r) => &r.meta,
        }
    }
}
