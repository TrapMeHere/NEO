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
    /// 命中路径。
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

    /// 当前绘制中的路径。
    pub fn pending_way(&self) -> Option<ObjectId> {
        self.pending_way
    }

    /// 当前绘制中的节点序列（含尚未成路径的起点）。
    pub fn pending_nodes(&self) -> &[ObjectId] {
        &self.pending_nodes
    }

    /// 放弃未完成的路径。
    pub fn reset_pending(&mut self) {
        self.pending_nodes.clear();
        self.pending_way = None;
        self.draw_step_cmds.clear();
    }

    /// 结束当前路径（面模式会先闭合并确保 `area=yes`；保留已创建的命令历史）。
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
        Outcome::Message(format!("已结束路径（{count} 个节点）"))
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
        // 线段吸附：新建节点并插入该路径，成为共享成员。
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

    /// 在路径线段中点插入节点并选中；成功时返回新节点 ID。
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

        // 线工具、草稿为空：点击开放路径端点 → 载入续画（不写状态栏）。
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
                Ok(()) => Outcome::Edited("追加路径节点".into()),
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
                    Outcome::Edited(format!("开始路径 {way_id}"))
                }
                Err(err) => Outcome::Message(format!("创建路径失败：{err}")),
            }
        } else {
            Outcome::None
        };
        self.note_draw_step(editor, before);
        outcome
    }

    /// 从已有开放路径的端点开始续画。
    fn begin_continue_way(&mut self, editor: &mut Editor, way: ObjectId, endpoint: ObjectId) -> Outcome {
        let Some(handle) = editor.data().way_handle(way) else {
            return Outcome::Message("路径不存在".into());
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

    /// 合并两条端点相邻的路径（自动处理方向）。
    pub fn merge_ways(&mut self, editor: &mut Editor, a: ObjectId, b: ObjectId) -> Outcome {
        match editor.execute(Box::new(neo_editor::MergeWay::new(a, b))) {
            Ok(()) => Outcome::Edited("合并路径".into()),
            Err(err) => Outcome::Message(format!("合并失败：{err}")),
        }
    }

    /// 合并所选头尾相连的路径（可多条；每条链合并成一条）。
    ///
    /// 只考虑开放路径，按共享端点把所选路径连成链；分叉、成环或互不相邻的组会跳过。
    /// 所有链的合并命令一次 `execute_batch` 提交，撤销一次可整批回退。
    pub fn merge_chain_ways(&mut self, editor: &mut Editor, ways: &[ObjectId]) -> Outcome {
        let mut node_seqs: Vec<(ObjectId, Vec<ObjectId>)> = Vec::with_capacity(ways.len());
        for way in ways {
            let Some(handle) = editor.data().way_handle(*way) else {
                continue;
            };
            // 已逻辑删除的路径不再参与编辑。
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
            return Outcome::Message("所选路径无法合并：需要至少两条头尾相连的开放路径".into());
        }
        let mut commands: Vec<Box<dyn neo_editor::Command>> = Vec::new();
        for chain in &chains {
            // 整条链串行并入链首：后续命令必须以仍存活的链路首为 a，
            // 否则会去合并已被前一条命令逻辑删除的中间路径。
            let Some((&survivor, rest)) = chain.split_first() else {
                continue;
            };
            for way in rest {
                commands.push(Box::new(neo_editor::MergeWay::new(survivor, *way)));
            }
        }
        match editor.execute_batch(commands) {
            Ok(()) => Outcome::Edited("合并路径".into()),
            Err(err) => Outcome::Message(format!("合并失败：{err}")),
        }
    }

    /// 批量移动节点（拖动选中的节点/路径 = 一条历史记录）。
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

    /// 反转选中的路径。
    pub fn reverse_way(&mut self, editor: &mut Editor, way: ObjectId) -> Outcome {
        match editor.execute(Box::new(neo_editor::ReverseWay::new(way))) {
            Ok(()) => Outcome::Edited("反转路径".into()),
            Err(err) => Outcome::Message(format!("反转失败：{err}")),
        }
    }

    /// 在选中节点处分割路径；未选中节点时退化为中点分割。
    pub fn split_way(&mut self, editor: &mut Editor, way: ObjectId, at_node: Option<ObjectId>) -> Outcome {
        let Some(handle) = editor.data().way_handle(way) else {
            return Outcome::Message("路径不存在".into());
        };
        let nodes = editor.data().way_node_ids(handle);
        let len = nodes.len();
        if len < 3 {
            return Outcome::Message("路径太短，无法分割".into());
        }
        // 只有内部节点能作为分割点（首尾切出来会得到单点路径）。
        let at = at_node
            .and_then(|node| nodes.iter().position(|id| *id == node))
            .filter(|index| *index >= 1 && *index + 1 < len)
            .unwrap_or(len / 2);
        match editor.execute(Box::new(neo_editor::SplitWay::new(way, at as u32))) {
            Ok(()) => Outcome::Edited("分割路径".into()),
            Err(err) => Outcome::Message(format!("分割失败：{err}")),
        }
    }

    /// 在节点处分割所有经过它的路径（仅内部节点会分割）。
    ///
    /// 引用该节点的每条可见路径都会各自生成一条 `SplitWay` 命令，全部命令一次
    /// `execute_batch` 原子提交（撤销一次即可整体回退）。节点不存在或只是路径端点
    /// （没有内部引用）时返回提示信息。
    pub fn split_ways_at_node(&mut self, editor: &mut Editor, node: ObjectId) -> Outcome {
        let Some(node_handle) = editor.data().node_handle(node) else {
            return Outcome::Message("节点不存在".into());
        };
        // 反查表是只读的，先按当前数据重建，避免拿到上一步编辑前的引用关系。
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
            // 只有内部节点能作为分割点：首尾节点切出来会得到单点路径。
            if index >= 1 && index + 1 < len {
                commands.push(Box::new(neo_editor::SplitWay::new(way, index as u32)));
            }
        }
        if commands.is_empty() {
            return Outcome::Message("该点不是路径的内部节点".into());
        }
        match editor.execute_batch(commands) {
            Ok(()) => Outcome::Edited("分割路径".into()),
            Err(err) => Outcome::Message(format!("分割失败：{err}")),
        }
    }
}

