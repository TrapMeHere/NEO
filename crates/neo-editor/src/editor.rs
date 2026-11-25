//! Editor: single writer, transactions, history, journal.

use std::collections::HashMap;

use neo_core::{Handle, LatLon, ObjectId, ObjectKind, Sym};
use neo_geo::{SpatialIndex, WorldCache, WorldPoint};
use neo_osm::{ChangeState, Tags};
use neo_store::{
    AnyHandle, DataSet, MissingPolicy, NodeSlot, Referrers, RelationSlot, StoredMember, TagSetId, WaySlot,
};

use crate::command::{Command, CommandError};
use crate::commands::{self, RelationMemberSpec};
use crate::history::History;
use crate::journal::{Journal, JournalCommand, JournalFrame, JournalSink};
use crate::selection::Selection;

/// Command execution context: data + spatial index + referrer table.
pub struct EditContext<'a> {
    /// Dataset (the only mutable entry point).
    pub data: &'a mut DataSet,
    /// Spatial index.
    pub index: &'a mut SpatialIndex,
    /// Node world-coordinate cache (render hot path; updated incrementally with edits).
    pub world_cache: &'a mut WorldCache,
    /// Reverse references (read-only).
    pub referrers: &'a Referrers,
    /// Whether the structure changed (the referrer table needs rebuilding).
    pub referrers_dirty: &'a mut bool,
}

