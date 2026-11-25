//! SoA storage for ways.
//!
//! A Way's node references are not `Vec<Vec<...>>` but a **contiguous reference pool + per-way (offset, len)**:
//!
//! ```text
//! way 0 -> offsets[0]=0, count[0]=3  -> node_refs[0..3]
//! way 1 -> offsets[1]=3, count[1]=5  -> node_refs[3..8]
//! ```
//!
//! Editing a way's node sequence appends a new range and updates offset/count; the old range is reclaimed on compact;
//! this keeps the reference pool append-only, so existing ways' references never move (otherwise all indexes would need updating).

use neo_core::{ByteSize, Handle, Meta, ObjectId, SlotAllocator};

use crate::node_store::{NodeSlot, NodeStore};
use crate::tag_store::TagSetId;

/// Marker type.
#[derive(Debug)]
pub struct WaySlot;

/// Read-only snapshot of a Way.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WayRef {
    /// Handle.
    pub handle: Handle<WaySlot>,
    /// External ID.
    pub id: ObjectId,
    /// Tag set.
    pub tags: TagSetId,
    /// Metadata.
    pub meta: Meta,
    /// Start of the node reference range.
    pub first_node: u32,
    /// Number of nodes.
    pub node_count: u32,
}

/// SoA storage for ways.
#[derive(Debug, Default)]
pub struct WayStore {
    ids: Vec<ObjectId>,
    tags: Vec<TagSetId>,
    meta: Vec<Meta>,
    offsets: Vec<u32>,
    counts: Vec<u32>,
    node_refs: Vec<Handle<NodeSlot>>,
    alloc: SlotAllocator,
}

impl WayStore {
    /// Empty storage.
    pub fn new() -> Self {
        Self::default()
    }

    /// Preallocate.
    pub fn with_capacity(capacity: usize) -> Self {
        let mut s = Self::new();
        s.ids.reserve(capacity);
        s.tags.reserve(capacity);
        s.meta.reserve(capacity);
        s.offsets.reserve(capacity);
        s.counts.reserve(capacity);
        s.alloc.reserve(capacity);
        s
    }

    /// Total number of slots.
    #[inline]
    pub fn slot_len(&self) -> usize {
        self.ids.len()
    }

    /// Number of live ways.
    #[inline]
    pub fn len(&self) -> usize {
        self.alloc.live_len()
    }

    /// Whether there are no ways.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Total number of node references in the reference pool.
    #[inline]
    pub fn ref_count(&self) -> usize {
        self.node_refs.len()
    }

    /// Append a way.
    pub fn push(
        &mut self,
        id: ObjectId,
        nodes: &[Handle<NodeSlot>],
        tags: TagSetId,
        meta: Meta,
    ) -> Handle<WaySlot> {
        let (index, generation) = self.alloc.alloc();
        let i = index as usize;
        if i >= self.ids.len() {
            self.ids.resize(i + 1, ObjectId::NONE);
            self.tags.resize(i + 1, TagSetId::EMPTY);
            self.meta.resize(i + 1, Meta::new_local());
            self.offsets.resize(i + 1, 0);
            self.counts.resize(i + 1, 0);
        }
        self.ids[i] = id;
        self.tags[i] = tags;
        self.meta[i] = meta;
        self.offsets[i] = self.node_refs.len() as u32;
        self.counts[i] = nodes.len() as u32;
        self.node_refs.extend_from_slice(nodes);
        Handle::new(index, generation)
    }

    /// Whether a handle is valid.
    #[inline]
    pub fn contains(&self, handle: Handle<WaySlot>) -> bool {
        self.alloc.is_valid(handle)
    }

    /// Current generation of a slot.
    #[inline]
    pub fn generation(&self, index: u32) -> Option<u32> {
        self.alloc.generation(index)
    }

    /// Read a snapshot.
    pub fn get(&self, handle: Handle<WaySlot>) -> Option<WayRef> {
        if !self.contains(handle) {
            return None;
        }
        let i = handle.index() as usize;
        Some(WayRef {
            handle,
            id: self.ids[i],
            tags: self.tags[i],
            meta: self.meta[i],
            first_node: self.offsets[i],
            node_count: self.counts[i],
        })
    }

    /// External ID.
    #[inline]
    pub fn id(&self, handle: Handle<WaySlot>) -> Option<ObjectId> {
        self.contains(handle).then(|| self.ids[handle.index() as usize])
    }

    /// Slice of node references (hot path for geometry building).
    #[inline]
    pub fn nodes(&self, handle: Handle<WaySlot>) -> Option<&[Handle<NodeSlot>]> {
        if !self.contains(handle) {
            return None;
        }
        let i = handle.index() as usize;
        let start = self.offsets[i] as usize;
        let end = start + self.counts[i] as usize;
        Some(&self.node_refs[start..end])
    }

    /// Replace the node sequence (append-only); returns the old reference range handle.
    pub fn set_nodes(&mut self, handle: Handle<WaySlot>, nodes: &[Handle<NodeSlot>]) -> Option<u32> {
        if !self.contains(handle) {
            return None;
        }
        let i = handle.index() as usize;
        let old_start = self.offsets[i];
        self.offsets[i] = self.node_refs.len() as u32;
        self.counts[i] = nodes.len() as u32;
        self.node_refs.extend_from_slice(nodes);
        Some(old_start)
    }

    /// Replace the external ID (mapped to the new server ID after a successful upload).
    pub fn set_id(&mut self, handle: Handle<WaySlot>, id: ObjectId) -> bool {
        if !self.contains(handle) {
            return false;
        }
        self.ids[handle.index() as usize] = id;
        true
    }

    /// Replace the tag set.
    pub fn set_tags(&mut self, handle: Handle<WaySlot>, tags: TagSetId) -> bool {
        if !self.contains(handle) {
            return false;
        }
        self.tags[handle.index() as usize] = tags;
        true
    }

    /// Replace metadata.
    pub fn set_meta(&mut self, handle: Handle<WaySlot>, meta: Meta) -> bool {
        if !self.contains(handle) {
            return false;
        }
        self.meta[handle.index() as usize] = meta;
        true
    }

    /// Delete (logical delete + slot recycling).
    pub fn remove(&mut self, handle: Handle<WaySlot>) -> Option<WayRef> {
        let view = self.get(handle)?;
        self.alloc.free(handle.index());
        Some(view)
    }

    /// Iterate snapshots of live ways.
    pub fn iter(&self) -> impl Iterator<Item = WayRef> + '_ {
        (0..self.slot_len()).filter_map(|i| {
            let generation = self.alloc.generation(i as u32)?;
            self.get(Handle::new(i as u32, generation))
        })
    }

    /// Geometry convenience: resolve node references into a coordinate sequence (skipping stale nodes).
    pub fn positions<'a>(
        &'a self,
        handle: Handle<WaySlot>,
        nodes: &'a NodeStore,
    ) -> Option<impl Iterator<Item = neo_core::LatLon> + 'a> {
        Some(self.nodes(handle)?.iter().filter_map(move |h| nodes.position(*h)))
    }

    /// Estimate heap memory.
    pub fn mem_bytes(&self) -> ByteSize {
        use core::mem::size_of;
        ByteSize(
            self.ids.capacity() * size_of::<ObjectId>()
                + self.tags.capacity() * size_of::<TagSetId>()
                + self.meta.capacity() * size_of::<Meta>()
                + self.offsets.capacity() * size_of::<u32>()
                + self.counts.capacity() * size_of::<u32>()
                + self.node_refs.capacity() * size_of::<Handle<NodeSlot>>()
                + self.alloc.mem_bytes(),
        )
    }
}
