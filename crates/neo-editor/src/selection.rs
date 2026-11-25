//! Selection set.

use std::collections::BTreeSet;

use neo_core::{ObjectId, ObjectKind};

/// Currently selected objects (grouped by kind).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Selection {
    nodes: BTreeSet<ObjectId>,
    ways: BTreeSet<ObjectId>,
    relations: BTreeSet<ObjectId>,
}

impl Selection {
    /// Empty selection.
    pub fn new() -> Self {
        Self::default()
    }

    /// Clear.
    pub fn clear(&mut self) {
        self.nodes.clear();
        self.ways.clear();
        self.relations.clear();
    }

    /// Whether it is empty.
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty() && self.ways.is_empty() && self.relations.is_empty()
    }

    /// Total count.
    pub fn len(&self) -> usize {
        self.nodes.len() + self.ways.len() + self.relations.len()
    }

    /// Whether it contains the object.
    pub fn contains(&self, kind: ObjectKind, id: ObjectId) -> bool {
        match kind {
            ObjectKind::Node => self.nodes.contains(&id),
            ObjectKind::Way => self.ways.contains(&id),
            ObjectKind::Relation => self.relations.contains(&id),
            ObjectKind::Changeset => false,
        }
    }

    /// Insert.
    pub fn insert(&mut self, kind: ObjectKind, id: ObjectId) {
        match kind {
            ObjectKind::Node => {
                self.nodes.insert(id);
            }
            ObjectKind::Way => {
                self.ways.insert(id);
            }
            ObjectKind::Relation => {
                self.relations.insert(id);
            }
            ObjectKind::Changeset => {}
        }
    }

    /// Remove.
    pub fn remove(&mut self, kind: ObjectKind, id: ObjectId) -> bool {
        match kind {
            ObjectKind::Node => self.nodes.remove(&id),
            ObjectKind::Way => self.ways.remove(&id),
            ObjectKind::Relation => self.relations.remove(&id),
            ObjectKind::Changeset => false,
        }
    }

    /// Toggle.
    pub fn toggle(&mut self, kind: ObjectKind, id: ObjectId) {
        if !self.remove(kind, id) {
            self.insert(kind, id);
        }
    }

    /// Select exactly one object.
    pub fn select_only(&mut self, kind: ObjectKind, id: ObjectId) {
        self.clear();
        self.insert(kind, id);
    }

    /// Iterate one kind.
    pub fn iter(&self, kind: ObjectKind) -> impl Iterator<Item = ObjectId> + '_ {
        let (a, b) = match kind {
            ObjectKind::Node => (&self.nodes, &self.ways),
            ObjectKind::Way => (&self.ways, &self.relations),
            ObjectKind::Relation => (&self.relations, &self.nodes),
            ObjectKind::Changeset => (&self.nodes, &self.nodes),
        };
        let _ = b; // keep the match exhaustive
        match kind {
            ObjectKind::Node => Box::new(a.iter().copied()) as Box<dyn Iterator<Item = ObjectId>>,
            ObjectKind::Way => Box::new(a.iter().copied()),
            ObjectKind::Relation => Box::new(a.iter().copied()),
            ObjectKind::Changeset => Box::new(std::iter::empty()),
        }
    }

    /// Node set.
    pub fn nodes(&self) -> &BTreeSet<ObjectId> {
        &self.nodes
    }

    /// Way set.
    pub fn ways(&self) -> &BTreeSet<ObjectId> {
        &self.ways
    }

    /// Relation set.
    pub fn relations(&self) -> &BTreeSet<ObjectId> {
        &self.relations
    }
}
