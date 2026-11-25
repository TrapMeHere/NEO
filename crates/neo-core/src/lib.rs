//! # neo-core — NEO's foundational primitives
//!
//! This layer is the leaf of the whole workspace: it **depends on no other NEO crate**, nor on GUI, GPU, or IO.
//!
//! It provides four kinds of primitives:
//!
//! * [`ObjectId`] / [`ObjectKind`] —— external OSM identifiers and object kinds. OSM IDs are never used as array indices.
//! * [`Handle<T>`] —— internal typed handle with a generation, preventing stale handles from being reused after deletion.
//! * [`Sym`] / [`StringPool`] —— string interning, so tags occupy only a 4-byte symbol in memory.
//! * [`Coord`] / [`LatLon`] —— fixed-point coordinates (1e-7 degree, i.e. OSM API precision), avoiding floating-point drift.
//!
//! Design constraints: `docs/03-osm-data-model-and-store.md`.

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
