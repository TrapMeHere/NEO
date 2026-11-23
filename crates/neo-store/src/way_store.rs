//! Way 的 SoA 存储。
//!
//! Way 的节点引用不是 `Vec<Vec<...>>`，而是**连续引用池 + 每 way 的 (offset, len)**：
//!
//! ```text
//! way 0 -> offsets[0]=0, count[0]=3  -> node_refs[0..3]
//! way 1 -> offsets[1]=3, count[1]=5  -> node_refs[3..8]
//! ```
//!
//! 编辑 way 的节点序列时采用"追加新区间 + 更新 offset/count"，旧区间在 compact 时回收；
//! 这样引用池永远只追加，不会移动已有 way 的引用（否则要更新所有索引）。

use neo_core::{ByteSize, Handle, Meta, ObjectId, SlotAllocator};

use crate::node_store::{NodeSlot, NodeStore};
use crate::tag_store::TagSetId;

/// 标记类型。
#[derive(Debug)]
pub struct WaySlot;

/// Way 的只读快照。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WayRef {
    /// 句柄。
    pub handle: Handle<WaySlot>,
    /// 外部 ID。
    pub id: ObjectId,
    /// tag 集合。
    pub tags: TagSetId,
    /// 元数据。
    pub meta: Meta,
    /// 节点引用区间起点。
    pub first_node: u32,
    /// 节点个数。
    pub node_count: u32,
}

/// Way 的 SoA 存储。
#[derive(Debug, Default)]
pub struct WayStore {
    ids: Vec<ObjectId>,
    tags: Vec<TagSetId>,
    meta: Vec<Meta>,
    offsets: Vec<u32>,
    counts: Vec<u32>,
    node_refs: Vec<Handle<NodeSlot>>,
    alloc: SlotAllocator,
}

impl WayStore {
    /// 空存储。
    pub fn new() -> Self {
        Self::default()
    }

    /// 预分配。
    pub fn with_capacity(capacity: usize) -> Self {
        let mut s = Self::new();
        s.ids.reserve(capacity);
        s.tags.reserve(capacity);
        s.meta.reserve(capacity);
        s.offsets.reserve(capacity);
        s.counts.reserve(capacity);
        s.alloc.reserve(capacity);
        s
    }

    /// 槽位总数。
    #[inline]
    pub fn slot_len(&self) -> usize {
        self.ids.len()
    }

    /// 存活 way 数。
    #[inline]
    pub fn len(&self) -> usize {
        self.alloc.live_len()
    }

    /// 是否没有 way。
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// 引用池中的节点引用总数。
    #[inline]
    pub fn ref_count(&self) -> usize {
        self.node_refs.len()
    }

    /// 追加一条 way。
    pub fn push(
        &mut self,
        id: ObjectId,
        nodes: &[Handle<NodeSlot>],
        tags: TagSetId,
        meta: Meta,
    ) -> Handle<WaySlot> {
        let (index, generation) = self.alloc.alloc();
        let i = index as usize;
        if i >= self.ids.len() {
            self.ids.resize(i + 1, ObjectId::NONE);
            self.tags.resize(i + 1, TagSetId::EMPTY);
            self.meta.resize(i + 1, Meta::new_local());
            self.offsets.resize(i + 1, 0);
            self.counts.resize(i + 1, 0);
        }
        self.ids[i] = id;
        self.tags[i] = tags;
        self.meta[i] = meta;
        self.offsets[i] = self.node_refs.len() as u32;
        self.counts[i] = nodes.len() as u32;
        self.node_refs.extend_from_slice(nodes);
        Handle::new(index, generation)
    }

    /// 句柄是否有效。
    #[inline]
    pub fn contains(&self, handle: Handle<WaySlot>) -> bool {
        self.alloc.is_valid(handle)
    }

    /// 槽位当前的 generation。
    #[inline]
    pub fn generation(&self, index: u32) -> Option<u32> {
        self.alloc.generation(index)
    }

