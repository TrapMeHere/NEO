//! NeoApp::imagery methods. Same behavior as before the app.rs split.

use eframe::egui;
use neo_geo::WorldPoint;
use neo_i18n::{t, tf};
use neo_style::imagery::HostTable;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock, mpsc::TryRecvError};

use super::{NeoApp, TileKey};

impl NeoApp {
    pub(super) fn show_osm_layer(&mut self) {
        self.osm_listed = true;
        self.osm_visible = true;
    }

    pub(super) fn show_imagery_preset(&mut self, id: &str) {
        let Some(preset) = neo_style::imagery_presets().iter().find(|preset| preset.id == id) else {
            return;
        };
        self.imagery.preset_id = preset.id.clone();
        self.imagery.url = preset.url.clone();
        self.imagery.wms = preset.kind == neo_style::ImageryKind::Wms;
        self.imagery.wms_layers = preset.layers.clone();
        self.imagery.attribution = preset.attribution.clone();
        self.imagery.wms_crs = preset.crs.clone();
        self.imagery.wms_lat_first = preset.bbox_lat_first;
        self.imagery.enabled = true;
        self.imagery_listed = true;
        self.drop_imagery_source(super::SOURCE_CUSTOM);
        self.imagery.status = t("imagery.loading").replace("{n}", "0");
        self.persist_state();
    }

    pub(super) fn drop_imagery_source(&mut self, source: u8) {
        self.imagery_tiles.retain(|key, _| key.source != source);
        self.imagery_pending.retain(|key| key.source != source);
        if source == super::SOURCE_CUSTOM {
            self.wms_texture = None;
            self.wms_key.clear();
            self.wms_bbox = None;
            self.wms_pending = false;
        }
    }

    pub(super) fn imagery_valid(&self) -> bool {
        let url = self.imagery.url.trim();
        let secure = url.starts_with("https://") || url.starts_with("http://");
        if self.imagery.wms {
            return secure && (url.contains("{bbox}") || !self.imagery.wms_layers.trim().is_empty());
        }
        let xyz = (url.contains("{z}") || url.contains("{zoom}"))
            && url.contains("{x}")
            && (url.contains("{y}") || url.contains("{-y}"));
        secure && (xyz || url.contains("{quadkey}") || url.contains("{u}"))
    }

    pub(super) fn imagery_zoom(&self) -> u32 {
        let preset_max = neo_style::imagery_presets()
            .iter()
            .find(|preset| preset.id == self.imagery.preset_id)
            .map(|preset| u32::from(preset.max_zoom));
        let zoom_key = (self.imagery.preset_id.clone(), self.camera.zoom.floor() as i32);
        if let Ok(cache) = self.imagery_zoom_cache.try_borrow()
            && let Some((key, zoom)) = cache.as_ref()
            && key == &zoom_key
        {
            return *zoom;
        }
        let zoom = if let Some(sidecar) = &self.imagery_sidecar {
            match sidecar.call(neo_ipc::ImageryRequest::CapZoom {
                preset_id: self.imagery.preset_id.clone(),
                url: self.imagery.url.clone(),
                camera_zoom: self.camera.zoom,
                preset_max_zoom: preset_max,
            }) {
                Ok(neo_ipc::ImageryResponse::Zoom { zoom }) => zoom,
                _ => local_imagery_zoom(
                    preset_max,
                    &self.imagery.preset_id,
                    &self.imagery.url,
                    self.camera.zoom,
                ),
            }
        } else {
            local_imagery_zoom(
                preset_max,
                &self.imagery.preset_id,
                &self.imagery.url,
                self.camera.zoom,
            )
        };
        if let Ok(mut cache) = self.imagery_zoom_cache.try_borrow_mut() {
            *cache = Some((zoom_key, zoom));
        }
        zoom
    }

