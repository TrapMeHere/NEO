//! Node world-coordinate cache.
//!
//! WebMercator projection involves `tan/ln/cos`, on the order of about 100 ns per call; one way has 2–10 nodes,
//! while scene building processes tens of thousands to hundreds of thousands of ways per frame. If every geometry build re-projected, projection itself
//! would become the largest cost (confirmed by prototype measurements).
//!
//! Therefore "node → world coordinates" is computed once (16 B/node) and geometry building only does table lookups.
//! Editing a node's coordinate invalidates a single point via `update()`, O(1). This is part of ADR-0009's incremental strategy.

use neo_core::Handle;
use neo_store::{DataSet, NodeSlot};

use crate::world::WorldPoint;

/// Node world-coordinate cache (indexed by node slot index).
#[derive(Debug)]
pub struct WorldCache {
    positions: Vec<Option<WorldPoint>>,
}

impl WorldCache {
    /// Build fully from the dataset, O(n).
    pub fn build(data: &DataSet) -> Self {
        let mut positions = vec![None; data.nodes.slot_len()];
        for node in data.iter_nodes() {
            positions[node.handle.index() as usize] = Some(WorldPoint::from_latlon(node.pos));
        }
        Self { positions }
    }

    /// Look up a node's world coordinates.
    #[inline]
    pub fn get(&self, handle: Handle<NodeSlot>) -> Option<WorldPoint> {
        self.positions.get(handle.index() as usize).copied().flatten()
    }

    /// Update a single point.
    pub fn update(&mut self, handle: Handle<NodeSlot>, pos: neo_core::LatLon) {
        let idx = handle.index() as usize;
        if idx >= self.positions.len() {
            self.positions.resize(idx + 1, None);
        }
        self.positions[idx] = Some(WorldPoint::from_latlon(pos));
    }

    /// Clear a slot (called when a node is physically deleted/the slot is reclaimed).
    pub fn clear(&mut self, index: u32) {
        if let Some(slot) = self.positions.get_mut(index as usize) {
            *slot = None;
        }
    }

    /// Estimate memory.
    pub fn mem_bytes(&self) -> neo_core::ByteSize {
        neo_core::ByteSize(self.positions.capacity() * core::mem::size_of::<Option<WorldPoint>>())
    }
}
