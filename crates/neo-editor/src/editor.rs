//! Editor：单写者、事务、历史、日志。

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

/// 命令执行上下文：数据 + 空间索引 + 反查表。
pub struct EditContext<'a> {
    /// 数据集（唯一可变入口）。
    pub data: &'a mut DataSet,
    /// 空间索引。
    pub index: &'a mut SpatialIndex,
    /// 节点世界坐标缓存（渲染热路径；随编辑增量更新）。
    pub world_cache: &'a mut WorldCache,
    /// 反向引用（只读）。
    pub referrers: &'a Referrers,
    /// 结构是否变化（需要重建反查表）。
    pub referrers_dirty: &'a mut bool,
}

impl EditContext<'_> {
    /// 节点句柄。
    pub fn node_handle(&self, id: ObjectId) -> Result<Handle<NodeSlot>, CommandError> {
        self.data.node_handle(id).ok_or(CommandError::NotFound {
            kind: ObjectKind::Node,
            id,
        })
    }
    /// Way 句柄。
    pub fn way_handle(&self, id: ObjectId) -> Result<Handle<WaySlot>, CommandError> {
        self.data.way_handle(id).ok_or(CommandError::NotFound {
            kind: ObjectKind::Way,
            id,
        })
    }
    /// Relation 句柄。
    pub fn relation_handle(&self, id: ObjectId) -> Result<Handle<RelationSlot>, CommandError> {
        self.data.relation_handle(id).ok_or(CommandError::NotFound {
            kind: ObjectKind::Relation,
            id,
        })
    }

    /// 把字符串 tag 对驻留成 TagSet。
    pub fn intern_tags(&mut self, pairs: &[(String, String)]) -> TagSetId {
        let converted: Vec<(Sym, Sym)> = pairs
            .iter()
            .map(|(k, v)| (self.data.intern(k), self.data.intern(v)))
            .collect();
        self.data.add_tags(&converted)
    }

    /// 读取某对象的标签（字符串对）。
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

    /// 设置标签。
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

    /// 移动节点，并维护空间索引 + 关联 way 的 bbox/脏标记。
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

    /// Way 的节点 ID 序列。
    pub fn way_node_ids(&self, id: ObjectId) -> Result<Vec<ObjectId>, CommandError> {
        let h = self.way_handle(id)?;
        Ok(self.data.try_way_node_ids(h)?)
    }

    /// 设置 way 的节点序列（ID 形式）。
    pub fn set_way_nodes(&mut self, id: ObjectId, nodes: &[ObjectId]) -> Result<(), CommandError> {
        let way_handle = self.way_handle(id)?;
        let mut node_handles = Vec::with_capacity(nodes.len());
        for node_id in nodes {
            node_handles.push(self.node_handle(*node_id)?);
        }
        self.data.set_way_nodes(way_handle, &node_handles)?;
        self.index.update_way(self.data, way_handle);
        self.data.mark_way_dirty(way_handle);
        // 节点序列变化会改变 node→way 反查关系（共享节点判定、移动节点时的父 way）。
        *self.referrers_dirty = true;
        Ok(())
    }

    /// 新增 relation。
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

    /// 物理移除 relation（undo AddRelation 用）。
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

    /// 读取 relation 成员（可序列化形式，供命令 undo/UI 使用）。
    pub fn relation_members(&self, id: ObjectId) -> Result<Vec<RelationMemberSpec>, CommandError> {
        relation_members_of(self.data, id)
    }

    /// 替换 relation 成员（保持顺序；角色驻留后写入）。
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
        // 成员关系变化会改变 relation→relation / way→relation 反查。
        *self.referrers_dirty = true;
        Ok(())
    }

    /// 新增节点。
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
        // 新节点还没有任何 way 引用，反查表无需重建（画 way 时每次点击都重建
        // referrers 是 O(N) 的编辑卡顿主因）。
        Ok(handle)
    }

    /// 新增 way。
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

    /// 物理移除节点（undo AddNode 用）。
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

    /// 物理移除 way。
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

    /// 标记删除（保留对象以便上传 diff / 撤销）。
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
        // 逻辑删除的 way/relation 不应再阻止其节点被删除；重建反查表以剔除它。
        *self.referrers_dirty = true;
        Ok(())
    }

    /// 对象当前的 visible 标记。
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

    /// 恢复删除。
    pub fn restore_deleted(&mut self, kind: ObjectKind, id: ObjectId, state: ChangeState, visible: bool) {
        self.data.set_change_state_visible(kind, id, state, visible);
        // 恢复可见会重新建立 way→node / relation→member 引用。
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

/// 编辑器。
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
    /// 从数据集构建（索引、世界坐标缓存与反查表立即构建）。
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

    /// 只读数据。
    pub fn data(&self) -> &DataSet {
        &self.data
    }

    /// 节点世界坐标缓存（渲染层只读）。
    pub fn world_cache(&self) -> &WorldCache {
        &self.world_cache
    }

    /// 读取 relation 成员（UI 用）。
    pub fn relation_members(&self, id: ObjectId) -> Result<Vec<RelationMemberSpec>, CommandError> {
        relation_members_of(&self.data, id)
    }

    /// 把一个名字驻留进数据集符号表。样式构建用它，不把 `&mut DataSet` 交出去。
    pub fn intern_sym(&mut self, name: &str) -> Sym {
        self.data.intern(name)
    }

    /// 装入一个已有节点，不记入撤销。测试和导入基线用；普通编辑走命令。
    pub fn import_node(&mut self, node: neo_osm::Node) -> Result<(), neo_core::NeoError> {
        self.data.push_node(node).map(|_| ())
    }

    /// 空间索引。
    pub fn index(&self) -> &SpatialIndex {
        &self.index
    }

    /// 选择集。
    pub fn selection(&self) -> &Selection {
        &self.selection
    }

    /// 选择集（可变）。
    pub fn selection_mut(&mut self) -> &mut Selection {
        &mut self.selection
    }

    /// 历史。
    pub fn history(&self) -> &History {
        &self.history
    }

    /// 历史（可变：保存后 clear_dirty 等）。
    pub fn history_mut(&mut self) -> &mut History {
        &mut self.history
    }

    /// 绑定 journal 输出。
    pub fn set_journal_sink(&mut self, sink: Box<dyn JournalSink>) {
        self.journal.set_sink(sink);
    }

    /// 分配本地 ID。
    pub fn next_local_id(&mut self) -> ObjectId {
        self.data.next_local_id()
    }

    /// 拖动预览：只改数据与索引，不产生历史/journal（释放时用 MoveNode 提交）。
    pub fn preview_move_node(&mut self, id: ObjectId, pos: LatLon) -> Result<(), CommandError> {
        let handle = self.data.node_handle(id).ok_or(CommandError::NotFound {
            kind: ObjectKind::Node,
            id,
        })?;
        self.data.set_node_position(handle, pos)?;
        self.data.mark_node_dirty(handle);
        self.index.update_node(handle, pos);
        self.world_cache.update(handle, pos);
        // 父 way 包围盒随节点移动，保证拖动中拾取/场景与几何一致。
        self.ensure_referrers();
        let parents: Vec<_> = self.referrers.ways_of_node(handle).to_vec();
        for way in parents {
            self.index.update_way(&self.data, way);
        }
        Ok(())
    }

    /// 返回与该节点直接相连的邻居节点 ID（拖动预览画线用）。
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

    /// 用当前数据重建反查表（结构变化后自动发生）。
    /// 反向引用（只读；场景构建判定共享节点等）。
    pub fn referrers(&self) -> &Referrers {
        &self.referrers
    }

    /// 渲染前刷新反查表（`referrers()` 是只读的，脏标记由写命令设置）。
    pub fn refresh_referrers(&mut self) {
        self.ensure_referrers();
    }

    fn ensure_referrers(&mut self) {
        if self.referrers_dirty {
            self.referrers = self.data.build_referrers();
            self.referrers_dirty = false;
        }
    }

    /// 执行单命令。
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
            // 命令自身可能已写了一半；best-effort undo + 恢复变更集，随后返回错误。
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

    /// 执行命令批（原子）。
    pub fn execute_batch(&mut self, mut cmds: Vec<Box<dyn Command>>) -> Result<(), CommandError> {
        if cmds.is_empty() {
            return Ok(());
        }
        let changes_before = self.data.changes.clone();
        let tx = self.journal.begin();
        for idx in 0..cmds.len() {
            // 每条命令执行前刷新反查表：批内前面的命令可能改变了引用关系。
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
                // 回滚失败命令自身 + 已应用部分（逆序 undo），保证事务失败零副作用。
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

    /// 撤销。
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
            // 用当前 undo 栈重建 journal：崩溃恢复后的状态与屏幕上一致，
            // 已撤销的编辑不会在重启后复活。
            let records = self.history.records();
            if let Err(err) = self.journal.rebase(&records) {
                tracing::warn!("journal rebase failed: {err}");
            }
        }
        Ok(done)
    }

    /// 重做。
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

    /// 清空 journal 基线（打开/下载新数据集、保存后调用）。
    pub fn reset_journal(&mut self) {
        let _ = self.journal.reset();
    }

    /// 最近一次 journal 写入失败（供 UI 告警）。
    pub fn take_journal_failure(&mut self) -> Option<String> {
        self.journal.take_failure()
    }

    /// 从 journal 帧恢复（只应用到最后一个 TxCommit）。
    pub fn recover(data: DataSet, frames: &[JournalFrame]) -> Result<(Self, usize), CommandError> {
        Self::recover_with_report(data, frames).map(|(editor, report)| (editor, report.applied))
    }

    /// 从 journal 恢复，并返回可展示的恢复报告（阶段 4：崩溃恢复可观测）。
    ///
    /// 恢复语义：
    /// * 以最后一个 `Rebase` 帧的命令为基线（没有则从 `data` 基线开始）；
    /// * 只重放最后一个完整 `TxBegin/TxCommit` 之前的 Cmd 帧；
    /// * 单条命令失败只计入 `skipped` 并继续，不再让一条坏帧毁掉整本日志。
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

/// 上传结果中的一条 ID/版本映射（与 `neo-api` 解耦，由应用层转换）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DiffMapping {
    /// 对象种类。
    pub kind: ObjectKind,
    /// 上传前的本地 ID（新建对象为负数）。
    pub old_id: ObjectId,
    /// 服务端 ID。
    pub new_id: ObjectId,
    /// 服务端 version（服务端未返回时为 `None`，保持本地版本）。
    pub new_version: Option<u32>,
}

