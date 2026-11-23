//! # neo-core — NEO 的基础原语
//!
//! 这一层是整个 workspace 的叶子：**不依赖任何其他 NEO crate**，也不依赖 GUI、GPU、IO。
//!
//! 提供四类基础原语：
//!
//! * [`ObjectId`] / [`ObjectKind`] —— 外部 OSM 标识与对象种类。OSM ID 绝不用作数组下标。
//! * [`Handle<T>`] —— 内部带 generation 的类型化句柄，避免删除后旧句柄被错误复用。
//! * [`Sym`] / [`StringPool`] —— 字符串驻留，让 Tag 在内存里只存 4 字节符号。
//! * [`Coord`] / [`LatLon`] —— 定点坐标（1e-7 度，即 OSM API 精度），避免浮点漂移。
//!
//! 设计约束见 `docs/03-osm-data-model-and-store.md`。

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod error;
pub mod handle;
pub mod ids;
pub mod meta;
pub mod obfuscate;
pub mod stats;
pub mod sym;
pub mod units;

pub use error::{NeoError, Result};
pub use handle::{Handle, SlotAllocator};
pub use ids::{ObjectId, ObjectKind};
pub use meta::Meta;
pub use obfuscate::reveal;
pub use stats::{ByteSize, MemSize};
pub use sym::{StringPool, Sym};
pub use units::{Coord, LatLon};
