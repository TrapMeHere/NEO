//! A small editor for style text: change the value of the nth declaration, then re-parse to confirm the stylesheet was not broken.

use crate::mapcss::MapCss;

/// Replace the value of the `nth` (0-based) `property:` with `value`.
pub fn set_declaration(source: &str, property: &str, nth: usize, value: &str) -> Result<String, String> {
    let needle = format!("{property}:");
    let mut found = 0usize;
    let mut search_from = 0usize;
    while let Some(rel) = source[search_from..].find(&needle) {
        let start = search_from + rel;
        if found == nth {
            let value_start = start + needle.len();
            let rest = &source[value_start..];
            let end_rel = rest.find(';').unwrap_or(rest.len());
            let mut out = String::with_capacity(source.len() + value.len());
            out.push_str(&source[..value_start]);
            out.push(' ');
            out.push_str(value.trim());
            out.push_str(&source[value_start + end_rel..]);
            let parsed = MapCss::parse(&out)?;
            if parsed.rules.is_empty() && !out.contains('{') {
                return Err("edit removed every rule".into());
            }
            return Ok(out);
        }
        found += 1;
        search_from = start + needle.len();
    }
    Err(format!("property `{property}` occurrence {nth} not found"))
}
