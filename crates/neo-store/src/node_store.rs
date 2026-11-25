//! SoA storage for nodes.
//!
//! ```text
//! ids   [ i64 | i64 | i64 | ... ]   ┐
//! lat   [ i32 | i32 | i32 | ... ]   │ the same slot index i corresponds to the same object
//! lon   [ i32 | i32 | i32 | ... ]   │
//! tags  [ u32 | u32 | u32 | ... ]   │
//! meta  [ 32B | 32B | 32B | ... ]   ┘
//! alive [ 1B  | 1B  | ... ]         ← managed by SlotAllocator
//! ```
//!
//! 10M nodes: 4+4+4+4+32 = 48 bytes/node ≈ 480 MB (about 700 MB with bitsets and the id index),
//! compared with 100+ bytes/node plus pointer chasing for `Vec<Box<Node>>`, this is an order-of-magnitude difference.

use neo_core::{Handle, LatLon, Meta, ObjectId, SlotAllocator};
use neo_osm::Tags;

use crate::tag_store::{TagSetId, TagStore};

/// Read-only snapshot view of a Node (tag contents excluded; resolve via `TagStore`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NodeRef {
    /// Handle.
    pub handle: Handle<NodeSlot>,
    /// External ID.
    pub id: ObjectId,
    /// Coordinates.
    pub pos: LatLon,
    /// Tag set handle.
    pub tags: TagSetId,
    /// Metadata.
    pub meta: Meta,
}

/// Marker type: the phantom parameter of the handle.
#[derive(Debug)]
pub struct NodeSlot;

/// SoA storage for nodes.
#[derive(Debug, Default)]
pub struct NodeStore {
    ids: Vec<ObjectId>,
    lat: Vec<i32>,
    lon: Vec<i32>,
    tags: Vec<TagSetId>,
    meta: Vec<Meta>,
    alloc: SlotAllocator,
}

impl NodeStore {
    /// Empty storage.
    pub fn new() -> Self {
        Self::default()
    }

    /// Preallocated capacity.
    pub fn with_capacity(capacity: usize) -> Self {
        let mut s = Self::new();
        s.reserve(capacity);
        s
    }

    /// Preallocate.
    pub fn reserve(&mut self, additional: usize) {
        self.ids.reserve(additional);
        self.lat.reserve(additional);
        self.lon.reserve(additional);
        self.tags.reserve(additional);
        self.meta.reserve(additional);
        self.alloc.reserve(additional);
    }

    /// Total number of slots (including deleted).
    #[inline]
    pub fn slot_len(&self) -> usize {
        self.ids.len()
    }

    /// Number of live nodes.
    #[inline]
    pub fn len(&self) -> usize {
        self.alloc.live_len()
    }

    /// Whether there are no live nodes.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Append a node (import path; does not reuse free slots).
    pub fn push(&mut self, id: ObjectId, pos: LatLon, tags: TagSetId, meta: Meta) -> Handle<NodeSlot> {
        let (index, generation) = self.alloc.alloc();
        let i = index as usize;
        if i >= self.ids.len() {
            self.ids.resize(i + 1, ObjectId::NONE);
            self.lat.resize(i + 1, 0);
            self.lon.resize(i + 1, 0);
            self.tags.resize(i + 1, TagSetId::EMPTY);
            self.meta.resize(i + 1, Meta::new_local());
        }
        self.ids[i] = id;
        self.lat[i] = pos.lat.units();
        self.lon[i] = pos.lon.units();
        self.tags[i] = tags;
        self.meta[i] = meta;
        Handle::new(index, generation)
    }

    /// Whether a handle is valid.
    #[inline]
    pub fn contains(&self, handle: Handle<NodeSlot>) -> bool {
        self.alloc.is_valid(handle)
    }

    /// Current generation of a slot (used to reconstruct handles from the dirty bitset).
    #[inline]
    pub fn generation(&self, index: u32) -> Option<u32> {
        self.alloc.generation(index)
    }