/// journal 恢复报告。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RecoveryReport {
    /// journal 总帧数。
    pub frames: usize,
    /// 已提交事务的命令帧数。
    pub committed_transactions: usize,
    /// 成功重放并应用的命令数。
    pub applied: usize,
    /// 未闭合事务/无法解析等被丢弃的帧数。
    pub dropped: usize,
    /// 被截断/跳过的帧数（半事务、无法解析等）。
    pub skipped: usize,
}

impl Editor {
    /// 冲突解决专用：把本地对象的版本对齐到服务端版本（KeepLocal 策略）。    ///
    /// 这不是普通编辑（不写入 history/journal），只用于 409 后让“保留本地”的重传
    /// 通过 OSM 的版本校验；调用方应把它当作冲突解决动作记录在 UI 日志中。
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

    /// 应用上传结果：负 ID → 服务端 ID、写回 version、清空变更集。
    ///
    /// 上传成功后**必须**调用：否则新建对象永远保留负 ID/version=0，下一次上传
    /// 会把同一批对象再次 create（重复建对象），修改也会因版本落后永久 409。
    ///
    /// 因为 ID 变化会让历史命令与 journal 记录失效，这里同时清空历史并重置
    /// journal 基线（当前内存数据成为新的基线）。
    pub fn apply_diff_result(&mut self, entries: &[DiffMapping]) -> usize {
        self.apply_uploaded(entries, true)
    }