impl EditContext<'_> {
    /// Node handle.
    pub fn node_handle(&self, id: ObjectId) -> Result<Handle<NodeSlot>, CommandError> {
        self.data.node_handle(id).ok_or(CommandError::NotFound {
            kind: ObjectKind::Node,
            id,
        })
    }
    /// Way handle.
    pub fn way_handle(&self, id: ObjectId) -> Result<Handle<WaySlot>, CommandError> {
        self.data.way_handle(id).ok_or(CommandError::NotFound {
            kind: ObjectKind::Way,
            id,
        })
    }
    /// Relation handle.
    pub fn relation_handle(&self, id: ObjectId) -> Result<Handle<RelationSlot>, CommandError> {
        self.data.relation_handle(id).ok_or(CommandError::NotFound {
            kind: ObjectKind::Relation,
            id,
        })
    }

    /// Intern string tag pairs into a TagSet.
    pub fn intern_tags(&mut self, pairs: &[(String, String)]) -> TagSetId {
        let converted: Vec<(Sym, Sym)> = pairs
            .iter()
            .map(|(k, v)| (self.data.intern(k), self.data.intern(v)))
            .collect();
        self.data.add_tags(&converted)
    }

    /// Read an object's tags (string pairs).
    pub fn tags_of(&self, kind: ObjectKind, id: ObjectId) -> Result<Vec<(String, String)>, CommandError> {
        let pairs: &[(Sym, Sym)] = match kind {
            ObjectKind::Node => {
                let h = self.node_handle(id)?;
                self.data
                    .nodes
                    .get(h)
                    .map(|v| self.data.tag_pairs(v.tags))
                    .ok_or(CommandError::NotFound { kind, id })?
            }
            ObjectKind::Way => {
                let h = self.way_handle(id)?;
                self.data
                    .ways
                    .get(h)
                    .map(|v| self.data.tag_pairs(v.tags))
                    .ok_or(CommandError::NotFound { kind, id })?
            }
            ObjectKind::Relation => {
                let h = self.relation_handle(id)?;
                self.data
                    .relations
                    .get(h)
                    .map(|v| self.data.tag_pairs(v.tags))
                    .ok_or(CommandError::NotFound { kind, id })?
            }
            ObjectKind::Changeset => return Ok(Vec::new()),
        };
        Ok(pairs
            .iter()
            .map(|(k, v)| (self.data.resolve(*k).to_owned(), self.data.resolve(*v).to_owned()))
            .collect())
    }

    /// Set tags.
    pub fn set_tags(
        &mut self,
        kind: ObjectKind,
        id: ObjectId,
        tags: &[(String, String)],
    ) -> Result<(), CommandError> {
        let tag_id = self.intern_tags(tags);
        match kind {
            ObjectKind::Node => {
                let h = self.node_handle(id)?;
                self.data.nodes.set_tags(h, tag_id);
            }
            ObjectKind::Way => {
                let h = self.way_handle(id)?;
                self.data.ways.set_tags(h, tag_id);
            }
            ObjectKind::Relation => {
                let h = self.relation_handle(id)?;
                self.data.relations.set_tags(h, tag_id);
            }
            ObjectKind::Changeset => {}
        }
        Ok(())
    }

    /// Move a node, maintaining the spatial index + parent way bbox/dirty flag.
    pub fn move_node(&mut self, id: ObjectId, pos: LatLon) -> Result<LatLon, CommandError> {
        let h = self.node_handle(id)?;
        let old = self.data.set_node_position(h, pos)?;
        self.index.update_node(h, pos);
        self.world_cache.update(h, pos);
        let parents: Vec<Handle<WaySlot>> = self.referrers.ways_of_node(h).to_vec();
        for way in parents {
            self.index.update_way(self.data, way);
            self.data.mark_way_dirty(way);
        }
        Ok(old)
    }

    /// Way's node ID sequence.
    pub fn way_node_ids(&self, id: ObjectId) -> Result<Vec<ObjectId>, CommandError> {
        let h = self.way_handle(id)?;
        Ok(self.data.try_way_node_ids(h)?)
    }

    /// Set the way's node sequence (ID form).
    pub fn set_way_nodes(&mut self, id: ObjectId, nodes: &[ObjectId]) -> Result<(), CommandError> {
        let way_handle = self.way_handle(id)?;
        let mut node_handles = Vec::with_capacity(nodes.len());
        for node_id in nodes {
            node_handles.push(self.node_handle(*node_id)?);
        }
        self.data.set_way_nodes(way_handle, &node_handles)?;
        self.index.update_way(self.data, way_handle);
        self.data.mark_way_dirty(way_handle);
        // A node-sequence change alters node->way reverse references (shared-node checks, parent ways when moving nodes).
        *self.referrers_dirty = true;
        Ok(())
    }

    /// Add a relation.
    pub fn add_relation(
        &mut self,
        id: ObjectId,
        members: &[RelationMemberSpec],
        tags: &[(String, String)],
    ) -> Result<Handle<RelationSlot>, CommandError> {
        let tag_id = self.intern_tags(tags);
        let members: Vec<neo_osm::RelationMember> = members
            .iter()
            .map(|member| neo_osm::RelationMember {
                kind: member.kind,
                id: member.id,
                role: self.data.intern(&member.role),
            })
            .collect();
        let handle = self
            .data
            .push_relation(
                neo_osm::Relation {
                    id,
                    members: members.into(),
                    tags: Tags::from_pairs(self.data.tag_pairs(tag_id).iter().copied()),
                    meta: neo_core::Meta::new_local(),
                },
                MissingPolicy::Error,
            )?
            .ok_or_else(|| CommandError::Invalid("relation rejected".into()))?;
        self.data
            .set_change_state(ObjectKind::Relation, id, ChangeState::Added);
        *self.referrers_dirty = true;
        Ok(handle)
    }

    /// Physically remove a relation (used by undo of AddRelation).
    pub fn remove_relation(&mut self, id: ObjectId) -> Result<(), CommandError> {
        let handle = self.data.relation_handle(id).ok_or(CommandError::NotFound {
            kind: ObjectKind::Relation,
            id,
        })?;
        self.data.relations.remove(handle);
        self.data.ids.remove_relation(id);
        self.data
            .changes
            .mark(ObjectKind::Relation, id, ChangeState::Unchanged);
        *self.referrers_dirty = true;
        Ok(())
    }

    /// Read relation members (serializable form, for command undo/UI).
    pub fn relation_members(&self, id: ObjectId) -> Result<Vec<RelationMemberSpec>, CommandError> {
        relation_members_of(self.data, id)
    }

    /// Replace relation members (keep order; roles are interned before writing).
    pub fn set_relation_members(
        &mut self,
        id: ObjectId,
        members: &[RelationMemberSpec],
    ) -> Result<(), CommandError> {
        let handle = self.data.relation_handle(id).ok_or(CommandError::NotFound {
            kind: ObjectKind::Relation,
            id,
        })?;
        let mut stored = Vec::with_capacity(members.len());
        for member in members {
            let target = match member.kind {
                ObjectKind::Node => self
                    .node_handle(member.id)
                    .map(AnyHandle::from_node)
                    .map_err(|_| CommandError::NotFound {
                        kind: ObjectKind::Node,
                        id: member.id,
                    })?,
                ObjectKind::Way => self.way_handle(member.id).map(AnyHandle::from_way).map_err(|_| {
                    CommandError::NotFound {
                        kind: ObjectKind::Way,
                        id: member.id,
                    }
                })?,
                ObjectKind::Relation => self
                    .relation_handle(member.id)
                    .map(AnyHandle::from_relation)
                    .map_err(|_| CommandError::NotFound {
                        kind: ObjectKind::Relation,
                        id: member.id,
                    })?,
                ObjectKind::Changeset => {
                    return Err(CommandError::Invalid(
                        "changeset cannot be a relation member".into(),
                    ));
                }
            };
            stored.push(StoredMember {
                role: self.data.intern(&member.role),
                target,
            });
        }
        self.data.relations.set_members(handle, &stored);
        self.data.mark_relation_dirty(handle);
        self.data.bump_version();
        // Member changes alter relation->relation / way->relation reverse references.
        *self.referrers_dirty = true;
        Ok(())
    }

    /// Add a node.
    pub fn add_node(
        &mut self,
        id: ObjectId,
        pos: LatLon,
        tags: &[(String, String)],
    ) -> Result<Handle<NodeSlot>, CommandError> {
        let tag_id = self.intern_tags(tags);
        let handle = self.data.push_node(neo_osm::Node {
            id,
            pos,
            tags: Tags::from_pairs(self.data.tag_pairs(tag_id).iter().copied()),
            meta: neo_core::Meta::new_local(),
        })?;
        self.data
            .set_change_state(ObjectKind::Node, id, ChangeState::Added);
        self.index.update_node(handle, pos);
        self.world_cache.update(handle, pos);
        // A new node is not yet referenced by any way, so the referrer table need not be rebuilt (rebuilding
        // referrers on every click while drawing a way is O(N) and the main cause of edit lag).
        Ok(handle)
    }

    /// Add a way.
    pub fn add_way(
        &mut self,
        id: ObjectId,
        nodes: &[ObjectId],
        tags: &[(String, String)],
    ) -> Result<Handle<WaySlot>, CommandError> {
        let nodes_small: smallvec::SmallVec<[ObjectId; 8]> = nodes.iter().copied().collect();
        let tag_id = self.intern_tags(tags);
        let handle = self
            .data
            .push_way(
                neo_osm::Way {
                    id,
                    nodes: nodes_small,
                    tags: Tags::from_pairs(self.data.tag_pairs(tag_id).iter().copied()),
                    meta: neo_core::Meta::new_local(),
                },
                MissingPolicy::Error,
            )?
            .ok_or_else(|| CommandError::Invalid("way rejected".into()))?;
        self.data
            .set_change_state(ObjectKind::Way, id, ChangeState::Added);
        self.index.update_way(self.data, handle);
        self.data.mark_way_dirty(handle);
        *self.referrers_dirty = true;
        Ok(handle)
    }

    /// Physically remove a node (used by undo of AddNode).
    pub fn remove_node(&mut self, id: ObjectId) -> Result<(), CommandError> {
        let h = self.node_handle(id)?;
        self.index.remove(ObjectKind::Node, h.index());
        self.world_cache.clear(h.index());
        self.data.nodes.remove(h);
        self.data.ids.remove_node(id);
        self.data
            .changes
            .mark(ObjectKind::Node, id, ChangeState::Unchanged);
        *self.referrers_dirty = true;
        Ok(())
    }

    /// Physically remove a way.
    pub fn remove_way(&mut self, id: ObjectId) -> Result<(), CommandError> {
        let h = self.way_handle(id)?;
        self.index.remove(ObjectKind::Way, h.index());
        self.data.ways.remove(h);
        self.data.ids.remove_way(id);
        self.data
            .changes
            .mark(ObjectKind::Way, id, ChangeState::Unchanged);
        *self.referrers_dirty = true;
        Ok(())
    }

    /// Mark deleted (keep the object for upload diff / undo).
    pub fn mark_deleted(&mut self, kind: ObjectKind, id: ObjectId) -> Result<(), CommandError> {
        match kind {
            ObjectKind::Node => {
                let h = self.node_handle(id)?;
                self.index.remove(ObjectKind::Node, h.index());
            }
            ObjectKind::Way => {
                let h = self.way_handle(id)?;
                self.index.remove(ObjectKind::Way, h.index());
            }
            ObjectKind::Relation => {
                let _ = self.relation_handle(id)?;
            }
            ObjectKind::Changeset => {}
        }
        self.data
            .set_change_state_visible(kind, id, ChangeState::Deleted, false);
        // Logically deleted ways/relations must no longer block deletion of their nodes; rebuild the referrer table to drop them.
        *self.referrers_dirty = true;
        Ok(())
    }

    /// The object's current visible flag.
    pub fn visible_of(&self, kind: ObjectKind, id: ObjectId) -> bool {
        match kind {
            ObjectKind::Node => self
                .data
                .node_handle(id)
                .and_then(|h| self.data.nodes.get(h))
                .map(|v| v.meta.visible)
                .unwrap_or(true),
            ObjectKind::Way => self
                .data
                .way_handle(id)
                .and_then(|h| self.data.ways.get(h))
                .map(|v| v.meta.visible)
                .unwrap_or(true),
            ObjectKind::Relation => self
                .data
                .relation_handle(id)
                .and_then(|h| self.data.relations.get(h))
                .map(|v| v.meta.visible)
                .unwrap_or(true),
            ObjectKind::Changeset => true,
        }
    }

    /// Restore a deletion.
    pub fn restore_deleted(&mut self, kind: ObjectKind, id: ObjectId, state: ChangeState, visible: bool) {
        self.data.set_change_state_visible(kind, id, state, visible);
        // Restoring visibility re-establishes way->node / relation->member references.
        *self.referrers_dirty = true;
        if visible {
            match kind {
                ObjectKind::Node => {
                    if let Some(h) = self.data.node_handle(id)
                        && let Some(pos) = self.data.nodes.position(h)
                    {
                        self.index.update_node(h, pos);
                        self.world_cache.update(h, pos);
                    }
                }
                ObjectKind::Way => {
                    if let Some(h) = self.data.way_handle(id) {
                        self.index.update_way(self.data, h);
                    }
                }
                _ => {}
            }
        }
    }
}

/// Editor.
pub struct Editor {
    data: DataSet,
    index: SpatialIndex,
    world_cache: WorldCache,
    referrers: Referrers,
    referrers_dirty: bool,
    selection: Selection,
    history: History,
    journal: Journal,
}

impl Editor {
    /// Build from a dataset (index, world-coordinate cache and referrer table are built immediately).
    pub fn new(data: DataSet) -> Self {
        let index = SpatialIndex::build(&data);
        let world_cache = WorldCache::build(&data);
        let referrers = data.build_referrers();
        Self {
            data,
            index,
            world_cache,
            referrers,
            referrers_dirty: false,
            selection: Selection::new(),
            history: History::new(),
            journal: Journal::new(),
        }
    }

    /// Read-only data.
    pub fn data(&self) -> &DataSet {
        &self.data
    }

    /// Node world-coordinate cache (read-only for the renderer).
    pub fn world_cache(&self) -> &WorldCache {
        &self.world_cache
    }

    /// Read relation members (for the UI).
    pub fn relation_members(&self, id: ObjectId) -> Result<Vec<RelationMemberSpec>, CommandError> {
        relation_members_of(&self.data, id)
    }

