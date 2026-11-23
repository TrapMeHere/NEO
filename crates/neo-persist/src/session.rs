//! JOSM 会话文件读写（`.jos` XML 与 `.joz` ZIP 两种容器）。
//!
//! 格式（docs/13 §P0「`.jos`/`.joz` 会话」；容忍未知插件图层段）：
//!
//! ```xml
//! <?xml version="1.0" encoding="UTF-8"?>
//! <josm-session version="0.1">
//!   <viewport center="47.5000000,8.5000000" zoom="14.0"/>
//!   <layer name="data" type="osm-data" file="data.osm"/>
//!   <layer name="imagery" type="imagery" file=""/>
//!   <active-layer index="0"/>
//!   <selection>
//!     <item type="node" id="1"/>
//!     <item type="way" id="10"/>
//!   </selection>
//! </josm-session>
//! ```
//!
//! * 读取时**未知元素/未知属性直接忽略**（JOSM 插件会写自定义图层段）；
//! * `.joz` = ZIP 容器：`session.jos` + `layers/<序号>_<文件名>`，读取时解压到指定目录
//!   并把 `file` 路径改写为解压后的路径。
//!
//! 已知限制：不处理 JOSM 的 imagery 图层参数细节（只保留 name/type/file），
//! 不处理插件扩展数据。

use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};

use quick_xml::Reader as EventReader;
use quick_xml::Writer;
use quick_xml::events::{BytesEnd, BytesStart, BytesText, Event};
use zip::write::SimpleFileOptions;
use zip::{ZipArchive, ZipWriter};

/// 会话 schema 版本（与 JOSM 对齐的 0.1）。
pub const SESSION_VERSION: &str = "0.1";

/// 会话错误。
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum SessionError {
    /// XML 结构错误。
    #[error("session xml: {0}")]
    Xml(String),
    /// IO 错误。
    #[error("session io: {0}")]
    Io(#[from] std::io::Error),
    /// ZIP 错误。
    #[error("session zip: {0}")]
    Zip(String),
}

impl From<quick_xml::Error> for SessionError {
    fn from(value: quick_xml::Error) -> Self {
        Self::Xml(value.to_string())
    }
}

/// 会话中的一个图层引用。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SessionLayer {
    /// 显示名。
    pub name: String,
    /// 图层类型（`osm-data` / `imagery` / 其他插件类型）。
    pub layer_type: String,
    /// 数据文件（相对或绝对路径；`None` 表示空图层）。
    pub file: Option<PathBuf>,
}

/// 视口（JOSM 用 lat/lon 中心 + 缩放级别近似）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SessionViewport {
    /// 中心纬度（度）。
    pub center_lat: f64,
    /// 中心经度（度）。
    pub center_lon: f64,
    /// 缩放级别。
    pub zoom: f64,
}

/// 选中项。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SessionSelection {
    /// 类型（`node` / `way` / `relation`）。
    pub kind: &'static str,
    /// ID。
    pub id: i64,
}

/// JOSM 会话。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Session {
    /// 图层列表。
    pub layers: Vec<SessionLayer>,
    /// 视口。
    pub viewport: Option<SessionViewport>,
    /// 当前活动图层下标。
    pub active_layer: Option<usize>,
    /// 选择集。
    pub selection: Vec<SessionSelection>,
}

impl Session {
    /// 空会话。
    pub fn new() -> Self {
        Self::default()
    }

    /// 第一个可用的数据图层。
    pub fn first_data_layer(&self) -> Option<&SessionLayer> {
        self.layers
            .iter()
            .find(|l| l.layer_type == "osm-data" && l.file.is_some())
            .or_else(|| self.layers.iter().find(|l| l.file.is_some()))
    }
}

// ---------------------------------------------------------------------------
// .jos（XML）
// ---------------------------------------------------------------------------

