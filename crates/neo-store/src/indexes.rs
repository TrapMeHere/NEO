//! 索引层：外部 ID → 句柄、反向引用（referrers）、变更跟踪。
//!
//! 三类索引解决三个不同问题：
//!
//! 1. [`IdIndex`]：`ObjectId -> Handle`。下载/上传/搜索/跳转都靠它。
//! 2. [`Referrers`]：反向引用（node→ways、node→relations、way→relations）。
//!    "移动一个节点要让关联 way 失效" 依赖它，见 ADR-0009。
//! 3. [`ChangeTrack`]：只记录**被编辑过**的对象状态，稀疏存储。
//!
//! `Referrers` 用 CSR（compressed sparse row）结构：一次 O(N+E) 构建后，
//! 查询是零分配的切片访问。

use hashbrown::HashMap;
use neo_core::{ByteSize, Handle, ObjectId, ObjectKind};
use neo_osm::ChangeState;
use rustc_hash::FxBuildHasher;

use crate::dataset::DataSet;
use crate::node_store::NodeSlot;
use crate::relation_store::{AnyHandle, RelationSlot};
use crate::way_store::WaySlot;

/// 外部 ID 到句柄的映射。
#[derive(Debug, Default)]
pub struct IdIndex {
    nodes: HashMap<ObjectId, Handle<NodeSlot>, FxBuildHasher>,
    ways: HashMap<ObjectId, Handle<WaySlot>, FxBuildHasher>,
    relations: HashMap<ObjectId, Handle<RelationSlot>, FxBuildHasher>,
}

impl IdIndex {
    /// 空索引。
    pub fn new() -> Self {
        Self::default()
    }

    /// 插入 node 映射。
    pub fn insert_node(&mut self, id: ObjectId, handle: Handle<NodeSlot>) -> Option<Handle<NodeSlot>> {
        self.nodes.insert(id, handle)
    }

    /// 插入 way 映射。
    pub fn insert_way(&mut self, id: ObjectId, handle: Handle<WaySlot>) -> Option<Handle<WaySlot>> {
        self.ways.insert(id, handle)
    }

    /// 插入 relation 映射。
    pub fn insert_relation(
        &mut self,
        id: ObjectId,
        handle: Handle<RelationSlot>,
    ) -> Option<Handle<RelationSlot>> {
        self.relations.insert(id, handle)
    }

    /// 查 node。
    #[inline]
    pub fn node(&self, id: ObjectId) -> Option<Handle<NodeSlot>> {
        self.nodes.get(&id).copied()
    }

    /// 查 way。
    #[inline]
    pub fn way(&self, id: ObjectId) -> Option<Handle<WaySlot>> {
        self.ways.get(&id).copied()
    }

    /// 查 relation。
    #[inline]
    pub fn relation(&self, id: ObjectId) -> Option<Handle<RelationSlot>> {
        self.relations.get(&id).copied()
    }

    /// 删除 node 映射。
    pub fn remove_node(&mut self, id: ObjectId) -> Option<Handle<NodeSlot>> {
        self.nodes.remove(&id)
    }

    /// 删除 way 映射。
    pub fn remove_way(&mut self, id: ObjectId) -> Option<Handle<WaySlot>> {
        self.ways.remove(&id)
    }

    /// 删除 relation 映射。
    pub fn remove_relation(&mut self, id: ObjectId) -> Option<Handle<RelationSlot>> {
        self.relations.remove(&id)
    }

    /// 各类对象数量。
    pub fn counts(&self) -> (usize, usize, usize) {
        (self.nodes.len(), self.ways.len(), self.relations.len())
    }

    /// 估算内存。
    pub fn mem_bytes(&self) -> ByteSize {
        // FxHashMap：key(8) + value(8) + 控制字节(1)，按 1.2 倍装载余量。
        let entry = (core::mem::size_of::<ObjectId>() + 8 + 1) as f64 * 1.2;
        let total = self.nodes.len() + self.ways.len() + self.relations.len();
        ByteSize((total as f64 * entry) as usize)
    }
}

/// CSR（压缩稀疏行）反向索引。
#[derive(Debug)]
struct Csr<T> {
    offsets: Vec<u32>,
    values: Vec<T>,
}

impl<T: Copy> Csr<T> {
    fn build(
        len: usize,
        counts: &[u32],
        placeholder: T,
        emit: impl FnOnce(&mut dyn FnMut(usize, T)),
    ) -> Self {
        let mut offsets = Vec::with_capacity(len + 1);
        offsets.push(0);
        let mut acc = 0u32;
        for c in counts {
            acc = acc.checked_add(*c).expect("CSR total exceeds u32");
            offsets.push(acc);
        }
        let total = acc as usize;
        let mut values = vec![placeholder; total];
        let mut cursors = offsets[..len].to_vec();
        {
            let mut push = |bucket: usize, value: T| {
                let pos = cursors[bucket] as usize;
                values[pos] = value;
                cursors[bucket] += 1;
            };
            emit(&mut push);
        }
        Self { offsets, values }
    }