    /// Intern a name into the dataset symbol table. Used by style building so that `&mut DataSet` is not exposed.
    pub fn intern_sym(&mut self, name: &str) -> Sym {
        self.data.intern(name)
    }

    /// Load an existing node without recording undo. Used by tests and import baselines; normal edits go through commands.
    pub fn import_node(&mut self, node: neo_osm::Node) -> Result<(), neo_core::NeoError> {
        self.data.push_node(node).map(|_| ())
    }

    /// Spatial index.
    pub fn index(&self) -> &SpatialIndex {
        &self.index
    }

    /// Selection set.
    pub fn selection(&self) -> &Selection {
        &self.selection
    }

    /// Selection set (mutable).
    pub fn selection_mut(&mut self) -> &mut Selection {
        &mut self.selection
    }

    /// History.
    pub fn history(&self) -> &History {
        &self.history
    }

    /// History (mutable: clear_dirty after saving, etc.).
    pub fn history_mut(&mut self) -> &mut History {
        &mut self.history
    }

    /// Bind the journal output.
    pub fn set_journal_sink(&mut self, sink: Box<dyn JournalSink>) {
        self.journal.set_sink(sink);
    }

    /// Allocate a local ID.
    pub fn next_local_id(&mut self) -> ObjectId {
        self.data.next_local_id()
    }

    /// Drag preview: only changes data and index, no history/journal (commit with MoveNode on release).
    pub fn preview_move_node(&mut self, id: ObjectId, pos: LatLon) -> Result<(), CommandError> {
        let handle = self.data.node_handle(id).ok_or(CommandError::NotFound {
            kind: ObjectKind::Node,
            id,
        })?;
        self.data.set_node_position(handle, pos)?;
        self.data.mark_node_dirty(handle);
        self.index.update_node(handle, pos);
        self.world_cache.update(handle, pos);
        // The parent way bbox follows the node so picking/scene and geometry stay consistent while dragging.
        self.ensure_referrers();
        let parents: Vec<_> = self.referrers.ways_of_node(handle).to_vec();
        for way in parents {
            self.index.update_way(&self.data, way);
        }
        Ok(())
    }

    /// Return the IDs of nodes directly connected to this node (for drawing drag-preview lines).
    pub fn node_neighbors(&self, node: ObjectId) -> Vec<ObjectId> {
        let Some(handle) = self.data.node_handle(node) else {
            return Vec::new();
        };
        let mut out: Vec<ObjectId> = Vec::new();
        for way in self.referrers.ways_of_node(handle) {
            let Some(nodes) = self.data.ways.nodes(*way) else {
                continue;
            };
            for (index, candidate) in nodes.iter().enumerate() {
                if *candidate != handle {
                    continue;
                }
                if index > 0
                    && let Some(id) = self.data.nodes.id(nodes[index - 1])
                {
                    out.push(id);
                }
                if index + 1 < nodes.len()
                    && let Some(id) = self.data.nodes.id(nodes[index + 1])
                {
                    out.push(id);
                }
            }
        }
        out.sort_unstable_by_key(|id| id.0);
        out.dedup();
        out
    }

    /// Rebuild the referrer table from the current data (happens automatically after structural changes).
    /// Reverse references (read-only; e.g. scene building needs shared-node detection).
    pub fn referrers(&self) -> &Referrers {
        &self.referrers
    }

    /// Refresh the referrer table before rendering (`referrers()` is read-only; the dirty flag is set by write commands).
    pub fn refresh_referrers(&mut self) {
        self.ensure_referrers();
    }

    fn ensure_referrers(&mut self) {
        if self.referrers_dirty {
            self.referrers = self.data.build_referrers();
            self.referrers_dirty = false;
        }
    }

    /// Execute a single command.
    pub fn execute(&mut self, mut cmd: Box<dyn Command>) -> Result<(), CommandError> {
        self.ensure_referrers();
        cmd.precheck(&self.data, &self.referrers)?;
        let changes_before = self.data.changes.clone();
        let tx = self.journal.begin();
        let applied = {
            let mut ctx = EditContext {
                data: &mut self.data,
                index: &mut self.index,
                world_cache: &mut self.world_cache,
                referrers: &self.referrers,
                referrers_dirty: &mut self.referrers_dirty,
            };
            cmd.apply(&mut ctx)
        };
        if let Err(err) = applied {
            // The command itself may have written half its changes; best-effort undo + restore the change set, then return the error.
            let mut ctx = EditContext {
                data: &mut self.data,
                index: &mut self.index,
                world_cache: &mut self.world_cache,
                referrers: &self.referrers,
                referrers_dirty: &mut self.referrers_dirty,
            };
            let _ = cmd.undo(&mut ctx);
            self.data.changes = changes_before;
            return Err(err);
        }
        self.journal.command(cmd.record());
        self.journal.commit(tx);
        self.history.push(cmd);
        self.data.bump_version();
        Ok(())
    }

    /// Execute a command batch (atomic).
    pub fn execute_batch(&mut self, mut cmds: Vec<Box<dyn Command>>) -> Result<(), CommandError> {
        if cmds.is_empty() {
            return Ok(());
        }
        let changes_before = self.data.changes.clone();
        let tx = self.journal.begin();
        for idx in 0..cmds.len() {
            // Refresh the referrer table before each command: earlier commands in the batch may have changed references.
            self.ensure_referrers();
            if let Err(err) = cmds[idx].precheck(&self.data, &self.referrers) {
                self.rollback_batch(&mut cmds, idx, changes_before);
                return Err(err);
            }
            let result = {
                let mut ctx = EditContext {
                    data: &mut self.data,
                    index: &mut self.index,
                    world_cache: &mut self.world_cache,
                    referrers: &self.referrers,
                    referrers_dirty: &mut self.referrers_dirty,
                };
                cmds[idx].apply(&mut ctx)
            };
            if let Err(err) = result {
                // Roll back the failed command itself + the already-applied part (undo in reverse order), guaranteeing zero side effects on failure.
                let mut ctx = EditContext {
                    data: &mut self.data,
                    index: &mut self.index,
                    world_cache: &mut self.world_cache,
                    referrers: &self.referrers,
                    referrers_dirty: &mut self.referrers_dirty,
                };
                let _ = cmds[idx].undo(&mut ctx);
                self.rollback_batch(&mut cmds, idx, changes_before);
                return Err(err);
            }
        }
        for cmd in &cmds {
            self.journal.command(cmd.record());
        }
        self.journal.commit(tx);
        self.history.push_batch(cmds);
        self.data.bump_version();
        Ok(())
    }

    fn rollback_batch(
        &mut self,
        cmds: &mut [Box<dyn Command>],
        applied: usize,
        changes_before: neo_store::ChangeTrack,
    ) {
        for back in (0..applied).rev() {
            let mut ctx = EditContext {
                data: &mut self.data,
                index: &mut self.index,
                world_cache: &mut self.world_cache,
                referrers: &self.referrers,
                referrers_dirty: &mut self.referrers_dirty,
            };
            let _ = cmds[back].undo(&mut ctx);
        }
        self.data.changes = changes_before;
    }

    /// Undo.
    pub fn undo(&mut self) -> Result<bool, CommandError> {
        self.ensure_referrers();
        let done = {
            let mut ctx = EditContext {
                data: &mut self.data,
                index: &mut self.index,
                world_cache: &mut self.world_cache,
                referrers: &self.referrers,
                referrers_dirty: &mut self.referrers_dirty,
            };
            self.history.undo(&mut ctx)?.is_some()
        };
        if done {
            self.data.bump_version();
            self.ensure_referrers();
            // Rebuild the journal from the current undo stack: state after crash recovery matches the screen,
            // and undone edits do not come back after a restart.
            let records = self.history.records();
            if let Err(err) = self.journal.rebase(&records) {
                tracing::warn!("journal rebase failed: {err}");
            }
        }
        Ok(done)
    }

