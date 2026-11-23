//! # neo-validate — 内置校验器
//!
//! 对 [`DataSet`] 做只读检查，产出 [`Report`]。规则刻意保持**可解释、可测试**：
//! 每条规则只产出“对象 + 规则 id + 细节”，界面文案由 i18n 层翻译，库本身不拼接 UI 文本。
//!
//! 当前规则（阶段 4 v1）：
//!
//! | 规则 | 严重级别 | 说明 |
//! |---|---|---|
//! | [`Rule::WayTooShort`] | Error | way 少于 2 个节点 |
//! | [`Rule::WayDuplicateNode`] | Warning | 相邻重复节点 |
//! | [`Rule::WayUntagged`] | Warning | way 没有任何标签 |
//! | [`Rule::BuildingNotClosed`] | Error | `building=*` 的 way 未闭合 |
//! | [`Rule::DuplicateWay`] | Warning | 两条 way 的节点序列完全相同 |
//! | [`Rule::RelationEmpty`] | Warning | relation 无标签 |
//! | [`Rule::RelationNoMembers`] | Error | relation 没有成员 |
//!
//! 校验是线性的、无分配热点（除报告本身），1M 对象可在 UI 线程内完成；
//! 后续会按 bbox/脏对象做增量校验（见 docs/12）。

#![forbid(unsafe_code)]
#![warn(missing_docs)]

use std::collections::{HashMap, HashSet};

use neo_core::{ObjectId, ObjectKind};
use neo_store::DataSet;

/// 规则 id。稳定标识用于 i18n、配置过滤与测试。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Rule {
    /// way 少于 2 个节点。
    WayTooShort,
    /// 相邻重复节点。
    WayDuplicateNode,
    /// way 没有任何标签。
    WayUntagged,
    /// `building=*` 的 way 未闭合。
    BuildingNotClosed,
    /// 两条 way 节点序列完全相同。
    DuplicateWay,
    /// relation 没有标签。
    RelationEmpty,
    /// relation 没有成员。
    RelationNoMembers,
    /// 未标注且不属于任何 way 的孤立节点（JOSM: untagged node）。
    UntaggedNode,
    /// 坐标完全相同的重复节点。
    DuplicateNode,
    /// 可疑标签组合（如 highway + building）。
    SuspiciousTagCombination,
    /// multipolygon 成员角色不是 outer/inner/空。
    MultipolygonBadRole,
    /// way 自相交。
    SelfIntersectingWay,
    /// way 超过 OSM API 的 2000 节点上限。
    WayTooLong,
    /// 闭合 way 少于 4 个节点（无法构成面）。
    ClosedWayTooFewNodes,
    /// relation 有成员但没有 `type` 标签。
    RelationTypeMissing,
    /// relation 重复引用同一成员。
    DuplicateRelationMember,
    /// `layer` 标签不是整数。
    InvalidLayerTag,
    /// `area=yes` 的 way 未闭合。
    AreaNotClosed,
    /// 非相邻位置重复引用同一节点。
    NonAdjacentDuplicateNode,
    /// `oneway` 值非法。
    OnewayInvalid,
    /// `name` 为空。
    EmptyName,
    /// `layer` 超出合理范围（|layer| > 5）。
    LayerOutOfRange,
    /// multipolygon 没有 outer 成员。
    MultipolygonNoOuter,
    /// multipolygon 的 outer way 未闭合。
    MultipolygonOuterNotClosed,
}

/// 严重级别。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    /// 提示。
    Info,
    /// 警告：可能的数据质量问题。
    Warning,
    /// 错误：数据上的明确问题。不阻止上传。
    Error,
}

