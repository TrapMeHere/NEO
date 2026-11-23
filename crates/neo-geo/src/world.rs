//! WebMercator 世界坐标。
//!
//! 坐标栈（很重要，三层必须分清）：
//!
//! ```text
//! LatLon (i32, 1e-7 度, 数据层)
//!    │  forward
//!    ▼
//! WorldPoint (f64, WebMercator 米, 空间/索引层)
//!    │  Camera::world_to_screen
//!    ▼
//! ScreenPoint (f32, 物理像素, 渲染/UI 层)
//! ```
//!
//! WebMercator 是**等角**投影：小区域内形状保持，这是 OSM 编辑器/瓦片服务的标准选择。
//! 它不保持距离，因此测距要用 haversine（见 `distance_meters`），不能用世界坐标差。
//!
//! 用 f64 存世界坐标：地球周长 4.0075e7 米，f64 在这个量级的分辨率约 1e-9 米，
//! 直到 zoom 25 都不会有精度问题。GPU 侧再转到相机原点相对坐标（f32），见 `scene`。

use neo_core::LatLon;

/// WebMercator 球体半径（米，OSM 约定）。
pub const EARTH_RADIUS_M: f64 = 6_378_137.0;

/// 赤道周长（米）。
pub const EARTH_CIRCUMFERENCE_M: f64 = 40_075_016.685_578_5;

/// WebMercator 允许的最大纬度（约 ±85.05112878°）。
pub const MAX_MERCATOR_LAT: f64 = 85.051_128_779_806_6;

/// 世界坐标：WebMercator 米。
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct WorldPoint {
    /// 东向（米，0 = 180°W）。
    pub x: f64,
    /// 南向（米，0 = 北纬 85.05°）。
    pub y: f64,
}

impl WorldPoint {
    /// 构造。
    #[inline]
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }

    /// 经纬度 → 世界坐标。
    pub fn from_latlon(p: LatLon) -> Self {
        let (lat, lon) = p.to_degrees();
        Self::from_degrees(lat, lon)
    }

    /// 十进制度 → 世界坐标。
    pub fn from_degrees(lat: f64, lon: f64) -> Self {
        let lat = lat.clamp(-MAX_MERCATOR_LAT, MAX_MERCATOR_LAT);
        // x=0 对应 180°W，x=C 对应 180°E（OSM 瓦片约定）。
        let lon = lon.clamp(-180.0, 180.0);
        let x = (lon + 180.0) / 360.0 * EARTH_CIRCUMFERENCE_M;
        let lat_rad = lat.to_radians();
        let y_norm = (1.0 - (lat_rad.tan() + 1.0 / lat_rad.cos()).ln() / core::f64::consts::PI) / 2.0;
        Self {
            x,
            y: y_norm * EARTH_CIRCUMFERENCE_M,
        }
    }

    /// 世界坐标 → 经纬度。
    pub fn to_latlon(self) -> LatLon {
        let lon = self.x / EARTH_CIRCUMFERENCE_M * 360.0 - 180.0;
        let y_norm = 1.0 - 2.0 * self.y / EARTH_CIRCUMFERENCE_M;
        let lat = (core::f64::consts::PI * y_norm).sinh().atan().to_degrees();
        LatLon::from_degrees(lat, lon)
    }

    /// 世界坐标差 → 屏幕像素距离（需要相机缩放）。
    #[inline]
    pub fn dist(self, other: Self) -> f64 {
        ((self.x - other.x).powi(2) + (self.y - other.y).powi(2)).sqrt()
    }

    /// 加上向量。
    #[inline]
    pub fn offset(self, dx: f64, dy: f64) -> Self {
        Self::new(self.x + dx, self.y + dy)
    }
}

/// 世界坐标包围盒。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WorldBounds {
    /// 最小点（西北）。
    pub min: WorldPoint,
    /// 最大点（东南）。
    pub max: WorldPoint,
}

impl Default for WorldBounds {
    fn default() -> Self {
        Self::EMPTY
    }
}

impl WorldBounds {
    /// 空盒（反向极值，便于 expand）。
    pub const EMPTY: Self = Self {
        min: WorldPoint {
            x: f64::INFINITY,
            y: f64::INFINITY,
        },
        max: WorldPoint {
            x: f64::NEG_INFINITY,
            y: f64::NEG_INFINITY,
        },
    };

