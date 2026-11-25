//! Delete planning: expand "delete object" into a batch of atomic commands following JOSM semantics.
//!
//! The OSM API rejects deleting a node still referenced by a way (HTTP 412 `Node … is still used by ways …`),
//! but allows **modifying the way first, then deleting the node** within the same changeset — JOSM/iD delete a vertex exactly this way.
//! So the referrers must be fixed locally first; otherwise the user cannot delete a point on a line or polygon at all.
//!
//! This module only **generates commands**; it does not execute them. The caller submits via [`Editor::execute_batch`],
//! so that "referrer fixes + deletion" share one transaction and one undo record.
//!
//! Division of labour with `DeleteObject::precheck`: the command itself stays strict (reject when referenced),
//! as the last safety net; cascade logic is concentrated in the planner so command undo stays simple.

use std::collections::{BTreeMap, BTreeSet};

use neo_core::{ObjectId, ObjectKind};

use crate::command::{Command, CommandError};
use crate::commands::{DeleteObject, RelationMemberSpec, SetRelationMembers, SetWayNodes};
use crate::editor::Editor;

/// The plan produced by expanding one deletion.
#[derive(Default)]
pub struct DeletePlan {
    /// Commands ordered safely: referrer fixes first, deletions last.
    pub commands: Vec<Box<dyn Command>>,
    /// Objects that will be explicitly deleted (deduplicated, sorted).
    pub targets: Vec<(ObjectKind, ObjectId)>,
    /// Ways removed along with the deletion because fewer than 2 nodes remain. If non-empty, the caller should confirm with the user first.
    pub reaped_ways: Vec<ObjectId>,
    /// Ways that had nodes removed.
    pub touched_ways: Vec<ObjectId>,
    /// Relations that had members removed.
    pub touched_relations: Vec<ObjectId>,
}

impl DeletePlan {
    /// Whether the plan is empty (no executable commands).
    pub fn is_empty(&self) -> bool {
        self.commands.is_empty()
    }
}

impl std::fmt::Debug for DeletePlan {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeletePlan")
            .field("targets", &self.targets)
            .field("reaped_ways", &self.reaped_ways)
            .field("touched_ways", &self.touched_ways)
            .field("touched_relations", &self.touched_relations)
            .field("commands", &self.commands.len())
            .finish()
    }
}

