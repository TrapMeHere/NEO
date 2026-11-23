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
        }
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
}

impl StyleSheet {
    /// 计算 way 样式：MapCSS 命中则使用其声明，否则回退内置调色板。
    ///
    /// `names` 为已解析的 `(key, value)` 字符串对；用于判断 area preset / `area=*` 语义。
    pub fn way_style(&self, tags: &[(Sym, Sym)], zoom: f32, names: &[(&str, &str)]) -> WayStyle {
        let fallback_fill = || self.fill_color(tags).or_else(|| self.area_fill(names));
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
                width_px: decl
                    .width
                    .unwrap_or_else(|| self.way_width_px * self.way_width_scale(tags)),
                dashes: decl.dashes.clone(),
                text: decl.text.clone(),
                text_color: decl.text_color.map(with_alpha),
                font_size_px: decl.font_size,
                text_position: decl.text_position.clone(),
                icon_image: decl.icon_image.clone(),
                icon_width_px: decl.icon_width,
            };
        }
        WayStyle {
            color: self.way_color(tags),
            fill_color: fallback_fill(),
            width_px: self.way_width_px * self.way_width_scale(tags),
            dashes: None,
            text: None,
            text_color: None,
            font_size_px: None,
            text_position: None,
            icon_image: None,
            icon_width_px: None,
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
