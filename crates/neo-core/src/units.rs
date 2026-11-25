//! Fixed-point coordinates and latitude/longitude.
//!
//! OSM's authoritative precision is 1e-7 degree (the API accepts only 7 decimals; PBF stores integer
//! nanodegrees internally, but storage/upload round back to 1e-7). Storing 1e-7 degrees in `i32`:
//!
//! * longitude ±180° → ±1_800_000_000, within `i32` (±2_147_483_647);
//! * latitude ±90°  → ±900_000_000;
//! * 4 bytes/coordinate: for 10M nodes, coordinates alone save 80 MB compared with `f64`.
//!
//! Cost: 1e-9-precision PBF values are normalized on import by "rounding to 1e-7", which is the same
//! semantics as the OSM API; a future need for lossless 1e-9 round-trips would require an extra `i64`
//! high-precision variant (see docs/03).

use core::fmt;

/// 1 degree = 10_000_000 coordinate units (1e-7 degree).
pub const COORD_UNITS_PER_DEGREE: i64 = 10_000_000;

/// Maximum longitude in 1e-7 degree units.
pub const MAX_LON_UNITS: i32 = 1_800_000_000;
/// Maximum latitude in 1e-7 degree units.
pub const MAX_LAT_UNITS: i32 = 900_000_000;

/// A single coordinate component in fixed-point 1e-7 degree.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Coord(pub i32);

impl Coord {
    /// Zero.
    pub const ZERO: Self = Self(0);

    /// Construct from a raw 1e-7 degree integer.
    #[inline]
    pub const fn from_units(units: i32) -> Self {
        Self(units)
    }

    /// Raw 1e-7 degree integer.
    #[inline]
    pub const fn units(self) -> i32 {
        self.0
    }

    /// Construct from decimal degrees (rounds to 1e-7 and clamps into the valid 1e-7 degree range).
    ///
    /// Non-finite values (NaN/±inf) become 0: callers handling untrusted input should validate explicitly
    /// with [`Coord::try_from_degrees`] first, to avoid silently turning bad data into "0,0 in the Gulf of Guinea".
    #[inline]
    pub fn from_degrees(deg: f64) -> Self {
        let scaled = if deg.is_finite() {
            (deg * COORD_UNITS_PER_DEGREE as f64).round()
        } else {
            0.0
        };
        Self(scaled.clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i32)
    }

    /// Construct from decimal degrees; returns `None` for non-finite or out-of-range 1e-7 degree values.
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

    /// Convert to decimal degrees.
    #[inline]
    pub fn to_degrees(self) -> f64 {
        f64::from(self.0) / COORD_UNITS_PER_DEGREE as f64
    }

    /// Construct from OSM PBF nanodegree (1e-9 degree) values, rounding to 1e-7.
    ///
    /// Uses an `i128` intermediate to avoid `nano + 50` overflowing near `i64::MAX`; values outside the
    /// representable `i32` range are clamped (callers should also validate the lat/lon range).
    #[inline]
    pub fn from_nanodegrees(nano: i64) -> Self {
        // 100 nanodegree = 1e-7 degree; round with integers to avoid floating-point error on large values.
        let n = i128::from(nano);
        let rounded = if n >= 0 { (n + 50) / 100 } else { (n - 50) / 100 };
        Self(rounded.clamp(i128::from(i32::MIN), i128::from(i32::MAX)) as i32)
    }

    /// Convert to nanodegrees (1e-9 degree).
    #[inline]
    pub fn to_nanodegrees(self) -> i64 {
        i64::from(self.0) * 100
    }

    /// Whether this is within the valid latitude/longitude range (longitude).
    #[inline]
    pub fn is_valid_lon(self) -> bool {
        self.0.unsigned_abs() <= MAX_LON_UNITS as u32
    }

    /// Whether this is within the valid latitude/longitude range (latitude).
    #[inline]
    pub fn is_valid_lat(self) -> bool {
        self.0.unsigned_abs() <= MAX_LAT_UNITS as u32
    }

    /// Approximate meters for the difference between two coordinate units (longitude direction, at the equator).
    #[inline]
    pub fn approx_meters(self, other: Coord) -> f64 {
        // All f64: subtracting i32 values across hemispheres (3.6e9) would overflow.
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

/// Latitude/longitude point.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Default, Debug)]
pub struct LatLon {
    /// Latitude.
    pub lat: Coord,
    /// Longitude.
    pub lon: Coord,
}

impl LatLon {
    /// Construct.
    #[inline]
    pub const fn new(lat: Coord, lon: Coord) -> Self {
        Self { lat, lon }
    }

    /// Construct from decimal degrees.
    #[inline]
    pub fn from_degrees(lat: f64, lon: f64) -> Self {
        Self {
            lat: Coord::from_degrees(lat),
            lon: Coord::from_degrees(lon),
        }
    }

    /// Construct from decimal degrees; returns `None` if either component is invalid (non-finite/out of range).
    #[inline]
    pub fn try_from_degrees(lat: f64, lon: f64) -> Option<Self> {
        Some(Self {
            lat: Coord::try_from_degrees(lat)?,
            lon: Coord::try_from_degrees(lon)?,
        })
        .filter(|p| p.is_valid())
    }
    /// Convert to (lat, lon) decimal degrees.
    #[inline]
    pub fn to_degrees(self) -> (f64, f64) {
        (self.lat.to_degrees(), self.lon.to_degrees())
    }

    /// Validity check (both latitude and longitude in range).
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
