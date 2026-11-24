//! # neo-style — 样式、预设与过滤器
//!
//! 本期实现"内置配色规则"的最小闭环（MapCSS 子集解析器在里程碑 4）：
//!
//! * 样式表不依赖 `neo-store` / GPU / UI，只依赖 `neo-core` 的符号类型；
//! * Sym 由调用方（`neo-scene`）预先驻留后注入，避免样式层反向依赖数据层；
//! * 颜色用 4 字节，直接进顶点。
//!
//! 设计要点（docs/11）：MapCSS 是 JOSM 的实际样式语言，兼容它是"完整替代"的必要条件；
//! 但第一阶段先用手写规则跑通渲染管线，MapCSS 解析器作为独立模块后接。

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod assets;
pub mod coverage;
pub mod editor;
pub mod imagery;
mod imagery_catalog;
mod imagery_secrets;
pub mod mapcss;
pub use assets::{asset_roots, find_asset, read_asset};
pub use coverage::{CoverageReport, CoverageRow, JOSM_GROUPS, coverage_markdown, preset_coverage};
pub use editor::set_declaration;
pub use imagery::{
    ImageryKind, ImageryOffset, ImageryPreset, ImagerySource, OffsetDb, bing_quadkey, imagery_presets,
    tile_url, tms_url, wms_url,
};
pub use mapcss::{CompiledMapCss, Element, MapCss};

use neo_core::Sym;

pub mod schema;
pub use schema::{
    PresetCategory, PresetGeometryFilter, SchemaPreset, best_preset_for_tags, categories_for_preset,
    nsi_package_version, preset_categories, preset_in_category, preset_match_score, schema_package_version,
    schema_preset, schema_presets, suggestion_presets,
};

/// 样式用到的 tag key（由调用方驻留后注入）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StyleKeys {
    /// `highway`
    pub highway: Sym,
    /// `building`
    pub building: Sym,
    /// `waterway`
    pub waterway: Sym,
    /// `natural`
    pub natural: Sym,
    /// `railway`
    pub railway: Sym,
    /// `area`
    pub area: Sym,
}

/// 调色板（RGBA）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Colors {
    /// 地图背景。
    pub background: [u8; 4],
    /// 普通道路。
    pub road: [u8; 4],
    /// 主干道。
    pub road_major: [u8; 4],
    /// 铁路。
    pub rail: [u8; 4],
    /// 建筑填充。
    pub building: [u8; 4],
    /// 水体填充。
    pub water: [u8; 4],
    /// 绿地填充。
    pub green: [u8; 4],
    /// 通用面填充（amenity/shop 等 area preset 回退色）。
    pub area: [u8; 4],
    /// 默认线（未知 way）。
    pub line_default: [u8; 4],
    /// 节点图标。
    pub node: [u8; 4],
    /// 选中对象。
    pub selection: [u8; 4],
    /// 桥梁外描边（casing）。
    pub bridge_casing: [u8; 4],
    /// 隧道外描边（casing）。
    pub tunnel_casing: [u8; 4],
    /// 过水路面（ford）的水色外描边。
    pub ford_water: [u8; 4],
    /// 路堤坡侧刻线（梯状坡面）色。
    pub embankment: [u8; 4],
    /// 路堑坡侧刻线（梳状齿线）色。
    pub cutting: [u8; 4],
}

impl Default for Colors {
    fn default() -> Self {
        Self {
            background: [24, 26, 30, 255],
            road: [235, 190, 120, 255],
            road_major: [245, 205, 130, 255],
            rail: [150, 150, 160, 255],
            building: [120, 110, 100, 200],
            water: [70, 120, 180, 220],
            green: [90, 140, 90, 200],
            area: [110, 120, 140, 120],
            line_default: [200, 200, 200, 255],
            node: [250, 250, 250, 255],
            selection: [255, 140, 0, 255],
            bridge_casing: [112, 104, 94, 255],
            tunnel_casing: [96, 104, 124, 255],
            ford_water: [82, 152, 214, 170],
            embankment: [216, 184, 120, 255],
            cutting: [198, 96, 74, 255],
        }
    }
}

/// way 的外描边（casing）：由场景层在本体下方扩宽绘制。
#[derive(Clone, Debug, PartialEq)]
pub struct CasingStyle {
    /// 描边颜色。
    pub color: [u8; 4],
    /// 描边宽度（屏幕像素）。
    pub width_px: f32,
    /// 虚线图案（屏幕像素）；`None` 为实线。
    pub dashes: Option<Vec<f32>>,
}

/// 坡侧刻线朝向。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HachureSide {
    /// 点序左侧。
    Left,
    /// 点序右侧。
    Right,
    /// 两侧。
    Both,
}

