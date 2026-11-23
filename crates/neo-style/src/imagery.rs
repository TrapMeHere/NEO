//! 影像层：WMS / TMS URL 与偏移数据库。
//!
//! 不打包第三方影像目录。调用方传入模板，本模块只负责拼 URL 和查偏移。

/// 影像种类。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageryKind {
    /// `{z}/{x}/{y}` 瓦片。
    Tms,
    /// GetMap。
    Wms,
}

/// 一条影像源。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImagerySource {
    /// 稳定 id。
    pub id: String,
    /// 显示名。
    pub name: String,
    /// 种类。
    pub kind: ImageryKind,
    /// TMS 模板或 WMS 端点。
    pub url: String,
    /// 署名，必须展示给用户。
    pub attribution: String,
    /// 最小缩放。
    pub min_zoom: u8,
    /// 最大缩放。
    pub max_zoom: u8,
}

/// 把 TMS 模板里的 `{z}` `{x}` `{y}` `{-y}` 换成具体瓦片。
pub fn tms_url(template: &str, z: u32, x: u32, y: u32) -> String {
    tile_url(template, z, x, y, "")
}

/// 展开一张瓦片的 URL。
///
/// 除 `{z}`/`{zoom}`、`{x}`、`{y}`、`{-y}` 外，还支持 iD / JOSM 用的：
/// `{quadkey}`（Bing）、`{switch:a,b}`（此处按瓦片散列选一个）、`{apikey}`。
/// 需要按速度和可达性挑选子域时，用 [`tile_url_candidates`] 再交给 [`HostTable`]。
pub fn tile_url(template: &str, z: u32, x: u32, y: u32, api_key: &str) -> String {
    expand_tile(&expand_switch(template, x, y), z, x, y, api_key)
}

/// `{switch:a,b,c}` 的每个子域各一条 URL，顺序与模板里一致。
///
/// 没有 `{switch:…}` 时只返回一条。调用方再用 [`HostTable`] 把快的、连通的排到前面。
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

/// Bing 瓦片系统的 quadkey。`z=3,x=3,y=5` 得到 `213`。
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

/// 按实测耗时和失败冷却给子域排序。
///
/// 成功过的主机按滑动平均耗时从快到慢；还没测过的排在它们后面；连续失败的进入冷却，冷却期内排到最后。
#[derive(Clone, Debug, Default)]
pub struct HostTable {
    hosts: std::collections::HashMap<String, HostStat>,
}

impl HostTable {
    /// 记下一次访问。`now_ms` 与 [`Self::order_urls`] 使用同一时钟。
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

    /// 快且可达的地址在前。`rotate` 只改变还没分出快慢的主机的相对顺序。
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

/// 一条影像预设。`countries` 为空表示全球，否则是 ISO 3166-1 alpha-2。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImageryPreset {
    /// 稳定 id。
    pub id: String,
    /// 显示名。界面会对少数内置 id 另做翻译。
    pub name: String,
    /// 种类。
    pub kind: ImageryKind,
    /// TMS 模板，或 WMS 端点 / 完整 GetMap 模板。
    pub url: String,
    /// WMS `LAYERS`。模板本身已含图层时为空。
    pub layers: String,
    /// 必须展示的署名。
    pub attribution: String,
    /// 模板含 `{apikey}`，要由用户填入自己的令牌。
    pub needs_key: bool,
    /// 最大缩放。
    pub max_zoom: u8,
    /// 作用范围。空表示全球。
    pub countries: Vec<String>,
    /// WMS 投影。TMS 为空。
    pub crs: String,
    /// WMS 1.3 地理坐标时，bbox 纬度在前。
    pub bbox_lat_first: bool,
    /// iD 标记的该地区优先影像。
    pub best: bool,
}

impl ImageryPreset {
    /// 全球影像，或当前国家落在作用范围内。
    pub fn covers(&self, country: Option<&str>) -> bool {
        self.countries.is_empty()
            || country.is_some_and(|code| self.countries.iter().any(|item| item.eq_ignore_ascii_case(code)))
    }
}

/// 内置全球影像，加上 iD 影像目录。
///
/// Mapbox 的 `a`/`b`/`c` 与天地图的 `t0`–`t7` 在请求时按速度和可达性挑选。
/// 目录里每条影像的作用范围是全球，或若干国家代码。
pub fn imagery_presets() -> &'static [ImageryPreset] {
    static CACHE: std::sync::OnceLock<Vec<ImageryPreset>> = std::sync::OnceLock::new();
    CACHE.get_or_init(load_presets)
}