/// 计算链式合并顺序。
///
/// 输入是 `(路径, 节点序列)` 列表（通常来自用户选择），返回每条可合并链的路径顺序：
/// 链内路径首尾相接，按顺序执行 `MergeWay::new(chain[0], chain[i])` 即可把整条链
/// 合成一条（`MergeWay` 会自动处理四条方向，并保留 `chain[0]` 的 ID）。
///
/// 规则：
/// * 只考虑开放路径（节点数 >= 2 且首尾节点不同）；闭合路径与重复路径直接忽略；
/// * 按共享端点把路径连成连通分量，每个分量必须是**链**：端点处最多两条路径相接且
///   不能成环（分叉、闭环分量整条跳过）；
/// * 只有含 2 条及以上路径的分量才算可合并链，孤立路径不返回；
/// * 链从较小的度 1 端点节点出发，保证顺序稳定。
pub fn chain_merge_order(node_seqs: &[(ObjectId, Vec<ObjectId>)]) -> Vec<Vec<ObjectId>> {
    // 过滤不可参与合并的路径，并按输入顺序去重。
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

    // 端点节点 -> 引用它的路径下标：共享端点即相邻。
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

    // 求连通分量（按路径相邻关系）。
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

    // 逐个分量校验并排出链序。
    let mut chains = Vec::new();
    for members in components {
        if members.len() < 2 {
            continue;
        }
        // 端点节点的度数：> 2 说明分叉，整条分量跳过。
        let mut degree: BTreeMap<ObjectId, usize> = BTreeMap::new();
        for &index in &members {
            let (_, first, last) = ways[index];
            *degree.entry(first).or_insert(0) += 1;
            *degree.entry(last).or_insert(0) += 1;
        }
        if degree.values().any(|degree| *degree > 2) {
            continue;
        }
        // 链（路径图）满足 边数 + 1 = 顶点数；否则说明成环或存在重复边。
        if members.len() + 1 != degree.len() {
            continue;
        }
        // 从最小的度 1 端点出发，沿未走过的路径走完整条链。
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

/// 在路径线段投影处新建节点并插入为成员。返回新节点 ID。
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

/// 在路径的 `seg` 段几何中点新建并插入节点。
pub fn insert_way_midpoint(editor: &mut Editor, way: ObjectId, seg: usize) -> Result<ObjectId, String> {
    let Some(handle) = editor.data().way_handle(way) else {
        return Err("路径不存在".into());
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

/// 在路径的 `seg` 段后插入已有节点（共享节点）。
fn insert_node_on_way(editor: &mut Editor, way: ObjectId, seg: usize, node: ObjectId) -> Result<(), String> {
    let Some(handle) = editor.data().way_handle(way) else {
        return Err("路径不存在".into());
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

/// 查找可从该端点续画的开放路径。
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
    // 端点仅挂接一条路径时直接续画该路径。
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
        let handle = editor.data().way_handle(way).expect("路径存在");
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
        // B 反向接入（B 首 = 4，B 尾 = A 尾 3），仍是同一条链。
        let reversed = chain_merge_order(&[(ObjectId(10), ids(&[1, 2, 3])), (ObjectId(11), ids(&[4, 3]))]);
        assert_eq!(reversed, vec![vec![ObjectId(10), ObjectId(11)]]);

        // B 首 = A 首（都从节点 1 出发）也能成链；重复路径只算一次。
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
        // 环：1-2-3-1，边数 = 顶点数，不成链。
        assert!(
            chain_merge_order(&[
                (ObjectId(10), ids(&[1, 2])),
                (ObjectId(11), ids(&[2, 3])),
                (ObjectId(12), ids(&[3, 1])),
            ])
            .is_empty()
        );
        // 分叉：节点 2 接三条路径。
        assert!(
            chain_merge_order(&[
                (ObjectId(10), ids(&[1, 2])),
                (ObjectId(11), ids(&[2, 3])),
                (ObjectId(12), ids(&[2, 4])),
            ])
            .is_empty()
        );
        // 闭合路径与互不相邻的路径都不参与合并。
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
        assert_eq!(outcome, Outcome::Edited("合并路径".into()));
        assert_eq!(editor.history().undo_len(), undo_before + 1);
        // 链从端点出发合并，保留的路径 ID 取决于起点方向；节点数必须等于各路径去重后的总数。
        let visible = visible_way_ids(&editor);
        assert_eq!(visible.len(), 1);
        let merged = way_nodes(&editor, visible[0]);
        assert_eq!(merged.len(), nodes.len());
        let mut reversed = nodes.clone();
        reversed.reverse();
        assert!(merged == nodes || merged == reversed, "合并顺序应为正向或反向");

        // 一条撤销回退整批合并。
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
        // A = 1→2→3；B = 1→4（B 首 = A 首），合并需自动把 B 反转接到 A 前面。
        let way_a = add_way(&mut editor, vec![a, b, c]);
        let way_b = add_way(&mut editor, vec![a, d]);
        let mut ctl = EditController::new(ToolKind::Select);

        let outcome = ctl.merge_chain_ways(&mut editor, &[way_a, way_b]);
        assert_eq!(outcome, Outcome::Edited("合并路径".into()));
        let visible = visible_way_ids(&editor);
        assert_eq!(visible.len(), 1);
        let merged = way_nodes(&editor, visible[0]);
        assert_eq!(merged.len(), 4);
        // 自动处理方向后必须首尾相接（正向或反向均可）。
        assert!(
            merged == vec![c, b, a, d] || merged == vec![d, a, b, c],
            "反向接入的路径应被自动翻转后合并：{merged:?}"
        );
    }

    #[test]
    fn merge_chain_ways_rejects_non_chain_selection() {
        // 互不相邻：不产生任何命令。
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

        // 成环：整条分量跳过。
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

        // 分叉：节点 p2 处三条路径相接。
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
        assert_eq!(outcome, Outcome::Edited("分割路径".into()));
        assert_eq!(editor.history().undo_len(), undo_before + 1);
        // 原路径保留前半段，新路径复制后半段。
        assert_eq!(way_nodes(&editor, way), nodes[0..2].to_vec());
        let visible = visible_way_ids(&editor);
        assert_eq!(visible.len(), 2);
        let new_way = *visible.iter().find(|id| **id != way).unwrap();
        assert_eq!(way_nodes(&editor, new_way), nodes[1..4].to_vec());

        // 一次撤销恢复原路径并移除新路径。
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
        assert_eq!(outcome, Outcome::Edited("分割路径".into()));
        // 两条路径各分一段，一次批处理只占一条历史。
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
