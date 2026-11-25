//! Presets come from the two npm packages used by iD, **external binaries**, loaded at runtime, not compiled into the binary.
//!
//! * presets and Chinese/English translations from `@openstreetmap/id-tagging-schema` (ISC)
//! * `buildIDPresets` from `name-suggestion-index` (BSD-3-Clause)
//!
//! The generation script `scripts/build-presets.mjs` writes `assets/presets/schema.neo` and
//! `suggestions.neo`. The schema is loaded on the first preset query; brand suggestions wait until the first search.
//! See [`crate::assets`] for lookup paths.
//!
//! File layout (little-endian, matching the generation script):
//! `NEOP`, `u16` version 1, `u16` kind (1 schema / 2 suggestions),
//! index of the version number in the string table, the string table, then an index record for each preset.
//! Geometry is a bit flag: point=1, vertex=2, line=4, area=8, relation=16.
//! A Chinese-name index of `u32::MAX` means the same as the English name.
//!
//! Categories are grouped by the first path segment of the preset id (e.g. `highway` / `amenity`);
//! geometry filtering and tag matching are aligned with iD (`tags` + `matchScore`).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::OnceLock;

use crate::assets;

/// A preset that can be applied to an object.
#[derive(Clone, Debug, PartialEq)]
pub struct SchemaPreset {
    /// Preset id (e.g. `amenity/bench` or `amenity/cafe/starbucks-xxxxx`).
    pub id: String,
    /// Chinese name. Same as the English name when a brand name has no translation.
    pub name_zh: String,
    /// English name.
    pub name_en: String,
    /// Tags written when the preset is selected (from `addTags`, excluding `*`).
    pub tags: Vec<(String, String)>,
    /// Tags used to match existing features (from `tags`, may contain `*`).
    pub match_tags: Vec<(String, String)>,
    /// iD `matchScore`, default 1.0.
    pub match_score: f32,
    /// Keys removed before writing. iD uses this to clear stale `*:wikipedia`.
    pub remove_keys: Vec<String>,
    /// Applicable geometry (`point` / `vertex` / `line` / `area` / `relation`). Empty means unrestricted.
    pub geometry: Vec<String>,
    /// From name-suggestion-index. Appears only during search, not in the default list.
    pub suggestion: bool,
}

/// Preset category (first path segment of the id, e.g. `highway`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PresetCategory {
    /// Category id, e.g. `highway`.
    pub id: String,
    /// Chinese name.
    pub name_zh: String,
    /// English name.
    pub name_en: String,
    /// Number of schema presets in this category (excluding brand suggestions).
    pub count: usize,
}

/// iD geometry filter for the currently selected object.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PresetGeometryFilter {
    /// Node: `point` or `vertex`.
    Node,
    /// Unclosed way: `line`.
    OpenWay,
    /// Closed way: `line` or `area`.
    ClosedWay,
    /// relation: `relation` or `area` (multipolygon etc.).
    Relation,
}

impl PresetGeometryFilter {
    /// Whether the preset applies to the current filter.
    pub fn allows(self, preset: &SchemaPreset) -> bool {
        if preset.geometry.is_empty() {
            return true;
        }
        match self {
            Self::Node => preset
                .geometry
                .iter()
                .any(|item| item == "point" || item == "vertex"),
            Self::OpenWay => preset.geometry.iter().any(|item| item == "line"),
            Self::ClosedWay => preset
                .geometry
                .iter()
                .any(|item| item == "line" || item == "area"),
            Self::Relation => preset
                .geometry
                .iter()
                .any(|item| item == "relation" || item == "area"),
        }
    }
}

struct Catalog {
    schema_version: String,
    nsi_version: String,
    schema: Vec<SchemaPreset>,
    categories: Vec<PresetCategory>,
    /// Preset id → its category id.
    membership: HashMap<String, String>,
    /// `(key, value)` → schema preset index; a `value` of `*` has its own entry.
    match_index: HashMap<(String, String), Vec<usize>>,
}

struct SuggestionSet {
    rows: Vec<SchemaPreset>,
    match_index: HashMap<(String, String), Vec<usize>>,
}

static CATALOG: OnceLock<Catalog> = OnceLock::new();
static SUGGESTIONS: OnceLock<SuggestionSet> = OnceLock::new();

fn empty_catalog() -> Catalog {
    Catalog {
        schema_version: String::new(),
        nsi_version: String::new(),
        schema: Vec::new(),
        categories: Vec::new(),
        membership: HashMap::new(),
        match_index: HashMap::new(),
    }
}

