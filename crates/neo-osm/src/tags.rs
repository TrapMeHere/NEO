//! Tag set (interned symbol pairs).
//!
//! Compared with `HashMap<String, String>`:
//!
//! * each tag takes only 8 bytes (two `Sym`s) and shares the text;
//! * small sets are inlined with `SmallVec`, avoiding one heap allocation per object;
//! * order is preserved (OSM tag order must round-trip faithfully through XML/OSC and the handshake protocol).

use neo_core::Sym;
use smallvec::SmallVec;

/// Tag set of one OSM object.
///
/// Invariant: keys are unique (only the last value set for a key is kept).
#[derive(Clone, Default, PartialEq, Eq, Debug)]
pub struct Tags {
    pairs: SmallVec<[(Sym, Sym); 6]>,
}

impl Tags {
    /// Empty tag set (no allocation).
    #[inline]
    pub fn new() -> Self {
        Self::default()
    }

    /// Preallocated capacity.
    pub fn with_capacity(cap: usize) -> Self {
        Self {
            pairs: SmallVec::with_capacity(cap),
        }
    }

    /// Number of tags.
    #[inline]
    pub fn len(&self) -> usize {
        self.pairs.len()
    }

    /// Whether the set is empty.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.pairs.is_empty()
    }

    /// Get the value for a key.
    #[inline]
    pub fn get(&self, key: Sym) -> Option<Sym> {
        self.pairs.iter().find_map(|&(k, v)| (k == key).then_some(v))
    }

    /// Whether the key is present.
    #[inline]
    pub fn contains_key(&self, key: Sym) -> bool {
        self.get(key).is_some()
    }

    /// Iterate in insertion order.
    #[inline]
    pub fn iter(&self) -> impl Iterator<Item = (Sym, Sym)> + '_ {
        self.pairs.iter().copied()
    }

    /// Insert or overwrite. Returns the old value.
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

    /// Remove a key; returns the old value.
    pub fn remove(&mut self, key: Sym) -> Option<Sym> {
        let idx = self.pairs.iter().position(|(k, _)| *k == key)?;
        Some(self.pairs.remove(idx).1)
    }

    /// Clear the set.
    pub fn clear(&mut self) {
        self.pairs.clear();
    }

    /// Canonical view: a (key, value) list sorted by key.
    ///
    /// Used for content hashing, diffing, and the "same tag set" check (independent of insertion order).
    pub fn canonical(&self) -> SmallVec<[(Sym, Sym); 6]> {
        let mut out = self.pairs.clone();
        out.sort_unstable_by_key(|(k, _)| *k);
        out
    }

    /// Order-independent content hash (used as a style/geometry cache key).
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

    /// Raw list access (order is insertion order).
    #[inline]
    pub fn raw(&self) -> &[(Sym, Sym)] {
        &self.pairs
    }

    /// Build from an iterator.
    pub fn from_pairs<I: IntoIterator<Item = (Sym, Sym)>>(iter: I) -> Self {
        let mut tags = Self::new();
        for (k, v) in iter {
            tags.set(k, v);
        }
        tags
    }

    /// Estimate heap usage (only when the inline capacity is exceeded).
    pub fn mem_bytes(&self) -> usize {
        // SmallVec inline capacity is 6 pairs; only longer sets allocate.
        if self.pairs.spilled() {
            self.pairs.capacity() * std::mem::size_of::<(Sym, Sym)>()
        } else {
            0
        }
    }
}
