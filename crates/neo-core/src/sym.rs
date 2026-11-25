//! String interning.
//!
//! OSM tag keys/values are highly repetitive (`highway`, `building`, `residential` …); if tens of millions
//! of objects each held a `String`, memory and caching would collapse. Here every tag string is interned
//! into a 4-byte [`Sym`], and the real text is stored only once.
//!
//! Notes:
//! * the interning pool is **one per DataSet**, not shared across projects (otherwise unload/recovery semantics get complicated);
//! * a `Sym` is only meaningful together with its owning [`StringPool`]; serialization must not write the bare `u32`,
//!   the pool contents must be persisted along with it (see docs/10).
//! * index 0 is permanently reserved for the empty string.

use core::fmt;
use std::collections::HashMap;

/// Interned string symbol (4 bytes).
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct Sym(u32);

impl Sym {
    /// Empty string.
    pub const EMPTY: Self = Self(0);

    /// Construct from a raw index (persistence/deserialization paths only).
    #[inline]
    pub const fn from_raw(raw: u32) -> Self {
        Self(raw)
    }

    /// Raw index.
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

/// String interning pool: O(1) both ways for `&str -> Sym` and `Sym -> &str`.
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
    /// Create a new pool; index 0 is the empty string.
    pub fn new() -> Self {
        let mut pool = Self {
            strings: Vec::new(),
            lookup: HashMap::new(),
            total_bytes: 0,
        };
        pool.intern("");
        pool
    }

    /// Intern a string. Returns the existing symbol if already present.
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

    /// Resolve a symbol; returns `None` when out of range (corrupt persistence / raw index from a plugin).
    #[inline]
    pub fn try_resolve(&self, sym: Sym) -> Option<&str> {
        self.strings.get(sym.0 as usize).map(|s| s.as_ref())
    }

    /// Resolve a symbol; returns the empty string instead of panicking when out of range (safe-boundary API).
    #[inline]
    pub fn resolve(&self, sym: Sym) -> &str {
        self.try_resolve(sym).unwrap_or("")
    }

    /// Number of interned strings.
    #[inline]
    pub fn len(&self) -> usize {
        self.strings.len()
    }

    /// Whether the pool contains only the empty string.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.strings.len() <= 1
    }

    /// Number of text bytes (excluding index structure overhead).
    #[inline]
    pub fn text_bytes(&self) -> usize {
        self.total_bytes
    }

    /// Estimate total memory usage in bytes, for memory benchmarks.
    pub fn mem_bytes(&self) -> usize {
        // Vec<Box<str>>: 16B each + text on the heap; HashMap entry estimated as 2*(16B + usize + control byte).
        self.strings.capacity() * core::mem::size_of::<Box<str>>()
            + self.total_bytes
            + self.lookup.capacity() * (core::mem::size_of::<Box<str>>() + core::mem::size_of::<u32>() + 1)
            + self.total_bytes // one duplicate Box<str> in lookup
    }
}