impl Rule {
    /// 规则 id 字符串（用于 i18n key：`validate.rule.<id>`）。
    pub fn id(self) -> &'static str {
        match self {
            Self::WayTooShort => "way_too_short",
            Self::WayDuplicateNode => "way_duplicate_node",
            Self::WayUntagged => "way_untagged",
            Self::BuildingNotClosed => "building_not_closed",
            Self::DuplicateWay => "duplicate_way",
            Self::RelationEmpty => "relation_empty",
            Self::RelationNoMembers => "relation_no_members",
            Self::UntaggedNode => "untagged_node",
            Self::DuplicateNode => "duplicate_node",
            Self::SuspiciousTagCombination => "suspicious_tag_combination",
            Self::MultipolygonBadRole => "multipolygon_bad_role",
            Self::SelfIntersectingWay => "self_intersecting_way",
            Self::WayTooLong => "way_too_long",
            Self::ClosedWayTooFewNodes => "closed_way_too_few_nodes",
            Self::RelationTypeMissing => "relation_type_missing",
            Self::DuplicateRelationMember => "duplicate_relation_member",
            Self::InvalidLayerTag => "invalid_layer_tag",
            Self::AreaNotClosed => "area_not_closed",
            Self::NonAdjacentDuplicateNode => "non_adjacent_duplicate_node",
            Self::OnewayInvalid => "oneway_invalid",
            Self::EmptyName => "empty_name",
            Self::LayerOutOfRange => "layer_out_of_range",
            Self::MultipolygonNoOuter => "multipolygon_no_outer",
            Self::MultipolygonOuterNotClosed => "multipolygon_outer_not_closed",
        }
    }

    /// 默认严重级别。
    pub fn severity(self) -> Severity {
        match self {
            Self::WayTooShort | Self::BuildingNotClosed | Self::RelationNoMembers => Severity::Error,
            Self::WayDuplicateNode
            | Self::WayUntagged
            | Self::DuplicateWay
            | Self::RelationEmpty
            | Self::DuplicateNode
            | Self::SuspiciousTagCombination
            | Self::MultipolygonBadRole => Severity::Warning,
            Self::UntaggedNode => Severity::Info,
            Self::SelfIntersectingWay | Self::WayTooLong | Self::ClosedWayTooFewNodes => Severity::Error,
            Self::RelationTypeMissing | Self::DuplicateRelationMember | Self::InvalidLayerTag => {
                Severity::Warning
            }
            Self::AreaNotClosed | Self::MultipolygonNoOuter | Self::MultipolygonOuterNotClosed => {
                Severity::Error
            }
            Self::NonAdjacentDuplicateNode
            | Self::OnewayInvalid
            | Self::EmptyName
            | Self::LayerOutOfRange => Severity::Warning,
        }
    }
}

/// 一条校验问题。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Issue {
    /// 规则。
    pub rule: Rule,
    /// 严重级别（可被用户配置覆盖，当前取规则默认）。
    pub severity: Severity,
    /// 涉及对象（第一个通常用于定位/选择）。
    pub objects: Vec<(ObjectKind, ObjectId)>,
    /// 具体细节（ID、节点数等，语言无关）。
    pub detail: String,
}

impl Issue {
    fn new(rule: Rule, objects: Vec<(ObjectKind, ObjectId)>, detail: impl Into<String>) -> Self {
        Self {
            rule,
            severity: rule.severity(),
            objects,
            detail: detail.into(),
        }
    }
}

/// 校验结果。
#[derive(Clone, Debug, Default)]
pub struct Report {
    /// 问题列表（按对象 id 顺序，稳定可复现）。
    pub issues: Vec<Issue>,
    /// 检查的节点数。
    pub nodes: usize,
    /// 检查的 way 数。
    pub ways: usize,
    /// 检查的 relation 数。
    pub relations: usize,
}

impl Report {
    /// 错误数。
    pub fn errors(&self) -> usize {
        self.issues
            .iter()
            .filter(|i| i.severity == Severity::Error)
            .count()
    }

    /// 警告数。
    pub fn warnings(&self) -> usize {
        self.issues
            .iter()
            .filter(|i| i.severity == Severity::Warning)
            .count()
    }

