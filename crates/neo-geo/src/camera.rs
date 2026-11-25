//! Camera: world coordinates ↔ screen pixels.
//!
//! Conventions:
//!
//! * `zoom` matches Web tiles: `pixels/meter = TILE_SIZE * 2^zoom / equatorial circumference`;
//! * screen y points down and world y also points down (WebMercator y faces south), so no flip is needed;
//! * the camera stores only `center` (WorldPoint) + `zoom` + `viewport` (physical pixels) and is a plain value type,
//!   so it can safely be copied/compared between the UI thread and the render thread.
//!
//! **Origin rebasing**: GPU vertices use `f32` meter coordinates relative to an origin supplied by
//! [`Camera::origin`]; after the camera pans beyond a threshold the geometry batches must be rebuilt. The threshold is
//! [`REBASE_THRESHOLD_M`] (4 km); within that range f32 precision is still far below 1 pixel.

use crate::world::{EARTH_CIRCUMFERENCE_M, WorldBounds, WorldPoint};
use neo_core::LatLon;

/// Web tile edge length (pixels).
pub const TILE_SIZE: f64 = 256.0;

/// Origin rebasing threshold (meters). Rebuild the batch once the camera center is farther than this from the batch origin.
pub const REBASE_THRESHOLD_M: f64 = 4_000.0;

/// Zoom range (consistent with common tile services).
pub const MIN_ZOOM: f64 = 0.0;
/// Maximum zoom (OSM data precision is about 1e-7 degrees; zooming further is meaningless).
pub const MAX_ZOOM: f64 = 22.0;

/// Screen pixel coordinates.
pub type ScreenPoint = [f32; 2];

/// Map camera.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Camera {
    /// View center (world coordinates).
    pub center: WorldPoint,
    /// Zoom level.
    pub zoom: f64,
    /// Viewport size (physical pixels).
    pub viewport: [u32; 2],
}

impl Default for Camera {
    fn default() -> Self {
        Self {
            center: WorldPoint::from_degrees(46.8, 8.2),
            zoom: 3.0,
            viewport: [1280, 800],
        }
    }
}

impl Camera {
    /// Construct.
    pub fn new(center: WorldPoint, zoom: f64, viewport: [u32; 2]) -> Self {
        Self {
            center,
            zoom: sanitize_zoom(zoom),
            viewport,
        }
    }

    /// Construct centered on a lon/lat.
    pub fn from_latlon(center: LatLon, zoom: f64, viewport: [u32; 2]) -> Self {
        Self::new(WorldPoint::from_latlon(center), zoom, viewport)
    }

    /// How many pixels per meter.
    #[inline]
    pub fn px_per_meter(&self) -> f64 {
        TILE_SIZE * self.zoom.exp2() / EARTH_CIRCUMFERENCE_M
    }

    /// How many meters per pixel.
    #[inline]
    pub fn meters_per_px(&self) -> f64 {
        1.0 / self.px_per_meter()
    }

    /// Viewport width/height (f64 pixels).
    #[inline]
    pub fn viewport_f64(&self) -> (f64, f64) {
        (self.viewport[0] as f64, self.viewport[1] as f64)
    }

    /// World coordinates → screen pixels.
    #[inline]
    pub fn world_to_screen(&self, p: WorldPoint) -> ScreenPoint {
        let ppm = self.px_per_meter();
        let (w, h) = self.viewport_f64();
        [
            ((p.x - self.center.x) * ppm + w * 0.5) as f32,
            ((p.y - self.center.y) * ppm + h * 0.5) as f32,
        ]
    }

    /// Screen pixels → world coordinates.
    #[inline]
    pub fn screen_to_world(&self, s: ScreenPoint) -> WorldPoint {
        let mpp = self.meters_per_px();
        let (w, h) = self.viewport_f64();
        WorldPoint::new(
            self.center.x + (s[0] as f64 - w * 0.5) * mpp,
            self.center.y + (s[1] as f64 - h * 0.5) * mpp,
        )
    }

    /// World coordinates → lon/lat.
    #[inline]
    pub fn world_to_latlon(&self, p: WorldPoint) -> LatLon {
        p.to_latlon()
    }