fn group_id_of(preset_id: &str) -> &str {
    preset_id.split('/').next().unwrap_or(preset_id)
}

fn group_label(id: &str) -> (&'static str, &'static str) {
    match id {
        "advertising" => ("广告", "Advertising"),
        "aerialway" => ("索道", "Aerialway"),
        "aeroway" => ("航空", "Aeroway"),
        "amenity" => ("设施", "Amenity"),
        "attraction" => ("景点", "Attraction"),
        "barrier" => ("障碍", "Barrier"),
        "boundary" => ("边界", "Boundary"),
        "building" => ("建筑", "Building"),
        "craft" => ("工艺", "Craft"),
        "emergency" => ("应急", "Emergency"),
        "golf" => ("高尔夫", "Golf"),
        "healthcare" => ("医疗", "Healthcare"),
        "highway" => ("道路", "Highway"),
        "historic" => ("历史", "Historic"),
        "landuse" => ("土地利用", "Landuse"),
        "leisure" => ("休闲", "Leisure"),
        "man_made" => ("人造", "Man-made"),
        "natural" => ("自然", "Natural"),
        "office" => ("办公", "Office"),
        "place" => ("地点", "Place"),
        "playground" => ("游乐", "Playground"),
        "power" => ("电力", "Power"),
        "public_transport" => ("公交", "Public transport"),
        "railway" => ("铁路", "Railway"),
        "route" => ("线路", "Route"),
        "shop" => ("商店", "Shop"),
        "tourism" => ("旅游", "Tourism"),
        "type" => ("关系类型", "Relation type"),
        "waterway" => ("水路", "Waterway"),
        _ => ("", ""),
    }
}

fn build_categories(schema: &[SchemaPreset]) -> (Vec<PresetCategory>, HashMap<String, String>) {
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    let mut membership = HashMap::with_capacity(schema.len());
    for preset in schema {
        let group = group_id_of(&preset.id).to_owned();
        *counts.entry(group.clone()).or_insert(0) += 1;
        membership.insert(preset.id.clone(), group);
    }
    let mut categories = Vec::with_capacity(counts.len());
    for (id, count) in counts {
        let (zh, en) = group_label(&id);
        categories.push(PresetCategory {
            name_zh: if zh.is_empty() { id.clone() } else { zh.to_owned() },
            name_en: if en.is_empty() { id.clone() } else { en.to_owned() },
            id,
            count,
        });
    }
    const PINNED: &[&str] = &[
        "highway", "building", "amenity", "shop", "natural", "landuse", "leisure", "railway", "waterway",
        "barrier", "tourism", "office", "man_made",
    ];
    categories.sort_by(|a, b| {
        let ai = PINNED.iter().position(|id| *id == a.id).unwrap_or(usize::MAX);
        let bi = PINNED.iter().position(|id| *id == b.id).unwrap_or(usize::MAX);
        ai.cmp(&bi).then_with(|| a.id.cmp(&b.id))
    });
    (categories, membership)
}

fn build_match_index(presets: &[SchemaPreset]) -> HashMap<(String, String), Vec<usize>> {
    let mut index: HashMap<(String, String), Vec<usize>> = HashMap::new();
    for (i, preset) in presets.iter().enumerate() {
        for (key, value) in &preset.match_tags {
            index.entry((key.clone(), value.clone())).or_default().push(i);
        }
    }
    index
}

fn catalog() -> &'static Catalog {
    CATALOG.get_or_init(load_catalog)
}

fn ensure_suggestions() -> &'static SuggestionSet {
    SUGGESTIONS.get_or_init(load_suggestions)
}

const KIND_SCHEMA: u16 = 1;
const KIND_SUGGESTIONS: u16 = 2;
const SAME_NAME: u32 = u32::MAX;

struct DecodedFile {
    schema_version: String,
    nsi_version: String,
    presets: Vec<SchemaPreset>,
}

struct ByteCursor<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl ByteCursor<'_> {
    fn take(&mut self, n: usize) -> Option<&[u8]> {
        let end = self.at.checked_add(n)?;
        if end > self.bytes.len() {
            return None;
        }
        let start = self.at;
        self.at = end;
        Some(&self.bytes[start..end])
    }

    fn u8(&mut self) -> Option<u8> {
        self.take(1).map(|bytes| bytes[0])
    }

    fn u16(&mut self) -> Option<u16> {
        self.take(2).map(|bytes| u16::from_le_bytes([bytes[0], bytes[1]]))
    }

    fn u32(&mut self) -> Option<u32> {
        self.take(4)
            .map(|bytes| u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }
}