/// 路堤 / 路堑的坡侧刻线（hachure）样式。
///
/// 路堤与路堑靠**符号形状 + 颜色**区分（只靠刻线朝向在小比例尺下看不出来）：
/// 路堤是带外缘坡脚线的“梯状坡面”，路堑是不带外缘线的“梳状齿线”。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HachureStyle {
    /// 刻线颜色。
    pub color: [u8; 4],
    /// 沿 way 的间隔（屏幕像素）。
    pub spacing_px: f32,
    /// 单根刻线长度（屏幕像素）。
    pub length_px: f32,
    /// 单根刻线宽度（屏幕像素）；外缘线也用这个宽度。
    pub width_px: f32,
    /// 刻线朝向。
    pub side: HachureSide,
    /// 是否在刻线外端再画一条平行的坡脚线（路堤的“梯状”符号）。
    pub baseline: bool,
}

/// 从标签解析出的结构物事实（桥、隧道、路堤、路堑、过水路面）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StructureFacts {
    /// `bridge=*`（真值）。
    pub bridge: bool,
    /// `tunnel=*`（真值）。
    pub tunnel: bool,
    /// 路堤及坡侧。
    pub embankment: Option<HachureSide>,
    /// 路堑及坡侧。
    pub cutting: Option<HachureSide>,
    /// `ford=*` 或 `highway=ford`。
    pub ford: bool,
}

/// tag 值是否为真：非空，且不是 `no` / `false` / `0`（大小写不敏感）。
fn tag_value_is_true(value: &str) -> bool {
    let value = value.trim();
    !value.is_empty()
        && !value.eq_ignore_ascii_case("no")
        && !value.eq_ignore_ascii_case("false")
        && value != "0"
}

/// 从 `embankment` / `cutting` 的值解析坡侧，无法识别时用 `default`。
fn hachure_side_from_value(value: &str, default: HachureSide) -> HachureSide {
    if value.eq_ignore_ascii_case("left") {
        HachureSide::Left
    } else if value.eq_ignore_ascii_case("right") {
        HachureSide::Right
    } else if value.eq_ignore_ascii_case("both") {
        HachureSide::Both
    } else {
        default
    }
}

/// 从字符串标签解析结构物事实（键名与真值判断均大小写不敏感）。
///
/// - `bridge` / `tunnel` 键为真即为真（`viaduct`、`building_passage` 等各类取值均算）；
/// - `man_made=embankment` 或 `embankment` 键为真 → 路堤，坡侧默认右侧；
/// - `man_made=cutting` 或 `cutting` 键为真 → 路堑，坡侧默认左侧；
/// - `ford` 键为真，或 `highway=ford` → 过水路面。
pub fn structure_facts<K: AsRef<str>, V: AsRef<str>>(tags: &[(K, V)]) -> StructureFacts {
    let value_of = |key: &str| -> Option<&str> {
        tags.iter()
            .find(|(k, _)| k.as_ref().eq_ignore_ascii_case(key))
            .map(|(_, v)| v.as_ref())
    };
    let truthy = |key: &str| value_of(key).is_some_and(tag_value_is_true);
    let man_made_is = |want: &str| value_of("man_made").is_some_and(|value| value.eq_ignore_ascii_case(want));

    let embankment = if let Some(value) = value_of("embankment").filter(|value| tag_value_is_true(value)) {
        Some(hachure_side_from_value(value, HachureSide::Right))
    } else if man_made_is("embankment") {
        Some(HachureSide::Right)
    } else {
        None
    };
    let cutting = if let Some(value) = value_of("cutting").filter(|value| tag_value_is_true(value)) {
        Some(hachure_side_from_value(value, HachureSide::Left))
    } else if man_made_is("cutting") {
        Some(HachureSide::Left)
    } else {
        None
    };

    StructureFacts {
        bridge: truthy("bridge"),
        tunnel: truthy("tunnel"),
        embankment,
        cutting,
        ford: truthy("ford") || value_of("highway").is_some_and(|value| value.eq_ignore_ascii_case("ford")),
    }
}

/// 样式表。
#[derive(Clone, Debug)]
pub struct StyleSheet {
    /// 内置调色板。
    pub colors: Colors,
    /// tag key 符号。
    pub keys: StyleKeys,
    /// 道路线宽（屏幕像素）。
    pub way_width_px: f32,
    /// 节点图标半径（屏幕像素）。
    pub node_radius_px: f32,
    /// 小于该屏幕长度的 way 在低 zoom 时被 LOD 跳过。
    pub min_way_length_px: f32,
    /// 可选 MapCSS 样式表（编译为 Sym 条件）。
    pub mapcss: Option<CompiledMapCss>,
}

