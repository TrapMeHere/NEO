//! osmChange 0.6 读取与应用（上传 diff / 改动包导入）。
//!
//! 输入形如：
//! ```xml
//! <osmChange version="0.6">
//!   <create><node id="-1" lat=".." lon=".."/></create>
//!   <modify><node id="10" .../></modify>
//!   <delete><way id="20"/></delete>
//! </osmChange>
//! ```
//! 语义：create → `ChangeState::Added`；modify → `Modified`；delete → `Deleted` 且
//! `visible=false`（对象保留在 store 中，供撤销与再次导出）。
//!
//! 注意：不存在 "OSC 0.3"；文件根元素版本是 0.6（docs/08）。

use std::io::BufRead;
use std::path::Path;

use neo_core::{Coord, LatLon, Meta, NeoError, ObjectId};
use neo_osm::{ChangeState, Node, Relation, RelationMember, Tags, Way};
use neo_store::{AnyHandle, DataSet, MissingPolicy, StoredMember};
use quick_xml::Reader as EventReader;
use quick_xml::XmlVersion;
use quick_xml::events::{BytesStart, Event};

/// 应用统计。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OscApplyReport {
    /// 新建节点/way/relation。
    pub created: [u64; 3],
    /// 修改节点/way/relation。
    pub modified: [u64; 3],
    /// 删除节点/way/relation。
    pub deleted: [u64; 3],
    /// 目标不存在而忽略的删除/修改。
    pub missing: u64,
}

impl core::fmt::Display for OscApplyReport {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "osmChange applied: +{} ~{} -{} elements (+{}/{}/{}, ~{}/{}/{}, -{}/{}/{}, missing {})",
            self.created.iter().sum::<u64>(),
            self.modified.iter().sum::<u64>(),
            self.deleted.iter().sum::<u64>(),
            self.created[0],
            self.created[1],
            self.created[2],
            self.modified[0],
            self.modified[1],
            self.modified[2],
            self.deleted[0],
            self.deleted[1],
            self.deleted[2],
            self.missing
        )
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Block {
    None,
    Create,
    Modify,
    Delete,
}

struct Draft {
    kind: &'static str,
    id: i64,
    lat: i32,
    lon: i32,
    tags: Vec<(String, String)>,
    refs: Vec<i64>,
    members: Vec<(String, i64, String)>,
    meta: Meta,
}

