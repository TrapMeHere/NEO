//! `DataSet`: the complete in-memory representation of one OSM data set.
//!
//! Composition:
//!
//! * [`StringPool`] + [`TagStore`]: the single home for all text and tags;
//! * [`NodeStore`] / [`WayStore`] / [`RelationStore`]: SoA data;
//! * [`IdIndex`]: external ID to handle;
//! * [`ChangeTrack`]: pending upload changes;
//! * dirty bitsets: the basis for incremental render/geometry/validation updates (see ADR-0009).
//!
//! **Threading model**: `DataSet` is only held as `&mut` by a single writer (the editor main thread / command
//! execution context). Rendering and background tasks read immutable snapshots that have been published
//! (`Arc<...>`), see docs/07.

use neo_core::{ByteSize, Handle, LatLon, Meta, NeoError, ObjectId, ObjectKind, Result, StringPool, Sym};
use neo_osm::{ChangeState, Node, Relation, Tags, Way};

use crate::bitset::BitSet;
use crate::indexes::{ChangeTrack, IdIndex, Referrers};
use crate::node_store::{NodeRef, NodeSlot, NodeStore};
use crate::relation_store::{AnyHandle, RelationRef, RelationSlot, RelationStore, StoredMember};
use crate::tag_store::{TagSetId, TagStore};
use crate::way_store::{WayRef, WaySlot, WayStore};

/// Import policy: skip or error out when encountering a dangling reference.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MissingPolicy {
    /// Skip the object (counting it). Used for partial data and incomplete downloads.
    Skip,
    /// Return [`NeoError::DanglingReference`]. Used for integrity checks.
    Error,
}

/// Full view of a Node (including tag contents).
#[derive(Clone, Copy, Debug)]
pub struct NodeView<'a> {
    /// Snapshot info.
    pub info: NodeRef,
    /// Slice of tag pairs.
    pub tag_pairs: &'a [(Sym, Sym)],
}

impl NodeView<'_> {
    /// External ID.
    #[inline]
    pub fn id(&self) -> ObjectId {
        self.info.id
    }

    /// Coordinates.
    #[inline]
    pub fn pos(&self) -> LatLon {
        self.info.pos
    }

    /// Metadata.
    #[inline]
    pub fn meta(&self) -> Meta {
        self.info.meta
    }

    /// Look up a tag.
    #[inline]
    pub fn tag(&self, key: Sym) -> Option<Sym> {
        self.tag_pairs.iter().find_map(|&(k, v)| (k == key).then_some(v))
    }

    /// Iterate tags.
    #[inline]
    pub fn tags(&self) -> impl Iterator<Item = (Sym, Sym)> + '_ {
        self.tag_pairs.iter().copied()
    }
}

/// Full view of a Way.
#[derive(Clone, Copy, Debug)]
pub struct WayView<'a> {
    /// Snapshot info.
    pub info: WayRef,
    /// Slice of tag pairs.
    pub tag_pairs: &'a [(Sym, Sym)],
    /// Node references.
    pub node_refs: &'a [Handle<NodeSlot>],
}

impl WayView<'_> {
    /// External ID.
    #[inline]
    pub fn id(&self) -> ObjectId {
        self.info.id
    }

    /// Look up a tag.
    #[inline]
    pub fn tag(&self, key: Sym) -> Option<Sym> {
        self.tag_pairs.iter().find_map(|&(k, v)| (k == key).then_some(v))
    }

    /// Iterate tags.
    #[inline]
    pub fn tags(&self) -> impl Iterator<Item = (Sym, Sym)> + '_ {
        self.tag_pairs.iter().copied()
    }

    /// Whether the way is closed (first and last references are identical).
    pub fn is_closed(&self) -> bool {
        match (self.node_refs.first(), self.node_refs.last()) {
            (Some(a), Some(b)) => a == b && self.node_refs.len() >= 4,
            _ => false,
        }
    }
}

/// Full view of a Relation.
#[derive(Clone, Copy, Debug)]
pub struct RelationView<'a> {
    /// Snapshot info.
    pub info: RelationRef,
    /// Slice of tag pairs.
    pub tag_pairs: &'a [(Sym, Sym)],
    /// Slice of members.
    pub members: &'a [StoredMember],
}

