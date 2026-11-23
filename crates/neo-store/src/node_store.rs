//! Node 的 SoA 存储。
//!
//! ```text
//! ids   [ i64 | i64 | i64 | ... ]   ┐
//! lat   [ i32 | i32 | i32 | ... ]   │ 同一槽位下标 i 对应同一对象
//! lon   [ i32 | i32 | i32 | ... ]   │
//! tags  [ u32 | u32 | u32 | ... ]   │
//! meta  [ 32B | 32B | 32B | ... ]   ┘
//! alive [ 1B  | 1B  | ... ]         ← 由 SlotAllocator 管理
//! ```
//!
//! 1000 万节点：4+4+4+4+32 = 48 字节/节点 ≈ 480 MB（加位图与 id 索引后约 700 MB），
//! 对比 `Vec<Box<Node>>` 的 100+ 字节/节点 + 指针追逐，这是数量级的差别。

use neo_core::{Handle, LatLon, Meta, ObjectId, SlotAllocator};
use neo_osm::Tags;

use crate::tag_store::{TagSetId, TagStore};

/// Node 的只读快照视图（不含 tag 内容，需配 `TagStore` 解析）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NodeRef {
    /// 句柄。
    pub handle: Handle<NodeSlot>,
    /// 外部 ID。
    pub id: ObjectId,
    /// 坐标。
    pub pos: LatLon,
    /// tag 集合句柄。
    pub tags: TagSetId,
    /// 元数据。
    pub meta: Meta,
}

/// 标记类型：句柄的 phantom 参数。
#[derive(Debug)]
pub struct NodeSlot;

/// Node 的 SoA 存储。
#[derive(Debug, Default)]
pub struct NodeStore {
    ids: Vec<ObjectId>,
    lat: Vec<i32>,
    lon: Vec<i32>,
    tags: Vec<TagSetId>,
    meta: Vec<Meta>,
    alloc: SlotAllocator,
}

impl NodeStore {
    /// 空存储。
    pub fn new() -> Self {
        Self::default()
    }

    /// 预分配容量。
    pub fn with_capacity(capacity: usize) -> Self {
        let mut s = Self::new();
        s.reserve(capacity);
        s
    }

    /// 预分配。
    pub fn reserve(&mut self, additional: usize) {
        self.ids.reserve(additional);
        self.lat.reserve(additional);
        self.lon.reserve(additional);
        self.tags.reserve(additional);
        self.meta.reserve(additional);
        self.alloc.reserve(additional);
    }

    /// 槽位总数（含已删除）。
    #[inline]
    pub fn slot_len(&self) -> usize {
        self.ids.len()
    }

    /// 存活节点数。
    #[inline]
    pub fn len(&self) -> usize {
        self.alloc.live_len()
    }

    /// 是否没有存活节点。
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// 追加节点（导入路径；不复用空闲槽位）。
    pub fn push(&mut self, id: ObjectId, pos: LatLon, tags: TagSetId, meta: Meta) -> Handle<NodeSlot> {
        let (index, generation) = self.alloc.alloc();
        let i = index as usize;
        if i >= self.ids.len() {
            self.ids.resize(i + 1, ObjectId::NONE);
            self.lat.resize(i + 1, 0);
            self.lon.resize(i + 1, 0);
            self.tags.resize(i + 1, TagSetId::EMPTY);
            self.meta.resize(i + 1, Meta::new_local());
        }
        self.ids[i] = id;
        self.lat[i] = pos.lat.units();
        self.lon[i] = pos.lon.units();
        self.tags[i] = tags;
        self.meta[i] = meta;
        Handle::new(index, generation)
    }

    /// 句柄是否有效。
    #[inline]
    pub fn contains(&self, handle: Handle<NodeSlot>) -> bool {
        self.alloc.is_valid(handle)
    }

    /// 槽位当前的 generation（用于从脏位图还原句柄）。
    #[inline]
    pub fn generation(&self, index: u32) -> Option<u32> {
        self.alloc.generation(index)
    }

