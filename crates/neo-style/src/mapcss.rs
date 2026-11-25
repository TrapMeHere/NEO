//! MapCSS subset: parse and compile into rules compared by Sym.
//!
//! Supports:
//! * selectors: `way` / `node` / `relation` / `area` / `*`, optionally with `[key]`, `[key=value]`;
//! * comma-separated multiple selectors;
//! * zoom filters: `|z12-`, `|z14-z18`;
//! * declarations: `color`, `fill-color`, `width`, `opacity`, `casing-color` /
//!   `casing-width` / `casing-dashes` (`#rgb/#rrggbb/#rrggbbaa`).
//!
//! Not supported: `@media`, `eval()`, `setting()`, patterns, etc.;
//! these are listed as future compatibility items in docs/11. A parse failure never crashes the program (the caller falls back to the built-in palette).

#![allow(missing_docs)]

use neo_core::Sym;

/// Selector element type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Element {
    Any,
    Node,
    Way,
    Area,
    Relation,
    /// Closed way. Matched as way/area during evaluation.
    ClosedWay,
    /// Canvas rule, not applied to geometry objects.
    Canvas,
    /// Style metadata, not applied to geometry objects.
    Meta,
}

/// Parsed selector (string conditions).
#[derive(Clone, Debug, PartialEq)]
pub struct Selector {
    pub element: Element,
    pub conditions: Vec<(String, Option<String>)>,
    pub min_zoom: f32,
    pub max_zoom: f32,
}

/// Declaration.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Declarations {
    pub color: Option<[u8; 4]>,
    pub fill_color: Option<[u8; 4]>,
    /// `fill-color: none`, explicitly requesting no built-in fill.
    pub clear_fill: bool,
    pub width: Option<f32>,
    pub opacity: Option<f32>,
    /// `dashes` list. The renderer may ignore it, but the parse result keeps it.
    pub dashes: Option<Vec<f32>>,
    /// `text`: label key (e.g. `name`).
    pub text: Option<String>,
    /// `text-color`.
    pub text_color: Option<[u8; 4]>,
    /// `font-size` (screen pixels).
    pub font_size: Option<f32>,
    /// `text-position` (`center`/`above`/`below`).
    pub text_position: Option<String>,
    /// `icon-image` (recorded but not yet drawn).
    pub icon_image: Option<String>,
    /// `icon-width` (screen pixels).
    pub icon_width: Option<f32>,
    /// `casing-color`: casing color.
    pub casing_color: Option<[u8; 4]>,
    /// `casing-width`: casing width (screen pixels).
    pub casing_width: Option<f32>,
    /// `casing-dashes`: casing dash pattern (screen pixels).
    pub casing_dashes: Option<Vec<f32>>,
}

impl Declarations {
    fn merge(&mut self, other: &Self) {
        if other.color.is_some() {
            self.color = other.color;
        }
        if other.clear_fill {
            self.fill_color = None;
            self.clear_fill = true;
        }
        if other.fill_color.is_some() {
            self.fill_color = other.fill_color;
            self.clear_fill = false;
        }
        if other.width.is_some() {
            self.width = other.width;
        }
        if other.opacity.is_some() {
            self.opacity = other.opacity;
        }
        if other.dashes.is_some() {
            self.dashes = other.dashes.clone();
        }
        if other.text.is_some() {
            self.text = other.text.clone();
        }
        if other.text_color.is_some() {
            self.text_color = other.text_color;
        }
        if other.font_size.is_some() {
            self.font_size = other.font_size;
        }
        if other.text_position.is_some() {
            self.text_position = other.text_position.clone();
        }
        if other.icon_image.is_some() {
            self.icon_image = other.icon_image.clone();
        }
        if other.icon_width.is_some() {
            self.icon_width = other.icon_width;
        }
        if other.casing_color.is_some() {
            self.casing_color = other.casing_color;
        }
        if other.casing_width.is_some() {
            self.casing_width = other.casing_width;
        }
        if other.casing_dashes.is_some() {
            self.casing_dashes = other.casing_dashes.clone();
        }
    }
}