/// 写 `.jos`。
pub fn write_jos<W: Write>(session: &Session, out: W) -> Result<(), SessionError> {
    let mut writer = Writer::new_with_indent(out, b' ', 2);
    writer.write_event(Event::Start(
        BytesStart::new("josm-session").with_attributes([("version", SESSION_VERSION)]),
    ))?;

    if let Some(viewport) = &session.viewport {
        writer.write_event(Event::Empty(BytesStart::new("viewport").with_attributes([
            (
                "center",
                format!("{:.7},{:.7}", viewport.center_lat, viewport.center_lon).as_str(),
            ),
            ("zoom", format!("{:.2}", viewport.zoom).as_str()),
        ])))?;
    }

    for layer in &session.layers {
        let mut start = BytesStart::new("layer");
        start.push_attribute(("name", layer.name.as_str()));
        start.push_attribute(("type", layer.layer_type.as_str()));
        if let Some(file) = &layer.file {
            start.push_attribute(("file", file.to_string_lossy().as_ref()));
        }
        writer.write_event(Event::Empty(start))?;
    }

    if let Some(active) = session.active_layer {
        writer.write_event(Event::Empty(
            BytesStart::new("active-layer").with_attributes([("index", active.to_string().as_str())]),
        ))?;
    }

    if !session.selection.is_empty() {
        writer.write_event(Event::Start(BytesStart::new("selection")))?;
        for item in &session.selection {
            writer.write_event(Event::Empty(
                BytesStart::new("item")
                    .with_attributes([("type", item.kind), ("id", item.id.to_string().as_str())]),
            ))?;
        }
        writer.write_event(Event::End(BytesEnd::new("selection")))?;
    }

    writer.write_event(Event::End(BytesEnd::new("josm-session")))?;
    Ok(())
}