    /// 应用已上传的映射；`complete=false` 时保留未上传对象的变更，用于分块上传
    /// 中途失败后仍能继续提交。
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
            // 整包已上传成功：其余脏标记（含本地删除）也回到基线。
            self.data.changes.clear();
        }
        // 分块上传中途失败时，已上传部分的历史不能再用于 undo（否则会改到
        // 服务端已接受的对象）；保留剩余 changes 供下次提交。
        self.history.clear();
        self.selection.clear();
        self.data.bump_version();
        self.ensure_referrers();
        self.index = SpatialIndex::build(&self.data);
        self.world_cache = WorldCache::build(&self.data);
        self.reset_journal();
        applied
    }

    /// 用服务端对象覆盖本地冲突对象（“采用远端”）。远端数据来自独立 DataSet，
    /// `Sym` 不可跨池，因此标签/角色按字符串重新驻留。
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

    /// 采用远端几何/成员，但保留本地标签。
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

    /// 合并远端标签：本地已有 key 保留，远端独有 key 补入；几何/成员保留本地。
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

    /// 把远端数据集合并进当前图层（下载 / 图层合并共用）。
    ///
    /// * 正 ID：按 OSM id 合并——本地无则新增并保留远端 meta；本地未改则更新；本地已脏则跳过
    /// * 负 ID：始终分配新本地 ID（图层间本地对象不会误并）
    /// * 不清空历史，用户未保存编辑可继续撤销
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

    /// 从另一个 DataSet 复制选中对象到当前数据集；分配新负 ID、重映射内部引用、可选偏移。
    /// 返回实际创建的对象数。对象按 node→way→relation 顺序加入一个命令批。
    pub fn import_from_dataset(
        &mut self,
        remote: &DataSet,
        selected: &[(ObjectKind, ObjectId)],
        offset_m: f64,
    ) -> Result<usize, CommandError> {
        // 第一遍：跳过 remote 中不存在的选中项、去重，并为每类对象分配新负 ID。
        let mut mapping: HashMap<(ObjectKind, ObjectId), ObjectId> = HashMap::new();
        let mut node_ids: Vec<ObjectId> = Vec::new();
        let mut way_ids: Vec<ObjectId> = Vec::new();
        let mut relation_ids: Vec<ObjectId> = Vec::new();
        for &(kind, id) in selected {
            let exists = match kind {
                ObjectKind::Node => remote.node_handle(id).is_some(),
                ObjectKind::Way => remote.way_handle(id).is_some(),
                ObjectKind::Relation => remote.relation_handle(id).is_some(),
                // changeset 不是可复制的图元。
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

        // 节点：位置在 WebMercator 米制下偏移，标签按字符串跨池重新驻留。
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

        // Way：被复制的节点引用映射到新 ID，未复制的保留远端 ID（precheck 校验）。
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

        // Relation：成员同样映射；若成员是被复制的 relation，先加入被引用者。
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

/// 在 WebMercator 米制下平移经纬度；`offset_m == 0` 时原样返回，避免浮点往返误差。
fn offset_latlon(pos: LatLon, offset_m: f64) -> LatLon {
    if offset_m == 0.0 {
        return pos;
    }
    WorldPoint::from_latlon(pos)
        .offset(offset_m, offset_m)
        .to_latlon()
}

/// 把远端 tag 集合解析成 (key, value) 字符串对（`Sym` 不可跨池）。
fn remote_tag_strings(remote: &DataSet, tags: TagSetId) -> Vec<(String, String)> {
    remote
        .tag_pairs(tags)
        .iter()
        .map(|(key, value)| (remote.resolve(*key).to_owned(), remote.resolve(*value).to_owned()))
        .collect()
}

/// relation 导入顺序：被复制的 relation 成员先于引用者加入（保持选择顺序稳定）。
///
/// 循环引用无法在批内建立，保持原顺序交由 `execute_batch` 的 precheck 报错并整体回滚。
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

/// 某个 relation 成员中「也被复制」的 relation ID 列表。
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

/// 读取 relation 成员为可序列化描述（Editor 与 EditContext 共用）。
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

/// 合并结果计数。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MergeStats {
    /// 新并入的对象数。
    pub added: usize,
    /// 用远端覆盖的未修改对象数。
    pub updated: usize,
    /// 因本地已修改而跳过的对象数。
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

/// 便捷：世界坐标（供工具/UI 使用）。
pub fn world_of(data: &DataSet, node: ObjectId) -> Option<WorldPoint> {
    let h = data.node_handle(node)?;
    let pos = data.nodes.position(h)?;
    Some(WorldPoint::from_latlon(pos))
}