/// One MapCSS rule.
#[derive(Clone, Debug, PartialEq)]
pub struct Rule {
    pub selector: Selector,
    pub decls: Declarations,
}

/// Parse result.
#[derive(Clone, Debug, Default)]
pub struct MapCss {
    pub rules: Vec<Rule>,
    /// Recognized but unimplemented properties (text/icon etc.). Not silently dropped.
    pub warnings: Vec<String>,
}

/// Compiled selector (Sym conditions).
#[derive(Clone, Debug)]
pub struct CompiledSelector {
    pub element: Element,
    pub conditions: Vec<(Sym, Option<Sym>)>,
    pub min_zoom: f32,
    pub max_zoom: f32,
}

/// Compiled rule.
#[derive(Clone, Debug)]
pub struct CompiledRule {
    pub selector: CompiledSelector,
    pub decls: Declarations,
}

/// Compiled stylesheet.
#[derive(Clone, Debug, Default)]
pub struct CompiledMapCss {
    pub rules: Vec<CompiledRule>,
}

impl CompiledMapCss {
    /// Compute the final declarations for an object (later rules override earlier ones).
    pub fn style(&self, element: Element, tags: &[(Sym, Sym)], zoom: f32) -> Option<Declarations> {
        let mut out = Declarations::default();
        let mut matched = false;
        for rule in &self.rules {
            let sel = &rule.selector;
            if !element_matches(sel.element, element) || zoom < sel.min_zoom || zoom > sel.max_zoom {
                continue;
            }
            let ok = sel.conditions.iter().all(|(key, value)| {
                let found = tags.iter().find(|(k, _)| k == key).map(|(_, v)| *v);
                match (found, value) {
                    (Some(v), Some(want)) => v == *want,
                    (Some(_), None) => true,
                    _ => false,
                }
            });
            if ok {
                matched = true;
                out.merge(&rule.decls);
            }
        }
        matched.then_some(out)
    }
}

fn element_matches(sel: Element, actual: Element) -> bool {
    match sel {
        Element::Any => true,
        Element::Way | Element::Area | Element::ClosedWay => matches!(actual, Element::Way | Element::Area),
        Element::Canvas | Element::Meta => false,
        other => other == actual,
    }
}

impl MapCss {
    /// Parse style text.
    pub fn parse(text: &str) -> Result<Self, String> {
        let cleaned = strip_comments(text);
        let mut rules = Vec::new();
        let mut warnings = Vec::new();
        let mut rest = cleaned.as_str();
        while let Some(open) = rest.find('{') {
            let selector_text = rest[..open].trim();
            let Some(close) = rest[open..].find('}') else {
                return Err("unclosed declaration block".to_owned());
            };
            let decl_text = &rest[open + 1..open + close];
            let decls = parse_declarations(decl_text, &mut warnings)?;
            for part in selector_text.split(',') {
                let part = part.trim();
                if part.is_empty() {
                    continue;
                }
                let selector = parse_selector(part)?;
                rules.push(Rule {
                    selector,
                    decls: decls.clone(),
                });
            }
            rest = &rest[open + close + 1..];
        }
        Ok(Self { rules, warnings })
    }

    /// Canvas fill color (`canvas { fill-color: ... }`), or `None` if absent.
    pub fn canvas_fill(&self) -> Option<[u8; 4]> {
        self.rules
            .iter()
            .find(|rule| rule.selector.element == Element::Canvas)
            .and_then(|rule| rule.decls.fill_color)
    }