impl StyleSheet {
    /// 构造。
    pub fn new(colors: Colors, keys: StyleKeys) -> Self {
        Self {
            colors,
            keys,
            way_width_px: 2.0,
            node_radius_px: 3.0,
            min_way_length_px: 3.0,
            mapcss: None,
        }
    }

    /// 安装编译后的 MapCSS。
    pub fn set_mapcss(&mut self, mapcss: CompiledMapCss) {
        self.mapcss = Some(mapcss);
    }

    fn tag(tags: &[(Sym, Sym)], key: Sym) -> Option<Sym> {
        tags.iter().find_map(|&(k, v)| (k == key).then_some(v))
    }

    /// way 的线颜色。
    pub fn way_color(&self, tags: &[(Sym, Sym)]) -> [u8; 4] {
        if Self::tag(tags, self.keys.highway).is_some() {
            self.colors.road
        } else if Self::tag(tags, self.keys.railway).is_some() {
            self.colors.rail
        } else {
            self.colors.line_default
        }
    }

    /// 线宽倍数（主干道更宽）。
    pub fn way_width_scale(&self, tags: &[(Sym, Sym)]) -> f32 {
        if Self::tag(tags, self.keys.highway).is_some() {
            // highway 的具体等级在 MapCSS 阶段细化；这里只区分有无。
            1.0
        } else {
            0.7
        }
    }

    /// 如果该 way 应作为面填充，返回填充色（仅 Sym 快速路径；完整语义见 [`Self::area_fill`]）。
    pub fn fill_color(&self, tags: &[(Sym, Sym)]) -> Option<[u8; 4]> {
        if Self::tag(tags, self.keys.building).is_some() {
            return Some(self.colors.building);
        }
        None
    }

    /// 按已解析的字符串标签决定面填充色（preset geometry + area 语义）。
    pub fn area_fill(&self, tags: &[(&str, &str)]) -> Option<[u8; 4]> {
        if !tags_imply_area(tags) {
            return None;
        }
        if tags.iter().any(|(key, _)| *key == "building") {
            return Some(self.colors.building);
        }
        if tags.iter().any(|(key, value)| {
            (*key == "natural" && matches!(*value, "water" | "bay" | "strait" | "harbour"))
                || (*key == "waterway" && *value == "riverbank")
                || (*key == "landuse" && *value == "reservoir")
                || (*key == "water")
        }) {
            return Some(self.colors.water);
        }
        if tags
            .iter()
            .any(|(key, _)| matches!(*key, "landuse" | "leisure" | "natural" | "boundary"))
        {
            return Some(self.colors.green);
        }
        Some(self.colors.area)
    }

    /// 是否整条 way 应被 LOD 跳过（长度低于阈值）。
    pub fn lod_skip(&self, screen_length_px: f32) -> bool {
        screen_length_px < self.min_way_length_px
    }
}

/// 一个内置预设：一组可直接写入标签编辑器的 tag。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Preset {
    /// 稳定 id（i18n key 为 `preset.<id>`）。
    pub id: &'static str,
    /// 建议标签（key, value）。
    pub tags: &'static [(&'static str, &'static str)],
}

/// 阶段 4 的内置预设（v1，覆盖最常见的编辑对象）。
pub fn builtin_presets() -> &'static [Preset] {
    &[
        Preset {
            id: "building",
            tags: &[("building", "yes")],
        },
        Preset {
            id: "highway_residential",
            tags: &[("highway", "residential")],
        },
        Preset {
            id: "highway_primary",
            tags: &[("highway", "primary")],
        },
        Preset {
            id: "highway_footway",
            tags: &[("highway", "footway")],
        },
        Preset {
            id: "water",
            tags: &[("natural", "water")],
        },
        Preset {
            id: "park",
            tags: &[("leisure", "park")],
        },
        Preset {
            id: "amenity_bench",
            tags: &[("amenity", "bench")],
        },
        Preset {
            id: "amenity_restaurant",
            tags: &[("amenity", "restaurant")],
        },
    ]
}

