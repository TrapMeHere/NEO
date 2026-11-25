//! # neo-tools — interactive tools (select / draw / delete)
//!
//! Tools are **pure state machines**: input "click/drag end + hit result + world coordinates", output edit commands (executed through
//! `neo_editor::Editor`). No dependency on egui/wgpu, so they are unit-testable and reusable by future CLI/scripts.
//!
//! Drawing semantics:
//! * **Point**: click to create a single node;
//! * **Line**: the first click places the start point, later clicks append nodes; clicking the newest node again or `finish_way` ends it;
//! * **Area**: same as line, auto-closes at the end; clicking the start point again also closes it.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod geom;

use std::collections::BTreeMap;

use neo_core::{LatLon, ObjectId, ObjectKind};
use neo_editor::{AddNode, AddWay, Editor, MoveNode, SetTags, SetWayNodes};
use neo_geo::{SnapHit, SnapTarget};

pub use geom::{
    ToolEffect, ToolNote, align_selected_nodes, distribute_selected_nodes, extrude_selected_way,
    join_selected_areas, merge_selected_nodes, mirror_selected_nodes, move_node_onto_way,
    orthogonalize_selected, parallel_copy_way, unglue_selected_nodes,
};
pub use neo_geo::{SnapHit as DrawSnapHit, SnapTarget as DrawSnapTarget};

/// Current tool.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum ToolKind {
    /// Select/move.
    #[default]
    Select,
    /// Create a node.
    DrawPoint,
    /// Draw a polyline.
    DrawLine,
    /// Draw a closed area.
    DrawArea,
}

impl ToolKind {
    /// Display name.
    pub fn label(self) -> &'static str {
        match self {
            Self::Select => "Select",
            Self::DrawPoint => "Point",
            Self::DrawLine => "Line",
            Self::DrawArea => "Area",
        }
    }

    /// Whether this is a drawing tool.
    pub fn is_draw(self) -> bool {
        matches!(self, Self::DrawPoint | Self::DrawLine | Self::DrawArea)
    }
}

/// Hit result (obtained by the app layer with `neo_geo::HitTester`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Pick {
    /// Hit node.
    pub node: Option<ObjectId>,
    /// Hit way.
    pub way: Option<ObjectId>,
    /// Hit relation (the point is on an area's inner ring).
    pub relation: Option<ObjectId>,
}

/// Tool processing result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// No change.
    None,
    /// The selection changed.
    SelectionChanged,
    /// One edit was executed.
    Edited(String),
    /// Hint/error message.
    Message(String),
}

impl Outcome {
    /// Whether an edit happened (the app layer refreshes scene/panels based on this).
    pub fn is_edit(&self) -> bool {
        matches!(self, Self::Edited(_))
    }

    /// Get the status text.
    pub fn message(&self) -> Option<&str> {
        match self {
            Self::Edited(m) | Self::Message(m) => Some(m),
            _ => None,
        }
    }
}

/// Tool state machine.
pub struct EditController {
    /// Current tool.
    pub tool: ToolKind,
    /// Whether geometric snapping is enabled while drawing (Alt temporarily disables it; handled by the app layer).
    pub snap_enabled: bool,
    pending_nodes: Vec<ObjectId>,
    pending_way: Option<ObjectId>,
    /// Number of history entries written per step in the current draw session (for stepwise rollback via right-click).
    draw_step_cmds: Vec<usize>,
    /// A complete feature was just finished (point placed / line or area ended); the app layer should switch back to the select tool.
    finished_feature: bool,
}

impl Default for EditController {
    fn default() -> Self {
        Self::new(ToolKind::Select)
    }
}

impl EditController {
    /// Construct.
    pub fn new(tool: ToolKind) -> Self {
        Self {
            tool,
            snap_enabled: true,
            pending_nodes: Vec::new(),
            pending_way: None,
            draw_step_cmds: Vec::new(),
            finished_feature: false,
        }
    }

    /// Switch tools (ends an unfinished drawing).
    pub fn set_tool(&mut self, tool: ToolKind) {
        if tool != self.tool {
            self.reset_pending();
        }
        self.tool = tool;
    }

    /// Take and clear the "feature finished" flag.
    pub fn take_finished_feature(&mut self) -> bool {
        let done = self.finished_feature;
        self.finished_feature = false;
        done
    }

    /// Number of nodes currently being drawn.
    pub fn pending_len(&self) -> usize {
        self.pending_nodes.len()
    }

    /// The way currently being drawn.
    pub fn pending_way(&self) -> Option<ObjectId> {
        self.pending_way
    }

    /// The node sequence currently being drawn (including the start point not yet made into a way).
    pub fn pending_nodes(&self) -> &[ObjectId] {
        &self.pending_nodes
    }

    /// Abandon the unfinished way.
    pub fn reset_pending(&mut self) {
        self.pending_nodes.clear();
        self.pending_way = None;
        self.draw_step_cmds.clear();
    }

    /// End the current way (area mode closes it first and ensures `area=yes`; the command history created so far is kept).
    pub fn finish_way(&mut self, editor: &mut Editor) -> Outcome {
        if self.pending_nodes.is_empty() {
            return Outcome::None;
        }
        if self.tool == ToolKind::DrawArea {
            if self.pending_nodes.len() < 3 {
                return Outcome::Message("an area needs at least 3 nodes".into());
            }
            let before = editor.history().undo_len();
            if let Err(err) = self.close_pending_area(editor) {
                return Outcome::Message(err);
            }
            if let Err(err) = self.ensure_area_yes(editor) {
                return Outcome::Message(err);
            }
            self.note_draw_step(editor, before);
        }
        let count = self.pending_nodes.len();
        let finished_way = self.pending_way;
        self.reset_pending();
        if let Some(way) = finished_way {
            editor.selection_mut().select_only(ObjectKind::Way, way);
        }
        self.finished_feature = true;
        Outcome::Message(format!("Finished way ({count} nodes)"))
    }

    /// In drawing mode, undo the previous drawing step; if the previous step was not a drawing, other history is left untouched.
    pub fn undo_last_draw(&mut self, editor: &mut Editor) -> Outcome {
        if !self.tool.is_draw() {
            return Outcome::None;
        }
        if let Some(steps) = self.draw_step_cmds.pop() {
            for _ in 0..steps {
                match editor.undo() {
                    Ok(true) => {}
                    Ok(false) | Err(_) => break,
                }
            }
            self.resync_pending_after_undo(editor);
            return Outcome::Edited("Undo drawing".into());
        }
        // Continuing a draw is loaded without writing history: cancel the draft only.
        if !self.pending_nodes.is_empty() || self.pending_way.is_some() {
            self.reset_pending();
            return Outcome::None;
        }
        Outcome::None
    }

    fn note_draw_step(&mut self, editor: &Editor, before_len: usize) {
        let added = editor.history().undo_len().saturating_sub(before_len);
        if added > 0 {
            self.draw_step_cmds.push(added);
        }
    }