    /// 是否干净。
    pub fn is_clean(&self) -> bool {
        self.issues.is_empty()
    }
}

/// 对本次新增或修改过的对象运行规则。未改动的下载数据不进入报告。
pub fn validate(data: &DataSet) -> Report {
    let mut report = Report::default();

    let mut seen_ways: HashMap<Vec<ObjectId>, ObjectId> = HashMap::new();
    let mut referenced_nodes: HashSet<ObjectId> = HashSet::new();

    for way in data.iter_ways() {
        let Some(view) = data.way_view(way.handle) else {
            continue;
        };
        let nodes: Vec<ObjectId> = view
            .node_refs
            .iter()
            .filter_map(|handle| data.nodes.id(*handle))
            .collect();
        referenced_nodes.extend(nodes.iter().copied());
        let way_edited = is_edited(data, ObjectKind::Way, way.id);
        if nodes.len() >= 2
            && let Some(first) = seen_ways.insert(nodes.clone(), way.id)
        {
            let other_edited = is_edited(data, ObjectKind::Way, first);
            if way_edited || other_edited {
                let (primary, secondary) = if way_edited {
                    (way.id, first)
                } else {
                    (first, way.id)
                };
                report.issues.push(Issue::new(
                    Rule::DuplicateWay,
                    vec![(ObjectKind::Way, primary), (ObjectKind::Way, secondary)],
                    format!("{}", nodes.len()),
                ));
            }
        }
        if !way_edited {
            continue;
        }
        report.ways += 1;
        let way_ref = (ObjectKind::Way, way.id);

        if nodes.len() < 2 {
            report.issues.push(Issue::new(
                Rule::WayTooShort,
                vec![way_ref],
                format!("{} node(s)", nodes.len()),
            ));
        }
        if nodes.len() > 2000 {
            report.issues.push(Issue::new(
                Rule::WayTooLong,
                vec![way_ref],
                format!("{} node(s)", nodes.len()),
            ));
        }
        if nodes.len() >= 2 && nodes.first() == nodes.last() && nodes.len() < 4 {
            report.issues.push(Issue::new(
                Rule::ClosedWayTooFewNodes,
                vec![way_ref],
                format!("{} node(s)", nodes.len()),
            ));
        }
        if invalid_layer_tag(data, view.tag_pairs).is_some() {
            report
                .issues
                .push(Issue::new(Rule::InvalidLayerTag, vec![way_ref], ""));
        }
        let closed = nodes.len() >= 2 && nodes.first() == nodes.last();
        if tag_text(data, view.tag_pairs, "area") == Some("yes") && !closed {
            report
                .issues
                .push(Issue::new(Rule::AreaNotClosed, vec![way_ref], ""));
        }
        if let Some(layer) = tag_text(data, view.tag_pairs, "layer")
            && let Ok(value) = layer.parse::<i32>()
            && value.unsigned_abs() > 5
        {
            report
                .issues
                .push(Issue::new(Rule::LayerOutOfRange, vec![way_ref], layer.to_owned()));
        }
        if let Some(oneway) = tag_text(data, view.tag_pairs, "oneway")
            && !matches!(
                oneway,
                "yes" | "no" | "true" | "false" | "1" | "0" | "-1" | "reversible"
            )
        {
            report
                .issues
                .push(Issue::new(Rule::OnewayInvalid, vec![way_ref], oneway.to_owned()));
        }
        if tag_text(data, view.tag_pairs, "name").is_some_and(|name| name.trim().is_empty()) {
            report.issues.push(Issue::new(Rule::EmptyName, vec![way_ref], ""));
        }
        let mut seen_nodes = HashSet::new();
        for (index, node) in nodes.iter().enumerate() {
            if index > 0 && nodes[index - 1] == *node {
                continue;
            }
            if closed && index + 1 == nodes.len() && nodes[0] == *node {
                continue;
            }
            if !seen_nodes.insert(*node) {
                report.issues.push(Issue::new(
                    Rule::NonAdjacentDuplicateNode,
                    vec![way_ref],
                    node.to_string(),
                ));
                break;
            }
        }
        if nodes.windows(2).any(|pair| pair[0] == pair[1]) {
            report
                .issues
                .push(Issue::new(Rule::WayDuplicateNode, vec![way_ref], ""));
        }
        if view.tag_pairs.is_empty() {
            report
                .issues
                .push(Issue::new(Rule::WayUntagged, vec![way_ref], ""));
        }
        let is_building = view
            .tag_pairs
            .iter()
            .any(|(key, _)| data.resolve(*key) == "building");
        if is_building && !view.is_closed() {
            report
                .issues
                .push(Issue::new(Rule::BuildingNotClosed, vec![way_ref], ""));
        }
        if let Some(combo) = suspicious_combination(data, view.tag_pairs) {
            report
                .issues
                .push(Issue::new(Rule::SuspiciousTagCombination, vec![way_ref], combo));
        }
        if nodes.len() <= 2048 && way_self_intersects(data, &nodes) {
            report
                .issues
                .push(Issue::new(Rule::SelfIntersectingWay, vec![way_ref], ""));
        }
    }

    for relation in data.iter_relations() {
        let Some(view) = data.relation_view(relation.handle) else {
            continue;
        };
        if !is_edited(data, ObjectKind::Relation, relation.id) {
            continue;
        }
        report.relations += 1;
        let relation_ref = (ObjectKind::Relation, relation.id);
        if view.tag_pairs.is_empty() {
            report
                .issues
                .push(Issue::new(Rule::RelationEmpty, vec![relation_ref], ""));
        }
        if view.members.is_empty() {
            report
                .issues
                .push(Issue::new(Rule::RelationNoMembers, vec![relation_ref], ""));
        } else if !view.tag_pairs.iter().any(|(key, _)| data.resolve(*key) == "type") {
            report
                .issues
                .push(Issue::new(Rule::RelationTypeMissing, vec![relation_ref], ""));
        }
        if invalid_layer_tag(data, view.tag_pairs).is_some() {
            report
                .issues
                .push(Issue::new(Rule::InvalidLayerTag, vec![relation_ref], ""));
        }
        let mut seen_members = HashSet::new();
        for member in view.members {
            let key = (member.target.kind, member.target.index, member.target.generation);
            if !seen_members.insert(key) {
                report.issues.push(Issue::new(
                    Rule::DuplicateRelationMember,
                    vec![relation_ref],
                    data.resolve(member.role).to_owned(),
                ));
                break;
            }
        }
        let relation_type = tag_text(data, view.tag_pairs, "type");
        let is_multipolygon = matches!(relation_type, Some("multipolygon" | "boundary"));
        if is_multipolygon {
            let mut outers = 0usize;
            let mut open_outer = false;
            for member in view.members {
                let role = data.resolve(member.role);
                if !matches!(role, "outer" | "inner" | "") {
                    report.issues.push(Issue::new(
                        Rule::MultipolygonBadRole,
                        vec![relation_ref],
                        role.to_owned(),
                    ));
                }
                if matches!(role, "outer" | "")
                    && let Some(way) = member.target.as_way()
                {
                    outers += 1;
                    if let Some(way_view) = data.way_view(way)
                        && !way_view.is_closed()
                    {
                        open_outer = true;
                    }
                }
            }
            if outers == 0 {
                report
                    .issues
                    .push(Issue::new(Rule::MultipolygonNoOuter, vec![relation_ref], ""));
            } else if open_outer {
                report.issues.push(Issue::new(
                    Rule::MultipolygonOuterNotClosed,
                    vec![relation_ref],
                    "",
                ));
            }
        }
    }

    // 节点规则：孤立未标注节点、完全重复坐标。
    let mut seen_positions: HashMap<(i32, i32), ObjectId> = HashMap::new();
    for node in data.iter_nodes() {
        let Some(view) = data.node_view(node.handle) else {
            continue;
        };
        let pos = view.pos();
        let node_edited = is_edited(data, ObjectKind::Node, node.id);
        if let Some(first) = seen_positions.insert((pos.lat.0, pos.lon.0), node.id) {
            let other_edited = is_edited(data, ObjectKind::Node, first);
            if node_edited || other_edited {
                let (primary, secondary) = if node_edited {
                    (node.id, first)
                } else {
                    (first, node.id)
                };
                report.issues.push(Issue::new(
                    Rule::DuplicateNode,
                    vec![(ObjectKind::Node, primary), (ObjectKind::Node, secondary)],
                    "",
                ));
            }
        }
        if !node_edited {
            continue;
        }
        report.nodes += 1;
        let node_ref = (ObjectKind::Node, node.id);
        if view.tag_pairs.is_empty() && !referenced_nodes.contains(&node.id) {
            report
                .issues
                .push(Issue::new(Rule::UntaggedNode, vec![node_ref], ""));
        }
    }

    report.issues.sort_by_key(|issue| {
        issue.objects.first().map(|(kind, id)| {
            let rank = match kind {
                ObjectKind::Node => 0u8,
                ObjectKind::Way => 1,
                ObjectKind::Relation => 2,
                ObjectKind::Changeset => 3,
            };
            (rank, id.0)
        })
    });
    report
}