/// 一条 way 的最终样式。
#[derive(Clone, Debug, PartialEq)]
pub struct WayStyle {
    /// 线色。
    pub color: [u8; 4],
    /// 面填充色（None 表示只画线）。
    pub fill_color: Option<[u8; 4]>,
    /// 线宽（屏幕像素）。
    pub width_px: f32,
    /// MapCSS `dashes`（屏幕像素；奇偶交替实/虚）。
    pub dashes: Option<Vec<f32>>,
    /// MapCSS `text`：标签键。
    pub text: Option<String>,
    /// 文字颜色。
    pub text_color: Option<[u8; 4]>,
    /// 字号（屏幕像素）。
    pub font_size_px: Option<f32>,
    /// 文字位置。
    pub text_position: Option<String>,
    /// MapCSS 图标路径（相对 assets）。
    pub icon_image: Option<String>,
    /// 图标宽度（屏幕像素）。
    pub icon_width_px: Option<f32>,
    /// 外描边（桥 / 隧道 / 过水路面）。
    pub casing: Option<CasingStyle>,
    /// 坡侧刻线（路堤 / 路堑）。
    pub hachure: Option<HachureStyle>,
    /// 图层偏置：桥 +1、隧道 -1、其它 0；场景层据此决定叠放顺序。
    pub layer_bias: i16,
}

/// 桥/隧道/过水路面外描边比本体每侧宽出的屏幕像素。
///
/// 轮廓总宽 = 本体线宽 + 2 × 该值；取 2.5 px 是为了在深色底图上仍能读出 2–3 px 的边。
pub const STRUCTURE_CASING_HALO_PX: f32 = 2.5;

impl StyleSheet {
    /// 内置外描边：优先级 bridge > tunnel > ford。
    ///
    /// `width_px` 为该 way 本体的线宽，外描边在其基础上加宽：每侧 `STRUCTURE_CASING_HALO_PX`，
    /// 使轮廓在深色底图上能看出 2–3 像素的边。
    fn builtin_casing(&self, facts: &StructureFacts, width_px: f32) -> Option<CasingStyle> {
        let with_halo = width_px + STRUCTURE_CASING_HALO_PX * 2.0;
        if facts.bridge {
            Some(CasingStyle {
                color: self.colors.bridge_casing,
                width_px: with_halo,
                dashes: None,
            })
        } else if facts.tunnel {
            Some(CasingStyle {
                color: self.colors.tunnel_casing,
                width_px: with_halo,
                dashes: Some(vec![8.0, 5.0]),
            })
        } else if facts.ford {
            Some(CasingStyle {
                color: self.colors.ford_water,
                width_px: with_halo,
                dashes: None,
            })
        } else {
            None
        }
    }

    /// 内置坡侧刻线：路堤优先于路堑。
    ///
    /// 两者用不同符号 + 不同色相，避免只靠刻线朝向区分：
    /// * 路堤：刻线长而疏（7 / 12 px），外端再画坡脚线 → 近看是“梯状坡面”；
    /// * 路堑：刻线短而密（4 / 6 px），没有外缘线 → 近看是“梳状齿线”。
    fn builtin_hachure(&self, facts: &StructureFacts) -> Option<HachureStyle> {
        if let Some(side) = facts.embankment {
            return Some(HachureStyle {
                color: self.colors.embankment,
                spacing_px: 12.0,
                length_px: 7.0,
                width_px: 1.2,
                side,
                baseline: true,
            });
        }
        let side = facts.cutting?;
        Some(HachureStyle {
            color: self.colors.cutting,
            spacing_px: 6.0,
            length_px: 4.0,
            width_px: 1.0,
            side,
            baseline: false,
        })
    }

    /// 图层偏置：桥 +1、隧道 -1、其它 0。
    fn layer_bias(facts: &StructureFacts) -> i16 {
        if facts.bridge {
            1
        } else if facts.tunnel {
            -1
        } else {
            0
        }
    }

    /// 解析最终外描边：MapCSS 显式声明 `casing-*` 时替换内置，否则用内置。
    fn resolve_casing(
        &self,
        facts: &StructureFacts,
        width_px: f32,
        decl: Option<&mapcss::Declarations>,
    ) -> Option<CasingStyle> {
        if let Some(decl) = decl
            && (decl.casing_color.is_some() || decl.casing_width.is_some() || decl.casing_dashes.is_some())
        {
            return Some(CasingStyle {
                color: decl.casing_color.unwrap_or(self.colors.bridge_casing),
                width_px: decl
                    .casing_width
                    .unwrap_or(width_px + STRUCTURE_CASING_HALO_PX * 2.0),
                dashes: decl.casing_dashes.clone(),
            });
        }
        self.builtin_casing(facts, width_px)
    }