    #[inline]
    fn get(&self, index: usize) -> &[T] {
        if index + 1 >= self.offsets.len() {
            return &[];
        }
        let start = self.offsets[index] as usize;
        let end = self.offsets[index + 1] as usize;
        &self.values[start..end]
    }

    fn mem_bytes(&self) -> usize {
        self.offsets.capacity() * core::mem::size_of::<u32>()
            + self.values.capacity() * core::mem::size_of::<T>()
    }
}

/// 反向引用索引。
///
/// 只包含**可见**对象之间的引用：已逻辑删除的 way/relation 不应再阻止节点删除，
/// 否则"先删 way 再删其孤儿节点"的标准流程会永久失败。
#[derive(Debug)]
pub struct Referrers {
    node_ways: Csr<Handle<WaySlot>>,
    node_relations: Csr<Handle<RelationSlot>>,
    way_relations: Csr<Handle<RelationSlot>>,
    relation_relations: Csr<Handle<RelationSlot>>,
}

/// Way 的可见引用（跳过相邻/闭合重复点与失效句柄）。
fn for_each_way_ref(data: &DataSet, mut f: impl FnMut(usize, Handle<WaySlot>, Handle<NodeSlot>)) {
    for way in data.ways.iter() {
        if !way.meta.visible {
            continue;
        }
        let Some(refs) = data.ways.nodes(way.handle) else {
            continue;
        };
        for (i, h) in refs.iter().enumerate() {
            if !data.nodes.contains(*h) || h.index() as usize >= data.nodes.slot_len() {
                continue;
            }
            if i > 0 && refs[i - 1] == *h {
                continue;
            }
            // 闭合 way 的首尾是同一个节点：同一 way 只计一次引用。
            if i + 1 == refs.len() && refs.len() > 2 && refs.first() == Some(h) {
                continue;
            }
            f(h.index() as usize, way.handle, *h);
        }
    }
}

/// Relation 的可见成员（跳过失效目标）。
fn for_each_relation_member(data: &DataSet, mut f: impl FnMut(Handle<RelationSlot>, ObjectKind, AnyHandle)) {
    for rel in data.relations.iter() {
        if !rel.meta.visible {
            continue;
        }
        let Some(members) = data.relations.members_of(rel.handle) else {
            continue;
        };
        for m in members {
            let valid = match m.target.kind {
                ObjectKind::Node => data
                    .nodes
                    .contains(m.target.as_node().unwrap_or(Handle::new(u32::MAX, 0))),
                ObjectKind::Way => data
                    .ways
                    .contains(m.target.as_way().unwrap_or(Handle::new(u32::MAX, 0))),
                ObjectKind::Relation => data
                    .relations
                    .contains(m.target.as_relation().unwrap_or(Handle::new(u32::MAX, 0))),
                ObjectKind::Changeset => false,
            };
            if valid {
                f(rel.handle, m.target.kind, m.target);
            }
        }
    }
}

impl Referrers {
    /// 从数据集全量构建，O(N + E)。
    pub fn build(data: &DataSet) -> Self {
        let node_slots = data.nodes.slot_len();
        let way_slots = data.ways.slot_len();
        let relation_slots = data.relations.slot_len();

        // node -> ways
        let mut node_way_counts = vec![0u32; node_slots];
        for_each_way_ref(data, |node, _, _| {
            if let Some(c) = node_way_counts.get_mut(node) {
                *c += 1;
            }
        });
        let node_ways = Csr::build(node_slots, &node_way_counts, Handle::new(0, 0), |push| {
            for_each_way_ref(data, |node, way, _| push(node, way));
        });

        // node -> relations
        let mut node_rel_counts = vec![0u32; node_slots];
        // way -> relations
        let mut way_rel_counts = vec![0u32; way_slots];
        // relation -> relations
        let mut rel_rel_counts = vec![0u32; relation_slots];
        for_each_relation_member(data, |_, kind, target| match kind {
            ObjectKind::Node => {
                if let Some(c) = node_rel_counts.get_mut(target.index as usize) {
                    *c += 1;
                }
            }
            ObjectKind::Way => {
                if let Some(c) = way_rel_counts.get_mut(target.index as usize) {
                    *c += 1;
                }
            }
            ObjectKind::Relation => {
                if let Some(c) = rel_rel_counts.get_mut(target.index as usize) {
                    *c += 1;
                }
            }
            ObjectKind::Changeset => {}
        });

        let node_relations = Csr::build(node_slots, &node_rel_counts, Handle::new(0, 0), |push| {
            for_each_relation_member(data, |rel, kind, target| {
                if kind == ObjectKind::Node {
                    push(target.index as usize, rel);
                }
            });
        });
        let way_relations = Csr::build(way_slots, &way_rel_counts, Handle::new(0, 0), |push| {
            for_each_relation_member(data, |rel, kind, target| {
                if kind == ObjectKind::Way {
                    push(target.index as usize, rel);
                }
            });
        });
        let relation_relations = Csr::build(relation_slots, &rel_rel_counts, Handle::new(0, 0), |push| {
            for_each_relation_member(data, |rel, kind, target| {
                if kind == ObjectKind::Relation {
                    push(target.index as usize, rel);
                }
            });
        });

        Self {
            node_ways,
            node_relations,
            way_relations,
            relation_relations,
        }
    }

