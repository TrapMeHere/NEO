//! Imagery layer: WMS / TMS URLs and the offset database.
//!
//! No third-party imagery catalog is bundled. The caller supplies templates; this module only assembles URLs and looks up offsets.

/// Imagery kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageryKind {
    /// `{z}/{x}/{y}` tiles.
    Tms,
    /// GetMap.
    Wms,
}

/// One imagery source.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImagerySource {
    /// Stable id.
    pub id: String,
    /// Display name.
    pub name: String,
    /// Kind.
    pub kind: ImageryKind,
    /// TMS template or WMS endpoint.
    pub url: String,
    /// Attribution, which must be shown to the user.
    pub attribution: String,
    /// Minimum zoom.
    pub min_zoom: u8,
    /// Maximum zoom.
    pub max_zoom: u8,
}

/// Substitute the concrete tile into `{z}` `{x}` `{y}` `{-y}` in a TMS template.
pub fn tms_url(template: &str, z: u32, x: u32, y: u32) -> String {
    tile_url(template, z, x, y, "")
}

/// Expand the URL of one tile.
///
/// Besides `{z}`/`{zoom}`, `{x}`, `{y}`, `{-y}`, also supports those used by iD / JOSM:
/// `{quadkey}` (Bing), `{switch:a,b}` (here one is chosen by hashing the tile), `{apikey}`.
/// When subdomains must be picked by speed and reachability, use [`tile_url_candidates`] and then hand them to [`HostTable`].
pub fn tile_url(template: &str, z: u32, x: u32, y: u32, api_key: &str) -> String {
    expand_tile(&expand_switch(template, x, y), z, x, y, api_key)
}

/// One URL per subdomain of `{switch:a,b,c}`, in the order they appear in the template.
///
/// Returns only one URL when there is no `{switch:…}`. The caller then uses [`HostTable`] to put fast, reachable ones first.
pub fn tile_url_candidates(template: &str, z: u32, x: u32, y: u32, api_key: &str) -> Vec<String> {
    let options = switch_options(template);
    if options.is_empty() {
        return vec![tile_url(template, z, x, y, api_key)];
    }
    options
        .into_iter()
        .map(|option| expand_tile(&substitute_first_switch(template, &option), z, x, y, api_key))
        .collect()
}

fn expand_tile(template: &str, z: u32, x: u32, y: u32, api_key: &str) -> String {
    let y_flip = if z >= 31 {
        0
    } else {
        (1u32 << z).saturating_sub(1).saturating_sub(y)
    };
    template
        .replace("{quadkey}", &bing_quadkey(z, x, y))
        .replace("{u}", &bing_quadkey(z, x, y))
        .replace("{zoom}", &z.to_string())
        .replace("{z}", &z.to_string())
        .replace("{x}", &x.to_string())
        .replace("{-y}", &y_flip.to_string())
        .replace("{y}", &y.to_string())
        .replace("{apikey}", api_key)
}

/// Bing tile-system quadkey. `z=3,x=3,y=5` yields `213`.
pub fn bing_quadkey(z: u32, x: u32, y: u32) -> String {
    let mut key = String::with_capacity(z as usize);
    for level in (0..z).rev() {
        let mask = 1u32 << level;
        let mut digit = 0u8;
        if x & mask != 0 {
            digit += 1;
        }
        if y & mask != 0 {
            digit += 2;
        }
        key.push(char::from(b'0' + digit));
    }
    key
}

fn expand_switch(template: &str, x: u32, y: u32) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find("{switch:") {
        out.push_str(&rest[..start]);
        let after = &rest[start + "{switch:".len()..];
        let Some(end) = after.find('}') else {
            out.push_str(&rest[start..]);
            return out;
        };
        let options: Vec<&str> = after[..end]
            .split(',')
            .map(str::trim)
            .filter(|part| !part.is_empty())
            .collect();
        if options.is_empty() {
            out.push_str(&rest[start..=start + "{switch:".len() + end]);
        } else {
            let index = (x.wrapping_add(y) as usize) % options.len();
            out.push_str(options[index]);
        }
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    out
}

fn switch_options(template: &str) -> Vec<String> {
    let Some(start) = template.find("{switch:") else {
        return Vec::new();
    };
    let after = &template[start + "{switch:".len()..];
    let Some(end) = after.find('}') else {
        return Vec::new();
    };
    after[..end]
        .split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(str::to_string)
        .collect()
}

