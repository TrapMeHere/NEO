//! WebMercator world coordinates.
//!
//! Coordinate stack (important; the three layers must be distinguished):
//!
//! ```text
//! LatLon (i32, 1e-7 degrees, data layer)
//!    │  forward
//!    ▼
//! WorldPoint (f64, WebMercator meters, spatial/index layer)
//!    │  Camera::world_to_screen
//!    ▼
//! ScreenPoint (f32, physical pixels, render/UI layer)
//! ```
//!
//! WebMercator is a **conformal** projection: shapes are preserved over small areas, which is the standard choice for OSM editors/tile services.
//! It does not preserve distance, so distance measurement must use haversine (see `distance_meters`) rather than subtracting world coordinates.
//!
//! World coordinates are stored as f64: the Earth's circumference is 4.0075e7 m, and f64 resolution at that magnitude is about 1e-9 m,
//! so there is no precision problem up to zoom 25. The GPU side then converts to camera-origin-relative coordinates (f32), see `scene`.

use neo_core::LatLon;

/// WebMercator sphere radius (meters, OSM convention).
pub const EARTH_RADIUS_M: f64 = 6_378_137.0;

/// Equatorial circumference (meters).
pub const EARTH_CIRCUMFERENCE_M: f64 = 40_075_016.685_578_5;

/// Maximum latitude allowed by WebMercator (about ±85.05112878°).
pub const MAX_MERCATOR_LAT: f64 = 85.051_128_779_806_6;

/// World coordinates: WebMercator meters.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct WorldPoint {
    /// Easting (meters, 0 = 180°W).
    pub x: f64,
    /// Southing (meters, 0 = 85.05°N).
    pub y: f64,
}

impl WorldPoint {
    /// Construct.
    #[inline]
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }

    /// Lon/lat → world coordinates.
    pub fn from_latlon(p: LatLon) -> Self {
        let (lat, lon) = p.to_degrees();
        Self::from_degrees(lat, lon)
    }

    /// Decimal degrees → world coordinates.
    pub fn from_degrees(lat: f64, lon: f64) -> Self {
        let lat = lat.clamp(-MAX_MERCATOR_LAT, MAX_MERCATOR_LAT);
        // x=0 corresponds to 180°W and x=C to 180°E (OSM tile convention).
        let lon = lon.clamp(-180.0, 180.0);
        let x = (lon + 180.0) / 360.0 * EARTH_CIRCUMFERENCE_M;
        let lat_rad = lat.to_radians();
        let y_norm = (1.0 - (lat_rad.tan() + 1.0 / lat_rad.cos()).ln() / core::f64::consts::PI) / 2.0;
        Self {
            x,
            y: y_norm * EARTH_CIRCUMFERENCE_M,
        }
    }

    /// World coordinates → lon/lat.
    pub fn to_latlon(self) -> LatLon {
        let lon = self.x / EARTH_CIRCUMFERENCE_M * 360.0 - 180.0;
        let y_norm = 1.0 - 2.0 * self.y / EARTH_CIRCUMFERENCE_M;
        let lat = (core::f64::consts::PI * y_norm).sinh().atan().to_degrees();
        LatLon::from_degrees(lat, lon)
    }

    /// World-coordinate difference → screen-pixel distance (requires camera zoom).
    #[inline]
    pub fn dist(self, other: Self) -> f64 {
        ((self.x - other.x).powi(2) + (self.y - other.y).powi(2)).sqrt()
    }

    /// Add a vector.
    #[inline]
    pub fn offset(self, dx: f64, dy: f64) -> Self {
        Self::new(self.x + dx, self.y + dy)
    }
}

/// World-coordinate bounding box.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WorldBounds {
    /// Minimum point (north-west).
    pub min: WorldPoint,
    /// Maximum point (south-east).
    pub max: WorldPoint,
}

impl Default for WorldBounds {
    fn default() -> Self {
        Self::EMPTY
    }
}

impl WorldBounds {
    /// Empty box (inverted extremes, convenient for expand).
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

    /// Construct from two points (sorted automatically).
    pub fn from_points(a: WorldPoint, b: WorldPoint) -> Self {
        Self {
            min: WorldPoint::new(a.x.min(b.x), a.y.min(b.y)),
            max: WorldPoint::new(a.x.max(b.x), a.y.max(b.y)),
        }
    }

    /// Construct from a set of points.
    pub fn from_points_iter<I: IntoIterator<Item = WorldPoint>>(iter: I) -> Self {
        let mut b = Self::EMPTY;
        for p in iter {
            b.expand_point(p);
        }
        b
    }

    /// Single-point box.
    pub fn from_point(p: WorldPoint) -> Self {
        Self { min: p, max: p }
    }

    /// Whether it is empty.
    pub fn is_empty(&self) -> bool {
        self.min.x > self.max.x || self.min.y > self.max.y
    }

    /// Expand to include a point.
    pub fn expand_point(&mut self, p: WorldPoint) {
        self.min.x = self.min.x.min(p.x);
        self.min.y = self.min.y.min(p.y);
        self.max.x = self.max.x.max(p.x);
        self.max.y = self.max.y.max(p.y);
    }

    /// Expand to include a box.
    pub fn expand(&mut self, other: &Self) {
        if other.is_empty() {
            return;
        }
        self.min.x = self.min.x.min(other.min.x);
        self.min.y = self.min.y.min(other.min.y);
        self.max.x = self.max.x.max(other.max.x);
        self.max.y = self.max.y.max(other.max.y);
    }

    /// Contains a point.
    #[inline]
    pub fn contains(&self, p: WorldPoint) -> bool {
        p.x >= self.min.x && p.x <= self.max.x && p.y >= self.min.y && p.y <= self.max.y
    }

    /// Intersects another box (boundaries included).
    #[inline]
    pub fn intersects(&self, other: &Self) -> bool {
        !(other.min.x > self.max.x
            || other.max.x < self.min.x
            || other.min.y > self.max.y
            || other.max.y < self.min.y)
    }

    /// Center.
    pub fn center(&self) -> WorldPoint {
        WorldPoint::new((self.min.x + self.max.x) * 0.5, (self.min.y + self.max.y) * 0.5)
    }

    /// Width and height (meters).
    pub fn size(&self) -> (f64, f64) {
        (self.max.x - self.min.x, self.max.y - self.min.y)
    }

    /// Convert to EPSG:3857's `minx,miny,maxx,maxy` (meters, y pointing north).
    ///
    /// This repository's world x starts at 180°W and y points south; standard 3857's origin is at the equator and prime meridian, with y pointing north.
    pub fn to_epsg3857(self) -> [f64; 4] {
        let half = EARTH_CIRCUMFERENCE_M / 2.0;
        [
            self.min.x - half,
            half - self.max.y,
            self.max.x - half,
            half - self.min.y,
        ]
    }

    /// Expansion for hit testing (meters).
    pub fn expanded_by(&self, meters: f64) -> Self {
        Self {
            min: WorldPoint::new(self.min.x - meters, self.min.y - meters),
            max: WorldPoint::new(self.max.x + meters, self.max.y + meters),
        }
    }
}

/// Geodesic distance between two points (meters, haversine).
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
