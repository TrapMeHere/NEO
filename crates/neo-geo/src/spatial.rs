//! Spatial index: rstar R-tree (**separate trees per object type**).
//!
//! The **object tier** of the two-tier index: nodes (points), Ways (precomputed AABBs). Relations do not enter the spatial index
//! directly — they have no coordinates of their own, and their bbox is derived from members (see docs/04).
//!
//! Why separate trees for nodes and ways:
//! At city-level zoom "all ways are within the viewport" is the norm; if both object types shared one tree, iterating only ways
//! would still scan every node entry (prototype measurement: about 100 ms extra at 2 million entries). After splitting,
//! `for_each_way` only touches the way tree, cutting traversal to 1/6.
//!
//! Construction strategy:
//!
//! * after import/bulk changes, use `RTree::bulk_load` (O(n log n), an order of magnitude faster than inserting one by one);
//! * during interactive editing, update incrementally with [`SpatialIndex::update_way`] / [`SpatialIndex::update_node`];
//! * `RTree::remove` requires "the same element", so an extra `entries` map is maintained to look up the old bbox.

use std::collections::HashMap;

use neo_core::{ByteSize, Handle, ObjectKind};
use neo_store::{DataSet, NodeSlot, WaySlot};
use rstar::{AABB, RTree, RTreeObject};

use crate::geometry;
use crate::world::{WorldBounds, WorldPoint};

const KIND_BITS: u64 = 2;

fn key(kind: ObjectKind, slot: u32) -> u64 {
    ((kind as u64) << 32) | u64::from(slot)
}

/// Spatial index entry.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpatialEntry {
    /// Object kind (Node / Way).
    pub kind: ObjectKind,
    /// Slot index (combined with generation to restore the handle).
    pub slot: u32,
    /// Slot generation.
    pub generation: u32,
    /// World-coordinate bounding box.
    pub bounds: WorldBounds,
}

impl SpatialEntry {
    /// Node handle (returns `None` if it is not a node).
    pub fn as_node(self) -> Option<Handle<NodeSlot>> {
        (self.kind == ObjectKind::Node).then(|| Handle::new(self.slot, self.generation))
    }

    /// Way handle.
    pub fn as_way(self) -> Option<Handle<WaySlot>> {
        (self.kind == ObjectKind::Way).then(|| Handle::new(self.slot, self.generation))
    }

    /// Hit-testing priority: nodes take priority over ways.
    pub fn priority(self) -> u8 {
        match self.kind {
            ObjectKind::Node => 0,
            ObjectKind::Way => 1,
            _ => 2,
        }
    }
}

impl RTreeObject for SpatialEntry {
    type Envelope = AABB<[f64; 2]>;

    fn envelope(&self) -> Self::Envelope {
        AABB::from_corners(
            [self.bounds.min.x, self.bounds.min.y],
            [self.bounds.max.x, self.bounds.max.y],
        )
    }
}

/// Object spatial index (separate node / way trees).
#[derive(Debug)]
pub struct SpatialIndex {
    node_tree: RTree<SpatialEntry>,
    way_tree: RTree<SpatialEntry>,
    /// slot key -> registered entry (for incremental deletion).
    entries: HashMap<u64, SpatialEntry>,
}

impl Default for SpatialIndex {
    fn default() -> Self {
        Self::new()
    }
}

impl SpatialIndex {
    /// Empty index.
    pub fn new() -> Self {
        Self {
            node_tree: RTree::new(),
            way_tree: RTree::new(),
            entries: HashMap::new(),
        }
    }

    /// Total number of entries.
    pub fn len(&self) -> usize {
        self.node_tree.size() + self.way_tree.size()
    }

    /// Whether it is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Number of node entries.
    pub fn node_count(&self) -> usize {
        self.node_tree.size()
    }

    /// Number of Way entries.
    pub fn way_count(&self) -> usize {
        self.way_tree.size()
    }

    /// Build fully from the dataset.
    pub fn build(data: &DataSet) -> Self {
        let mut nodes = Vec::with_capacity(data.node_count());
        let mut ways = Vec::with_capacity(data.way_count());

        for node in data.iter_nodes() {
            // Objects deleted on the server (visible=false) do not enter the spatial index; otherwise they could still be picked/rendered
            // after importing historical data.
            if !node.meta.visible {
                continue;
            }
            let wp = WorldPoint::from_latlon(node.pos);
            nodes.push(SpatialEntry {
                kind: ObjectKind::Node,
                slot: node.handle.index(),
                generation: node.handle.generation(),
                bounds: WorldBounds::from_point(wp),
            });
        }
        for way in data.iter_ways() {
            if !way.meta.visible {
                continue;
            }
            if let Some(bounds) = geometry::way_bounds(data, way.handle) {
                ways.push(SpatialEntry {
                    kind: ObjectKind::Way,
                    slot: way.handle.index(),
                    generation: way.handle.generation(),
                    bounds,
                });
            }
        }

        let mut map = HashMap::with_capacity(nodes.len() + ways.len());
        for e in nodes.iter().chain(ways.iter()) {
            map.insert(key(e.kind, e.slot), *e);
        }
        Self {
            node_tree: RTree::bulk_load(nodes),
            way_tree: RTree::bulk_load(ways),
            entries: map,
        }
    }

