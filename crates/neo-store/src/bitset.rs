//! 紧凑位图，用于 dirty set、可见集、删除标记等。
//!
//! 10M 对象的 dirty 标记只需要 1.25 MB（`Vec<u64>`），而 `HashSet<u32>` 会到
//! 数百 MB 且缓存不友好。位图是编辑路径上的第一选择。

use neo_core::ByteSize;

/// 紧凑位图。
#[derive(Debug, Default, Clone)]
pub struct BitSet {
    words: Vec<u64>,
    len: usize,
}

impl BitSet {
    /// 新建空位图。
    pub fn new() -> Self {
        Self::default()
    }

    /// 指定 bit 数量的空位图。
    pub fn with_bits(len: usize) -> Self {
        Self {
            words: vec![0; len.div_ceil(64)],
            len,
        }
    }

    /// 位数。
    #[inline]
    pub fn len(&self) -> usize {
        self.len
    }

    /// 是否没有任何位。
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// 确保能容纳 `len` 个 bit。
    pub fn ensure_len(&mut self, len: usize) {
        if len > self.len {
            self.len = len;
            let words = len.div_ceil(64);
            if self.words.len() < words {
                self.words.resize(words, 0);
            }
        }
    }

    /// 置位；越界自动扩容。
    #[inline]
    pub fn set(&mut self, index: usize) {
        if index >= self.len {
            self.ensure_len(index + 1);
        }
        let (w, b) = (index / 64, index % 64);
        self.words[w] |= 1u64 << b;
    }

    /// 清位。
    #[inline]
    pub fn clear(&mut self, index: usize) {
        if index < self.len {
            let (w, b) = (index / 64, index % 64);
            self.words[w] &= !(1u64 << b);
        }
    }

    /// 读取位。
    #[inline]
    pub fn get(&self, index: usize) -> bool {
        if index >= self.len {
            return false;
        }
        let (w, b) = (index / 64, index % 64);
        (self.words[w] >> b) & 1 == 1
    }

    /// 全部清零（保留容量）。
    pub fn clear_all(&mut self) {
        self.words.fill(0);
    }

    /// 已置位的 bit 数。
    pub fn count_ones(&self) -> usize {
        self.words.iter().map(|w| w.count_ones() as usize).sum()
    }

    /// 迭代所有置位的下标（升序）。
    pub fn iter_set(&self) -> impl Iterator<Item = usize> + '_ {
        self.words.iter().enumerate().flat_map(|(wi, &word)| {
            let base = wi * 64;
            let mut w = word;
            core::iter::from_fn(move || {
                if w == 0 {
                    return None;
                }
                let b = w.trailing_zeros() as usize;
                w &= w - 1;
                Some(base + b)
            })
        })
    }

    /// 估算内存。
    pub fn mem_bytes(&self) -> ByteSize {
        ByteSize(self.words.capacity() * core::mem::size_of::<u64>())
    }
}