    /// Redo.
    pub fn redo(&mut self) -> Result<bool, CommandError> {
        self.ensure_referrers();
        let done = {
            let mut ctx = EditContext {
                data: &mut self.data,
                index: &mut self.index,
                world_cache: &mut self.world_cache,
                referrers: &self.referrers,
                referrers_dirty: &mut self.referrers_dirty,
            };
            self.history.redo(&mut ctx)?.is_some()
        };
        if done {
            self.data.bump_version();
            self.ensure_referrers();
            let records = self.history.records();
            if let Err(err) = self.journal.rebase(&records) {
                tracing::warn!("journal rebase failed: {err}");
            }
        }
        Ok(done)
    }

    /// Clear the journal baseline (called when opening/downloading a new dataset or after saving).
    pub fn reset_journal(&mut self) {
        let _ = self.journal.reset();
    }

    /// Most recent journal write failure (for UI alerts).
    pub fn take_journal_failure(&mut self) -> Option<String> {
        self.journal.take_failure()
    }

    /// Recover from journal frames (apply only up to the last TxCommit).
    pub fn recover(data: DataSet, frames: &[JournalFrame]) -> Result<(Self, usize), CommandError> {
        Self::recover_with_report(data, frames).map(|(editor, report)| (editor, report.applied))
    }

    /// Recover from the journal and return a displayable recovery report (stage 4: observable crash recovery).
    ///
    /// Recovery semantics:
    /// * start from the commands of the last `Rebase` frame as baseline (or from the `data` baseline if none);
    /// * replay only Cmd frames before the last complete `TxBegin/TxCommit`;
    /// * a single command failure only counts toward `skipped` and continues, so one bad frame no longer ruins the whole log.
    pub fn recover_with_report(
        data: DataSet,
        frames: &[JournalFrame],
    ) -> Result<(Self, RecoveryReport), CommandError> {
        let plan = crate::journal::replay_plan(frames);
        let mut editor = Editor::new(data);
        let mut report = RecoveryReport {
            frames: frames.len(),
            committed_transactions: plan
                .frames
                .iter()
                .filter(|frame| matches!(frame, JournalFrame::TxCommit { .. }))
                .count(),
            dropped: plan.dropped,
            ..RecoveryReport::default()
        };

        let tail_commands: Vec<JournalCommand> = plan
            .frames
            .iter()
            .filter_map(|frame| match frame {
                JournalFrame::Cmd { command } => Some(command.clone()),
                _ => None,
            })
            .collect();
        let all: Vec<&JournalCommand> = plan.base.iter().chain(tail_commands.iter()).collect();

        for record in all {
            let Some(cmd) = commands::from_journal(record) else {
                report.skipped += 1;
                continue;
            };
            match editor.execute(cmd) {
                Ok(()) => report.applied += 1,
                Err(_) => report.skipped += 1,
            }
        }
        report.skipped += plan.dropped;
        editor.data.bump_version();
        Ok((editor, report))
    }
}

/// One ID/version mapping in an upload result (decoupled from `neo-api`; converted by the app layer).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DiffMapping {
    /// Object kind.
    pub kind: ObjectKind,
    /// Local ID before upload (negative for newly created objects).
    pub old_id: ObjectId,
    /// Server ID.
    pub new_id: ObjectId,
    /// Server version (`None` when the server did not return one; the local version is kept).
    pub new_version: Option<u32>,
}

/// Journal recovery report.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RecoveryReport {
    /// Total journal frame count.
    pub frames: usize,
    /// Number of command frames in committed transactions.
    pub committed_transactions: usize,
    /// Number of commands successfully replayed and applied.
    pub applied: usize,
    /// Number of discarded frames (unclosed transactions, unparsable, etc.).
    pub dropped: usize,
    /// Number of truncated/skipped frames (half transactions, unparsable, etc.).
    pub skipped: usize,
}

impl Editor {
    /// Conflict-resolution only: align the local object's version to the server version (KeepLocal strategy).    ///
    /// This is not a normal edit (it writes no history/journal); it only makes the "keep local" re-upload pass
    /// OSM's version check after a 409. Callers should record it as a conflict-resolution action in the UI log.
    pub fn set_object_version(&mut self, kind: ObjectKind, id: ObjectId, version: u32) -> bool {
        let updated = match kind {
            ObjectKind::Node => {
                if let Some(handle) = self.data.node_handle(id) {
                    if let Some(view) = self.data.node_view(handle) {
                        let mut meta = view.info.meta;
                        meta.version = version;
                        self.data.nodes.set_meta(handle, meta)
                    } else {
                        false
                    }
                } else {
                    false
                }
            }
            ObjectKind::Way => {
                if let Some(handle) = self.data.way_handle(id) {
                    if let Some(view) = self.data.way_view(handle) {
                        let mut meta = view.info.meta;
                        meta.version = version;
                        self.data.ways.set_meta(handle, meta)
                    } else {
                        false
                    }
                } else {
                    false
                }
            }
            ObjectKind::Relation => {
                if let Some(handle) = self.data.relation_handle(id) {
                    if let Some(view) = self.data.relation_view(handle) {
                        let mut meta = view.info.meta;
                        meta.version = version;
                        self.data.relations.set_meta(handle, meta)
                    } else {
                        false
                    }
                } else {
                    false
                }
            }
            ObjectKind::Changeset => false,
        };
        if updated {
            self.data.bump_version();
        }
        updated
    }

    /// Apply an upload result: negative ID -> server ID, write back version, clear the change set.
    ///
    /// **Must** be called after a successful upload: otherwise new objects keep a negative ID/version=0, and the next upload
    /// creates the same objects again (duplicate objects), while modifications keep returning 409 due to a stale version.
    ///
    /// Because ID changes invalidate history commands and journal records, this also clears history and resets the
    /// journal baseline (the current in-memory data becomes the new baseline).
    pub fn apply_diff_result(&mut self, entries: &[DiffMapping]) -> usize {
        self.apply_uploaded(entries, true)
    }

    /// Apply the uploaded mapping; with `complete=false`, keep changes for objects not yet uploaded, so a chunked upload
    /// can continue after a mid-way failure.
    pub fn apply_uploaded(&mut self, entries: &[DiffMapping], complete: bool) -> usize {
        let mut applied = 0usize;
        for entry in entries {
            if entry.old_id != entry.new_id && !self.data.reassign_id(entry.kind, entry.old_id, entry.new_id)
            {
                continue;
            }
            if let Some(version) = entry.new_version {
                self.data.set_version(entry.kind, entry.new_id, version);
            }
            self.data.mark_synced(entry.kind, entry.new_id);
            applied += 1;
        }
        if complete {
            // The whole package uploaded successfully: the remaining dirty markers (including local deletions) also return to baseline.
            self.data.changes.clear();
        }
        // If a chunked upload fails midway, the history of the uploaded part can no longer be used for undo (otherwise it
        // would modify objects already accepted by the server); keep the remaining changes for the next submit.
        self.history.clear();
        self.selection.clear();
        self.data.bump_version();
        self.ensure_referrers();
        self.index = SpatialIndex::build(&self.data);
        self.world_cache = WorldCache::build(&self.data);
        self.reset_journal();
        applied
    }