    fn imagery_request_bbox(&self, bounds: neo_geo::WorldBounds) -> [f64; 4] {
        let crs = self.imagery.wms_crs.as_str();
        if crs.contains("4326") || crs.contains("CRS:84") {
            let (north, west) = neo_geo::WorldPoint::new(bounds.min.x, bounds.min.y)
                .to_latlon()
                .to_degrees();
            let (south, east) = neo_geo::WorldPoint::new(bounds.max.x, bounds.max.y)
                .to_latlon()
                .to_degrees();
            if self.imagery.wms_lat_first {
                [south, west, north, east]
            } else {
                [west, south, east, north]
            }
        } else {
            bounds.to_epsg3857()
        }
    }

    pub(super) fn ensure_imagery_tiles(&mut self, ctx: &egui::Context) {
        self.imagery_clock = self.imagery_clock.wrapping_add(1);
        if self.osm_visible {
            let z = self.camera.zoom.floor().clamp(0.0, 19.0) as u32;
            self.ensure_tms(ctx, super::SOURCE_OSM, neo_style::imagery::OSM_IMAGERY_URL, "", z);
        }
        if self.imagery.enabled && self.imagery_valid() {
            if self.imagery.wms {
                self.ensure_wms(ctx);
            } else {
                let z = self.imagery_zoom();
                let url = self.imagery.url.trim().to_string();
                let api_key = self.imagery.api_key.trim().to_string();
                self.ensure_tms(ctx, super::SOURCE_CUSTOM, &url, &api_key, z);
            }
        }
    }

