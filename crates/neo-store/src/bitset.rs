//! Compact bitset, used for dirty sets, visible sets, deletion marks, etc.
//!
//! Dirty flags for 10M objects take only 1.25 MB (`Vec<u64>`), whereas `HashSet<u32>` would grow to
//! hundreds of MB with poor cache behavior. A bitset is the first choice on the editing path.

use neo_core::ByteSize;

/// Compact bitset.
#[derive(Debug, Default, Clone)]
pub struct BitSet {
    words: Vec<u64>,
    len: usize,
}

impl BitSet {
    /// Create an empty bitset.
    pub fn new() -> Self {
        Self::default()
    }

    /// Empty bitset with the given number of bits.
    pub fn with_bits(len: usize) -> Self {
        Self {
            words: vec![0; len.div_ceil(64)],
            len,
        }
    }

    /// Number of bits.
    #[inline]
    pub fn len(&self) -> usize {
        self.len
    }

    /// Whether there are no bits at all.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Ensure capacity for `len` bits.
    pub fn ensure_len(&mut self, len: usize) {
        if len > self.len {
            self.len = len;
            let words = len.div_ceil(64);
            if self.words.len() < words {
                self.words.resize(words, 0);
            }
        }
    }

    /// Set a bit; grows automatically when out of range.
    #[inline]
    pub fn set(&mut self, index: usize) {
        if index >= self.len {
            self.ensure_len(index + 1);
        }
        let (w, b) = (index / 64, index % 64);
        self.words[w] |= 1u64 << b;
    }

    /// Clear a bit.
    #[inline]
    pub fn clear(&mut self, index: usize) {
        if index < self.len {
            let (w, b) = (index / 64, index % 64);
            self.words[w] &= !(1u64 << b);
        }
    }

    /// Read a bit.
    #[inline]
    pub fn get(&self, index: usize) -> bool {
        if index >= self.len {
            return false;
        }
        let (w, b) = (index / 64, index % 64);
        (self.words[w] >> b) & 1 == 1
    }

    /// Clear all bits (keeping capacity).
    pub fn clear_all(&mut self) {
        self.words.fill(0);
    }

    /// Number of bits that are set.
    pub fn count_ones(&self) -> usize {
        self.words.iter().map(|w| w.count_ones() as usize).sum()
    }

    /// Iterate all set indices (ascending).
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

    /// Estimate memory.
    pub fn mem_bytes(&self) -> ByteSize {
        ByteSize(self.words.capacity() * core::mem::size_of::<u64>())
    }
}