impl RelationView<'_> {
    /// External ID.
    #[inline]
    pub fn id(&self) -> ObjectId {
        self.info.id
    }

    /// Look up a tag.
    #[inline]
    pub fn tag(&self, key: Sym) -> Option<Sym> {
        self.tag_pairs.iter().find_map(|&(k, v)| (k == key).then_some(v))
    }
}

/// One OSM data set.
#[derive(Debug)]
pub struct DataSet {
    /// String interning pool.
    pub strings: StringPool,
    /// Tag arena.
    pub tags: TagStore,
    /// Nodes.
    pub nodes: NodeStore,
    /// Way。
    pub ways: WayStore,
    /// Relation。
    pub relations: RelationStore,
    /// External ID index.
    pub ids: IdIndex,
    /// Pending upload changes.
    pub changes: ChangeTrack,
    version: u64,
    next_local_id: i64,
    dirty_nodes: BitSet,
    dirty_ways: BitSet,
    dirty_relations: BitSet,
}

impl Default for DataSet {
    fn default() -> Self {
        Self::new()
    }
}

impl DataSet {
    /// Empty data set.
    pub fn new() -> Self {
        Self {
            strings: StringPool::new(),
            tags: TagStore::new(),
            nodes: NodeStore::new(),
            ways: WayStore::new(),
            relations: RelationStore::new(),
            ids: IdIndex::new(),
            changes: ChangeTrack::default(),
            version: 1,
            next_local_id: -1,
            dirty_nodes: BitSet::new(),
            dirty_ways: BitSet::new(),
            dirty_relations: BitSet::new(),
        }
    }

    /// Current data version (incremented on every committed edit; rendering/caches use it for invalidation).
    #[inline]
    pub fn version(&self) -> u64 {
        self.version
    }

    /// Increment the version.
    #[inline]
    pub fn bump_version(&mut self) {
        self.version = self.version.wrapping_add(1);
    }

    /// Allocate an ID for a locally created object (negative, decrementing from -1).
    ///
    /// Skips any ID that is already taken: after importing XML/OSC containing negative IDs, if the counter
    /// does not advance, new objects would collide with existing ones and silently overwrite `IdIndex` (data corruption).
    pub fn next_local_id(&mut self) -> ObjectId {
        loop {
            let id = ObjectId(self.next_local_id);
            self.next_local_id -= 1;
            if self.ids.node(id).is_none() && self.ids.way(id).is_none() && self.ids.relation(id).is_none() {
                return id;
            }
        }
    }

    /// Record an existing negative ID, ensuring a later `next_local_id` does not wrap onto it.
    fn note_existing_local_id(&mut self, id: ObjectId) {
        if id.is_local() && id.0 <= self.next_local_id {
            self.next_local_id = id.0 - 1;
        }
    }

    /// Intern a string.
    #[inline]
    pub fn intern(&mut self, s: &str) -> Sym {
        self.strings.intern(s)
    }

    /// Intern a tag key/value.
    #[inline]
    pub fn intern_tag(&mut self, key: &str, value: &str) -> (Sym, Sym) {
        (self.strings.intern(key), self.strings.intern(value))
    }

    /// Resolve a symbol.
    #[inline]
    pub fn resolve(&self, sym: Sym) -> &str {
        self.strings.resolve(sym)
    }

    /// Add a tag set.
    pub fn add_tags(&mut self, pairs: &[(Sym, Sym)]) -> TagSetId {
        self.tags.add(pairs)
    }

    /// Get the contents of a tag set.
    #[inline]
    pub fn tag_pairs(&self, id: TagSetId) -> &[(Sym, Sym)] {
        self.tags.get(id)
    }

