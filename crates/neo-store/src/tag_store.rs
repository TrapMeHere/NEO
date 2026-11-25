//! Tag arena: storage for all TagSets.
//!
//! Design points:
//!
//! * all `(key, value)` pairs are stored contiguously in one `Vec`;
//! * each TagSet records only `(offset, len)`, so a TagSetId is 4 bytes;
//! * the empty set is always `TagSetId::EMPTY` (index 0);
//! * the write path is **append-only**: editing an object's tags writes a new set,
//!   and the old range is reclaimed on `compact`. This makes tag edits free of reference counting and invalidation risk.

use neo_core::{ByteSize, Sym};

/// Handle to a tag set in [`TagStore`].
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default, Debug)]
pub struct TagSetId(pub u32);

impl TagSetId {
    /// Empty tag set.
    pub const EMPTY: Self = Self(0);

    /// Raw index.
    #[inline]
    pub const fn raw(self) -> u32 {
        self.0
    }
}

/// Tag storage: flat arena + a range per set.
#[derive(Debug)]
pub struct TagStore {
    pairs: Vec<(Sym, Sym)>,
    /// `spans[i]..spans[i + 1]` is the range of the i-th tag set within `pairs`.
    spans: Vec<u32>,
}

impl Default for TagStore {
    fn default() -> Self {
        Self::new()
    }
}

impl TagStore {
    /// Create a new store containing the empty set.
    pub fn new() -> Self {
        Self {
            pairs: Vec::new(),
            // Index 0 is the empty set, range [0, 0).
            spans: vec![0, 0],
        }
    }

    /// Number of stored tag sets (including the empty set).
    #[inline]
    pub fn len(&self) -> usize {
        self.spans.len() - 1
    }

    /// Whether only the empty set exists.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len() <= 1
    }

    /// Total number of tags.
    #[inline]
    pub fn tag_count(&self) -> usize {
        self.pairs.len()
    }

    /// Add a tag set and return its handle.
    pub fn add(&mut self, pairs: &[(Sym, Sym)]) -> TagSetId {
        if pairs.is_empty() {
            return TagSetId::EMPTY;
        }
        let end = self.pairs.len() as u32 + pairs.len() as u32;
        self.pairs.extend_from_slice(pairs);
        self.spans.push(end);
        TagSetId((self.spans.len() - 2) as u32)
    }

    /// Get the set contents; an invalid `TagSetId` (out-of-range index) returns an empty slice instead of panicking.
    ///
    /// `TagSetId`'s field is public, so any safe code can construct an out-of-range value; the project's
    /// `forbid(unsafe_code)` premise is that safe APIs must not panic.
    #[inline]
    pub fn get(&self, id: TagSetId) -> &[(Sym, Sym)] {
        let idx = id.0 as usize;
        let (Some(start), Some(end)) = (self.spans.get(idx), self.spans.get(idx + 1)) else {
            debug_assert!(false, "invalid TagSetId {}", id.0);
            return &[];
        };
        self.pairs.get(*start as usize..*end as usize).unwrap_or(&[])
    }

    /// Look up a value by key.
    #[inline]
    pub fn tag(&self, id: TagSetId, key: Sym) -> Option<Sym> {
        self.get(id).iter().find_map(|&(k, v)| (k == key).then_some(v))
    }

    /// Iterate the set contents.
    #[inline]
    pub fn iter(&self, id: TagSetId) -> impl Iterator<Item = (Sym, Sym)> + '_ {
        self.get(id).iter().copied()
    }

    /// Estimate heap memory.
    pub fn mem_bytes(&self) -> ByteSize {
        ByteSize(
            self.pairs.capacity() * core::mem::size_of::<(Sym, Sym)>()
                + self.spans.capacity() * core::mem::size_of::<u32>(),
        )
    }
}