    /// Overwrite a local conflicting object with the server object ("take remote"). Remote data comes from a separate DataSet,
    /// and `Sym` values cannot cross pools, so tags/roles are re-interned from strings.
    pub fn replace_from_remote(
        &mut self,
        kind: ObjectKind,
        id: ObjectId,
        remote: &DataSet,
    ) -> Result<(), CommandError> {
        match kind {
            ObjectKind::Node => {
                let remote_handle = remote
                    .node_handle(id)
                    .ok_or(CommandError::NotFound { kind, id })?;
                let remote_view = remote
                    .node_view(remote_handle)
                    .ok_or(CommandError::NotFound { kind, id })?;
                let handle = self
                    .data
                    .node_handle(id)
                    .ok_or(CommandError::NotFound { kind, id })?;
                self.data
                    .set_node_position(handle, remote_view.pos())
                    .map_err(|err| CommandError::Invalid(err.to_string()))?;
                let tags = intern_remote_tags(&mut self.data, remote, remote_view.info.tags);
                self.data.nodes.set_tags(handle, tags);
                if remote_view.info.meta.version > 0 {
                    self.data.set_version(kind, id, remote_view.info.meta.version);
                }
            }
            ObjectKind::Way => {
                let remote_handle = remote.way_handle(id).ok_or(CommandError::NotFound { kind, id })?;
                let remote_view = remote
                    .way_view(remote_handle)
                    .ok_or(CommandError::NotFound { kind, id })?;
                let handle = self
                    .data
                    .way_handle(id)
                    .ok_or(CommandError::NotFound { kind, id })?;
                let mut nodes = Vec::new();
                for node in remote.way_node_ids(remote_handle) {
                    nodes.push(self.data.node_handle(node).ok_or(CommandError::NotFound {
                        kind: ObjectKind::Node,
                        id: node,
                    })?);
                }
                self.data
                    .set_way_nodes(handle, &nodes)
                    .map_err(|err| CommandError::Invalid(err.to_string()))?;
                let tags = intern_remote_tags(&mut self.data, remote, remote_view.info.tags);
                self.data.ways.set_tags(handle, tags);
                if remote_view.info.meta.version > 0 {
                    self.data.set_version(kind, id, remote_view.info.meta.version);
                }
            }
            ObjectKind::Relation => {
                let remote_handle = remote
                    .relation_handle(id)
                    .ok_or(CommandError::NotFound { kind, id })?;
                let remote_view = remote
                    .relation_view(remote_handle)
                    .ok_or(CommandError::NotFound { kind, id })?;
                let handle = self
                    .data
                    .relation_handle(id)
                    .ok_or(CommandError::NotFound { kind, id })?;
                let mut stored = Vec::with_capacity(remote_view.members.len());
                for member in remote_view.members {
                    let remote_id = match member.target.kind {
                        ObjectKind::Node => member.target.as_node().and_then(|h| remote.nodes.id(h)),
                        ObjectKind::Way => member.target.as_way().and_then(|h| remote.ways.id(h)),
                        ObjectKind::Relation => {
                            member.target.as_relation().and_then(|h| remote.relations.id(h))
                        }
                        ObjectKind::Changeset => None,
                    };
                    let Some(remote_id) = remote_id else {
                        return Err(CommandError::Invalid("bad remote member".into()));
                    };
                    let target = match member.target.kind {
                        ObjectKind::Node => self.data.node_handle(remote_id).map(AnyHandle::from_node),
                        ObjectKind::Way => self.data.way_handle(remote_id).map(AnyHandle::from_way),
                        ObjectKind::Relation => {
                            self.data.relation_handle(remote_id).map(AnyHandle::from_relation)
                        }
                        ObjectKind::Changeset => {
                            return Err(CommandError::Invalid("bad remote member kind".into()));
                        }
                    }
                    .ok_or(CommandError::NotFound {
                        kind: member.target.kind,
                        id: remote_id,
                    })?;
                    stored.push(StoredMember {
                        role: self.data.intern(remote.resolve(member.role)),
                        target,
                    });
                }
                self.data.relations.set_members(handle, &stored);
                let tags = intern_remote_tags(&mut self.data, remote, remote_view.info.tags);
                self.data.relations.set_tags(handle, tags);
                if remote_view.info.meta.version > 0 {
                    self.data.set_version(kind, id, remote_view.info.meta.version);
                }
            }
            ObjectKind::Changeset => {
                return Err(CommandError::Invalid("cannot take a changeset".into()));
            }
        }
        self.data.mark_synced(kind, id);
        self.data.bump_version();
        self.history.clear();
        self.selection.clear();
        self.ensure_referrers();
        self.index = SpatialIndex::build(&self.data);
        self.world_cache = WorldCache::build(&self.data);
        self.reset_journal();
        Ok(())
    }

    /// Take remote geometry/members but keep local tags.
    pub fn merge_geometry_from_remote(
        &mut self,
        kind: ObjectKind,
        id: ObjectId,
        remote: &DataSet,
    ) -> Result<(), CommandError> {
        let local_tags = match kind {
            ObjectKind::Node => self
                .data
                .node_handle(id)
                .and_then(|handle| self.data.node_view(handle))
                .map(|view| view.info.tags),
            ObjectKind::Way => self
                .data
                .way_handle(id)
                .and_then(|handle| self.data.way_view(handle))
                .map(|view| view.info.tags),
            ObjectKind::Relation => self
                .data
                .relation_handle(id)
                .and_then(|handle| self.data.relation_view(handle))
                .map(|view| view.info.tags),
            ObjectKind::Changeset => None,
        }
        .ok_or(CommandError::NotFound { kind, id })?;
        self.replace_from_remote(kind, id, remote)?;
        match kind {
            ObjectKind::Node => {
                let handle = self
                    .data
                    .node_handle(id)
                    .ok_or(CommandError::NotFound { kind, id })?;
                self.data.nodes.set_tags(handle, local_tags);
            }
            ObjectKind::Way => {
                let handle = self
                    .data
                    .way_handle(id)
                    .ok_or(CommandError::NotFound { kind, id })?;
                self.data.ways.set_tags(handle, local_tags);
            }
            ObjectKind::Relation => {
                let handle = self
                    .data
                    .relation_handle(id)
                    .ok_or(CommandError::NotFound { kind, id })?;
                self.data.relations.set_tags(handle, local_tags);
            }
            ObjectKind::Changeset => {}
        }
        self.data.bump_version();
        Ok(())
    }

    /// Merge remote tags: keys already present locally are kept; keys unique to remote are added; geometry/members stay local.
    pub fn merge_from_remote(
        &mut self,
        kind: ObjectKind,
        id: ObjectId,
        remote: &DataSet,
    ) -> Result<(), CommandError> {
        let (remote_tags, remote_version) = match kind {
            ObjectKind::Node => {
                let handle = remote
                    .node_handle(id)
                    .ok_or(CommandError::NotFound { kind, id })?;
                let view = remote
                    .node_view(handle)
                    .ok_or(CommandError::NotFound { kind, id })?;
                (view.info.tags, view.info.meta.version)
            }
            ObjectKind::Way => {
                let handle = remote.way_handle(id).ok_or(CommandError::NotFound { kind, id })?;
                let view = remote
                    .way_view(handle)
                    .ok_or(CommandError::NotFound { kind, id })?;
                (view.info.tags, view.info.meta.version)
            }
            ObjectKind::Relation => {
                let handle = remote
                    .relation_handle(id)
                    .ok_or(CommandError::NotFound { kind, id })?;
                let view = remote
                    .relation_view(handle)
                    .ok_or(CommandError::NotFound { kind, id })?;
                (view.info.tags, view.info.meta.version)
            }
            ObjectKind::Changeset => {
                return Err(CommandError::Invalid("cannot merge a changeset".into()));
            }
        };
        let local_tags = match kind {
            ObjectKind::Node => self
                .data
                .node_handle(id)
                .and_then(|handle| self.data.node_view(handle))
                .map(|view| view.info.tags),
            ObjectKind::Way => self
                .data
                .way_handle(id)
                .and_then(|handle| self.data.way_view(handle))
                .map(|view| view.info.tags),
            ObjectKind::Relation => self
                .data
                .relation_handle(id)
                .and_then(|handle| self.data.relation_view(handle))
                .map(|view| view.info.tags),
            ObjectKind::Changeset => None,
        }
        .ok_or(CommandError::NotFound { kind, id })?;
        let mut pairs: Vec<(String, String)> = self
            .data
            .tag_pairs(local_tags)
            .iter()
            .map(|(key, value)| {
                (
                    self.data.resolve(*key).to_owned(),
                    self.data.resolve(*value).to_owned(),
                )
            })
            .collect();
        for (key, value) in remote.tag_pairs(remote_tags) {
            let key = remote.resolve(*key).to_owned();
            if !pairs.iter().any(|(existing, _)| *existing == key) {
                pairs.push((key, remote.resolve(*value).to_owned()));
            }
        }
        let interned: Vec<(Sym, Sym)> = pairs
            .iter()
            .map(|(key, value)| (self.data.intern(key), self.data.intern(value)))
            .collect();
        let tag_set = self.data.add_tags(&interned);
        match kind {
            ObjectKind::Node => {
                let handle = self
                    .data
                    .node_handle(id)
                    .ok_or(CommandError::NotFound { kind, id })?;
                self.data.nodes.set_tags(handle, tag_set);
            }
            ObjectKind::Way => {
                let handle = self
                    .data
                    .way_handle(id)
                    .ok_or(CommandError::NotFound { kind, id })?;
                self.data.ways.set_tags(handle, tag_set);
            }
            ObjectKind::Relation => {
                let handle = self
                    .data
                    .relation_handle(id)
                    .ok_or(CommandError::NotFound { kind, id })?;
                self.data.relations.set_tags(handle, tag_set);
            }
            ObjectKind::Changeset => {}
        }
        if remote_version > 0 {
            self.data.set_version(kind, id, remote_version);
        }
        self.data.mark_synced(kind, id);
        self.data.bump_version();
        self.history.clear();
        self.selection.clear();
        self.ensure_referrers();
        self.index = SpatialIndex::build(&self.data);
        self.world_cache = WorldCache::build(&self.data);
        self.reset_journal();
        Ok(())
    }

