//! # neo-tools — 交互工具（选择 / 绘制 / 删除）
//!
//! 工具是**纯状态机**：输入"点击/拖拽结束 + 命中结果 + 世界坐标"，输出编辑命令（通过
//! `neo_editor::Editor` 执行）。不依赖 egui/wgpu，因此可以单测、可被未来 CLI/脚本复用。
//!
//! 绘制语义：
//! * **点**：点击创建单个节点；
//! * **线**：首次点击放起点，后续点击追加节点；再次点击最新节点或 `finish_way` 结束；
//! * **面**：同线，结束时自动闭合；再次点击起点也可闭合。

#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod geom;

use neo_core::{LatLon, ObjectId, ObjectKind};
use neo_editor::{AddNode, AddWay, Editor, MoveNode, SetTags, SetWayNodes};
use neo_geo::{SnapHit, SnapTarget};

pub use geom::{
    ToolEffect, ToolNote, align_selected_nodes, distribute_selected_nodes, extrude_selected_way,
    join_selected_areas, merge_selected_nodes, mirror_selected_nodes, move_node_onto_way,
    orthogonalize_selected, parallel_copy_way, unglue_selected_nodes,
};
pub use neo_geo::{SnapHit as DrawSnapHit, SnapTarget as DrawSnapTarget};

/// 当前工具。
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum ToolKind {
    /// 选择/移动。
    #[default]
    Select,
    /// 创建节点。
    DrawPoint,
    /// 绘制折线。
    DrawLine,
    /// 绘制闭合面。
    DrawArea,
}

impl ToolKind {
    /// 显示名。
    pub fn label(self) -> &'static str {
        match self {
            Self::Select => "选择",
            Self::DrawPoint => "点",
            Self::DrawLine => "线",
            Self::DrawArea => "面",
        }
    }

    /// 是否为绘制类工具。
    pub fn is_draw(self) -> bool {
        matches!(self, Self::DrawPoint | Self::DrawLine | Self::DrawArea)
    }
}

/// 命中结果（由应用层用 `neo_geo::HitTester` 得到）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Pick {
    /// 命中节点。
    pub node: Option<ObjectId>,
    /// 命中 way。
    pub way: Option<ObjectId>,
    /// 命中关系（点在面的内圈上）。
    pub relation: Option<ObjectId>,
}

/// 工具处理结果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// 无变化。
    None,
    /// 选择集变了。
    SelectionChanged,
    /// 执行了一次编辑。
    Edited(String),
    /// 提示/错误信息。
    Message(String),
}

impl Outcome {
    /// 是否有编辑发生（应用层据此刷新场景/面板）。
    pub fn is_edit(&self) -> bool {
        matches!(self, Self::Edited(_))
    }

    /// 取状态文本。
    pub fn message(&self) -> Option<&str> {
        match self {
            Self::Edited(m) | Self::Message(m) => Some(m),
            _ => None,
        }
    }
}

/// 工具状态机。
pub struct EditController {
    /// 当前工具。
    pub tool: ToolKind,
    /// 绘制时是否启用几何吸附（Alt 临时关闭由应用层处理）。
    pub snap_enabled: bool,
    pending_nodes: Vec<ObjectId>,
    pending_way: Option<ObjectId>,
    /// 当前绘制会话中每一步写入的历史条数（供右键逐步回退）。
    draw_step_cmds: Vec<usize>,
    /// 刚完成一个完整要素（点落成 / 线面结束），应用层应切回选择工具。
    finished_feature: bool,
}

impl Default for EditController {
    fn default() -> Self {
        Self::new(ToolKind::Select)
    }
}

impl EditController {
    /// 构造。
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

    /// 切换工具（会结束未完成的绘制）。
    pub fn set_tool(&mut self, tool: ToolKind) {
        if tool != self.tool {
            self.reset_pending();
        }
        self.tool = tool;
    }

    /// 取出并清除「要素已完成」标志。
    pub fn take_finished_feature(&mut self) -> bool {
        let done = self.finished_feature;
        self.finished_feature = false;
        done
    }

    /// 当前绘制中的节点数。
    pub fn pending_len(&self) -> usize {
        self.pending_nodes.len()
    }

    /// 当前绘制中的 way。
    pub fn pending_way(&self) -> Option<ObjectId> {
        self.pending_way
    }

    /// 当前绘制中的节点序列（含尚未成 way 的起点）。
    pub fn pending_nodes(&self) -> &[ObjectId] {
        &self.pending_nodes
    }

    /// 放弃未完成的 way。
    pub fn reset_pending(&mut self) {
        self.pending_nodes.clear();
        self.pending_way = None;
        self.draw_step_cmds.clear();
    }

