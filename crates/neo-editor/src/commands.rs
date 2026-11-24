//! 具体命令实现。

use std::any::Any;

use neo_core::{LatLon, ObjectId, ObjectKind};
use neo_store::DataSet;

use crate::command::{Command, CommandError, CommandKind};
use crate::editor::EditContext;
use crate::journal::JournalCommand;

/// 关系成员的可序列化描述（编辑命令与 UI 共用）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RelationMemberSpec {
    /// 成员类型。
    pub kind: ObjectKind,
    /// 成员对象 ID。
    pub id: ObjectId,
    /// 角色（空串表示无角色）。
    pub role: String,
}

fn member_byte(kind: ObjectKind) -> u8 {
    match kind {
        ObjectKind::Node => 0,
        ObjectKind::Way => 1,
        ObjectKind::Relation => 2,
        ObjectKind::Changeset => 3,
    }
}

fn member_kind(byte: u8) -> Option<ObjectKind> {
    match byte {
        0 => Some(ObjectKind::Node),
        1 => Some(ObjectKind::Way),
        2 => Some(ObjectKind::Relation),
        _ => None,
    }
}

fn members_to_journal(members: &[RelationMemberSpec]) -> Vec<(u8, i64, String)> {
    members
        .iter()
        .map(|member| (member_byte(member.kind), member.id.0, member.role.clone()))
        .collect()
}

fn kind_byte(kind: ObjectKind) -> u8 {
    match kind {
        ObjectKind::Node => 0,
        ObjectKind::Way => 1,
        ObjectKind::Relation => 2,
        ObjectKind::Changeset => 3,
    }
}

fn kind_from_byte(byte: u8) -> Option<ObjectKind> {
    match byte {
        0 => Some(ObjectKind::Node),
        1 => Some(ObjectKind::Way),
        2 => Some(ObjectKind::Relation),
        _ => None,
    }
}

/// 添加节点。
pub struct AddNode {
    /// 期望 ID（None 表示自动分配负数本地 ID）。
    pub id: Option<ObjectId>,
    /// 位置。
    pub pos: LatLon,
    /// 标签。
    pub tags: Vec<(String, String)>,
    applied: Option<ObjectId>,
}

impl AddNode {
    /// 构造。
    pub fn new(pos: LatLon, tags: Vec<(String, String)>) -> Self {
        Self {
            id: None,
            pos,
            tags,
            applied: None,
        }
    }
}

