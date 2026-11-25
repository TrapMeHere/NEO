//! # neo-style — styles, presets and filters
//!
//! This phase implements the minimal closed loop of "built-in color rules" (the MapCSS subset parser lands in milestone 4):
//!
//! * the stylesheet does not depend on `neo-store` / GPU / UI, only on symbol types from `neo-core`;
//! * Syms are interned beforehand by the caller (`neo-scene`) and injected, avoiding a reverse dependency from the style layer onto the data layer;
//! * colors are 4 bytes and go straight into vertices.
//!
//! Design notes (docs/11): MapCSS is JOSM's de facto styling language, and compatibility is a prerequisite for "complete replacement";
//! but phase one first gets the render pipeline working with hand-written rules, with the MapCSS parser as a separate module added later.

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

/// Tag keys used by styles (interned and injected by the caller).
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

/// Palette (RGBA).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Colors {
    /// Map background.
    pub background: [u8; 4],
    /// Ordinary roads.
    pub road: [u8; 4],
    /// Trunk roads.
    pub road_major: [u8; 4],
    /// Railways.
    pub rail: [u8; 4],
    /// Building fill.
    pub building: [u8; 4],
    /// Water fill.
    pub water: [u8; 4],
    /// Green-space fill.
    pub green: [u8; 4],
    /// Generic area fill (fallback color for area presets like amenity/shop).
    pub area: [u8; 4],
    /// Default line (unknown way).
    pub line_default: [u8; 4],
    /// Node icon.
    pub node: [u8; 4],
    /// Selected objects.
    pub selection: [u8; 4],
    /// Bridge casing.
    pub bridge_casing: [u8; 4],
    /// Tunnel casing.
    pub tunnel_casing: [u8; 4],
    /// Water-colored casing of fords.
    pub ford_water: [u8; 4],
    /// Color of embankment side hachures (terraced slope).
    pub embankment: [u8; 4],
    /// Color of cutting side hachures (comb-like teeth).
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

/// Way casing: the scene layer draws it widened beneath the body.
#[derive(Clone, Debug, PartialEq)]
pub struct CasingStyle {
    /// Casing color.
    pub color: [u8; 4],
    /// Casing width (screen pixels).
    pub width_px: f32,
    /// Dash pattern (screen pixels); `None` means solid.
    pub dashes: Option<Vec<f32>>,
}

/// Slope-side hachure direction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HachureSide {
    /// Left of the point order.
    Left,
    /// Right of the point order.
    Right,
    /// Both sides.
    Both,
}

/// Hachure style for embankment / cutting slopes.
///
/// Embankments and cuttings are distinguished by **symbol shape + color** (hachure direction alone is invisible at small scales):
/// an embankment is a "terraced slope" with an outer slope-foot line, a cutting is "comb-like teeth" without an outer line.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HachureStyle {
    /// Hachure color.
    pub color: [u8; 4],
    /// Interval along the way (screen pixels).
    pub spacing_px: f32,
    /// Length of a single hachure (screen pixels).
    pub length_px: f32,
    /// Width of a single hachure (screen pixels); the outer slope-foot line uses this width too.
    pub width_px: f32,
    /// Hachure direction.
    pub side: HachureSide,
    /// Whether to draw a parallel slope-foot line at the outer end of the hachures (the embankment's "terraced" symbol).
    pub baseline: bool,
}

/// Structure facts parsed from tags (bridge, tunnel, embankment, cutting, ford).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StructureFacts {
    /// `bridge=*` (truthy).
    pub bridge: bool,
    /// `tunnel=*` (truthy).
    pub tunnel: bool,
    /// Embankment and its slope side.
    pub embankment: Option<HachureSide>,
    /// Cutting and its slope side.
    pub cutting: Option<HachureSide>,
    /// `ford=*` or `highway=ford`.
    pub ford: bool,
}

/// Whether a tag value is truthy: non-empty and not `no` / `false` / `0` (case-insensitive).
fn tag_value_is_true(value: &str) -> bool {
    let value = value.trim();
    !value.is_empty()
        && !value.eq_ignore_ascii_case("no")
        && !value.eq_ignore_ascii_case("false")
        && value != "0"
}