    /// Compile tag keys/values into Syms with `intern`.
    pub fn compile(self, mut intern: impl FnMut(&str) -> Sym) -> CompiledMapCss {
        let rules = self
            .rules
            .into_iter()
            .map(|rule| {
                let conditions = rule
                    .selector
                    .conditions
                    .iter()
                    .map(|(key, value)| (intern(key), value.as_ref().map(|value| intern(value))))
                    .collect();
                CompiledRule {
                    selector: CompiledSelector {
                        element: rule.selector.element,
                        conditions,
                        min_zoom: rule.selector.min_zoom,
                        max_zoom: rule.selector.max_zoom,
                    },
                    decls: rule.decls,
                }
            })
            .collect();
        CompiledMapCss { rules }
    }
}

fn strip_comments(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    let mut quote: Option<u8> = None;
    while i < bytes.len() {
        let byte = bytes[i];
        if let Some(q) = quote {
            if byte == b'\\' && i + 1 < bytes.len() {
                let ch = text[i..].chars().next().unwrap_or('\0');
                out.push(ch);
                i += ch.len_utf8();
                if let Some(next) = text[i..].chars().next() {
                    out.push(next);
                    i += next.len_utf8();
                }
                continue;
            }
            if byte == q {
                quote = None;
            }
            let ch = text[i..].chars().next().unwrap_or('\0');
            out.push(ch);
            i += ch.len_utf8();
            continue;
        }
        if byte == b'"' || byte == b'\'' {
            quote = Some(byte);
            out.push(byte as char);
            i += 1;
        } else if byte == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'*' {
            i += 2;
            while i + 1 < bytes.len() && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                i += 1;
            }
            i = (i + 2).min(bytes.len());
        } else if byte == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'/' {
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
        } else {
            let ch = text[i..].chars().next().unwrap_or('\0');
            out.push(ch);
            i += ch.len_utf8();
        }
    }
    out
}

fn parse_selector(text: &str) -> Result<Selector, String> {
    let mut base = text.to_owned();
    let mut min_zoom = 0.0;
    let mut max_zoom = f32::MAX;
    if let Some(bar) = base.find('|') {
        let start = bar + 1;
        let rest = &base[start..];
        let end = rest
            .find(|c: char| c == '[' || c.is_whitespace())
            .unwrap_or(rest.len());
        let token = rest[..end].to_owned();
        (min_zoom, max_zoom) = parse_zoom(Some(&token))?;
        base.replace_range(bar..start + end, "");
    }
    let (element, rest) = split_element(&base);
    let mut conditions = Vec::new();
    let mut rest = rest;
    while let Some(start) = rest.find('[') {
        let Some(end) = rest[start..].find(']') else {
            return Err(format!("unclosed condition in {text}"));
        };
        let inside = rest[start + 1..start + end].trim();
        let (key, value) = match inside.split_once('=') {
            Some((key, value)) => (
                key.trim().to_owned(),
                Some(value.trim().trim_matches('"').trim_matches('\'').to_owned()),
            ),
            None => (inside.to_owned(), None),
        };
        conditions.push((key, value));
        rest = &rest[start + end + 1..];
    }
    Ok(Selector {
        element,
        conditions,
        min_zoom,
        max_zoom,
    })
}

fn split_element(text: &str) -> (Element, &str) {
    let idx = text
        .find(|c: char| c == '[' || c == '|' || c.is_whitespace())
        .unwrap_or(text.len());
    let name = &text[..idx];
    let element = match name {
        "node" => Element::Node,
        "way" => Element::Way,
        "area" => Element::Area,
        "relation" => Element::Relation,
        "closedway" => Element::ClosedWay,
        "canvas" => Element::Canvas,
        "meta" | "setting" | "settings" => Element::Meta,
        "*" | "" => Element::Any,
        _ => Element::Any,
    };
    (element, &text[idx..])
}

