//! # neo-geo — spatial and geometry layer
//!
//! Responsibilities:
//!
//! * [`world`]: WebMercator world coordinates (f64 meters) and bounding boxes;
//! * [`camera`]: world coordinates ↔ screen pixels, zoom/pan/fit;
//! * [`spatial`]: rstar R-tree object index (node points + way AABBs), with incremental updates;
//! * [`geometry`]: polylines/polygon triangulation/stroke generation/simplification/distance;
//! * [`picking`]: hit testing with a screen-pixel tolerance (without iterating all objects).
//!
//! Dependency direction: `neo-core`, `neo-osm`, `neo-store` (read-only dataset) → this crate → `neo-scene`.
//! This crate does not depend on GPU/UI/IO.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod camera;
pub mod geometry;
pub mod picking;
pub mod snap;
pub mod spatial;
pub mod world;
pub mod world_cache;

pub use camera::{Camera, MAX_ZOOM, MIN_ZOOM, REBASE_THRESHOLD_M, TILE_SIZE};
pub use geometry::{BoxSelectMode, box_select_mode, segment_intersects_bounds, way_matches_box};
pub use picking::{Hit, HitTester};
pub use snap::{SnapHit, SnapTarget, closest_way_segment, resolve_snap, resolve_snap_unfiltered};
pub use spatial::{SpatialEntry, SpatialIndex};
pub use world::{
    EARTH_CIRCUMFERENCE_M, EARTH_RADIUS_M, MAX_MERCATOR_LAT, WorldBounds, WorldPoint, distance_meters,
};
pub use world_cache::WorldCache;