/// 从 osmChange XML 中读取并应用到数据集。
pub fn read_osm_change<R: BufRead>(
    data: &mut DataSet,
    reader: R,
    policy: MissingPolicy,
) -> Result<OscApplyReport, NeoError> {
    let mut xml = EventReader::from_reader(reader);
    xml.config_mut().trim_text(true);
    let mut buf = Vec::new();
    let mut block = Block::None;
    let mut draft: Option<Draft> = None;
    let mut report = OscApplyReport::default();
    let mut seen_root = false;
    let mut closed_root = false;

    loop {
        let event = xml
            .read_event_into(&mut buf)
            .map_err(|e| NeoError::Parse(format!("osmChange: {e}")))?;
        let empty = matches!(&event, Event::Empty(_));
        match event {
            Event::Start(e) | Event::Empty(e) => {
                let name = e.name();
                if !seen_root {
                    if name.as_ref() != "osmChange" {
                        return Err(NeoError::Parse(format!(
                            "expected <osmChange> root, found <{}>",
                            name.as_ref()
                        )));
                    }
                    seen_root = true;
                }
                match name.as_ref() {
                    "create" | "modify" | "delete" => {
                        // 自闭合的空块不改变当前状态（否则 `<delete/>` 之后的所有
                        // 元素都会被当成 delete 执行）。
                        if !empty {
                            block = match name.as_ref() {
                                "create" => Block::Create,
                                "modify" => Block::Modify,
                                _ => Block::Delete,
                            };
                        }
                    }
                    "node" | "way" | "relation" if draft.is_none() => {
                        draft = Some(parse_draft(name.as_ref(), &e)?);
                        if empty {
                            apply_draft(data, draft.take().unwrap(), block, policy, &mut report)?;
                        }
                    }
                    "tag" => {
                        if let Some(d) = draft.as_mut() {
                            d.tags.push((
                                attr_str(&e, "k").unwrap_or_default(),
                                attr_str(&e, "v").unwrap_or_default(),
                            ));
                        }
                    }
                    "nd" => {
                        if let Some(d) = draft.as_mut()
                            && let Some(reference) = attr_i64(&e, "ref")
                        {
                            d.refs.push(reference);
                        }
                    }
                    "member" => {
                        if let Some(d) = draft.as_mut() {
                            let kind = attr_str(&e, "type").unwrap_or_default();
                            if let Some(id) = attr_i64(&e, "ref") {
                                d.members
                                    .push((kind, id, attr_str(&e, "role").unwrap_or_default()));
                            }
                        }
                    }
                    _ => {}
                }
            }
            Event::End(e) => match e.name().as_ref() {
                "create" | "modify" | "delete" => block = Block::None,
                "osmChange" => {
                    closed_root = true;
                    break;
                }
                "node" | "way" | "relation" => {
                    if let Some(d) = draft.take() {
                        apply_draft(data, d, block, policy, &mut report)?;
                    }
                }
                _ => {}
            },
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }
    if !seen_root {
        return Err(NeoError::Parse("empty or non-osmChange XML".into()));
    }
    if !closed_root {
        return Err(NeoError::Parse(
            "truncated osmChange: missing </osmChange>".into(),
        ));
    }
    Ok(report)
}

/// 从文件读取并应用。
pub fn read_osm_change_file(data: &mut DataSet, path: impl AsRef<Path>) -> Result<OscApplyReport, NeoError> {
    let file = std::fs::File::open(path.as_ref())?;
    read_osm_change(data, std::io::BufReader::new(file), MissingPolicy::Skip)
}

fn parse_draft(kind: &str, e: &BytesStart) -> Result<Draft, NeoError> {
    let kind: &'static str = match kind {
        "node" => "node",
        "way" => "way",
        _ => "relation",
    };
    let id = attr_i64(e, "id").ok_or_else(|| NeoError::Parse("element missing id".into()))?;
    let visible = attr_bool(e, "visible").unwrap_or(true);
    let (lat, lon) = match (attr_f64(e, "lat"), attr_f64(e, "lon")) {
        (Some(lat), Some(lon)) => {
            // create/modify 的坐标必须合法；delete 可以省略。
            let pos = LatLon::try_from_degrees(lat, lon).ok_or_else(|| {
                NeoError::Parse(format!("element {id} has out-of-range coordinates {lat},{lon}"))
            })?;
            (pos.lat.units(), pos.lon.units())
        }
        _ if kind == "node" && !visible => (0, 0),
        _ if kind == "node" => {
            return Err(NeoError::Parse(format!("node {id} missing or invalid lat/lon")));
        }
        _ => (0, 0),
    };
    Ok(Draft {
        kind,
        id,
        lat,
        lon,
        tags: Vec::new(),
        refs: Vec::new(),
        members: Vec::new(),
        meta: parse_meta(e)?,
    })
}