    /// Whether the data set is empty.
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty() && self.ways.is_empty() && self.relations.is_empty()
    }

    /// Number of nodes.
    #[inline]
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    /// Number of Ways.
    #[inline]
    pub fn way_count(&self) -> usize {
        self.ways.len()
    }

    /// Number of Relations.
    #[inline]
    pub fn relation_count(&self) -> usize {
        self.relations.len()
    }

    /// Insert a node DTO.
    ///
    /// Reject duplicate IDs: `IdIndex` is a `HashMap`, and silently overwriting would leave the old object as an
    /// "alive but ID-less" orphan (duplicate IDs appear in render/export, and the server rejects the upload).
    pub fn push_node(&mut self, node: Node) -> Result<Handle<NodeSlot>> {
        if self.ids.node(node.id).is_some() {
            return Err(NeoError::Consistency(format!("duplicate node id {}", node.id.0)));
        }
        self.note_existing_local_id(node.id);
        let tag_id = self.tags.add(node.tags.raw());
        let handle = self.nodes.push(node.id, node.pos, tag_id, node.meta);
        self.ids.insert_node(node.id, handle);
        self.bump_version();
        Ok(handle)
    }

    /// Insert a way DTO.
    pub fn push_way(&mut self, way: Way, policy: MissingPolicy) -> Result<Option<Handle<WaySlot>>> {
        if self.ids.way(way.id).is_some() {
            return Err(NeoError::Consistency(format!("duplicate way id {}", way.id.0)));
        }
        let mut refs = Vec::with_capacity(way.nodes.len());
        for id in &way.nodes {
            match self.ids.node(*id) {
                Some(h) if self.nodes.contains(h) => refs.push(h),
                _ => match policy {
                    MissingPolicy::Skip => return Ok(None),
                    MissingPolicy::Error => {
                        return Err(NeoError::DanglingReference {
                            kind: ObjectKind::Node,
                            id: *id,
                            referrer_kind: ObjectKind::Way,
                            referrer: way.id,
                        });
                    }
                },
            }
        }
        let tag_id = self.tags.add(way.tags.raw());
        let handle = self.ways.push(way.id, &refs, tag_id, way.meta);
        self.ids.insert_way(way.id, handle);
        self.note_existing_local_id(way.id);
        self.bump_version();
        Ok(Some(handle))
    }

    /// Insert a relation DTO.
    pub fn push_relation(
        &mut self,
        relation: Relation,
        policy: MissingPolicy,
    ) -> Result<Option<Handle<RelationSlot>>> {
        if self.ids.relation(relation.id).is_some() {
            return Err(NeoError::Consistency(format!(
                "duplicate relation id {}",
                relation.id.0
            )));
        }
        let mut members = Vec::with_capacity(relation.members.len());
        for m in &relation.members {
            let target = match m.kind {
                ObjectKind::Node => self.ids.node(m.id).map(AnyHandle::from_node),
                ObjectKind::Way => self.ids.way(m.id).map(AnyHandle::from_way),
                ObjectKind::Relation => self.ids.relation(m.id).map(AnyHandle::from_relation),
                ObjectKind::Changeset => None,
            };
            match target {
                Some(target) => members.push(StoredMember { role: m.role, target }),
                None => match policy {
                    MissingPolicy::Skip => return Ok(None),
                    MissingPolicy::Error => {
                        return Err(NeoError::DanglingReference {
                            kind: m.kind,
                            id: m.id,
                            referrer_kind: ObjectKind::Relation,
                            referrer: relation.id,
                        });
                    }
                },
            }
        }
        let tag_id = self.tags.add(relation.tags.raw());
        let handle = self.relations.push(relation.id, &members, tag_id, relation.meta);
        self.ids.insert_relation(relation.id, handle);
        self.note_existing_local_id(relation.id);
        self.bump_version();
        Ok(Some(handle))
    }

    /// Insert an `Element` DTO. Returns the accepted object's ID (`None` when skipped).
    pub fn push_element(
        &mut self,
        element: neo_osm::Element,
        policy: MissingPolicy,
    ) -> Result<Option<ObjectId>> {
        match element {
            neo_osm::Element::Node(n) => {
                let id = n.id;
                self.push_node(n)?;
                Ok(Some(id))
            }
            neo_osm::Element::Way(w) => {
                let id = w.id;
                Ok(self.push_way(w, policy)?.map(|_| id))
            }
            neo_osm::Element::Relation(r) => {
                let id = r.id;
                Ok(self.push_relation(r, policy)?.map(|_| id))
            }
        }
    }

    /// Full Node view.
    pub fn node_view(&self, handle: Handle<NodeSlot>) -> Option<NodeView<'_>> {
        let info = self.nodes.get(handle)?;
        Some(NodeView {
            info,
            tag_pairs: self.tags.get(info.tags),
        })
    }

    /// Full Way view.
    pub fn way_view(&self, handle: Handle<WaySlot>) -> Option<WayView<'_>> {
        let info = self.ways.get(handle)?;
        Some(WayView {
            info,
            tag_pairs: self.tags.get(info.tags),
            node_refs: self.ways.nodes(handle)?,
        })
    }

    /// Full Relation view.
    pub fn relation_view(&self, handle: Handle<RelationSlot>) -> Option<RelationView<'_>> {
        let info = self.relations.get(handle)?;
        Some(RelationView {
            info,
            tag_pairs: self.tags.get(info.tags),
            members: self.relations.members_of(handle)?,
        })
    }

    /// Node coordinates.
    #[inline]
    pub fn position(&self, handle: Handle<NodeSlot>) -> Option<LatLon> {
        self.nodes.position(handle)
    }

    /// Look up a node handle by ID.
    #[inline]
    pub fn node_handle(&self, id: ObjectId) -> Option<Handle<NodeSlot>> {
        self.ids.node(id)
    }

    /// Look up a way handle by ID.
    #[inline]
    pub fn way_handle(&self, id: ObjectId) -> Option<Handle<WaySlot>> {
        self.ids.way(id)
    }

    /// Look up a relation handle by ID.
    #[inline]
    pub fn relation_handle(&self, id: ObjectId) -> Option<Handle<RelationSlot>> {
        self.ids.relation(id)
    }

    /// Node ID sequence of a Way (for display; stale references are skipped).
    ///
    /// Write paths (OSC/PBF export, editing commands) must use [`Self::try_way_node_ids`]:
    /// silently skipping dangling references would write shortened geometry, an irreversible data corruption.
    pub fn way_node_ids(&self, handle: Handle<WaySlot>) -> Vec<ObjectId> {
        let Some(refs) = self.ways.nodes(handle) else {
            return Vec::new();
        };
        refs.iter().filter_map(|h| self.nodes.id(*h)).collect()
    }

    /// Node ID sequence of a Way; returns an error instead of silently compacting when any reference is stale.
    pub fn try_way_node_ids(&self, handle: Handle<WaySlot>) -> Result<Vec<ObjectId>> {
        let Some(refs) = self.ways.nodes(handle) else {
            return Ok(Vec::new());
        };
        let mut out = Vec::with_capacity(refs.len());
        for h in refs {
            let id = self.nodes.id(*h).ok_or(NeoError::StaleHandle {
                kind: ObjectKind::Node,
                index: h.index(),
            })?;
            out.push(id);
        }
        Ok(out)
    }

    /// Modify node coordinates (edit hot path): validate → write → mark dirty → bump version.
    pub fn set_node_position(&mut self, handle: Handle<NodeSlot>, pos: LatLon) -> Result<LatLon> {
        if !pos.is_valid() {
            return Err(NeoError::InvalidCoordinate(format!(
                "lat={}, lon={}",
                pos.lat, pos.lon
            )));
        }
        let old = self
            .nodes
            .set_position(handle, pos)
            .ok_or_else(|| NeoError::StaleHandle {
                kind: ObjectKind::Node,
                index: handle.index(),
            })?;
        self.dirty_nodes.set(handle.index() as usize);
        self.bump_version();
        Ok(old)
    }

    /// Replace a Way's node sequence.
    ///
    /// Validate that every node handle is still valid: otherwise dangling handles would be written into the
    /// reference pool, and later `Referrers`/`WorldCache` accesses by index would go out of bounds or map to the wrong object.
    pub fn set_way_nodes(&mut self, handle: Handle<WaySlot>, refs: &[Handle<NodeSlot>]) -> Result<()> {
        if !self.ways.contains(handle) {
            return Err(NeoError::StaleHandle {
                kind: ObjectKind::Way,
                index: handle.index(),
            });
        }
        for h in refs {
            if !self.nodes.contains(*h) {
                return Err(NeoError::StaleHandle {
                    kind: ObjectKind::Node,
                    index: h.index(),
                });
            }
        }
        self.ways.set_nodes(handle, refs);
        self.dirty_ways.set(handle.index() as usize);
        self.bump_version();
        Ok(())
    }

    /// Mark an object as having pending changes to upload.
    pub fn set_change_state(&mut self, kind: ObjectKind, id: ObjectId, state: ChangeState) {
        self.changes.mark(kind, id, state);
    }

    /// Read an object's change state.
    pub fn change_state(&self, kind: ObjectKind, id: ObjectId) -> ChangeState {
        self.changes.state(kind, id)
    }

    /// Set both the change state and the `visible` metadata (used for delete/undelete).
    pub fn set_change_state_visible(
        &mut self,
        kind: ObjectKind,
        id: ObjectId,
        state: ChangeState,
        visible: bool,
    ) {
        self.changes.mark(kind, id, state);
        match kind {
            ObjectKind::Node => {
                if let Some(handle) = self.ids.node(id)
                    && let Some(view) = self.nodes.get(handle)
                {
                    let mut meta = view.meta;
                    meta.visible = visible;
                    self.nodes.set_meta(handle, meta);
                    self.dirty_nodes.set(handle.index() as usize);
                }
            }
            ObjectKind::Way => {
                if let Some(handle) = self.ids.way(id)
                    && let Some(view) = self.ways.get(handle)
                {
                    let mut meta = view.meta;
                    meta.visible = visible;
                    self.ways.set_meta(handle, meta);
                    self.dirty_ways.set(handle.index() as usize);
                }
            }
            ObjectKind::Relation => {
                if let Some(handle) = self.ids.relation(id)
                    && let Some(view) = self.relations.get(handle)
                {
                    let mut meta = view.meta;
                    meta.visible = visible;
                    self.relations.set_meta(handle, meta);
                    self.dirty_relations.set(handle.index() as usize);
                }
            }
            ObjectKind::Changeset => {}
        }
        // Visibility affects rendering/picking/upload, so caches must be invalidated.
        self.bump_version();
    }

    /// Change an object's external ID from `old` to `new` (negative ID → server ID after a successful upload).
    ///
    /// Referrers (way nds, relation members) store handles, so they need no rewriting; only
    /// `IdIndex` and the object's own ID need updating.
    pub fn reassign_id(&mut self, kind: ObjectKind, old: ObjectId, new: ObjectId) -> bool {
        if old == new {
            return true;
        }
        if self.object_handle_exists(kind, new) {
            return false;
        }
        let updated = match kind {
            ObjectKind::Node => self.ids.node(old).map(|handle| {
                self.ids.remove_node(old);
                self.nodes.set_id(handle, new);
                self.ids.insert_node(new, handle);
            }),
            ObjectKind::Way => self.ids.way(old).map(|handle| {
                self.ids.remove_way(old);
                self.ways.set_id(handle, new);
                self.ids.insert_way(new, handle);
            }),
            ObjectKind::Relation => self.ids.relation(old).map(|handle| {
                self.ids.remove_relation(old);
                self.relations.set_id(handle, new);
                self.ids.insert_relation(new, handle);
            }),
            ObjectKind::Changeset => None,
        }
        .is_some();
        if updated {
            self.changes.mark(kind, old, ChangeState::Unchanged);
            self.bump_version();
        }
        updated
    }

    fn object_handle_exists(&self, kind: ObjectKind, id: ObjectId) -> bool {
        match kind {
            ObjectKind::Node => self.ids.node(id).is_some(),
            ObjectKind::Way => self.ids.way(id).is_some(),
            ObjectKind::Relation => self.ids.relation(id).is_some(),
            ObjectKind::Changeset => false,
        }
    }

    /// Modify an object's version (write back the server version after a successful upload).
    pub fn set_version(&mut self, kind: ObjectKind, id: ObjectId, version: u32) -> bool {
        let updated = match kind {
            ObjectKind::Node => self.ids.node(id).and_then(|h| {
                let mut meta = self.nodes.get(h)?.meta;
                meta.version = version;
                Some(self.nodes.set_meta(h, meta))
            }),
            ObjectKind::Way => self.ids.way(id).and_then(|h| {
                let mut meta = self.ways.get(h)?.meta;
                meta.version = version;
                Some(self.ways.set_meta(h, meta))
            }),
            ObjectKind::Relation => self.ids.relation(id).and_then(|h| {
                let mut meta = self.relations.get(h)?.meta;
                meta.version = version;
                Some(self.relations.set_meta(h, meta))
            }),
            ObjectKind::Changeset => None,
        }
        .unwrap_or(false);
        if updated {
            self.bump_version();
        }
        updated
    }

    /// Mark an object as synced (clear its dirty state after a successful upload).
    pub fn mark_synced(&mut self, kind: ObjectKind, id: ObjectId) {
        self.changes.mark(kind, id, ChangeState::Unchanged);
    }

    /// Whether all changes have been cleared.
    pub fn has_pending_changes(&self) -> bool {
        !self.changes.is_empty()
    }

    // ---- dirty sets ----

    /// Mark a way's geometry/style as needing rebuild.
    #[inline]
    pub fn mark_way_dirty(&mut self, handle: Handle<WaySlot>) {
        self.dirty_ways.set(handle.index() as usize);
    }

    /// Mark a node dirty (node icon / picking index).
    #[inline]
    pub fn mark_node_dirty(&mut self, handle: Handle<NodeSlot>) {
        self.dirty_nodes.set(handle.index() as usize);
    }

    /// Mark a relation dirty (member/tag changes).
    #[inline]
    pub fn mark_relation_dirty(&mut self, handle: Handle<RelationSlot>) {
        self.dirty_relations.set(handle.index() as usize);
    }

    /// Take and clear the list of dirty ways.
    pub fn take_dirty_ways(&mut self) -> Vec<Handle<WaySlot>> {
        let out: Vec<Handle<WaySlot>> = self
            .dirty_ways
            .iter_set()
            .filter_map(|i| {
                let generation = self.ways.generation(i as u32)?;
                let handle = Handle::new(i as u32, generation);
                self.ways.contains(handle).then_some(handle)
            })
            .collect();
        self.dirty_ways.clear_all();
        out
    }

    /// Take and clear the list of dirty nodes.
    pub fn take_dirty_nodes(&mut self) -> Vec<Handle<NodeSlot>> {
        let out: Vec<Handle<NodeSlot>> = self
            .dirty_nodes
            .iter_set()
            .filter_map(|i| {
                let generation = self.nodes.generation(i as u32)?;
                let handle = Handle::new(i as u32, generation);
                self.nodes.contains(handle).then_some(handle)
            })
            .collect();
        self.dirty_nodes.clear_all();
        out
    }

    /// Whether any geometry needs rebuilding.
    pub fn has_dirty(&self) -> bool {
        self.dirty_nodes.count_ones() > 0 || self.dirty_ways.count_ones() > 0
    }

    /// Build the reverse-reference index.
    pub fn build_referrers(&self) -> Referrers {
        Referrers::build(self)
    }

    /// Statistics.
    pub fn stats(&self) -> StoreStats {
        StoreStats {
            nodes: self.nodes.len(),
            ways: self.ways.len(),
            relations: self.relations.len(),
            node_refs: self.ways.ref_count(),
            relation_members: self.relations.member_count(),
            tag_sets: self.tags.len(),
            tags: self.tags.tag_count(),
            strings: self.strings.len(),
            bytes: self.mem_bytes(),
        }
    }

    /// Estimate heap memory.
    pub fn mem_bytes(&self) -> ByteSize {
        self.nodes.mem_bytes()
            + self.ways.mem_bytes()
            + self.relations.mem_bytes()
            + self.tags.mem_bytes()
            + ByteSize(self.strings.mem_bytes())
            + self.ids.mem_bytes()
            + self.changes.mem_bytes()
            + self.dirty_nodes.mem_bytes()
            + self.dirty_ways.mem_bytes()
            + self.dirty_relations.mem_bytes()
    }

    /// Iterate snapshots of all nodes.
    pub fn iter_nodes(&self) -> impl Iterator<Item = NodeRef> + '_ {
        self.nodes.iter()
    }

    /// Iterate snapshots of all ways.
    pub fn iter_ways(&self) -> impl Iterator<Item = WayRef> + '_ {
        self.ways.iter()
    }

    /// Iterate snapshots of all relations.
    pub fn iter_relations(&self) -> impl Iterator<Item = RelationRef> + '_ {
        self.relations.iter()
    }

    /// Resolve a TagSet into a DTO.
    pub fn tags_of(&self, id: TagSetId) -> Tags {
        Tags::from_pairs(self.tags.iter(id))
    }
}