    /// Merge a remote dataset into the current layer (shared by download / layer merge).
    ///
    /// * Positive ID: merge by OSM id -- add if missing locally and keep remote meta; update if unmodified locally; skip if locally dirty
    /// * Negative ID: always allocate a new local ID (local objects are never wrongly merged across layers)
    /// * History is not cleared, so unsaved user edits can still be undone
    pub fn merge_from_dataset(&mut self, remote: &DataSet) -> Result<MergeStats, CommandError> {
        let mut stats = MergeStats::default();
        let mut mapping: HashMap<(ObjectKind, ObjectId), ObjectId> = HashMap::new();

        for node in remote.iter_nodes() {
            let id = node.id;
            let view = remote.node_view(node.handle).ok_or(CommandError::NotFound {
                kind: ObjectKind::Node,
                id,
            })?;
            if id.is_local() {
                let new_id = self.next_local_id();
                mapping.insert((ObjectKind::Node, id), new_id);
                let tags = tags_from_remote(&mut self.data, remote, view.info.tags);
                let handle = self.data.push_node(neo_osm::Node {
                    id: new_id,
                    pos: view.pos(),
                    tags,
                    meta: neo_core::Meta::new_local(),
                })?;
                self.data
                    .set_change_state(ObjectKind::Node, new_id, ChangeState::Added);
                self.index.update_node(handle, view.pos());
                self.world_cache.update(handle, view.pos());
                stats.added += 1;
                continue;
            }
            if self.data.node_handle(id).is_some() {
                if self.data.change_state(ObjectKind::Node, id).is_dirty() {
                    stats.skipped += 1;
                } else {
                    let meta = copy_meta(&mut self.data, remote, view.info.meta);
                    let handle = self.data.node_handle(id).expect("checked");
                    self.data
                        .set_node_position(handle, view.pos())
                        .map_err(|err| CommandError::Invalid(err.to_string()))?;
                    let tags = intern_remote_tags(&mut self.data, remote, view.info.tags);
                    self.data.nodes.set_tags(handle, tags);
                    self.data.nodes.set_meta(handle, meta);
                    self.data.mark_synced(ObjectKind::Node, id);
                    self.index.update_node(handle, view.pos());
                    self.world_cache.update(handle, view.pos());
                    stats.updated += 1;
                }
                continue;
            }
            let tags = tags_from_remote(&mut self.data, remote, view.info.tags);
            let meta = copy_meta(&mut self.data, remote, view.info.meta);
            let handle = self.data.push_node(neo_osm::Node {
                id,
                pos: view.pos(),
                tags,
                meta,
            })?;
            self.index.update_node(handle, view.pos());
            self.world_cache.update(handle, view.pos());
            stats.added += 1;
        }

        for way in remote.iter_ways() {
            let id = way.id;
            let view = remote.way_view(way.handle).ok_or(CommandError::NotFound {
                kind: ObjectKind::Way,
                id,
            })?;
            let node_ids = map_way_node_ids(remote, view.node_refs, &mapping)?;
            if id.is_local() {
                let new_id = self.next_local_id();
                mapping.insert((ObjectKind::Way, id), new_id);
                let tags = tags_from_remote(&mut self.data, remote, view.info.tags);
                let handle = self
                    .data
                    .push_way(
                        neo_osm::Way {
                            id: new_id,
                            nodes: node_ids.iter().copied().collect(),
                            tags,
                            meta: neo_core::Meta::new_local(),
                        },
                        MissingPolicy::Error,
                    )?
                    .ok_or_else(|| CommandError::Invalid("way rejected".into()))?;
                self.data
                    .set_change_state(ObjectKind::Way, new_id, ChangeState::Added);
                self.index.update_way(&self.data, handle);
                self.data.mark_way_dirty(handle);
                stats.added += 1;
                continue;
            }
            if self.data.way_handle(id).is_some() {
                if self.data.change_state(ObjectKind::Way, id).is_dirty() {
                    stats.skipped += 1;
                } else {
                    let handle = self.data.way_handle(id).expect("checked");
                    let mut node_handles = Vec::with_capacity(node_ids.len());
                    for node_id in &node_ids {
                        node_handles.push(self.data.node_handle(*node_id).ok_or(CommandError::NotFound {
                            kind: ObjectKind::Node,
                            id: *node_id,
                        })?);
                    }
                    self.data
                        .set_way_nodes(handle, &node_handles)
                        .map_err(|err| CommandError::Invalid(err.to_string()))?;
                    let tags = intern_remote_tags(&mut self.data, remote, view.info.tags);
                    self.data.ways.set_tags(handle, tags);
                    let meta = copy_meta(&mut self.data, remote, view.info.meta);
                    self.data.ways.set_meta(handle, meta);
                    self.data.mark_synced(ObjectKind::Way, id);
                    self.index.update_way(&self.data, handle);
                    self.data.mark_way_dirty(handle);
                    stats.updated += 1;
                }
                continue;
            }
            let tags = tags_from_remote(&mut self.data, remote, view.info.tags);
            let meta = copy_meta(&mut self.data, remote, view.info.meta);
            let handle = self
                .data
                .push_way(
                    neo_osm::Way {
                        id,
                        nodes: node_ids.iter().copied().collect(),
                        tags,
                        meta,
                    },
                    MissingPolicy::Error,
                )?
                .ok_or_else(|| CommandError::Invalid("way rejected".into()))?;
            self.index.update_way(&self.data, handle);
            self.data.mark_way_dirty(handle);
            stats.added += 1;
        }

        for relation in remote.iter_relations() {
            if relation.id.is_local() {
                mapping
                    .entry((ObjectKind::Relation, relation.id))
                    .or_insert_with(|| self.next_local_id());
            }
        }
        let local_rel_ids: Vec<ObjectId> = remote
            .iter_relations()
            .filter(|item| item.id.is_local())
            .map(|item| item.id)
            .collect();
        for id in order_relations_for_import(remote, &local_rel_ids, &mapping) {
            let handle = remote.relation_handle(id).ok_or(CommandError::NotFound {
                kind: ObjectKind::Relation,
                id,
            })?;
            let view = remote.relation_view(handle).ok_or(CommandError::NotFound {
                kind: ObjectKind::Relation,
                id,
            })?;
            let new_id = mapping[&(ObjectKind::Relation, id)];
            let members = map_relation_members(remote, view.members, &mapping)?;
            let osm_members = relation_members_with_roles(&mut self.data, &members);
            let tags = tags_from_remote(&mut self.data, remote, view.info.tags);
            self.data
                .push_relation(
                    neo_osm::Relation {
                        id: new_id,
                        members: osm_members,
                        tags,
                        meta: neo_core::Meta::new_local(),
                    },
                    MissingPolicy::Error,
                )?
                .ok_or_else(|| CommandError::Invalid("relation rejected".into()))?;
            self.data
                .set_change_state(ObjectKind::Relation, new_id, ChangeState::Added);
            stats.added += 1;
        }
        for relation in remote.iter_relations() {
            let id = relation.id;
            if id.is_local() {
                continue;
            }
            let view = remote
                .relation_view(relation.handle)
                .ok_or(CommandError::NotFound {
                    kind: ObjectKind::Relation,
                    id,
                })?;
            let members = map_relation_members(remote, view.members, &mapping)?;
            if self.data.relation_handle(id).is_some() {
                if self.data.change_state(ObjectKind::Relation, id).is_dirty() {
                    stats.skipped += 1;
                } else {
                    let handle = self.data.relation_handle(id).expect("checked");
                    let stored = members_to_stored(&mut self.data, &members)?;
                    self.data.relations.set_members(handle, &stored);
                    let tags = intern_remote_tags(&mut self.data, remote, view.info.tags);
                    self.data.relations.set_tags(handle, tags);
                    let meta = copy_meta(&mut self.data, remote, view.info.meta);
                    self.data.relations.set_meta(handle, meta);
                    self.data.mark_synced(ObjectKind::Relation, id);
                    stats.updated += 1;
                }
                continue;
            }
            let osm_members = relation_members_with_roles(&mut self.data, &members);
            let tags = tags_from_remote(&mut self.data, remote, view.info.tags);
            let meta = copy_meta(&mut self.data, remote, view.info.meta);
            self.data
                .push_relation(
                    neo_osm::Relation {
                        id,
                        members: osm_members,
                        tags,
                        meta,
                    },
                    MissingPolicy::Error,
                )?
                .ok_or_else(|| CommandError::Invalid("relation rejected".into()))?;
            stats.added += 1;
        }

        self.ensure_referrers();
        self.index = SpatialIndex::build(&self.data);
        self.world_cache = WorldCache::build(&self.data);
        Ok(stats)
    }