fn apply_draft(
    data: &mut DataSet,
    d: Draft,
    block: Block,
    policy: MissingPolicy,
    report: &mut OscApplyReport,
) -> Result<(), NeoError> {
    let id = ObjectId(d.id);
    match d.kind {
        "node" => match block {
            Block::Create => {
                let mut meta = d.meta;
                meta.visible = true;
                let tags = make_tags(data, &d.tags);
                data.push_node(Node {
                    id,
                    pos: LatLon::new(Coord::from_units(d.lat), Coord::from_units(d.lon)),
                    tags,
                    meta,
                })?;
                data.set_change_state(neo_core::ObjectKind::Node, id, ChangeState::Added);
                report.created[0] += 1;
            }
            Block::Modify => {
                let Some(handle) = data.node_handle(id) else {
                    report.missing += 1;
                    return Ok(());
                };
                let previous_user = data.nodes.get(handle).map(|view| view.meta.user);
                data.nodes.set_position(
                    handle,
                    LatLon::new(Coord::from_units(d.lat), Coord::from_units(d.lon)),
                );
                let pairs = intern_pairs(data, &d.tags);
                let tag_id = data.add_tags(&pairs);
                data.nodes.set_tags(handle, tag_id);
                let mut meta = d.meta;
                // 不要用空串覆盖服务端 user；OSC 的 user 属性通常省略。
                meta.user = data.intern("");
                if let Some(user) = previous_user {
                    meta.user = user;
                }
                data.nodes.set_meta(handle, meta);
                data.set_change_state(neo_core::ObjectKind::Node, id, ChangeState::Modified);
                report.modified[0] += 1;
            }
            Block::Delete => {
                if data.node_handle(id).is_none() {
                    report.missing += 1;
                    return Ok(());
                }
                data.set_change_state_visible(neo_core::ObjectKind::Node, id, ChangeState::Deleted, false);
                report.deleted[0] += 1;
            }
            Block::None => {}
        },
        "way" => match block {
            Block::Create | Block::Modify => {
                if let Some(handle) = data.way_handle(id) {
                    // 修改已有 way：缺失引用不能静默丢掉（会让上传的几何被截短）。
                    let mut handles = Vec::with_capacity(d.refs.len());
                    for r in &d.refs {
                        match data.node_handle(ObjectId(*r)) {
                            Some(node) => handles.push(node),
                            None => match policy {
                                MissingPolicy::Error => {
                                    return Err(NeoError::DanglingReference {
                                        kind: neo_core::ObjectKind::Node,
                                        id: ObjectId(*r),
                                        referrer_kind: neo_core::ObjectKind::Way,
                                        referrer: id,
                                    });
                                }
                                MissingPolicy::Skip => {
                                    report.missing += 1;
                                    return Ok(());
                                }
                            },
                        }
                    }
                    let pairs = intern_pairs(data, &d.tags);
                    let tag_id = data.add_tags(&pairs);
                    data.set_way_nodes(handle, &handles)?;
                    data.ways.set_tags(handle, tag_id);
                    let mut meta = d.meta;
                    if let Some(previous) = data.ways.get(handle) {
                        meta.user = previous.meta.user;
                    }
                    data.ways.set_meta(handle, meta);
                    if data.change_state(neo_core::ObjectKind::Way, id) != ChangeState::Added {
                        data.set_change_state(neo_core::ObjectKind::Way, id, ChangeState::Modified);
                    }
                    report.modified[1] += 1;
                } else {
                    let way = Way {
                        id,
                        nodes: d.refs.iter().map(|r| ObjectId(*r)).collect(),
                        tags: make_tags(data, &d.tags),
                        meta: d.meta,
                    };
                    match data.push_way(way, policy)? {
                        Some(_) => {
                            data.set_change_state(
                                neo_core::ObjectKind::Way,
                                id,
                                if block == Block::Create {
                                    ChangeState::Added
                                } else {
                                    ChangeState::Modified
                                },
                            );
                            if block == Block::Create {
                                report.created[1] += 1;
                            } else {
                                report.modified[1] += 1;
                            }
                        }
                        None => report.missing += 1,
                    }
                }
            }
            Block::Delete => {
                if data.way_handle(id).is_none() {
                    report.missing += 1;
                    return Ok(());
                }
                data.set_change_state_visible(neo_core::ObjectKind::Way, id, ChangeState::Deleted, false);
                report.deleted[1] += 1;
            }
            Block::None => {}
        },
        _ => match block {
            Block::Create | Block::Modify => {
                let mut members: smallvec::SmallVec<[RelationMember; 4]> = smallvec::SmallVec::new();
                for (kind, member_id, role) in &d.members {
                    let kind = neo_core::ObjectKind::from_type_name(kind)
                        .ok_or_else(|| NeoError::Parse(format!("invalid relation member type {kind:?}")))?;
                    members.push(RelationMember {
                        kind,
                        id: ObjectId(*member_id),
                        role: data.intern(role),
                    });
                }
                if let Some(handle) = data.relation_handle(id) {
                    let mut stored: Vec<StoredMember> = Vec::with_capacity(members.len());
                    for m in &members {
                        let target = match m.kind {
                            neo_core::ObjectKind::Node => data.ids.node(m.id).map(AnyHandle::from_node),
                            neo_core::ObjectKind::Way => data.ids.way(m.id).map(AnyHandle::from_way),
                            neo_core::ObjectKind::Relation => {
                                data.ids.relation(m.id).map(AnyHandle::from_relation)
                            }
                            neo_core::ObjectKind::Changeset => None,
                        };
                        match target {
                            Some(target) => stored.push(StoredMember { role: m.role, target }),
                            None => match policy {
                                MissingPolicy::Error => {
                                    return Err(NeoError::DanglingReference {
                                        kind: m.kind,
                                        id: m.id,
                                        referrer_kind: neo_core::ObjectKind::Relation,
                                        referrer: id,
                                    });
                                }
                                MissingPolicy::Skip => {
                                    report.missing += 1;
                                    return Ok(());
                                }
                            },
                        }
                    }
                    let pairs = intern_pairs(data, &d.tags);
                    let tag_id = data.add_tags(&pairs);
                    data.relations.set_members(handle, &stored);
                    data.relations.set_tags(handle, tag_id);
                    let mut meta = d.meta;
                    if let Some(previous) = data.relations.get(handle) {
                        meta.user = previous.meta.user;
                    }
                    data.relations.set_meta(handle, meta);
                    if data.change_state(neo_core::ObjectKind::Relation, id) != ChangeState::Added {
                        data.set_change_state(neo_core::ObjectKind::Relation, id, ChangeState::Modified);
                    }
                    report.modified[2] += 1;
                } else {
                    let relation = Relation {
                        id,
                        members,
                        tags: make_tags(data, &d.tags),
                        meta: d.meta,
                    };
                    match data.push_relation(relation, policy)? {
                        Some(_) => {
                            data.set_change_state(
                                neo_core::ObjectKind::Relation,
                                id,
                                if block == Block::Create {
                                    ChangeState::Added
                                } else {
                                    ChangeState::Modified
                                },
                            );
                            if block == Block::Create {
                                report.created[2] += 1;
                            } else {
                                report.modified[2] += 1;
                            }
                        }
                        None => report.missing += 1,
                    }
                }
            }
            Block::Delete => {
                if data.relation_handle(id).is_none() {
                    report.missing += 1;
                    return Ok(());
                }
                data.set_change_state_visible(
                    neo_core::ObjectKind::Relation,
                    id,
                    ChangeState::Deleted,
                    false,
                );
                report.deleted[2] += 1;
            }
            Block::None => {}
        },
    }
    Ok(())
}

fn intern_pairs(data: &mut DataSet, pairs: &[(String, String)]) -> Vec<(neo_core::Sym, neo_core::Sym)> {
    pairs
        .iter()
        .map(|(k, v)| (data.intern(k), data.intern(v)))
        .collect()
}

fn make_tags(data: &mut DataSet, pairs: &[(String, String)]) -> Tags {
    let symmetric = intern_pairs(data, pairs);
    Tags::from_pairs(symmetric)
}

fn parse_meta(e: &BytesStart) -> Result<Meta, NeoError> {
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
        user: neo_core::Sym::EMPTY,
        visible: attr_bool(e, "visible").unwrap_or(true),
    })
}

fn attr_str(e: &BytesStart, key: &str) -> Option<String> {
    e.attributes()
        .with_checks(false)
        .flatten()
        .find(|a| a.key.as_ref() == key)
        .and_then(|a| a.normalized_value(XmlVersion::Explicit1_0).ok())
        .map(|value| value.into_owned())
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