fn substitute_first_switch(template: &str, option: &str) -> String {
    let Some(start) = template.find("{switch:") else {
        return template.to_string();
    };
    let after = &template[start + "{switch:".len()..];
    let Some(end) = after.find('}') else {
        return template.to_string();
    };
    let mut out = String::with_capacity(template.len());
    out.push_str(&template[..start]);
    out.push_str(option);
    out.push_str(&after[end + 1..]);
    out
}

fn url_host(url: &str) -> &str {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    rest.split(['/', '?', '#']).next().unwrap_or(rest)
}

#[derive(Clone, Debug)]
struct HostStat {
    ewma_ms: f64,
    samples: u32,
    failures: u32,
    cooldown_until_ms: u64,
}

/// Order subdomains by measured latency and failure cooldown.
///
/// Hosts that have succeeded are ordered from fastest to slowest by moving average; untested ones come after them; hosts with consecutive failures enter cooldown and are placed last during it.
#[derive(Clone, Debug, Default)]
pub struct HostTable {
    hosts: std::collections::HashMap<String, HostStat>,
}

impl HostTable {
    /// Record one access. `now_ms` uses the same clock as [`Self::order_urls`].
    pub fn record_url(&mut self, url: &str, ok: bool, elapsed_ms: u64, now_ms: u64) {
        let host = url_host(url).to_string();
        let stat = self.hosts.entry(host).or_insert(HostStat {
            ewma_ms: 0.0,
            samples: 0,
            failures: 0,
            cooldown_until_ms: 0,
        });
        if ok {
            stat.ewma_ms = if stat.samples == 0 {
                elapsed_ms as f64
            } else {
                stat.ewma_ms.mul_add(0.5, elapsed_ms as f64 * 0.5)
            };
            stat.samples = stat.samples.saturating_add(1);
            stat.failures = 0;
            stat.cooldown_until_ms = 0;
        } else {
            stat.failures = stat.failures.saturating_add(1);
            let shift = stat.failures.min(5);
            let pause = 1_000u64.saturating_mul(1u64 << shift);
            stat.cooldown_until_ms = now_ms.saturating_add(pause);
        }
    }

    /// Fast and reachable addresses come first. `rotate` only changes the relative order of hosts that have not yet been ranked.
    pub fn order_urls(&self, mut urls: Vec<String>, now_ms: u64, rotate: usize) -> Vec<String> {
        if urls.is_empty() {
            return urls;
        }
        let shift = rotate % urls.len();
        urls.rotate_left(shift);
        urls.sort_by(|left, right| {
            self.penalty(url_host(left), now_ms)
                .total_cmp(&self.penalty(url_host(right), now_ms))
        });
        urls
    }

    fn penalty(&self, host: &str, now_ms: u64) -> f64 {
        match self.hosts.get(host) {
            Some(stat) if stat.cooldown_until_ms > now_ms => 100_000.0 + stat.ewma_ms,
            Some(stat) if stat.samples > 0 => stat.ewma_ms,
            _ => 10_000.0,
        }
    }
}

/// One imagery preset. An empty `countries` means global; otherwise it is an ISO 3166-1 alpha-2 list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImageryPreset {
    /// Stable id.
    pub id: String,
    /// Display name. The UI applies extra translations to a few built-in ids.
    pub name: String,
    /// Kind.
    pub kind: ImageryKind,
    /// TMS template, or WMS endpoint / full GetMap template.
    pub url: String,
    /// WMS `LAYERS`. Empty when the template itself already contains the layers.
    pub layers: String,
    /// Attribution that must be shown.
    pub attribution: String,
    /// The template contains `{apikey}`, so the user must supply their own token.
    pub needs_key: bool,
    /// Maximum zoom.
    pub max_zoom: u8,
    /// Scope. Empty means global.
    pub countries: Vec<String>,
    /// WMS projection. Empty for TMS.
    pub crs: String,
    /// For WMS 1.3 geographic coordinates, latitude comes first in the bbox.
    pub bbox_lat_first: bool,
    /// iD-marked preferred imagery for the region.
    pub best: bool,
}

impl ImageryPreset {
    /// Global imagery, or the current country falls within the scope.
    pub fn covers(&self, country: Option<&str>) -> bool {
        self.countries.is_empty()
            || country.is_some_and(|code| self.countries.iter().any(|item| item.eq_ignore_ascii_case(code)))
    }
}