fn geometry_from_bits(bits: u8) -> Vec<String> {
    const NAMES: [(u8, &str); 5] = [
        (1, "point"),
        (2, "vertex"),
        (4, "line"),
        (8, "area"),
        (16, "relation"),
    ];
    NAMES
        .into_iter()
        .filter(|(bit, _)| bits & bit != 0)
        .map(|(_, name)| name.to_owned())
        .collect()
}

fn decode_presets(bytes: &[u8], expect_kind: u16, suggestion: bool) -> Option<DecodedFile> {
    let mut cur = ByteCursor { bytes, at: 0 };
    if cur.take(4)? != b"NEOP" {
        return None;
    }
    if cur.u16()? != 1 || cur.u16()? != expect_kind {
        return None;
    }
    let schema_idx = cur.u32()? as usize;
    let nsi_idx = cur.u32()? as usize;
    let string_count = cur.u32()? as usize;
    let blob_len = cur.u32()? as usize;
    let preset_count = cur.u32()? as usize;
    if string_count > 200_000 || preset_count > 200_000 {
        return None;
    }
    let mut offsets = Vec::with_capacity(string_count + 1);
    for _ in 0..=string_count {
        offsets.push(cur.u32()? as usize);
    }
    if offsets.first().copied() != Some(0) || offsets.last().copied() != Some(blob_len) {
        return None;
    }
    if offsets.windows(2).any(|pair| pair[0] > pair[1]) {
        return None;
    }
    let blob = cur.take(blob_len)?;
    let mut strings = Vec::with_capacity(string_count);
    for pair in offsets.windows(2) {
        let text = std::str::from_utf8(blob.get(pair[0]..pair[1])?).ok()?;
        strings.push(text.to_owned());
    }
    let schema_version = strings.get(schema_idx)?.clone();
    let nsi_version = strings.get(nsi_idx)?.clone();
    let mut presets = Vec::with_capacity(preset_count);
    for _ in 0..preset_count {
        let id_i = cur.u32()? as usize;
        let en_i = cur.u32()? as usize;
        let zh_i = cur.u32()?;
        let score_milli = cur.u16()?;
        let geom = cur.u8()?;
        let _pad = cur.u8()?;
        let n_tags = cur.u16()? as usize;
        let n_match = cur.u16()? as usize;
        let n_remove = cur.u16()? as usize;
        let _pad2 = cur.u16()?;
        let mut tags = Vec::with_capacity(n_tags);
        for _ in 0..n_tags {
            let key = strings.get(cur.u32()? as usize)?.clone();
            let value = strings.get(cur.u32()? as usize)?.clone();
            tags.push((key, value));
        }
        let mut match_tags = Vec::with_capacity(n_match);
        for _ in 0..n_match {
            let key = strings.get(cur.u32()? as usize)?.clone();
            let value = strings.get(cur.u32()? as usize)?.clone();
            match_tags.push((key, value));
        }
        let mut remove_keys = Vec::with_capacity(n_remove);
        for _ in 0..n_remove {
            remove_keys.push(strings.get(cur.u32()? as usize)?.clone());
        }
        if tags.is_empty() {
            continue;
        }
        let name_en = strings.get(en_i)?.clone();
        let name_zh = if zh_i == SAME_NAME {
            name_en.clone()
        } else {
            strings.get(zh_i as usize)?.clone()
        };
        presets.push(SchemaPreset {
            id: strings.get(id_i)?.clone(),
            name_zh,
            name_en,
            tags,
            match_tags,
            match_score: score_milli as f32 / 1000.0,
            remove_keys,
            geometry: geometry_from_bits(geom),
            suggestion,
        });
    }
    if cur.at != bytes.len() {
        return None;
    }
    Some(DecodedFile {
        schema_version,
        nsi_version,
        presets,
    })
}

fn load_catalog() -> Catalog {
    let Some(bytes) = assets::read_asset("presets/schema.neo") else {
        eprintln!(
            "neo-style: presets/schema.neo not found; preset list is empty. Run `npm ci` and `node scripts/build-presets.mjs`, or set NEO_ASSET_DIR."
        );
        return empty_catalog();
    };
    let Some(decoded) = decode_presets(&bytes, KIND_SCHEMA, false) else {
        eprintln!("neo-style: schema.neo is not a readable preset file");
        return empty_catalog();
    };
    let (categories, membership) = build_categories(&decoded.presets);
    let match_index = build_match_index(&decoded.presets);
    Catalog {
        schema_version: decoded.schema_version,
        nsi_version: decoded.nsi_version,
        schema: decoded.presets,
        categories,
        membership,
        match_index,
    }
}

