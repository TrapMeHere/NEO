//! `.neopkg` 里的 `manifest.toml` 子集。
//!
//! 不引入 toml crate：只接受本文件写明的键，避免插件清单变成通用配置语言。

use crate::PluginError;

/// 清单里的能力声明。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ManifestCaps {
    /// `submit-command`。
    pub command: bool,
    /// 面板。
    pub ui: bool,
    /// 事件主题。
    pub events: Vec<String>,
    /// 网络主机或 URL。
    pub network: Vec<String>,
    /// 读插件目录。
    pub filesystem: bool,
    /// 影像。
    pub imagery: bool,
}

/// 插件清单。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PluginManifest {
    /// 必须是 1。
    pub schema: u32,
    /// 稳定 id。
    pub id: String,
    /// 显示名。
    pub name: String,
    /// 插件版本。
    pub version: String,
    /// 宿主 ABI，`1` 或 `1.x`。
    pub api: String,
    /// 包内入口文件名。
    pub entry: String,
    /// 可选的最低 NEO 版本。
    pub min_neo_version: Option<String>,
    /// 声明的能力。
    pub capabilities: ManifestCaps,
    /// 十六进制 SHA-256。空字符串表示未签名。
    pub signature: String,
}

impl PluginManifest {
    /// 宿主只实现 ABI 1.x。
    pub fn api_compatible(&self) -> bool {
        api_compatible(&self.api)
    }
}

/// `1`、`1.0`、`1.2.3` 兼容；`2.0` 不兼容。
pub fn api_compatible(api: &str) -> bool {
    api.split('.').next() == Some("1")
}

/// 解析清单文本。
pub fn parse_manifest(text: &str) -> Result<PluginManifest, PluginError> {
    let mut schema = 0u32;
    let mut id = String::new();
    let mut name = String::new();
    let mut version = String::new();
    let mut api = String::new();
    let mut entry = String::new();
    let mut min_neo_version = None;
    let mut signature = String::new();
    let mut caps = ManifestCaps::default();
    let mut section = String::new();

    for raw in text.lines() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            section = line[1..line.len() - 1].trim().to_owned();
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            return Err(PluginError::Manifest(format!("expected key = value, got {line}")));
        };
        let key = key.trim();
        let value = value.trim();
        match (section.as_str(), key) {
            ("", "schema") => schema = parse_u32(value)?,
            ("", "id") => id = parse_string(value)?,
            ("", "name") => name = parse_string(value)?,
            ("", "version") => version = parse_string(value)?,
            ("", "api") => api = parse_string(value)?,
            ("", "entry") => entry = parse_string(value)?,
            ("", "min_neo_version") => min_neo_version = Some(parse_string(value)?),
            ("", "signature") => signature = parse_string(value)?,
            ("capabilities", "command") => caps.command = parse_bool(value)?,
            ("capabilities", "ui") => caps.ui = parse_bool(value)?,
            ("capabilities", "filesystem") => caps.filesystem = parse_bool(value)?,
            ("capabilities", "imagery") => caps.imagery = parse_bool(value)?,
            ("capabilities", "events") => caps.events = parse_string_list(value)?,
            ("capabilities", "network") => caps.network = parse_string_list(value)?,
            ("limits", _) => {}
            _ => {}
        }
    }

    if schema != 1 {
        return Err(PluginError::Manifest(format!("schema must be 1, got {schema}")));
    }
    if id.is_empty() || name.is_empty() || version.is_empty() || api.is_empty() || entry.is_empty() {
        return Err(PluginError::Manifest(
            "id, name, version, api and entry are required".into(),
        ));
    }
    if entry.contains("..") || entry.contains('\\') || entry.starts_with('/') {
        return Err(PluginError::Manifest(format!(
            "entry path is not allowed: {entry}"
        )));
    }
    Ok(PluginManifest {
        schema,
        id,
        name,
        version,
        api,
        entry,
        min_neo_version,
        capabilities: caps,
        signature,
    })
}

fn parse_u32(value: &str) -> Result<u32, PluginError> {
    value
        .parse()
        .map_err(|_| PluginError::Manifest(format!("bad integer: {value}")))
}

fn parse_bool(value: &str) -> Result<bool, PluginError> {
    match value {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err(PluginError::Manifest(format!("bad bool: {value}"))),
    }
}

fn parse_string(value: &str) -> Result<String, PluginError> {
    let value = value.trim();
    if value.len() >= 2 && value.starts_with('"') && value.ends_with('"') {
        Ok(value[1..value.len() - 1].replace("\\\"", "\""))
    } else {
        Err(PluginError::Manifest(format!(
            "expected quoted string, got {value}"
        )))
    }
}

fn parse_string_list(value: &str) -> Result<Vec<String>, PluginError> {
    let value = value.trim();
    let Some(inner) = value.strip_prefix('[').and_then(|v| v.strip_suffix(']')) else {
        return Err(PluginError::Manifest(format!("expected list, got {value}")));
    };
    if inner.trim().is_empty() {
        return Ok(Vec::new());
    }
    inner.split(',').map(|part| parse_string(part.trim())).collect()
}