fn parse_zoom(part: Option<&str>) -> Result<(f32, f32), String> {
    let Some(part) = part else {
        return Ok((0.0, f32::MAX));
    };
    let part = part.trim();
    let Some(rest) = part.strip_prefix('z') else {
        return Ok((0.0, f32::MAX));
    };
    let (min, max) = match rest.split_once('-') {
        Some((min, max)) => (min, max),
        // MapCSS's `|z18` means "z18 only", not everything above 18.
        None => (rest, rest),
    };
    let min = if min.is_empty() {
        0.0
    } else {
        min.parse::<f32>().map_err(|_| format!("bad zoom: {part}"))?
    };
    let max = if max.is_empty() {
        f32::MAX
    } else {
        max.parse::<f32>().map_err(|_| format!("bad zoom: {part}"))?
    };
    if min > max {
        return Err(format!("bad zoom range: {part}"));
    }
    Ok((min, max))
}

fn parse_declarations(text: &str, warnings: &mut Vec<String>) -> Result<Declarations, String> {
    let mut decls = Declarations::default();
    for part in text.split(';') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let Some((key, value)) = part.split_once(':') else {
            continue;
        };
        let key = key.trim();
        let value = value.trim();
        match key {
            "color" => decls.color = Some(parse_color(value)?),
            "fill-color" => {
                if value.eq_ignore_ascii_case("none") || value.eq_ignore_ascii_case("transparent") {
                    decls.fill_color = None;
                    decls.clear_fill = true;
                } else {
                    decls.fill_color = Some(parse_color(value)?);
                    decls.clear_fill = false;
                }
            }
            "width" => decls.width = Some(parse_f32(value)?),
            "opacity" => decls.opacity = Some(parse_f32(value)?),
            "dashes" => decls.dashes = Some(parse_dashes(value)?),
            "text" => match parse_text_expr(value) {
                Some(text) => decls.text = Some(text),
                None => warnings.push(format!("unsupported text expression `{value}`")),
            },
            "text-color" => decls.text_color = Some(parse_color(value)?),
            "font-size" => decls.font_size = Some(parse_f32(value)?),
            "text-position" => decls.text_position = Some(parse_word(value)?),
            "icon-image" => match parse_text_expr(value) {
                Some(icon) => decls.icon_image = Some(icon),
                None => warnings.push(format!("unsupported icon expression `{value}`")),
            },
            "icon-width" => decls.icon_width = Some(parse_f32(value)?),
            "casing-color" => decls.casing_color = Some(parse_color(value)?),
            "casing-width" => decls.casing_width = Some(parse_f32(value)?),
            "casing-dashes" => decls.casing_dashes = Some(parse_dashes(value)?),
            _ => warnings.push(format!("unknown property `{key}`")),
        }
    }
    Ok(decls)
}

fn parse_text_expr(value: &str) -> Option<String> {
    let value = value.trim().trim_matches('"').trim();
    if value.is_empty() || value.contains('(') || value.contains('\'') {
        return None;
    }
    Some(value.to_owned())
}

fn parse_word(value: &str) -> Result<String, String> {
    let value = value.trim().trim_matches('"');
    if value.is_empty() || value.contains(char::is_whitespace) {
        return Err(format!("bad keyword: {value}"));
    }
    Ok(value.to_owned())
}

fn parse_dashes(value: &str) -> Result<Vec<f32>, String> {
    value
        .split(',')
        .map(|part| part.trim())
        .filter(|part| !part.is_empty())
        .map(parse_f32)
        .collect()
}

fn parse_f32(value: &str) -> Result<f32, String> {
    value.parse::<f32>().map_err(|_| format!("bad number: {value}"))
}

