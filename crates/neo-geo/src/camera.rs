//! 相机：世界坐标 ↔ 屏幕像素。
//!
//! 约定：
//!
//! * `zoom` 与 Web 瓦片一致：`像素/米 = TILE_SIZE * 2^zoom / 赤道周长`；
//! * 屏幕 y 向下，世界 y 也向下（WebMercator y 朝南），因此无需翻转；
//! * 相机只存 `center`(WorldPoint) + `zoom` + `viewport`(物理像素)，是纯值类型，
//!   可以安全地在 UI 线程与渲染线程之间复制/比较。
//!
//! **原点重基（origin rebasing）**：GPU 顶点用相对原点的 `f32` 米坐标，原点由
//! [`Camera::origin`] 给出；相机平移超过阈值后需要重建几何批次。阈值取
//! [`REBASE_THRESHOLD_M`]（4 km），在该范围内 f32 的精度仍远小于 1 像素。

use crate::world::{EARTH_CIRCUMFERENCE_M, WorldBounds, WorldPoint};
use neo_core::LatLon;

/// Web 瓦片边长（像素）。
pub const TILE_SIZE: f64 = 256.0;

/// 原点重基阈值（米）。相机中心离批次原点超过该距离就需要重建批次。
pub const REBASE_THRESHOLD_M: f64 = 4_000.0;

/// 缩放范围（与常见瓦片服务一致）。
pub const MIN_ZOOM: f64 = 0.0;
/// 缩放上限（OSM 数据精度约 1e-7 度，再放大没有意义）。
pub const MAX_ZOOM: f64 = 22.0;

/// 屏幕像素坐标。
pub type ScreenPoint = [f32; 2];

/// 地图相机。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Camera {
    /// 视图中心（世界坐标）。
    pub center: WorldPoint,
    /// 缩放级别。
    pub zoom: f64,
    /// 视口尺寸（物理像素）。
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
    /// 构造。
    pub fn new(center: WorldPoint, zoom: f64, viewport: [u32; 2]) -> Self {
        Self {
            center,
            zoom: sanitize_zoom(zoom),
            viewport,
        }
    }

    /// 以经纬度为中心构造。
    pub fn from_latlon(center: LatLon, zoom: f64, viewport: [u32; 2]) -> Self {
        Self::new(WorldPoint::from_latlon(center), zoom, viewport)
    }

    /// 每米多少像素。
    #[inline]
    pub fn px_per_meter(&self) -> f64 {
        TILE_SIZE * self.zoom.exp2() / EARTH_CIRCUMFERENCE_M
    }

    /// 每像素多少米。
    #[inline]
    pub fn meters_per_px(&self) -> f64 {
        1.0 / self.px_per_meter()
    }

    /// 视口宽高（f64 像素）。
    #[inline]
    pub fn viewport_f64(&self) -> (f64, f64) {
        (self.viewport[0] as f64, self.viewport[1] as f64)
    }

    /// 世界坐标 → 屏幕像素。
    #[inline]
    pub fn world_to_screen(&self, p: WorldPoint) -> ScreenPoint {
        let ppm = self.px_per_meter();
        let (w, h) = self.viewport_f64();
        [
            ((p.x - self.center.x) * ppm + w * 0.5) as f32,
            ((p.y - self.center.y) * ppm + h * 0.5) as f32,
        ]
    }

    /// 屏幕像素 → 世界坐标。
    #[inline]
    pub fn screen_to_world(&self, s: ScreenPoint) -> WorldPoint {
        let mpp = self.meters_per_px();
        let (w, h) = self.viewport_f64();
        WorldPoint::new(
            self.center.x + (s[0] as f64 - w * 0.5) * mpp,
            self.center.y + (s[1] as f64 - h * 0.5) * mpp,
        )
    }

    /// 世界坐标 → 经纬度。
    #[inline]
    pub fn world_to_latlon(&self, p: WorldPoint) -> LatLon {
        p.to_latlon()
    }

    /// 当前视口覆盖的世界范围。
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

    /// 屏幕像素距离对应的世界距离（米）。
    #[inline]
    pub fn px_to_meters(&self, px: f32) -> f64 {
        f64::from(px) * self.meters_per_px()
    }

    /// 世界距离对应的屏幕像素。
    #[inline]
    pub fn meters_to_px(&self, meters: f64) -> f64 {
        meters * self.px_per_meter()
    }

    /// 更新视口尺寸。
    pub fn set_viewport(&mut self, viewport: [u32; 2]) {
        self.viewport = viewport;
    }

    /// 平移（屏幕像素增量，正值 = 内容右/下移，相机中心左/上移）。
    pub fn pan_by_screen(&mut self, dx: f32, dy: f32) {
        let mpp = self.meters_per_px();
        self.center.x -= f64::from(dx) * mpp;
        self.center.y -= f64::from(dy) * mpp;
        // 不允许把相机拖出世界范围（否则 to_latlon 会给出非法经纬度，
        // 下载 bbox/会话保存都会写出坏值）。
        self.center.x = self.center.x.clamp(0.0, EARTH_CIRCUMFERENCE_M);
        self.center.y = self.center.y.clamp(0.0, EARTH_CIRCUMFERENCE_M);
    }

    /// 以屏幕锚点缩放（滚轮缩放的标准行为：锚点下的地图点保持不动）。
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

    /// 直接设置缩放（保持中心）。
    pub fn set_zoom(&mut self, zoom: f64) {
        self.zoom = sanitize_zoom(zoom);
    }

    /// 让某个世界包围盒适配视口。
    pub fn fit_bounds(&mut self, bounds: &WorldBounds, padding_px: f64) {
        if bounds.is_empty() {
            return;
        }
        let (bw, bh) = bounds.size();
        let (vw, vh) = self.viewport_f64();
        let avail_w = (vw - padding_px * 2.0).max(1.0);
        let avail_h = (vh - padding_px * 2.0).max(1.0);
        // 退化 bbox（单点/单线）：保持当前缩放，不要直接跳到最大缩放。
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

    /// 是否需要重基几何批次（与给定批次原点比较）。
    #[inline]
    pub fn needs_rebase(&self, batch_origin: WorldPoint) -> bool {
        self.center.dist(batch_origin) > REBASE_THRESHOLD_M
    }

    /// 当前批次原点建议值（取视口中心）。
    #[inline]
    pub fn suggested_origin(&self) -> WorldPoint {
        self.center
    }
}

/// NaN/∞ 会让 uniform 与顶点全变 NaN，整张地图空白且无日志；统一在这里兜底。
fn sanitize_zoom(zoom: f64) -> f64 {
    if zoom.is_finite() {
        zoom.clamp(MIN_ZOOM, MAX_ZOOM)
    } else {
        3.0
    }
}