    /// 计算 way 样式：MapCSS 命中则使用其声明，否则回退内置调色板。
    ///
    /// `names` 为已解析的 `(key, value)` 字符串对；用于判断 area preset / `area=*` 语义。
    pub fn way_style(&self, tags: &[(Sym, Sym)], zoom: f32, names: &[(&str, &str)]) -> WayStyle {
        let fallback_fill = || self.fill_color(tags).or_else(|| self.area_fill(names));
        // 结构物（桥/隧道/路堤/路堑/过水路面）由字符串标签判定：Sym 无法还原字符串。
        let facts = structure_facts(names);
        if let Some(mapcss) = &self.mapcss
            && let Some(decl) = mapcss.style(Element::Way, tags, zoom)
        {
            let opacity = decl.opacity.unwrap_or(1.0).clamp(0.0, 1.0);
            let with_alpha = |color: [u8; 4]| {
                [
                    color[0],
                    color[1],
                    color[2],
                    (f32::from(color[3]) * opacity) as u8,
                ]
            };
            let width_px = decl
                .width
                .unwrap_or_else(|| self.way_width_px * self.way_width_scale(tags));
            return WayStyle {
                color: with_alpha(decl.color.unwrap_or_else(|| self.way_color(tags))),
                // 命中规则但没有 fill-color 时仍回退内置填充（否则一条通用
                // way 规则会让所有面失去颜色）。`fill-color: none` 则保持透明。
                fill_color: if decl.clear_fill {
                    None
                } else {
                    decl.fill_color
                        .map(with_alpha)
                        .or_else(|| fallback_fill().map(with_alpha))
                },
                width_px,
                dashes: decl.dashes.clone(),
                text: decl.text.clone(),
                text_color: decl.text_color.map(with_alpha),
                font_size_px: decl.font_size,
                text_position: decl.text_position.clone(),
                icon_image: decl.icon_image.clone(),
                icon_width_px: decl.icon_width,
                casing: self.resolve_casing(&facts, width_px, Some(&decl)),
                hachure: self.builtin_hachure(&facts),
                layer_bias: Self::layer_bias(&facts),
            };
        }
        let width_px = self.way_width_px * self.way_width_scale(tags);
        WayStyle {
            color: self.way_color(tags),
            fill_color: fallback_fill(),
            width_px,
            dashes: None,
            text: None,
            text_color: None,
            font_size_px: None,
            text_position: None,
            icon_image: None,
            icon_width_px: None,
            casing: self.builtin_casing(&facts, width_px),
            hachure: self.builtin_hachure(&facts),
            layer_bias: Self::layer_bias(&facts),
        }
    }

    /// 计算节点样式。未命中 MapCSS 时用调色板里的节点色。
    pub fn node_style(&self, tags: &[(Sym, Sym)], zoom: f32) -> NodeStyle {
        if let Some(mapcss) = &self.mapcss
            && let Some(decl) = mapcss.style(Element::Node, tags, zoom)
        {
            return NodeStyle {
                color: decl.color.unwrap_or(self.colors.node),
                text_color: decl.text_color,
                font_size_px: decl.font_size,
                icon_image: decl.icon_image.clone(),
                icon_width_px: decl.icon_width,
            };
        }
        NodeStyle {
            color: self.colors.node,
            text_color: None,
            font_size_px: None,
            icon_image: None,
            icon_width_px: None,
        }
    }
}

/// 闭合 way 的标签是否应按面渲染（对齐 iD：`area=*`、areaKeys、再看预设）。
///
/// - `area=no` → 否
/// - `area=*`（其它值）→ 是
/// - 常见 areaKeys（`building`/`landuse`/`leisure`/…）→ 是
/// - 已有 **line** 预设可匹配 → 否（闭合道路仍是线；避免 `highway/service_area`
///   因更高 `matchScore` 压过 `highway/service`）
/// - 否则若仅 area 预设匹配 → 是
pub fn tags_imply_area<K: AsRef<str>, V: AsRef<str>>(tags: &[(K, V)]) -> bool {
    let value_of = |key: &str| -> Option<&str> {
        tags.iter()
            .find(|(k, _)| k.as_ref() == key)
            .map(|(_, v)| v.as_ref())
    };

    match value_of("area") {
        Some("no") => return false,
        Some(_) => return true,
        None => {}
    }

    if area_keys_imply(tags) {
        return true;
    }

    let owned: Vec<(String, String)> = tags
        .iter()
        .map(|(k, v)| (k.as_ref().to_owned(), v.as_ref().to_owned()))
        .collect();
    if best_preset_for_tags(&owned, PresetGeometryFilter::OpenWay).is_some() {
        return false;
    }
    best_preset_for_tags(&owned, PresetGeometryFilter::ClosedWay)
        .is_some_and(|preset| preset.geometry.iter().any(|item| item == "area"))
}