fn load_suggestions() -> SuggestionSet {
    let Some(bytes) = assets::read_asset("presets/suggestions.neo") else {
        eprintln!(
            "neo-style: presets/suggestions.neo not found; brand suggestions are empty. Run `node scripts/build-presets.mjs`."
        );
        return SuggestionSet {
            rows: Vec::new(),
            match_index: HashMap::new(),
        };
    };
    let Some(decoded) = decode_presets(&bytes, KIND_SUGGESTIONS, true) else {
        eprintln!("neo-style: suggestions.neo is not a readable preset file");
        return SuggestionSet {
            rows: Vec::new(),
            match_index: HashMap::new(),
        };
    };
    let match_index = build_match_index(&decoded.presets);
    SuggestionSet {
        rows: decoded.presets,
        match_index,
    }
}

enum Hit {
    Schema(usize),
    Suggestion(usize),
}

/// Match score between a preset and feature tags; returns `None` when they do not match.
///
/// Higher is more specific: matched key count first, then `matchScore`, then id path depth.
pub fn preset_match_score(preset: &SchemaPreset, entity_tags: &[(String, String)]) -> Option<i64> {
    if preset.match_tags.is_empty() {
        return Some(0);
    }
    let mut matched = 0i64;
    for (key, want) in &preset.match_tags {
        let (_, have) = entity_tags.iter().find(|(k, _)| k == key)?;
        if want != "*" && have != want {
            return None;
        }
        matched += 1;
    }
    let score_boost = (preset.match_score * 1000.0).round() as i64;
    let depth = preset.id.bytes().filter(|b| *b == b'/').count() as i64;
    Some(matched * 1_000_000 + score_boost * 100 + depth * 10 + if preset.suggestion { 0 } else { 1 })
}

fn push_hits(
    index: &HashMap<(String, String), Vec<usize>>,
    entity_tags: &[(String, String)],
    suggestion: bool,
    seen: &mut HashSet<u64>,
    out: &mut Vec<Hit>,
) {
    for (key, value) in entity_tags {
        for lookup in [(key.clone(), value.clone()), (key.clone(), "*".to_owned())] {
            if let Some(list) = index.get(&lookup) {
                for &slot in list {
                    let id = if suggestion {
                        (1u64 << 63) | slot as u64
                    } else {
                        slot as u64
                    };
                    if seen.insert(id) {
                        out.push(if suggestion {
                            Hit::Suggestion(slot)
                        } else {
                            Hit::Schema(slot)
                        });
                    }
                }
            }
        }
    }
}

fn candidate_hits(entity_tags: &[(String, String)]) -> Vec<Hit> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    push_hits(&catalog().match_index, entity_tags, false, &mut seen, &mut out);
    if let Some(suggestions) = SUGGESTIONS.get() {
        push_hits(&suggestions.match_index, entity_tags, true, &mut seen, &mut out);
    }
    out
}

fn hit_preset(hit: Hit) -> &'static SchemaPreset {
    match hit {
        Hit::Schema(index) => &catalog().schema[index],
        Hit::Suggestion(index) => &SUGGESTIONS.get().expect("suggestion hit before load").rows[index],
    }
}

/// Find the best-matching preset from the current tags and geometry.
///
/// Brand suggestions participate in matching only after they have been loaded by a search.
pub fn best_preset_for_tags(
    entity_tags: &[(String, String)],
    geometry: PresetGeometryFilter,
) -> Option<&'static SchemaPreset> {
    if entity_tags.is_empty() {
        return None;
    }
    let mut best: Option<(&'static SchemaPreset, i64)> = None;
    for hit in candidate_hits(entity_tags) {
        let preset = hit_preset(hit);
        if !geometry.allows(preset) {
            continue;
        }
        let Some(score) = preset_match_score(preset, entity_tags) else {
            continue;
        };
        if score <= 0 {
            continue;
        }
        match best {
            Some((_, best_score)) if best_score >= score => {}
            _ => best = Some((preset, score)),
        }
    }
    best.map(|(preset, _)| preset)
}

/// schema presets (excluding brand suggestions).
pub fn schema_presets() -> &'static [SchemaPreset] {
    &catalog().schema
}

/// Brand suggestions generated by name-suggestion-index. `suggestions.neo` is read on first call only.
pub fn suggestion_presets() -> &'static [SchemaPreset] {
    &ensure_suggestions().rows
}