/// Parse the slope side from `embankment` / `cutting` values, falling back to `default` when unrecognized.
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

/// Parse structure facts from string tags (key names and truthiness checks are case-insensitive).
///
/// - a truthy `bridge` / `tunnel` key means true (any value such as `viaduct`, `building_passage` counts);
/// - `man_made=embankment` or a truthy `embankment` key → embankment, slope side defaults to right;
/// - `man_made=cutting` or a truthy `cutting` key → cutting, slope side defaults to left;
/// - a truthy `ford` key, or `highway=ford` → ford.
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

/// Stylesheet.
#[derive(Clone, Debug)]
pub struct StyleSheet {
    /// Built-in palette.
    pub colors: Colors,
    /// Tag key symbols.
    pub keys: StyleKeys,
    /// Road line width (screen pixels).
    pub way_width_px: f32,
    /// Node icon radius (screen pixels).
    pub node_radius_px: f32,
    /// Ways shorter than this on screen are skipped by LOD at low zoom.
    pub min_way_length_px: f32,
    /// Optional MapCSS stylesheet (compiled to Sym conditions).
    pub mapcss: Option<CompiledMapCss>,
}

impl StyleSheet {
    /// Construct.
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

    /// Install the compiled MapCSS.
    pub fn set_mapcss(&mut self, mapcss: CompiledMapCss) {
        self.mapcss = Some(mapcss);
    }

    fn tag(tags: &[(Sym, Sym)], key: Sym) -> Option<Sym> {
        tags.iter().find_map(|&(k, v)| (k == key).then_some(v))
    }

    /// Line color of a way.
    pub fn way_color(&self, tags: &[(Sym, Sym)]) -> [u8; 4] {
        if Self::tag(tags, self.keys.highway).is_some() {
            self.colors.road
        } else if Self::tag(tags, self.keys.railway).is_some() {
            self.colors.rail
        } else {
            self.colors.line_default
        }
    }

    /// Line-width multiplier (trunk roads are wider).
    pub fn way_width_scale(&self, tags: &[(Sym, Sym)]) -> f32 {
        if Self::tag(tags, self.keys.highway).is_some() {
            // The concrete highway class is refined in the MapCSS phase; here only presence/absence is distinguished.
            1.0
        } else {
            0.7
        }
    }

    /// If the way should be filled as an area, return the fill color (Sym fast path only; full semantics in [`Self::area_fill`]).
    pub fn fill_color(&self, tags: &[(Sym, Sym)]) -> Option<[u8; 4]> {
        if Self::tag(tags, self.keys.building).is_some() {
            return Some(self.colors.building);
        }
        None
    }

    /// Decide the area fill color from parsed string tags (preset geometry + area semantics).
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

    /// Whether the whole way should be skipped by LOD (length below the threshold).
    pub fn lod_skip(&self, screen_length_px: f32) -> bool {
        screen_length_px < self.min_way_length_px
    }
}

/// A built-in preset: a set of tags that can be written straight into the tag editor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Preset {
    /// Stable id (i18n key is `preset.<id>`).
    pub id: &'static str,
    /// Suggested tags (key, value).
    pub tags: &'static [(&'static str, &'static str)],
}

/// Phase 4 built-in presets (v1, covering the most common editing objects).
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

/// Final style of one way.
#[derive(Clone, Debug, PartialEq)]
pub struct WayStyle {
    /// Line color.
    pub color: [u8; 4],
    /// Area fill color (None means draw lines only).
    pub fill_color: Option<[u8; 4]>,
    /// Line width (screen pixels).
    pub width_px: f32,
    /// MapCSS `dashes` (screen pixels; alternating solid/dash).
    pub dashes: Option<Vec<f32>>,
    /// MapCSS `text`: label key.
    pub text: Option<String>,
    /// Text color.
    pub text_color: Option<[u8; 4]>,
    /// Font size (screen pixels).
    pub font_size_px: Option<f32>,
    /// Text position.
    pub text_position: Option<String>,
    /// MapCSS icon path (relative to assets).
    pub icon_image: Option<String>,
    /// Icon width (screen pixels).
    pub icon_width_px: Option<f32>,
    /// Casing (bridge / tunnel / ford).
    pub casing: Option<CasingStyle>,
    /// Slope-side hachures (embankment / cutting).
    pub hachure: Option<HachureStyle>,
    /// Layer bias: bridge +1, tunnel -1, others 0; the scene layer uses it to decide stacking order.
    pub layer_bias: i16,
}

