//! Typed generational handles.
//!
//! SoA storage uses `index` to locate a slot; when a slot is freed its `generation` is incremented, so old
//! handles are detected as stale instead of silently pointing at "a new object reusing the same slot". This is
//! key to data safety under deletion/undo.
//!
//! `PhantomData<fn() -> T>` inside `Handle<T>` keeps handles always `Send + Sync + Copy`, regardless of whether
//! `T` is `Send` (a handle is just a pair of integers).

use core::fmt;
use core::hash::{Hash, Hasher};
use core::marker::PhantomData;

/// Handle to the slot of an object of type `T`.
pub struct Handle<T> {
    index: u32,
    generation: u32,
    _marker: PhantomData<fn() -> T>,
}

// Clone/Copy/PartialEq/Eq/Ord are implemented by hand to avoid derive adding unnecessary trait bounds on `T`:
// a handle is just two integers, independent of whether `T` is `Copy`/`Eq`/`Ord`.
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
    /// Construct a handle. Normally called only by the storage layer.
    #[inline]
    pub const fn new(index: u32, generation: u32) -> Self {
        Self {
            index,
            generation,
            _marker: PhantomData,
        }
    }

    /// Slot index (`u32`: about 4.2 billion per object kind, far beyond realistic OSM scale).
    #[inline]
    pub const fn index(self) -> u32 {
        self.index
    }

    /// Slot generation; a handle is stale when it differs from the generation recorded in storage.
    #[inline]
    pub const fn generation(self) -> u32 {
        self.generation
    }

    /// Change only the generation, used to rebuild a new handle for the same slot.
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

/// Slot allocation/recycling state. Storage layers reuse this logic when implementing `Arena`.
#[derive(Debug, Default)]
pub struct SlotAllocator {
    /// Current generation of each slot.
    generations: Vec<u32>,
    /// Whether each slot is alive.
    alive: Vec<bool>,
    /// LIFO stack of reusable slots.
    free: Vec<u32>,
    /// Number of live slots (maintained in O(1); `len()` used to do a full scan over 10M objects).
    live: usize,
}

impl SlotAllocator {
    /// Empty allocator.
    pub fn new() -> Self {
        Self::default()
    }

    /// Total number of slots (including freed ones).
    #[inline]
    pub fn len(&self) -> usize {
        self.generations.len()
    }

    /// Whether there are no slots at all.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.generations.is_empty()
    }

    /// Preallocate capacity.
    pub fn reserve(&mut self, additional: usize) {
        self.generations.reserve(additional);
        self.alive.reserve(additional);
        self.free.reserve(additional.min(1024));
    }

    /// Allocate a slot; returns (index, generation).
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

    /// Free a slot; increments the generation to invalidate old handles. Returns `false` if already freed.
    pub fn free(&mut self, index: u32) -> bool {
        let Some(alive) = self.alive.get_mut(index as usize) else {
            return false;
        };
        if !*alive {
            return false;
        }
        *alive = false;
        // Overflow means this slot has been reused 2^32 times and old handles would become valid again (ABA).
        // This is an unrecoverable consistency error; failing explicitly beats silently wrapping (docs/03 convention).
        self.generations[index as usize] = self.generations[index as usize]
            .checked_add(1)
            .expect("handle generation overflow: slot reused 2^32 times");
        self.live -= 1;
        self.free.push(index);
        true
    }

    /// Whether a handle is still valid.
    pub fn is_valid<T>(&self, handle: Handle<T>) -> bool {
        let i = handle.index() as usize;
        self.generations.get(i).copied() == Some(handle.generation())
            && self.alive.get(i).copied().unwrap_or(false)
    }

    /// Current generation (returns `None` when out of range).
    pub fn generation(&self, index: u32) -> Option<u32> {
        self.generations.get(index as usize).copied()
    }

    /// Number of live slots (O(1)).
    pub fn live_len(&self) -> usize {
        self.live
    }

    /// Estimate memory usage in bytes.
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