/// 按当前国家过滤。`region_only` 为假时返回全部。
pub fn imagery_for_view(country: Option<&str>, region_only: bool) -> Vec<&'static ImageryPreset> {
    imagery_presets()
        .iter()
        .filter(|preset| !region_only || preset.covers(country))
        .collect()
}

/// 经纬度所在国家。海域为 `None`。
pub fn country_at(lat: f64, lon: f64) -> Option<&'static str> {
    crate::imagery_catalog::country_at(lat, lon)
}

/// 国家英文名和中文名。
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

/// 目录里的 Mapbox URL 用 `{apikey}` 占位；加载时填入混淆还原的令牌。
fn resolve_catalog_url(url: &str) -> String {
    if url.contains("{apikey}") && url.contains("mapbox.com") {
        url.replace("{apikey}", &crate::imagery_secrets::mapbox_token())
    } else {
        url.to_string()
    }
}

/// 标准 OSM 栅格瓦片。
pub const OSM_IMAGERY_URL: &str = "https://tile.openstreetmap.org/{z}/{x}/{y}.png";

/// OSM 瓦片必须展示的署名。
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

/// 展开 iD 风格的 WMS 模板。`bbox` 已经是目标投影下的 min、max 顺序。
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

/// WMS GetMap URL（EPSG:3857，bbox 为 WebMercator 米，minx,miny,maxx,maxy）。
pub fn wms_url_3857(base: &str, layers: &str, bbox: [f64; 4], width: u32, height: u32) -> String {
    let sep = if base.contains('?') { '&' } else { '?' };
    format!(
        "{base}{sep}SERVICE=WMS&REQUEST=GetMap&VERSION=1.1.1&LAYERS={layers}&SRS=EPSG:3857&BBOX={},{},{},{}&WIDTH={width}&HEIGHT={height}&FORMAT=image/png&TRANSPARENT=TRUE",
        bbox[0], bbox[1], bbox[2], bbox[3]
    )
}

/// WMS GetMap URL。`bbox` 是 min-lon, min-lat, max-lon, max-lat。
pub fn wms_url(base: &str, layers: &str, bbox: [f64; 4], width: u32, height: u32) -> String {
    let sep = if base.contains('?') { '&' } else { '?' };
    format!(
        "{base}{sep}SERVICE=WMS&REQUEST=GetMap&VERSION=1.1.1&LAYERS={layers}&SRS=EPSG:4326&BBOX={},{},{},{}&WIDTH={width}&HEIGHT={height}&FORMAT=image/png&TRANSPARENT=TRUE",
        bbox[0], bbox[1], bbox[2], bbox[3]
    )
}

/// 影像偏移（像素）。同一数据源在不同地点可以有不同偏移。
#[derive(Clone, Debug, PartialEq)]
pub struct ImageryOffset {
    /// 影像源 id。
    pub source_id: String,
    /// 纬度。
    pub lat: f64,
    /// 经度。
    pub lon: f64,
    /// 向东的像素偏移。
    pub dx_px: f64,
    /// 向南的像素偏移。
    pub dy_px: f64,
}

/// 内存里的偏移库。
#[derive(Clone, Debug, Default)]
pub struct OffsetDb {
    /// 全部记录。
    pub entries: Vec<ImageryOffset>,
}

impl OffsetDb {
    /// 从 JSON 数组加载。缺字段的条目被跳过并计入返回的错误数。
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

    /// 同一数据源里离查询点最近的偏移。
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

    /// `nearest` 的别名，强调按数据源查询。
    pub fn nearest_for_source(&self, source_id: &str, lat: f64, lon: f64) -> Option<&ImageryOffset> {
        self.nearest(source_id, lat, lon)
    }

    /// 序列化为 JSON 数组，便于持久化与分享。
    ///
    /// 输出顺序与 [`entries`](Self::entries) 一致，因此同样的内存数据总是得到同样的
    /// 字符串。非有限浮点数（NaN / 无穷）会被序列化成 JSON `null`，加载时这些条目
    /// 会被跳过或按 0 处理。
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

    /// 从 `path` 读取 JSON 并调用 [`from_json`](Self::from_json)。
    ///
    /// 返回 `(数据库, 被跳过条目数)`。
    pub fn load(path: impl AsRef<std::path::Path>) -> Result<(Self, usize), String> {
        let path = path.as_ref();
        let text = std::fs::read_to_string(path)
            .map_err(|err| format!("failed to read offset db {}: {err}", path.display()))?;
        Self::from_json(&text)
    }