    fn resync_pending_after_undo(&mut self, editor: &mut Editor) {
        if let Some(way) = self.pending_way {
            if let Some(handle) = editor.data().way_handle(way) {
                self.pending_nodes = editor.data().way_node_ids(handle);
                return;
            }
            self.pending_way = None;
        }
        self.pending_nodes
            .retain(|id| editor.data().node_handle(*id).is_some());
        if self.pending_nodes.is_empty() {
            self.draw_step_cmds.clear();
        }
    }

    /// Handle one click.
    pub fn on_click(&mut self, editor: &mut Editor, pick: Pick, pos: LatLon) -> Outcome {
        self.on_click_with_snap(editor, pick, pos, None)
    }

    /// Handle one click, optionally with a snap result (used by drawing tools).
    pub fn on_click_with_snap(
        &mut self,
        editor: &mut Editor,
        pick: Pick,
        pos: LatLon,
        snap: Option<SnapHit>,
    ) -> Outcome {
        match self.tool {
            ToolKind::Select => self.select(editor, pick, false),
            ToolKind::DrawPoint => self.draw_point(editor, pick, pos, snap),
            ToolKind::DrawLine | ToolKind::DrawArea => self.draw_way(editor, pick, pos, snap),
        }
    }

    /// Handle one click (`additive` = Shift/Ctrl append selection, for multi-select).
    pub fn on_click_multi(
        &mut self,
        editor: &mut Editor,
        pick: Pick,
        pos: LatLon,
        additive: bool,
    ) -> Outcome {
        self.on_click_multi_with_snap(editor, pick, pos, additive, None)
    }

    /// Multi-select click + optional snapping.
    pub fn on_click_multi_with_snap(
        &mut self,
        editor: &mut Editor,
        pick: Pick,
        pos: LatLon,
        additive: bool,
        snap: Option<SnapHit>,
    ) -> Outcome {
        if self.tool == ToolKind::Select {
            return self.select(editor, pick, additive);
        }
        self.on_click_with_snap(editor, pick, pos, snap)
    }

    fn select(&mut self, editor: &mut Editor, pick: Pick, additive: bool) -> Outcome {
        if additive {
            if let Some(node) = pick.node {
                editor.selection_mut().insert(ObjectKind::Node, node);
                return Outcome::SelectionChanged;
            }
            if let Some(way) = pick.way {
                editor.selection_mut().insert(ObjectKind::Way, way);
                return Outcome::SelectionChanged;
            }
            if let Some(relation) = pick.relation {
                editor.selection_mut().insert(ObjectKind::Relation, relation);
                return Outcome::SelectionChanged;
            }
            return Outcome::None;
        }
        if let Some(node) = pick.node {
            editor.selection_mut().select_only(ObjectKind::Node, node);
            return Outcome::SelectionChanged;
        }
        if let Some(way) = pick.way {
            editor.selection_mut().select_only(ObjectKind::Way, way);
            return Outcome::SelectionChanged;
        }
        if let Some(relation) = pick.relation {
            editor.selection_mut().select_only(ObjectKind::Relation, relation);
            return Outcome::SelectionChanged;
        }
        editor.selection_mut().clear();
        Outcome::SelectionChanged
    }

    fn draw_point(&mut self, editor: &mut Editor, pick: Pick, pos: LatLon, snap: Option<SnapHit>) -> Outcome {
        self.reset_pending();
        let snap = self.snap_enabled.then_some(snap).flatten();
        if let Some(SnapHit {
            target: SnapTarget::Node { .. },
            ..
        }) = snap
        {
            return Outcome::Message("node already exists".into());
        }
        if pick.node.is_some() && snap.is_none() {
            return Outcome::Message("node already exists".into());
        }
        let before = editor.history().undo_len();
        // Segment snapping: create a node and insert it into the way, making it a shared member.
        if let Some(SnapHit {
            target: SnapTarget::WaySegment { way, seg, at },
            ..
        }) = snap
        {
            return match insert_node_on_segment(editor, way, seg, at) {
                Ok(id) => {
                    self.note_draw_step(editor, before);
                    editor.selection_mut().select_only(ObjectKind::Node, id);
                    self.finished_feature = true;
                    Outcome::Edited(format!("Create node {id} on segment"))
                }
                Err(err) => Outcome::Message(err),
            };
        }
        let id = editor.next_local_id();
        let mut cmd = AddNode::new(pos, Vec::new());
        cmd.id = Some(id);
        match editor.execute(Box::new(cmd)) {
            Ok(()) => {
                self.note_draw_step(editor, before);
                editor.selection_mut().select_only(ObjectKind::Node, id);
                self.finished_feature = true;
                Outcome::Edited(format!("Create node {id}"))
            }
            Err(err) => Outcome::Message(format!("Failed to create node: {err}")),
        }
    }

    /// Insert a node at the midpoint of a way segment and select it; returns the new node ID on success.
    pub fn insert_midpoint_node(
        &mut self,
        editor: &mut Editor,
        way: ObjectId,
        seg: usize,
    ) -> Result<ObjectId, String> {
        let id = insert_way_midpoint(editor, way, seg)?;
        editor.selection_mut().select_only(ObjectKind::Node, id);
        Ok(id)
    }

    fn draw_way(&mut self, editor: &mut Editor, pick: Pick, pos: LatLon, snap: Option<SnapHit>) -> Outcome {
        let snap = self.snap_enabled.then_some(snap).flatten();

        let hit_node = snap
            .as_ref()
            .and_then(|hit| match hit.target {
                SnapTarget::Node { id } => Some(id),
                _ => None,
            })
            .or(pick.node);

        // Line tool with an empty draft: clicking an open way's endpoint -> load for continuation (no status bar message).
        if self.pending_nodes.is_empty()
            && self.tool == ToolKind::DrawLine
            && let Some(node) = hit_node
            && let Some(way) = find_continuable_way(editor, node, pick.way)
        {
            return self.begin_continue_way(editor, way, node);
        }

        // Line: clicking the newest node again -> end drawing (before hit resolution, to avoid creating a duplicate node).
        if let Some(id) = hit_node
            && self.pending_nodes.last() == Some(&id)
        {
            if self.tool == ToolKind::DrawLine && self.pending_nodes.len() >= 2 {
                return self.finish_way(editor);
            }
            return Outcome::None;
        }

        // Area: clicking the start point again closes and ends it.
        if self.tool == ToolKind::DrawArea
            && self.pending_nodes.len() >= 3
            && self.pending_nodes.first() == hit_node.as_ref()
        {
            return self.finish_way(editor);
        }

        let before = editor.history().undo_len();
        let (node_id, created_on_segment) = match resolve_draw_node(editor, pick, pos, snap) {
            Ok(v) => v,
            Err(err) => return Outcome::Message(err),
        };

        if self.pending_nodes.is_empty() {
            self.pending_nodes.push(node_id);
            self.note_draw_step(editor, before);
            let tip = if self.tool == ToolKind::DrawArea {
                "Start point placed; keep clicking to draw the area, then click the start point again or press finish to close it"
            } else {
                "Start point placed; keep clicking to draw the line, then click the latest node again to finish"
            };
            return if created_on_segment || before < editor.history().undo_len() {
                Outcome::Edited(tip.into())
            } else {
                Outcome::Message(tip.into())
            };
        }

        // If hit resolution still lands on the newest node (e.g. only the pick matched), end/ignore likewise.
        if self.pending_nodes.last() == Some(&node_id) {
            if self.tool == ToolKind::DrawLine && self.pending_nodes.len() >= 2 {
                return self.finish_way(editor);
            }
            return Outcome::None;
        }

        // Area: if hit resolution lands on the start point, close it.
        if self.tool == ToolKind::DrawArea
            && self.pending_nodes.len() >= 3
            && self.pending_nodes.first() == Some(&node_id)
        {
            return self.finish_way(editor);
        }

        self.pending_nodes.push(node_id);

        let outcome = if let Some(way) = self.pending_way {
            let nodes = self.pending_nodes.clone();
            match editor.execute(Box::new(SetWayNodes::new(way, nodes))) {
                Ok(()) => Outcome::Edited("Append way node".into()),
                Err(err) => Outcome::Message(format!("Failed to append: {err}")),
            }
        } else if self.pending_nodes.len() >= 2 {
            let way_id = editor.next_local_id();
            let tags = if self.tool == ToolKind::DrawArea {
                vec![("area".to_owned(), "yes".to_owned())]
            } else {
                Vec::new()
            };
            let mut cmd = AddWay::new(self.pending_nodes.clone(), tags);
            cmd.id = Some(way_id);
            match editor.execute(Box::new(cmd)) {
                Ok(()) => {
                    self.pending_way = Some(way_id);
                    Outcome::Edited(format!("Start way {way_id}"))
                }
                Err(err) => Outcome::Message(format!("Failed to create way: {err}")),
            }
        } else {
            Outcome::None
        };
        self.note_draw_step(editor, before);
        outcome
    }