    fn ensure_tms(&mut self, ctx: &egui::Context, source: u8, template: &str, api_key: &str, z: u32) {
        let bounds = self.camera.visible_bounds();
        let window = tile_window(bounds, z);
        let mut need_cover = false;
        for x in window.x0..=window.x1 {
            for y in window.y0..=window.y1 {
                let key = TileKey { source, z, x, y };
                if self.imagery_tiles.contains_key(&key) {
                    self.touch_tile(key);
                } else if let Some(parent) = finest_ancestor(
                    |candidate| self.imagery_tiles.contains_key(&candidate),
                    source,
                    z,
                    x,
                    y,
                ) {
                    self.touch_tile(parent);
                } else {
                    need_cover = true;
                }
            }
        }
        let mut launched = 0usize;
        if need_cover && let Some(coarse) = cover_zoom(bounds, z) {
            let cover = tile_window(bounds, coarse);
            'cover: for x in cover.x0..=cover.x1 {
                for y in cover.y0..=cover.y1 {
                    if !self.queue_tile(
                        template,
                        api_key,
                        TileKey {
                            source,
                            z: coarse,
                            x,
                            y,
                        },
                        &mut launched,
                    ) {
                        break 'cover;
                    }
                }
            }
        }
        'exact: for x in window.x0..=window.x1 {
            for y in window.y0..=window.y1 {
                if !self.queue_tile(template, api_key, TileKey { source, z, x, y }, &mut launched) {
                    break 'exact;
                }
            }
        }
        if !self.imagery_pending.is_empty() {
            self.imagery.status = tf(
                "imagery.loading",
                &[("n", &self.imagery_pending.len().to_string())],
            );
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
        }
    }

    /// Starts downloading one tile. Returns false when the budget is exhausted; the caller should stop queueing.
    fn queue_tile(&mut self, template: &str, api_key: &str, key: TileKey, launched: &mut usize) -> bool {
        if *launched >= 48 {
            return false;
        }
        if self.imagery_tiles.contains_key(&key) || self.imagery_pending.contains(&key) {
            return true;
        }
        self.imagery_pending.insert(key);
        *launched += 1;
        let candidates = neo_style::imagery::tile_url_candidates(template, key.z, key.x, key.y, api_key);
        let urls = host_table().order_urls(candidates, unix_ms(), next_host_rotate());
        let tx = self.imagery_tx.clone();
        let sidecar = self.imagery_sidecar.clone();
        std::thread::spawn(move || {
            let result = if let Some(sidecar) = sidecar {
                fetch_via_sidecar(&sidecar, &urls)
            } else {
                Err("neo-imagery unavailable".into())
            };
            let _ = tx.send((key, result));
        });
        true
    }

    fn touch_tile(&mut self, key: TileKey) {
        let clock = self.imagery_clock;
        if let Some(tile) = self.imagery_tiles.get_mut(&key) {
            tile.used = clock;
        }
    }

    pub(super) fn ensure_wms(&mut self, ctx: &egui::Context) {
        if self.wms_pending || self.imagery.wms_layers.trim().is_empty() {
            return;
        }
        let bounds = self.camera.visible_bounds();
        let bbox = [bounds.min.x, bounds.min.y, bounds.max.x, bounds.max.y];
        let key = format!("{:.1},{:.1},{:.1},{:.1}", bbox[0], bbox[1], bbox[2], bbox[3]);
        if key == self.wms_key && self.wms_texture.is_some() {
            return;
        }
        let width = self.camera.viewport[0].clamp(256, 1600);
        let height = self.camera.viewport[1].clamp(256, 1200);
        let request_bbox = self.imagery_request_bbox(bounds);
        let url = if self.imagery.url.contains("{bbox}") {
            neo_style::imagery::expand_wms_template(
                self.imagery.url.trim(),
                request_bbox,
                width,
                height,
                &self.imagery.wms_crs,
            )
        } else {
            neo_style::imagery::wms_url_3857(
                self.imagery.url.trim(),
                self.imagery.wms_layers.trim(),
                request_bbox,
                width,
                height,
            )
        };
        let tx = self.wms_tx.clone();
        self.wms_key = key.clone();
        self.wms_pending = true;
        self.imagery.status = t("imagery.loading").replace("{n}", "1");
        let sidecar = self.imagery_sidecar.clone();
        std::thread::spawn(move || {
            let result = if let Some(sidecar) = sidecar {
                match sidecar.call(neo_ipc::ImageryRequest::Fetch { url }) {
                    Ok(neo_ipc::ImageryResponse::Fetched { bytes }) => Ok(bytes),
                    Ok(neo_ipc::ImageryResponse::Error { message }) => Err(message),
                    Ok(other) => Err(format!("unexpected imagery reply: {other:?}")),
                    Err(err) => Err(err.to_string()),
                }
            } else {
                Err("neo-imagery unavailable".into())
            };
            let _ = tx.send((key, bbox, result));
        });
        ctx.request_repaint_after(std::time::Duration::from_millis(300));
    }

    pub(super) fn poll_wms(&mut self, ctx: &egui::Context) {
        loop {
            match self.wms_rx.try_recv() {
                Ok((key, bbox, result)) => {
                    self.wms_pending = false;
                    if key != self.wms_key {
                        continue;
                    }
                    match result {
                        Ok(bytes) => match image::load_from_memory(&bytes) {
                            Ok(image) => {
                                let rgba = image.to_rgba8();
                                let size = [rgba.width() as usize, rgba.height() as usize];
                                let color = egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw());
                                self.wms_texture =
                                    Some(ctx.load_texture("neo-wms", color, egui::TextureOptions::LINEAR));
                                self.wms_bbox = Some(bbox);
                                self.imagery.status.clear();
                            }
                            Err(err) => {
                                self.imagery.status = tf("imagery.error", &[("err", &err.to_string())]);
                            }
                        },
                        Err(err) => {
                            self.imagery.status = tf("imagery.error", &[("err", &err)]);
                        }
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => break,
            }
        }
        if self.wms_pending {
            ctx.request_repaint_after(std::time::Duration::from_millis(300));
        }
    }

    pub(super) fn poll_imagery(&mut self, ctx: &egui::Context) {
        self.poll_wms(ctx);
        loop {
            let received = self.imagery_rx.try_recv();
            let (key, result) = match received {
                Ok(item) => item,
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => break,
            };
            self.imagery_pending.remove(&key);
            match result {
                Ok(bytes) => match image::load_from_memory(&bytes) {
                    Ok(image) => {
                        let rgba = image.to_rgba8();
                        let size = [rgba.width() as usize, rgba.height() as usize];
                        let color = egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw());
                        let texture = ctx.load_texture(
                            format!("neo-tile-{}-{}-{}-{}", key.source, key.z, key.x, key.y),
                            color,
                            egui::TextureOptions::LINEAR,
                        );
                        let source = key.source;
                        let zoom = self.view_zoom(source);
                        self.imagery_tiles.insert(
                            key,
                            super::CachedTile {
                                texture,
                                used: self.imagery_clock,
                            },
                        );
                        self.evict_source(source, zoom);
                    }
                    Err(err) => {
                        self.imagery.status = tf("imagery.error", &[("err", &err.to_string())]);
                    }
                },
                Err(err) => {
                    self.imagery.status = tf("imagery.error", &[("err", &err)]);
                }
            }
        }
        self.imagery.cached = self.imagery_tiles.len();
        if !self.imagery_pending.is_empty() {
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
        }
    }

    pub(super) fn draw_imagery(&self, painter: &egui::Painter, rect: egui::Rect, ppp: f32) {
        if self.osm_visible {
            let z = self.camera.zoom.floor().clamp(0.0, 19.0) as u32;
            self.draw_tms(
                painter,
                rect,
                ppp,
                TmsPaint {
                    source: super::SOURCE_OSM,
                    z,
                    opacity: 1.0,
                    offset: egui::Vec2::ZERO,
                },
            );
        }
        if self.imagery.enabled && self.imagery_valid() && !self.imagery.wms {
            let z = self.imagery_zoom();
            self.draw_tms(
                painter,
                rect,
                ppp,
                TmsPaint {
                    source: super::SOURCE_CUSTOM,
                    z,
                    opacity: self.imagery.opacity,
                    offset: egui::vec2(self.imagery.offset_x, self.imagery.offset_y),
                },
            );
        }
        if self.imagery.enabled
            && self.imagery_valid()
            && self.imagery.wms
            && let (Some(texture), Some(bbox)) = (&self.wms_texture, self.wms_bbox)
        {
            let a = self.camera.world_to_screen(WorldPoint::new(bbox[0], bbox[1]));
            let b = self.camera.world_to_screen(WorldPoint::new(bbox[2], bbox[3]));
            let screen = egui::Rect::from_min_max(
                egui::pos2(rect.min.x + a[0] / ppp, rect.min.y + a[1] / ppp),
                egui::pos2(rect.min.x + b[0] / ppp, rect.min.y + b[1] / ppp),
            );
            let alpha = (self.imagery.opacity.clamp(0.0, 1.0) * 255.0) as u8;
            painter.image(
                texture.id(),
                screen,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                egui::Color32::from_white_alpha(alpha),
            );
        }
        self.draw_imagery_credit(painter, rect);
    }

    fn view_zoom(&self, source: u8) -> u32 {
        if source == super::SOURCE_OSM {
            self.camera.zoom.floor().clamp(0.0, 19.0) as u32
        } else {
            self.imagery_zoom()
        }
    }

    /// When one source exceeds its limit, keep the current viewport and the ancestor tiles currently underpinning it; drop the rest by distance from the viewport and from the current level.
    fn evict_source(&mut self, source: u8, z: u32) {
        const CAP: usize = 160;
        let count = self
            .imagery_tiles
            .keys()
            .filter(|tile| tile.source == source)
            .count();
        if count <= CAP {
            return;
        }
        let bounds = self.camera.visible_bounds();
        let window = tile_window(bounds, z);
        let mut protect = std::collections::HashSet::new();
        for x in window.x0..=window.x1 {
            for y in window.y0..=window.y1 {
                let key = TileKey { source, z, x, y };
                if self.imagery_tiles.contains_key(&key) {
                    protect.insert(key);
                } else if let Some(parent) = finest_ancestor(
                    |candidate| self.imagery_tiles.contains_key(&candidate),
                    source,
                    z,
                    x,
                    y,
                ) {
                    protect.insert(parent);
                }
            }
        }
        let mut victims = Vec::new();
        for (key, tile) in &self.imagery_tiles {
            if key.source == source && !protect.contains(key) {
                let outside = !tile_intersects(*key, bounds);
                victims.push((*key, tile.used, outside, key.z.abs_diff(z)));
            }
        }
        victims.sort_by(|left, right| {
            // Drop off-viewport tiles first, then those farther from the current level, then the least recently used.
            u8::from(!left.2)
                .cmp(&u8::from(!right.2))
                .then(right.3.cmp(&left.3))
                .then(left.1.cmp(&right.1))
        });
        let mut extra = count - CAP;
        for (key, _, _, _) in victims {
            if extra == 0 {
                break;
            }
            self.imagery_tiles.remove(&key);
            extra -= 1;
        }
    }

    fn draw_tms(&self, painter: &egui::Painter, rect: egui::Rect, ppp: f32, paint: TmsPaint) {
        let window = tile_window(self.camera.visible_bounds(), paint.z);
        let mut exact = Vec::new();
        let mut under = Vec::new();
        for x in window.x0..=window.x1 {
            for y in window.y0..=window.y1 {
                let key = TileKey {
                    source: paint.source,
                    z: paint.z,
                    x,
                    y,
                };
                if self.imagery_tiles.contains_key(&key) {
                    exact.push(key);
                } else if let Some(parent) = finest_ancestor(
                    |candidate| self.imagery_tiles.contains_key(&candidate),
                    paint.source,
                    paint.z,
                    x,
                    y,
                ) {
                    // Fill only this cell. Laying down the whole ancestor would stack alpha twice over landed tiles, making one side of the seam brighter.
                    under.push((parent, key));
                }
            }
        }
        under.sort_by_key(|(parent, _)| parent.z);
        for (parent, hole) in under {
            self.paint_tile(
                painter,
                rect,
                ppp,
                TilePatch {
                    paint: &paint,
                    texture: parent,
                    dest: hole,
                    uv: tile_uv(hole, parent),
                },
            );
        }
        let full = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
        for key in exact {
            self.paint_tile(
                painter,
                rect,
                ppp,
                TilePatch {
                    paint: &paint,
                    texture: key,
                    dest: key,
                    uv: full,
                },
            );
        }
    }

    fn paint_tile(&self, painter: &egui::Painter, rect: egui::Rect, ppp: f32, patch: TilePatch<'_>) {
        let Some(tile) = self.imagery_tiles.get(&patch.texture) else {
            return;
        };
        let span = tile_span(patch.dest.z);
        let x0 = f64::from(patch.dest.x) * span;
        let y0 = f64::from(patch.dest.y) * span;
        let a = self.camera.world_to_screen(WorldPoint::new(x0, y0));
        let b = self.camera.world_to_screen(WorldPoint::new(x0 + span, y0 + span));
        let screen = egui::Rect::from_min_max(
            egui::pos2(rect.min.x + a[0] / ppp, rect.min.y + a[1] / ppp),
            egui::pos2(rect.min.x + b[0] / ppp, rect.min.y + b[1] / ppp),
        )
        .translate(patch.paint.offset);
        if !rect.intersects(screen) {
            return;
        }
        let alpha = (patch.paint.opacity.clamp(0.0, 1.0) * 255.0) as u8;
        painter.image(
            tile.texture.id(),
            screen,
            patch.uv,
            egui::Color32::from_white_alpha(alpha),
        );
    }

    fn draw_imagery_credit(&self, painter: &egui::Painter, rect: egui::Rect) {
        let mut credit = String::new();
        if self.osm_visible {
            credit.push_str(neo_style::imagery::OSM_IMAGERY_ATTRIBUTION);
        }
        if self.imagery.enabled && !self.imagery.attribution.is_empty() {
            if !credit.is_empty() {
                credit.push_str(" · ");
            }
            credit.push_str(&self.imagery.attribution);
        }
        if credit.is_empty() {
            return;
        }
        let pos = rect.left_bottom() + egui::vec2(8.0, -6.0);
        let font = egui::FontId::proportional(11.0);
        for offset in [
            egui::vec2(-1.0, 0.0),
            egui::vec2(1.0, 0.0),
            egui::vec2(0.0, -1.0),
            egui::vec2(0.0, 1.0),
        ] {
            painter.text(
                pos + offset,
                egui::Align2::LEFT_BOTTOM,
                &credit,
                font.clone(),
                egui::Color32::BLACK,
            );
        }
        painter.text(pos, egui::Align2::LEFT_BOTTOM, credit, font, egui::Color32::WHITE);
    }
}

