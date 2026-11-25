//! The `manifest.toml` subset inside `.neopkg`.
//!
//! No toml crate is pulled in: only the keys spelled out in this file are accepted, so the plugin manifest
//! does not turn into a general-purpose configuration language.

use crate::PluginError;

/// Capability declarations in the manifest.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ManifestCaps {
    /// The `submit-command` capability.
    pub command: bool,
    /// Panels.
    pub ui: bool,
    /// Event topics.
    pub events: Vec<String>,
    /// Network hosts or URLs.
    pub network: Vec<String>,
    /// Reads the plugin directory.
    pub filesystem: bool,
    /// Imagery.
    pub imagery: bool,
}

/// Plugin manifest.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PluginManifest {
    /// Must be 1.
    pub schema: u32,
    /// Stable id.
    pub id: String,
    /// Display name.
    pub name: String,
    /// Plugin version.
    pub version: String,
    /// Host ABI, `1` or `1.x`.
    pub api: String,
    /// Entry file name inside the package.
    pub entry: String,
    /// Optional minimum NEO version.
    pub min_neo_version: Option<String>,
    /// Declared capabilities.
    pub capabilities: ManifestCaps,
    /// Hex SHA-256. An empty string means unsigned.
    pub signature: String,
}

impl PluginManifest {
    /// The host only implements ABI 1.x.
    pub fn api_compatible(&self) -> bool {
        api_compatible(&self.api)
    }
}

/// `1`, `1.0`, `1.2.3` are compatible; `2.0` is not.
pub fn api_compatible(api: &str) -> bool {
    api.split('.').next() == Some("1")
}

/// Parses the manifest text.
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
