//! 多线程场景构建：每个相机一份 `SceneBuilder`，结果顺序与串行一致。
//!
//! `SceneBuilder` 内部有 `RefCell` scratch，不能跨线程共享；这里按相机克隆样式后并行。

use neo_geo::{Camera, SpatialIndex};
use neo_store::{DataSet, Referrers};
use rayon::prelude::*;

use crate::batches::SceneFrame;
use crate::builder::SceneBuilder;

/// 串行构建（对照用）。
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

/// 并行构建。每个相机一个 rayon 任务。
pub fn build_parallel(
    prototype: &SceneBuilder,
    data: &DataSet,
    index: &SpatialIndex,
    referrers: &Referrers,
    world_cache: &neo_geo::WorldCache,
    cameras: &[Camera],
) -> Vec<SceneFrame> {
    // 先克隆出 Sync 的样式和配置。`SceneBuilder` 自己含 RefCell，不能跨线程共享。
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