    /// Read a node snapshot.
    pub fn get(&self, handle: Handle<NodeSlot>) -> Option<NodeRef> {
        if !self.contains(handle) {
            return None;
        }
        let i = handle.index() as usize;
        Some(NodeRef {
            handle,
            id: self.ids[i],
            pos: LatLon::new(
                neo_core::Coord::from_units(self.lat[i]),
                neo_core::Coord::from_units(self.lon[i]),
            ),
            tags: self.tags[i],
            meta: self.meta[i],
        })
    }

    /// External ID.
    #[inline]
    pub fn id(&self, handle: Handle<NodeSlot>) -> Option<ObjectId> {
        self.contains(handle).then(|| self.ids[handle.index() as usize])
    }

    /// Coordinates (render/geometry hot path, avoiding construction of a full view).
    #[inline]
    pub fn position(&self, handle: Handle<NodeSlot>) -> Option<LatLon> {
        if !self.contains(handle) {
            return None;
        }
        let i = handle.index() as usize;
        Some(LatLon::new(
            neo_core::Coord::from_units(self.lat[i]),
            neo_core::Coord::from_units(self.lon[i]),
        ))
    }

    /// Modify coordinates and return the old value. Returns `None` for a stale handle.
    pub fn set_position(&mut self, handle: Handle<NodeSlot>, pos: LatLon) -> Option<LatLon> {
        if !self.contains(handle) {
            return None;
        }
        let i = handle.index() as usize;
        let old = LatLon::new(
            neo_core::Coord::from_units(self.lat[i]),
            neo_core::Coord::from_units(self.lon[i]),
        );
        self.lat[i] = pos.lat.units();
        self.lon[i] = pos.lon.units();
        Some(old)
    }

    /// Replace the external ID (maps a negative local ID to the new server ID after a successful upload).
    pub fn set_id(&mut self, handle: Handle<NodeSlot>, id: ObjectId) -> bool {
        if !self.contains(handle) {
            return false;
        }
        self.ids[handle.index() as usize] = id;
        true
    }

    /// Replace the tag set handle.
    pub fn set_tags(&mut self, handle: Handle<NodeSlot>, tags: TagSetId) -> bool {
        if !self.contains(handle) {
            return false;
        }
        self.tags[handle.index() as usize] = tags;
        true
    }

    /// Replace metadata.
    pub fn set_meta(&mut self, handle: Handle<NodeSlot>, meta: Meta) -> bool {
        if !self.contains(handle) {
            return false;
        }
        self.meta[handle.index() as usize] = meta;
        true
    }

    /// Delete a node (logical delete + slot recycling).
    pub fn remove(&mut self, handle: Handle<NodeSlot>) -> Option<NodeRef> {
        let view = self.get(handle)?;
        self.alloc.free(handle.index());
        Some(view)
    }

    /// Iterate snapshots of all live nodes.
    pub fn iter(&self) -> impl Iterator<Item = NodeRef> + '_ {
        (0..self.slot_len()).filter_map(|i| {
            let generation = self.alloc.generation(i as u32)?;
            self.get(Handle::new(i as u32, generation))
        })
    }

    /// Resolve tags into a domain object (for cases that need pool access).
    pub fn tags_of(&self, handle: Handle<NodeSlot>, tags: &TagStore) -> Option<Tags> {
        let t = self.get(handle)?;
        Some(Tags::from_pairs(tags.iter(t.tags)))
    }

    /// Estimate heap memory.
    pub fn mem_bytes(&self) -> neo_core::ByteSize {
        use core::mem::size_of;
        use neo_core::ByteSize;
        ByteSize(
            self.ids.capacity() * size_of::<ObjectId>()
                + self.lat.capacity() * size_of::<i32>()
                + self.lon.capacity() * size_of::<i32>()
                + self.tags.capacity() * size_of::<TagSetId>()
                + self.meta.capacity() * size_of::<Meta>()
                + self.alloc.mem_bytes(),
        )
    }
}
