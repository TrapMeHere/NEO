//! Memory usage estimation.
//!
//! The performance docs require reporting "bytes per object". `std` has no stable capacity query API, so the
//! storage layer uniformly implements [`MemSize`], estimating with `capacity * size_of::<T>()` (excluding
//! allocator metadata). The difference between this figure and RSS is reported side by side by the benchmark
//! tooling, see docs/14.

use core::mem::size_of;

/// Capacity in bytes, convenient for direct printing.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Default)]
pub struct ByteSize(pub usize);

impl ByteSize {
    /// Zero.
    pub const ZERO: Self = Self(0);

    /// Number of bytes.
    #[inline]
    pub const fn bytes(self) -> usize {
        self.0
    }

    /// Expressed in MiB.
    #[inline]
    pub fn mib(self) -> f64 {
        self.0 as f64 / (1024.0 * 1024.0)
    }
}

impl core::ops::Add for ByteSize {
    type Output = Self;
    fn add(self, rhs: Self) -> Self::Output {
        Self(self.0 + rhs.0)
    }
}

impl core::ops::AddAssign for ByteSize {
    fn add_assign(&mut self, rhs: Self) {
        self.0 += rhs.0;
    }
}

impl core::fmt::Display for ByteSize {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        if self.0 >= 1024 * 1024 {
            write!(f, "{:.1} MiB", self.mib())
        } else if self.0 >= 1024 {
            write!(f, "{:.1} KiB", self.0 as f64 / 1024.0)
        } else {
            write!(f, "{} B", self.0)
        }
    }
}

/// Types that support memory estimation.
pub trait MemSize {
    /// Estimated heap bytes used.
    fn mem_bytes(&self) -> ByteSize;
}

impl<T> MemSize for Vec<T> {
    #[inline]
    fn mem_bytes(&self) -> ByteSize {
        ByteSize(self.capacity() * size_of::<T>())
    }
}

/// Byte size of a `Vec<T>`'s capacity.
#[inline]
pub fn vec_bytes<T>(v: &Vec<T>) -> usize {
    v.capacity() * size_of::<T>()
}