    /// Continue drawing from an existing open way's endpoint.
    fn begin_continue_way(&mut self, editor: &mut Editor, way: ObjectId, endpoint: ObjectId) -> Outcome {
        let Some(handle) = editor.data().way_handle(way) else {
            return Outcome::Message("way does not exist".into());
        };
        let mut nodes = editor.data().way_node_ids(handle);
        if nodes.len() < 2 {
            return Outcome::None;
        }
        if nodes.first() == Some(&endpoint) {
            nodes.reverse();
        } else if nodes.last() != Some(&endpoint) {
            return Outcome::None;
        }
        self.pending_way = Some(way);
        self.pending_nodes = nodes;
        // Canvas overlay feedback is enough; no status bar message.
        Outcome::None
    }

    fn close_pending_area(&mut self, editor: &mut Editor) -> Result<(), String> {
        let Some(first) = self.pending_nodes.first().copied() else {
            return Ok(());
        };
        if self.pending_nodes.last() != Some(&first) {
            self.pending_nodes.push(first);
        }
        let Some(way) = self.pending_way else {
            return Ok(());
        };
        let nodes = self.pending_nodes.clone();
        // First and last must be the same node: otherwise it is still treated as an unclosed way.
        if nodes.len() < 4 || nodes.first() != nodes.last() {
            return Err("the area could not be closed".into());
        }
        editor
            .execute(Box::new(SetWayNodes::new(way, nodes)))
            .map_err(|err| format!("Failed to close: {err}"))
    }

    fn ensure_area_yes(&self, editor: &mut Editor) -> Result<(), String> {
        let Some(way) = self.pending_way else {
            return Ok(());
        };
        let Some(handle) = editor.data().way_handle(way) else {
            return Ok(());
        };
        let Some(view) = editor.data().way_view(handle) else {
            return Ok(());
        };
        let mut tags: Vec<(String, String)> = view
            .tag_pairs
            .iter()
            .map(|(k, v)| {
                (
                    editor.data().resolve(*k).to_owned(),
                    editor.data().resolve(*v).to_owned(),
                )
            })
            .collect();
        if tags.iter().any(|(k, v)| k == "area" && v == "yes") {
            return Ok(());
        }
        tags.retain(|(k, _)| k != "area");
        tags.push(("area".to_owned(), "yes".to_owned()));
        editor
            .execute(Box::new(SetTags::new(ObjectKind::Way, way, tags)))
            .map_err(|err| format!("Failed to write area=yes: {err}"))
    }

    /// Drag end: move the node from `from` to `to` (one drag = one history entry).
    pub fn on_drag_end(&mut self, editor: &mut Editor, node: ObjectId, from: LatLon, to: LatLon) -> Outcome {
        if from == to {
            return Outcome::None;
        }
        match editor.execute(Box::new(MoveNode::new(node, from, to))) {
            Ok(()) => Outcome::Edited("Move node".into()),
            Err(err) => Outcome::Message(format!("Failed to move: {err}")),
        }
    }

    /// Merge two endpoint-adjacent ways (direction handled automatically).
    pub fn merge_ways(&mut self, editor: &mut Editor, a: ObjectId, b: ObjectId) -> Outcome {
        match editor.execute(Box::new(neo_editor::MergeWay::new(a, b))) {
            Ok(()) => Outcome::Edited("Merge ways".into()),
            Err(err) => Outcome::Message(format!("Failed to merge: {err}")),
        }
    }

    /// Merge the selected end-to-end connected ways (possibly several; each chain becomes one way).
    ///
    /// Only open ways are considered; selected ways are linked into chains by shared endpoints, and groups that branch, loop, or are disconnected are skipped.
    /// All chains' merge commands are submitted in one `execute_batch`; a single undo reverts the whole batch.
    pub fn merge_chain_ways(&mut self, editor: &mut Editor, ways: &[ObjectId]) -> Outcome {
        let mut node_seqs: Vec<(ObjectId, Vec<ObjectId>)> = Vec::with_capacity(ways.len());
        for way in ways {
            let Some(handle) = editor.data().way_handle(*way) else {
                continue;
            };
            // Logically deleted ways no longer take part in editing.
            let Some(view) = editor.data().way_view(handle) else {
                continue;
            };
            if !view.info.meta.visible {
                continue;
            }
            let Ok(nodes) = editor.data().try_way_node_ids(handle) else {
                continue;
            };
            node_seqs.push((*way, nodes));
        }
        let chains = chain_merge_order(&node_seqs);
        if chains.is_empty() {
            return Outcome::Message(
                "Selected ways cannot be merged: need at least two end-to-end open ways".into(),
            );
        }
        let mut commands: Vec<Box<dyn neo_editor::Command>> = Vec::new();
        for chain in &chains {
            // Merge each chain serially into its head: subsequent commands must use the still-alive chain head as `a`,
            // otherwise they would merge a middle way already logically deleted by the previous command.
            let Some((&survivor, rest)) = chain.split_first() else {
                continue;
            };
            for way in rest {
                commands.push(Box::new(neo_editor::MergeWay::new(survivor, *way)));
            }
        }
        match editor.execute_batch(commands) {
            Ok(()) => Outcome::Edited("Merge ways".into()),
            Err(err) => Outcome::Message(format!("Failed to merge: {err}")),
        }
    }