struct TmsPaint {
    source: u8,
    z: u32,
    opacity: f32,
    offset: egui::Vec2,
}

struct TilePatch<'a> {
    paint: &'a TmsPaint,
    texture: TileKey,
    dest: TileKey,
    uv: egui::Rect,
}

fn host_table() -> std::sync::MutexGuard<'static, HostTable> {
    static TABLE: OnceLock<Mutex<HostTable>> = OnceLock::new();
    TABLE
        .get_or_init(|| Mutex::new(HostTable::default()))
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
}

fn next_host_rotate() -> usize {
    static ROTATE: AtomicUsize = AtomicUsize::new(0);
    ROTATE.fetch_add(1, Ordering::Relaxed)
}

fn unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

fn note_host(url: &str, ok: bool, elapsed_ms: u64) {
    host_table().record_url(url, ok, elapsed_ms, unix_ms());
}

/// Try hosts in the current order, at most 3. Successful ones keep their latency; failed ones enter cooldown, and the next tile switches to another subdomain.
fn local_imagery_zoom(preset_max: Option<u32>, preset_id: &str, url: &str, camera_zoom: f64) -> u32 {
    let mut cap = preset_max.unwrap_or(22);
    if preset_id == "tianditu" || url.contains("tianditu.gov.cn") {
        cap = cap.min(18);
    }
    camera_zoom.floor().clamp(0.0, f64::from(cap)) as u32
}

