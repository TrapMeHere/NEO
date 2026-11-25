//! Preset coverage: count the id-tagging-schema presets loaded by NEO by JOSM's default group prefixes.
//!
//! Does not read JOSM's `defaultpresets.xml` (GPL). Group names are public editing concepts,
//! compared against schema id prefixes. Groups with a count of 0 go into the missing list rather than being treated as covered.

use crate::schema::schema_presets;

/// One coverage row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CoverageRow {
    /// A group name users know from JOSM.
    pub josm_group: &'static str,
    /// schema id prefix, e.g. `amenity/`.
    pub prefix: &'static str,
    /// Number of matched presets.
    pub count: usize,
}

/// Coverage report.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CoverageReport {
    /// One row per group, including those with a count of 0.
    pub rows: Vec<CoverageRow>,
}

impl CoverageReport {
    /// Groups with a count of 0. These are the gaps relative to JOSM's default groups.
    pub fn missing(&self) -> Vec<&'static str> {
        self.rows
            .iter()
            .filter(|row| row.count == 0)
            .map(|row| row.josm_group)
            .collect()
    }

    /// Number of groups with at least one preset.
    pub fn covered_groups(&self) -> usize {
        self.rows.iter().filter(|row| row.count > 0).count()
    }
}

/// Groups users look for in JOSM's default presets, and their schema id prefixes.
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

/// Compare against the currently embedded schema presets.
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

/// Markdown table, used by `docs/prototypes/presets-coverage.md` and the CLI.
pub fn coverage_markdown() -> String {
    let report = preset_coverage();
    let mut out = String::from("| JOSM group | schema prefix | count |\n|---|---|---:|\n");
    for row in &report.rows {
        out.push_str(&format!(
            "| {} | `{}` | {} |\n",
            row.josm_group, row.prefix, row.count
        ));
    }
    out.push_str("\nMissing groups: ");
    let missing = report.missing();
    if missing.is_empty() {
        out.push_str("(none)\n");
    } else {
        out.push('\n');
        for name in missing {
            out.push_str(&format!("- {name}\n"));
        }
    }
    out
}
