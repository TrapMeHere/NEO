//! 空间索引：rstar R-tree（**按对象类型分树**）。
//!
//! 双层索引中的**对象层**：节点（点）、Way（预计算 AABB）。Relation 不直接进空间索引
//! ——它没有自己的坐标，bbox 由成员派生（见 docs/04）。
//!
//! 为什么 node 和 way 各一棵树：
//! 城市级缩放下"所有 way 都在视口内"是常态，如果两种对象混在一棵树里，只遍历 way 时
//! 仍然要扫描全部 node 条目（原型实测：200 万条目视角下多花约 100 ms）。分树后
//! `for_each_way` 只碰 way 树，遍历量降到 1/6。
//!
//! 构建策略：
//!
//! * 导入/大批量变更后用 `RTree::bulk_load`（O(n log n)，比逐个 insert 快一个量级）；
//! * 交互编辑时用 [`SpatialIndex::update_way`] / [`SpatialIndex::update_node`] 增量更新；
//! * `RTree::remove` 需要"相同元素"，因此额外维护 `entries` 映射拿到旧 bbox。

use std::collections::HashMap;

use neo_core::{ByteSize, Handle, ObjectKind};
use neo_store::{DataSet, NodeSlot, WaySlot};
use rstar::{AABB, RTree, RTreeObject};

use crate::geometry;
use crate::world::{WorldBounds, WorldPoint};

const KIND_BITS: u64 = 2;

fn key(kind: ObjectKind, slot: u32) -> u64 {
    ((kind as u64) << 32) | u64::from(slot)
}

/// 空间索引条目。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpatialEntry {
    /// 对象种类（Node / Way）。
    pub kind: ObjectKind,
    /// 槽位下标（配合 generation 还原句柄）。
    pub slot: u32,
    /// 槽位代号。
    pub generation: u32,
    /// 世界坐标包围盒。
    pub bounds: WorldBounds,
}

impl SpatialEntry {
    /// 节点句柄（若不是节点返回 `None`）。
    pub fn as_node(self) -> Option<Handle<NodeSlot>> {
        (self.kind == ObjectKind::Node).then(|| Handle::new(self.slot, self.generation))
    }

    /// Way 句柄。
    pub fn as_way(self) -> Option<Handle<WaySlot>> {
        (self.kind == ObjectKind::Way).then(|| Handle::new(self.slot, self.generation))
    }

    /// 命中测试优先级：节点优先于 way。
    pub fn priority(self) -> u8 {
        match self.kind {
            ObjectKind::Node => 0,
            ObjectKind::Way => 1,
            _ => 2,
        }
    }
}

impl RTreeObject for SpatialEntry {
    type Envelope = AABB<[f64; 2]>;

    fn envelope(&self) -> Self::Envelope {
        AABB::from_corners(
            [self.bounds.min.x, self.bounds.min.y],
            [self.bounds.max.x, self.bounds.max.y],
        )
    }
}

/// 对象空间索引（node / way 分树）。
#[derive(Debug)]
pub struct SpatialIndex {
    node_tree: RTree<SpatialEntry>,
    way_tree: RTree<SpatialEntry>,
    /// slot key -> 已注册条目（用于增量删除）。
    entries: HashMap<u64, SpatialEntry>,
}

impl Default for SpatialIndex {
    fn default() -> Self {
        Self::new()
    }
}

impl SpatialIndex {
    /// 空索引。
    pub fn new() -> Self {
        Self {
            node_tree: RTree::new(),
            way_tree: RTree::new(),
            entries: HashMap::new(),
        }
    }

    /// 条目总数。
    pub fn len(&self) -> usize {
        self.node_tree.size() + self.way_tree.size()
    }

    /// 是否为空。
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// 节点条目数。
    pub fn node_count(&self) -> usize {
        self.node_tree.size()
    }

    /// Way 条目数。
    pub fn way_count(&self) -> usize {
        self.way_tree.size()
    }

    /// 从数据集全量构建。
    pub fn build(data: &DataSet) -> Self {
        let mut nodes = Vec::with_capacity(data.node_count());
        let mut ways = Vec::with_capacity(data.way_count());

        for node in data.iter_nodes() {
            // 服务端已删除（visible=false）的对象不进入空间索引：否则导入历史
            // 数据后仍能被拾取/渲染。
            if !node.meta.visible {
                continue;
            }
            let wp = WorldPoint::from_latlon(node.pos);
            nodes.push(SpatialEntry {
                kind: ObjectKind::Node,
                slot: node.handle.index(),
                generation: node.handle.generation(),
                bounds: WorldBounds::from_point(wp),
            });
        }
        for way in data.iter_ways() {
            if !way.meta.visible {
                continue;
            }
            if let Some(bounds) = geometry::way_bounds(data, way.handle) {
                ways.push(SpatialEntry {
                    kind: ObjectKind::Way,
                    slot: way.handle.index(),
                    generation: way.handle.generation(),
                    bounds,
                });
            }
        }

        let mut map = HashMap::with_capacity(nodes.len() + ways.len());
        for e in nodes.iter().chain(ways.iter()) {
            map.insert(key(e.kind, e.slot), *e);
        }
        Self {
            node_tree: RTree::bulk_load(nodes),
            way_tree: RTree::bulk_load(ways),
            entries: map,
        }
    }