    /// Copy selected objects from another DataSet into the current dataset; allocate new negative IDs, remap internal references, optionally offset.
    /// Returns the number of objects actually created. Objects are added in one command batch in node->way->relation order.
    pub fn import_from_dataset(
        &mut self,
        remote: &DataSet,
        selected: &[(ObjectKind, ObjectId)],
        offset_m: f64,
    ) -> Result<usize, CommandError> {
        // First pass: skip selected items absent from remote, deduplicate, and allocate new negative IDs per object kind.
        let mut mapping: HashMap<(ObjectKind, ObjectId), ObjectId> = HashMap::new();
        let mut node_ids: Vec<ObjectId> = Vec::new();
        let mut way_ids: Vec<ObjectId> = Vec::new();
        let mut relation_ids: Vec<ObjectId> = Vec::new();
        for &(kind, id) in selected {
            let exists = match kind {
                ObjectKind::Node => remote.node_handle(id).is_some(),
                ObjectKind::Way => remote.way_handle(id).is_some(),
                ObjectKind::Relation => remote.relation_handle(id).is_some(),
                // changeset is not a copyable primitive.
                ObjectKind::Changeset => false,
            };
            if !exists || mapping.contains_key(&(kind, id)) {
                continue;
            }
            let new_id = self.next_local_id();
            mapping.insert((kind, id), new_id);
            match kind {
                ObjectKind::Node => node_ids.push(id),
                ObjectKind::Way => way_ids.push(id),
                ObjectKind::Relation => relation_ids.push(id),
                ObjectKind::Changeset => {}
            }
        }

        let mut cmds: Vec<Box<dyn Command>> = Vec::with_capacity(mapping.len());

        // Nodes: positions are offset in WebMercator metres; tags are re-interned across pools.
        for id in node_ids {
            let handle = remote.node_handle(id).ok_or(CommandError::NotFound {
                kind: ObjectKind::Node,
                id,
            })?;
            let view = remote.node_view(handle).ok_or(CommandError::NotFound {
                kind: ObjectKind::Node,
                id,
            })?;
            let tags = remote_tag_strings(remote, view.info.tags);
            let mut cmd = commands::AddNode::new(offset_latlon(view.pos(), offset_m), tags);
            cmd.id = Some(mapping[&(ObjectKind::Node, id)]);
            cmds.push(Box::new(cmd));
        }

        // Ways: copied node references are mapped to new IDs; uncopied ones keep the remote ID (validated by precheck).
        for id in way_ids {
            let handle = remote.way_handle(id).ok_or(CommandError::NotFound {
                kind: ObjectKind::Way,
                id,
            })?;
            let view = remote.way_view(handle).ok_or(CommandError::NotFound {
                kind: ObjectKind::Way,
                id,
            })?;
            let mut nodes = Vec::with_capacity(view.node_refs.len());
            for node_handle in view.node_refs {
                let remote_node = remote
                    .nodes
                    .id(*node_handle)
                    .ok_or_else(|| CommandError::Invalid(format!("way {} has a dangling node ref", id.0)))?;
                nodes.push(
                    mapping
                        .get(&(ObjectKind::Node, remote_node))
                        .copied()
                        .unwrap_or(remote_node),
                );
            }
            let tags = remote_tag_strings(remote, view.info.tags);
            let mut cmd = commands::AddWay::new(nodes, tags);
            cmd.id = Some(mapping[&(ObjectKind::Way, id)]);
            cmds.push(Box::new(cmd));
        }

        // Relations: members are mapped the same way; if a member is a copied relation, it is added before its referrers.
        for id in order_relations_for_import(remote, &relation_ids, &mapping) {
            let handle = remote.relation_handle(id).ok_or(CommandError::NotFound {
                kind: ObjectKind::Relation,
                id,
            })?;
            let view = remote.relation_view(handle).ok_or(CommandError::NotFound {
                kind: ObjectKind::Relation,
                id,
            })?;
            let mut members = Vec::with_capacity(view.members.len());
            for member in view.members {
                let remote_member = match member.target.kind {
                    ObjectKind::Node => member.target.as_node().and_then(|h| remote.nodes.id(h)),
                    ObjectKind::Way => member.target.as_way().and_then(|h| remote.ways.id(h)),
                    ObjectKind::Relation => member.target.as_relation().and_then(|h| remote.relations.id(h)),
                    ObjectKind::Changeset => None,
                }
                .ok_or_else(|| CommandError::Invalid("dangling relation member".into()))?;
                members.push(RelationMemberSpec {
                    kind: member.target.kind,
                    id: mapping
                        .get(&(member.target.kind, remote_member))
                        .copied()
                        .unwrap_or(remote_member),
                    role: remote.resolve(member.role).to_owned(),
                });
            }
            let tags = remote_tag_strings(remote, view.info.tags);
            let mut cmd = commands::AddRelation::new(members, tags);
            cmd.id = Some(mapping[&(ObjectKind::Relation, id)]);
            cmds.push(Box::new(cmd));
        }

        let created = cmds.len();
        self.execute_batch(cmds)?;
        Ok(created)
    }
}

/// Translate lat/lon in WebMercator metres; return as-is when `offset_m == 0` to avoid floating-point round-trip error.
fn offset_latlon(pos: LatLon, offset_m: f64) -> LatLon {
    if offset_m == 0.0 {
        return pos;
    }
    WorldPoint::from_latlon(pos)
        .offset(offset_m, offset_m)
        .to_latlon()
}

/// Parse a remote tag set into (key, value) string pairs (`Sym` cannot cross pools).
fn remote_tag_strings(remote: &DataSet, tags: TagSetId) -> Vec<(String, String)> {
    remote
        .tag_pairs(tags)
        .iter()
        .map(|(key, value)| (remote.resolve(*key).to_owned(), remote.resolve(*value).to_owned()))
        .collect()
}