/// List of preset categories (first path segment).
pub fn preset_categories() -> &'static [PresetCategory] {
    &catalog().categories
}

/// Look up a schema preset by id; if absent from the schema, load brand suggestions too.
pub fn schema_preset(id: &str) -> Option<&'static SchemaPreset> {
    if let Some(preset) = schema_presets().iter().find(|preset| preset.id == id) {
        return Some(preset);
    }
    suggestion_presets().iter().find(|preset| preset.id == id)
}

/// Whether a preset belongs to a category (first path segment; brand suggestions use the same prefix rule).
pub fn preset_in_category(preset_id: &str, category_id: &str) -> bool {
    if category_id.is_empty() {
        return true;
    }
    if let Some(group) = catalog().membership.get(preset_id) {
        return group == category_id;
    }
    group_id_of(preset_id) == category_id
}

/// Category id the preset belongs to (0 or 1).
pub fn categories_for_preset(preset_id: &str) -> Vec<&'static str> {
    if let Some(group) = catalog().membership.get(preset_id) {
        return vec![group.as_str()];
    }
    let group = group_id_of(preset_id);
    if group.is_empty() {
        Vec::new()
    } else {
        preset_categories()
            .iter()
            .find(|category| category.id == group)
            .map(|category| vec![category.id.as_str()])
            .unwrap_or_default()
    }
}

/// Loaded schema package version.
pub fn schema_package_version() -> &'static str {
    catalog().schema_version.as_str()
}

/// Loaded name-suggestion-index package version.
pub fn nsi_package_version() -> &'static str {
    catalog().nsi_version.as_str()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn push_u16(buf: &mut Vec<u8>, value: u16) {
        buf.extend(value.to_le_bytes());
    }

    fn push_u32(buf: &mut Vec<u8>, value: u32) {
        buf.extend(value.to_le_bytes());
    }

    fn sample_schema() -> Vec<u8> {
        let strings = [
            "7.3.0",
            "8.0.0",
            "amenity/bench",
            "Bench",
            "amenity",
            "bench",
            "brand:wikipedia",
        ];
        let encoded: Vec<&[u8]> = strings.iter().map(|text| text.as_bytes()).collect();
        let blob_len: u32 = encoded.iter().map(|part| part.len() as u32).sum();
        let mut buf = Vec::new();
        buf.extend(b"NEOP");
        push_u16(&mut buf, 1);
        push_u16(&mut buf, KIND_SCHEMA);
        push_u32(&mut buf, 0);
        push_u32(&mut buf, 1);
        push_u32(&mut buf, strings.len() as u32);
        push_u32(&mut buf, blob_len);
        push_u32(&mut buf, 1);
        let mut cursor = 0u32;
        for part in &encoded {
            push_u32(&mut buf, cursor);
            cursor += part.len() as u32;
        }
        push_u32(&mut buf, cursor);
        for part in &encoded {
            buf.extend(*part);
        }
        push_u32(&mut buf, 2);
        push_u32(&mut buf, 3);
        push_u32(&mut buf, SAME_NAME);
        push_u16(&mut buf, 2000);
        buf.push(1 | 8);
        buf.push(0);
        push_u16(&mut buf, 1);
        push_u16(&mut buf, 1);
        push_u16(&mut buf, 1);
        push_u16(&mut buf, 0);
        push_u32(&mut buf, 4);
        push_u32(&mut buf, 5);
        push_u32(&mut buf, 4);
        push_u32(&mut buf, 5);
        push_u32(&mut buf, 6);
        buf
    }

    #[test]
    fn decodes_string_table_and_shared_name() {
        let decoded = decode_presets(&sample_schema(), KIND_SCHEMA, false).expect("preset file");
        assert_eq!(decoded.schema_version, "7.3.0");
        assert_eq!(decoded.nsi_version, "8.0.0");
        let preset = &decoded.presets[0];
        assert_eq!(preset.id, "amenity/bench");
        assert_eq!(preset.name_zh, "Bench");
        assert_eq!(preset.name_en, "Bench");
        assert_eq!(preset.tags, vec![("amenity".to_owned(), "bench".to_owned())]);
        assert_eq!(preset.remove_keys, vec!["brand:wikipedia".to_owned()]);
        assert_eq!(preset.geometry, vec!["point".to_owned(), "area".to_owned()]);
        assert!((preset.match_score - 2.0).abs() < f32::EPSILON);
        assert!(!preset.suggestion);
    }

    #[test]
    fn rejects_a_short_file() {
        assert!(decode_presets(b"NEOP", KIND_SCHEMA, false).is_none());
    }
}
