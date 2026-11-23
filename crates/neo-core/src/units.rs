//! 定点坐标与经纬度。
//!
//! OSM 的权威精度是 1e-7 度（API 只接受 7 位小数；PBF 内部是 1e-9 度的整数值，
//! 但落库/上传都会回到 1e-7）。用 `i32` 存 1e-7 度：
//!
//! * 经度 ±180° → ±1_800_000_000，落在 `i32`（±2_147_483_647）范围内；
//! * 纬度 ±90°  → ±900_000_000；
//! * 4 字节/坐标，1000 万节点仅坐标就比 `f64` 省 80 MB。
//!
//! 代价：PBF 中 1e-9 精度的值在导入时按"四舍五入到 1e-7"归一，这是与 OSM API 一致的
//! 语义；如果将来需要 1e-9 保真往返，需要额外的 `i64` 高精度变体（见 docs/03）。

use core::fmt;

/// 1 度 = 10_000_000 个坐标单位（1e-7 度）。
pub const COORD_UNITS_PER_DEGREE: i64 = 10_000_000;

/// 经度在 1e-7 度下的最大值。
pub const MAX_LON_UNITS: i32 = 1_800_000_000;
/// 纬度在 1e-7 度下的最大值。
pub const MAX_LAT_UNITS: i32 = 900_000_000;

/// 单个坐标分量，定点 1e-7 度。
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Coord(pub i32);

impl Coord {
    /// 0。
    pub const ZERO: Self = Self(0);

    /// 从原始 1e-7 度整数构造。
    #[inline]
    pub const fn from_units(units: i32) -> Self {
        Self(units)
    }

    /// 原始 1e-7 度整数。
    #[inline]
    pub const fn units(self) -> i32 {
        self.0
    }

    /// 从十进制度构造（四舍五入到 1e-7，并钳制到合法的 1e-7 度范围）。
    ///
    /// 非有限值（NaN/±inf）会得到 0：调用方若处理不可信输入，应先用
    /// [`Coord::try_from_degrees`] 做显式校验，避免把坏数据静默变成"几内亚湾的 0,0"。
    #[inline]
    pub fn from_degrees(deg: f64) -> Self {
        let scaled = if deg.is_finite() {
            (deg * COORD_UNITS_PER_DEGREE as f64).round()
        } else {
            0.0
        };
        Self(scaled.clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i32)
    }

    /// 从十进制度构造；非有限或超出 1e-7 度可表示范围时返回 `None`。
    #[inline]
    pub fn try_from_degrees(deg: f64) -> Option<Self> {
        if !deg.is_finite() {
            return None;
        }
        let scaled = (deg * COORD_UNITS_PER_DEGREE as f64).round();
        if scaled < f64::from(i32::MIN) || scaled > f64::from(i32::MAX) {
            return None;
        }
        Some(Self(scaled as i32))
    }

    /// 转十进制度。
    #[inline]
    pub fn to_degrees(self) -> f64 {
        f64::from(self.0) / COORD_UNITS_PER_DEGREE as f64
    }

    /// 从 OSM PBF 的 1e-9 度（nanodegree）值构造，四舍五入到 1e-7。
    ///
    /// 用 `i128` 中间量，避免 `nano + 50` 在 `i64::MAX` 附近溢出；超出 `i32`
    /// 可表示范围的值被钳制（调用方应同时做经纬度范围校验）。
    #[inline]
    pub fn from_nanodegrees(nano: i64) -> Self {
        // 100 nanodegree = 1e-7 degree；整数四舍五入，避免浮点大数误差。
        let n = i128::from(nano);
        let rounded = if n >= 0 { (n + 50) / 100 } else { (n - 50) / 100 };
        Self(rounded.clamp(i128::from(i32::MIN), i128::from(i32::MAX)) as i32)
    }

    /// 转 1e-9 度。
    #[inline]
    pub fn to_nanodegrees(self) -> i64 {
        i64::from(self.0) * 100
    }

