//! 预设覆盖率：把 NEO 载入的 id-tagging-schema 预设按 JOSM 默认分组前缀计数。
//!
//! 不读取 JOSM 的 `defaultpresets.xml`（GPL）。分组名是公开的编辑概念，
//! 用 schema id 前缀对照。计数为 0 的分组记入缺失列表，而不是当成已覆盖。

use crate::schema::schema_presets;

/// 一行覆盖率。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CoverageRow {
    /// JOSM 里用户认识的分组名。
    pub josm_group: &'static str,
    /// schema id 前缀，例如 `amenity/`。
    pub prefix: &'static str,
    /// 命中的预设数。
    pub count: usize,
}

/// 覆盖率报告。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CoverageReport {
    /// 每个分组一行，包括计数为 0 的。
    pub rows: Vec<CoverageRow>,
}

impl CoverageReport {
    /// 计数为 0 的分组。这些是相对 JOSM 默认分组的缺口。
    pub fn missing(&self) -> Vec<&'static str> {
        self.rows
            .iter()
            .filter(|row| row.count == 0)
            .map(|row| row.josm_group)
            .collect()
    }

    /// 至少有一条预设的分组数。
    pub fn covered_groups(&self) -> usize {
        self.rows.iter().filter(|row| row.count > 0).count()
    }
}

/// JOSM 默认预设里用户会找的分组，以及 schema id 前缀。
pub const JOSM_GROUPS: &[(&str, &str)] = &[
    ("Highways", "highway/"),
    ("Waterways", "waterway/"),
    ("Railways", "railway/"),
    ("Power", "power/"),
    ("Buildings", "building/"),
    ("Amenities", "amenity/"),
    ("Man made", "man_made/"),
    ("Natural", "natural/"),
    ("Leisure", "leisure/"),
    ("Shops", "shop/"),
    ("Tourism", "tourism/"),
    ("Emergency", "emergency/"),
    ("Barriers", "barrier/"),
    ("Aerialways", "aerialway/"),
    ("Aeroways", "aeroway/"),
    ("Historic", "historic/"),
    ("Office", "office/"),
    ("Craft", "craft/"),
    ("Healthcare", "healthcare/"),
    ("Telecom", "telecom/"),
    ("Seamarks", "seamark/"),
    ("Pipelines", "pipeline/"),
];

/// 对照当前嵌入的 schema 预设。
pub fn preset_coverage() -> CoverageReport {
    let presets = schema_presets();
    let rows = JOSM_GROUPS
        .iter()
        .map(|(group, prefix)| CoverageRow {
            josm_group: group,
            prefix,
            count: presets
                .iter()
                .filter(|preset| preset.id.starts_with(prefix))
                .count(),
        })
        .collect();
    CoverageReport { rows }
}

/// Markdown 表，给 `docs/prototypes/presets-coverage.md` 和 CLI 用。
pub fn coverage_markdown() -> String {
    let report = preset_coverage();
    let mut out = String::from("| JOSM 分组 | schema 前缀 | 条数 |\n|---|---|---:|\n");
    for row in &report.rows {
        out.push_str(&format!(
            "| {} | `{}` | {} |\n",
            row.josm_group, row.prefix, row.count
        ));
    }
    out.push_str("\n缺失分组：");
    let missing = report.missing();
    if missing.is_empty() {
        out.push_str("（无）\n");
    } else {
        out.push('\n');
        for name in missing {
            out.push_str(&format!("- {name}\n"));
        }
    }
    out
}