fn area_keys_imply<K: AsRef<str>, V: AsRef<str>>(tags: &[(K, V)]) -> bool {
    for (key, value) in tags {
        let key = key.as_ref();
        let value = value.as_ref();
        match key {
            "building" | "landuse" | "leisure" | "amenity" | "shop" | "tourism" | "historic" | "military"
            | "office" | "craft" | "place" | "boundary" | "cemetery" | "aeroway" => {
                if key == "aeroway" && matches!(value, "taxiway" | "runway") {
                    continue;
                }
                if key == "leisure" && matches!(value, "track" | "slipway") {
                    continue;
                }
                if key == "amenity" && matches!(value, "bench" | "waste_basket" | "post_box" | "telephone") {
                    continue;
                }
                return true;
            }
            "natural" => {
                // 线性自然要素：闭合也不当面积。
                if matches!(
                    value,
                    "coastline" | "cliff" | "tree_row" | "ridge" | "arete" | "valley" | "gorge"
                ) {
                    continue;
                }
                return true;
            }
            "man_made" => {
                if matches!(value, "cutline" | "pipeline" | "embankment") {
                    continue;
                }
                return true;
            }
            "waterway" if value == "riverbank" => return true,
            _ => {}
        }
    }
    false
}

/// 一个节点的最终样式。
#[derive(Clone, Debug, PartialEq)]
pub struct NodeStyle {
    /// 点的颜色。
    pub color: [u8; 4],
    /// 文字颜色。
    pub text_color: Option<[u8; 4]>,
    /// 字号（屏幕像素）。
    pub font_size_px: Option<f32>,
    /// 图标。`poi:` 开头的由界面直接画，其余当作 assets 路径。
    pub icon_image: Option<String>,
    /// 图标宽度（屏幕像素）。
    pub icon_width_px: Option<f32>,
}

#[cfg(test)]
mod area_tests {
    use super::*;

    #[test]
    fn area_yes_forces_area() {
        assert!(tags_imply_area(&[("highway", "footway"), ("area", "yes")]));
    }

    #[test]
    fn area_no_blocks_area() {
        assert!(!tags_imply_area(&[("building", "yes"), ("area", "no")]));
    }

    #[test]
    fn parking_preset_is_area() {
        assert!(tags_imply_area(&[("amenity", "parking")]));
    }

    #[test]
    fn highway_residential_is_not_area() {
        assert!(!tags_imply_area(&[("highway", "residential")]));
    }

    #[test]
    fn closed_service_road_is_not_area_without_area_yes() {
        // schema 里 service_area 的 matchScore 高于 service；不能因闭合就认成面。
        assert!(!tags_imply_area(&[("highway", "service"), ("access", "private")]));
        assert!(tags_imply_area(&[("highway", "service"), ("area", "yes")]));
    }

    #[test]
    fn building_and_landuse_are_areas() {
        assert!(tags_imply_area(&[("building", "yes")]));
        assert!(tags_imply_area(&[("landuse", "residential")]));
    }

    #[test]
    fn coastline_is_not_area() {
        assert!(!tags_imply_area(&[("natural", "coastline")]));
    }

    #[test]
    fn area_fill_picks_palette() {
        let style = StyleSheet::new(
            Colors::default(),
            StyleKeys {
                highway: Sym::from_raw(1),
                building: Sym::from_raw(2),
                waterway: Sym::from_raw(3),
                natural: Sym::from_raw(4),
                railway: Sym::from_raw(5),
                area: Sym::from_raw(6),
            },
        );
        assert_eq!(
            style.area_fill(&[("building", "yes")]),
            Some(style.colors.building)
        );
        assert_eq!(
            style.area_fill(&[("amenity", "parking")]),
            Some(style.colors.area)
        );
        assert_eq!(style.area_fill(&[("highway", "primary")]), None);
    }
}

#[cfg(test)]
mod structure_tests {
    use super::*;

    fn test_keys() -> StyleKeys {
        StyleKeys {
            highway: Sym::from_raw(1),
            building: Sym::from_raw(2),
            waterway: Sym::from_raw(3),
            natural: Sym::from_raw(4),
            railway: Sym::from_raw(5),
            area: Sym::from_raw(6),
        }
    }

    fn sheet() -> StyleSheet {
        StyleSheet::new(Colors::default(), test_keys())
    }

    /// 测试用稳定 intern：同一字符串得到同一 Sym，不依赖真实符号表。
    fn intern(text: &str) -> Sym {
        let mut hash: u32 = 2_166_136_261;
        for byte in text.bytes() {
            hash ^= u32::from(byte);
            hash = hash.wrapping_mul(16_777_619);
        }
        Sym::from_raw(hash)
    }

