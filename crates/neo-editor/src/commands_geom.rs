//! Geometry commands: split / merge / reverse ways.
//!
//! Semantics follow JOSM as closely as possible (docs/13 §3.1):
//! * Split: split a way in two at an interior node; the new way copies the original tags;
//! * Merge: merge two endpoint-adjacent ways (all four directions handled automatically); the old way is marked deleted;
//! * Reverse: reverse the node order and fix directional tags (`oneway=yes ↔ -1`).

use std::any::Any;

use neo_core::{ObjectId, ObjectKind};
use neo_store::DataSet;

use crate::command::{Command, CommandError, CommandKind};
use crate::commands::RelationMemberSpec;
use crate::editor::EditContext;
use crate::journal::JournalCommand;

/// Directional tag fix (the core semantics of ReverseWay).
///
/// Currently covers the most common rules of JOSM's `ReverseWayTagCorrector`:
/// `oneway=yes -> -1`, `oneway=-1 -> yes`, `oneway=1 -> -1`; all other tags are left unchanged.
/// Complete rules (turn:lanes, incline, :forward/:backward, etc.) land in stage 4.
pub fn reverse_direction_tags(tags: &[(String, String)]) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::with_capacity(tags.len());
    let mut push_unique = |key: String, value: String| {
        if let Some(slot) = out.iter_mut().find(|(k, _)| *k == key) {
            slot.1 = value;
        } else {
            out.push((key, value));
        }
    };
    for (k, v) in tags {
        // Directional suffix: `*:forward` ↔ `*:backward` (oneway:forward, turn:lanes:forward, etc.).
        if let Some(base) = k.strip_suffix(":forward") {
            push_unique(format!("{base}:backward"), v.clone());
            continue;
        }
        if let Some(base) = k.strip_suffix(":backward") {
            push_unique(format!("{base}:forward"), v.clone());
            continue;
        }
        let reversed = match (k.as_str(), v.as_str()) {
            ("oneway", "yes" | "true" | "1") => Some("-1"),
            ("oneway", "-1") => Some("yes"),
            ("incline", "up") => Some("down"),
            ("incline", "down") => Some("up"),
            _ => None,
        };
        push_unique(k.clone(), reversed.unwrap_or(v).to_owned());
    }
    out
}

/// Split a way at an interior node.
pub struct SplitWay {
    /// The original way.
    pub way: ObjectId,
    /// Index of the split point in the way's node sequence (1..len-1).
    pub at: u32,
    /// Expected ID of the new way (None = auto-assign).
    pub new_way: Option<ObjectId>,
    undo_old_nodes: Option<Vec<ObjectId>>,
    applied_new: Option<ObjectId>,
    undo_state: Option<neo_osm::ChangeState>,
    /// Relation that got the new way appended: `(relation, old member list, old change state)`.
    undo_relations: Vec<(ObjectId, Vec<RelationMemberSpec>, neo_osm::ChangeState)>,
}

impl SplitWay {
    /// Construct.
    pub fn new(way: ObjectId, at: u32) -> Self {
        Self {
            way,
            at,
            new_way: None,
            undo_old_nodes: None,
            applied_new: None,
            undo_state: None,
            undo_relations: Vec::new(),
        }
    }
}