    /// 由两个点构造（自动排序）。
    pub fn from_points(a: WorldPoint, b: WorldPoint) -> Self {
        Self {
            min: WorldPoint::new(a.x.min(b.x), a.y.min(b.y)),
            max: WorldPoint::new(a.x.max(b.x), a.y.max(b.y)),
        }
    }

    /// 由点集构造。
    pub fn from_points_iter<I: IntoIterator<Item = WorldPoint>>(iter: I) -> Self {
        let mut b = Self::EMPTY;
        for p in iter {
            b.expand_point(p);
        }
        b
    }

    /// 单点盒。
    pub fn from_point(p: WorldPoint) -> Self {
        Self { min: p, max: p }
    }

    /// 是否为空。
    pub fn is_empty(&self) -> bool {
        self.min.x > self.max.x || self.min.y > self.max.y
    }

    /// 扩展以包含一个点。
    pub fn expand_point(&mut self, p: WorldPoint) {
        self.min.x = self.min.x.min(p.x);
        self.min.y = self.min.y.min(p.y);
        self.max.x = self.max.x.max(p.x);
        self.max.y = self.max.y.max(p.y);
    }

    /// 扩展以包含一个盒子。
    pub fn expand(&mut self, other: &Self) {
        if other.is_empty() {
            return;
        }
        self.min.x = self.min.x.min(other.min.x);
        self.min.y = self.min.y.min(other.min.y);
        self.max.x = self.max.x.max(other.max.x);
        self.max.y = self.max.y.max(other.max.y);
    }

    /// 包含点。
    #[inline]
    pub fn contains(&self, p: WorldPoint) -> bool {
        p.x >= self.min.x && p.x <= self.max.x && p.y >= self.min.y && p.y <= self.max.y
    }

    /// 与另一盒相交（含边界）。
    #[inline]
    pub fn intersects(&self, other: &Self) -> bool {
        !(other.min.x > self.max.x
            || other.max.x < self.min.x
            || other.min.y > self.max.y
            || other.max.y < self.min.y)
    }

    /// 中心。
    pub fn center(&self) -> WorldPoint {
        WorldPoint::new((self.min.x + self.max.x) * 0.5, (self.min.y + self.max.y) * 0.5)
    }

    /// 宽高（米）。
    pub fn size(&self) -> (f64, f64) {
        (self.max.x - self.min.x, self.max.y - self.min.y)
    }

    /// 转成 EPSG:3857 的 `minx,miny,maxx,maxy`（米，y 向北）。
    ///
    /// 本仓库的世界坐标 x 从 180°W 起算、y 向南；标准 3857 的原点在赤道与本初子午线，y 向北。
    pub fn to_epsg3857(self) -> [f64; 4] {
        let half = EARTH_CIRCUMFERENCE_M / 2.0;
        [
            self.min.x - half,
            half - self.max.y,
            self.max.x - half,
            half - self.min.y,
        ]
    }

    /// 命中测试用的放大（米）。
    pub fn expanded_by(&self, meters: f64) -> Self {
        Self {
            min: WorldPoint::new(self.min.x - meters, self.min.y - meters),
            max: WorldPoint::new(self.max.x + meters, self.max.y + meters),
        }
    }
}

/// 两点间的测地线距离（米，haversine）。
pub fn distance_meters(a: LatLon, b: LatLon) -> f64 {
    let (lat1, lon1) = (a.lat.to_degrees().to_radians(), a.lon.to_degrees().to_radians());
    let (lat2, lon2) = (b.lat.to_degrees().to_radians(), b.lon.to_degrees().to_radians());
    let dlat = lat2 - lat1;
    let dlon = lon2 - lon1;
    let h = (dlat * 0.5).sin().powi(2) + lat1.cos() * lat2.cos() * (dlon * 0.5).sin().powi(2);
    2.0 * EARTH_RADIUS_M * h.sqrt().asin()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn equator_origin_is_epsg3857_zero() {
        let point = WorldPoint::from_degrees(0.0, 0.0);
        let bbox = WorldBounds::from_point(point).to_epsg3857();
        assert!(bbox[0].abs() < 1.0, "{}", bbox[0]);
        assert!(bbox[1].abs() < 1.0, "{}", bbox[1]);
        assert!(bbox[2].abs() < 1.0, "{}", bbox[2]);
        assert!(bbox[3].abs() < 1.0, "{}", bbox[3]);
    }
}