/// Screen pixels by which the bridge/tunnel/ford casing is wider than the body on each side.
///
/// Total outline width = body line width + 2 × this value; 2.5 px is chosen so a 2–3 px edge remains legible on dark basemaps.
pub const STRUCTURE_CASING_HALO_PX: f32 = 2.5;

impl StyleSheet {
    /// Built-in casing: priority bridge > tunnel > ford.
    ///
    /// `width_px` is the body line width of the way; the casing widens it by `STRUCTURE_CASING_HALO_PX` on each side,
    /// so the outline shows a 2–3 pixel edge on dark basemaps.
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

    /// Built-in slope-side hachures: embankment takes priority over cutting.
    ///
    /// The two use different symbols + different hues, so they are not distinguished by hachure direction alone:
    /// * embankment: long, sparse hachures (7 / 12 px) plus an outer slope-foot line → looks like a "terraced slope" up close;
    /// * cutting: short, dense hachures (4 / 6 px) with no outer line → looks like "comb-like teeth" up close.
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

    /// Layer bias: bridge +1, tunnel -1, others 0.
    fn layer_bias(facts: &StructureFacts) -> i16 {
        if facts.bridge {
            1
        } else if facts.tunnel {
            -1
        } else {
            0
        }
    }

    /// Resolve the final casing: an explicit MapCSS `casing-*` declaration replaces the built-in one, otherwise use the built-in one.
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

