//! `DataSet`：一个 OSM 数据集的完整内存表示。
//!
//! 组成：
//!
//! * [`StringPool`] + [`TagStore`]：所有文本与 tag 的唯一归属地；
//! * [`NodeStore`] / [`WayStore`] / [`RelationStore`]：SoA 数据；
//! * [`IdIndex`]：外部 ID 到句柄；
//! * [`ChangeTrack`]：待上传修改；
//! * 脏位图：给渲染/几何/验证做增量更新的依据（见 ADR-0009）。
//!
//! **线程模型**：`DataSet` 只由单写者（编辑器主线程/命令执行上下文）持有 `&mut`。
//! 渲染和后台任务读的是发布出去的不可变快照（`Arc<...>`），见 docs/07。

use neo_core::{ByteSize, Handle, LatLon, Meta, NeoError, ObjectId, ObjectKind, Result, StringPool, Sym};
use neo_osm::{ChangeState, Node, Relation, Tags, Way};

use crate::bitset::BitSet;
use crate::indexes::{ChangeTrack, IdIndex, Referrers};
use crate::node_store::{NodeRef, NodeSlot, NodeStore};
use crate::relation_store::{AnyHandle, RelationRef, RelationSlot, RelationStore, StoredMember};
use crate::tag_store::{TagSetId, TagStore};
use crate::way_store::{WayRef, WaySlot, WayStore};

/// 导入策略：遇到悬空引用时跳过还是报错。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MissingPolicy {
    /// 跳过该对象（记录计数）。用于 partial data 与不完整下载。
    Skip,
    /// 返回 [`NeoError::DanglingReference`]。用于完整性校验。
    Error,
}

/// Node 的完整视图（含 tag 内容）。
#[derive(Clone, Copy, Debug)]
pub struct NodeView<'a> {
    /// 快照信息。
    pub info: NodeRef,
    /// tag 对切片。
    pub tag_pairs: &'a [(Sym, Sym)],
}

impl NodeView<'_> {
    /// 外部 ID。
    #[inline]
    pub fn id(&self) -> ObjectId {
        self.info.id
    }

    /// 坐标。
    #[inline]
    pub fn pos(&self) -> LatLon {
        self.info.pos
    }

    /// 元数据。
    #[inline]
    pub fn meta(&self) -> Meta {
        self.info.meta
    }

    /// 查 tag。
    #[inline]
    pub fn tag(&self, key: Sym) -> Option<Sym> {
        self.tag_pairs.iter().find_map(|&(k, v)| (k == key).then_some(v))
    }

    /// 迭代 tag。
    #[inline]
    pub fn tags(&self) -> impl Iterator<Item = (Sym, Sym)> + '_ {
        self.tag_pairs.iter().copied()
    }
}

/// Way 的完整视图。
#[derive(Clone, Copy, Debug)]
pub struct WayView<'a> {
    /// 快照信息。
    pub info: WayRef,
    /// tag 对切片。
    pub tag_pairs: &'a [(Sym, Sym)],
    /// 节点引用。
    pub node_refs: &'a [Handle<NodeSlot>],
}

impl WayView<'_> {
    /// 外部 ID。
    #[inline]
    pub fn id(&self) -> ObjectId {
        self.info.id
    }

    /// 查 tag。
    #[inline]
    pub fn tag(&self, key: Sym) -> Option<Sym> {
        self.tag_pairs.iter().find_map(|&(k, v)| (k == key).then_some(v))
    }

    /// 迭代 tag。
    #[inline]
    pub fn tags(&self) -> impl Iterator<Item = (Sym, Sym)> + '_ {
        self.tag_pairs.iter().copied()
    }

    /// 是否闭合（首尾引用相同）。
    pub fn is_closed(&self) -> bool {
        match (self.node_refs.first(), self.node_refs.last()) {
            (Some(a), Some(b)) => a == b && self.node_refs.len() >= 4,
            _ => false,
        }
    }
}

/// Relation 的完整视图。
#[derive(Clone, Copy, Debug)]
pub struct RelationView<'a> {
    /// 快照信息。
    pub info: RelationRef,
    /// tag 对切片。
    pub tag_pairs: &'a [(Sym, Sym)],
    /// 成员切片。
    pub members: &'a [StoredMember],
}

impl RelationView<'_> {
    /// 外部 ID。
    #[inline]
    pub fn id(&self) -> ObjectId {
        self.info.id
    }

    /// 查 tag。
    #[inline]
    pub fn tag(&self, key: Sym) -> Option<Sym> {
        self.tag_pairs.iter().find_map(|&(k, v)| (k == key).then_some(v))
    }
}

/// 一个 OSM 数据集。
#[derive(Debug)]
pub struct DataSet {
    /// 字符串驻留池。
    pub strings: StringPool,
    /// tag 竞技场。
    pub tags: TagStore,
    /// 节点。
    pub nodes: NodeStore,
    /// Way。
    pub ways: WayStore,
    /// Relation。
    pub relations: RelationStore,
    /// 外部 ID 索引。
    pub ids: IdIndex,
    /// 待上传修改。
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
    /// 空数据集。
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