fn parse_color(value: &str) -> Result<[u8; 4], String> {
    let hex = value.trim().trim_start_matches('#');
    // Reject non-ASCII first: the byte slicing below would panic in the middle of a multi-byte UTF-8 char.
    if !hex.is_ascii() {
        return Err(format!("bad color: {value}"));
    }
    let parse = |s: &str| u8::from_str_radix(s, 16).map_err(|_| format!("bad color: {value}"));
    match hex.len() {
        3 => {
            let r = parse(&hex[0..1])?;
            let g = parse(&hex[1..2])?;
            let b = parse(&hex[2..3])?;
            Ok([r * 17, g * 17, b * 17, 255])
        }
        4 => {
            let r = parse(&hex[0..1])?;
            let g = parse(&hex[1..2])?;
            let b = parse(&hex[2..3])?;
            let a = parse(&hex[3..4])?;
            Ok([r * 17, g * 17, b * 17, a * 17])
        }
        6 => Ok([parse(&hex[0..2])?, parse(&hex[2..4])?, parse(&hex[4..6])?, 255]),
        8 => Ok([
            parse(&hex[0..2])?,
            parse(&hex[2..4])?,
            parse(&hex[4..6])?,
            parse(&hex[6..8])?,
        ]),
        _ => Err(format!("bad color: {value}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_ascii_color_is_an_error_not_a_panic() {
        assert!(parse_color("#aébbc").is_err());
        assert!(parse_color("#中文颜色").is_err());
    }

    #[test]
    fn fill_none_clears_fill() {
        let css = MapCss::parse("way { fill-color: none; }").unwrap();
        assert!(css.rules[0].decls.clear_fill);
        assert!(css.rules[0].decls.fill_color.is_none());
    }

    #[test]
    fn dashes_are_parsed() {
        let css = MapCss::parse("way { color: #ff0000; dashes: 4, 2; }").unwrap();
        assert_eq!(css.rules[0].decls.dashes, Some(vec![4.0, 2.0]));
    }

    #[test]
    fn text_and_dashes_are_parsed() {
        let css = MapCss::parse(
            "way { color: #ff0000; dashes: 4, 2; text: name; text-color: #112233; font-size: 13; icon-image: icons/pin.png; icon-width: 18; }",
        )
        .unwrap();
        let decls = &css.rules[0].decls;
        assert_eq!(decls.dashes, Some(vec![4.0, 2.0]));
        assert_eq!(decls.text.as_deref(), Some("name"));
        assert_eq!(decls.text_color, Some([0x11, 0x22, 0x33, 255]));
        assert_eq!(decls.font_size, Some(13.0));
        assert_eq!(decls.icon_image.as_deref(), Some("icons/pin.png"));
        assert_eq!(decls.icon_width, Some(18.0));
    }

    #[test]
    fn zoom_filter_without_dash_is_exact() {
        assert_eq!(parse_zoom(Some("z18")).unwrap(), (18.0, 18.0));
        assert_eq!(parse_zoom(Some("z14-")).unwrap().0, 14.0);
        assert!(parse_zoom(Some("z18-14")).is_err());
    }

    #[test]
    fn casing_declarations_parse_and_merge() {
        let css = MapCss::parse(
            "way { casing-width: 4; casing-color: #112233; } way { casing-width: 6; casing-dashes: 4, 2; }",
        )
        .unwrap();
        assert!(
            css.warnings.is_empty(),
            "casing-* must not be reported as an unknown property: {:?}",
            css.warnings
        );
        let compiled = css.compile(|_| Sym::from_raw(0));
        let decl = compiled.style(Element::Way, &[], 15.0).unwrap();
        // Later rules override earlier ones.
        assert_eq!(decl.casing_width, Some(6.0));
        assert_eq!(decl.casing_color, Some([0x11, 0x22, 0x33, 255]));
        assert_eq!(decl.casing_dashes, Some(vec![4.0, 2.0]));
    }

    #[test]
    fn casing_dashes_are_parsed() {
        let css = MapCss::parse("way { casing-dashes: 4, 2; }").unwrap();
        assert_eq!(css.rules[0].decls.casing_dashes, Some(vec![4.0, 2.0]));
        assert_eq!(css.rules[0].decls.casing_width, None);
        assert_eq!(css.rules[0].decls.casing_color, None);
    }

    #[test]
    fn comments_preserve_utf8_and_quoted_slashes() {
        let css = r#"
            /* 中文注释 */
            way[name="a//b"] { color: #ff0000; }
        "#;
        let cleaned = strip_comments(css);
        assert!(cleaned.contains("a//b"), "quoted // must not start a comment");
        assert!(MapCss::parse(css).is_ok());
    }
}