    /// Compute the way style: use MapCSS declarations on a hit, otherwise fall back to the built-in palette.
    ///
    /// `names` are parsed `(key, value)` string pairs; used to decide area preset / `area=*` semantics.
    pub fn way_style(&self, tags: &[(Sym, Sym)], zoom: f32, names: &[(&str, &str)]) -> WayStyle {
        let fallback_fill = || self.fill_color(tags).or_else(|| self.area_fill(names));
        // Structures (bridge/tunnel/embankment/cutting/ford) are determined from string tags: Syms cannot recover strings.
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
                // When a rule matches but has no fill-color, still fall back to the built-in fill (otherwise a generic
                // way rule would remove colors from every area). `fill-color: none` keeps it transparent.
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

    /// Compute the node style. Uses the palette node color when MapCSS does not match.
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

/// Whether a closed way's tags should render as an area (aligned with iD: `area=*`, areaKeys, then presets).
///
/// - `area=no` → no
/// - `area=*` (any other value) → yes
/// - common areaKeys (`building`/`landuse`/`leisure`/…) → yes
/// - an existing **line** preset matches → no (closed roads stay lines; avoids `highway/service_area`
///   overriding `highway/service` due to a higher `matchScore`)
/// - otherwise, if only an area preset matches → yes
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
                // Linear natural features: never treated as areas even when closed.
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

/// Final style of one node.
#[derive(Clone, Debug, PartialEq)]
pub struct NodeStyle {
    /// Point color.
    pub color: [u8; 4],
    /// Text color.
    pub text_color: Option<[u8; 4]>,
    /// Font size (screen pixels).
    pub font_size_px: Option<f32>,
    /// Icon. Ones starting with `poi:` are drawn directly by the UI; the rest are asset paths.
    pub icon_image: Option<String>,
    /// Icon width (screen pixels).
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
        // In the schema, service_area's matchScore is higher than service's; being closed must not make it an area.
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

    /// Stable interner for tests: the same string yields the same Sym, without depending on the real symbol table.
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
        let compiled = MapCss::parse(css).expect("MapCSS should parse").compile(intern);
        style_sheet.set_mapcss(compiled);
        style_sheet
    }

    /// Sym tags for `highway=residential`.
    fn highway_tags() -> Vec<(Sym, Sym)> {
        vec![(Sym::from_raw(1), Sym::from_raw(100))]
    }

    fn tags_with(key: &str, value: &str) -> Vec<(Sym, Sym)> {
        let mut tags = highway_tags();
        tags.push((intern(key), intern(value)));
        tags
    }

    /// String tags paired with `tags_with`.
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
        // Truthiness checks are case-insensitive.
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
        let casing = style.casing.expect("bridge should have a casing");
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
        let casing = style.casing.expect("tunnel should have a casing");
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
            style.casing.expect("should have a casing").color,
            Colors::default().bridge_casing
        );
        assert_eq!(style.layer_bias, 1);
    }

    #[test]
    fn way_style_ford_casing_is_water() {
        let sheet = sheet();
        let style = sheet.way_style(&tags_with("ford", "yes"), 16.0, &names_with("ford", "yes"));
        let width = style.width_px;
        let casing = style.casing.expect("ford should have a water-colored casing");
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
        let embankment = style.hachure.expect("embankment should have hachures");
        assert_eq!(embankment.side, HachureSide::Left);
        assert_eq!(embankment.color, sheet.colors.embankment);
        assert_eq!(embankment.spacing_px, 12.0);
        assert_eq!(embankment.length_px, 7.0);
        assert_eq!(embankment.width_px, 1.2);
        // Embankments have an outer slope-foot line (terraced slope).
        assert!(embankment.baseline);
        assert!(style.casing.is_none());

        // Cutting: defaults to the left side, short and dense, no outer line (comb-like teeth), different color.
        let cutting = sheet.way_style(
            &tags_with("man_made", "cutting"),
            16.0,
            &names_with("man_made", "cutting"),
        );
        let cutting = cutting.hachure.expect("cutting should have hachures");
        assert_eq!(cutting.side, HachureSide::Left);
        assert_eq!(cutting.color, sheet.colors.cutting);
        assert_eq!(cutting.spacing_px, 6.0);
        assert_eq!(cutting.length_px, 4.0);
        assert!(!cutting.baseline);
        // The two symbols must differ in both shape and color, not just direction.
        assert_ne!(embankment.color, cutting.color);
        assert_ne!(embankment.baseline, cutting.baseline);
        assert_ne!(embankment.length_px, cutting.length_px);

        // Embankment takes priority over cutting.
        let mut names = names_with("embankment", "both");
        names.push(("cutting", "left"));
        let both = sheet.way_style(&tags_with("embankment", "both"), 16.0, &names);
        assert_eq!(
            both.hachure.expect("embankment should have hachures").side,
            HachureSide::Both
        );
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
        let casing = style.casing.expect("MapCSS should override the casing");
        assert_eq!(casing.color, [0x11, 0x22, 0x33, 255]);
        assert_eq!(casing.width_px, 6.0);
        assert!(casing.dashes.is_none());
    }

    #[test]
    fn mapcss_casing_dashes_only_keeps_defaults() {
        let sheet = sheet_with_css("way[bridge] { casing-dashes: 4, 2; }");
        let style = sheet.way_style(&tags_with("bridge", "yes"), 16.0, &names_with("bridge", "yes"));
        let width = style.width_px;
        let casing = style.casing.expect("should match the MapCSS casing declaration");
        assert_eq!(casing.dashes, Some(vec![4.0, 2.0]));
        // Color defaults to bridge_casing, width defaults to width + 2 × per-side halo.
        assert_eq!(casing.color, sheet.colors.bridge_casing);
        assert!((casing.width_px - (width + STRUCTURE_CASING_HALO_PX * 2.0)).abs() < 1e-6);
    }

    #[test]
    fn mapcss_without_casing_keeps_builtin() {
        let sheet = sheet_with_css("way { width: 5; }");
        let style = sheet.way_style(&tags_with("bridge", "yes"), 16.0, &names_with("bridge", "yes"));
        assert_eq!(style.width_px, 5.0);
        let casing = style
            .casing
            .expect("built-in casing should be kept when casing-* is not declared");
        assert_eq!(casing.color, sheet.colors.bridge_casing);
        // The built-in casing width is based on the way's final width_px.
        assert!((casing.width_px - (5.0 + STRUCTURE_CASING_HALO_PX * 2.0)).abs() < 1e-6);
    }
}