    /// 当前数据版本（每次提交编辑递增；渲染/缓存用它做失效判断）。
    #[inline]
    pub fn version(&self) -> u64 {
        self.version
    }

    /// 递增版本。
    #[inline]
    pub fn bump_version(&mut self) {
        self.version = self.version.wrapping_add(1);
    }

    /// 分配本地新建对象 ID（负数，从 -1 递减）。
    ///
    /// 会跳过任何已被占用的 ID：导入含负 ID 的 XML/OSC 后，计数器若不前进，
    /// 新建对象会与已存在对象撞号并静默覆盖 `IdIndex`（数据损坏）。
    pub fn next_local_id(&mut self) -> ObjectId {
        loop {
            let id = ObjectId(self.next_local_id);
            self.next_local_id -= 1;
            if self.ids.node(id).is_none() && self.ids.way(id).is_none() && self.ids.relation(id).is_none() {
                return id;
            }
        }
    }

    /// 记录一个已存在的负 ID，保证后续 `next_local_id` 不会回绕到它。
    fn note_existing_local_id(&mut self, id: ObjectId) {
        if id.is_local() && id.0 <= self.next_local_id {
            self.next_local_id = id.0 - 1;
        }
    }

    /// 字符串驻留。
    #[inline]
    pub fn intern(&mut self, s: &str) -> Sym {
        self.strings.intern(s)
    }

    /// 驻留 tag key/value。
    #[inline]
    pub fn intern_tag(&mut self, key: &str, value: &str) -> (Sym, Sym) {
        (self.strings.intern(key), self.strings.intern(value))
    }

    /// 解析符号。
    #[inline]
    pub fn resolve(&self, sym: Sym) -> &str {
        self.strings.resolve(sym)
    }

    /// 新增 tag 集合。
    pub fn add_tags(&mut self, pairs: &[(Sym, Sym)]) -> TagSetId {
        self.tags.add(pairs)
    }

    /// 取 tag 集合内容。
    #[inline]
    pub fn tag_pairs(&self, id: TagSetId) -> &[(Sym, Sym)] {
        self.tags.get(id)
    }

