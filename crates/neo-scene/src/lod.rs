//! Streaming LOD: zoom-based tiers, plus a bounded tile cache to prevent long sessions from filling up with scene copies.

use std::collections::{HashMap, VecDeque};

use neo_geo::{Camera, WorldPoint};

use crate::batches::SceneFrame;
use crate::builder::{SceneBuilder, SceneConfig};

/// Zoom tier. Smaller numbers mean harsher sampling.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LodBand {
    /// Continent/country.
    Overview,
    /// City.
    City,
    /// Street.
    Street,
    /// Edit (the scale at which nodes are visible).
    Edit,
}

/// Select the tier by zoom.
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

/// Way sampling stride for this tier (1 = no sampling).
pub fn way_stride(band: LodBand) -> usize {
    match band {
        LodBand::Overview => 8,
        LodBand::City => 2,
        LodBand::Street | LodBand::Edit => 1,
    }
}

/// Write the tier into the scene config (without touching the caller's other fields).
pub fn apply_band(config: &mut SceneConfig, band: LodBand) {
    config.way_stride = way_stride(band);
    if matches!(band, LodBand::Overview) {
        config.simplify_tolerance_px = config.simplify_tolerance_px.max(2.0);
    }
}

/// WebMercator tile key.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TileKey {
    /// Zoom.
    pub z: u8,
    /// Column.
    pub x: u32,
    /// Row.
    pub y: u32,
}

/// Lon/lat → tile. `z` is clamped to 0..=22.
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

/// Tile containing the camera center (zoom rounded).
pub fn camera_tile(camera: &Camera) -> TileKey {
    let ll = camera.world_to_latlon(camera.center);
    let (lat, lon) = ll.to_degrees();
    tile_key(lat, lon, camera.zoom.floor() as u8)
}

/// Bounded tile cache. When capacity is exceeded, drop the oldest frame to avoid monotonic growth during a session.
#[derive(Debug)]
pub struct TileCache {
    cap: usize,
    order: VecDeque<TileKey>,
    tiles: HashMap<TileKey, SceneFrame>,
}

impl TileCache {
    /// Capacity is at least 1.
    pub fn new(cap: usize) -> Self {
        Self {
            cap: cap.max(1),
            order: VecDeque::new(),
            tiles: HashMap::new(),
        }
    }

    /// Number of cached frames.
    pub fn len(&self) -> usize {
        self.tiles.len()
    }

    /// Whether it is empty.
    pub fn is_empty(&self) -> bool {
        self.tiles.is_empty()
    }

    /// On a hit, refresh it as most recently used.
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

    /// Insert and evict the oldest item when over capacity.
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

    /// Bytes occupied by vertices and indices (an observable measure of the cache bound).
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

/// Cache scenes by camera tile. A second build for the same tile does not re-run `SceneBuilder`.
pub struct StreamingScene {
    cache: TileCache,
    /// Actual build count (cache hits do not increase it).
    pub builds: u64,
}

impl StreamingScene {
    /// `cap` is the number of tile frames kept simultaneously.
    pub fn new(cap: usize) -> Self {
        Self {
            cache: TileCache::new(cap),
            builds: 0,
        }
    }

    /// Current number of cached frames.
    pub fn cached(&self) -> usize {
        self.cache.len()
    }

    /// Bytes used by the cache.
    pub fn retained_bytes(&self) -> usize {
        self.cache.retained_bytes()
    }

    /// Get the scene for the camera's tile. On a miss, build it according to the LOD tier.
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

/// Empty frame for tests (occupies a fixed number of bytes).
pub fn placeholder_frame(origin: WorldPoint) -> SceneFrame {
    SceneFrame::empty(0, Camera::new(origin, 3.0, [32, 32]))
}
