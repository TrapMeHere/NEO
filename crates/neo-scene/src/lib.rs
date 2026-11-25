//! # neo-scene — scene assembly layer
//!
//! Input: read-only [`neo_store::DataSet`] + [`neo_geo::SpatialIndex`] + camera + style;
//! Output: [`batches::SceneFrame`] (vertex/index/color batches).
//!
//! This layer **does not depend on the GPU or the UI**, so it can:
//!
//! * be unit-tested in CI without a GPU;
//! * be benchmarked directly (scene build time is the largest CPU-side cost);
//! * be reused by other future backends (SVG/PDF export, headless rendering).
//!
//! Incremental strategy: geometry is rebuilt only on "data change / camera moves past the rebase threshold"; pan/zoom rely on a GPU uniform.
//! See docs/05 and ADR-0009 for details.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod batches;
pub mod builder;
pub mod cull;
pub mod lod;
pub mod parallel;

pub use batches::{
    AreaBand, BatchKind, LabelPlace, MapVertex, NAME_COLOR, RenderBatch, SceneFrame, SceneIcon, SceneLabel,
    SceneStats, name_color,
};
pub use builder::{
    NODE_ALL_ZOOM, NODE_MIN_ZOOM, SHARED_NODE_MIN_WAYS, SceneBuilder, SceneConfig, node_visible_at,
};
pub use cull::instance_count;
pub use lod::{
    LodBand, StreamingScene, TileCache, TileKey, apply_band, camera_tile, lod_band, tile_key, way_stride,
};
pub use parallel::{build_parallel, build_serial};