    /// Batch-move nodes (dragging selected nodes/ways = one history entry).
    pub fn move_nodes_batch(&mut self, editor: &mut Editor, moves: &[(ObjectId, LatLon, LatLon)]) -> Outcome {
        let moves: Vec<_> = moves.iter().copied().filter(|(_, from, to)| from != to).collect();
        if moves.is_empty() {
            return Outcome::None;
        }
        let commands: Vec<Box<dyn neo_editor::Command>> = moves
            .iter()
            .map(|(node, from, to)| {
                Box::new(MoveNode::new(*node, *from, *to)) as Box<dyn neo_editor::Command>
            })
            .collect();
        match editor.execute_batch(commands) {
            Ok(()) => Outcome::Edited(format!("Moved {} nodes", moves.len())),
            Err(err) => Outcome::Message(format!("Failed to move: {err}")),
        }
    }

    /// Reverse the selected way.
    pub fn reverse_way(&mut self, editor: &mut Editor, way: ObjectId) -> Outcome {
        match editor.execute(Box::new(neo_editor::ReverseWay::new(way))) {
            Ok(()) => Outcome::Edited("Reverse way".into()),
            Err(err) => Outcome::Message(format!("Failed to reverse: {err}")),
        }
    }

    /// Split the way at the selected node; falls back to midpoint splitting when no node is selected.
    pub fn split_way(&mut self, editor: &mut Editor, way: ObjectId, at_node: Option<ObjectId>) -> Outcome {
        let Some(handle) = editor.data().way_handle(way) else {
            return Outcome::Message("way does not exist".into());
        };
        let nodes = editor.data().way_node_ids(handle);
        let len = nodes.len();
        if len < 3 {
            return Outcome::Message("Way too short to split".into());
        }
        // Only interior nodes can be split points (splitting at the ends would yield single-point ways).
        let at = at_node
            .and_then(|node| nodes.iter().position(|id| *id == node))
            .filter(|index| *index >= 1 && *index + 1 < len)
            .unwrap_or(len / 2);
        match editor.execute(Box::new(neo_editor::SplitWay::new(way, at as u32))) {
            Ok(()) => Outcome::Edited("Split way".into()),
            Err(err) => Outcome::Message(format!("Failed to split: {err}")),
        }
    }

    /// Split every way passing through a node (only interior nodes are split).
    ///
    /// For every visible way referencing the node, one `SplitWay` command is generated; all commands are committed atomically
    /// in a single `execute_batch` (one undo reverts everything). If the node does not exist or is only an endpoint
    /// (no interior references), a hint message is returned.
    pub fn split_ways_at_node(&mut self, editor: &mut Editor, node: ObjectId) -> Outcome {
        let Some(node_handle) = editor.data().node_handle(node) else {
            return Outcome::Message("node does not exist".into());
        };
        // The referrer table is read-only; rebuild it from current data first to avoid using references from before the last edit.
        editor.refresh_referrers();
        let way_handles: Vec<_> = editor.referrers().ways_of_node(node_handle).to_vec();
        let mut commands: Vec<Box<dyn neo_editor::Command>> = Vec::new();
        for way_handle in way_handles {
            let Some(view) = editor.data().way_view(way_handle) else {
                continue;
            };
            let way = view.id();
            let Ok(nodes) = editor.data().try_way_node_ids(way_handle) else {
                continue;
            };
            let Some(index) = nodes.iter().position(|id| *id == node) else {
                continue;
            };
            let len = nodes.len();
            // Only interior nodes can be split points: splitting at the first/last node would yield a single-point way.
            if index >= 1 && index + 1 < len {
                commands.push(Box::new(neo_editor::SplitWay::new(way, index as u32)));
            }
        }
        if commands.is_empty() {
            return Outcome::Message("this point is not an interior node of a way".into());
        }
        match editor.execute_batch(commands) {
            Ok(()) => Outcome::Edited("Split way".into()),
            Err(err) => Outcome::Message(format!("Failed to split: {err}")),
        }
    }
}

/// Compute the chain merge order.
///
/// Input is a list of `(way, node sequence)` (usually from the user selection); returns each mergeable chain's way order:
/// ways in a chain are end-to-end, and executing `MergeWay::new(chain[0], chain[i])` in order merges the whole chain
/// into one way (`MergeWay` handles all four directions automatically and keeps `chain[0]`'s ID).
///
/// Rules:
/// * Only open ways are considered (node count >= 2 and first node != last node); closed and duplicate ways are ignored;
/// * Ways are linked into connected components by shared endpoints, and each component must be a **chain**: at most two ways meet at an endpoint and
///   no cycle is allowed (branching or looping components are skipped entirely);
/// * Only components with 2 or more ways count as mergeable chains; isolated ways are not returned;
/// * The chain starts from the smaller degree-1 endpoint node, keeping the order stable.
pub fn chain_merge_order(node_seqs: &[(ObjectId, Vec<ObjectId>)]) -> Vec<Vec<ObjectId>> {
    // Filter out ways that cannot take part in merging and deduplicate in input order.
    let mut ways: Vec<(ObjectId, ObjectId, ObjectId)> = Vec::with_capacity(node_seqs.len());
    let mut seen: Vec<ObjectId> = Vec::with_capacity(node_seqs.len());
    for (way, nodes) in node_seqs {
        if seen.contains(way) {
            continue;
        }
        seen.push(*way);
        let Some((first, last)) = nodes.first().zip(nodes.last()) else {
            continue;
        };
        if nodes.len() < 2 || first == last {
            continue;
        }
        ways.push((*way, *first, *last));
    }
    if ways.len() < 2 {
        return Vec::new();
    }

    // Endpoint node -> indices of ways referencing it: sharing an endpoint means adjacency.
    let mut at_node: BTreeMap<ObjectId, Vec<usize>> = BTreeMap::new();
    for (index, (_, first, last)) in ways.iter().enumerate() {
        at_node.entry(*first).or_default().push(index);
        at_node.entry(*last).or_default().push(index);
    }
    let mut adjacency: Vec<Vec<usize>> = vec![Vec::new(); ways.len()];
    for members in at_node.values() {
        for &i in members {
            for &j in members {
                if i != j && !adjacency[i].contains(&j) {
                    adjacency[i].push(j);
                }
            }
        }
    }

    // Find connected components (by way adjacency).
    let mut component: Vec<Option<usize>> = vec![None; ways.len()];
    let mut components: Vec<Vec<usize>> = Vec::new();
    let mut unvisited: Vec<usize> = (0..ways.len()).rev().collect();
    while let Some(start) = unvisited.pop() {
        if component[start].is_some() {
            continue;
        }
        let id = components.len();
        let mut members = Vec::new();
        let mut stack = vec![start];
        component[start] = Some(id);
        while let Some(index) = stack.pop() {
            members.push(index);
            for &next in &adjacency[index] {
                if component[next].is_none() {
                    component[next] = Some(id);
                    stack.push(next);
                }
            }
        }
        components.push(members);
    }

    // Validate and order each component.
    let mut chains = Vec::new();
    for members in components {
        if members.len() < 2 {
            continue;
        }
        // Endpoint node degree: > 2 means branching, skip the whole component.
        let mut degree: BTreeMap<ObjectId, usize> = BTreeMap::new();
        for &index in &members {
            let (_, first, last) = ways[index];
            *degree.entry(first).or_insert(0) += 1;
            *degree.entry(last).or_insert(0) += 1;
        }
        if degree.values().any(|degree| *degree > 2) {
            continue;
        }
        // A chain (path graph) satisfies edges + 1 = vertices; otherwise there is a cycle or a duplicate edge.
        if members.len() + 1 != degree.len() {
            continue;
        }
        // Start from the smallest degree-1 endpoint and walk the whole chain along unvisited ways.
        let Some(&start) = degree
            .iter()
            .find_map(|(node, degree)| (*degree == 1).then_some(node))
        else {
            continue;
        };
        let mut order = Vec::with_capacity(members.len());
        let mut current = start;
        let mut previous: Option<usize> = None;
        while order.len() < members.len() {
            let Some(candidates) = at_node.get(&current) else {
                break;
            };
            let next: Vec<usize> = candidates
                .iter()
                .copied()
                .filter(|index| Some(*index) != previous)
                .collect();
            let [index] = next.as_slice() else {
                break;
            };
            order.push(ways[*index].0);
            let (_, first, last) = ways[*index];
            current = if first == current { last } else { first };
            previous = Some(*index);
        }
        if order.len() == members.len() {
            chains.push(order);
        }
    }
    chains
}

