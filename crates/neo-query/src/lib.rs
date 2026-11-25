//! # neo-query — read-only map query capabilities
//!
//! Rationale (architecture gap fix, see ADR-0007 and docs/15):
//!
//! * Plugins, validators and background tasks need "read object by ID / query objects by bbox / look up parent references";
//! * But they are **not allowed** to hold `&mut DataSet`, nor should they depend directly on `neo-store`'s write interfaces;
//! * So read-only capability is consolidated into one trait + one immutable snapshot view.
//!
//! ```text
//! editor thread (single writer)        background / plugins (many readers)
//! ┌──────────────┐   snapshot publish   ┌────────────────────┐
//! │ &mut DataSet │ ──────────►          │ Arc<MapSnapshot>   │
//! └──────────────┘   arc-swap           │  → QueryView<'_>   │
//!                                       └────────────────────┘
//! ```
//!
//! Note: this crate only **queries**. Any modification must go through `neo-editor`'s Command channel.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

use std::sync::Arc;

use neo_core::{ObjectId, ObjectKind};
use neo_geo::{SpatialIndex, WorldBounds, WorldPoint};
use neo_osm::{Node, Relation, Way};
use neo_store::{DataSet, Referrers};

/// Read-only map query interface.
///
/// All implementations must be "do not change visible state while reading"; background tasks may call them concurrently and safely.
pub trait MapQuery {
    /// Underlying dataset (read-only).
    fn data(&self) -> &DataSet;

    /// Read a node DTO by ID.
    fn node(&self, id: ObjectId) -> Option<Node>;
    /// Read a way DTO by ID.
    fn way(&self, id: ObjectId) -> Option<Way>;
    /// Read a relation DTO by ID.
    fn relation(&self, id: ObjectId) -> Option<Relation>;

    /// Ways referencing a node.
    fn parent_ways(&self, node: ObjectId) -> Vec<ObjectId>;
    /// Relations referencing an object.
    fn parent_relations(&self, kind: ObjectKind, id: ObjectId) -> Vec<ObjectId>;

    /// Nodes inside a bounding box (world coordinates).
    fn nodes_in(&self, bounds: WorldBounds) -> Vec<ObjectId>;
    /// Ways inside a bounding box.
    fn ways_in(&self, bounds: WorldBounds) -> Vec<ObjectId>;

    /// Nearest node (within `max_meters`).
    fn nearest_node(&self, point: WorldPoint, max_meters: f64) -> Option<(ObjectId, f64)>;
}

/// A query view: dataset + optional spatial index/reverse references.
///
/// Index and referrers may be absent (e.g. just imported and not built yet); related queries then return empty sets instead of panicking.
pub struct QueryView<'a> {
    data: &'a DataSet,
    index: Option<&'a SpatialIndex>,
    referrers: Option<&'a Referrers>,
}

impl<'a> QueryView<'a> {
    /// Dataset only (no spatial query capability).
    pub fn new(data: &'a DataSet) -> Self {
        Self {
            data,
            index: None,
            referrers: None,
        }
    }

    /// Dataset + spatial index.
    pub fn with_index(data: &'a DataSet, index: &'a SpatialIndex) -> Self {
        Self {
            data,
            index: Some(index),
            referrers: None,
        }
    }

    /// Full capability.
    pub fn with_all(data: &'a DataSet, index: &'a SpatialIndex, referrers: &'a Referrers) -> Self {
        Self {
            data,
            index: Some(index),
            referrers: Some(referrers),
        }
    }
}