fn is_edited(data: &DataSet, kind: ObjectKind, id: ObjectId) -> bool {
    matches!(
        data.change_state(kind, id),
        neo_osm::ChangeState::Added | neo_osm::ChangeState::Modified
    )
}

fn tag_text<'a>(data: &'a DataSet, tags: &'a [(neo_core::Sym, neo_core::Sym)], key: &str) -> Option<&'a str> {
    tags.iter()
        .find(|(k, _)| data.resolve(*k) == key)
        .map(|(_, value)| data.resolve(*value))
}

fn invalid_layer_tag(data: &DataSet, tags: &[(neo_core::Sym, neo_core::Sym)]) -> Option<String> {
    tags.iter()
        .find(|(key, _)| data.resolve(*key) == "layer")
        .and_then(|(_, value)| {
            let text = data.resolve(*value).trim();
            if text.parse::<i32>().is_ok() {
                None
            } else {
                Some(text.to_owned())
            }
        })
}

fn suspicious_combination(data: &DataSet, tags: &[(neo_core::Sym, neo_core::Sym)]) -> Option<String> {
    let has = |key: &str, value: Option<&str>| {
        tags.iter().any(|(k, v)| {
            data.resolve(*k) == key && value.map(|want| data.resolve(*v) == want).unwrap_or(true)
        })
    };
    if has("highway", None) && has("building", None) {
        return Some("highway + building".to_owned());
    }
    if has("natural", Some("water")) && has("building", None) {
        return Some("natural=water + building".to_owned());
    }
    if has("highway", None) && has("railway", None) {
        return Some("highway + railway".to_owned());
    }
    None
}