/// Built-in global imagery plus the iD imagery catalog.
///
/// Mapbox's `a`/`b`/`c` and Tianditu's `t0`–`t7` are chosen at request time by speed and reachability.
/// Each imagery entry in the catalog is scoped globally or to a few country codes.
pub fn imagery_presets() -> &'static [ImageryPreset] {
    static CACHE: std::sync::OnceLock<Vec<ImageryPreset>> = std::sync::OnceLock::new();
    CACHE.get_or_init(load_presets)
}

/// Filter by the current country. Returns everything when `region_only` is false.
pub fn imagery_for_view(country: Option<&str>, region_only: bool) -> Vec<&'static ImageryPreset> {
    imagery_presets()
        .iter()
        .filter(|preset| !region_only || preset.covers(country))
        .collect()
}

/// Country at a lon/lat. `None` over the ocean.
pub fn country_at(lat: f64, lon: f64) -> Option<&'static str> {
    crate::imagery_catalog::country_at(lat, lon)
}

/// English and Chinese names of a country.
pub fn country_names(code: &str) -> Option<(&'static str, &'static str)> {
    crate::imagery_catalog::country_names(code)
}

fn load_presets() -> Vec<ImageryPreset> {
    let mut presets = builtin_presets();
    if let Some(tianditu) = presets.iter_mut().find(|preset| preset.id == "tianditu") {
        tianditu.countries = vec!["CN".to_string(), "HK".to_string(), "MO".to_string()];
    }
    for source in crate::imagery_catalog::sources() {
        if presets.iter().any(|preset| preset.id == source.id) {
            continue;
        }
        let kind = if source.kind == "wms" {
            ImageryKind::Wms
        } else {
            ImageryKind::Tms
        };
        let url = resolve_catalog_url(&source.url);
        presets.push(ImageryPreset {
            needs_key: url.contains("{apikey}"),
            id: source.id.clone(),
            name: source.name.clone(),
            kind,
            url,
            layers: String::new(),
            attribution: source.attribution.clone(),
            max_zoom: source.max_zoom,
            countries: source.countries.clone(),
            crs: source.crs.clone(),
            bbox_lat_first: source.bbox_lat_first,
            best: source.best,
        });
    }
    presets
}

/// Mapbox URLs in the catalog use the `{apikey}` placeholder; on load it is filled with the de-obfuscated token.
fn resolve_catalog_url(url: &str) -> String {
    if url.contains("{apikey}") && url.contains("mapbox.com") {
        url.replace("{apikey}", &crate::imagery_secrets::mapbox_token())
    } else {
        url.to_string()
    }
}

/// Standard OSM raster tiles.
pub const OSM_IMAGERY_URL: &str = "https://tile.openstreetmap.org/{z}/{x}/{y}.png";

/// Attribution that OSM tiles must display.
pub const OSM_IMAGERY_ATTRIBUTION: &str = "© OpenStreetMap contributors";