impl MapQuery for QueryView<'_> {
    fn data(&self) -> &DataSet {
        self.data
    }

    fn node(&self, id: ObjectId) -> Option<Node> {
        let handle = self.data.node_handle(id)?;
        let view = self.data.node_view(handle)?;
        Some(Node {
            id: view.info.id,
            pos: view.info.pos,
            tags: self.data.tags_of(view.info.tags),
            meta: view.info.meta,
        })
    }

    fn way(&self, id: ObjectId) -> Option<Way> {
        let handle = self.data.way_handle(id)?;
        let view = self.data.way_view(handle)?;
        let nodes = view
            .node_refs
            .iter()
            .filter_map(|h| self.data.nodes.id(*h))
            .collect();
        Some(Way {
            id: view.info.id,
            nodes,
            tags: self.data.tags_of(view.info.tags),
            meta: view.info.meta,
        })
    }

    fn relation(&self, id: ObjectId) -> Option<Relation> {
        let handle = self.data.relation_handle(id)?;
        let view = self.data.relation_view(handle)?;
        let members = view
            .members
            .iter()
            .filter_map(|m| {
                let (kind, target_id) = match m.target.kind {
                    ObjectKind::Node => (ObjectKind::Node, self.data.nodes.id(m.target.as_node()?)),
                    ObjectKind::Way => (ObjectKind::Way, self.data.ways.id(m.target.as_way()?)),
                    ObjectKind::Relation => (
                        ObjectKind::Relation,
                        self.data.relations.id(m.target.as_relation()?),
                    ),
                    ObjectKind::Changeset => return None,
                };
                Some(neo_osm::RelationMember {
                    kind,
                    id: target_id?,
                    role: m.role,
                })
            })
            .collect();
        Some(Relation {
            id: view.info.id,
            members,
            tags: self.data.tags_of(view.info.tags),
            meta: view.info.meta,
        })
    }

    fn parent_ways(&self, node: ObjectId) -> Vec<ObjectId> {
        let Some(referrers) = self.referrers else {
            return Vec::new();
        };
        let Some(handle) = self.data.node_handle(node) else {
            return Vec::new();
        };
        referrers
            .ways_of_node(handle)
            .iter()
            .filter_map(|h| self.data.ways.id(*h))
            .collect()
    }

    fn parent_relations(&self, kind: ObjectKind, id: ObjectId) -> Vec<ObjectId> {
        let Some(referrers) = self.referrers else {
            return Vec::new();
        };
        let handles: Vec<neo_core::Handle<neo_store::RelationSlot>> = match kind {
            ObjectKind::Node => self
                .data
                .node_handle(id)
                .map(|h| referrers.relations_of_node(h).to_vec())
                .unwrap_or_default(),
            ObjectKind::Way => self
                .data
                .way_handle(id)
                .map(|h| referrers.relations_of_way(h).to_vec())
                .unwrap_or_default(),
            _ => Vec::new(),
        };
        handles
            .iter()
            .filter_map(|h| self.data.relations.id(*h))
            .collect()
    }

    fn nodes_in(&self, bounds: WorldBounds) -> Vec<ObjectId> {
        let Some(index) = self.index else {
            return Vec::new();
        };
        index
            .query_nodes(bounds)
            .iter()
            .filter_map(|e| e.as_node())
            .filter_map(|h| self.data.nodes.id(h))
            .collect()
    }

    fn ways_in(&self, bounds: WorldBounds) -> Vec<ObjectId> {
        let Some(index) = self.index else {
            return Vec::new();
        };
        index
            .query_ways(bounds)
            .iter()
            .filter_map(|e| e.as_way())
            .filter_map(|h| self.data.ways.id(h))
            .collect()
    }

    fn nearest_node(&self, point: WorldPoint, max_meters: f64) -> Option<(ObjectId, f64)> {
        let index = self.index?;
        let bounds = WorldBounds::from_point(point).expanded_by(max_meters);
        index
            .query_nodes(bounds)
            .into_iter()
            .filter_map(|e| {
                let handle = e.as_node()?;
                let pos = self.data.nodes.position(handle)?;
                let id = self.data.nodes.id(handle)?;
                Some((id, WorldPoint::from_latlon(pos).dist(point)))
            })
            .filter(|(_, d)| *d <= max_meters)
            .min_by(|a, b| a.1.total_cmp(&b.1))
    }
}

/// Immutable map snapshot (can be held across threads and published atomically).
///
/// The current implementation uses `Arc<DataSet>`; **note**: publishing a new snapshot after the editor thread commits a transaction requires deep-copying
/// the dataset at O(n) cost. The first prototype only publishes snapshots on "import finished / large batch changes"; interactive editing
/// rendering uses the single-threaded incremental path. The real COW/chunked snapshot design is in docs/07 and ADR-0006.
#[derive(Clone)]
pub struct MapSnapshot {
    data: Arc<DataSet>,
    index: Option<Arc<SpatialIndex>>,
    referrers: Option<Arc<Referrers>>,
}

impl MapSnapshot {
    /// Dataset only.
    pub fn new(data: Arc<DataSet>) -> Self {
        Self {
            data,
            index: None,
            referrers: None,
        }
    }

    /// Dataset + built index.
    pub fn with_index(data: Arc<DataSet>, index: Arc<SpatialIndex>) -> Self {
        Self {
            data,
            index: Some(index),
            referrers: None,
        }
    }

    /// Dataset + index + reverse references.
    pub fn with_all(data: Arc<DataSet>, index: Arc<SpatialIndex>, referrers: Arc<Referrers>) -> Self {
        Self {
            data,
            index: Some(index),
            referrers: Some(referrers),
        }
    }

    /// Get a read-only query view.
    pub fn view(&self) -> QueryView<'_> {
        QueryView {
            data: &self.data,
            index: self.index.as_deref(),
            referrers: self.referrers.as_deref(),
        }
    }

    /// Underlying dataset Arc.
    pub fn data(&self) -> &Arc<DataSet> {
        &self.data
    }

    /// Snapshot version (equals the data version).
    pub fn version(&self) -> u64 {
        self.data.version()
    }
}

/// Container that publishes/reads snapshots via `arc_swap` (lock-free read path).
///
/// The writer (editor thread) calls `store(new_snapshot)` after committing a transaction; any number of readers call `load()` and get
/// a stable view without seeing half-updated state.
pub struct SnapshotCell {
    cell: arc_swap::ArcSwap<MapSnapshot>,
}

impl SnapshotCell {
    /// Construct with an initial snapshot.
    pub fn new(initial: MapSnapshot) -> Self {
        Self {
            cell: arc_swap::ArcSwap::from_pointee(initial),
        }
    }

    /// Read the current snapshot.
    pub fn load(&self) -> arc_swap::Guard<Arc<MapSnapshot>> {
        self.cell.load()
    }

    /// Publish a new snapshot.
    pub fn store(&self, snapshot: MapSnapshot) {
        self.cell.store(Arc::new(snapshot));
    }
}

impl Default for SnapshotCell {
    fn default() -> Self {
        Self::new(MapSnapshot::new(Arc::new(DataSet::new())))
    }
}
