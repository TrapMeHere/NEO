//! OSM XML 0.6 读写。
//!
//! 覆盖两种"合规模式"（docs/08 §2.6、docs/13 §3.2）：
//!
//! * **Server-conformant**：与 OSM API 交换用的标准 XML（`visible` 表示删除）；
//! * **JOSM-conformant**：本地 `.osm` 文件用 `action="modify|delete"` 表达未上传修改。
//!
//! 读取是流式的（quick-xml），不把整个文件读进内存；元素按出现顺序写入 [`DataSet`]。
//! 注意：XML 里 node 必须先于引用它的 way，否则 way 会被记为 skipped —— 这与 OSM
//! 官方导出顺序一致。

use std::io::{BufRead, Write};
use std::path::Path;

use neo_core::{Coord, LatLon, Meta, NeoError, ObjectId, ObjectKind, Sym};
use neo_osm::{ChangeState, Node, Relation, RelationMember, Tags, Way};
use neo_store::{DataSet, MissingPolicy};
use quick_xml::Reader as EventReader;
use quick_xml::XmlVersion;
use quick_xml::events::{BytesStart, Event};

/// XML 读取选项。
#[derive(Clone, Copy, Debug)]
pub struct XmlReadOptions {
    /// 引用缺失时跳过对象还是报错。
    pub policy: MissingPolicy,
}
impl Default for XmlReadOptions {
    fn default() -> Self {
        Self {
            policy: MissingPolicy::Skip,
        }
    }
}

/// XML 导入统计。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct XmlImportReport {
    /// 节点数。
    pub nodes: u64,
    /// Way 数。
    pub ways: u64,
    /// Relation 数。
    pub relations: u64,
    /// 被跳过的 Way（引用缺失）。
    pub skipped_ways: u64,
    /// 被跳过的 Relation。
    pub skipped_relations: u64,
    /// 遇到的 `<bounds>`（min_lat, min_lon, max_lat, max_lon）。
    pub bounds: Option<[f64; 4]>,
}

impl core::fmt::Display for XmlImportReport {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "XML import: {} nodes, {} ways, {} relations ({} / {} skipped)",
            self.nodes, self.ways, self.relations, self.skipped_ways, self.skipped_relations
        )
    }
}

/// 写出模式。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum XmlWriteMode {
    /// OSM API 风格：`visible="false"` 表示删除，不写 `action`。
    Server,
    /// JOSM 风格：`action="modify|delete"` 表达未上传修改。
    Josm,
}

// ---------------------------------------------------------------------------
// 读取
// ---------------------------------------------------------------------------

#[derive(Default)]
struct PendingNode {
    id: i64,
    lat: i32,
    lon: i32,
    tags: Vec<(String, String)>,
    meta: Meta,
    user: String,
    visible: bool,
    action: Option<Action>,
}

#[derive(Default)]
struct PendingWay {
    id: i64,
    tags: Vec<(String, String)>,
    refs: Vec<i64>,
    meta: Meta,
    user: String,
    visible: bool,
    action: Option<Action>,
}

#[derive(Default)]
struct PendingRelation {
    id: i64,
    tags: Vec<(String, String)>,
    members: Vec<RelationMemberDraft>,
    meta: Meta,
    user: String,
    visible: bool,
    action: Option<Action>,
}

/// JOSM `.osm` 的未上传动作。
#[derive(Clone, Copy, PartialEq, Eq)]
enum Action {
    Modify,
    Delete,
}

impl Action {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "modify" => Some(Self::Modify),
            "delete" => Some(Self::Delete),
            _ => None,
        }
    }
}

struct RelationMemberDraft {
    kind: &'static str,
    id: i64,
    role: String,
}

enum Pending {
    Node(PendingNode),
    Way(PendingWay),
    Relation(PendingRelation),
}

