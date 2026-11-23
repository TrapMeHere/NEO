//! 流式 LOD：按缩放分级，并用有界瓦片缓存避免长时间会话把场景副本堆满。

use std::collections::{HashMap, VecDeque};

use neo_geo::{Camera, WorldPoint};

use crate::batches::SceneFrame;
use crate::builder::{SceneBuilder, SceneConfig};

/// 缩放分级。数字越小，抽样越狠。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LodBand {
    /// 洲/国家。
    Overview,
    /// 城市。
    City,
    /// 街区。
    Street,
    /// 编辑（节点可见的尺度）。
    Edit,
}

/// 按 zoom 选分级。
pub fn lod_band(zoom: f64) -> LodBand {
    if zoom < 8.0 {
        LodBand::Overview
    } else if zoom < 13.0 {
        LodBand::City
    } else if zoom < 17.0 {
        LodBand::Street
    } else {
        LodBand::Edit
    }
}

/// 该分级下的 way 抽样步长（1 = 不抽样）。
pub fn way_stride(band: LodBand) -> usize {
    match band {
        LodBand::Overview => 8,
        LodBand::City => 2,
        LodBand::Street | LodBand::Edit => 1,
    }
}

/// 把分级写进场景配置（不改动调用方的其他字段）。
pub fn apply_band(config: &mut SceneConfig, band: LodBand) {
    config.way_stride = way_stride(band);
    if matches!(band, LodBand::Overview) {
        config.simplify_tolerance_px = config.simplify_tolerance_px.max(2.0);
    }
}

/// WebMercator 瓦片键。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TileKey {
    /// 缩放。
    pub z: u8,
    /// 列。
    pub x: u32,
    /// 行。
    pub y: u32,
}

/// 经纬度 → 瓦片。`z` 会被限制在 0..=22。
pub fn tile_key(lat: f64, lon: f64, z: u8) -> TileKey {
    let z = z.min(22);
    let n = f64::from(1u32 << z);
    let x = ((lon + 180.0) / 360.0 * n).floor().clamp(0.0, n - 1.0) as u32;
    let lat_rad = lat.clamp(-85.0, 85.0).to_radians();
    let y = ((1.0 - (lat_rad.tan() + 1.0 / lat_rad.cos()).ln() / std::f64::consts::PI) / 2.0 * n)
        .floor()
        .clamp(0.0, n - 1.0) as u32;
    TileKey { z, x, y }
}

/// 相机中心所在的瓦片（zoom 取整）。
pub fn camera_tile(camera: &Camera) -> TileKey {
    let ll = camera.world_to_latlon(camera.center);
    let (lat, lon) = ll.to_degrees();
    tile_key(lat, lon, camera.zoom.floor() as u8)
}

/// 有界瓦片缓存。超过容量时丢掉最旧的一帧，避免会话期间单调增长。
#[derive(Debug)]
pub struct TileCache {
    cap: usize,
    order: VecDeque<TileKey>,
    tiles: HashMap<TileKey, SceneFrame>,
}

impl TileCache {
    /// 容量至少为 1。
    pub fn new(cap: usize) -> Self {
        Self {
            cap: cap.max(1),
            order: VecDeque::new(),
            tiles: HashMap::new(),
        }
    }

    /// 已缓存帧数。
    pub fn len(&self) -> usize {
        self.tiles.len()
    }

    /// 是否为空。
    pub fn is_empty(&self) -> bool {
        self.tiles.is_empty()
    }

    /// 命中则刷新为最近使用。
    pub fn get(&mut self, key: &TileKey) -> Option<&SceneFrame> {
        if !self.tiles.contains_key(key) {
            return None;
        }
        if let Some(pos) = self.order.iter().position(|k| k == key)
            && let Some(k) = self.order.remove(pos)
        {
            self.order.push_back(k);
        }
        self.tiles.get(key)
    }

    /// 插入并在超容量时淘汰最旧项。
    pub fn insert(&mut self, key: TileKey, frame: SceneFrame) {
        if let std::collections::hash_map::Entry::Occupied(mut e) = self.tiles.entry(key) {
            e.insert(frame);
            let _ = self.get(&key);
            return;
        }
        while self.tiles.len() >= self.cap {
            if let Some(old) = self.order.pop_front() {
                self.tiles.remove(&old);
            } else {
                break;
            }
        }
        self.order.push_back(key);
        self.tiles.insert(key, frame);
    }

    /// 顶点与索引占用的字节（缓存上限的可观测量）。
    pub fn retained_bytes(&self) -> usize {
        self.tiles
            .values()
            .map(|frame| {
                frame
                    .batches
                    .iter()
                    .map(|batch| batch.vertices.len() * 12 + batch.indices.len() * 4)
                    .sum::<usize>()
            })
            .sum()
    }
}

/// 按相机瓦片缓存场景。同一瓦片第二次构建不会再跑 `SceneBuilder`。
pub struct StreamingScene {
    cache: TileCache,
    /// 实际构建次数（缓存命中不增加）。
    pub builds: u64,
}

impl StreamingScene {
    /// `cap` 是同时保留的瓦片帧数。
    pub fn new(cap: usize) -> Self {
        Self {
            cache: TileCache::new(cap),
            builds: 0,
        }
    }

    /// 当前缓存帧数。
    pub fn cached(&self) -> usize {
        self.cache.len()
    }

    /// 缓存占用字节。
    pub fn retained_bytes(&self) -> usize {
        self.cache.retained_bytes()
    }

    /// 取相机所在瓦片的场景。未命中时按 LOD 分级构建。
    pub fn frame(
        &mut self,
        prototype: &SceneBuilder,
        data: &neo_store::DataSet,
        index: &neo_geo::SpatialIndex,
        referrers: &neo_store::Referrers,
        world_cache: &neo_geo::WorldCache,
        camera: &Camera,
    ) -> SceneFrame {
        let key = camera_tile(camera);
        if let Some(hit) = self.cache.get(&key) {
            return hit.clone();
        }
        let mut config = prototype.config.clone();
        apply_band(&mut config, lod_band(camera.zoom));
        let builder = SceneBuilder::new(prototype.style.clone(), config);
        let frame = builder.build(data, index, referrers, world_cache, camera);
        self.builds += 1;
        self.cache.insert(key, frame.clone());
        frame
    }
}

/// 给测试用的空帧（只占固定字节）。
pub fn placeholder_frame(origin: WorldPoint) -> SceneFrame {
    SceneFrame::empty(0, Camera::new(origin, 3.0, [32, 32]))
}