/// Data set statistics (for benchmarks and the UI status bar).
#[derive(Clone, Debug, Default)]
pub struct StoreStats {
    /// Number of nodes.
    pub nodes: usize,
    /// Number of Ways.
    pub ways: usize,
    /// Number of Relations.
    pub relations: usize,
    /// Total number of node references.
    pub node_refs: usize,
    /// Total number of relation members.
    pub relation_members: usize,
    /// Number of tag sets.
    pub tag_sets: usize,
    /// Total number of tag pairs.
    pub tags: usize,
    /// Number of interned strings.
    pub strings: usize,
    /// Estimate memory.
    pub bytes: ByteSize,
}

impl core::fmt::Display for StoreStats {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "nodes={} ways={} relations={} node_refs={} members={} tags={} ({} tag sets) strings={} mem={}",
            self.nodes,
            self.ways,
            self.relations,
            self.node_refs,
            self.relation_members,
            self.tags,
            self.tag_sets,
            self.strings,
            self.bytes
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use neo_core::Meta;

    fn node(id: i64, lat: f64, lon: f64) -> Node {
        Node {
            id: ObjectId(id),
            pos: LatLon::from_degrees(lat, lon),
            tags: Tags::new(),
            meta: Meta::new_local(),
        }
    }

    #[test]
    fn duplicate_node_id_is_rejected_instead_of_overwriting() {
        let mut data = DataSet::new();
        data.push_node(node(1, 47.0, 8.0)).unwrap();
        let err = data.push_node(node(1, 48.0, 9.0)).unwrap_err();
        assert!(matches!(err, NeoError::Consistency(_)));
        assert!(data.node_handle(ObjectId(1)).is_some());
        assert_eq!(data.node_count(), 1);
    }