    /// 读取快照。
    pub fn get(&self, handle: Handle<WaySlot>) -> Option<WayRef> {
        if !self.contains(handle) {
            return None;
        }
        let i = handle.index() as usize;
        Some(WayRef {
            handle,
            id: self.ids[i],
            tags: self.tags[i],
            meta: self.meta[i],
            first_node: self.offsets[i],
            node_count: self.counts[i],
        })
    }

    /// 外部 ID。
    #[inline]
    pub fn id(&self, handle: Handle<WaySlot>) -> Option<ObjectId> {
        self.contains(handle).then(|| self.ids[handle.index() as usize])
    }

    /// 节点引用切片（几何构建热路径）。
    #[inline]
    pub fn nodes(&self, handle: Handle<WaySlot>) -> Option<&[Handle<NodeSlot>]> {
        if !self.contains(handle) {
            return None;
        }
        let i = handle.index() as usize;
        let start = self.offsets[i] as usize;
        let end = start + self.counts[i] as usize;
        Some(&self.node_refs[start..end])
    }

    /// 替换节点序列（追加式）；返回旧引用区间句柄。
    pub fn set_nodes(&mut self, handle: Handle<WaySlot>, nodes: &[Handle<NodeSlot>]) -> Option<u32> {
        if !self.contains(handle) {
            return None;
        }
        let i = handle.index() as usize;
        let old_start = self.offsets[i];
        self.offsets[i] = self.node_refs.len() as u32;
        self.counts[i] = nodes.len() as u32;
        self.node_refs.extend_from_slice(nodes);
        Some(old_start)
    }

    /// 替换外部 ID（上传成功后映射为服务端新 ID）。
    pub fn set_id(&mut self, handle: Handle<WaySlot>, id: ObjectId) -> bool {
        if !self.contains(handle) {
            return false;
        }
        self.ids[handle.index() as usize] = id;
        true
    }

    /// 替换 tag 集合。
    pub fn set_tags(&mut self, handle: Handle<WaySlot>, tags: TagSetId) -> bool {
        if !self.contains(handle) {
            return false;
        }
        self.tags[handle.index() as usize] = tags;
        true
    }

    /// 替换元数据。
    pub fn set_meta(&mut self, handle: Handle<WaySlot>, meta: Meta) -> bool {
        if !self.contains(handle) {
            return false;
        }
        self.meta[handle.index() as usize] = meta;
        true
    }

    /// 删除（逻辑删除 + 槽位回收）。
    pub fn remove(&mut self, handle: Handle<WaySlot>) -> Option<WayRef> {
        let view = self.get(handle)?;
        self.alloc.free(handle.index());
        Some(view)
    }

    /// 迭代存活 way 快照。
    pub fn iter(&self) -> impl Iterator<Item = WayRef> + '_ {
        (0..self.slot_len()).filter_map(|i| {
            let generation = self.alloc.generation(i as u32)?;
            self.get(Handle::new(i as u32, generation))
        })
    }

    /// 几何便捷方法：把节点引用解析为坐标序列（跳过失效节点）。
    pub fn positions<'a>(
        &'a self,
        handle: Handle<WaySlot>,
        nodes: &'a NodeStore,
    ) -> Option<impl Iterator<Item = neo_core::LatLon> + 'a> {
        Some(self.nodes(handle)?.iter().filter_map(move |h| nodes.position(*h)))
    }

    /// 估算堆内存。
    pub fn mem_bytes(&self) -> ByteSize {
        use core::mem::size_of;
        ByteSize(
            self.ids.capacity() * size_of::<ObjectId>()
                + self.tags.capacity() * size_of::<TagSetId>()
                + self.meta.capacity() * size_of::<Meta>()
                + self.offsets.capacity() * size_of::<u32>()
                + self.counts.capacity() * size_of::<u32>()
                + self.node_refs.capacity() * size_of::<Handle<NodeSlot>>()
                + self.alloc.mem_bytes(),
        )
    }
}