fn builtin_presets() -> Vec<ImageryPreset> {
    vec![
        builtin(
            "osm",
            "OpenStreetMap",
            ImageryKind::Tms,
            OSM_IMAGERY_URL,
            "",
            OSM_IMAGERY_ATTRIBUTION,
            19,
        ),
        builtin(
            "esri-world",
            "Esri World Imagery",
            ImageryKind::Tms,
            "https://{switch:server,services}.arcgisonline.com/arcgis/rest/services/World_Imagery/MapServer/tile/{z}/{y}/{x}",
            "",
            "Esri, Maxar, Earthstar Geographics, and the GIS User Community",
            22,
        ),
        builtin(
            "esri-clarity",
            "Esri World Imagery (Clarity)",
            ImageryKind::Tms,
            "https://clarity.maptiles.arcgis.com/arcgis/rest/services/World_Imagery/MapServer/tile/{z}/{y}/{x}",
            "",
            "Esri, Maxar, Earthstar Geographics, and the GIS User Community",
            22,
        ),
        builtin(
            "bing",
            "Bing aerial imagery",
            ImageryKind::Tms,
            "https://ecn.{switch:t0,t1,t2,t3}.tiles.virtualearth.net/tiles/a{quadkey}.jpeg?g=1",
            "",
            "© Microsoft Bing",
            22,
        ),
        builtin(
            "mapbox",
            "Mapbox Satellite",
            ImageryKind::Tms,
            format!(
                "https://{{switch:a,b,c}}.tiles.mapbox.com/v4/mapbox.satellite/{{z}}/{{x}}/{{y}}@2x.jpg?access_token={}",
                crate::imagery_secrets::mapbox_token()
            ),
            "",
            "© Mapbox © OpenStreetMap",
            22,
        ),
        builtin(
            "tianditu",
            "天地图卫星影像",
            ImageryKind::Tms,
            format!(
                "https://{{switch:t0,t1,t2,t3,t4,t5,t6,t7}}.tianditu.gov.cn/DataServer?T=img_w&x={{x}}&y={{y}}&l={{z}}&tk={}",
                crate::imagery_secrets::tianditu_tk()
            ),
            "",
            "国家地理信息公共服务平台 天地图",
            18,
        ),
        builtin(
            "sentinel-2025",
            "Sentinel-2 cloudless 2025",
            ImageryKind::Wms,
            "https://tiles.maps.eox.at/wms",
            "s2cloudless-2025_3857",
            "Sentinel-2 cloudless by EOX (modified Copernicus Sentinel data 2025)",
            18,
        ),
        builtin(
            "sentinel-2020",
            "Sentinel-2 cloudless 2020",
            ImageryKind::Wms,
            "https://tiles.maps.eox.at/wms",
            "s2cloudless-2020_3857",
            "Sentinel-2 cloudless by EOX (modified Copernicus Sentinel data 2020)",
            18,
        ),
    ]
}

fn builtin(
    id: &str,
    name: &str,
    kind: ImageryKind,
    url: impl Into<String>,
    layers: &str,
    attribution: &str,
    max_zoom: u8,
) -> ImageryPreset {
    let url = url.into();
    ImageryPreset {
        id: id.to_string(),
        name: name.to_string(),
        kind,
        url: url.clone(),
        layers: layers.to_string(),
        attribution: attribution.to_string(),
        needs_key: url.contains("{apikey}"),
        max_zoom,
        countries: Vec::new(),
        crs: if kind == ImageryKind::Wms {
            "EPSG:3857".to_string()
        } else {
            String::new()
        },
        bbox_lat_first: false,
        best: false,
    }
}

/// Expand an iD-style WMS template. `bbox` is already in the target projection's min, max order.
pub fn expand_wms_template(
    template: &str,
    bbox: [f64; 4],
    width: u32,
    height: u32,
    projection: &str,
) -> String {
    let proj = if projection.is_empty() {
        "EPSG:3857"
    } else {
        projection
    };
    let wkid = proj.rsplit(':').next().unwrap_or(proj);
    let bbox_text = format!("{:.6},{:.6},{:.6},{:.6}", bbox[0], bbox[1], bbox[2], bbox[3]);
    template
        .replace("{proj}", proj)
        .replace("{wkid}", wkid)
        .replace("{bbox}", &bbox_text)
        .replace("{width}", &width.to_string())
        .replace("{height}", &height.to_string())
}

/// WMS GetMap URL (EPSG:3857, bbox in WebMercator meters, minx,miny,maxx,maxy).
pub fn wms_url_3857(base: &str, layers: &str, bbox: [f64; 4], width: u32, height: u32) -> String {
    let sep = if base.contains('?') { '&' } else { '?' };
    format!(
        "{base}{sep}SERVICE=WMS&REQUEST=GetMap&VERSION=1.1.1&LAYERS={layers}&SRS=EPSG:3857&BBOX={},{},{},{}&WIDTH={width}&HEIGHT={height}&FORMAT=image/png&TRANSPARENT=TRUE",
        bbox[0], bbox[1], bbox[2], bbox[3]
    )
}

/// WMS GetMap URL. `bbox` is min-lon, min-lat, max-lon, max-lat.
pub fn wms_url(base: &str, layers: &str, bbox: [f64; 4], width: u32, height: u32) -> String {
    let sep = if base.contains('?') { '&' } else { '?' };
    format!(
        "{base}{sep}SERVICE=WMS&REQUEST=GetMap&VERSION=1.1.1&LAYERS={layers}&SRS=EPSG:4326&BBOX={},{},{},{}&WIDTH={width}&HEIGHT={height}&FORMAT=image/png&TRANSPARENT=TRUE",
        bbox[0], bbox[1], bbox[2], bbox[3]
    )
}