/// 从 XML 读取器导入 OSM 数据。
pub fn read_osm_xml<R: BufRead>(
    data: &mut DataSet,
    reader: R,
    opts: XmlReadOptions,
) -> Result<XmlImportReport, NeoError> {
    let mut xml = EventReader::from_reader(reader);
    xml.config_mut().trim_text(true);
    let mut buf = Vec::new();
    let mut report = XmlImportReport::default();
    let mut pending: Option<Pending> = None;
    let mut seen_root = false;
    let mut closed_root = false;

    loop {
        let event = xml
            .read_event_into(&mut buf)
            .map_err(|e| NeoError::Parse(format!("XML: {e}")))?;
        let is_empty_event = matches!(&event, Event::Empty(_));
        match event {
            Event::Start(e) | Event::Empty(e) => {
                let is_empty = is_empty_event;
                let name = e.name();
                if !seen_root {
                    if name.as_ref() != "osm" {
                        return Err(NeoError::Parse(format!(
                            "expected <osm> root element, found <{}>",
                            name.as_ref()
                        )));
                    }
                    seen_root = true;
                    if is_empty {
                        closed_root = true;
                    }
                }
                match name.as_ref() {
                    "bounds" if pending.is_none() => {
                        report.bounds = read_bounds(&e);
                    }
                    "node" if pending.is_none() => {
                        let node = parse_node(&e)?;
                        if is_empty {
                            finish_node(data, node, &opts, &mut report)?;
                        } else {
                            pending = Some(Pending::Node(node));
                        }
                    }
                    "way" if pending.is_none() => {
                        let way = parse_way(&e)?;
                        if is_empty {
                            finish_way(data, way, &opts, &mut report)?;
                        } else {
                            pending = Some(Pending::Way(way));
                        }
                    }
                    "relation" if pending.is_none() => {
                        let rel = parse_relation(&e)?;
                        if is_empty {
                            finish_relation(data, rel, &opts, &mut report)?;
                        } else {
                            pending = Some(Pending::Relation(rel));
                        }
                    }
                    "node" | "way" | "relation" => {
                        return Err(NeoError::Parse("nested OSM element".into()));
                    }
                    "tag" => {
                        if let Some(current) = &mut pending {
                            let (k, v) = parse_tag(&e);
                            match current {
                                Pending::Node(n) => n.tags.push((k, v)),
                                Pending::Way(w) => w.tags.push((k, v)),
                                Pending::Relation(r) => r.tags.push((k, v)),
                            }
                        }
                    }
                    "nd" => {
                        if let Some(Pending::Way(w)) = &mut pending {
                            let reference = attr_i64(&e, "ref")
                                .ok_or_else(|| NeoError::Parse("<nd> missing or invalid ref".into()))?;
                            w.refs.push(reference);
                        }
                    }
                    "member" => {
                        if let Some(Pending::Relation(r)) = &mut pending
                            && let (Some(kind), Some(id)) = (attr_str(&e, "type"), attr_i64(&e, "ref"))
                        {
                            let kind: &'static str = match kind.as_str() {
                                "node" => "node",
                                "way" => "way",
                                "relation" => "relation",
                                other => {
                                    return Err(NeoError::Parse(format!(
                                        "invalid relation member type {other:?}"
                                    )));
                                }
                            };
                            r.members.push(RelationMemberDraft {
                                kind,
                                id,
                                role: attr_str(&e, "role").unwrap_or_default(),
                            });
                        }
                    }
                    _ => {}
                }
            }
            Event::End(e) => match e.name().as_ref() {
                "node" => {
                    if let Some(Pending::Node(n)) = pending.take() {
                        finish_node(data, n, &opts, &mut report)?;
                    }
                }
                "way" => {
                    if let Some(Pending::Way(w)) = pending.take() {
                        finish_way(data, w, &opts, &mut report)?;
                    }
                }
                "relation" => {
                    if let Some(Pending::Relation(r)) = pending.take() {
                        finish_relation(data, r, &opts, &mut report)?;
                    }
                }
                "osm" => {
                    closed_root = true;
                    break;
                }
                _ => {}
            },
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }
    if !seen_root {
        return Err(NeoError::Parse("empty or non-OSM XML".into()));
    }
    if !closed_root {
        return Err(NeoError::Parse(
            "truncated OSM XML: missing </osm> (file may be incomplete)".into(),
        ));
    }
    Ok(report)
}

/// 从文件读取。
pub fn read_osm_xml_file(data: &mut DataSet, path: impl AsRef<Path>) -> Result<XmlImportReport, NeoError> {
    let file = std::fs::File::open(path.as_ref())?;
    let reader = std::io::BufReader::new(file);
    read_osm_xml(data, reader, XmlReadOptions::default())
}

