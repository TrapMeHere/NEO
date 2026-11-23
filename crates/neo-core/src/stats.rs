//! 内存占用估算。
//!
//! 性能文档要求报告"每对象字节数"。`std` 没有稳定的容量查询 API，因此存储层统一实现
//! [`MemSize`]，用 `capacity * size_of::<T>()` 估算（不含分配器元数据）。
//! 该数值与 RSS 的差值由基准工具对照报告，见 docs/14。

use core::mem::size_of;

/// 以字节为单位的容量，便于直接打印。
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Default)]
pub struct ByteSize(pub usize);

impl ByteSize {
    /// 0。
    pub const ZERO: Self = Self(0);

    /// 字节数。
    #[inline]
    pub const fn bytes(self) -> usize {
        self.0
    }

    /// 以 MiB 表示。
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

/// 实现内存估算的类型。
pub trait MemSize {
    /// 估算占用的堆字节数。
    fn mem_bytes(&self) -> ByteSize;
}

impl<T> MemSize for Vec<T> {
    #[inline]
    fn mem_bytes(&self) -> ByteSize {
        ByteSize(self.capacity() * size_of::<T>())
    }
}

/// `Vec<T>` 容量的字节数。
#[inline]
pub fn vec_bytes<T>(v: &Vec<T>) -> usize {
    v.capacity() * size_of::<T>()
}