    /// 原子写入 `path`：先写同目录临时文件，再重命名覆盖目标。
    ///
    /// Windows 上重命名不能覆盖已存在的文件，因此先尝试 rename；失败时删除目标后
    /// 重试。删除与重命名之间不是严格原子的，但不会留下半截 JSON。
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

        // 先把内容完整落盘并 fsync，避免 rename 后出现空文件。
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
            // Windows: rename 不覆盖现有文件，先删除再重试。
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

    /// 插入或替换一条偏移。同一 `source_id` 且坐标相差不超过 1e-6 度视为同一条记录。
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

/// 生成同目录临时文件名后缀，避免并发写入互相覆盖。
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

    /// 测试用临时文件，Drop 时清理，断言失败也不会在 temp 目录留垃圾。
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
        let expected_tianditu = format!(
            "https://t3.tianditu.gov.cn/DataServer?T=img_w&x=100&y=51&l=7&tk={tianditu_tk}"
        );
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
        assert_eq!(first, second, "to_json 必须是确定性的");

        // 输出本身就是合法 JSON 数组，并且包含全部约定字段。
        let parsed: serde_json::Value = serde_json::from_str(&first).expect("to_json 输出合法 JSON");
        assert!(parsed.is_array());
        for key in ["source_id", "lat", "lon", "dx_px", "dy_px"] {
            assert!(first.contains(&format!("\"{key}\"")), "缺少字段 {key}: {first}");
        }

        let (loaded, skipped) = OffsetDb::from_json(&first).expect("from_json 应接受 to_json 输出");
        assert_eq!(skipped, 0, "自己写出的条目不应被跳过");
        assert_eq!(loaded.entries, db.entries, "往返后条目完全一致");
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
        // 同一数据源、坐标在 1e-6 度以内：替换而不是追加。
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

        // 不同数据源即使坐标相同也各自保留。
        db.insert_or_replace(ImageryOffset {
            source_id: "esri".into(),
            lat: 39.9,
            lon: 116.4,
            dx_px: 3.0,
            dy_px: 4.0,
        });
        assert_eq!(db.entries.len(), 2);

        // 同一数据源但坐标超出容差：新增一条。
        db.insert_or_replace(ImageryOffset {
            source_id: "bing".into(),
            lat: 40.9,
            lon: 116.4,
            dx_px: 5.0,
            dy_px: 6.0,
        });
        assert_eq!(db.entries.len(), 3);

        let nearest = db.nearest_for_source("bing", 39.9, 116.4).expect("有记录");
        assert_eq!(nearest.dx_px, 9.0);
        assert!(db.nearest_for_source("missing", 0.0, 0.0).is_none());
    }

    #[test]
    fn save_then_load_round_trips() {
        let file = TempFile::new("save-load");
        let db = sample_db();

        db.save(&file.0).expect("首次保存");
        assert!(file.0.exists(), "保存后目标文件应存在");
        let (loaded, skipped) = OffsetDb::load(&file.0).expect("加载");
        assert_eq!(skipped, 0);
        assert_eq!(loaded.entries, db.entries);

        // 覆盖已有文件：Windows 上 rename 不能直接覆盖，代码会先删除目标。
        let mut updated = db.clone();
        updated.insert_or_replace(ImageryOffset {
            source_id: "bing".into(),
            lat: 39.9042,
            lon: 116.4074,
            dx_px: 42.0,
            dy_px: 0.5,
        });
        updated.save(&file.0).expect("覆盖保存");
        let (reloaded, skipped) = OffsetDb::load(&file.0).expect("重新加载");
        assert_eq!(skipped, 0);
        assert_eq!(reloaded.entries, updated.entries);
        assert_eq!(reloaded.entries.len(), 2);

        // 保存后同目录不应残留临时文件。
        let dir = file.0.parent().expect("temp 目录");
        let prefix = format!(".{}.", file.0.file_name().expect("文件名").to_string_lossy());
        let leftovers: Vec<_> = std::fs::read_dir(dir)
            .expect("读取 temp 目录")
            .filter_map(|item| item.ok())
            .map(|item| item.file_name().to_string_lossy().into_owned())
            .filter(|name| name.starts_with(&prefix) && name.ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "残留临时文件: {leftovers:?}");
    }
}