fn fetch_via_sidecar(
    sidecar: &neo_ipc::ImagerySession,
    urls: &[String],
) -> std::result::Result<Vec<u8>, String> {
    let mut last = String::from("no imagery host");
    if urls.is_empty() {
        return Err(last);
    }
    for url in urls.iter().take(3) {
        let started = std::time::Instant::now();
        match sidecar.call(neo_ipc::ImageryRequest::Fetch { url: (*url).clone() }) {
            Ok(neo_ipc::ImageryResponse::Fetched { bytes }) => {
                note_host(url, true, started.elapsed().as_millis() as u64);
                return Ok(bytes);
            }
            Ok(neo_ipc::ImageryResponse::Error { message }) => {
                note_host(url, false, started.elapsed().as_millis() as u64);
                last = message;
            }
            Ok(other) => {
                note_host(url, false, started.elapsed().as_millis() as u64);
                last = format!("unexpected imagery reply: {other:?}");
            }
            Err(err) => {
                note_host(url, false, started.elapsed().as_millis() as u64);
                last = err.to_string();
            }
        }
    }
    Err(last)
}

fn tile_span(z: u32) -> f64 {
    neo_geo::EARTH_CIRCUMFERENCE_M / f64::from(1u32 << z.min(31))
}

struct TileWindow {
    x0: u32,
    x1: u32,
    y0: u32,
    y1: u32,
}