fn segments_cross(a: (f64, f64), b: (f64, f64), c: (f64, f64), d: (f64, f64)) -> bool {
    fn orient(p: (f64, f64), q: (f64, f64), r: (f64, f64)) -> f64 {
        (q.0 - p.0) * (r.1 - p.1) - (q.1 - p.1) * (r.0 - p.0)
    }
    let o1 = orient(a, b, c);
    let o2 = orient(a, b, d);
    let o3 = orient(c, d, a);
    let o4 = orient(c, d, b);
    (o1 > 0.0) != (o2 > 0.0) && (o3 > 0.0) != (o4 > 0.0)
}

fn way_self_intersects(data: &DataSet, nodes: &[ObjectId]) -> bool {
    let mut coords: Vec<(f64, f64)> = Vec::with_capacity(nodes.len());
    for id in nodes {
        let Some(handle) = data.node_handle(*id) else {
            continue;
        };
        let Some(view) = data.node_view(handle) else {
            continue;
        };
        let pos = view.pos();
        coords.push((f64::from(pos.lon.0), f64::from(pos.lat.0)));
    }
    if coords.len() < 4 {
        return false;
    }
    let n = coords.len();
    let closed = coords[0] == coords[n - 1];
    for i in 0..n - 1 {
        for j in i + 2..n - 1 {
            if closed && i == 0 && j == n - 2 {
                continue;
            }
            if segments_cross(coords[i], coords[i + 1], coords[j], coords[j + 1]) {
                return true;
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use neo_core::{LatLon, Meta};
    use neo_osm::{Node, Relation, Tags, Way};
    use neo_store::MissingPolicy;

    #[test]
    fn new_rules_fire_on_common_mistakes() {
        let mut data = DataSet::new();
        for id in [1, 2] {
            data.push_node(Node {
                id: ObjectId(id),
                pos: LatLon::from_degrees(47.0 + f64::from(id as i32) * 0.001, 8.0),
                tags: Tags::new(),
                meta: Meta::new_local(),
            })
            .unwrap();
        }
        let mut tags = Tags::new();
        for (key, value) in [("area", "yes"), ("oneway", "maybe"), ("layer", "99")] {
            let k = data.intern(key);
            let v = data.intern(value);
            tags.set(k, v);
        }
        data.push_way(
            Way {
                id: ObjectId(10),
                nodes: vec![ObjectId(1), ObjectId(2)].into(),
                tags,
                meta: Meta::new_local(),
            },
            MissingPolicy::Error,
        )
        .unwrap();
        let type_key = data.intern("type");
        let type_value = data.intern("multipolygon");
        let mut relation_tags = Tags::new();
        relation_tags.set(type_key, type_value);
        data.push_relation(
            Relation {
                id: ObjectId(20),
                members: Vec::new().into(),
                tags: relation_tags,
                meta: Meta::new_local(),
            },
            MissingPolicy::Error,
        )
        .unwrap();
        data.set_change_state(ObjectKind::Way, ObjectId(10), neo_osm::ChangeState::Added);
        data.set_change_state(ObjectKind::Relation, ObjectId(20), neo_osm::ChangeState::Added);
        let report = validate(&data);
        let rules: Vec<Rule> = report.issues.iter().map(|issue| issue.rule).collect();
        assert!(rules.contains(&Rule::AreaNotClosed));
        assert!(rules.contains(&Rule::OnewayInvalid));
        assert!(rules.contains(&Rule::LayerOutOfRange));
        assert!(rules.contains(&Rule::MultipolygonNoOuter));
        assert_eq!(report.ways, 1);
        assert_eq!(report.relations, 1);
        assert_eq!(report.nodes, 0);
    }

    #[test]
    fn unchanged_download_is_left_out() {
        let mut data = DataSet::new();
        data.push_node(Node {
            id: ObjectId(1),
            pos: LatLon::from_degrees(47.0, 8.0),
            tags: Tags::new(),
            meta: Meta::new_local(),
        })
        .unwrap();
        data.push_node(Node {
            id: ObjectId(2),
            pos: LatLon::from_degrees(47.0, 8.0),
            tags: Tags::new(),
            meta: Meta::new_local(),
        })
        .unwrap();
        data.push_way(
            Way {
                id: ObjectId(10),
                nodes: vec![ObjectId(1)].into(),
                tags: Tags::new(),
                meta: Meta::new_local(),
            },
            MissingPolicy::Error,
        )
        .unwrap();
        let report = validate(&data);
        assert!(report.issues.is_empty());
        assert_eq!((report.nodes, report.ways, report.relations), (0, 0, 0));
    }
}