    /// 是否空数据集。
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty() && self.ways.is_empty() && self.relations.is_empty()
    }

    /// 节点数。
    #[inline]
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    /// Way 数。
    #[inline]
    pub fn way_count(&self) -> usize {
        self.ways.len()
    }

    /// Relation 数。
    #[inline]
    pub fn relation_count(&self) -> usize {
        self.relations.len()
    }

    /// 插入一个 node DTO。
    ///
    /// 拒绝重复 ID：`IdIndex` 是 `HashMap`，静默覆盖会让旧对象变成"存活但无 ID"
    /// 的孤儿（渲染/导出出现重复 ID，上传被服务端拒绝）。
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

    /// 插入一个 way DTO。
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

    /// 插入一个 relation DTO。
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

    /// 插入一个 `Element` DTO。返回被接受对象的 ID（被跳过时返回 `None`）。
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

    /// Node 完整视图。
    pub fn node_view(&self, handle: Handle<NodeSlot>) -> Option<NodeView<'_>> {
        let info = self.nodes.get(handle)?;
        Some(NodeView {
            info,
            tag_pairs: self.tags.get(info.tags),
        })
    }

    /// Way 完整视图。
    pub fn way_view(&self, handle: Handle<WaySlot>) -> Option<WayView<'_>> {
        let info = self.ways.get(handle)?;
        Some(WayView {
            info,
            tag_pairs: self.tags.get(info.tags),
            node_refs: self.ways.nodes(handle)?,
        })
    }

    /// Relation 完整视图。
    pub fn relation_view(&self, handle: Handle<RelationSlot>) -> Option<RelationView<'_>> {
        let info = self.relations.get(handle)?;
        Some(RelationView {
            info,
            tag_pairs: self.tags.get(info.tags),
            members: self.relations.members_of(handle)?,
        })
    }

    /// 节点坐标。
    #[inline]
    pub fn position(&self, handle: Handle<NodeSlot>) -> Option<LatLon> {
        self.nodes.position(handle)
    }

    /// 按 ID 查节点句柄。
    #[inline]
    pub fn node_handle(&self, id: ObjectId) -> Option<Handle<NodeSlot>> {
        self.ids.node(id)
    }

    /// 按 ID 查 way 句柄。
    #[inline]
    pub fn way_handle(&self, id: ObjectId) -> Option<Handle<WaySlot>> {
        self.ids.way(id)
    }

    /// 按 ID 查 relation 句柄。
    #[inline]
    pub fn relation_handle(&self, id: ObjectId) -> Option<Handle<RelationSlot>> {
        self.ids.relation(id)
    }

    /// Way 的节点 ID 序列（用于展示；失效引用会被跳过）。
    ///
    /// 写路径（OSC/PBF 导出、编辑命令）必须用 [`Self::try_way_node_ids`]：
    /// 静默跳过悬空引用会把几何写短，属不可逆数据损坏。
    pub fn way_node_ids(&self, handle: Handle<WaySlot>) -> Vec<ObjectId> {
        let Some(refs) = self.ways.nodes(handle) else {
            return Vec::new();
        };
        refs.iter().filter_map(|h| self.nodes.id(*h)).collect()
    }

    /// Way 的节点 ID 序列；任一引用失效时返回错误而不是静默压缩。
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

    /// 修改节点坐标（编辑热路径）：校验 → 写入 → 标脏 → 版本递增。
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

    /// 替换 Way 的节点序列。
    ///
    /// 校验每个节点句柄仍然有效：否则会把悬空句柄写进引用池，后续
    /// `Referrers`/`WorldCache` 按 index 访问时越界或张冠李戴。
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

    /// 把某个对象标记为待上传修改。
    pub fn set_change_state(&mut self, kind: ObjectKind, id: ObjectId, state: ChangeState) {
        self.changes.mark(kind, id, state);
    }

    /// 读取对象修改状态。
    pub fn change_state(&self, kind: ObjectKind, id: ObjectId) -> ChangeState {
        self.changes.state(kind, id)
    }

    /// 同时设置修改状态与 `visible` 元数据（删除/恢复删除用）。
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
        // 可见性影响渲染/拾取/上传，必须让缓存失效。
        self.bump_version();
    }

    /// 把对象的外部 ID 从 `old` 改为 `new`（上传成功后负 ID → 服务端 ID）。
    ///
    /// 引用方（way 的 nd、relation 成员）存的是句柄，因此不需要改写；只有
    /// `IdIndex` 与对象自身的 ID 需要更新。
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

    /// 修改对象版本（上传成功后写回服务端 version）。
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

    /// 标记某个对象已同步（上传成功后清除脏状态）。
    pub fn mark_synced(&mut self, kind: ObjectKind, id: ObjectId) {
        self.changes.mark(kind, id, ChangeState::Unchanged);
    }

    /// 全部变更是否已清空。
    pub fn has_pending_changes(&self) -> bool {
        !self.changes.is_empty()
    }

    // ---- 脏集合 ----

    /// 标记 way 的几何/样式需要重建。
    #[inline]
    pub fn mark_way_dirty(&mut self, handle: Handle<WaySlot>) {
        self.dirty_ways.set(handle.index() as usize);
    }

    /// 标记节点脏（节点图标/拾取索引）。
    #[inline]
    pub fn mark_node_dirty(&mut self, handle: Handle<NodeSlot>) {
        self.dirty_nodes.set(handle.index() as usize);
    }

    /// 标记 relation 脏（成员/标签变化）。
    #[inline]
    pub fn mark_relation_dirty(&mut self, handle: Handle<RelationSlot>) {
        self.dirty_relations.set(handle.index() as usize);
    }

    /// 取出并清空脏 way 列表。
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

    /// 取出并清空脏节点列表。
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

    /// 是否有待重建的几何。
    pub fn has_dirty(&self) -> bool {
        self.dirty_nodes.count_ones() > 0 || self.dirty_ways.count_ones() > 0
    }

    /// 构建反向引用索引。
    pub fn build_referrers(&self) -> Referrers {
        Referrers::build(self)
    }

    /// 统计信息。
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

    /// 估算堆内存。
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

    /// 迭代所有节点快照。
    pub fn iter_nodes(&self) -> impl Iterator<Item = NodeRef> + '_ {
        self.nodes.iter()
    }

    /// 迭代所有 way 快照。
    pub fn iter_ways(&self) -> impl Iterator<Item = WayRef> + '_ {
        self.ways.iter()
    }

    /// 迭代所有 relation 快照。
    pub fn iter_relations(&self) -> impl Iterator<Item = RelationRef> + '_ {
        self.relations.iter()
    }

    /// 把 TagSet 解析为 DTO。
    pub fn tags_of(&self, id: TagSetId) -> Tags {
        Tags::from_pairs(self.tags.iter(id))
    }
}

/// 数据集统计（用于基准与 UI 状态栏）。
#[derive(Clone, Debug, Default)]
pub struct StoreStats {
    /// 节点数。
    pub nodes: usize,
    /// Way 数。
    pub ways: usize,
    /// Relation 数。
    pub relations: usize,
    /// 节点引用总数。
    pub node_refs: usize,
    /// relation 成员总数。
    pub relation_members: usize,
    /// tag set 数。
    pub tag_sets: usize,
    /// tag 对总数。
    pub tags: usize,
    /// 驻留字符串数。
    pub strings: usize,
    /// 估算内存。
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
        // 即使计数器被重置，也不允许复用已有 ID。
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
        // 物理删除节点后，旧句柄不应再能写进 way。
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
