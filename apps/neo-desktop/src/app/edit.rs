//! NeoApp::edit methods. Same behavior as before the app.rs split.

use neo_core::{LatLon, ObjectId, ObjectKind};
use neo_editor::{AddNode, AddRelation, AddWay, RelationMemberSpec};
use neo_geo::WorldPoint;
use neo_i18n::{t, tf};
use neo_tools::{ToolEffect, ToolNote};

use super::{Clipboard, ClipboardNode, ClipboardRelation, ClipboardWay, NeoApp};

impl NeoApp {
    /// Nodes involved in the current selection (selected nodes + all nodes of selected ways, deduplicated)
    /// Copies the current selection to the internal clipboard
    pub(super) fn copy_selection(&mut self) {
        let data = self.editor.data();
        let mut clip = Clipboard::default();
        for id in self.editor.selection().nodes() {
            if let Some(handle) = data.node_handle(*id)
                && let Some(view) = data.node_view(handle)
            {
                clip.nodes.push(ClipboardNode {
                    id: *id,
                    pos: view.info.pos,
                    tags: data
                        .tags_of(view.info.tags)
                        .iter()
                        .map(|(key, value)| (data.resolve(key).to_owned(), data.resolve(value).to_owned()))
                        .collect(),
                });
            }
        }
        for id in self.editor.selection().ways() {
            if let Some(handle) = data.way_handle(*id)
                && let Some(view) = data.way_view(handle)
            {
                let nodes = data.way_node_ids(handle);
                // JOSM copies a way together with its nodes; otherwise pasting into another document would
                // reference nodes with the same id but different meaning
                for node in &nodes {
                    if clip.nodes.iter().any(|existing| existing.id == *node) {
                        continue;
                    }
                    if let Some(node_handle) = data.node_handle(*node)
                        && let Some(node_view) = data.node_view(node_handle)
                    {
                        clip.nodes.push(ClipboardNode {
                            id: *node,
                            pos: node_view.info.pos,
                            tags: data
                                .tags_of(node_view.info.tags)
                                .iter()
                                .map(|(key, value)| {
                                    (data.resolve(key).to_owned(), data.resolve(value).to_owned())
                                })
                                .collect(),
                        });
                    }
                }
                clip.ways.push(ClipboardWay {
                    id: *id,
                    nodes,
                    tags: data
                        .tags_of(view.info.tags)
                        .iter()
                        .map(|(key, value)| (data.resolve(key).to_owned(), data.resolve(value).to_owned()))
                        .collect(),
                });
            }
        }
        for id in self.editor.selection().relations() {
            if let Some(handle) = data.relation_handle(*id)
                && let Some(view) = data.relation_view(handle)
            {
                let mut members = Vec::with_capacity(view.members.len());
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
                    if let Some(member_id) = member_id {
                        members.push((
                            member.target.kind,
                            member_id,
                            data.resolve(member.role).to_owned(),
                        ));
                    }
                }
                clip.relations.push(ClipboardRelation {
                    id: *id,
                    members,
                    tags: data
                        .tags_of(view.info.tags)
                        .iter()
                        .map(|(key, value)| (data.resolve(key).to_owned(), data.resolve(value).to_owned()))
                        .collect(),
                });
            }
        }
        let count = clip.len();
        self.clipboard = clip;
        self.status = tf("status.copied", &[("n", &count.to_string())]);
    }

    /// Pastes the clipboard: allocates new
    /// IDs, remaps internal references, and commits as one atomic batch
    pub(super) fn paste_clipboard(&mut self, offset_m: f64) {
        if self.clipboard.len() == 0 {
            self.status = t("app.status.clipboard_empty").to_owned();
            return;
        }
        if !self.layer_active || !self.layer_visible {
            self.status = t("app.status.layer_readonly").to_owned();
            return;
        }
        let clip = self.clipboard.clone();
        let mut node_map = std::collections::HashMap::<ObjectId, ObjectId>::new();
        let mut way_map = std::collections::HashMap::<ObjectId, ObjectId>::new();
        let mut relation_map = std::collections::HashMap::<ObjectId, ObjectId>::new();
        for node in &clip.nodes {
            node_map.insert(node.id, self.editor.next_local_id());
        }
        for way in &clip.ways {
            way_map.insert(way.id, self.editor.next_local_id());
        }
        for relation in &clip.relations {
            relation_map.insert(relation.id, self.editor.next_local_id());
        }

        let mut commands: Vec<Box<dyn neo_editor::Command>> = Vec::new();
        let mut created_nodes = Vec::new();
        for node in &clip.nodes {
            let Some(new_id) = node_map.get(&node.id).copied() else {
                continue;
            };
            let world = WorldPoint::from_latlon(node.pos);
            let pos = WorldPoint::new(world.x + offset_m, world.y + offset_m).to_latlon();
            let mut cmd = AddNode::new(pos, node.tags.clone());
            cmd.id = Some(new_id);
            created_nodes.push(new_id);
            commands.push(Box::new(cmd));
        }
        for way in &clip.ways {
            let Some(new_id) = way_map.get(&way.id).copied() else {
                continue;
            };
            let nodes = way
                .nodes
                .iter()
                .map(|id| node_map.get(id).copied().unwrap_or(*id))
                .collect();
            let mut cmd = AddWay::new(nodes, way.tags.clone());
            cmd.id = Some(new_id);
            commands.push(Box::new(cmd));
        }
        // Relations can have member dependencies: when a parent relation references a newly created child relation in the same batch,
        // the child must be created first, otherwise AddRelation::precheck can't find the member and the whole batch rolls back (paste silently fails).
        let relation_ids: Vec<ObjectId> = clip.relations.iter().map(|relation| relation.id).collect();
        let relation_order = neo_editor::order_relations_by_dependency(&relation_ids, |id| {
            clip.relations
                .iter()
                .find(|relation| relation.id == id)
                .map(|relation| {
                    let members: Vec<ObjectId> = relation
                        .members
                        .iter()
                        .filter(|(kind, _, _)| *kind == ObjectKind::Relation)
                        .map(|(_, member, _)| *member)
                        .collect();
                    members
                })
                .unwrap_or_default()
        });
        for relation_id in relation_order {
            let Some(relation) = clip.relations.iter().find(|relation| relation.id == relation_id) else {
                continue;
            };
            let Some(new_id) = relation_map.get(&relation.id).copied() else {
                continue;
            };
            let members = relation
                .members
                .iter()
                .map(|(kind, id, role)| RelationMemberSpec {
                    kind: *kind,
                    id: match kind {
                        ObjectKind::Node => node_map.get(id).copied().unwrap_or(*id),
                        ObjectKind::Way => way_map.get(id).copied().unwrap_or(*id),
                        ObjectKind::Relation => relation_map.get(id).copied().unwrap_or(*id),
                        ObjectKind::Changeset => *id,
                    },
                    role: role.clone(),
                })
                .collect();
            let mut cmd = AddRelation::new(members, relation.tags.clone());
            cmd.id = Some(new_id);
            commands.push(Box::new(cmd));
        }

        match self.editor.execute_batch(commands) {
            Ok(()) => {
                self.editor.selection_mut().clear();
                for id in &created_nodes {
                    self.editor.selection_mut().insert(ObjectKind::Node, *id);
                }
                for id in way_map.values() {
                    self.editor.selection_mut().insert(ObjectKind::Way, *id);
                }
                for id in relation_map.values() {
                    self.editor.selection_mut().insert(ObjectKind::Relation, *id);
                }
                self.scene = None;
                self.status = tf("status.pasted", &[("n", &clip.len().to_string())]);
            }
            Err(err) => {
                self.status = tf("app.error.paste", &[("err", &err.to_string())]);
            }
        }
    }

    fn render_note(note: ToolNote) -> String {
        match note.field {
            Some((name, value)) => tf(note.key, &[(name, &value)]),
            None => t(note.key).to_owned(),
        }
    }

    pub(super) fn apply_tool(&mut self, effect: ToolEffect) {
        match effect {
            ToolEffect::Note(note) => self.status = Self::render_note(note),
            ToolEffect::Edited { note, select } => {
                if let Some((kind, id)) = select {
                    self.editor.selection_mut().select_only(kind, id);
                }
                self.scene = None;
                self.status = Self::render_note(note);
            }
            ToolEffect::Moved(outcome) => {
                self.apply_outcome(outcome);
                self.scene = None;
            }
        }
    }

    pub(super) fn merge_selected_nodes(&mut self) {
        let effect = neo_tools::merge_selected_nodes(&mut self.editor);
        self.apply_tool(effect);
    }

    pub(super) fn align_selected_nodes(&mut self) {
        let effect = neo_tools::align_selected_nodes(&mut self.editor);
        self.apply_tool(effect);
    }

    pub(super) fn distribute_selected_nodes(&mut self) {
        let effect = neo_tools::distribute_selected_nodes(&mut self.editor);
        self.apply_tool(effect);
    }

    pub(super) fn mirror_selected_nodes(&mut self, horizontal: bool) {
        let effect = neo_tools::mirror_selected_nodes(&mut self.editor, horizontal);
        self.apply_tool(effect);
    }

    pub(super) fn unglue_selected_nodes(&mut self) {
        let effect = neo_tools::unglue_selected_nodes(&mut self.editor);
        self.apply_tool(effect);
    }

    pub(super) fn parallel_copy_way(&mut self) {
        let effect = neo_tools::parallel_copy_way(&mut self.editor);
        self.apply_tool(effect);
    }

    pub(super) fn orthogonalize_selected(&mut self) {
        let effect = neo_tools::orthogonalize_selected(&mut self.editor);
        self.apply_tool(effect);
    }

    pub(super) fn join_selected_areas(&mut self) {
        let effect = neo_tools::join_selected_areas(&mut self.editor);
        self.apply_tool(effect);
    }

    pub(super) fn move_node_onto_way(&mut self) {
        let effect = neo_tools::move_node_onto_way(&mut self.editor);
        self.apply_tool(effect);
    }

    pub(super) fn extrude_selected_way(&mut self) {
        let effect = neo_tools::extrude_selected_way(&mut self.editor);
        self.apply_tool(effect);
    }

    pub(super) fn collect_selection_nodes(&self) -> Vec<(ObjectId, LatLon)> {
        let mut nodes: Vec<(ObjectId, LatLon)> = Vec::new();
        let mut seen = std::collections::BTreeSet::new();
        let selection = self.editor.selection();
        for id in selection.nodes().iter().copied().collect::<Vec<_>>() {
            if seen.insert(id)
                && let Some(handle) = self.editor.data().node_handle(id)
                && let Some(from) = self.editor.data().nodes.position(handle)
            {
                nodes.push((id, from));
            }
        }
        for way in selection.ways().iter().copied().collect::<Vec<_>>() {
            if let Some(handle) = self.editor.data().way_handle(way) {
                for node in self.editor.data().way_node_ids(handle) {
                    if seen.insert(node)
                        && let Some(nh) = self.editor.data().node_handle(node)
                        && let Some(from) = self.editor.data().nodes.position(nh)
                    {
                        nodes.push((node, from));
                    }
                }
            }
        }
        nodes
    }
}