/// Resolve a drawing target: reuse node / insert on segment / create new. Returns `(node_id, created_on_segment)`.
fn resolve_draw_node(
    editor: &mut Editor,
    pick: Pick,
    pos: LatLon,
    snap: Option<SnapHit>,
) -> Result<(ObjectId, bool), String> {
    match snap.map(|hit| hit.target) {
        Some(SnapTarget::Node { id }) => Ok((id, false)),
        Some(SnapTarget::WaySegment { way, seg, at }) => {
            let id = insert_node_on_segment(editor, way, seg, at)?;
            Ok((id, true))
        }
        None => {
            if let Some(node) = pick.node {
                return Ok((node, false));
            }
            let id = editor.next_local_id();
            let mut cmd = AddNode::new(pos, Vec::new());
            cmd.id = Some(id);
            editor
                .execute(Box::new(cmd))
                .map_err(|err| format!("Failed to create node: {err}"))?;
            Ok((id, false))
        }
    }
}

/// Create a node at the projection onto a way segment and insert it as a member. Returns the new node ID.
pub fn insert_node_on_segment(
    editor: &mut Editor,
    way: ObjectId,
    seg: usize,
    at: LatLon,
) -> Result<ObjectId, String> {
    let id = editor.next_local_id();
    let mut add = AddNode::new(at, Vec::new());
    add.id = Some(id);
    editor
        .execute(Box::new(add))
        .map_err(|err| format!("Failed to create node: {err}"))?;
    insert_node_on_way(editor, way, seg, id)?;
    Ok(id)
}

/// Create and insert a node at the geometric midpoint of the way's `seg` segment.
pub fn insert_way_midpoint(editor: &mut Editor, way: ObjectId, seg: usize) -> Result<ObjectId, String> {
    let Some(handle) = editor.data().way_handle(way) else {
        return Err("way does not exist".into());
    };
    let nodes = editor.data().way_node_ids(handle);
    let (a, b) = nodes
        .get(seg)
        .copied()
        .zip(nodes.get(seg + 1).copied())
        .ok_or_else(|| "invalid segment index".to_owned())?;
    let pa = editor
        .data()
        .node_handle(a)
        .and_then(|h| editor.data().nodes.position(h))
        .ok_or_else(|| "segment endpoint missing".to_owned())?;
    let pb = editor
        .data()
        .node_handle(b)
        .and_then(|h| editor.data().nodes.position(h))
        .ok_or_else(|| "segment endpoint missing".to_owned())?;
    let at = segment_midpoint_latlon(pa, pb);
    insert_node_on_segment(editor, way, seg, at)
}

/// Geometric midpoint of a segment (world-coordinate midpoint converted back to lat/lon).
pub fn segment_midpoint_latlon(a: LatLon, b: LatLon) -> LatLon {
    let wa = neo_geo::WorldPoint::from_latlon(a);
    let wb = neo_geo::WorldPoint::from_latlon(b);
    neo_geo::WorldPoint::new((wa.x + wb.x) * 0.5, (wa.y + wb.y) * 0.5).to_latlon()
}

/// Insert an existing node after the way's `seg` segment (shared node).
fn insert_node_on_way(editor: &mut Editor, way: ObjectId, seg: usize, node: ObjectId) -> Result<(), String> {
    let Some(handle) = editor.data().way_handle(way) else {
        return Err("way does not exist".into());
    };
    let mut nodes = editor.data().way_node_ids(handle);
    if nodes.contains(&node) {
        return Ok(());
    }
    let insert_at = seg + 1;
    if insert_at > nodes.len() {
        return Err("invalid segment index".into());
    }
    nodes.insert(insert_at, node);
    editor
        .execute(Box::new(SetWayNodes::new(way, nodes)))
        .map_err(|err| format!("Failed to insert node: {err}"))
}