/// Imagery offset (pixels). The same data source can have different offsets in different places.
#[derive(Clone, Debug, PartialEq)]
pub struct ImageryOffset {
    /// Imagery source id.
    pub source_id: String,
    /// Latitude.
    pub lat: f64,
    /// Longitude.
    pub lon: f64,
    /// Eastward pixel offset.
    pub dx_px: f64,
    /// Southward pixel offset.
    pub dy_px: f64,
}

/// In-memory offset database.
#[derive(Clone, Debug, Default)]
pub struct OffsetDb {
    /// All records.
    pub entries: Vec<ImageryOffset>,
}

impl OffsetDb {
    /// Load from a JSON array. Entries with missing fields are skipped and counted into the returned error count.
    pub fn from_json(text: &str) -> Result<(Self, usize), String> {
        let value: serde_json::Value = serde_json::from_str(text).map_err(|err| err.to_string())?;
        let Some(items) = value.as_array() else {
            return Err("offset db must be a JSON array".into());
        };
        let mut entries = Vec::new();
        let mut skipped = 0;
        for item in items {
            let Some(source_id) = item.get("source_id").and_then(|v| v.as_str()) else {
                skipped += 1;
                continue;
            };
            let Some(lat) = item.get("lat").and_then(|v| v.as_f64()) else {
                skipped += 1;
                continue;
            };
            let Some(lon) = item.get("lon").and_then(|v| v.as_f64()) else {
                skipped += 1;
                continue;
            };
            entries.push(ImageryOffset {
                source_id: source_id.to_owned(),
                lat,
                lon,
                dx_px: item.get("dx_px").and_then(|v| v.as_f64()).unwrap_or(0.0),
                dy_px: item.get("dy_px").and_then(|v| v.as_f64()).unwrap_or(0.0),
            });
        }
        Ok((Self { entries }, skipped))
    }

    /// Offset nearest to the query point within the same data source.
    pub fn nearest(&self, source_id: &str, lat: f64, lon: f64) -> Option<&ImageryOffset> {
        self.entries
            .iter()
            .filter(|entry| entry.source_id == source_id)
            .min_by(|a, b| {
                dist2(a.lat, a.lon, lat, lon)
                    .partial_cmp(&dist2(b.lat, b.lon, lat, lon))
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
    }

    /// Alias of `nearest`, emphasizing lookup by data source.
    pub fn nearest_for_source(&self, source_id: &str, lat: f64, lon: f64) -> Option<&ImageryOffset> {
        self.nearest(source_id, lat, lon)
    }

    /// Serialize to a JSON array for persistence and sharing.
    ///
    /// Output order matches [`entries`](Self::entries), so the same in-memory data always yields the same
    /// string. Non-finite floats (NaN / infinity) are serialized as JSON `null`; on load those entries
    /// are skipped or treated as 0.
    pub fn to_json(&self) -> String {
        let items: Vec<serde_json::Value> = self
            .entries
            .iter()
            .map(|entry| {
                serde_json::json!({
                    "source_id": entry.source_id.as_str(),
                    "lat": entry.lat,
                    "lon": entry.lon,
                    "dx_px": entry.dx_px,
                    "dy_px": entry.dy_px,
                })
            })
            .collect();
        serde_json::to_string(&serde_json::Value::Array(items)).unwrap_or_else(|_| "[]".to_owned())
    }

    /// Read JSON from `path` and call [`from_json`](Self::from_json).
    ///
    /// Returns `(database, skipped entry count)`.
    pub fn load(path: impl AsRef<std::path::Path>) -> Result<(Self, usize), String> {
        let path = path.as_ref();
        let text = std::fs::read_to_string(path)
            .map_err(|err| format!("failed to read offset db {}: {err}", path.display()))?;
        Self::from_json(&text)
    }

    /// Atomically write `path`: first write a temporary file in the same directory, then rename over the target.
    ///
    /// On Windows a rename cannot overwrite an existing file, so try rename first; on failure delete the target and
    /// retry. Deletion and rename are not strictly atomic, but no half-written JSON is left behind.
    pub fn save(&self, path: impl AsRef<std::path::Path>) -> Result<(), String> {
        use std::io::Write;

        let path = path.as_ref();
        let parent = match path.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => parent,
            _ => std::path::Path::new("."),
        };
        let file_name = path
            .file_name()
            .ok_or_else(|| format!("invalid offset db path: {}", path.display()))?;
        let tmp = parent.join(format!(
            ".{}.{}.tmp",
            file_name.to_string_lossy(),
            unique_suffix()
        ));

        // Flush the content fully and fsync before rename, avoiding an empty file after the rename.
        let write_result = (|| -> std::io::Result<()> {
            let mut file = std::fs::File::create(&tmp)?;
            file.write_all(self.to_json().as_bytes())?;
            file.sync_all()
        })();
        if let Err(err) = write_result {
            let _ = std::fs::remove_file(&tmp);
            return Err(format!("failed to write temp offset db {}: {err}", tmp.display()));
        }

        if let Err(first_err) = std::fs::rename(&tmp, path) {
            if !path.exists() {
                let _ = std::fs::remove_file(&tmp);
                return Err(format!(
                    "failed to rename temp offset db {}: {first_err}",
                    tmp.display()
                ));
            }
            // Windows: rename does not overwrite existing files, so delete first and retry.
            if let Err(err) = std::fs::remove_file(path) {
                let _ = std::fs::remove_file(&tmp);
                return Err(format!("failed to replace offset db {}: {err}", path.display()));
            }
            if let Err(err) = std::fs::rename(&tmp, path) {
                let _ = std::fs::remove_file(&tmp);
                return Err(format!(
                    "failed to rename temp offset db {}: {err}",
                    tmp.display()
                ));
            }
        }
        Ok(())
    }

    /// Insert or replace an offset. Same `source_id` with coordinates within 1e-6 degrees is treated as the same record.
    pub fn insert_or_replace(&mut self, entry: ImageryOffset) {
        const EPSILON: f64 = 1e-6;
        let existing = self.entries.iter_mut().find(|existing| {
            existing.source_id == entry.source_id
                && (existing.lat - entry.lat).abs() <= EPSILON
                && (existing.lon - entry.lon).abs() <= EPSILON
        });
        match existing {
            Some(slot) => *slot = entry,
            None => self.entries.push(entry),
        }
    }
}

/// Generate a temporary filename suffix in the same directory to avoid concurrent writes clobbering each other.
fn unique_suffix() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{}.{nanos}.{seq}", std::process::id())
}

