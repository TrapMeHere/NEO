//! Multi-threaded scene building: one `SceneBuilder` per camera, with results in the same order as serial.
//!
//! `SceneBuilder` has `RefCell` scratch internally and cannot be shared across threads; here the style is cloned per camera and then run in parallel.

use neo_geo::{Camera, SpatialIndex};
use neo_store::{DataSet, Referrers};
use rayon::prelude::*;

use crate::batches::SceneFrame;
use crate::builder::SceneBuilder;

/// Serial build (for comparison).
pub fn build_serial(
    prototype: &SceneBuilder,
    data: &DataSet,
    index: &SpatialIndex,
    referrers: &Referrers,
    world_cache: &neo_geo::WorldCache,
    cameras: &[Camera],
) -> Vec<SceneFrame> {
    cameras
        .iter()
        .map(|camera| {
            let builder = SceneBuilder::new(prototype.style.clone(), prototype.config.clone());
            builder.build(data, index, referrers, world_cache, camera)
        })
        .collect()
}

/// Parallel build. One rayon task per camera.
pub fn build_parallel(
    prototype: &SceneBuilder,
    data: &DataSet,
    index: &SpatialIndex,
    referrers: &Referrers,
    world_cache: &neo_geo::WorldCache,
    cameras: &[Camera],
) -> Vec<SceneFrame> {
    // First clone the Sync style and config. `SceneBuilder` itself contains a `RefCell` and cannot be shared across threads.
    let style = prototype.style.clone();
    let config = prototype.config.clone();
    cameras
        .par_iter()
        .map(|camera| {
            let builder = SceneBuilder::new(style.clone(), config.clone());
            builder.build(data, index, referrers, world_cache, camera)
        })
        .collect()
}