fn finish_node(
    data: &mut DataSet,
    n: PendingNode,
    _opts: &XmlReadOptions,
    report: &mut XmlImportReport,
) -> Result<(), NeoError> {
    let tags = intern_tags(data, &n.tags);
    let mut meta = n.meta;
    meta.user = data.intern(&n.user);
    let deleted = !n.visible || n.action == Some(Action::Delete);
    if deleted {
        meta.visible = false;
    }
    let id = ObjectId(n.id);
    data.push_node(Node {
        id,
        pos: LatLon::new(Coord::from_units(n.lat), Coord::from_units(n.lon)),
        tags,
        meta,
    })?;
    if deleted {
        data.set_change_state(neo_core::ObjectKind::Node, id, ChangeState::Deleted);
    } else if id.is_local() {
        // JOSM 的 `.osm` 里负 ID 表示未上传的新对象；不标 Added 会导致导出
        // osmChange 时完全看不到它（新对象永远传不上去）。
        data.set_change_state(neo_core::ObjectKind::Node, id, ChangeState::Added);
    } else if n.action == Some(Action::Modify) {
        data.set_change_state(neo_core::ObjectKind::Node, id, ChangeState::Modified);
    }
    report.nodes += 1;
    Ok(())
}

fn finish_way(
    data: &mut DataSet,
    w: PendingWay,
    opts: &XmlReadOptions,
    report: &mut XmlImportReport,
) -> Result<(), NeoError> {
    let tags = intern_tags(data, &w.tags);
    let mut meta = w.meta;
    meta.user = data.intern(&w.user);
    let id = ObjectId(w.id);
    let deleted = !w.visible || w.action == Some(Action::Delete);
    if deleted {
        meta.visible = false;
    }
    let way = Way {
        id,
        nodes: w.refs.into_iter().map(ObjectId).collect(),
        tags,
        meta,
    };
    match data.push_way(way, opts.policy)? {
        Some(_) => {
            if deleted {
                data.set_change_state(neo_core::ObjectKind::Way, id, ChangeState::Deleted);
            } else if id.is_local() {
                data.set_change_state(neo_core::ObjectKind::Way, id, ChangeState::Added);
            } else if w.action == Some(Action::Modify) {
                data.set_change_state(neo_core::ObjectKind::Way, id, ChangeState::Modified);
            }
            report.ways += 1;
        }
        None => report.skipped_ways += 1,
    }
    Ok(())
}

fn finish_relation(
    data: &mut DataSet,
    r: PendingRelation,
    opts: &XmlReadOptions,
    report: &mut XmlImportReport,
) -> Result<(), NeoError> {
    let tags = intern_tags(data, &r.tags);
    let members = r
        .members
        .iter()
        .filter_map(|m| {
            let kind = neo_core::ObjectKind::from_type_name(m.kind)?;
            Some(RelationMember {
                kind,
                id: ObjectId(m.id),
                role: data.intern(&m.role),
            })
        })
        .collect();
    let mut meta = r.meta;
    meta.user = data.intern(&r.user);
    let id = ObjectId(r.id);
    let deleted = !r.visible || r.action == Some(Action::Delete);
    if deleted {
        meta.visible = false;
    }
    let rel = Relation {
        id,
        members,
        tags,
        meta,
    };
    match data.push_relation(rel, opts.policy)? {
        Some(_) => {
            if deleted {
                data.set_change_state(neo_core::ObjectKind::Relation, id, ChangeState::Deleted);
            } else if id.is_local() {
                data.set_change_state(neo_core::ObjectKind::Relation, id, ChangeState::Added);
            } else if r.action == Some(Action::Modify) {
                data.set_change_state(neo_core::ObjectKind::Relation, id, ChangeState::Modified);
            }
            report.relations += 1;
        }
        None => report.skipped_relations += 1,
    }
    Ok(())
}

fn intern_tags(data: &mut DataSet, pairs: &[(String, String)]) -> Tags {
    let mut out = Vec::with_capacity(pairs.len());
    for (k, v) in pairs {
        out.push((data.intern(k), data.intern(v)));
    }
    Tags::from_pairs(out)
}