    /// 包围盒查询（返回所有相交条目）。
    pub fn query(&self, bounds: WorldBounds) -> Vec<SpatialEntry> {
        let env = AABB::from_corners([bounds.min.x, bounds.min.y], [bounds.max.x, bounds.max.y]);
        let mut out: Vec<SpatialEntry> = self
            .node_tree
            .locate_in_envelope_intersecting(env)
            .copied()
            .collect();
        out.extend(self.way_tree.locate_in_envelope_intersecting(env).copied());
        out
    }

    /// 零分配遍历：对包围盒内每个条目调用 `f`（不构造中间 Vec）。
    pub fn for_each_in<F>(&self, bounds: WorldBounds, mut f: F)
    where
        F: FnMut(&SpatialEntry),
    {
        let env = AABB::from_corners([bounds.min.x, bounds.min.y], [bounds.max.x, bounds.max.y]);
        for entry in self.node_tree.locate_in_envelope_intersecting(env) {
            f(entry);
        }
        for entry in self.way_tree.locate_in_envelope_intersecting(env) {
            f(entry);
        }
    }

    /// 只遍历 Way（不触碰 node 树）。
    pub fn for_each_way<F>(&self, bounds: WorldBounds, mut f: F)
    where
        F: FnMut(&SpatialEntry),
    {
        let env = AABB::from_corners([bounds.min.x, bounds.min.y], [bounds.max.x, bounds.max.y]);
        for entry in self.way_tree.locate_in_envelope_intersecting(env) {
            f(entry);
        }
    }

    /// 只遍历 Node。
    pub fn for_each_node<F>(&self, bounds: WorldBounds, mut f: F)
    where
        F: FnMut(&SpatialEntry),
    {
        let env = AABB::from_corners([bounds.min.x, bounds.min.y], [bounds.max.x, bounds.max.y]);
        for entry in self.node_tree.locate_in_envelope_intersecting(env) {
            f(entry);
        }
    }

    /// 只查询 Way。
    pub fn query_ways(&self, bounds: WorldBounds) -> Vec<SpatialEntry> {
        let env = AABB::from_corners([bounds.min.x, bounds.min.y], [bounds.max.x, bounds.max.y]);
        self.way_tree
            .locate_in_envelope_intersecting(env)
            .copied()
            .collect()
    }

    /// 只查询 Node。
    pub fn query_nodes(&self, bounds: WorldBounds) -> Vec<SpatialEntry> {
        let env = AABB::from_corners([bounds.min.x, bounds.min.y], [bounds.max.x, bounds.max.y]);
        self.node_tree
            .locate_in_envelope_intersecting(env)
            .copied()
            .collect()
    }

    /// 增量更新节点。
    pub fn update_node(&mut self, handle: Handle<NodeSlot>, node: neo_core::LatLon) {
        self.remove_key(ObjectKind::Node, handle.index());
        let wp = WorldPoint::from_latlon(node);
        self.insert_entry(SpatialEntry {
            kind: ObjectKind::Node,
            slot: handle.index(),
            generation: handle.generation(),
            bounds: WorldBounds::from_point(wp),
        });
    }

    /// 增量更新一条 way 的 bbox。
    pub fn update_way(&mut self, data: &DataSet, handle: Handle<WaySlot>) -> bool {
        self.remove_key(ObjectKind::Way, handle.index());
        let Some(bounds) = geometry::way_bounds(data, handle) else {
            return false;
        };
        self.insert_entry(SpatialEntry {
            kind: ObjectKind::Way,
            slot: handle.index(),
            generation: handle.generation(),
            bounds,
        });
        true
    }

    /// 删除某对象条目。
    pub fn remove(&mut self, kind: ObjectKind, slot: u32) -> bool {
        self.remove_key(kind, slot)
    }

    fn remove_key(&mut self, kind: ObjectKind, slot: u32) -> bool {
        let key = key(kind, slot);
        match self.entries.remove(&key) {
            Some(old) => {
                self.tree_mut(kind).remove(&old);
                true
            }
            None => false,
        }
    }

    fn tree_mut(&mut self, kind: ObjectKind) -> &mut RTree<SpatialEntry> {
        match kind {
            ObjectKind::Node => &mut self.node_tree,
            _ => &mut self.way_tree,
        }
    }

    fn insert_entry(&mut self, entry: SpatialEntry) {
        self.entries.insert(key(entry.kind, entry.slot), entry);
        self.tree_mut(entry.kind).insert(entry);
    }

    /// 估算内存。
    pub fn mem_bytes(&self) -> ByteSize {
        ByteSize(
            self.entries.capacity()
                * (core::mem::size_of::<u64>() + core::mem::size_of::<SpatialEntry>() + 8)
                + self.len() * core::mem::size_of::<SpatialEntry>(),
        )
    }
}

const _: () = assert!(KIND_BITS == 2);