    fn sheet_with_css(css: &str) -> StyleSheet {
        let mut style_sheet = sheet();
        let compiled = MapCss::parse(css).expect("MapCSS 应能解析").compile(intern);
        style_sheet.set_mapcss(compiled);
        style_sheet
    }

    /// `highway=residential` 的 Sym 标签。
    fn highway_tags() -> Vec<(Sym, Sym)> {
        vec![(Sym::from_raw(1), Sym::from_raw(100))]
    }

    fn tags_with(key: &str, value: &str) -> Vec<(Sym, Sym)> {
        let mut tags = highway_tags();
        tags.push((intern(key), intern(value)));
        tags
    }

    /// 与 `tags_with` 配对的字符串标签。
    fn names_with(key: &'static str, value: &'static str) -> Vec<(&'static str, &'static str)> {
        vec![("highway", "residential"), (key, value)]
    }

    #[test]
    fn facts_bridge_and_tunnel() {
        assert!(structure_facts(&[("bridge", "yes")]).bridge);
        assert!(structure_facts(&[("bridge", "viaduct")]).bridge);
        assert!(!structure_facts(&[("bridge", "no")]).bridge);
        assert!(!structure_facts(&[("bridge", "false")]).bridge);
        assert!(structure_facts(&[("tunnel", "building_passage")]).tunnel);
        assert!(!structure_facts(&[("tunnel", "0")]).tunnel);
        // 真值判断大小写不敏感。
        assert!(structure_facts(&[("bridge", "YES")]).bridge);
    }

    #[test]
    fn facts_embankment_and_cutting_sides() {
        assert_eq!(
            structure_facts(&[("man_made", "embankment")]).embankment,
            Some(HachureSide::Right)
        );
        assert_eq!(
            structure_facts(&[("embankment", "left")]).embankment,
            Some(HachureSide::Left)
        );
        assert_eq!(
            structure_facts(&[("embankment", "right")]).embankment,
            Some(HachureSide::Right)
        );
        assert_eq!(
            structure_facts(&[("embankment", "both")]).embankment,
            Some(HachureSide::Both)
        );
        assert_eq!(
            structure_facts(&[("man_made", "cutting")]).cutting,
            Some(HachureSide::Left)
        );
        assert_eq!(
            structure_facts(&[("cutting", "right")]).cutting,
            Some(HachureSide::Right)
        );
        assert_eq!(structure_facts(&[("man_made", "cutting")]).embankment, None);
        assert_eq!(structure_facts(&[("embankment", "no")]).embankment, None);
    }

    #[test]
    fn facts_ford() {
        assert!(structure_facts(&[("ford", "yes")]).ford);
        assert!(structure_facts(&[("highway", "ford")]).ford);
        assert!(!structure_facts(&[("ford", "no")]).ford);
    }

    #[test]
    fn way_style_bridge_has_casing_and_layer_bias() {
        let sheet = sheet();
        let style = sheet.way_style(&tags_with("bridge", "yes"), 16.0, &names_with("bridge", "yes"));
        let width = style.width_px;
        let casing = style.casing.expect("桥梁应有外描边");
        assert_eq!(casing.color, sheet.colors.bridge_casing);
        assert!((casing.width_px - (width + STRUCTURE_CASING_HALO_PX * 2.0)).abs() < 1e-6);
        assert!(casing.dashes.is_none());
        assert_eq!(style.layer_bias, 1);
        assert!(style.hachure.is_none());
    }

    #[test]
    fn way_style_tunnel_casing_is_dashed() {
        let sheet = sheet();
        let style = sheet.way_style(
            &tags_with("tunnel", "building_passage"),
            16.0,
            &names_with("tunnel", "building_passage"),
        );
        let casing = style.casing.expect("隧道应有外描边");
        assert_eq!(casing.color, sheet.colors.tunnel_casing);
        assert_eq!(casing.dashes, Some(vec![8.0, 5.0]));
        assert_eq!(style.layer_bias, -1);
    }

    #[test]
    fn way_style_bridge_beats_tunnel() {
        let mut tags = tags_with("bridge", "yes");
        tags.push((intern("tunnel"), intern("yes")));
        let style = sheet().way_style(
            &tags,
            16.0,
            &[("highway", "residential"), ("bridge", "yes"), ("tunnel", "yes")],
        );
        assert_eq!(
            style.casing.expect("应有外描边").color,
            Colors::default().bridge_casing
        );
        assert_eq!(style.layer_bias, 1);
    }