    /// 是否在合法经纬度范围内（经度）。
    #[inline]
    pub fn is_valid_lon(self) -> bool {
        self.0.unsigned_abs() <= MAX_LON_UNITS as u32
    }

    /// 是否在合法经纬度范围内（纬度）。
    #[inline]
    pub fn is_valid_lat(self) -> bool {
        self.0.unsigned_abs() <= MAX_LAT_UNITS as u32
    }

    /// 两个坐标单位之差对应的米数近似（经度方向、赤道处）。
    #[inline]
    pub fn approx_meters(self, other: Coord) -> f64 {
        // 全程 f64：跨半球时 i32 相减（3.6e9）会溢出。
        let deg = (f64::from(self.0) - f64::from(other.0)) / COORD_UNITS_PER_DEGREE as f64;
        deg.abs() * 111_319.9
    }
}

impl fmt::Debug for Coord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Coord({:.7})", self.to_degrees())
    }
}

impl fmt::Display for Coord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:.7}", self.to_degrees())
    }
}

/// 经纬度点。
#[derive(Clone, Copy, PartialEq, Eq, Hash, Default, Debug)]
pub struct LatLon {
    /// 纬度。
    pub lat: Coord,
    /// 经度。
    pub lon: Coord,
}

impl LatLon {
    /// 构造。
    #[inline]
    pub const fn new(lat: Coord, lon: Coord) -> Self {
        Self { lat, lon }
    }

    /// 从十进制度构造。
    #[inline]
    pub fn from_degrees(lat: f64, lon: f64) -> Self {
        Self {
            lat: Coord::from_degrees(lat),
            lon: Coord::from_degrees(lon),
        }
    }

    /// 从十进制度构造；任一分量非法（非有限/超范围）时返回 `None`。
    #[inline]
    pub fn try_from_degrees(lat: f64, lon: f64) -> Option<Self> {
        Some(Self {
            lat: Coord::try_from_degrees(lat)?,
            lon: Coord::try_from_degrees(lon)?,
        })
        .filter(|p| p.is_valid())
    }
    /// 转 (lat, lon) 十进制度。
    #[inline]
    pub fn to_degrees(self) -> (f64, f64) {
        (self.lat.to_degrees(), self.lon.to_degrees())
    }

    /// 合法性检查（纬度、经度均在范围内）。
    #[inline]
    pub fn is_valid(self) -> bool {
        self.lat.is_valid_lat() && self.lon.is_valid_lon()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn i32_min_is_not_valid() {
        assert!(!Coord::from_units(i32::MIN).is_valid_lat());
        assert!(!Coord::from_units(i32::MIN).is_valid_lon());
        assert!(Coord::from_units(MAX_LAT_UNITS).is_valid_lat());
        assert!(Coord::from_units(-MAX_LON_UNITS).is_valid_lon());
    }

    #[test]
    fn approx_meters_does_not_overflow_across_hemispheres() {
        let west = Coord::from_units(-MAX_LON_UNITS);
        let east = Coord::from_units(MAX_LON_UNITS);
        let meters = west.approx_meters(east);
        assert!(meters.is_finite() && meters > 39_000_000.0);
    }

    #[test]
    fn try_from_degrees_rejects_nan_and_out_of_range() {
        assert!(Coord::try_from_degrees(f64::NAN).is_none());
        assert!(Coord::try_from_degrees(1e30).is_none());
        assert!(LatLon::try_from_degrees(91.0, 0.0).is_none());
        assert!(LatLon::try_from_degrees(0.0, 181.0).is_none());
        assert!(LatLon::try_from_degrees(47.0, 8.0).is_some());
    }

    #[test]
    fn nanodegrees_rounding_handles_extremes() {
        assert_eq!(Coord::from_nanodegrees(150).units(), 2);
        assert_eq!(Coord::from_nanodegrees(-150).units(), -2);
        assert_eq!(
            Coord::from_nanodegrees(i64::MAX).units(),
            i32::MAX,
            "must saturate instead of overflowing"
        );
    }
}
