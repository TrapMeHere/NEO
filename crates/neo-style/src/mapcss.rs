//! MapCSS 子集：解析 + 编译为按 Sym 比较的规则。
//!
//! 支持：
//! * 选择器：`way` / `node` / `relation` / `area` / `*`，可带 `[key]`、`[key=value]`；
//! * 逗号分隔多个选择器；
//! * 缩放过滤：`|z12-`、`|z14-z18`；
//! * 声明：`color`、`fill-color`、`width`、`opacity`（`#rgb/#rrggbb/#rrggbbaa`）。
//!
//! 未支持：`@media`、`eval()`、`setting()`、图案、dashes、icon-image、text 等；
//! 这些在 docs/11 中列为后续兼容项。解析失败不会让程序崩溃（调用方回退内置调色板）。

#![allow(missing_docs)]

use neo_core::Sym;

/// 选择器元素类型。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Element {
    Any,
    Node,
    Way,
    Area,
    Relation,
    /// 闭合 way。求值时按 way/area 匹配。
    ClosedWay,
    /// 画布规则，不作用到几何对象。
    Canvas,
    /// 样式元数据，不作用到几何对象。
    Meta,
}

/// 解析后的选择器（字符串条件）。
#[derive(Clone, Debug, PartialEq)]
pub struct Selector {
    pub element: Element,
    pub conditions: Vec<(String, Option<String>)>,
    pub min_zoom: f32,
    pub max_zoom: f32,
}

/// 声明。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Declarations {
    pub color: Option<[u8; 4]>,
    pub fill_color: Option<[u8; 4]>,
    /// `fill-color: none`，明确不要内置填充。
    pub clear_fill: bool,
    pub width: Option<f32>,
    pub opacity: Option<f32>,
    /// `dashes` 列表。渲染器可以忽略，但解析结果保留。
    pub dashes: Option<Vec<f32>>,
    /// `text`：标签键（如 `name`）。
    pub text: Option<String>,
    /// `text-color`。
    pub text_color: Option<[u8; 4]>,
    /// `font-size`（屏幕像素）。
    pub font_size: Option<f32>,
    /// `text-position`（`center`/`above`/`below`）。
    pub text_position: Option<String>,
    /// `icon-image`（记录但暂不绘制）。
    pub icon_image: Option<String>,
    /// `icon-width`（屏幕像素）。
    pub icon_width: Option<f32>,
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
    }
}

/// 一条 MapCSS 规则。
#[derive(Clone, Debug, PartialEq)]
pub struct Rule {
    pub selector: Selector,
    pub decls: Declarations,
}

/// 解析结果。
#[derive(Clone, Debug, Default)]
pub struct MapCss {
    pub rules: Vec<Rule>,
    /// 识别到但未实现的属性（text/icon 等）。不静默丢弃。
    pub warnings: Vec<String>,
}

/// 编译后的选择器（Sym 条件）。
#[derive(Clone, Debug)]
pub struct CompiledSelector {
    pub element: Element,
    pub conditions: Vec<(Sym, Option<Sym>)>,
    pub min_zoom: f32,
    pub max_zoom: f32,
}

/// 编译后的规则。
#[derive(Clone, Debug)]
pub struct CompiledRule {
    pub selector: CompiledSelector,
    pub decls: Declarations,
}

/// 编译后的样式表。
#[derive(Clone, Debug, Default)]
pub struct CompiledMapCss {
    pub rules: Vec<CompiledRule>,
}

impl CompiledMapCss {
    /// 计算某对象的最终声明（后规则覆盖前规则）。
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
    /// 解析样式文本。
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

    /// 画布填充色（`canvas { fill-color: ... }`），没有则 `None`。
    pub fn canvas_fill(&self) -> Option<[u8; 4]> {
        self.rules
            .iter()
            .find(|rule| rule.selector.element == Element::Canvas)
            .and_then(|rule| rule.decls.fill_color)
    }

    /// 用 `intern` 把标签 key/value 编译成 Sym。
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
        // MapCSS 的 `|z18` 表示"仅 z18"，不是 18 以上全部。
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
    // 先拒绝非 ASCII：下面的字节切片在多字节 UTF-8 中间会 panic。
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