fn tile_window(bounds: neo_geo::WorldBounds, z: u32) -> TileWindow {
    let count = 1u32 << z.min(31);
    let span = tile_span(z);
    let clamp = |value: f64| value.floor().clamp(0.0, f64::from(count.saturating_sub(1))) as u32;
    TileWindow {
        x0: clamp(bounds.min.x / span),
        x1: clamp(bounds.max.x / span),
        y0: clamp(bounds.min.y / span),
        y1: clamp(bounds.max.y / span),
    }
}

/// The finest level whose viewport can be covered with at most 4 tiles. The current level itself doesn't count.
fn cover_zoom(bounds: neo_geo::WorldBounds, z: u32) -> Option<u32> {
    if z == 0 {
        return None;
    }
    for cz in (0..z).rev() {
        let window = tile_window(bounds, cz);
        let width = window.x1 - window.x0 + 1;
        let height = window.y1 - window.y0 + 1;
        if width.saturating_mul(height) <= 4 {
            return Some(cz);
        }
    }
    None
}

fn ancestor_key(source: u8, z: u32, x: u32, y: u32, dz: u32) -> TileKey {
    TileKey {
        source,
        z: z - dz,
        x: x >> dz,
        y: y >> dz,
    }
}

/// Sample range of a child tile within the ancestor texture. A whole ancestor maps to `(0,0)–(1,1)`.
fn tile_uv(dest: TileKey, source: TileKey) -> egui::Rect {
    if dest.z <= source.z {
        return egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
    }
    let n = 1u32 << (dest.z - source.z).min(31);
    let size = 1.0 / (n as f32);
    let ox = (dest.x & (n - 1)) as f32 * size;
    let oy = (dest.y & (n - 1)) as f32 * size;
    egui::Rect::from_min_max(egui::pos2(ox, oy), egui::pos2(ox + size, oy + size))
}