/// 读 `.jos`。
pub fn read_jos<R: BufRead>(reader: R) -> Result<Session, SessionError> {
    let mut xml = EventReader::from_reader(reader);
    xml.config_mut().trim_text(true);
    let mut session = Session::new();
    let mut in_selection = false;
    let mut buf = Vec::new();

    loop {
        let event = xml.read_event_into(&mut buf)?;
        match event {
            Event::Start(e) | Event::Empty(e) => {
                let name = e.name();
                match name.as_ref() {
                    "viewport" => {
                        if let Some(center) = attr(&e, "center") {
                            let mut parts = center.split(',');
                            let lat = parts.next().and_then(|s| s.trim().parse::<f64>().ok());
                            let lon = parts.next().and_then(|s| s.trim().parse::<f64>().ok());
                            let valid = |value: f64, limit: f64| value.is_finite() && value.abs() <= limit;
                            if let (Some(lat), Some(lon)) = (lat, lon)
                                && valid(lat, 90.0)
                                && valid(lon, 180.0)
                            {
                                let zoom = attr(&e, "zoom")
                                    .and_then(|s| s.parse::<f64>().ok())
                                    .filter(|zoom| zoom.is_finite())
                                    .unwrap_or(14.0)
                                    .clamp(0.0, 22.0);
                                session.viewport = Some(SessionViewport {
                                    center_lat: lat,
                                    center_lon: lon,
                                    zoom,
                                });
                            }
                        }
                    }
                    "layer" => {
                        session.layers.push(SessionLayer {
                            name: attr(&e, "name").unwrap_or_default(),
                            layer_type: attr(&e, "type").unwrap_or_else(|| "osm-data".to_owned()),
                            file: attr(&e, "file").filter(|s| !s.is_empty()).map(PathBuf::from),
                        });
                    }
                    "active-layer" => {
                        session.active_layer = attr(&e, "index").and_then(|s| s.parse().ok());
                    }
                    "selection" => in_selection = true,
                    "item" if in_selection => {
                        let kind = match attr(&e, "type").as_deref() {
                            Some("node") => "node",
                            Some("way") => "way",
                            Some("relation") => "relation",
                            _ => continue,
                        };
                        if let Some(id) = attr(&e, "id").and_then(|s| s.parse::<i64>().ok()) {
                            session.selection.push(SessionSelection { kind, id });
                        }
                    }
                    // 未知元素（插件图层段等）：忽略
                    _ => {}
                }
            }
            Event::End(e) => {
                if e.name().as_ref() == "selection" {
                    in_selection = false;
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }
    Ok(session)
}

/// 从文件读 `.jos`。
pub fn read_jos_file(path: impl AsRef<Path>) -> Result<Session, SessionError> {
    let file = std::fs::File::open(path)?;
    read_jos(BufReader::new(file))
}

/// 写文件 `.jos`。
pub fn write_jos_file(session: &Session, path: impl AsRef<Path>) -> Result<(), SessionError> {
    let path = path.as_ref();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = temp_path(path);
    let result = (|| -> Result<(), SessionError> {
        let file = std::fs::File::create(&tmp)?;
        let mut buffered = std::io::BufWriter::new(file);
        write_jos(session, &mut buffered)?;
        buffered.flush()?;
        buffered.get_ref().sync_all()?;
        Ok(())
    })();
    if let Err(err) = result {
        let _ = std::fs::remove_file(&tmp);
        return Err(err);
    }
    finish_atomic_rename(&tmp, path)
}

fn temp_path(path: &Path) -> PathBuf {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "neo-session".into());
    path.with_file_name(format!(".{name}.{}.tmp", std::process::id()))
}

fn finish_atomic_rename(tmp: &Path, path: &Path) -> Result<(), SessionError> {
    match std::fs::rename(tmp, path) {
        Ok(()) => Ok(()),
        Err(first) => {
            if std::fs::remove_file(path).is_ok() {
                std::fs::rename(tmp, path).map_err(SessionError::Io)
            } else {
                let _ = std::fs::remove_file(tmp);
                Err(SessionError::Io(first))
            }
        }
    }
}

// ---------------------------------------------------------------------------
// .joz（ZIP）
// ---------------------------------------------------------------------------

/// `.joz` 解压限制：条目数、单文件、总解压量（防止 zip bomb）。
const MAX_JOZ_ENTRIES: usize = 4096;
const MAX_JOZ_SESSION_BYTES: u64 = 4 * 1024 * 1024;
const MAX_JOZ_LAYER_BYTES: u64 = 64 * 1024 * 1024;
const MAX_JOZ_TOTAL_BYTES: u64 = 256 * 1024 * 1024;

/// 写 `.joz`：把会话 XML 与图层文件一起打包。
pub fn write_joz(session: &Session, path: impl AsRef<Path>) -> Result<(), SessionError> {
    if let Some(parent) = path.as_ref().parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = temp_path(path.as_ref());
    let result = write_joz_inner(session, &tmp);
    if let Err(err) = result {
        let _ = std::fs::remove_file(&tmp);
        return Err(err);
    }
    finish_atomic_rename(&tmp, path.as_ref())
}

fn write_joz_inner(session: &Session, path: &Path) -> Result<(), SessionError> {
    let file = std::fs::File::create(path)?;
    let mut zip = ZipWriter::new(file);
    let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);

    // session.jos 里只写 zip 内的相对条目名，避免泄露本机绝对路径/用户名。
    let mut portable = session.clone();
    let mut payloads: Vec<(String, PathBuf)> = Vec::new();
    for (index, layer) in portable.layers.iter_mut().enumerate() {
        let Some(file_path) = layer.file.clone() else {
            continue;
        };
        if !file_path.exists() {
            layer.file = None;
            continue;
        }
        let name = file_path
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| format!("layer{index}"));
        let entry = format!("layers/{index}_{name}");
        layer.file = Some(PathBuf::from(&entry));
        payloads.push((entry, file_path));
    }

    zip.start_file("session.jos", options)
        .map_err(|e| SessionError::Zip(e.to_string()))?;
    let mut xml = Vec::new();
    write_jos(&portable, &mut xml)?;
    zip.write_all(&xml)?;

    for (entry, file_path) in payloads {
        zip.start_file(&entry, options)
            .map_err(|e| SessionError::Zip(e.to_string()))?;
        let bytes = std::fs::read(&file_path)?;
        zip.write_all(&bytes)?;
    }

    zip.finish().map_err(|e| SessionError::Zip(e.to_string()))?;
    Ok(())
}

