//! SoA storage for relations.
//!
//! Members are `(role, target)` pairs, where `target` is [`AnyHandle`] —— kind + slot + generation.
//! Member **order is data** (route / multipolygon semantics), so this storage only appends and never reorders.

use neo_core::{ByteSize, Handle, Meta, ObjectId, ObjectKind, SlotAllocator, Sym};

use crate::node_store::NodeSlot;
use crate::tag_store::TagSetId;
use crate::way_store::WaySlot;

/// Marker type.
#[derive(Debug)]
pub struct RelationSlot;

/// Handle to any primitive (kind + slot + generation).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct AnyHandle {
    /// Object kind.
    pub kind: ObjectKind,
    /// Slot index.
    pub index: u32,
    /// Slot generation.
    pub generation: u32,
}

impl AnyHandle {
    /// Construct from a node handle.
    #[inline]
    pub fn from_node(h: Handle<NodeSlot>) -> Self {
        Self {
            kind: ObjectKind::Node,
            index: h.index(),
            generation: h.generation(),
        }
    }

    /// Construct from a way handle.
    #[inline]
    pub fn from_way(h: Handle<WaySlot>) -> Self {
        Self {
            kind: ObjectKind::Way,
            index: h.index(),
            generation: h.generation(),
        }
    }

    /// Construct from a relation handle.
    #[inline]
    pub fn from_relation(h: Handle<RelationSlot>) -> Self {
        Self {
            kind: ObjectKind::Relation,
            index: h.index(),
            generation: h.generation(),
        }
    }

    /// Convert back to a node handle (returns `None` if the kind does not match).
    #[inline]
    pub fn as_node(self) -> Option<Handle<NodeSlot>> {
        (self.kind == ObjectKind::Node).then(|| Handle::new(self.index, self.generation))
    }

    /// Convert back to a way handle.
    #[inline]
    pub fn as_way(self) -> Option<Handle<WaySlot>> {
        (self.kind == ObjectKind::Way).then(|| Handle::new(self.index, self.generation))
    }

    /// Convert back to a relation handle.
    #[inline]
    pub fn as_relation(self) -> Option<Handle<RelationSlot>> {
        (self.kind == ObjectKind::Relation).then(|| Handle::new(self.index, self.generation))
    }
}

/// One relation member.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StoredMember {
    /// Role (interned symbol).
    pub role: Sym,
    /// Target primitive handle.
    pub target: AnyHandle,
}

/// Read-only snapshot of a Relation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RelationRef {
    /// Handle.
    pub handle: Handle<RelationSlot>,
    /// External ID.
    pub id: ObjectId,
    /// Tag set.
    pub tags: TagSetId,
    /// Metadata.
    pub meta: Meta,
    /// Start of the member range.
    pub first_member: u32,
    /// Number of members.
    pub member_count: u32,
}

/// SoA storage for relations.
#[derive(Debug, Default)]
pub struct RelationStore {
    ids: Vec<ObjectId>,
    tags: Vec<TagSetId>,
    meta: Vec<Meta>,
    offsets: Vec<u32>,
    counts: Vec<u32>,
    members: Vec<StoredMember>,
    alloc: SlotAllocator,
}

impl RelationStore {
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

    /// Number of live relations.
    #[inline]
    pub fn len(&self) -> usize {
        self.alloc.live_len()
    }

    /// Whether there are no relations.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Total number of members.
    #[inline]
    pub fn member_count(&self) -> usize {
        self.members.len()
    }

    /// Append a relation.
    pub fn push(
        &mut self,
        id: ObjectId,
        members: &[StoredMember],
        tags: TagSetId,
        meta: Meta,
    ) -> Handle<RelationSlot> {
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
        self.offsets[i] = self.members.len() as u32;
        self.counts[i] = members.len() as u32;
        self.members.extend_from_slice(members);
        Handle::new(index, generation)
    }

    /// Whether a handle is valid.
    #[inline]
    pub fn contains(&self, handle: Handle<RelationSlot>) -> bool {
        self.alloc.is_valid(handle)
    }

    /// Current generation of a slot.
    #[inline]
    pub fn generation(&self, index: u32) -> Option<u32> {
        self.alloc.generation(index)
    }

    /// Read a snapshot.
    pub fn get(&self, handle: Handle<RelationSlot>) -> Option<RelationRef> {
        if !self.contains(handle) {
            return None;
        }
        let i = handle.index() as usize;
        Some(RelationRef {
            handle,
            id: self.ids[i],
            tags: self.tags[i],
            meta: self.meta[i],
            first_member: self.offsets[i],
            member_count: self.counts[i],
        })
    }

    /// External ID.
    #[inline]
    pub fn id(&self, handle: Handle<RelationSlot>) -> Option<ObjectId> {
        self.contains(handle).then(|| self.ids[handle.index() as usize])
    }

    /// Slice of members.
    #[inline]
    pub fn members_of(&self, handle: Handle<RelationSlot>) -> Option<&[StoredMember]> {
        if !self.contains(handle) {
            return None;
        }
        let i = handle.index() as usize;
        let start = self.offsets[i] as usize;
        let end = start + self.counts[i] as usize;
        Some(&self.members[start..end])
    }

    /// Replace members (append-only).
    pub fn set_members(&mut self, handle: Handle<RelationSlot>, members: &[StoredMember]) -> bool {
        if !self.contains(handle) {
            return false;
        }
        let i = handle.index() as usize;
        self.offsets[i] = self.members.len() as u32;
        self.counts[i] = members.len() as u32;
        self.members.extend_from_slice(members);
        true
    }

    /// Replace the external ID (mapped to the new server ID after a successful upload).
    pub fn set_id(&mut self, handle: Handle<RelationSlot>, id: ObjectId) -> bool {
        if !self.contains(handle) {
            return false;
        }
        self.ids[handle.index() as usize] = id;
        true
    }

    /// Replace the tag set.
    pub fn set_tags(&mut self, handle: Handle<RelationSlot>, tags: TagSetId) -> bool {
        if !self.contains(handle) {
            return false;
        }
        self.tags[handle.index() as usize] = tags;
        true
    }

    /// Replace metadata.
    pub fn set_meta(&mut self, handle: Handle<RelationSlot>, meta: Meta) -> bool {
        if !self.contains(handle) {
            return false;
        }
        self.meta[handle.index() as usize] = meta;
        true
    }

    /// Delete (logical delete + slot recycling).
    pub fn remove(&mut self, handle: Handle<RelationSlot>) -> Option<RelationRef> {
        let view = self.get(handle)?;
        self.alloc.free(handle.index());
        Some(view)
    }

    /// Iterate snapshots of live relations.
    pub fn iter(&self) -> impl Iterator<Item = RelationRef> + '_ {
        (0..self.slot_len()).filter_map(|i| {
            let generation = self.alloc.generation(i as u32)?;
            self.get(Handle::new(i as u32, generation))
        })
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
                + self.members.capacity() * size_of::<StoredMember>()
                + self.alloc.mem_bytes(),
        )
    }
}