/// Relation import order: copied relation members are added before their referrers (keeps the selection order stable).
///
/// Cyclic references cannot be established within a batch; keep the original order and let `execute_batch`'s precheck report an error and roll back.
fn order_relations_for_import(
    remote: &DataSet,
    ids: &[ObjectId],
    copied: &HashMap<(ObjectKind, ObjectId), ObjectId>,
) -> Vec<ObjectId> {
    let dependencies: Vec<(ObjectId, Vec<ObjectId>)> = ids
        .iter()
        .map(|id| (*id, copied_relation_members(remote, *id, copied)))
        .collect();
    let mut ordered: Vec<ObjectId> = Vec::with_capacity(ids.len());
    let mut remaining: Vec<ObjectId> = ids.to_vec();
    while !remaining.is_empty() {
        let mut progressed = false;
        let mut deferred: Vec<ObjectId> = Vec::with_capacity(remaining.len());
        for id in remaining {
            let ready = dependencies
                .iter()
                .find(|(owner, _)| *owner == id)
                .map(|(_, deps)| deps.iter().all(|dep| ordered.contains(dep)))
                .unwrap_or(true);
            if ready {
                ordered.push(id);
                progressed = true;
            } else {
                deferred.push(id);
            }
        }
        if !progressed {
            ordered.extend(deferred);
            break;
        }
        remaining = deferred;
    }
    ordered
}

/// List of relation IDs among a relation's members that are "also being copied".
fn copied_relation_members(
    remote: &DataSet,
    id: ObjectId,
    copied: &HashMap<(ObjectKind, ObjectId), ObjectId>,
) -> Vec<ObjectId> {
    let Some(handle) = remote.relation_handle(id) else {
        return Vec::new();
    };
    let Some(view) = remote.relation_view(handle) else {
        return Vec::new();
    };
    view.members
        .iter()
        .filter(|member| member.target.kind == ObjectKind::Relation)
        .filter_map(|member| member.target.as_relation().and_then(|h| remote.relations.id(h)))
        .filter(|member_id| copied.contains_key(&(ObjectKind::Relation, *member_id)))
        .collect()
}

fn intern_remote_tags(data: &mut DataSet, remote: &DataSet, tags: TagSetId) -> TagSetId {
    let pairs: Vec<(Sym, Sym)> = remote
        .tag_pairs(tags)
        .iter()
        .map(|(key, value)| {
            (
                data.intern(remote.resolve(*key)),
                data.intern(remote.resolve(*value)),
            )
        })
        .collect();
    data.add_tags(&pairs)
}

/// Read relation members as serializable descriptions (shared by Editor and EditContext).
fn relation_members_of(data: &DataSet, id: ObjectId) -> Result<Vec<RelationMemberSpec>, CommandError> {
    let handle = data.relation_handle(id).ok_or(CommandError::NotFound {
        kind: ObjectKind::Relation,
        id,
    })?;
    let view = data.relation_view(handle).ok_or(CommandError::NotFound {
        kind: ObjectKind::Relation,
        id,
    })?;
    let mut out = Vec::with_capacity(view.members.len());
    for member in view.members {
        let member_id = match member.target.kind {
            ObjectKind::Node => member.target.as_node().and_then(|handle| data.nodes.id(handle)),
            ObjectKind::Way => member.target.as_way().and_then(|handle| data.ways.id(handle)),
            ObjectKind::Relation => member
                .target
                .as_relation()
                .and_then(|handle| data.relations.id(handle)),
            ObjectKind::Changeset => None,
        };
        let Some(member_id) = member_id else {
            return Err(CommandError::Invalid("dangling relation member".into()));
        };
        out.push(RelationMemberSpec {
            kind: member.target.kind,
            id: member_id,
            role: data.resolve(member.role).to_owned(),
        });
    }
    Ok(out)
}

/// Merge result counts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MergeStats {
    /// Number of objects newly merged in.
    pub added: usize,
    /// Number of unmodified objects overwritten with remote.
    pub updated: usize,
    /// Number of objects skipped because they were modified locally.
    pub skipped: usize,
}

fn tags_from_remote(data: &mut DataSet, remote: &DataSet, tags: TagSetId) -> Tags {
    Tags::from_pairs(remote.tag_pairs(tags).iter().map(|(key, value)| {
        (
            data.intern(remote.resolve(*key)),
            data.intern(remote.resolve(*value)),
        )
    }))
}

fn copy_meta(data: &mut DataSet, remote: &DataSet, meta: neo_core::Meta) -> neo_core::Meta {
    let user = if meta.user == Sym::EMPTY {
        Sym::EMPTY
    } else {
        data.intern(remote.resolve(meta.user))
    };
    neo_core::Meta { user, ..meta }
}

fn map_way_node_ids(
    remote: &DataSet,
    node_refs: &[Handle<NodeSlot>],
    mapping: &HashMap<(ObjectKind, ObjectId), ObjectId>,
) -> Result<Vec<ObjectId>, CommandError> {
    let mut nodes = Vec::with_capacity(node_refs.len());
    for node_handle in node_refs {
        let remote_node = remote
            .nodes
            .id(*node_handle)
            .ok_or_else(|| CommandError::Invalid("way has a dangling node ref".into()))?;
        nodes.push(
            mapping
                .get(&(ObjectKind::Node, remote_node))
                .copied()
                .unwrap_or(remote_node),
        );
    }
    Ok(nodes)
}

fn map_relation_members(
    remote: &DataSet,
    members: &[StoredMember],
    mapping: &HashMap<(ObjectKind, ObjectId), ObjectId>,
) -> Result<Vec<RelationMemberSpec>, CommandError> {
    let mut out = Vec::with_capacity(members.len());
    for member in members {
        let remote_id = match member.target.kind {
            ObjectKind::Node => member.target.as_node().and_then(|h| remote.nodes.id(h)),
            ObjectKind::Way => member.target.as_way().and_then(|h| remote.ways.id(h)),
            ObjectKind::Relation => member.target.as_relation().and_then(|h| remote.relations.id(h)),
            ObjectKind::Changeset => None,
        }
        .ok_or_else(|| CommandError::Invalid("dangling relation member".into()))?;
        out.push(RelationMemberSpec {
            kind: member.target.kind,
            id: mapping
                .get(&(member.target.kind, remote_id))
                .copied()
                .unwrap_or(remote_id),
            role: remote.resolve(member.role).to_owned(),
        });
    }
    Ok(out)
}

fn members_to_stored(
    data: &mut DataSet,
    members: &[RelationMemberSpec],
) -> Result<Vec<StoredMember>, CommandError> {
    let mut stored = Vec::with_capacity(members.len());
    for member in members {
        let target = match member.kind {
            ObjectKind::Node => data.node_handle(member.id).map(AnyHandle::from_node),
            ObjectKind::Way => data.way_handle(member.id).map(AnyHandle::from_way),
            ObjectKind::Relation => data.relation_handle(member.id).map(AnyHandle::from_relation),
            ObjectKind::Changeset => {
                return Err(CommandError::Invalid("bad relation member kind".into()));
            }
        }
        .ok_or(CommandError::NotFound {
            kind: member.kind,
            id: member.id,
        })?;
        stored.push(StoredMember {
            role: data.intern(&member.role),
            target,
        });
    }
    Ok(stored)
}

fn relation_members_with_roles(
    data: &mut DataSet,
    members: &[RelationMemberSpec],
) -> smallvec::SmallVec<[neo_osm::RelationMember; 4]> {
    members
        .iter()
        .map(|member| neo_osm::RelationMember {
            kind: member.kind,
            id: member.id,
            role: data.intern(&member.role),
        })
        .collect()
}

/// Convenience: world coordinates (for tools/UI).
pub fn world_of(data: &DataSet, node: ObjectId) -> Option<WorldPoint> {
    let h = data.node_handle(node)?;
    let pos = data.nodes.position(h)?;
    Some(WorldPoint::from_latlon(pos))
}