impl Command for SplitWay {
    fn kind(&self) -> CommandKind {
        CommandKind::SplitWay
    }
    fn label(&self) -> &'static str {
        "Split way"
    }
    fn precheck(&self, data: &DataSet, _referrers: &neo_store::Referrers) -> Result<(), CommandError> {
        let handle = data.way_handle(self.way).ok_or(CommandError::NotFound {
            kind: ObjectKind::Way,
            id: self.way,
        })?;
        let len = data.ways.nodes(handle).map(|n| n.len()).unwrap_or(0);
        if (self.at as usize) < 1 || (self.at as usize) + 1 >= len {
            return Err(CommandError::Invalid(format!(
                "split index {} out of range for way of {len} nodes",
                self.at
            )));
        }
        Ok(())
    }
    fn apply(&mut self, ctx: &mut EditContext<'_>) -> Result<(), CommandError> {
        let old = ctx.way_node_ids(self.way)?;
        let tags = ctx.tags_of(ObjectKind::Way, self.way)?;
        let at = self.at as usize;
        let first: Vec<ObjectId> = old[..=at].to_vec();
        let second: Vec<ObjectId> = old[at..].to_vec();
        let new_id = self.new_way.unwrap_or_else(|| ctx.data.next_local_id());
        // Fix the new way's ID: undo->redo must not change IDs, or old IDs in the selection/log dangle.
        self.new_way = Some(new_id);
        let state = ctx.data.change_state(ObjectKind::Way, self.way);
        ctx.set_way_nodes(self.way, &first)?;
        ctx.add_way(new_id, &second, &tags)?;
        if state == neo_osm::ChangeState::Unchanged {
            ctx.data
                .set_change_state(ObjectKind::Way, self.way, neo_osm::ChangeState::Modified);
        }
        // Relations referencing the original way: insert the new way right after it (same role).
        // Otherwise the relation only covers the first half after the split, silently breaking routes and dropping a multipolygon edge.
        let way_handle = ctx.way_handle(self.way)?;
        let relation_handles: Vec<_> = ctx.referrers.relations_of_way(way_handle).to_vec();
        for handle in relation_handles {
            let Some(relation_id) = ctx.data.relations.id(handle) else {
                continue;
            };
            let old = ctx.relation_members(relation_id)?;
            let mut updated: Vec<RelationMemberSpec> = Vec::with_capacity(old.len() + 1);
            let mut changed = false;
            for member in &old {
                updated.push(member.clone());
                if member.kind == ObjectKind::Way && member.id == self.way {
                    updated.push(RelationMemberSpec {
                        kind: ObjectKind::Way,
                        id: new_id,
                        role: member.role.clone(),
                    });
                    changed = true;
                }
            }
            if !changed {
                continue;
            }
            let relation_state = ctx.data.change_state(ObjectKind::Relation, relation_id);
            ctx.set_relation_members(relation_id, &updated)?;
            if relation_state == neo_osm::ChangeState::Unchanged {
                ctx.data
                    .set_change_state(ObjectKind::Relation, relation_id, neo_osm::ChangeState::Modified);
            }
            self.undo_relations.push((relation_id, old, relation_state));
        }
        self.undo_old_nodes = Some(old);
        self.applied_new = Some(new_id);
        self.undo_state = Some(state);
        Ok(())
    }
    fn undo(&mut self, ctx: &mut EditContext<'_>) -> Result<(), CommandError> {
        // Restore relation members first (they reference the new way about to be removed), then undo the way itself.
        for (relation_id, old, state) in std::mem::take(&mut self.undo_relations).into_iter().rev() {
            ctx.set_relation_members(relation_id, &old)?;
            ctx.data
                .set_change_state(ObjectKind::Relation, relation_id, state);
        }
        if let Some(new_id) = self.applied_new {
            // Keep applied_new so redo reuses the same ID.
            ctx.remove_way(new_id)?;
        }
        if let Some(old) = self.undo_old_nodes.take() {
            ctx.set_way_nodes(self.way, &old)?;
        }
        if let Some(state) = self.undo_state.take() {
            ctx.data.set_change_state(ObjectKind::Way, self.way, state);
        }
        Ok(())
    }
    fn record(&self) -> JournalCommand {
        JournalCommand::SplitWay {
            way: self.way.0,
            at: self.at,
            new_way: self.applied_new.map(|id| id.0).unwrap_or(0),
        }
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Merge two endpoint-adjacent ways.
pub struct MergeWay {
    /// The way to keep.
    pub a: ObjectId,
    /// The way to merge in and delete.
    pub b: ObjectId,
    undo_a_nodes: Option<Vec<ObjectId>>,
    undo_a_tags: Option<Vec<(String, String)>>,
    undo_a_state: Option<neo_osm::ChangeState>,
    undo_b_state: Option<(neo_osm::ChangeState, bool)>,
    /// Relation whose members were migrated: `(relation, old member list, old change state)`.
    undo_relations: Vec<(ObjectId, Vec<RelationMemberSpec>, neo_osm::ChangeState)>,
    merged: Option<Vec<ObjectId>>,
}

impl MergeWay {
    /// Construct.
    pub fn new(a: ObjectId, b: ObjectId) -> Self {
        Self {
            a,
            b,
            undo_a_nodes: None,
            undo_a_tags: None,
            undo_a_state: None,
            undo_b_state: None,
            undo_relations: Vec::new(),
            merged: None,
        }
    }
}

/// Compute the merged node sequence (b's endpoint must coincide with one of a's endpoints).
pub fn merged_nodes(a: &[ObjectId], b: &[ObjectId]) -> Option<Vec<ObjectId>> {
    if a.len() < 2 || b.len() < 2 {
        return None;
    }
    if a.last() == b.first() {
        let mut out = a.to_vec();
        out.extend_from_slice(&b[1..]);
        return Some(out);
    }
    if a.last() == b.last() {
        let mut out = a.to_vec();
        out.extend(b.iter().rev().skip(1).copied());
        return Some(out);
    }
    if a.first() == b.last() {
        let mut out = b.to_vec();
        out.extend_from_slice(&a[1..]);
        return Some(out);
    }
    if a.first() == b.first() {
        let mut out = b.iter().rev().copied().collect::<Vec<_>>();
        out.extend_from_slice(&a[1..]);
        return Some(out);
    }
    None
}

impl Command for MergeWay {
    fn kind(&self) -> CommandKind {
        CommandKind::MergeWay
    }
    fn label(&self) -> &'static str {
        "Merge ways"
    }
    fn precheck(&self, data: &DataSet, _referrers: &neo_store::Referrers) -> Result<(), CommandError> {
        if self.a == self.b {
            return Err(CommandError::Invalid("cannot merge a way with itself".into()));
        }
        let a = data.way_handle(self.a).ok_or(CommandError::NotFound {
            kind: ObjectKind::Way,
            id: self.a,
        })?;
        let b = data.way_handle(self.b).ok_or(CommandError::NotFound {
            kind: ObjectKind::Way,
            id: self.b,
        })?;
        // Being referenced by a relation is no longer rejected: apply replaces member `b` with `a` in place (JOSM semantics).
        let an: Vec<ObjectId> = data
            .try_way_node_ids(a)
            .map_err(|_| CommandError::Invalid("way has stale node references".into()))?;
        let bn: Vec<ObjectId> = data
            .try_way_node_ids(b)
            .map_err(|_| CommandError::Invalid("way has stale node references".into()))?;
        if merged_nodes(&an, &bn).is_none() {
            return Err(CommandError::Invalid("ways are not endpoint-adjacent".into()));
        }
        Ok(())
    }
    fn apply(&mut self, ctx: &mut EditContext<'_>) -> Result<(), CommandError> {
        let an = ctx.way_node_ids(self.a)?;
        let bn = ctx.way_node_ids(self.b)?;
        let merged =
            merged_nodes(&an, &bn).ok_or_else(|| CommandError::Invalid("ways are not adjacent".into()))?;
        let a_state = ctx.data.change_state(ObjectKind::Way, self.a);
        let a_tags = ctx.tags_of(ObjectKind::Way, self.a)?;
        let b_tags = ctx.tags_of(ObjectKind::Way, self.b)?;
        let b_state = ctx.data.change_state(ObjectKind::Way, self.b);
        let b_visible = ctx.visible_of(ObjectKind::Way, self.b);
        ctx.set_way_nodes(self.a, &merged)?;
        // Merge tags: a wins; keys unique to b are added so bridge/name/ref are not silently lost.
        let mut merged_tags = a_tags.clone();
        for (key, value) in b_tags {
            if !a_tags.iter().any(|(k, _)| *k == key) {
                merged_tags.push((key, value));
            }
        }
        if merged_tags != a_tags {
            ctx.set_tags(ObjectKind::Way, self.a, &merged_tags)?;
        }
        if a_state == neo_osm::ChangeState::Unchanged {
            ctx.data
                .set_change_state(ObjectKind::Way, self.a, neo_osm::ChangeState::Modified);
        }
        // Relations referencing b: replace the member with a in place (keeping role and order) and drop duplicates.
        // This way merging a member way of a route/multipolygon is no longer rejected and leaves no dangling members.
        let b_handle = ctx.way_handle(self.b)?;
        let relation_handles: Vec<_> = ctx.referrers.relations_of_way(b_handle).to_vec();
        for handle in relation_handles {
            let Some(relation_id) = ctx.data.relations.id(handle) else {
                continue;
            };
            let old = ctx.relation_members(relation_id)?;
            let mut updated: Vec<RelationMemberSpec> = Vec::with_capacity(old.len());
            let mut changed = false;
            for member in &old {
                let next = if member.kind == ObjectKind::Way && member.id == self.b {
                    changed = true;
                    RelationMemberSpec {
                        kind: ObjectKind::Way,
                        id: self.a,
                        role: member.role.clone(),
                    }
                } else {
                    member.clone()
                };
                let duplicate = updated
                    .iter()
                    .any(|kept| kept.kind == next.kind && kept.id == next.id && kept.role == next.role);
                if duplicate {
                    changed = true;
                    continue;
                }
                updated.push(next);
            }
            if !changed {
                continue;
            }
            let relation_state = ctx.data.change_state(ObjectKind::Relation, relation_id);
            ctx.set_relation_members(relation_id, &updated)?;
            if relation_state == neo_osm::ChangeState::Unchanged {
                ctx.data
                    .set_change_state(ObjectKind::Relation, relation_id, neo_osm::ChangeState::Modified);
            }
            self.undo_relations.push((relation_id, old, relation_state));
        }
        ctx.mark_deleted(ObjectKind::Way, self.b)?;
        self.undo_a_nodes = Some(an);
        self.undo_a_tags = Some(a_tags);
        self.undo_a_state = Some(a_state);
        self.undo_b_state = Some((b_state, b_visible));
        self.merged = Some(merged);
        Ok(())
    }
    fn undo(&mut self, ctx: &mut EditContext<'_>) -> Result<(), CommandError> {
        if let Some((state, visible)) = self.undo_b_state.take() {
            ctx.restore_deleted(ObjectKind::Way, self.b, state, visible);
        }
        for (relation_id, old, state) in std::mem::take(&mut self.undo_relations).into_iter().rev() {
            ctx.set_relation_members(relation_id, &old)?;
            ctx.data
                .set_change_state(ObjectKind::Relation, relation_id, state);
        }
        if let Some(old) = self.undo_a_nodes.take() {
            ctx.set_way_nodes(self.a, &old)?;
        }
        if let Some(tags) = self.undo_a_tags.take() {
            ctx.set_tags(ObjectKind::Way, self.a, &tags)?;
        }
        if let Some(state) = self.undo_a_state.take() {
            ctx.data.set_change_state(ObjectKind::Way, self.a, state);
        }
        Ok(())
    }
    fn record(&self) -> JournalCommand {
        JournalCommand::MergeWay {
            a: self.a.0,
            b: self.b.0,
        }
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Reverse a way + fix directional tags.
pub struct ReverseWay {
    /// Way ID.
    pub way: ObjectId,
    undo_nodes: Option<Vec<ObjectId>>,
    undo_tags: Option<Vec<(String, String)>>,
    undo_state: Option<neo_osm::ChangeState>,
}

impl ReverseWay {
    /// Construct.
    pub fn new(way: ObjectId) -> Self {
        Self {
            way,
            undo_nodes: None,
            undo_tags: None,
            undo_state: None,
        }
    }
}

impl Command for ReverseWay {
    fn kind(&self) -> CommandKind {
        CommandKind::ReverseWay
    }
    fn label(&self) -> &'static str {
        "Reverse way"
    }
    fn precheck(&self, data: &DataSet, _referrers: &neo_store::Referrers) -> Result<(), CommandError> {
        if data.way_handle(self.way).is_none() {
            return Err(CommandError::NotFound {
                kind: ObjectKind::Way,
                id: self.way,
            });
        }
        Ok(())
    }
    fn apply(&mut self, ctx: &mut EditContext<'_>) -> Result<(), CommandError> {
        let mut nodes = ctx.way_node_ids(self.way)?;
        nodes.reverse();
        let tags = ctx.tags_of(ObjectKind::Way, self.way)?;
        let fixed = reverse_direction_tags(&tags);
        let state = ctx.data.change_state(ObjectKind::Way, self.way);
        ctx.set_way_nodes(self.way, &nodes)?;
        ctx.set_tags(ObjectKind::Way, self.way, &fixed)?;
        if state == neo_osm::ChangeState::Unchanged {
            ctx.data
                .set_change_state(ObjectKind::Way, self.way, neo_osm::ChangeState::Modified);
        }
        self.undo_nodes = Some(nodes);
        self.undo_tags = Some(tags);
        self.undo_state = Some(state);
        Ok(())
    }
    fn undo(&mut self, ctx: &mut EditContext<'_>) -> Result<(), CommandError> {
        if let Some(tags) = self.undo_tags.take() {
            ctx.set_tags(ObjectKind::Way, self.way, &tags)?;
        }
        if let Some(mut nodes) = self.undo_nodes.take() {
            nodes.reverse();
            ctx.set_way_nodes(self.way, &nodes)?;
        }
        if let Some(state) = self.undo_state.take() {
            ctx.data.set_change_state(ObjectKind::Way, self.way, state);
        }
        Ok(())
    }
    fn record(&self) -> JournalCommand {
        JournalCommand::ReverseWay { way: self.way.0 }
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}