    #[test]
    fn next_local_id_skips_existing_negative_ids() {
        let mut data = DataSet::new();
        data.push_node(node(-1, 47.0, 8.0)).unwrap();
        data.push_node(node(-2, 47.1, 8.1)).unwrap();
        let next = data.next_local_id();
        assert_eq!(next.0, -3);
        // Even if the counter is reset, existing IDs must never be reused.
        data.push_node(node(-10, 47.2, 8.2)).unwrap();
        assert_ne!(data.next_local_id().0, -10);
    }

    #[test]
    fn set_way_nodes_rejects_stale_node_handles() {
        let mut data = DataSet::new();
        let node_handle = data.push_node(node(1, 47.0, 8.0)).unwrap();
        let mut way = Way {
            id: ObjectId(10),
            nodes: vec![ObjectId(1), ObjectId(1)].into(),
            tags: Tags::new(),
            meta: Meta::new_local(),
        };
        way.nodes = vec![ObjectId(1), ObjectId(1)].into();
        let way_handle = data.push_way(way, MissingPolicy::Error).unwrap().unwrap();
        // After a node is physically deleted, its old handle must no longer be writable into a way.
        data.nodes.remove(node_handle);
        let err = data.set_way_nodes(way_handle, &[node_handle]).unwrap_err();
        assert!(matches!(err, NeoError::StaleHandle { .. }));
    }