    /// Bounding-box query (returns all intersecting entries).
    pub fn query(&self, bounds: WorldBounds) -> Vec<SpatialEntry> {
        let env = AABB::from_corners([bounds.min.x, bounds.min.y], [bounds.max.x, bounds.max.y]);
        let mut out: Vec<SpatialEntry> = self
            .node_tree
            .locate_in_envelope_intersecting(env)
            .copied()
            .collect();
        out.extend(self.way_tree.locate_in_envelope_intersecting(env).copied());
        out
    }

    /// Allocation-free traversal: call `f` for each entry in the bbox (without building an intermediate Vec).
    pub fn for_each_in<F>(&self, bounds: WorldBounds, mut f: F)
    where
        F: FnMut(&SpatialEntry),
    {
        let env = AABB::from_corners([bounds.min.x, bounds.min.y], [bounds.max.x, bounds.max.y]);
        for entry in self.node_tree.locate_in_envelope_intersecting(env) {
            f(entry);
        }
        for entry in self.way_tree.locate_in_envelope_intersecting(env) {
            f(entry);
        }
    }

    /// Traverse only Ways (without touching the node tree).
    pub fn for_each_way<F>(&self, bounds: WorldBounds, mut f: F)
    where
        F: FnMut(&SpatialEntry),
    {
        let env = AABB::from_corners([bounds.min.x, bounds.min.y], [bounds.max.x, bounds.max.y]);
        for entry in self.way_tree.locate_in_envelope_intersecting(env) {
            f(entry);
        }
    }

    /// Traverse only Nodes.
    pub fn for_each_node<F>(&self, bounds: WorldBounds, mut f: F)
    where
        F: FnMut(&SpatialEntry),
    {
        let env = AABB::from_corners([bounds.min.x, bounds.min.y], [bounds.max.x, bounds.max.y]);
        for entry in self.node_tree.locate_in_envelope_intersecting(env) {
            f(entry);
        }
    }

    /// Query only Ways.
    pub fn query_ways(&self, bounds: WorldBounds) -> Vec<SpatialEntry> {
        let env = AABB::from_corners([bounds.min.x, bounds.min.y], [bounds.max.x, bounds.max.y]);
        self.way_tree
            .locate_in_envelope_intersecting(env)
            .copied()
            .collect()
    }

    /// Query only Nodes.
    pub fn query_nodes(&self, bounds: WorldBounds) -> Vec<SpatialEntry> {
        let env = AABB::from_corners([bounds.min.x, bounds.min.y], [bounds.max.x, bounds.max.y]);
        self.node_tree
            .locate_in_envelope_intersecting(env)
            .copied()
            .collect()
    }

    /// Incrementally update a node.
    pub fn update_node(&mut self, handle: Handle<NodeSlot>, node: neo_core::LatLon) {
        self.remove_key(ObjectKind::Node, handle.index());
        let wp = WorldPoint::from_latlon(node);
        self.insert_entry(SpatialEntry {
            kind: ObjectKind::Node,
            slot: handle.index(),
            generation: handle.generation(),
            bounds: WorldBounds::from_point(wp),
        });
    }

    /// Incrementally update one way's bbox.
    pub fn update_way(&mut self, data: &DataSet, handle: Handle<WaySlot>) -> bool {
        self.remove_key(ObjectKind::Way, handle.index());
        let Some(bounds) = geometry::way_bounds(data, handle) else {
            return false;
        };
        self.insert_entry(SpatialEntry {
            kind: ObjectKind::Way,
            slot: handle.index(),
            generation: handle.generation(),
            bounds,
        });
        true
    }

    /// Remove an object's entry.
    pub fn remove(&mut self, kind: ObjectKind, slot: u32) -> bool {
        self.remove_key(kind, slot)
    }

    fn remove_key(&mut self, kind: ObjectKind, slot: u32) -> bool {
        let key = key(kind, slot);
        match self.entries.remove(&key) {
            Some(old) => {
                self.tree_mut(kind).remove(&old);
                true
            }
            None => false,
        }
    }

    fn tree_mut(&mut self, kind: ObjectKind) -> &mut RTree<SpatialEntry> {
        match kind {
            ObjectKind::Node => &mut self.node_tree,
            _ => &mut self.way_tree,
        }
    }

    fn insert_entry(&mut self, entry: SpatialEntry) {
        self.entries.insert(key(entry.kind, entry.slot), entry);
        self.tree_mut(entry.kind).insert(entry);
    }

    /// Estimate memory.
    pub fn mem_bytes(&self) -> ByteSize {
        ByteSize(
            self.entries.capacity()
                * (core::mem::size_of::<u64>() + core::mem::size_of::<SpatialEntry>() + 8)
                + self.len() * core::mem::size_of::<SpatialEntry>(),
        )
    }
}

const _: () = assert!(KIND_BITS == 2);