fn dist2(lat: f64, lon: f64, qlat: f64, qlon: f64) -> f64 {
    let dlat = lat - qlat;
    let dlon = lon - qlon;
    dlat * dlat + dlon * dlon
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_db() -> OffsetDb {
        OffsetDb {
            entries: vec![
                ImageryOffset {
                    source_id: "bing".into(),
                    lat: 39.9042,
                    lon: 116.4074,
                    dx_px: 12.5,
                    dy_px: -3.25,
                },
                ImageryOffset {
                    source_id: "esri".into(),
                    lat: -33.8688,
                    lon: 151.2093,
                    dx_px: 0.0,
                    dy_px: 0.0,
                },
            ],
        }
    }

    /// Temporary file for tests, cleaned up on Drop, so failed assertions leave no garbage in the temp directory.
    struct TempFile(std::path::PathBuf);

    impl TempFile {
        fn new(tag: &str) -> Self {
            let name = format!(
                "neo-style-offsets-{tag}-{}-{}",
                std::process::id(),
                unique_suffix()
            );
            Self(std::env::temp_dir().join(name))
        }
    }

    impl Drop for TempFile {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    #[test]
    fn tile_templates_follow_editor_layer_index() {
        assert_eq!(bing_quadkey(3, 3, 5), "213");
        let esri = tile_url(
            "https://{switch:server,services}.example/tile/{zoom}/{y}/{x}",
            3,
            3,
            5,
            "",
        );
        assert_eq!(esri, "https://server.example/tile/3/5/3");
        let bing = tile_url(
            "https://ecn.{switch:t0,t1}.tiles.virtualearth.net/tiles/a{quadkey}.jpeg?g=1",
            3,
            3,
            5,
            "",
        );
        assert_eq!(bing, "https://ecn.t0.tiles.virtualearth.net/tiles/a213.jpeg?g=1");
        let mapbox = tile_url(
            "https://a.tiles.mapbox.com/v4/mapbox.satellite/{z}/{x}/{y}.jpg?access_token={apikey}",
            1,
            1,
            0,
            "pk.test",
        );
        assert!(mapbox.ends_with("access_token=pk.test"));
        assert!(mapbox.contains("/1/1/0.jpg"));
        assert!(
            imagery_presets()
                .iter()
                .any(|preset| preset.id == "sentinel-2025")
        );
    }

    #[test]
    fn satellite_presets_list_every_subdomain() {
        let mapbox = imagery_presets()
            .iter()
            .find(|preset| preset.id == "mapbox")
            .expect("mapbox");
        let mapbox_urls = tile_url_candidates(&mapbox.url, 16, 52191, 28298, "");
        assert_eq!(mapbox_urls.len(), 3);
        let mapbox_token = crate::imagery_secrets::mapbox_token();
        let expected_mapbox = format!(
            "https://b.tiles.mapbox.com/v4/mapbox.satellite/16/52191/28298@2x.jpg?access_token={mapbox_token}"
        );
        assert!(mapbox_urls.iter().any(|url| url == &expected_mapbox));
        let tianditu = imagery_presets()
            .iter()
            .find(|preset| preset.id == "tianditu")
            .expect("tianditu");
        let tianditu_urls = tile_url_candidates(&tianditu.url, 7, 100, 51, "");
        assert_eq!(tianditu_urls.len(), 8);
        let tianditu_tk = crate::imagery_secrets::tianditu_tk();
        let expected_tianditu =
            format!("https://t3.tianditu.gov.cn/DataServer?T=img_w&x=100&y=51&l=7&tk={tianditu_tk}");
        assert!(tianditu_urls.iter().any(|url| url == &expected_tianditu));
    }

    #[test]
    fn imagery_follows_the_country_under_the_map() {
        assert!(imagery_presets().len() > 200);
        assert_eq!(country_at(39.9, 116.4), Some("CN"));
        assert_eq!(country_at(38.9, -77.0), Some("US"));
        assert_eq!(country_names("CN").map(|names| names.1), Some("中华人民共和国"));
        let germany = imagery_for_view(Some("DE"), true);
        assert!(
            germany
                .iter()
                .any(|preset| preset.countries.iter().any(|code| code == "DE"))
        );
        assert!(germany.iter().all(|preset| preset.covers(Some("DE"))));
        assert!(
            imagery_for_view(Some("US"), true)
                .iter()
                .any(|preset| preset.id == "esri-world")
        );
        let tianditu = imagery_presets()
            .iter()
            .find(|preset| preset.id == "tianditu")
            .expect("tianditu");
        assert_eq!(tianditu.max_zoom, 18);
        assert!(tianditu.covers(Some("CN")));
        assert!(!tianditu.covers(Some("US")));
        let osm = imagery_presets()
            .iter()
            .find(|preset| preset.id == "osm")
            .expect("osm");
        assert!(osm.url.contains("tile.openstreetmap.org/{z}/{x}/{y}.png"));
        assert!(osm.covers(Some("CH")));
        let expanded = expand_wms_template(
            "https://example.test/wms?SRS={proj}&BBOX={bbox}&WIDTH={width}&HEIGHT={height}",
            [1.0, 2.0, 3.0, 4.0],
            256,
            256,
            "EPSG:3857",
        );
        assert!(expanded.contains("SRS=EPSG:3857"));
        assert!(expanded.contains("BBOX=1.000000,2.000000,3.000000,4.000000"));
        assert!(expanded.contains("WIDTH=256"));
    }

    #[test]
    fn host_order_prefers_fast_reachable_subdomains() {
        let mut table = HostTable::default();
        table.record_url("https://b.tiles.mapbox.com/v4/x", true, 80, 1_000);
        table.record_url("https://a.tiles.mapbox.com/v4/x", true, 400, 1_000);
        table.record_url("https://c.tiles.mapbox.com/v4/x", false, 8_000, 1_000);
        let ordered = table.order_urls(
            vec![
                "https://a.tiles.mapbox.com/v4/x".into(),
                "https://b.tiles.mapbox.com/v4/x".into(),
                "https://c.tiles.mapbox.com/v4/x".into(),
            ],
            1_500,
            0,
        );
        assert!(ordered[0].contains("://b."));
        assert!(ordered[1].contains("://a."));
        assert!(ordered[2].contains("://c."));

        table.record_url("https://t0.tianditu.gov.cn/DataServer", true, 120, 2_000);
        let tianditu: Vec<String> = (0..8)
            .map(|index| format!("https://t{index}.tianditu.gov.cn/DataServer"))
            .collect();
        let ordered = table.order_urls(tianditu, 2_000, 3);
        assert!(ordered[0].contains("://t0."));
        assert!(ordered[1].contains("://t"));
    }

    #[test]
    fn json_round_trip_is_deterministic() {
        let db = sample_db();
        let first = db.to_json();
        let second = db.to_json();
        assert_eq!(first, second, "to_json must be deterministic");

        // The output itself is a valid JSON array and contains all agreed fields.
        let parsed: serde_json::Value = serde_json::from_str(&first).expect("to_json must output valid JSON");
        assert!(parsed.is_array());
        for key in ["source_id", "lat", "lon", "dx_px", "dy_px"] {
            assert!(
                first.contains(&format!("\"{key}\"")),
                "missing field {key}: {first}"
            );
        }

        let (loaded, skipped) = OffsetDb::from_json(&first).expect("from_json must accept to_json output");
        assert_eq!(skipped, 0, "entries we wrote ourselves must not be skipped");
        assert_eq!(
            loaded.entries, db.entries,
            "entries must be identical after the round trip"
        );
    }

    #[test]
    fn insert_or_replace_deduplicates_by_source_and_position() {
        let mut db = OffsetDb::default();

        db.insert_or_replace(ImageryOffset {
            source_id: "bing".into(),
            lat: 39.9,
            lon: 116.4,
            dx_px: 1.0,
            dy_px: 2.0,
        });
        // Same data source, coordinates within 1e-6 degrees: replace rather than append.
        db.insert_or_replace(ImageryOffset {
            source_id: "bing".into(),
            lat: 39.9 + 5e-7,
            lon: 116.4 - 5e-7,
            dx_px: 9.0,
            dy_px: 8.0,
        });
        assert_eq!(db.entries.len(), 1);
        assert_eq!(db.entries[0].dx_px, 9.0);
        assert_eq!(db.entries[0].dy_px, 8.0);

        // Different data sources are kept separately even at identical coordinates.
        db.insert_or_replace(ImageryOffset {
            source_id: "esri".into(),
            lat: 39.9,
            lon: 116.4,
            dx_px: 3.0,
            dy_px: 4.0,
        });
        assert_eq!(db.entries.len(), 2);

        // Same data source but coordinates beyond the tolerance: add a new entry.
        db.insert_or_replace(ImageryOffset {
            source_id: "bing".into(),
            lat: 40.9,
            lon: 116.4,
            dx_px: 5.0,
            dy_px: 6.0,
        });
        assert_eq!(db.entries.len(), 3);

        let nearest = db
            .nearest_for_source("bing", 39.9, 116.4)
            .expect("existing entry");
        assert_eq!(nearest.dx_px, 9.0);
        assert!(db.nearest_for_source("missing", 0.0, 0.0).is_none());
    }

    #[test]
    fn save_then_load_round_trips() {
        let file = TempFile::new("save-load");
        let db = sample_db();

        db.save(&file.0).expect("first save");
        assert!(file.0.exists(), "target file must exist after saving");
        let (loaded, skipped) = OffsetDb::load(&file.0).expect("load");
        assert_eq!(skipped, 0);
        assert_eq!(loaded.entries, db.entries);

        // Overwrite an existing file: on Windows rename cannot overwrite directly, so the code deletes the target first.
        let mut updated = db.clone();
        updated.insert_or_replace(ImageryOffset {
            source_id: "bing".into(),
            lat: 39.9042,
            lon: 116.4074,
            dx_px: 42.0,
            dy_px: 0.5,
        });
        updated.save(&file.0).expect("overwrite save");
        let (reloaded, skipped) = OffsetDb::load(&file.0).expect("reload");
        assert_eq!(skipped, 0);
        assert_eq!(reloaded.entries, updated.entries);
        assert_eq!(reloaded.entries.len(), 2);

        // After saving, no temporary files should remain in the same directory.
        let dir = file.0.parent().expect("temp directory");
        let prefix = format!(".{}.", file.0.file_name().expect("file name").to_string_lossy());
        let leftovers: Vec<_> = std::fs::read_dir(dir)
            .expect("read temp directory")
            .filter_map(|item| item.ok())
            .map(|item| item.file_name().to_string_lossy().into_owned())
            .filter(|name| name.starts_with(&prefix) && name.ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "leftover temp files: {leftovers:?}");
    }
}