/// Find an open way that can be continued from this endpoint.
fn find_continuable_way(
    editor: &mut Editor,
    endpoint: ObjectId,
    preferred_way: Option<ObjectId>,
) -> Option<ObjectId> {
    // Commands such as AddWay mark the referrer table dirty, so refresh before querying reverse references.
    editor.refresh_referrers();
    let handle = editor.data().node_handle(endpoint)?;
    let parent_count = editor.referrers().ways_of_node(handle).len();
    let mut candidates: Vec<ObjectId> = editor
        .referrers()
        .ways_of_node(handle)
        .iter()
        .filter_map(|way_handle| {
            let way_id = editor.data().ways.id(*way_handle)?;
            let nodes = editor.data().way_node_ids(*way_handle);
            if nodes.len() < 2 {
                return None;
            }
            let first = *nodes.first()?;
            let last = *nodes.last()?;
            if first == last {
                return None; // closed
            }
            if first == endpoint || last == endpoint {
                Some(way_id)
            } else {
                None
            }
        })
        .collect();
    if candidates.is_empty() {
        return None;
    }
    // When the endpoint is attached to only one way, continue that way directly.
    if parent_count == 1 {
        return candidates.pop();
    }
    if let Some(preferred) = preferred_way
        && candidates.contains(&preferred)
    {
        return Some(preferred);
    }
    if candidates.len() == 1 {
        return candidates.pop();
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use neo_store::DataSet;

    fn add_node(editor: &mut Editor, lat: f64, lon: f64) -> ObjectId {
        let id = editor.next_local_id();
        let mut cmd = AddNode::new(LatLon::from_degrees(lat, lon), Vec::new());
        cmd.id = Some(id);
        editor.execute(Box::new(cmd)).unwrap();
        id
    }

    fn add_way(editor: &mut Editor, nodes: Vec<ObjectId>) -> ObjectId {
        let id = editor.next_local_id();
        let mut cmd = AddWay::new(nodes, Vec::new());
        cmd.id = Some(id);
        editor.execute(Box::new(cmd)).unwrap();
        id
    }

    fn way_nodes(editor: &Editor, way: ObjectId) -> Vec<ObjectId> {
        let handle = editor.data().way_handle(way).expect("way exists");
        editor.data().way_node_ids(handle)
    }

    fn visible_way_ids(editor: &Editor) -> Vec<ObjectId> {
        editor
            .data()
            .iter_ways()
            .filter(|way| way.meta.visible)
            .map(|way| way.id)
            .collect()
    }

    fn ids(values: &[i64]) -> Vec<ObjectId> {
        values.iter().copied().map(ObjectId).collect()
    }

    #[test]
    fn continue_from_tail_keeps_order() {
        let mut editor = Editor::new(DataSet::new());
        let a = add_node(&mut editor, 0.0, 0.0);
        let b = add_node(&mut editor, 0.0, 0.001);
        let c = add_node(&mut editor, 0.0, 0.002);
        let way = add_way(&mut editor, vec![a, b, c]);
        let mut ctl = EditController::new(ToolKind::DrawLine);
        let outcome = ctl.begin_continue_way(&mut editor, way, c);
        assert_eq!(outcome, Outcome::None);
        assert_eq!(ctl.pending_way(), Some(way));
        assert_eq!(ctl.pending_nodes(), &[a, b, c]);
    }

    #[test]
    fn continue_from_head_reverses_pending() {
        let mut editor = Editor::new(DataSet::new());
        let a = add_node(&mut editor, 0.0, 0.0);
        let b = add_node(&mut editor, 0.0, 0.001);
        let c = add_node(&mut editor, 0.0, 0.002);
        let way = add_way(&mut editor, vec![a, b, c]);
        let mut ctl = EditController::new(ToolKind::DrawLine);
        ctl.begin_continue_way(&mut editor, way, a);
        assert_eq!(ctl.pending_nodes(), &[c, b, a]);
    }

    #[test]
    fn insert_node_on_segment_updates_members() {
        let mut editor = Editor::new(DataSet::new());
        let a = add_node(&mut editor, 0.0, 0.0);
        let b = add_node(&mut editor, 0.0, 0.002);
        let way = add_way(&mut editor, vec![a, b]);
        let mid = add_node(&mut editor, 0.0, 0.001);
        insert_node_on_way(&mut editor, way, 0, mid).unwrap();
        let handle = editor.data().way_handle(way).unwrap();
        assert_eq!(editor.data().way_node_ids(handle), vec![a, mid, b]);
    }

    #[test]
    fn draw_way_with_segment_snap_inserts_shared_node() {
        let mut editor = Editor::new(DataSet::new());
        let a = add_node(&mut editor, 0.0, 0.0);
        let b = add_node(&mut editor, 0.0, 0.002);
        let way = add_way(&mut editor, vec![a, b]);
        let mut ctl = EditController::new(ToolKind::DrawLine);
        let at = LatLon::from_degrees(0.0, 0.001);
        let snap = SnapHit {
            target: SnapTarget::WaySegment { way, seg: 0, at },
            screen_dist: 2.0,
        };
        // Place a free start point first
        let start = LatLon::from_degrees(0.001, 0.0);
        ctl.on_click_with_snap(&mut editor, Pick::default(), start, None);
        let outcome = ctl.on_click_with_snap(&mut editor, Pick::default(), at, Some(snap));
        assert!(outcome.is_edit());
        let handle = editor.data().way_handle(way).unwrap();
        let nodes = editor.data().way_node_ids(handle);
        assert_eq!(nodes.len(), 3);
        assert_eq!(nodes[0], a);
        assert_eq!(nodes[2], b);
        assert_eq!(ctl.pending_nodes().len(), 2);
        assert_eq!(ctl.pending_nodes()[1], nodes[1]);
    }

    #[test]
    fn draw_point_segment_snap_inserts_into_way() {
        let mut editor = Editor::new(DataSet::new());
        let a = add_node(&mut editor, 0.0, 0.0);
        let b = add_node(&mut editor, 0.0, 0.002);
        let way = add_way(&mut editor, vec![a, b]);
        let mut ctl = EditController::new(ToolKind::DrawPoint);
        let at = LatLon::from_degrees(0.0, 0.001);
        let snap = SnapHit {
            target: SnapTarget::WaySegment { way, seg: 0, at },
            screen_dist: 2.0,
        };
        let outcome = ctl.on_click_with_snap(&mut editor, Pick::default(), at, Some(snap));
        assert!(outcome.is_edit());
        let handle = editor.data().way_handle(way).unwrap();
        let nodes = editor.data().way_node_ids(handle);
        assert_eq!(nodes.len(), 3);
        assert_eq!(nodes[0], a);
        assert_eq!(nodes[2], b);
        assert!(editor.selection().contains(ObjectKind::Node, nodes[1]));
    }

    #[test]
    fn insert_midpoint_splits_segment_and_selects() {
        let mut editor = Editor::new(DataSet::new());
        let a = add_node(&mut editor, 0.0, 0.0);
        let b = add_node(&mut editor, 0.0, 0.002);
        let way = add_way(&mut editor, vec![a, b]);
        let mut ctl = EditController::new(ToolKind::Select);
        let mid = ctl.insert_midpoint_node(&mut editor, way, 0).unwrap();
        let handle = editor.data().way_handle(way).unwrap();
        let nodes = editor.data().way_node_ids(handle);
        assert_eq!(nodes, vec![a, mid, b]);
        let pos = editor
            .data()
            .node_handle(mid)
            .and_then(|h| editor.data().nodes.position(h))
            .unwrap();
        let expected =
            segment_midpoint_latlon(LatLon::from_degrees(0.0, 0.0), LatLon::from_degrees(0.0, 0.002));
        assert!((pos.lat.to_degrees() - expected.lat.to_degrees()).abs() < 1e-9);
        assert!((pos.lon.to_degrees() - expected.lon.to_degrees()).abs() < 1e-9);
        assert!(editor.selection().contains(ObjectKind::Node, mid));
    }

    #[test]
    fn undo_last_draw_rolls_back_one_step_only() {
        let mut editor = Editor::new(DataSet::new());
        let mut ctl = EditController::new(ToolKind::DrawLine);
        let a = LatLon::from_degrees(0.0, 0.0);
        let b = LatLon::from_degrees(0.0, 0.001);
        ctl.on_click_with_snap(&mut editor, Pick::default(), a, None);
        ctl.on_click_with_snap(&mut editor, Pick::default(), b, None);
        assert_eq!(ctl.pending_len(), 2);
        assert!(ctl.pending_way().is_some());
        assert!(ctl.undo_last_draw(&mut editor).is_edit());
        assert_eq!(ctl.pending_len(), 1);
        assert!(ctl.pending_way().is_none());
        assert!(matches!(ctl.undo_last_draw(&mut editor), Outcome::Edited(_)));
        assert_eq!(ctl.pending_len(), 0);
        // Right-click again: no drawing step, leave other history untouched
        let undo_len = editor.history().undo_len();
        assert_eq!(ctl.undo_last_draw(&mut editor), Outcome::None);
        assert_eq!(editor.history().undo_len(), undo_len);
    }

    #[test]
    fn empty_draft_click_endpoint_continues_line() {
        let mut editor = Editor::new(DataSet::new());
        let a = add_node(&mut editor, 0.0, 0.0);
        let b = add_node(&mut editor, 0.0, 0.001);
        let way = add_way(&mut editor, vec![a, b]);
        let mut ctl = EditController::new(ToolKind::DrawLine);
        let pick = Pick {
            node: Some(b),
            way: Some(way),
            relation: None,
        };
        let snap = SnapHit {
            target: SnapTarget::Node { id: b },
            screen_dist: 1.0,
        };
        let outcome = ctl.on_click_with_snap(&mut editor, pick, LatLon::from_degrees(0.0, 0.001), Some(snap));
        assert_eq!(outcome, Outcome::None);
        assert_eq!(ctl.pending_way(), Some(way));
        assert_eq!(ctl.pending_nodes(), &[a, b]);
    }

    #[test]
    fn click_latest_node_finishes_line_without_duplicate() {
        let mut editor = Editor::new(DataSet::new());
        let mut ctl = EditController::new(ToolKind::DrawLine);
        let p0 = LatLon::from_degrees(0.0, 0.0);
        let p1 = LatLon::from_degrees(0.0, 0.001);
        ctl.on_click_with_snap(&mut editor, Pick::default(), p0, None);
        ctl.on_click_with_snap(&mut editor, Pick::default(), p1, None);
        assert_eq!(ctl.pending_len(), 2);
        let way = ctl.pending_way().expect("way started");
        let last = *ctl.pending_nodes().last().unwrap();
        let nodes_before = editor.data().node_count();
        let snap = SnapHit {
            target: SnapTarget::Node { id: last },
            screen_dist: 0.5,
        };
        let outcome = ctl.on_click_with_snap(
            &mut editor,
            Pick {
                node: Some(last),
                way: Some(way),
                relation: None,
            },
            p1,
            Some(snap),
        );
        assert!(matches!(outcome, Outcome::Message(_)));
        assert_eq!(ctl.pending_len(), 0);
        assert!(ctl.pending_way().is_none());
        assert!(ctl.take_finished_feature());
        assert_eq!(editor.data().node_count(), nodes_before);
        assert!(editor.selection().contains(ObjectKind::Way, way));
    }

    #[test]
    fn chain_merge_order_orders_three_ways() {
        let chain = chain_merge_order(&[
            (ObjectId(10), ids(&[1, 2, 3])),
            (ObjectId(11), ids(&[3, 4, 5])),
            (ObjectId(12), ids(&[5, 6, 7])),
        ]);
        assert_eq!(chain, vec![vec![ObjectId(10), ObjectId(11), ObjectId(12)]]);
    }

    #[test]
    fn chain_merge_order_handles_reversed_and_duplicate_endpoints() {
        // B joins in reverse (B first = 4, B last = A last 3); still the same chain.
        let reversed = chain_merge_order(&[(ObjectId(10), ids(&[1, 2, 3])), (ObjectId(11), ids(&[4, 3]))]);
        assert_eq!(reversed, vec![vec![ObjectId(10), ObjectId(11)]]);

        // B first = A first (both start from node 1) can still form a chain; duplicate ways count once.
        let head_to_head = chain_merge_order(&[
            (ObjectId(10), ids(&[1, 2, 3])),
            (ObjectId(11), ids(&[1, 4])),
            (ObjectId(11), ids(&[1, 4])),
        ]);
        assert_eq!(head_to_head, vec![vec![ObjectId(10), ObjectId(11)]]);
    }

    #[test]
    fn chain_merge_order_returns_separate_chains() {
        let chains = chain_merge_order(&[
            (ObjectId(10), ids(&[1, 2])),
            (ObjectId(11), ids(&[2, 3])),
            (ObjectId(12), ids(&[4, 5])),
            (ObjectId(13), ids(&[5, 6])),
        ]);
        assert_eq!(
            chains,
            vec![vec![ObjectId(10), ObjectId(11)], vec![ObjectId(12), ObjectId(13)],]
        );
    }

    #[test]
    fn chain_merge_order_skips_cycle_branch_and_closed_ways() {
        // Cycle: 1-2-3-1, edges = vertices, not a chain.
        assert!(
            chain_merge_order(&[
                (ObjectId(10), ids(&[1, 2])),
                (ObjectId(11), ids(&[2, 3])),
                (ObjectId(12), ids(&[3, 1])),
            ])
            .is_empty()
        );
        // Branch: node 2 connects three ways.
        assert!(
            chain_merge_order(&[
                (ObjectId(10), ids(&[1, 2])),
                (ObjectId(11), ids(&[2, 3])),
                (ObjectId(12), ids(&[2, 4])),
            ])
            .is_empty()
        );
        // Closed ways and mutually disconnected ways do not take part in merging.
        assert!(
            chain_merge_order(&[
                (ObjectId(10), ids(&[1, 2, 1])),
                (ObjectId(11), ids(&[1, 2])),
                (ObjectId(12), ids(&[3, 4])),
            ])
            .is_empty()
        );
    }

    #[test]
    fn merge_chain_ways_merges_three_ways_in_one_undo() {
        let mut editor = Editor::new(DataSet::new());
        let nodes: Vec<ObjectId> = (0..7)
            .map(|i| add_node(&mut editor, 0.0, i as f64 * 0.001))
            .collect();
        let first = add_way(&mut editor, nodes[0..3].to_vec());
        let middle = add_way(&mut editor, nodes[2..5].to_vec());
        let last = add_way(&mut editor, nodes[4..7].to_vec());
        let mut ctl = EditController::new(ToolKind::Select);
        let undo_before = editor.history().undo_len();

        let outcome = ctl.merge_chain_ways(&mut editor, &[last, first, middle]);
        assert_eq!(outcome, Outcome::Edited("Merge ways".into()));
        assert_eq!(editor.history().undo_len(), undo_before + 1);
        // The chain is merged starting from an endpoint, so the surviving way ID depends on the starting direction; the node count must equal the total after deduplication.
        let visible = visible_way_ids(&editor);
        assert_eq!(visible.len(), 1);
        let merged = way_nodes(&editor, visible[0]);
        assert_eq!(merged.len(), nodes.len());
        let mut reversed = nodes.clone();
        reversed.reverse();
        assert!(
            merged == nodes || merged == reversed,
            "merge order must be forward or reversed"
        );

        // One undo reverts the whole merge batch.
        assert!(editor.undo().unwrap());
        assert_eq!(way_nodes(&editor, first), nodes[0..3].to_vec());
        assert_eq!(way_nodes(&editor, middle), nodes[2..5].to_vec());
        assert_eq!(way_nodes(&editor, last), nodes[4..7].to_vec());
        assert_eq!(visible_way_ids(&editor).len(), 3);
    }

    #[test]
    fn merge_chain_ways_handles_reversed_direction() {
        let mut editor = Editor::new(DataSet::new());
        let a = add_node(&mut editor, 47.0, 8.0);
        let b = add_node(&mut editor, 47.0, 8.001);
        let c = add_node(&mut editor, 47.0, 8.002);
        let d = add_node(&mut editor, 47.0, 8.003);
        // A = 1->2->3; B = 1->4 (B first = A first); merging must automatically flip B and attach it before A.
        let way_a = add_way(&mut editor, vec![a, b, c]);
        let way_b = add_way(&mut editor, vec![a, d]);
        let mut ctl = EditController::new(ToolKind::Select);

        let outcome = ctl.merge_chain_ways(&mut editor, &[way_a, way_b]);
        assert_eq!(outcome, Outcome::Edited("Merge ways".into()));
        let visible = visible_way_ids(&editor);
        assert_eq!(visible.len(), 1);
        let merged = way_nodes(&editor, visible[0]);
        assert_eq!(merged.len(), 4);
        // After automatic direction handling the nodes must be end-to-end (forward or reverse both fine).
        assert!(
            merged == vec![c, b, a, d] || merged == vec![d, a, b, c],
            "a way attached in reverse must be flipped automatically before merging: {merged:?}"
        );
    }

    #[test]
    fn merge_chain_ways_rejects_non_chain_selection() {
        // Mutually disconnected: no command is produced.
        let mut editor = Editor::new(DataSet::new());
        let a = add_node(&mut editor, 0.0, 0.0);
        let b = add_node(&mut editor, 0.0, 0.001);
        let c = add_node(&mut editor, 0.0, 0.002);
        let d = add_node(&mut editor, 0.0, 0.003);
        let way_a = add_way(&mut editor, vec![a, b]);
        let way_b = add_way(&mut editor, vec![c, d]);
        let mut ctl = EditController::new(ToolKind::Select);
        let undo_before = editor.history().undo_len();
        assert!(matches!(
            ctl.merge_chain_ways(&mut editor, &[way_a, way_b]),
            Outcome::Message(_)
        ));
        assert_eq!(editor.history().undo_len(), undo_before);
        assert_eq!(visible_way_ids(&editor).len(), 2);

        // Cycle: skip the whole component.
        let mut editor = Editor::new(DataSet::new());
        let p1 = add_node(&mut editor, 0.0, 0.0);
        let p2 = add_node(&mut editor, 0.0, 0.001);
        let p3 = add_node(&mut editor, 0.0, 0.002);
        let w1 = add_way(&mut editor, vec![p1, p2]);
        let w2 = add_way(&mut editor, vec![p2, p3]);
        let w3 = add_way(&mut editor, vec![p3, p1]);
        let mut ctl = EditController::new(ToolKind::Select);
        assert!(matches!(
            ctl.merge_chain_ways(&mut editor, &[w1, w2, w3]),
            Outcome::Message(_)
        ));
        assert_eq!(visible_way_ids(&editor).len(), 3);

        // Branch: three ways meet at node p2.
        let mut editor = Editor::new(DataSet::new());
        let q1 = add_node(&mut editor, 0.0, 0.0);
        let q2 = add_node(&mut editor, 0.0, 0.001);
        let q3 = add_node(&mut editor, 0.0, 0.002);
        let q4 = add_node(&mut editor, 0.0, 0.003);
        let x1 = add_way(&mut editor, vec![q1, q2]);
        let x2 = add_way(&mut editor, vec![q2, q3]);
        let x3 = add_way(&mut editor, vec![q2, q4]);
        let mut ctl = EditController::new(ToolKind::Select);
        assert!(matches!(
            ctl.merge_chain_ways(&mut editor, &[x1, x2, x3]),
            Outcome::Message(_)
        ));
        assert_eq!(visible_way_ids(&editor).len(), 3);
    }

    #[test]
    fn split_ways_at_node_splits_internal_node() {
        let mut editor = Editor::new(DataSet::new());
        let nodes: Vec<ObjectId> = (0..4)
            .map(|i| add_node(&mut editor, 0.0, i as f64 * 0.001))
            .collect();
        let way = add_way(&mut editor, nodes.clone());
        let mut ctl = EditController::new(ToolKind::Select);
        let undo_before = editor.history().undo_len();

        let outcome = ctl.split_ways_at_node(&mut editor, nodes[1]);
        assert_eq!(outcome, Outcome::Edited("Split way".into()));
        assert_eq!(editor.history().undo_len(), undo_before + 1);
        // The original way keeps the first half; the new way copies the second half.
        assert_eq!(way_nodes(&editor, way), nodes[0..2].to_vec());
        let visible = visible_way_ids(&editor);
        assert_eq!(visible.len(), 2);
        let new_way = *visible.iter().find(|id| **id != way).unwrap();
        assert_eq!(way_nodes(&editor, new_way), nodes[1..4].to_vec());

        // One undo restores the original way and removes the new way.
        assert!(editor.undo().unwrap());
        assert_eq!(visible_way_ids(&editor), vec![way]);
        assert_eq!(way_nodes(&editor, way), nodes);
    }

    #[test]
    fn split_ways_at_node_ignores_endpoints_and_missing_nodes() {
        let mut editor = Editor::new(DataSet::new());
        let nodes: Vec<ObjectId> = (0..3)
            .map(|i| add_node(&mut editor, 0.0, i as f64 * 0.001))
            .collect();
        let way = add_way(&mut editor, nodes.clone());
        let mut ctl = EditController::new(ToolKind::Select);
        let undo_before = editor.history().undo_len();

        assert!(matches!(
            ctl.split_ways_at_node(&mut editor, nodes[0]),
            Outcome::Message(_)
        ));
        assert!(matches!(
            ctl.split_ways_at_node(&mut editor, nodes[2]),
            Outcome::Message(_)
        ));
        assert!(matches!(
            ctl.split_ways_at_node(&mut editor, ObjectId(999)),
            Outcome::Message(_)
        ));
        assert_eq!(editor.history().undo_len(), undo_before);
        assert_eq!(visible_way_ids(&editor), vec![way]);
        assert_eq!(way_nodes(&editor, way), nodes);
    }

    #[test]
    fn split_ways_at_node_splits_all_ways_in_one_batch() {
        let mut editor = Editor::new(DataSet::new());
        let a = add_node(&mut editor, 0.0, 0.0);
        let shared = add_node(&mut editor, 0.0, 0.001);
        let c = add_node(&mut editor, 0.0, 0.002);
        let d = add_node(&mut editor, 0.0, 0.003);
        let e = add_node(&mut editor, 0.0, 0.004);
        let way_a = add_way(&mut editor, vec![a, shared, c]);
        let way_b = add_way(&mut editor, vec![d, shared, e]);
        let mut ctl = EditController::new(ToolKind::Select);
        let undo_before = editor.history().undo_len();

        let outcome = ctl.split_ways_at_node(&mut editor, shared);
        assert_eq!(outcome, Outcome::Edited("Split way".into()));
        // Both ways are split; one batch takes only one history entry.
        assert_eq!(editor.history().undo_len(), undo_before + 1);
        assert_eq!(visible_way_ids(&editor).len(), 4);
        assert_eq!(way_nodes(&editor, way_a), vec![a, shared]);
        assert_eq!(way_nodes(&editor, way_b), vec![d, shared]);

        assert!(editor.undo().unwrap());
        assert_eq!(visible_way_ids(&editor).len(), 2);
        assert_eq!(way_nodes(&editor, way_a), vec![a, shared, c]);
        assert_eq!(way_nodes(&editor, way_b), vec![d, shared, e]);
    }
}
