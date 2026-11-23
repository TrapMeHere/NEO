//! 节点世界坐标缓存。
//!
//! WebMercator 投影包含 `tan/ln/cos`，单次约 100 ns 量级；一条 way 有 2–10 个节点，
//! 而场景构建每帧要处理数万到数十万条 way。如果每次几何构建都重新投影，投影本身
//! 会成为最大开销（原型实测确认了这一点）。
//!
//! 因此把"节点 → 世界坐标"一次性算好（16 B/节点），几何构建只做查表。
//! 编辑节点坐标时 `update()` 单点失效，O(1)。这是 ADR-0009 增量策略的一部分。

use neo_core::Handle;
use neo_store::{DataSet, NodeSlot};

use crate::world::WorldPoint;

/// 节点世界坐标缓存（按 node 槽位下标索引）。
#[derive(Debug)]
pub struct WorldCache {
    positions: Vec<Option<WorldPoint>>,
}

impl WorldCache {
    /// 从数据集全量构建，O(n)。
    pub fn build(data: &DataSet) -> Self {
        let mut positions = vec![None; data.nodes.slot_len()];
        for node in data.iter_nodes() {
            positions[node.handle.index() as usize] = Some(WorldPoint::from_latlon(node.pos));
        }
        Self { positions }
    }

    /// 查节点世界坐标。
    #[inline]
    pub fn get(&self, handle: Handle<NodeSlot>) -> Option<WorldPoint> {
        self.positions.get(handle.index() as usize).copied().flatten()
    }

    /// 单点更新。
    pub fn update(&mut self, handle: Handle<NodeSlot>, pos: neo_core::LatLon) {
        let idx = handle.index() as usize;
        if idx >= self.positions.len() {
            self.positions.resize(idx + 1, None);
        }
        self.positions[idx] = Some(WorldPoint::from_latlon(pos));
    }

    /// 清除某槽位（节点被物理删除/槽位回收时调用）。
    pub fn clear(&mut self, index: u32) {
        if let Some(slot) = self.positions.get_mut(index as usize) {
            *slot = None;
        }
    }

    /// 估算内存。
    pub fn mem_bytes(&self) -> neo_core::ByteSize {
        neo_core::ByteSize(self.positions.capacity() * core::mem::size_of::<Option<WorldPoint>>())
    }
}
