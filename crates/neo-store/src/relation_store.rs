//! Relation 的 SoA 存储。
//!
//! 成员是 `(role, target)` 对，`target` 用 [`AnyHandle`] 表示——种类 + 槽位 + generation。
//! 成员**顺序是数据**（route / multipolygon 语义），因此这里只追加、不改顺序。

use neo_core::{ByteSize, Handle, Meta, ObjectId, ObjectKind, SlotAllocator, Sym};

use crate::node_store::NodeSlot;
use crate::tag_store::TagSetId;
use crate::way_store::WaySlot;

/// 标记类型。
#[derive(Debug)]
pub struct RelationSlot;

/// 指向任意图元的句柄（种类 + 槽位 + generation）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct AnyHandle {
    /// 对象种类。
    pub kind: ObjectKind,
    /// 槽位下标。
    pub index: u32,
    /// 槽位代号。
    pub generation: u32,
}

impl AnyHandle {
    /// 由 node 句柄构造。
    #[inline]
    pub fn from_node(h: Handle<NodeSlot>) -> Self {
        Self {
            kind: ObjectKind::Node,
            index: h.index(),
            generation: h.generation(),
        }
    }

    /// 由 way 句柄构造。
    #[inline]
    pub fn from_way(h: Handle<WaySlot>) -> Self {
        Self {
            kind: ObjectKind::Way,
            index: h.index(),
            generation: h.generation(),
        }
    }

    /// 由 relation 句柄构造。
    #[inline]
    pub fn from_relation(h: Handle<RelationSlot>) -> Self {
        Self {
            kind: ObjectKind::Relation,
            index: h.index(),
            generation: h.generation(),
        }
    }

    /// 转回 node 句柄（种类不匹配返回 `None`）。
    #[inline]
    pub fn as_node(self) -> Option<Handle<NodeSlot>> {
        (self.kind == ObjectKind::Node).then(|| Handle::new(self.index, self.generation))
    }

    /// 转回 way 句柄。
    #[inline]
    pub fn as_way(self) -> Option<Handle<WaySlot>> {
        (self.kind == ObjectKind::Way).then(|| Handle::new(self.index, self.generation))
    }

    /// 转回 relation 句柄。
    #[inline]
    pub fn as_relation(self) -> Option<Handle<RelationSlot>> {
        (self.kind == ObjectKind::Relation).then(|| Handle::new(self.index, self.generation))
    }
}

/// 一个 relation 成员。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StoredMember {
    /// 角色（驻留符号）。
    pub role: Sym,
    /// 目标图元句柄。
    pub target: AnyHandle,
}

/// Relation 的只读快照。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RelationRef {
    /// 句柄。
    pub handle: Handle<RelationSlot>,
    /// 外部 ID。
    pub id: ObjectId,
    /// tag 集合。
    pub tags: TagSetId,
    /// 元数据。
    pub meta: Meta,
    /// 成员区间起点。
    pub first_member: u32,
    /// 成员个数。
    pub member_count: u32,
}

/// Relation 的 SoA 存储。
#[derive(Debug, Default)]
pub struct RelationStore {
    ids: Vec<ObjectId>,
    tags: Vec<TagSetId>,
    meta: Vec<Meta>,
    offsets: Vec<u32>,
    counts: Vec<u32>,
    members: Vec<StoredMember>,
    alloc: SlotAllocator,
}

impl RelationStore {
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

    /// 存活 relation 数。
    #[inline]
    pub fn len(&self) -> usize {
        self.alloc.live_len()
    }

    /// 是否没有 relation。
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// 成员总数。
    #[inline]
    pub fn member_count(&self) -> usize {
        self.members.len()
    }

    /// 追加关系。
    pub fn push(
        &mut self,
        id: ObjectId,
        members: &[StoredMember],
        tags: TagSetId,
        meta: Meta,
    ) -> Handle<RelationSlot> {
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
        self.offsets[i] = self.members.len() as u32;
        self.counts[i] = members.len() as u32;
        self.members.extend_from_slice(members);
        Handle::new(index, generation)
    }

    /// 句柄是否有效。
    #[inline]
    pub fn contains(&self, handle: Handle<RelationSlot>) -> bool {
        self.alloc.is_valid(handle)
    }

    /// 槽位当前的 generation。
    #[inline]
    pub fn generation(&self, index: u32) -> Option<u32> {
        self.alloc.generation(index)
    }

    /// 读取快照。
    pub fn get(&self, handle: Handle<RelationSlot>) -> Option<RelationRef> {
        if !self.contains(handle) {
            return None;
        }
        let i = handle.index() as usize;
        Some(RelationRef {
            handle,
            id: self.ids[i],
            tags: self.tags[i],
            meta: self.meta[i],
            first_member: self.offsets[i],
            member_count: self.counts[i],
        })
    }

    /// 外部 ID。
    #[inline]
    pub fn id(&self, handle: Handle<RelationSlot>) -> Option<ObjectId> {
        self.contains(handle).then(|| self.ids[handle.index() as usize])
    }

    /// 成员切片。
    #[inline]
    pub fn members_of(&self, handle: Handle<RelationSlot>) -> Option<&[StoredMember]> {
        if !self.contains(handle) {
            return None;
        }
        let i = handle.index() as usize;
        let start = self.offsets[i] as usize;
        let end = start + self.counts[i] as usize;
        Some(&self.members[start..end])
    }

    /// 替换成员（追加式）。
    pub fn set_members(&mut self, handle: Handle<RelationSlot>, members: &[StoredMember]) -> bool {
        if !self.contains(handle) {
            return false;
        }
        let i = handle.index() as usize;
        self.offsets[i] = self.members.len() as u32;
        self.counts[i] = members.len() as u32;
        self.members.extend_from_slice(members);
        true
    }

    /// 替换外部 ID（上传成功后映射为服务端新 ID）。
    pub fn set_id(&mut self, handle: Handle<RelationSlot>, id: ObjectId) -> bool {
        if !self.contains(handle) {
            return false;
        }
        self.ids[handle.index() as usize] = id;
        true
    }

    /// 替换 tag 集合。
    pub fn set_tags(&mut self, handle: Handle<RelationSlot>, tags: TagSetId) -> bool {
        if !self.contains(handle) {
            return false;
        }
        self.tags[handle.index() as usize] = tags;
        true
    }

    /// 替换元数据。
    pub fn set_meta(&mut self, handle: Handle<RelationSlot>, meta: Meta) -> bool {
        if !self.contains(handle) {
            return false;
        }
        self.meta[handle.index() as usize] = meta;
        true
    }

    /// 删除（逻辑删除 + 槽位回收）。
    pub fn remove(&mut self, handle: Handle<RelationSlot>) -> Option<RelationRef> {
        let view = self.get(handle)?;
        self.alloc.free(handle.index());
        Some(view)
    }

    /// 迭代存活 relation 快照。
    pub fn iter(&self) -> impl Iterator<Item = RelationRef> + '_ {
        (0..self.slot_len()).filter_map(|i| {
            let generation = self.alloc.generation(i as u32)?;
            self.get(Handle::new(i as u32, generation))
        })
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
                + self.members.capacity() * size_of::<StoredMember>()
                + self.alloc.mem_bytes(),
        )
    }
}