fn finest_ancestor(has: impl Fn(TileKey) -> bool, source: u8, z: u32, x: u32, y: u32) -> Option<TileKey> {
    let mut dz = 1;
    while dz <= z {
        let key = ancestor_key(source, z, x, y, dz);
        if has(key) {
            return Some(key);
        }
        dz += 1;
    }
    None
}

fn tile_intersects(key: TileKey, bounds: neo_geo::WorldBounds) -> bool {
    let span = tile_span(key.z);
    let tile = neo_geo::WorldBounds {
        min: WorldPoint::new(f64::from(key.x) * span, f64::from(key.y) * span),
        max: WorldPoint::new(f64::from(key.x + 1) * span, f64::from(key.y + 1) * span),
    };
    tile.intersects(&bounds)
}

#[cfg(test)]
mod tests {
    use super::super::TileKey;
    use super::{ancestor_key, cover_zoom, finest_ancestor, tile_span, tile_uv};
    use neo_geo::{WorldBounds, WorldPoint};

    #[test]
    fn ancestor_shifts_xy_toward_zero() {
        let key = ancestor_key(0, 5, 0b10110, 0b01011, 2);
        assert_eq!((key.z, key.x, key.y), (3, 0b101, 0b010));
    }

    #[test]
    fn finest_ancestor_skips_the_missing_parent() {
        let parent = ancestor_key(1, 6, 16, 8, 2);
        let found = finest_ancestor(|key| key == parent, 1, 6, 16, 8);
        assert_eq!(found, Some(parent));
    }

    #[test]
    fn cover_zoom_is_the_finest_level_of_at_most_four_tiles() {
        let z = 10;
        let span = tile_span(z);
        let bounds = WorldBounds {
            min: WorldPoint::new(span * 0.1, span * 0.1),
            max: WorldPoint::new(span * 7.9, span * 3.9),
        };
        assert_eq!(cover_zoom(bounds, z), Some(8));
    }

    #[test]
    fn ancestor_uv_is_the_child_quadrant() {
        let source = ancestor_key(0, 5, 0b10110, 0b01011, 2);
        let dest = TileKey {
            source: 0,
            z: 5,
            x: 0b10110,
            y: 0b01011,
        };
        let uv = tile_uv(dest, source);
        assert!((uv.min.x - 0.5).abs() < 1e-5);
        assert!((uv.min.y - 0.75).abs() < 1e-5);
        assert!((uv.width() - 0.25).abs() < 1e-5);
        assert!((uv.height() - 0.25).abs() < 1e-5);
    }

    #[test]
    fn jpeg_tiles_decode() {
        let image = image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(2, 2, image::Rgb([9, 8, 7])));
        let mut bytes = std::io::Cursor::new(Vec::new());
        image.write_to(&mut bytes, image::ImageFormat::Jpeg).unwrap();
        let decoded = image::load_from_memory(bytes.get_ref()).unwrap();
        assert_eq!(decoded.width(), 2);
        assert_eq!(decoded.height(), 2);
    }
}