fn parse_node(e: &BytesStart) -> Result<PendingNode, NeoError> {
    let id = attr_i64(e, "id").ok_or_else(|| NeoError::Parse("node missing id".into()))?;
    let visible = attr_bool(e, "visible").unwrap_or(true);
    let action = attr_str(e, "action").as_deref().and_then(Action::parse);
    // 删除片段可以省略坐标；可见节点必须提供合法坐标。缺省会静默落到 (0,0)，
    // 在真实数据里表现为"内罗毕湾多了一个点"，比直接报错危险得多。
    let deleted = !visible || action == Some(Action::Delete);
    let (lat, lon) = match (attr_f64(e, "lat"), attr_f64(e, "lon")) {
        (Some(lat), Some(lon)) => {
            let pos = LatLon::try_from_degrees(lat, lon).ok_or_else(|| {
                NeoError::Parse(format!("node {id} has out-of-range coordinates {lat},{lon}"))
            })?;
            (pos.lat.units(), pos.lon.units())
        }
        _ if deleted => (0, 0),
        _ => {
            return Err(NeoError::Parse(format!("node {id} missing or invalid lat/lon")));
        }
    };
    Ok(PendingNode {
        id,
        lat,
        lon,
        tags: Vec::new(),
        meta: parse_meta(e)?,
        user: attr_str(e, "user").unwrap_or_default(),
        visible,
        action,
    })
}

fn parse_way(e: &BytesStart) -> Result<PendingWay, NeoError> {
    Ok(PendingWay {
        id: attr_i64(e, "id").ok_or_else(|| NeoError::Parse("way missing id".into()))?,
        tags: Vec::new(),
        refs: Vec::new(),
        meta: parse_meta(e)?,
        user: attr_str(e, "user").unwrap_or_default(),
        visible: attr_bool(e, "visible").unwrap_or(true),
        action: attr_str(e, "action").as_deref().and_then(Action::parse),
    })
}

fn parse_relation(e: &BytesStart) -> Result<PendingRelation, NeoError> {
    Ok(PendingRelation {
        id: attr_i64(e, "id").ok_or_else(|| NeoError::Parse("relation missing id".into()))?,
        tags: Vec::new(),
        members: Vec::new(),
        meta: parse_meta(e)?,
        user: attr_str(e, "user").unwrap_or_default(),
        visible: attr_bool(e, "visible").unwrap_or(true),
        action: attr_str(e, "action").as_deref().and_then(Action::parse),
    })
}

fn parse_meta(e: &BytesStart) -> Result<Meta, NeoError> {
    // OSM XML 的时间戳是 ISO-8601；旧实现按整数解析，真实文件全部静默变 0。
    let timestamp = match attr_str(e, "timestamp") {
        Some(text) if !text.trim().is_empty() => crate::time::parse_timestamp(text.trim())
            .ok_or_else(|| NeoError::Parse(format!("invalid timestamp {text:?}")))?,
        _ => 0,
    };
    Ok(Meta {
        version: attr_i32(e, "version").unwrap_or(0).max(0) as u32,
        timestamp,
        changeset: attr_i64(e, "changeset").unwrap_or(0),
        uid: attr_i32(e, "uid").unwrap_or(0).max(0) as u32,
        user: Sym::EMPTY,
        visible: attr_bool(e, "visible").unwrap_or(true),
    })
}

fn parse_tag(e: &BytesStart) -> (String, String) {
    (
        attr_str(e, "k").unwrap_or_default(),
        attr_str(e, "v").unwrap_or_default(),
    )
}

fn read_bounds(e: &BytesStart) -> Option<[f64; 4]> {
    let min_lat = attr_f64(e, "minlat")?;
    let min_lon = attr_f64(e, "minlon")?;
    let max_lat = attr_f64(e, "maxlat")?;
    let max_lon = attr_f64(e, "maxlon")?;
    Some([min_lat, min_lon, max_lat, max_lon])
}

fn attr_bytes(e: &BytesStart, key: &str) -> Option<String> {
    e.attributes()
        .with_checks(false)
        .flatten()
        .find(|a| a.key.as_ref() == key)
        .and_then(|a| a.normalized_value(XmlVersion::Explicit1_0).ok())
        .map(|value| value.into_owned())
}