    #[test]
    fn way_style_ford_casing_is_water() {
        let sheet = sheet();
        let style = sheet.way_style(&tags_with("ford", "yes"), 16.0, &names_with("ford", "yes"));
        let width = style.width_px;
        let casing = style.casing.expect("过水路面应有水色外描边");
        assert_eq!(casing.color, sheet.colors.ford_water);
        assert!((casing.width_px - (width + STRUCTURE_CASING_HALO_PX * 2.0)).abs() < 1e-6);
        assert_eq!(style.layer_bias, 0);
    }

    #[test]
    fn way_style_embankment_and_cutting_have_distinct_symbols() {
        let sheet = sheet();
        let style = sheet.way_style(
            &tags_with("embankment", "left"),
            16.0,
            &names_with("embankment", "left"),
        );
        let embankment = style.hachure.expect("路堤应有坡侧刻线");
        assert_eq!(embankment.side, HachureSide::Left);
        assert_eq!(embankment.color, sheet.colors.embankment);
        assert_eq!(embankment.spacing_px, 12.0);
        assert_eq!(embankment.length_px, 7.0);
        assert_eq!(embankment.width_px, 1.2);
        // 路堤带外缘坡脚线（梯状坡面）。
        assert!(embankment.baseline);
        assert!(style.casing.is_none());

        // 路堑：默认左侧、短而密、无外缘线（梳状齿线）、颜色不同。
        let cutting = sheet.way_style(
            &tags_with("man_made", "cutting"),
            16.0,
            &names_with("man_made", "cutting"),
        );
        let cutting = cutting.hachure.expect("路堑应有坡侧刻线");
        assert_eq!(cutting.side, HachureSide::Left);
        assert_eq!(cutting.color, sheet.colors.cutting);
        assert_eq!(cutting.spacing_px, 6.0);
        assert_eq!(cutting.length_px, 4.0);
        assert!(!cutting.baseline);
        // 两种符号必须在形状和颜色上都不同，不能只靠朝向。
        assert_ne!(embankment.color, cutting.color);
        assert_ne!(embankment.baseline, cutting.baseline);
        assert_ne!(embankment.length_px, cutting.length_px);

        // 路堤优先于路堑。
        let mut names = names_with("embankment", "both");
        names.push(("cutting", "left"));
        let both = sheet.way_style(&tags_with("embankment", "both"), 16.0, &names);
        assert_eq!(both.hachure.expect("路堤应有坡侧刻线").side, HachureSide::Both);
    }

    #[test]
    fn way_style_plain_road_has_no_structure_style() {
        let style = sheet().way_style(&highway_tags(), 16.0, &[("highway", "residential")]);
        assert!(style.casing.is_none());
        assert!(style.hachure.is_none());
        assert_eq!(style.layer_bias, 0);
    }

    #[test]
    fn mapcss_casing_overrides_builtin() {
        let sheet = sheet_with_css("way[bridge] { casing-width: 6; casing-color: #112233; }");
        let style = sheet.way_style(&tags_with("bridge", "yes"), 16.0, &names_with("bridge", "yes"));
        let casing = style.casing.expect("MapCSS 应覆盖出外描边");
        assert_eq!(casing.color, [0x11, 0x22, 0x33, 255]);
        assert_eq!(casing.width_px, 6.0);
        assert!(casing.dashes.is_none());
    }

    #[test]
    fn mapcss_casing_dashes_only_keeps_defaults() {
        let sheet = sheet_with_css("way[bridge] { casing-dashes: 4, 2; }");
        let style = sheet.way_style(&tags_with("bridge", "yes"), 16.0, &names_with("bridge", "yes"));
        let width = style.width_px;
        let casing = style.casing.expect("应命中 MapCSS casing 声明");
        assert_eq!(casing.dashes, Some(vec![4.0, 2.0]));
        // 颜色缺省用 bridge_casing，宽度缺省用 width + 2 × 每侧留边。
        assert_eq!(casing.color, sheet.colors.bridge_casing);
        assert!((casing.width_px - (width + STRUCTURE_CASING_HALO_PX * 2.0)).abs() < 1e-6);
    }

    #[test]
    fn mapcss_without_casing_keeps_builtin() {
        let sheet = sheet_with_css("way { width: 5; }");
        let style = sheet.way_style(&tags_with("bridge", "yes"), 16.0, &names_with("bridge", "yes"));
        assert_eq!(style.width_px, 5.0);
        let casing = style.casing.expect("未声明 casing-* 时应保留内置外描边");
        assert_eq!(casing.color, sheet.colors.bridge_casing);
        // 内置外描边宽度基于该 way 的最终 width_px。
        assert!((casing.width_px - (5.0 + STRUCTURE_CASING_HALO_PX * 2.0)).abs() < 1e-6);
    }
}
