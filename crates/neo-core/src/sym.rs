//! 字符串驻留（interning）。
//!
//! OSM 的 tag key/value 高度重复（`highway`、`building`、`residential` …），
//! 数千万对象如果每个都持有 `String`，内存与缓存都会崩溃。这里把所有 tag 字符串
//! 驻留成 4 字节的 [`Sym`]，真正文本只存一份。
//!
//! 注意：
//! * 驻留池是**每 DataSet 一份**，不跨工程共享（否则卸载/恢复语义会变复杂）；
//! * `Sym` 只有配合所属的 [`StringPool`] 才有意义，序列化时不能直接写 `u32`，
//!   必须连同池内容一起持久化（见 docs/10）。
//! * 索引 0 永久保留给空字符串。

use core::fmt;
use std::collections::HashMap;

/// 驻留字符串符号（4 字节）。
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct Sym(u32);

impl Sym {
    /// 空字符串。
    pub const EMPTY: Self = Self(0);

    /// 从原始索引构造（仅持久化/反序列化路径使用）。
    #[inline]
    pub const fn from_raw(raw: u32) -> Self {
        Self(raw)
    }

    /// 原始索引。
    #[inline]
    pub const fn raw(self) -> u32 {
        self.0
    }
}

impl fmt::Debug for Sym {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Sym({})", self.0)
    }
}

/// 字符串驻留池：`&str -> Sym` 与 `Sym -> &str` 双向 O(1)。
#[derive(Debug)]
pub struct StringPool {
    strings: Vec<Box<str>>,
    lookup: HashMap<Box<str>, u32>,
    total_bytes: usize,
}

impl Default for StringPool {
    fn default() -> Self {
        Self::new()
    }
}

impl StringPool {
    /// 新建池，索引 0 为空字符串。
    pub fn new() -> Self {
        let mut pool = Self {
            strings: Vec::new(),
            lookup: HashMap::new(),
            total_bytes: 0,
        };
        pool.intern("");
        pool
    }

    /// 驻留一个字符串。已存在则返回旧符号。
    pub fn intern(&mut self, s: &str) -> Sym {
        if let Some(&idx) = self.lookup.get(s) {
            return Sym(idx);
        }
        let idx = self.strings.len() as u32;
        self.strings.push(s.into());
        self.lookup.insert(s.into(), idx);
        self.total_bytes += s.len();
        Sym(idx)
    }

    /// 解析符号；越界（损坏的持久化/插件传入的裸索引）返回 `None`。
    #[inline]
    pub fn try_resolve(&self, sym: Sym) -> Option<&str> {
        self.strings.get(sym.0 as usize).map(|s| s.as_ref())
    }

    /// 解析符号；越界时返回空串而不是 panic（安全边界 API）。
    #[inline]
    pub fn resolve(&self, sym: Sym) -> &str {
        self.try_resolve(sym).unwrap_or("")
    }

    /// 已驻留的字符串个数。
    #[inline]
    pub fn len(&self) -> usize {
        self.strings.len()
    }

    /// 池是否只含空字符串。
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.strings.len() <= 1
    }

    /// 文本字节数（不含索引结构开销）。
    #[inline]
    pub fn text_bytes(&self) -> usize {
        self.total_bytes
    }

    /// 估算总内存占用（字节），用于内存基准。
    pub fn mem_bytes(&self) -> usize {
        // Vec<Box<str>>: 每个 16B + 堆上的文本；HashMap 条目估算 2*(16B + usize + 控制字节)。
        self.strings.capacity() * core::mem::size_of::<Box<str>>()
            + self.total_bytes
            + self.lookup.capacity() * (core::mem::size_of::<Box<str>>() + core::mem::size_of::<u32>() + 1)
            + self.total_bytes // lookup 里的一份 Box<str> 副本
    }
}
