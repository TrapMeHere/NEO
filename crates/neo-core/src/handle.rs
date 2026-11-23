//! 类型化的 generational 句柄。
//!
//! SoA 存储用 `index` 定位槽位；槽位被释放后 `generation` 递增，因此旧句柄会
//! 被判定为失效，而不会静默指向"复用了同一槽位的新对象"。这是删除/撤销编辑下
//! 数据安全的关键。
//!
//! `Handle<T>` 里的 `PhantomData<fn() -> T>` 使句柄恒为 `Send + Sync + Copy`，
//! 与 `T` 是否 `Send` 无关（句柄本身只是整数对）。

use core::fmt;
use core::hash::{Hash, Hasher};
use core::marker::PhantomData;

/// 指向 `T` 类型对象槽位的句柄。
pub struct Handle<T> {
    index: u32,
    generation: u32,
    _marker: PhantomData<fn() -> T>,
}

// 手工实现 Clone/Copy/PartialEq/Eq/Ord，避免 derive 给 `T` 加上不必要的 trait bound：
// 句柄只是两个整数，与 `T` 是否有 `Copy`/`Eq`/`Ord` 无关。
impl<T> Clone for Handle<T> {
    #[inline]
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> Copy for Handle<T> {}

impl<T> PartialEq for Handle<T> {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        self.index == other.index && self.generation == other.generation
    }
}

impl<T> Eq for Handle<T> {}

impl<T> PartialOrd for Handle<T> {
    #[inline]
    fn partial_cmp(&self, other: &Self) -> Option<core::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl<T> Ord for Handle<T> {
    #[inline]
    fn cmp(&self, other: &Self) -> core::cmp::Ordering {
        self.index
            .cmp(&other.index)
            .then(self.generation.cmp(&other.generation))
    }
}

impl<T> Handle<T> {
    /// 构造句柄。通常只由存储层调用。
    #[inline]
    pub const fn new(index: u32, generation: u32) -> Self {
        Self {
            index,
            generation,
            _marker: PhantomData,
        }
    }

    /// 槽位下标（`u32`：单类对象上限约 42 亿，远超 OSM 现实规模）。
    #[inline]
    pub const fn index(self) -> u32 {
        self.index
    }

    /// 槽位代号；与存储中记录的代号不一致即为失效句柄。
    #[inline]
    pub const fn generation(self) -> u32 {
        self.generation
    }

    /// 仅改 Generation，用于重建同一槽位的新句柄。
    #[inline]
    pub const fn with_generation(self, generation: u32) -> Self {
        Self {
            index: self.index,
            generation,
            _marker: PhantomData,
        }
    }
}

impl<T> fmt::Debug for Handle<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Handle({}#{})", self.index, self.generation)
    }
}

impl<T> Hash for Handle<T> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.index.hash(state);
        self.generation.hash(state);
    }
}

/// 槽位分配/回收状态。存储层实现 `Arena` 时复用这段逻辑。
#[derive(Debug, Default)]
pub struct SlotAllocator {
    /// 每个槽位当前的 generation。
    generations: Vec<u32>,
    /// 每个槽位是否存活。
    alive: Vec<bool>,
    /// 可复用槽位的 LIFO 栈。
    free: Vec<u32>,
    /// 存活槽位数（O(1) 维护；`len()` 曾在 10M 对象上做全量扫描）。
    live: usize,
}

impl SlotAllocator {
    /// 空分配器。
    pub fn new() -> Self {
        Self::default()
    }

    /// 槽位总数（含已释放）。
    #[inline]
    pub fn len(&self) -> usize {
        self.generations.len()
    }

    /// 是否没有任何槽位。
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.generations.is_empty()
    }

    /// 预分配容量。
    pub fn reserve(&mut self, additional: usize) {
        self.generations.reserve(additional);
        self.alive.reserve(additional);
        self.free.reserve(additional.min(1024));
    }

    /// 分配一个槽位，返回 (index, generation)。
    pub fn alloc(&mut self) -> (u32, u32) {
        if let Some(index) = self.free.pop() {
            debug_assert!(!self.alive[index as usize]);
            self.alive[index as usize] = true;
            self.live += 1;
            (index, self.generations[index as usize])
        } else {
            let index = self.generations.len() as u32;
            self.generations.push(0);
            self.alive.push(true);
            self.live += 1;
            (index, 0)
        }
    }

    /// 释放槽位；generation 递增以作废旧句柄。重复释放返回 `false`。
    pub fn free(&mut self, index: u32) -> bool {
        let Some(alive) = self.alive.get_mut(index as usize) else {
            return false;
        };
        if !*alive {
            return false;
        }
        *alive = false;
        // 溢出即意味着同一槽位已复用 2^32 次，旧句柄会重新有效（ABA）。
        // 这是不可恢复的一致性错误，与其静默回绕不如显式失败（docs/03 的约定）。
        self.generations[index as usize] = self.generations[index as usize]
            .checked_add(1)
            .expect("handle generation overflow: slot reused 2^32 times");
        self.live -= 1;
        self.free.push(index);
        true
    }

    /// 判断句柄是否仍有效。
    pub fn is_valid<T>(&self, handle: Handle<T>) -> bool {
        let i = handle.index() as usize;
        self.generations.get(i).copied() == Some(handle.generation())
            && self.alive.get(i).copied().unwrap_or(false)
    }

    /// 当前 generation（越界返回 `None`）。
    pub fn generation(&self, index: u32) -> Option<u32> {
        self.generations.get(index as usize).copied()
    }

    /// 存活槽位数（O(1)）。
    pub fn live_len(&self) -> usize {
        self.live
    }

    /// 估算内存占用（字节）。
    pub fn mem_bytes(&self) -> usize {
        self.generations.capacity() * core::mem::size_of::<u32>()
            + self.alive.capacity() * core::mem::size_of::<bool>()
            + self.free.capacity() * core::mem::size_of::<u32>()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_len_is_tracked_and_generation_invalidates() {
        let mut alloc = SlotAllocator::new();
        let (a, ga) = alloc.alloc();
        let (b, gb) = alloc.alloc();
        assert_eq!(alloc.live_len(), 2);
        assert!(alloc.is_valid(Handle::<u8>::new(a, ga)));
        assert!(alloc.free(a));
        assert_eq!(alloc.live_len(), 1);
        assert!(!alloc.is_valid(Handle::<u8>::new(a, ga)));
        let (c, gc) = alloc.alloc();
        assert_eq!(c, a, "freed slot is reused");
        assert_ne!(gc, ga, "generation must change");
        assert!(alloc.is_valid(Handle::<u8>::new(c, gc)));
        assert!(!alloc.is_valid(Handle::<u8>::new(b, gb.wrapping_add(1))));
        assert_eq!(alloc.live_len(), 2);
    }

    #[test]
    fn free_twice_is_rejected() {
        let mut alloc = SlotAllocator::new();
        let (index, _) = alloc.alloc();
        assert!(alloc.free(index));
        assert!(!alloc.free(index));
        assert_eq!(alloc.live_len(), 0);
    }
}