/// 读 `.joz`：把图层文件安全解压到 `extract_dir`，返回改写路径后的会话。
///
/// 安全边界：拒绝绝对路径/盘符/`..`（zip slip）、限制条目数与解压总量
/// （zip bomb）；未在包内出现的图层把 `file` 置空，绝不回退到会话里记录的
/// 任意本机路径。
pub fn read_joz(path: impl AsRef<Path>, extract_dir: impl AsRef<Path>) -> Result<Session, SessionError> {
    let file = std::fs::File::open(path)?;
    let mut archive = ZipArchive::new(file).map_err(|e| SessionError::Zip(e.to_string()))?;
    if archive.len() > MAX_JOZ_ENTRIES {
        return Err(SessionError::Zip(format!(
            "too many entries: {} > {MAX_JOZ_ENTRIES}",
            archive.len()
        )));
    }
    std::fs::create_dir_all(extract_dir.as_ref())?;
    let root = extract_dir.as_ref();

    let mut session = Session::new();
    let mut extracted = std::collections::HashMap::<usize, PathBuf>::new();
    let mut total: u64 = 0;

    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|e| SessionError::Zip(e.to_string()))?;
        let name = entry.name().to_owned();
        if name == "session.jos" {
            if entry.size() > MAX_JOZ_SESSION_BYTES {
                return Err(SessionError::Zip("session.jos is too large".into()));
            }
            let mut xml = String::new();
            entry.read_to_string(&mut xml)?;
            session = read_jos(std::io::Cursor::new(xml))?;
            continue;
        }
        let Some(rest) = name.strip_prefix("layers/") else {
            continue;
        };
        // 只取文件名部分：`layers/0_../../x` 会被拒绝，而不是写到目录外。
        let Some(file_name) = Path::new(rest)
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|name| !name.is_empty() && *name != "." && *name != "..")
        else {
            return Err(SessionError::Zip(format!("unsafe layer entry {name:?}")));
        };
        if file_name.contains(['/', '\\', ':']) {
            return Err(SessionError::Zip(format!("unsafe layer entry {name:?}")));
        }
        let layer_index = rest.split('_').next().and_then(|v| v.parse::<usize>().ok());
        if entry.size() > MAX_JOZ_LAYER_BYTES {
            return Err(SessionError::Zip(format!("layer {name:?} is too large")));
        }
        total = total.saturating_add(entry.size());
        if total > MAX_JOZ_TOTAL_BYTES {
            return Err(SessionError::Zip("archive expands beyond size limit".into()));
        }
        let target = root.join(file_name);
        let mut out = std::fs::File::create(&target)?;
        // 双重上限：不信任 zip 头里的 size 字段。
        let copied = std::io::copy(&mut entry.by_ref().take(MAX_JOZ_LAYER_BYTES + 1), &mut out)?;
        if copied > MAX_JOZ_LAYER_BYTES {
            let _ = std::fs::remove_file(&target);
            return Err(SessionError::Zip(format!("layer {name:?} is too large")));
        }
        if let Some(layer_index) = layer_index {
            extracted.insert(layer_index, target);
        }
    }

    for (index, layer) in session.layers.iter_mut().enumerate() {
        layer.file = extracted.get(&index).cloned();
        if layer.file.is_none() {
            // 包内缺失的图层不能回退到会话 XML 里的任意本机路径。
            layer.file = None;
        }
    }
    Ok(session)
}

fn attr(e: &quick_xml::events::BytesStart<'_>, key: &str) -> Option<String> {
    e.attributes()
        .with_checks(false)
        .flatten()
        .find(|a| a.key.as_ref() == key)
        .and_then(|a| a.normalized_value(quick_xml::XmlVersion::Explicit1_0).ok())
        .map(|value| value.into_owned())
}

/// 把经纬度/缩放转换为会话视口（便于应用层调用）。
pub fn viewport_from_camera(center_lat: f64, center_lon: f64, zoom: f64) -> SessionViewport {
    SessionViewport {
        center_lat,
        center_lon,
        zoom,
    }
}

/// XML 文本（测试/调试用）。
pub fn to_xml_string(session: &Session) -> Result<String, SessionError> {
    let mut out = Vec::new();
    write_jos(session, &mut out)?;
    Ok(String::from_utf8_lossy(&out).into_owned())
}

#[allow(dead_code)]
fn _text(s: &str) -> BytesText<'_> {
    BytesText::new(s)
}
