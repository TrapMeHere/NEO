//! # neo-store — compact storage layer for OSM data
//!
//! Goal: tens of millions of OSM objects that can be edited, rendered, and uploaded in a single machine's memory.
//!
//! Core techniques (see docs/03 and ADR-0003):
//!
//! * **SoA**: `Vec<i32>` for coordinates, `Vec<ObjectId>` for IDs, `Vec<TagSetId>` for tag references,
//!   avoiding one heap allocation and pointer chase per object;
//! * **Arena + generation**: stale-handle detection, so delete/undo never silently points at the wrong object;
//! * **Interned strings**: tag text is stored once;
//! * **Append-only reference pool**: a way's node references are stored contiguously, edits never move other objects;
//! * **Sparse change tracking**: only edited objects are recorded;
//! * **Bitmap dirty sets**: the basis for incremental geometry/render/validation rebuilds.
//!
//! This crate does not depend on GUI, GPU, or network; it depends only on `neo-core` and `neo-osm`.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod bitset;
pub mod compact;
pub mod dataset;
pub mod indexes;
pub mod integrity;
pub mod node_store;
pub mod relation_store;
pub mod tag_store;
pub mod way_store;

pub use bitset::BitSet;
pub use compact::{
    decode_coords, decode_delta_varints, decode_sorted_ids, encode_coords, encode_delta_varints,
    encode_sorted_ids,
};
pub use dataset::{DataSet, MissingPolicy, NodeView, RelationView, StoreStats, WayView};
pub use indexes::{ChangeTrack, IdIndex, Referrers};
pub use integrity::{IntegrityIssue, IntegrityReport, check as check_integrity};
pub use node_store::{NodeRef, NodeSlot, NodeStore};
pub use relation_store::{AnyHandle, RelationRef, RelationSlot, RelationStore, StoredMember};
pub use tag_store::{TagSetId, TagStore};
pub use way_store::{WayRef, WaySlot, WayStore};
