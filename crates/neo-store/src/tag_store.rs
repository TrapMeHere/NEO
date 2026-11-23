//! Tag 竞技场：所有 TagSet 的存储。
//!
//! 设计要点：
//!
//! * 所有 `(key, value)` 对连续存放在一个 `Vec` 里；
//! * 每个 TagSet 只记录 `(offset, len)`，因此 TagSetId 是 4 字节；
//! * 空集合固定为 `TagSetId::EMPTY`（索引 0）；
//! * 写路径是**追加式**（append-only）：编辑一个对象的 tag 会写入一条新集合，
//!   旧区间在 `compact` 时回收。这让 tag 修改无引用计数、无失效风险。

use neo_core::{ByteSize, Sym};

/// Tag 集合在 [`TagStore`] 中的句柄。
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default, Debug)]
pub struct TagSetId(pub u32);

impl TagSetId {
    /// 空 tag 集合。
    pub const EMPTY: Self = Self(0);

    /// 原始索引。
    #[inline]
    pub const fn raw(self) -> u32 {
        self.0
    }
}

/// Tag 存储：扁平 arena + 每集合的 range。
#[derive(Debug)]
pub struct TagStore {
    pairs: Vec<(Sym, Sym)>,
    /// `spans[i]..spans[i + 1]` 是第 i 个 tag set 在 `pairs` 中的范围。
    spans: Vec<u32>,
}

impl Default for TagStore {
    fn default() -> Self {
        Self::new()
    }
}

impl TagStore {
    /// 新建，包含空集合。
    pub fn new() -> Self {
        Self {
            pairs: Vec::new(),
            // 索引 0 是空集合，范围 [0, 0)。
            spans: vec![0, 0],
        }
    }

    /// 已存储的 tag set 数量（含空集合）。
    #[inline]
    pub fn len(&self) -> usize {
        self.spans.len() - 1
    }

    /// 是否只有空集合。
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len() <= 1
    }

    /// 全部 tag 个数。
    #[inline]
    pub fn tag_count(&self) -> usize {
        self.pairs.len()
    }

    /// 新增一个 tag set，返回句柄。
    pub fn add(&mut self, pairs: &[(Sym, Sym)]) -> TagSetId {
        if pairs.is_empty() {
            return TagSetId::EMPTY;
        }
        let end = self.pairs.len() as u32 + pairs.len() as u32;
        self.pairs.extend_from_slice(pairs);
        self.spans.push(end);
        TagSetId((self.spans.len() - 2) as u32)
    }

    /// 取集合内容；非法 `TagSetId`（越界索引）返回空切片而不是 panic。
    ///
    /// `TagSetId` 的字段是公开的，任何安全代码都能构造越界值；项目
    /// `forbid(unsafe_code)` 的前提是安全 API 不应 panic。
    #[inline]
    pub fn get(&self, id: TagSetId) -> &[(Sym, Sym)] {
        let idx = id.0 as usize;
        let (Some(start), Some(end)) = (self.spans.get(idx), self.spans.get(idx + 1)) else {
            debug_assert!(false, "invalid TagSetId {}", id.0);
            return &[];
        };
        self.pairs.get(*start as usize..*end as usize).unwrap_or(&[])
    }

    /// 按 key 查 value。
    #[inline]
    pub fn tag(&self, id: TagSetId, key: Sym) -> Option<Sym> {
        self.get(id).iter().find_map(|&(k, v)| (k == key).then_some(v))
    }

    /// 迭代集合内容。
    #[inline]
    pub fn iter(&self, id: TagSetId) -> impl Iterator<Item = (Sym, Sym)> + '_ {
        self.get(id).iter().copied()
    }

    /// 估算堆内存。
    pub fn mem_bytes(&self) -> ByteSize {
        ByteSize(
            self.pairs.capacity() * core::mem::size_of::<(Sym, Sym)>()
                + self.spans.capacity() * core::mem::size_of::<u32>(),
        )
    }
}
