//! Tag 集合（驻留符号对）。
//!
//! 与 `HashMap<String, String>` 相比：
//!
//! * 每个 tag 只占 8 字节（两个 `Sym`），且共享文本；
//! * 小集合用 `SmallVec` 内联，避免每个对象一次堆分配；
//! * 顺序保留（OSM 的 tag 顺序在 XML/OSC 往返和握手协议里都需要保真）。

use neo_core::Sym;
use smallvec::SmallVec;

/// 一个 OSM 对象的 tag 集合。
///
/// 不变量：key 唯一（同一 key 只保留最后一次设置的值）。
#[derive(Clone, Default, PartialEq, Eq, Debug)]
pub struct Tags {
    pairs: SmallVec<[(Sym, Sym); 6]>,
}

impl Tags {
    /// 空 tag 集合（不分配）。
    #[inline]
    pub fn new() -> Self {
        Self::default()
    }

    /// 预分配容量。
    pub fn with_capacity(cap: usize) -> Self {
        Self {
            pairs: SmallVec::with_capacity(cap),
        }
    }

    /// tag 数量。
    #[inline]
    pub fn len(&self) -> usize {
        self.pairs.len()
    }

    /// 是否为空。
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.pairs.is_empty()
    }

    /// 取 key 对应的 value。
    #[inline]
    pub fn get(&self, key: Sym) -> Option<Sym> {
        self.pairs.iter().find_map(|&(k, v)| (k == key).then_some(v))
    }

    /// 是否包含 key。
    #[inline]
    pub fn contains_key(&self, key: Sym) -> bool {
        self.get(key).is_some()
    }

    /// 按插入顺序迭代。
    #[inline]
    pub fn iter(&self) -> impl Iterator<Item = (Sym, Sym)> + '_ {
        self.pairs.iter().copied()
    }

    /// 插入或覆盖。返回旧值。
    pub fn set(&mut self, key: Sym, value: Sym) -> Option<Sym> {
        if let Some(slot) = self.pairs.iter_mut().find(|(k, _)| *k == key) {
            let old = slot.1;
            slot.1 = value;
            Some(old)
        } else {
            self.pairs.push((key, value));
            None
        }
    }

    /// 删除 key，返回旧值。
    pub fn remove(&mut self, key: Sym) -> Option<Sym> {
        let idx = self.pairs.iter().position(|(k, _)| *k == key)?;
        Some(self.pairs.remove(idx).1)
    }

    /// 清空。
    pub fn clear(&mut self) {
        self.pairs.clear();
    }

    /// 规范化视图：按 key 排序后的 (key, value) 列表。
    ///
    /// 用于计算内容哈希、diff、以及"相同 tag 集"判定（不依赖插入顺序）。
    pub fn canonical(&self) -> SmallVec<[(Sym, Sym); 6]> {
        let mut out = self.pairs.clone();
        out.sort_unstable_by_key(|(k, _)| *k);
        out
    }

    /// 与顺序无关的内容哈希（用于样式/几何缓存 key）。
    pub fn content_hash(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let canonical = self.canonical();
        let mut h = std::collections::hash_map::DefaultHasher::new();
        for (k, v) in &canonical {
            k.hash(&mut h);
            v.hash(&mut h);
        }
        canonical.len().hash(&mut h);
        h.finish()
    }

    /// 原始列表访问（顺序即插入顺序）。
    #[inline]
    pub fn raw(&self) -> &[(Sym, Sym)] {
        &self.pairs
    }

    /// 从迭代器构造。
    pub fn from_pairs<I: IntoIterator<Item = (Sym, Sym)>>(iter: I) -> Self {
        let mut tags = Self::new();
        for (k, v) in iter {
            tags.set(k, v);
        }
        tags
    }

    /// 估算堆内存（超出内联容量时）。
    pub fn mem_bytes(&self) -> usize {
        // SmallVec 内联容量为 6 对；更长的才分配。
        if self.pairs.spilled() {
            self.pairs.capacity() * std::mem::size_of::<(Sym, Sym)>()
        } else {
            0
        }
    }
}