    /// 结束当前 way（面模式会先闭合并确保 `area=yes`；保留已创建的命令历史）。
    pub fn finish_way(&mut self, editor: &mut Editor) -> Outcome {
        if self.pending_nodes.is_empty() {
            return Outcome::None;
        }
        if self.tool == ToolKind::DrawArea {
            if self.pending_nodes.len() < 3 {
                return Outcome::Message("面至少需要 3 个节点".into());
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
        Outcome::Message(format!("已结束 way（{count} 个节点）"))
    }

    /// 绘制模式下回退上一步绘制；若上一步不是绘制则不撤销其它历史。
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
            return Outcome::Edited("回退绘制".into());
        }
        // 续画载入尚未写入历史：仅取消草稿。
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

    /// 处理一次点击。
    pub fn on_click(&mut self, editor: &mut Editor, pick: Pick, pos: LatLon) -> Outcome {
        self.on_click_with_snap(editor, pick, pos, None)
    }

    /// 处理一次点击，可附带吸附结果（绘制工具使用）。
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

    /// 处理一次点击（`additive` = Shift/Ctrl 追加选择，用于多选）。
    pub fn on_click_multi(
        &mut self,
        editor: &mut Editor,
        pick: Pick,
        pos: LatLon,
        additive: bool,
    ) -> Outcome {
        self.on_click_multi_with_snap(editor, pick, pos, additive, None)
    }

    /// 多选点击 + 可选吸附。
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
            return Outcome::Message("已有节点".into());
        }
        if pick.node.is_some() && snap.is_none() {
            return Outcome::Message("已有节点".into());
        }
        let before = editor.history().undo_len();
        // 线段吸附：新建节点并插入该 way，成为共享成员。
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
                    Outcome::Edited(format!("在线段上创建节点 {id}"))
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
                Outcome::Edited(format!("创建节点 {id}"))
            }
            Err(err) => Outcome::Message(format!("创建节点失败：{err}")),
        }
    }

    /// 在 way 线段中点插入节点并选中；成功时返回新节点 ID。
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

        // 线工具、草稿为空：点击开放 way 端点 → 载入续画（不写状态栏）。
        if self.pending_nodes.is_empty()
            && self.tool == ToolKind::DrawLine
            && let Some(node) = hit_node
            && let Some(way) = find_continuable_way(editor, node, pick.way)
        {
            return self.begin_continue_way(editor, way, node);
        }

        // 线：再次点击最新节点 → 结束绘制（先于落点解析，避免新建重复节点）。
        if let Some(id) = hit_node
            && self.pending_nodes.last() == Some(&id)
        {
            if self.tool == ToolKind::DrawLine && self.pending_nodes.len() >= 2 {
                return self.finish_way(editor);
            }
            return Outcome::None;
        }

        // 面：再次点到起点则闭合结束。
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
                "已放置起点，继续点击绘制面；再次点击起点或按完成以闭合"
            } else {
                "已放置起点，继续点击绘制线；再次点击最新节点结束"
            };
            return if created_on_segment || before < editor.history().undo_len() {
                Outcome::Edited(tip.into())
            } else {
                Outcome::Message(tip.into())
            };
        }

        // 落点解析后若仍落到最新节点（例如仅 pick 命中），同样结束/忽略。
        if self.pending_nodes.last() == Some(&node_id) {
            if self.tool == ToolKind::DrawLine && self.pending_nodes.len() >= 2 {
                return self.finish_way(editor);
            }
            return Outcome::None;
        }

        // 面：落点解析后点到起点则闭合。
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
                Ok(()) => Outcome::Edited("追加 way 节点".into()),
                Err(err) => Outcome::Message(format!("追加失败：{err}")),
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
                    Outcome::Edited(format!("开始 way {way_id}"))
                }
                Err(err) => Outcome::Message(format!("创建 way 失败：{err}")),
            }
        } else {
            Outcome::None
        };
        self.note_draw_step(editor, before);
        outcome
    }

    /// 从已有开放 way 的端点开始续画。
    fn begin_continue_way(&mut self, editor: &mut Editor, way: ObjectId, endpoint: ObjectId) -> Outcome {
        let Some(handle) = editor.data().way_handle(way) else {
            return Outcome::Message("way 不存在".into());
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
        // 画布 overlay 反馈即可，不写状态栏。
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
        // 首尾必须同一节点：否则仍会被当成未闭合路径。
        if nodes.len() < 4 || nodes.first() != nodes.last() {
            return Err("面未能闭合".into());
        }
        editor
            .execute(Box::new(SetWayNodes::new(way, nodes)))
            .map_err(|err| format!("闭合失败：{err}"))
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
            .map_err(|err| format!("写入 area=yes 失败：{err}"))
    }

    /// 拖拽结束：把节点从 `from` 移到 `to`（一次拖动 = 一条历史）。
    pub fn on_drag_end(&mut self, editor: &mut Editor, node: ObjectId, from: LatLon, to: LatLon) -> Outcome {
        if from == to {
            return Outcome::None;
        }
        match editor.execute(Box::new(MoveNode::new(node, from, to))) {
            Ok(()) => Outcome::Edited("移动节点".into()),
            Err(err) => Outcome::Message(format!("移动失败：{err}")),
        }
    }

    /// 合并两条端点相邻的 way（自动处理方向）。
    pub fn merge_ways(&mut self, editor: &mut Editor, a: ObjectId, b: ObjectId) -> Outcome {
        match editor.execute(Box::new(neo_editor::MergeWay::new(a, b))) {
            Ok(()) => Outcome::Edited("合并 way".into()),
            Err(err) => Outcome::Message(format!("合并失败：{err}")),
        }
    }

    /// 批量移动节点（拖动选中的节点/way = 一条历史记录）。
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
            Ok(()) => Outcome::Edited(format!("移动 {} 个节点", moves.len())),
            Err(err) => Outcome::Message(format!("移动失败：{err}")),
        }
    }

    /// 反转选中的 way。
    pub fn reverse_way(&mut self, editor: &mut Editor, way: ObjectId) -> Outcome {
        match editor.execute(Box::new(neo_editor::ReverseWay::new(way))) {
            Ok(()) => Outcome::Edited("反转 way".into()),
            Err(err) => Outcome::Message(format!("反转失败：{err}")),
        }
    }

    /// 在选中节点处分割 way；未选中节点时退化为中点分割。
    pub fn split_way(&mut self, editor: &mut Editor, way: ObjectId, at_node: Option<ObjectId>) -> Outcome {
        let Some(handle) = editor.data().way_handle(way) else {
            return Outcome::Message("way 不存在".into());
        };
        let nodes = editor.data().way_node_ids(handle);
        let len = nodes.len();
        if len < 3 {
            return Outcome::Message("way 太短，无法分割".into());
        }
        // 只有内部节点能作为分割点（首尾切出来会得到单点 way）。
        let at = at_node
            .and_then(|node| nodes.iter().position(|id| *id == node))
            .filter(|index| *index >= 1 && *index + 1 < len)
            .unwrap_or(len / 2);
        match editor.execute(Box::new(neo_editor::SplitWay::new(way, at as u32))) {
            Ok(()) => Outcome::Edited("分割 way".into()),
            Err(err) => Outcome::Message(format!("分割失败：{err}")),
        }
    }
}