fn attr_str(e: &BytesStart, key: &str) -> Option<String> {
    attr_bytes(e, key)
}

fn attr_i64(e: &BytesStart, key: &str) -> Option<i64> {
    attr_str(e, key)?.trim().parse().ok()
}

fn attr_i32(e: &BytesStart, key: &str) -> Option<i32> {
    attr_str(e, key)?.trim().parse().ok()
}

fn attr_f64(e: &BytesStart, key: &str) -> Option<f64> {
    attr_str(e, key)?.trim().parse().ok()
}

fn attr_bool(e: &BytesStart, key: &str) -> Option<bool> {
    match attr_str(e, key)?.trim() {
        "true" | "1" => Some(true),
        "false" | "0" => Some(false),
        _ => None,
    }
}

// 属性值由 quick-xml 的 `normalized_value` 解码（含 XML 属性值归一化），
// 不再手写实体解码：手写版本不处理 `\t/\n/\r` 归一化，也未限制实体嵌套深度。

// ---------------------------------------------------------------------------
// 写出
// ---------------------------------------------------------------------------

/// 把数据集写成 OSM XML 0.6。
pub fn write_osm_xml<W: Write>(data: &DataSet, out: &mut W, mode: XmlWriteMode) -> std::io::Result<()> {
    writeln!(out, "<?xml version=\"1.0\" encoding=\"UTF-8\"?>")?;
    writeln!(out, "<osm version=\"0.6\" generator=\"NEO\">")?;

    for node in data.iter_nodes() {
        let view = data.node_view(node.handle);
        let Some(view) = view else { continue };
        let deleted = data.change_state(neo_core::ObjectKind::Node, node.id) == ChangeState::Deleted;
        let (lat, lon) = node.pos.to_degrees();
        write!(
            out,
            "  <node id=\"{}\" lat=\"{lat:.7}\" lon=\"{lon:.7}\"",
            node.id.0
        )?;
        write_meta(
            data,
            out,
            ObjectKind::Node,
            node.id,
            &view.info.meta,
            deleted,
            mode,
        )?;
        write_tags(data, out, view.tag_pairs)?;
        writeln!(out, "  </node>")?;
    }

    for way in data.iter_ways() {
        let Some(view) = data.way_view(way.handle) else {
            continue;
        };
        let deleted = data.change_state(neo_core::ObjectKind::Way, way.id) == ChangeState::Deleted;
        write!(out, "  <way id=\"{}\"", way.id.0)?;
        write_meta(data, out, ObjectKind::Way, way.id, &view.info.meta, deleted, mode)?;
        write_tags(data, out, view.tag_pairs)?;
        for node_handle in view.node_refs {
            if let Some(id) = data.nodes.id(*node_handle) {
                writeln!(out, "    <nd ref=\"{}\"/>", id.0)?;
            }
        }
        writeln!(out, "  </way>")?;
    }

    for rel in data.iter_relations() {
        let Some(view) = data.relation_view(rel.handle) else {
            continue;
        };
        let deleted = data.change_state(neo_core::ObjectKind::Relation, rel.id) == ChangeState::Deleted;
        write!(out, "  <relation id=\"{}\"", rel.id.0)?;
        write_meta(
            data,
            out,
            ObjectKind::Relation,
            rel.id,
            &view.info.meta,
            deleted,
            mode,
        )?;
        write_tags(data, out, view.tag_pairs)?;
        for member in view.members {
            let target_id = match member.target.kind {
                neo_core::ObjectKind::Node => data
                    .nodes
                    .id(member.target.as_node().unwrap_or(neo_core::Handle::new(0, 0))),
                neo_core::ObjectKind::Way => data
                    .ways
                    .id(member.target.as_way().unwrap_or(neo_core::Handle::new(0, 0))),
                neo_core::ObjectKind::Relation => data
                    .relations
                    .id(member.target.as_relation().unwrap_or(neo_core::Handle::new(0, 0))),
                neo_core::ObjectKind::Changeset => None,
            };
            if let Some(target_id) = target_id {
                writeln!(
                    out,
                    "    <member type=\"{}\" ref=\"{}\" role=\"{}\"/>",
                    member.target.kind.type_name(),
                    target_id.0,
                    escape_xml(data.resolve(member.role)),
                )?;
            }
        }
        writeln!(out, "  </relation>")?;
    }

    writeln!(out, "</osm>")?;
    Ok(())
}