    /// 读取节点快照。
    pub fn get(&self, handle: Handle<NodeSlot>) -> Option<NodeRef> {
        if !self.contains(handle) {
            return None;
        }
        let i = handle.index() as usize;
        Some(NodeRef {
            handle,
            id: self.ids[i],
            pos: LatLon::new(
                neo_core::Coord::from_units(self.lat[i]),
                neo_core::Coord::from_units(self.lon[i]),
            ),
            tags: self.tags[i],
            meta: self.meta[i],
        })
    }

    /// 外部 ID。
    #[inline]
    pub fn id(&self, handle: Handle<NodeSlot>) -> Option<ObjectId> {
        self.contains(handle).then(|| self.ids[handle.index() as usize])
    }

    /// 坐标（渲染/几何热路径，避免构造完整视图）。
    #[inline]
    pub fn position(&self, handle: Handle<NodeSlot>) -> Option<LatLon> {
        if !self.contains(handle) {
            return None;
        }
        let i = handle.index() as usize;
        Some(LatLon::new(
            neo_core::Coord::from_units(self.lat[i]),
            neo_core::Coord::from_units(self.lon[i]),
        ))
    }

    /// 修改坐标，返回旧值。失效句柄返回 `None`。
    pub fn set_position(&mut self, handle: Handle<NodeSlot>, pos: LatLon) -> Option<LatLon> {
        if !self.contains(handle) {
            return None;
        }
        let i = handle.index() as usize;
        let old = LatLon::new(
            neo_core::Coord::from_units(self.lat[i]),
            neo_core::Coord::from_units(self.lon[i]),
        );
        self.lat[i] = pos.lat.units();
        self.lon[i] = pos.lon.units();
        Some(old)
    }

    /// 替换外部 ID（上传成功后把负 ID 映射为服务端新 ID）。
    pub fn set_id(&mut self, handle: Handle<NodeSlot>, id: ObjectId) -> bool {
        if !self.contains(handle) {
            return false;
        }
        self.ids[handle.index() as usize] = id;
        true
    }

    /// 替换 tag 集合句柄。
    pub fn set_tags(&mut self, handle: Handle<NodeSlot>, tags: TagSetId) -> bool {
        if !self.contains(handle) {
            return false;
        }
        self.tags[handle.index() as usize] = tags;
        true
    }

    /// 替换元数据。
    pub fn set_meta(&mut self, handle: Handle<NodeSlot>, meta: Meta) -> bool {
        if !self.contains(handle) {
            return false;
        }
        self.meta[handle.index() as usize] = meta;
        true
    }

    /// 删除节点（逻辑删除 + 槽位回收）。
    pub fn remove(&mut self, handle: Handle<NodeSlot>) -> Option<NodeRef> {
        let view = self.get(handle)?;
        self.alloc.free(handle.index());
        Some(view)
    }

    /// 迭代所有存活节点快照。
    pub fn iter(&self) -> impl Iterator<Item = NodeRef> + '_ {
        (0..self.slot_len()).filter_map(|i| {
            let generation = self.alloc.generation(i as u32)?;
            self.get(Handle::new(i as u32, generation))
        })
    }

    /// 把 tags 解析成领域对象（需要 pool 访问时用）。
    pub fn tags_of(&self, handle: Handle<NodeSlot>, tags: &TagStore) -> Option<Tags> {
        let t = self.get(handle)?;
        Some(Tags::from_pairs(tags.iter(t.tags)))
    }

    /// 估算堆内存。
    pub fn mem_bytes(&self) -> neo_core::ByteSize {
        use core::mem::size_of;
        use neo_core::ByteSize;
        ByteSize(
            self.ids.capacity() * size_of::<ObjectId>()
                + self.lat.capacity() * size_of::<i32>()
                + self.lon.capacity() * size_of::<i32>()
                + self.tags.capacity() * size_of::<TagSetId>()
                + self.meta.capacity() * size_of::<Meta>()
                + self.alloc.mem_bytes(),
        )
    }
}