/// 解析绘制落点：复用节点 / 线段插点 / 新建。返回 `(node_id, created_on_segment)`。
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
                .map_err(|err| format!("创建节点失败：{err}"))?;
            Ok((id, false))
        }
    }
}

/// 在 way 线段投影处新建节点并插入为成员。返回新节点 ID。
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
        .map_err(|err| format!("创建节点失败：{err}"))?;
    insert_node_on_way(editor, way, seg, id)?;
    Ok(id)
}

/// 在 way 的 `seg` 段几何中点新建并插入节点。
pub fn insert_way_midpoint(editor: &mut Editor, way: ObjectId, seg: usize) -> Result<ObjectId, String> {
    let Some(handle) = editor.data().way_handle(way) else {
        return Err("way 不存在".into());
    };
    let nodes = editor.data().way_node_ids(handle);
    let (a, b) = nodes
        .get(seg)
        .copied()
        .zip(nodes.get(seg + 1).copied())
        .ok_or_else(|| "线段下标无效".to_owned())?;
    let pa = editor
        .data()
        .node_handle(a)
        .and_then(|h| editor.data().nodes.position(h))
        .ok_or_else(|| "线段端点缺失".to_owned())?;
    let pb = editor
        .data()
        .node_handle(b)
        .and_then(|h| editor.data().nodes.position(h))
        .ok_or_else(|| "线段端点缺失".to_owned())?;
    let at = segment_midpoint_latlon(pa, pb);
    insert_node_on_segment(editor, way, seg, at)
}

/// 线段几何中点（世界坐标中点再转回经纬度）。
pub fn segment_midpoint_latlon(a: LatLon, b: LatLon) -> LatLon {
    let wa = neo_geo::WorldPoint::from_latlon(a);
    let wb = neo_geo::WorldPoint::from_latlon(b);
    neo_geo::WorldPoint::new((wa.x + wb.x) * 0.5, (wa.y + wb.y) * 0.5).to_latlon()
}

/// 在 way 的 `seg` 段后插入已有节点（共享节点）。
fn insert_node_on_way(editor: &mut Editor, way: ObjectId, seg: usize, node: ObjectId) -> Result<(), String> {
    let Some(handle) = editor.data().way_handle(way) else {
        return Err("way 不存在".into());
    };
    let mut nodes = editor.data().way_node_ids(handle);
    if nodes.contains(&node) {
        return Ok(());
    }
    let insert_at = seg + 1;
    if insert_at > nodes.len() {
        return Err("线段下标无效".into());
    }
    nodes.insert(insert_at, node);
    editor
        .execute(Box::new(SetWayNodes::new(way, nodes)))
        .map_err(|err| format!("插入节点失败：{err}"))
}

/// 查找可从该端点续画的开放 way。
fn find_continuable_way(
    editor: &mut Editor,
    endpoint: ObjectId,
    preferred_way: Option<ObjectId>,
) -> Option<ObjectId> {
    // AddWay 等命令会标脏，这里先刷新再查反向引用。
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
                return None; // 闭合
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
    // 端点仅挂接一条 way 时直接续画该 way。
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
        // 先放一个自由起点
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
        // 再右键：没有绘制步骤，不碰其它历史
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
}