/// 写到文件。
///
/// 先写同目录临时文件再 rename：写一半失败/磁盘满时不会把已有文件截断成垃圾
/// （Windows 上 rename 覆盖需要先删除目标；这里在 rename 失败时回退到该路径）。
pub fn write_osm_xml_file(data: &DataSet, path: impl AsRef<Path>, mode: XmlWriteMode) -> std::io::Result<()> {
    let path = path.as_ref();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = temp_path(path);
    {
        let file = std::fs::File::create(&tmp)?;
        let mut writer = std::io::BufWriter::new(file);
        let result = write_osm_xml(data, &mut writer, mode).and_then(|()| writer.flush());
        if let Err(err) = result {
            let _ = std::fs::remove_file(&tmp);
            return Err(err);
        }
        writer.get_ref().sync_all()?;
    }
    finish_atomic_rename(&tmp, path)
}

pub(crate) fn temp_path(path: &Path) -> std::path::PathBuf {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "neo-export".into());
    path.with_file_name(format!(".{name}.{}.tmp", std::process::id()))
}

pub(crate) fn finish_atomic_rename(tmp: &Path, path: &Path) -> std::io::Result<()> {
    match std::fs::rename(tmp, path) {
        Ok(()) => Ok(()),
        Err(first) => {
            // Windows 的 rename 不覆盖已存在文件：退化为"删除目标后重命名"。
            if std::fs::remove_file(path).is_ok() {
                std::fs::rename(tmp, path)
            } else {
                let _ = std::fs::remove_file(tmp);
                Err(first)
            }
        }
    }
}

fn write_meta<W: Write>(
    data: &DataSet,
    out: &mut W,
    kind: ObjectKind,
    id: ObjectId,
    meta: &Meta,
    deleted: bool,
    mode: XmlWriteMode,
) -> std::io::Result<()> {
    write!(out, " version=\"{}\"", meta.version)?;
    if meta.changeset > 0 {
        write!(out, " changeset=\"{}\"", meta.changeset)?;
    }
    // OSM XML 0.6 要求 ISO-8601；写 Unix 秒会让 JOSM/API 拒绝解析。
    if meta.timestamp != 0 {
        write!(
            out,
            " timestamp=\"{}\"",
            crate::time::format_timestamp(meta.timestamp)
        )?;
    }
    let user = data.resolve(meta.user);
    if !user.is_empty() {
        write!(out, " user=\"{}\"", escape_xml(user))?;
    }
    if meta.uid != 0 {
        write!(out, " uid=\"{}\"", meta.uid)?;
    }
    match mode {
        XmlWriteMode::Server => {
            if deleted || !meta.visible {
                write!(out, " visible=\"false\"")?;
            }
        }
        XmlWriteMode::Josm => {
            if deleted {
                write!(out, " action=\"delete\" visible=\"false\"")?;
            } else if data.change_state(kind, id) == ChangeState::Modified {
                write!(out, " action=\"modify\"")?;
            }
        }
    }
    Ok(())
}

fn write_tags<W: Write>(data: &DataSet, out: &mut W, tag_pairs: &[(Sym, Sym)]) -> std::io::Result<()> {
    if tag_pairs.is_empty() {
        writeln!(out, ">")?;
        return Ok(());
    }
    writeln!(out, ">")?;
    for (k, v) in tag_pairs {
        writeln!(
            out,
            "    <tag k=\"{}\" v=\"{}\"/>",
            escape_xml(data.resolve(*k)),
            escape_xml(data.resolve(*v))
        )?;
    }
    Ok(())
}

fn escape_xml(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            // XML 1.0 不允许的码位：直接丢弃，避免写出非法文件。
            '\u{0}'..='\u{8}' | '\u{B}' | '\u{C}' | '\u{E}'..='\u{1F}' | '\u{FFFE}' | '\u{FFFF}' => {}
            _ => out.push(ch),
        }
    }
    out
}