    /// 引用某节点的 way。
    #[inline]
    pub fn ways_of_node(&self, node: Handle<NodeSlot>) -> &[Handle<WaySlot>] {
        self.node_ways.get(node.index() as usize)
    }

    /// 引用某节点的 relation。
    #[inline]
    pub fn relations_of_node(&self, node: Handle<NodeSlot>) -> &[Handle<RelationSlot>] {
        self.node_relations.get(node.index() as usize)
    }

    /// 引用某 way 的 relation。
    #[inline]
    pub fn relations_of_way(&self, way: Handle<WaySlot>) -> &[Handle<RelationSlot>] {
        self.way_relations.get(way.index() as usize)
    }

    /// 引用某 relation 的父 relation（删除检查必需）。
    #[inline]
    pub fn relations_of_relation(&self, relation: Handle<RelationSlot>) -> &[Handle<RelationSlot>] {
        self.relation_relations.get(relation.index() as usize)
    }

    /// 估算内存。
    pub fn mem_bytes(&self) -> ByteSize {
        ByteSize(
            self.node_ways.mem_bytes()
                + self.node_relations.mem_bytes()
                + self.way_relations.mem_bytes()
                + self.relation_relations.mem_bytes(),
        )
    }
}

/// 变更跟踪：只记录被编辑过的对象。
#[derive(Debug, Default, Clone)]
pub struct ChangeTrack {
    nodes: HashMap<ObjectId, ChangeState, FxBuildHasher>,
    ways: HashMap<ObjectId, ChangeState, FxBuildHasher>,
    relations: HashMap<ObjectId, ChangeState, FxBuildHasher>,
}

impl ChangeTrack {
    /// 清空（上传成功后调用）。
    pub fn clear(&mut self) {
        self.nodes.clear();
        self.ways.clear();
        self.relations.clear();
    }

    /// 记录状态；`Unchanged` 表示移除记录。
    pub fn mark(&mut self, kind: ObjectKind, id: ObjectId, state: ChangeState) {
        let map = match kind {
            ObjectKind::Node => &mut self.nodes,
            ObjectKind::Way => &mut self.ways,
            ObjectKind::Relation => &mut self.relations,
            ObjectKind::Changeset => return,
        };
        if state == ChangeState::Unchanged {
            map.remove(&id);
        } else {
            map.insert(id, state);
        }
    }

    /// 读取状态。
    pub fn state(&self, kind: ObjectKind, id: ObjectId) -> ChangeState {
        let map = match kind {
            ObjectKind::Node => &self.nodes,
            ObjectKind::Way => &self.ways,
            ObjectKind::Relation => &self.relations,
            ObjectKind::Changeset => return ChangeState::Unchanged,
        };
        map.get(&id).copied().unwrap_or_default()
    }

    /// 是否有待上传修改。
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty() && self.ways.is_empty() && self.relations.is_empty()
    }

    /// 待上传对象总数。
    pub fn len(&self) -> usize {
        self.nodes.len() + self.ways.len() + self.relations.len()
    }

    /// 迭代某类脏对象（按 `ObjectId` 升序，保证 OSC 导出可复现）。
    pub fn iter(&self, kind: ObjectKind) -> Vec<(ObjectId, ChangeState)> {
        let mut out: Vec<(ObjectId, ChangeState)> = match kind {
            ObjectKind::Node => self.nodes.iter().map(|(k, v)| (*k, *v)).collect(),
            ObjectKind::Way => self.ways.iter().map(|(k, v)| (*k, *v)).collect(),
            ObjectKind::Relation => self.relations.iter().map(|(k, v)| (*k, *v)).collect(),
            ObjectKind::Changeset => Vec::new(),
        };
        out.sort_unstable_by_key(|(id, _)| id.0);
        out
    }

    /// 估算内存。
    pub fn mem_bytes(&self) -> ByteSize {
        let n = self.nodes.len() + self.ways.len() + self.relations.len();
        ByteSize(n * (core::mem::size_of::<ObjectId>() + 1 + 16))
    }
}