    #[test]
    fn mutations_bump_version_even_without_editor() {
        let mut data = DataSet::new();
        let before = data.version();
        data.push_node(node(1, 47.0, 8.0)).unwrap();
        assert!(data.version() > before);
        let handle = data.node_handle(ObjectId(1)).unwrap();
        let before = data.version();
        data.set_node_position(handle, LatLon::from_degrees(47.1, 8.1))
            .unwrap();
        assert!(data.version() > before);
    }

    #[test]
    fn deleted_ways_do_not_block_node_deletion() {
        let mut data = DataSet::new();
        data.push_node(node(1, 47.0, 8.0)).unwrap();
        data.push_node(node(2, 47.1, 8.1)).unwrap();
        let way = Way {
            id: ObjectId(10),
            nodes: vec![ObjectId(1), ObjectId(2)].into(),
            tags: Tags::new(),
            meta: Meta::new_local(),
        };
        data.push_way(way, MissingPolicy::Error).unwrap().unwrap();
        data.set_change_state_visible(ObjectKind::Way, ObjectId(10), ChangeState::Deleted, false);
        let referrers = data.build_referrers();
        let node = data.node_handle(ObjectId(1)).unwrap();
        assert!(
            referrers.ways_of_node(node).is_empty(),
            "logically deleted way must not count as a referrer"
        );
    }
}