impl Command for AddNode {
    fn kind(&self) -> CommandKind {
        CommandKind::AddNode
    }
    fn label(&self) -> &'static str {
        "添加节点"
    }
    fn apply(&mut self, ctx: &mut EditContext<'_>) -> Result<(), CommandError> {
        let id = self
            .id
            .or(self.applied)
            .unwrap_or_else(|| ctx.data.next_local_id());
        ctx.add_node(id, self.pos, &self.tags)?;
        self.applied = Some(id);
        Ok(())
    }
    fn undo(&mut self, ctx: &mut EditContext<'_>) -> Result<(), CommandError> {
        if let Some(id) = self.applied {
            ctx.remove_node(id)?;
        }
        Ok(())
    }
    fn record(&self) -> JournalCommand {
        JournalCommand::AddNode {
            // 必须记录**实际分配**的 ID：记录 0 会让重放重新分配 ID，
            // 后续引用该节点的 AddWay/SetWayNodes 全部悬空。
            id: self.applied.or(self.id).unwrap_or(ObjectId::NONE).0,
            lat: self.pos.lat.units(),
            lon: self.pos.lon.units(),
            tags: self.tags.clone(),
        }
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// 添加路径。
pub struct AddWay {
    /// 期望 ID。
    pub id: Option<ObjectId>,
    /// 节点 ID 序列。
    pub nodes: Vec<ObjectId>,
    /// 标签。
    pub tags: Vec<(String, String)>,
    applied: Option<ObjectId>,
}

impl AddWay {
    /// 构造。
    pub fn new(nodes: Vec<ObjectId>, tags: Vec<(String, String)>) -> Self {
        Self {
            id: None,
            nodes,
            tags,
            applied: None,
        }
    }
}

impl Command for AddWay {
    fn kind(&self) -> CommandKind {
        CommandKind::AddWay
    }
    fn label(&self) -> &'static str {
        "添加路径"
    }
    fn precheck(&self, _data: &DataSet, _referrers: &neo_store::Referrers) -> Result<(), CommandError> {
        if self.nodes.len() < 2 {
            return Err(CommandError::Invalid("way needs at least 2 nodes".into()));
        }
        Ok(())
    }
    fn apply(&mut self, ctx: &mut EditContext<'_>) -> Result<(), CommandError> {
        let id = self
            .id
            .or(self.applied)
            .unwrap_or_else(|| ctx.data.next_local_id());
        ctx.add_way(id, &self.nodes, &self.tags)?;
        self.applied = Some(id);
        Ok(())
    }
    fn undo(&mut self, ctx: &mut EditContext<'_>) -> Result<(), CommandError> {
        if let Some(id) = self.applied {
            ctx.remove_way(id)?;
        }
        Ok(())
    }
    fn record(&self) -> JournalCommand {
        JournalCommand::AddWay {
            id: self.applied.or(self.id).unwrap_or(ObjectId::NONE).0,
            nodes: self.nodes.iter().map(|id| id.0).collect(),
            tags: self.tags.clone(),
        }
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// 移动节点。
pub struct MoveNode {
    /// 节点 ID。
    pub id: ObjectId,
    /// 原位置。
    pub from: LatLon,
    /// 新位置。
    pub to: LatLon,
    undo_pos: Option<LatLon>,
    undo_state: Option<neo_osm::ChangeState>,
}

impl MoveNode {
    /// 构造。
    pub fn new(id: ObjectId, from: LatLon, to: LatLon) -> Self {
        Self {
            id,
            from,
            to,
            undo_pos: None,
            undo_state: None,
        }
    }
}

impl Command for MoveNode {
    fn kind(&self) -> CommandKind {
        CommandKind::MoveNode
    }
    fn label(&self) -> &'static str {
        "移动节点"
    }
    fn precheck(&self, data: &DataSet, _referrers: &neo_store::Referrers) -> Result<(), CommandError> {
        if data.node_handle(self.id).is_none() {
            return Err(CommandError::NotFound {
                kind: ObjectKind::Node,
                id: self.id,
            });
        }
        Ok(())
    }
    fn apply(&mut self, ctx: &mut EditContext<'_>) -> Result<(), CommandError> {
        let state = ctx.data.change_state(ObjectKind::Node, self.id);
        ctx.move_node(self.id, self.to)?;
        if state == neo_osm::ChangeState::Unchanged {
            ctx.data
                .set_change_state(ObjectKind::Node, self.id, neo_osm::ChangeState::Modified);
        }
        // undo 必须回到构造时记录的起点：拖动预览可能已经改过 DataSet。
        self.undo_pos = Some(self.from);
        self.undo_state = Some(state);
        Ok(())
    }
    fn undo(&mut self, ctx: &mut EditContext<'_>) -> Result<(), CommandError> {
        if let Some(pos) = self.undo_pos.take() {
            ctx.move_node(self.id, pos)?;
        }
        if let Some(state) = self.undo_state.take() {
            ctx.data.set_change_state(ObjectKind::Node, self.id, state);
        }
        Ok(())
    }
    fn merge(&mut self, newer: &dyn Command) -> bool {
        if let Some(other) = newer.as_any().downcast_ref::<MoveNode>()
            && other.id == self.id
        {
            self.to = other.to;
            return true;
        }
        false
    }
    fn record(&self) -> JournalCommand {
        JournalCommand::MoveNode {
            id: self.id.0,
            lat: self.to.lat.units(),
            lon: self.to.lon.units(),
            from_lat: Some(self.from.lat.units()),
            from_lon: Some(self.from.lon.units()),
        }
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// 修改标签。
pub struct SetTags {
    /// 对象种类。
    pub kind: ObjectKind,
    /// 对象 ID。
    pub id: ObjectId,
    /// 新标签。
    pub new: Vec<(String, String)>,
    undo_old: Option<Vec<(String, String)>>,
    undo_state: Option<neo_osm::ChangeState>,
}

impl SetTags {
    /// 构造。
    pub fn new(kind: ObjectKind, id: ObjectId, new: Vec<(String, String)>) -> Self {
        Self {
            kind,
            id,
            new,
            undo_old: None,
            undo_state: None,
        }
    }
}

impl Command for SetTags {
    fn kind(&self) -> CommandKind {
        CommandKind::SetTags
    }
    fn label(&self) -> &'static str {
        "修改标签"
    }
    fn apply(&mut self, ctx: &mut EditContext<'_>) -> Result<(), CommandError> {
        let old = ctx.tags_of(self.kind, self.id)?;
        let state = ctx.data.change_state(self.kind, self.id);
        ctx.set_tags(self.kind, self.id, &self.new)?;
        if state == neo_osm::ChangeState::Unchanged {
            ctx.data
                .set_change_state(self.kind, self.id, neo_osm::ChangeState::Modified);
        }
        self.undo_old = Some(old);
        self.undo_state = Some(state);
        Ok(())
    }
    fn undo(&mut self, ctx: &mut EditContext<'_>) -> Result<(), CommandError> {
        if let Some(old) = self.undo_old.take() {
            ctx.set_tags(self.kind, self.id, &old)?;
        }
        if let Some(state) = self.undo_state.take() {
            ctx.data.set_change_state(self.kind, self.id, state);
        }
        Ok(())
    }
    fn merge(&mut self, newer: &dyn Command) -> bool {
        if let Some(other) = newer.as_any().downcast_ref::<SetTags>()
            && other.kind == self.kind
            && other.id == self.id
        {
            self.new = other.new.clone();
            return true;
        }
        false
    }
    fn record(&self) -> JournalCommand {
        JournalCommand::SetTags {
            kind: kind_byte(self.kind),
            id: self.id.0,
            tags: self.new.clone(),
        }
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// 修改路径的节点序列。
pub struct SetWayNodes {
    /// Way ID。
    pub way: ObjectId,
    /// 新节点序列。
    pub new: Vec<ObjectId>,
    undo_old: Option<Vec<ObjectId>>,
    undo_state: Option<neo_osm::ChangeState>,
}

impl SetWayNodes {
    /// 构造。
    pub fn new(way: ObjectId, new: Vec<ObjectId>) -> Self {
        Self {
            way,
            new,
            undo_old: None,
            undo_state: None,
        }
    }
}

impl Command for SetWayNodes {
    fn kind(&self) -> CommandKind {
        CommandKind::SetWayNodes
    }
    fn label(&self) -> &'static str {
        "修改路径节点"
    }
    fn apply(&mut self, ctx: &mut EditContext<'_>) -> Result<(), CommandError> {
        let old = ctx.way_node_ids(self.way)?;
        let state = ctx.data.change_state(ObjectKind::Way, self.way);
        ctx.set_way_nodes(self.way, &self.new)?;
        if state == neo_osm::ChangeState::Unchanged {
            ctx.data
                .set_change_state(ObjectKind::Way, self.way, neo_osm::ChangeState::Modified);
        }
        self.undo_old = Some(old);
        self.undo_state = Some(state);
        Ok(())
    }
    fn undo(&mut self, ctx: &mut EditContext<'_>) -> Result<(), CommandError> {
        if let Some(old) = self.undo_old.take() {
            ctx.set_way_nodes(self.way, &old)?;
        }
        if let Some(state) = self.undo_state.take() {
            ctx.data.set_change_state(ObjectKind::Way, self.way, state);
        }
        Ok(())
    }
    fn record(&self) -> JournalCommand {
        JournalCommand::SetWayNodes {
            way: self.way.0,
            nodes: self.new.iter().map(|id| id.0).collect(),
        }
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// 新增关系。
pub struct AddRelation {
    /// 期望 ID（None 自动分配）。
    pub id: Option<ObjectId>,
    /// 成员。
    pub members: Vec<RelationMemberSpec>,
    /// 标签。
    pub tags: Vec<(String, String)>,
    applied: Option<ObjectId>,
}

impl AddRelation {
    /// 构造。
    pub fn new(members: Vec<RelationMemberSpec>, tags: Vec<(String, String)>) -> Self {
        Self {
            id: None,
            members,
            tags,
            applied: None,
        }
    }
}

impl Command for AddRelation {
    fn kind(&self) -> CommandKind {
        CommandKind::AddRelation
    }
    fn label(&self) -> &'static str {
        "添加关系"
    }
    fn precheck(&self, data: &DataSet, _referrers: &neo_store::Referrers) -> Result<(), CommandError> {
        for member in &self.members {
            let exists = match member.kind {
                ObjectKind::Node => data.node_handle(member.id).is_some(),
                ObjectKind::Way => data.way_handle(member.id).is_some(),
                ObjectKind::Relation => data.relation_handle(member.id).is_some(),
                ObjectKind::Changeset => false,
            };
            if !exists {
                return Err(CommandError::NotFound {
                    kind: member.kind,
                    id: member.id,
                });
            }
        }
        Ok(())
    }
    fn apply(&mut self, ctx: &mut EditContext<'_>) -> Result<(), CommandError> {
        let id = self
            .id
            .or(self.applied)
            .unwrap_or_else(|| ctx.data.next_local_id());
        ctx.add_relation(id, &self.members, &self.tags)?;
        self.applied = Some(id);
        Ok(())
    }
    fn undo(&mut self, ctx: &mut EditContext<'_>) -> Result<(), CommandError> {
        if let Some(id) = self.applied {
            ctx.remove_relation(id)?;
        }
        Ok(())
    }
    fn record(&self) -> JournalCommand {
        JournalCommand::AddRelation {
            id: self.applied.or(self.id).unwrap_or(ObjectId::NONE).0,
            members: members_to_journal(&self.members),
            tags: self.tags.clone(),
        }
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// 替换关系成员（增删/改角色/排序统一走这一条命令）。
pub struct SetRelationMembers {
    /// Relation ID。
    pub relation: ObjectId,
    /// 新成员列表（顺序即语义）。
    pub members: Vec<RelationMemberSpec>,
    undo_old: Option<Vec<RelationMemberSpec>>,
    undo_state: Option<neo_osm::ChangeState>,
}

impl SetRelationMembers {
    /// 构造。
    pub fn new(relation: ObjectId, members: Vec<RelationMemberSpec>) -> Self {
        Self {
            relation,
            members,
            undo_old: None,
            undo_state: None,
        }
    }
}

impl Command for SetRelationMembers {
    fn kind(&self) -> CommandKind {
        CommandKind::SetRelationMembers
    }
    fn label(&self) -> &'static str {
        "修改关系成员"
    }
    fn precheck(&self, data: &DataSet, _referrers: &neo_store::Referrers) -> Result<(), CommandError> {
        // 成员必须存在；关系成员还不能形成环（OSM 服务端会 412，本地先拦）。
        for member in &self.members {
            let exists = match member.kind {
                ObjectKind::Node => data.node_handle(member.id).is_some(),
                ObjectKind::Way => data.way_handle(member.id).is_some(),
                ObjectKind::Relation => data.relation_handle(member.id).is_some(),
                ObjectKind::Changeset => false,
            };
            if !exists {
                return Err(CommandError::NotFound {
                    kind: member.kind,
                    id: member.id,
                });
            }
            if member.kind == ObjectKind::Relation
                && (member.id == self.relation || relation_reaches(data, member.id, self.relation))
            {
                return Err(CommandError::Invalid(format!(
                    "relation {} would contain itself as a member (via relation {})",
                    self.relation, member.id
                )));
            }
        }
        Ok(())
    }
    fn apply(&mut self, ctx: &mut EditContext<'_>) -> Result<(), CommandError> {
        let old = ctx.relation_members(self.relation)?;
        let state = ctx.data.change_state(ObjectKind::Relation, self.relation);
        ctx.set_relation_members(self.relation, &self.members)?;
        if state == neo_osm::ChangeState::Unchanged {
            ctx.data.set_change_state(
                ObjectKind::Relation,
                self.relation,
                neo_osm::ChangeState::Modified,
            );
        }
        self.undo_old = Some(old);
        self.undo_state = Some(state);
        Ok(())
    }
    fn undo(&mut self, ctx: &mut EditContext<'_>) -> Result<(), CommandError> {
        if let Some(old) = self.undo_old.take() {
            ctx.set_relation_members(self.relation, &old)?;
        }
        if let Some(state) = self.undo_state.take() {
            ctx.data
                .set_change_state(ObjectKind::Relation, self.relation, state);
        }
        Ok(())
    }
    fn record(&self) -> JournalCommand {
        JournalCommand::SetRelationMembers {
            relation: self.relation.0,
            members: members_to_journal(&self.members),
        }
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// 删除对象（保守语义：仍被引用则拒绝）。
pub struct DeleteObject {
    /// 种类。
    pub kind: ObjectKind,
    /// ID。
    pub id: ObjectId,
    undo_state: Option<(neo_osm::ChangeState, bool)>,
}

impl DeleteObject {
    /// 构造。
    pub fn new(kind: ObjectKind, id: ObjectId) -> Self {
        Self {
            kind,
            id,
            undo_state: None,
        }
    }
}

impl Command for DeleteObject {
    fn kind(&self) -> CommandKind {
        CommandKind::Delete
    }
    fn label(&self) -> &'static str {
        "删除对象"
    }
    fn precheck(&self, data: &DataSet, referrers: &neo_store::Referrers) -> Result<(), CommandError> {
        if data.change_state(self.kind, self.id) == neo_osm::ChangeState::Deleted {
            return Err(CommandError::Invalid(format!(
                "{} {} is already deleted",
                self.kind, self.id
            )));
        }
        match self.kind {
            ObjectKind::Node => {
                let handle = data.node_handle(self.id).ok_or(CommandError::NotFound {
                    kind: ObjectKind::Node,
                    id: self.id,
                })?;
                if !referrers.ways_of_node(handle).is_empty() {
                    return Err(CommandError::Referenced {
                        kind: ObjectKind::Node,
                        id: self.id,
                        what: format!("{} ways", referrers.ways_of_node(handle).len()),
                    });
                }
                if !referrers.relations_of_node(handle).is_empty() {
                    return Err(CommandError::Referenced {
                        kind: ObjectKind::Node,
                        id: self.id,
                        what: "relations".into(),
                    });
                }
            }
            ObjectKind::Way => {
                let handle = data.way_handle(self.id).ok_or(CommandError::NotFound {
                    kind: ObjectKind::Way,
                    id: self.id,
                })?;
                if !referrers.relations_of_way(handle).is_empty() {
                    return Err(CommandError::Referenced {
                        kind: ObjectKind::Way,
                        id: self.id,
                        what: "relations".into(),
                    });
                }
            }
            ObjectKind::Relation => {
                if data.relation_handle(self.id).is_none() {
                    return Err(CommandError::NotFound {
                        kind: ObjectKind::Relation,
                        id: self.id,
                    });
                }
                if !referrers
                    .relations_of_relation(data.relation_handle(self.id).expect("checked above"))
                    .is_empty()
                {
                    return Err(CommandError::Referenced {
                        kind: ObjectKind::Relation,
                        id: self.id,
                        what: "parent relations".into(),
                    });
                }
            }
            ObjectKind::Changeset => {}
        }
        Ok(())
    }
    fn apply(&mut self, ctx: &mut EditContext<'_>) -> Result<(), CommandError> {
        let state = ctx.data.change_state(self.kind, self.id);
        let visible = ctx.visible_of(self.kind, self.id);
        ctx.mark_deleted(self.kind, self.id)?;
        self.undo_state = Some((state, visible));
        Ok(())
    }
    fn undo(&mut self, ctx: &mut EditContext<'_>) -> Result<(), CommandError> {
        if let Some((state, visible)) = self.undo_state.take() {
            ctx.restore_deleted(self.kind, self.id, state, visible);
        }
        Ok(())
    }
    fn record(&self) -> JournalCommand {
        JournalCommand::Delete {
            kind: kind_byte(self.kind),
            id: self.id.0,
        }
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// 从 journal 记录重建命令（用于 replay；不需要 undo 信息）。
pub fn from_journal(record: &JournalCommand) -> Option<Box<dyn Command>> {
    let cmd: Box<dyn Command> = match record {
        JournalCommand::AddNode { id, lat, lon, tags } => {
            let mut c = AddNode::new(
                LatLon::new(
                    neo_core::Coord::from_units(*lat),
                    neo_core::Coord::from_units(*lon),
                ),
                tags.clone(),
            );
            if *id != 0 {
                c.id = Some(ObjectId(*id));
            }
            Box::new(c)
        }
        JournalCommand::AddWay { id, nodes, tags } => {
            let mut c = AddWay::new(nodes.iter().map(|v| ObjectId(*v)).collect(), tags.clone());
            if *id != 0 {
                c.id = Some(ObjectId(*id));
            }
            Box::new(c)
        }
        JournalCommand::AddRelation { id, members, tags } => {
            let specs = members
                .iter()
                .map(|(kind, id, role)| {
                    Some(RelationMemberSpec {
                        kind: member_kind(*kind)?,
                        id: ObjectId(*id),
                        role: role.clone(),
                    })
                })
                .collect::<Option<Vec<_>>>()?;
            let mut c = AddRelation::new(specs, tags.clone());
            if *id != 0 {
                c.id = Some(ObjectId(*id));
            }
            Box::new(c)
        }
        JournalCommand::SetRelationMembers { relation, members } => {
            let specs = members
                .iter()
                .map(|(kind, id, role)| {
                    Some(RelationMemberSpec {
                        kind: member_kind(*kind)?,
                        id: ObjectId(*id),
                        role: role.clone(),
                    })
                })
                .collect::<Option<Vec<_>>>()?;
            Box::new(SetRelationMembers::new(ObjectId(*relation), specs))
        }
        JournalCommand::MoveNode {
            id,
            lat,
            lon,
            from_lat,
            from_lon,
        } => {
            let to = LatLon::new(
                neo_core::Coord::from_units(*lat),
                neo_core::Coord::from_units(*lon),
            );
            let from = match (from_lat, from_lon) {
                (Some(lat), Some(lon)) => LatLon::new(
                    neo_core::Coord::from_units(*lat),
                    neo_core::Coord::from_units(*lon),
                ),
                _ => to,
            };
            Box::new(MoveNode::new(ObjectId(*id), from, to))
        }
        JournalCommand::SetTags { kind, id, tags } => {
            let kind = kind_from_byte(*kind)?;
            Box::new(SetTags::new(kind, ObjectId(*id), tags.clone()))
        }
        JournalCommand::SetWayNodes { way, nodes } => Box::new(SetWayNodes::new(
            ObjectId(*way),
            nodes.iter().map(|v| ObjectId(*v)).collect(),
        )),
        JournalCommand::Delete { kind, id } => {
            let kind = kind_from_byte(*kind)?;
            Box::new(DeleteObject::new(kind, ObjectId(*id)))
        }
        JournalCommand::SplitWay { way, at, new_way } => {
            let mut cmd = crate::commands_geom::SplitWay::new(ObjectId(*way), *at);
            if *new_way != 0 {
                cmd.new_way = Some(ObjectId(*new_way));
            }
            Box::new(cmd)
        }
        JournalCommand::MergeWay { a, b } => {
            Box::new(crate::commands_geom::MergeWay::new(ObjectId(*a), ObjectId(*b)))
        }
        JournalCommand::ReverseWay { way } => Box::new(crate::commands_geom::ReverseWay::new(ObjectId(*way))),
    };
    Some(cmd)
}

/// 从 `start` 沿关系成员向下能否到达 `target`（用于成环检测）。
///
/// 带 visited 集合：即使数据集里已经存在环（历史数据/插件写入）也不会死循环。
fn relation_reaches(data: &DataSet, start: ObjectId, target: ObjectId) -> bool {
    let mut stack = vec![start];
    let mut seen: std::collections::BTreeSet<ObjectId> = std::collections::BTreeSet::new();
    while let Some(current) = stack.pop() {
        if current == target {
            return true;
        }
        if !seen.insert(current) {
            continue;
        }
        let Some(handle) = data.relation_handle(current) else {
            continue;
        };
        let Some(view) = data.relation_view(handle) else {
            continue;
        };
        for member in view.members {
            if member.target.kind != ObjectKind::Relation {
                continue;
            }
            if let Some(id) = member.target.as_relation().and_then(|h| data.relations.id(h)) {
                stack.push(id);
            }
        }
    }
    false
}