    /// World extent currently covered by the viewport.
    pub fn visible_bounds(&self) -> WorldBounds {
        let mpp = self.meters_per_px();
        let (w, h) = self.viewport_f64();
        let half_w = w * 0.5 * mpp;
        let half_h = h * 0.5 * mpp;
        WorldBounds {
            min: WorldPoint::new(self.center.x - half_w, self.center.y - half_h),
            max: WorldPoint::new(self.center.x + half_w, self.center.y + half_h),
        }
    }

    /// World distance (meters) corresponding to a screen-pixel distance.
    #[inline]
    pub fn px_to_meters(&self, px: f32) -> f64 {
        f64::from(px) * self.meters_per_px()
    }

    /// Screen pixels corresponding to a world distance.
    #[inline]
    pub fn meters_to_px(&self, meters: f64) -> f64 {
        meters * self.px_per_meter()
    }

    /// Update the viewport size.
    pub fn set_viewport(&mut self, viewport: [u32; 2]) {
        self.viewport = viewport;
    }

    /// Pan (screen-pixel delta; a positive value moves the content right/down and the camera center left/up).
    pub fn pan_by_screen(&mut self, dx: f32, dy: f32) {
        let mpp = self.meters_per_px();
        self.center.x -= f64::from(dx) * mpp;
        self.center.y -= f64::from(dy) * mpp;
        // Never let the camera be dragged outside the world extent (otherwise to_latlon yields illegal lon/lat,
        // and downloaded bbox/session saves write bad values).
        self.center.x = self.center.x.clamp(0.0, EARTH_CIRCUMFERENCE_M);
        self.center.y = self.center.y.clamp(0.0, EARTH_CIRCUMFERENCE_M);
    }

    /// Zoom around a screen anchor (standard wheel-zoom behavior: the map point under the anchor stays fixed).
    pub fn zoom_about(&mut self, anchor: ScreenPoint, delta_zoom: f64) {
        if !delta_zoom.is_finite() {
            return;
        }
        let before = self.screen_to_world(anchor);
        self.zoom = sanitize_zoom(self.zoom + delta_zoom);
        let after = self.screen_to_world(anchor);
        self.center.x += before.x - after.x;
        self.center.y += before.y - after.y;
        self.center.x = self.center.x.clamp(0.0, EARTH_CIRCUMFERENCE_M);
        self.center.y = self.center.y.clamp(0.0, EARTH_CIRCUMFERENCE_M);
    }

    /// Set the zoom directly (keeping the center).
    pub fn set_zoom(&mut self, zoom: f64) {
        self.zoom = sanitize_zoom(zoom);
    }

    /// Fit a world bounding box to the viewport.
    pub fn fit_bounds(&mut self, bounds: &WorldBounds, padding_px: f64) {
        if bounds.is_empty() {
            return;
        }
        let (bw, bh) = bounds.size();
        let (vw, vh) = self.viewport_f64();
        let avail_w = (vw - padding_px * 2.0).max(1.0);
        let avail_h = (vh - padding_px * 2.0).max(1.0);
        // Degenerate bbox (single point/line): keep the current zoom rather than jumping straight to maximum zoom.
        if bw <= 1e-9 && bh <= 1e-9 {
            self.center = bounds.center();
            return;
        }
        let ratio_w = (bw / EARTH_CIRCUMFERENCE_M).max(1e-12);
        let ratio_h = (bh / EARTH_CIRCUMFERENCE_M).max(1e-12);
        let zoom_x = (avail_w / TILE_SIZE / ratio_w).log2();
        let zoom_y = (avail_h / TILE_SIZE / ratio_h).log2();
        self.zoom = sanitize_zoom(zoom_x.min(zoom_y));
        self.center = bounds.center();
    }

    /// Whether the geometry batches need rebasing (compared against the given batch origin).
    #[inline]
    pub fn needs_rebase(&self, batch_origin: WorldPoint) -> bool {
        self.center.dist(batch_origin) > REBASE_THRESHOLD_M
    }

    /// Suggested origin for the current batch (takes the viewport center).
    #[inline]
    pub fn suggested_origin(&self) -> WorldPoint {
        self.center
    }
}

/// NaN/∞ would turn the uniform and all vertices into NaN, leaving the whole map blank with no log; catch it here uniformly.
fn sanitize_zoom(zoom: f64) -> f64 {
    if zoom.is_finite() {
        zoom.clamp(MIN_ZOOM, MAX_ZOOM)
    } else {
        3.0
    }
}