/// Build the complete plan for one deletion.
///
/// Objects in `targets` are deleted, and the ways / relations referencing them are fixed up accordingly:
///
/// * A node still referenced by a way -> remove it from that way's node sequence; if fewer than 2 nodes remain,
///   delete the way as well (counted in [`DeletePlan::reaped_ways`]; the caller should notify the user).
/// * A node / way / relation still referenced by a relation -> drop that member from the member list.
/// * The order is fixed: fix relation members -> fix way nodes -> delete ways -> delete nodes -> delete relations.
///   This order guarantees every command's `precheck` passes during batch execution.
///
/// Non-existent objects yield [`CommandError::NotFound`]; if no valid object is left, an empty plan is returned.
pub fn plan_delete(
    editor: &mut Editor,
    targets: &[(ObjectKind, ObjectId)],
) -> Result<DeletePlan, CommandError> {
    // The referrer table is read-only and the dirty flag is set by write commands; refresh before planning or referrers will be missed.
    editor.refresh_referrers();

    let mut nodes: BTreeSet<ObjectId> = BTreeSet::new();
    let mut ways: BTreeSet<ObjectId> = BTreeSet::new();
    let mut relations: BTreeSet<ObjectId> = BTreeSet::new();
    for (kind, id) in targets {
        match kind {
            ObjectKind::Node => {
                nodes.insert(*id);
            }
            ObjectKind::Way => {
                ways.insert(*id);
            }
            ObjectKind::Relation => {
                relations.insert(*id);
            }
            ObjectKind::Changeset => {
                return Err(CommandError::Invalid(
                    "changeset is not a deletable edit object".into(),
                ));
            }
        }
    }

    for id in &nodes {
        if editor.data().node_handle(*id).is_none() {
            return Err(CommandError::NotFound {
                kind: ObjectKind::Node,
                id: *id,
            });
        }
    }
    for id in &ways {
        if editor.data().way_handle(*id).is_none() {
            return Err(CommandError::NotFound {
                kind: ObjectKind::Way,
                id: *id,
            });
        }
    }
    for id in &relations {
        if editor.data().relation_handle(*id).is_none() {
            return Err(CommandError::NotFound {
                kind: ObjectKind::Relation,
                id: *id,
            });
        }
    }

    // 1. Find ways that reference a deleted node but are not themselves in the delete set (these ways need node removal or full deletion).
    let mut shrink_candidates: BTreeSet<ObjectId> = BTreeSet::new();
    {
        let data = editor.data();
        let referrers = editor.referrers();
        for id in &nodes {
            let Some(handle) = data.node_handle(*id) else {
                continue;
            };
            for way in referrers.ways_of_node(handle) {
                if let Some(way_id) = data.ways.id(*way)
                    && !ways.contains(&way_id)
                {
                    shrink_candidates.insert(way_id);
                }
            }
        }
    }

    let mut resized: BTreeMap<ObjectId, Vec<ObjectId>> = BTreeMap::new();
    let mut reaped: BTreeSet<ObjectId> = BTreeSet::new();
    for way_id in shrink_candidates {
        let Some(handle) = editor.data().way_handle(way_id) else {
            continue;
        };
        let current = editor.data().try_way_node_ids(handle)?;
        let before = current.len();
        let kept: Vec<ObjectId> = current.into_iter().filter(|node| !nodes.contains(node)).collect();
        if kept.len() == before {
            // Referrer table and node sequence disagree (should not happen in theory): do not invent data changes.
            continue;
        }
        if kept.len() >= 2 {
            resized.insert(way_id, kept);
        } else {
            reaped.insert(way_id);
        }
    }

    // 2. Find relations whose member lists need changes: those referencing deleted nodes/ways/relations, or referencing ways removed with them.
    let mut relations_to_rewrite: BTreeSet<ObjectId> = BTreeSet::new();
    {
        let data = editor.data();
        let referrers = editor.referrers();
        for id in &nodes {
            let Some(handle) = data.node_handle(*id) else {
                continue;
            };
            for relation in referrers.relations_of_node(handle) {
                if let Some(relation_id) = data.relations.id(*relation) {
                    relations_to_rewrite.insert(relation_id);
                }
            }
        }
        for id in reaped.iter().chain(ways.iter()) {
            let Some(handle) = data.way_handle(*id) else {
                continue;
            };
            for relation in referrers.relations_of_way(handle) {
                if let Some(relation_id) = data.relations.id(*relation) {
                    relations_to_rewrite.insert(relation_id);
                }
            }
        }
        for id in &relations {
            let Some(handle) = data.relation_handle(*id) else {
                continue;
            };
            for relation in referrers.relations_of_relation(handle) {
                if let Some(relation_id) = data.relations.id(*relation) {
                    relations_to_rewrite.insert(relation_id);
                }
            }
        }
    }

    let mut member_updates: BTreeMap<ObjectId, Vec<RelationMemberSpec>> = BTreeMap::new();
    for relation_id in relations_to_rewrite {
        let members = editor.relation_members(relation_id)?;
        let before = members.len();
        let kept: Vec<RelationMemberSpec> = members
            .into_iter()
            .filter(|member| !vanishes(member.kind, member.id, &nodes, &ways, &reaped, &relations))
            .collect();
        if kept.len() != before {
            member_updates.insert(relation_id, kept);
        }
    }

    // 3. Assemble commands in dependency order.
    let mut commands: Vec<Box<dyn Command>> = Vec::new();
    let touched_relations: Vec<ObjectId> = member_updates.keys().copied().collect();
    for (relation, members) in member_updates {
        commands.push(Box::new(SetRelationMembers::new(relation, members)));
    }
    let touched_ways: Vec<ObjectId> = resized.keys().copied().collect();
    for (way, node_ids) in resized {
        commands.push(Box::new(SetWayNodes::new(way, node_ids)));
    }
    let reaped_ways: Vec<ObjectId> = reaped.iter().copied().collect();
    for way in &reaped_ways {
        commands.push(Box::new(DeleteObject::new(ObjectKind::Way, *way)));
    }
    for way in &ways {
        commands.push(Box::new(DeleteObject::new(ObjectKind::Way, *way)));
    }
    for node in &nodes {
        commands.push(Box::new(DeleteObject::new(ObjectKind::Node, *node)));
    }
    for relation in &relations {
        commands.push(Box::new(DeleteObject::new(ObjectKind::Relation, *relation)));
    }

    let mut target_list: Vec<(ObjectKind, ObjectId)> = Vec::new();
    target_list.extend(nodes.iter().map(|id| (ObjectKind::Node, *id)));
    target_list.extend(ways.iter().map(|id| (ObjectKind::Way, *id)));
    target_list.extend(relations.iter().map(|id| (ObjectKind::Relation, *id)));
    Ok(DeletePlan {
        commands,
        targets: target_list,
        reaped_ways,
        touched_ways,
        touched_relations,
    })
}

/// Whether this member disappears with this deletion.
fn vanishes(
    kind: ObjectKind,
    id: ObjectId,
    nodes: &BTreeSet<ObjectId>,
    ways: &BTreeSet<ObjectId>,
    reaped: &BTreeSet<ObjectId>,
    relations: &BTreeSet<ObjectId>,
) -> bool {
    match kind {
        ObjectKind::Node => nodes.contains(&id),
        ObjectKind::Way => ways.contains(&id) || reaped.contains(&id),
        ObjectKind::Relation => relations.contains(&id),
        ObjectKind::Changeset => false,
    }
}
