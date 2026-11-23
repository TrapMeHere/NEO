//! 几何操作：只读当前数据集，产出命令批。界面负责翻译 `key`。

use std::collections::HashMap;

use neo_core::{LatLon, ObjectId, ObjectKind};
use neo_editor::{
    AddNode, AddRelation, AddWay, Command, DeleteObject, Editor, MoveNode, RelationMemberSpec,
    SetRelationMembers, SetTags, SetWayNodes,
};
use neo_geo::WorldPoint;

/// 交给界面翻译的一条状态。`field` 是 `{name}` 占位符。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolNote {
    /// i18n 键。
    pub key: &'static str,
    /// 单个占位符。
    pub field: Option<(&'static str, String)>,
}

/// 几何操作的结果。命令已经执行，或明确没有执行。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ToolEffect {
    /// 没有改数据。
    Note(ToolNote),
    /// 执行了一批命令。
    Edited {
        /// 成功文案。
        note: ToolNote,
        /// 执行后要单独选中的对象。
        select: Option<(ObjectKind, ObjectId)>,
    },
    /// 走已有的批量移动。
    Moved(crate::Outcome),
}

fn note(key: &'static str) -> ToolEffect {
    ToolEffect::Note(ToolNote { key, field: None })
}

fn note_field(key: &'static str, field: &'static str, value: impl ToString) -> ToolEffect {
    ToolEffect::Note(ToolNote {
        key,
        field: Some((field, value.to_string())),
    })
}

fn edited(
    key: &'static str,
    field: Option<(&'static str, String)>,
    select: Option<(ObjectKind, ObjectId)>,
) -> ToolEffect {
    ToolEffect::Edited {
        note: ToolNote { key, field },
        select,
    }
}

fn resolved_tags(data: &neo_store::DataSet, tags: neo_store::TagSetId) -> Vec<(String, String)> {
    data.tags_of(tags)
        .iter()
        .map(|(key, value)| (data.resolve(key).to_owned(), data.resolve(value).to_owned()))
        .collect()
}

fn selection_nodes(editor: &Editor) -> Vec<(ObjectId, LatLon)> {
    let mut nodes = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    let selection = editor.selection();
    for id in selection.nodes().iter().copied().collect::<Vec<_>>() {
        if seen.insert(id)
            && let Some(handle) = editor.data().node_handle(id)
            && let Some(from) = editor.data().nodes.position(handle)
        {
            nodes.push((id, from));
        }
    }
    for way in selection.ways().iter().copied().collect::<Vec<_>>() {
        if let Some(handle) = editor.data().way_handle(way) {
            for node in editor.data().way_node_ids(handle) {
                if seen.insert(node)
                    && let Some(nh) = editor.data().node_handle(node)
                    && let Some(from) = editor.data().nodes.position(nh)
                {
                    nodes.push((node, from));
                }
            }
        }
    }
    nodes
}

fn primary_target(editor: &Editor) -> Option<(ObjectKind, ObjectId)> {
    let selection = editor.selection();
    if let Some(id) = selection.nodes().iter().next() {
        return Some((ObjectKind::Node, *id));
    }
    if let Some(id) = selection.ways().iter().next() {
        return Some((ObjectKind::Way, *id));
    }
    if let Some(id) = selection.relations().iter().next() {
        return Some((ObjectKind::Relation, *id));
    }
    None
}

fn fail(err: impl ToString) -> ToolEffect {
    note_field("app.error.generic", "err", err.to_string())
}

/// 把选中节点粘成第一个节点。
pub fn merge_selected_nodes(editor: &mut Editor) -> ToolEffect {
    let selected: Vec<ObjectId> = editor.selection().nodes().iter().copied().collect();
    if selected.len() < 2 {
        return note("app.error.need_two_nodes");
    }
    let target = selected[0];
    let sources = selected[1..].to_vec();
    let mut merged_tags = editor
        .data()
        .node_handle(target)
        .and_then(|handle| editor.data().node_view(handle))
        .map(|view| resolved_tags(editor.data(), view.info.tags))
        .unwrap_or_default();
    let mut tags_added = false;
    for source in &sources {
        if let Some(view) = editor
            .data()
            .node_handle(*source)
            .and_then(|handle| editor.data().node_view(handle))
        {
            for (key, value) in editor.data().tags_of(view.info.tags).iter() {
                let key = editor.data().resolve(key).to_owned();
                if !merged_tags.iter().any(|(existing, _)| *existing == key) {
                    merged_tags.push((key, editor.data().resolve(value).to_owned()));
                    tags_added = true;
                }
            }
        }
    }
    let mut way_updates: HashMap<ObjectId, Vec<ObjectId>> = HashMap::new();
    let mut relation_updates: HashMap<ObjectId, Vec<RelationMemberSpec>> = HashMap::new();
    for source in &sources {
        let Some(handle) = editor.data().node_handle(*source) else {
            continue;
        };
        let ways: Vec<_> = editor
            .referrers()
            .ways_of_node(handle)
            .iter()
            .filter_map(|way| editor.data().ways.id(*way))
            .collect();
        for way in ways {
            let list = way_updates.entry(way).or_insert_with(|| {
                editor
                    .data()
                    .way_handle(way)
                    .map(|handle| editor.data().way_node_ids(handle))
                    .unwrap_or_default()
            });
            for node in list.iter_mut() {
                if *node == *source {
                    *node = target;
                }
            }
            list.dedup();
        }
        let relations: Vec<_> = editor
            .referrers()
            .relations_of_node(handle)
            .iter()
            .filter_map(|relation| editor.data().relations.id(*relation))
            .collect();
        for relation in relations {
            let members = relation_updates
                .entry(relation)
                .or_insert_with(|| editor.relation_members(relation).unwrap_or_default());
            for member in members.iter_mut() {
                if member.kind == ObjectKind::Node && member.id == *source {
                    member.id = target;
                }
            }
        }
    }
    let mut commands: Vec<Box<dyn Command>> = Vec::new();
    if tags_added {
        commands.push(Box::new(SetTags::new(ObjectKind::Node, target, merged_tags)));
    }
    for (way, nodes) in way_updates {
        if nodes.len() < 2 {
            return note_field("app.error.glue_way_too_short", "way", way);
        }
        commands.push(Box::new(SetWayNodes::new(way, nodes)));
    }
    for (relation, members) in relation_updates {
        commands.push(Box::new(SetRelationMembers::new(relation, members)));
    }
    for source in &sources {
        commands.push(Box::new(DeleteObject::new(ObjectKind::Node, *source)));
    }
    match editor.execute_batch(commands) {
        Ok(()) => {
            editor.selection_mut().select_only(ObjectKind::Node, target);
            edited(
                "status.nodes_glued",
                Some(("n", selected.len().to_string())),
                None,
            )
        }
        Err(err) => note_field("app.error.glue", "err", err),
    }
}

/// 按选择的长轴把节点压到同一条经线或纬线上。
pub fn align_selected_nodes(editor: &mut Editor) -> ToolEffect {
    let nodes = selection_nodes(editor);
    if nodes.len() < 2 {
        return note("app.error.need_two_nodes");
    }
    let min_lat = nodes
        .iter()
        .map(|(_, p)| p.lat.to_degrees())
        .fold(f64::MAX, f64::min);
    let max_lat = nodes
        .iter()
        .map(|(_, p)| p.lat.to_degrees())
        .fold(f64::MIN, f64::max);
    let min_lon = nodes
        .iter()
        .map(|(_, p)| p.lon.to_degrees())
        .fold(f64::MAX, f64::min);
    let max_lon = nodes
        .iter()
        .map(|(_, p)| p.lon.to_degrees())
        .fold(f64::MIN, f64::max);
    let vertical = (max_lat - min_lat) >= (max_lon - min_lon);
    let target = if vertical {
        nodes.iter().map(|(_, p)| p.lon.to_degrees()).sum::<f64>() / nodes.len() as f64
    } else {
        nodes.iter().map(|(_, p)| p.lat.to_degrees()).sum::<f64>() / nodes.len() as f64
    };
    let moves: Vec<(ObjectId, LatLon, LatLon)> = nodes
        .iter()
        .filter_map(|(id, pos)| {
            let to = if vertical {
                LatLon::from_degrees(pos.lat.to_degrees(), target)
            } else {
                LatLon::from_degrees(target, pos.lon.to_degrees())
            };
            (*pos != to).then_some((*id, *pos, to))
        })
        .collect();
    if moves.is_empty() {
        return note("status.already_aligned");
    }
    let outcome = crate::EditController::new(crate::ToolKind::Select).move_nodes_batch(editor, &moves);
    ToolEffect::Moved(outcome)
}

/// 沿长轴把首尾之间的节点等距分布。
pub fn distribute_selected_nodes(editor: &mut Editor) -> ToolEffect {
    let mut nodes = selection_nodes(editor);
    if nodes.len() < 3 {
        return note("app.error.need_three_nodes");
    }
    let lat_span = nodes
        .iter()
        .map(|(_, p)| p.lat.to_degrees())
        .fold(f64::MIN, f64::max)
        - nodes
            .iter()
            .map(|(_, p)| p.lat.to_degrees())
            .fold(f64::MAX, f64::min);
    let lon_span = nodes
        .iter()
        .map(|(_, p)| p.lon.to_degrees())
        .fold(f64::MIN, f64::max)
        - nodes
            .iter()
            .map(|(_, p)| p.lon.to_degrees())
            .fold(f64::MAX, f64::min);
    let vertical = lat_span >= lon_span;
    nodes.sort_by(|a, b| {
        let av = if vertical {
            a.1.lat.to_degrees()
        } else {
            a.1.lon.to_degrees()
        };
        let bv = if vertical {
            b.1.lat.to_degrees()
        } else {
            b.1.lon.to_degrees()
        };
        av.partial_cmp(&bv).unwrap_or(std::cmp::Ordering::Equal)
    });
    let first = nodes[0].1;
    let last = nodes[nodes.len() - 1].1;
    let count = (nodes.len() - 1) as f64;
    let moves: Vec<(ObjectId, LatLon, LatLon)> = nodes
        .iter()
        .enumerate()
        .skip(1)
        .take(nodes.len().saturating_sub(2))
        .filter_map(|(index, (id, pos))| {
            let t = index as f64 / count;
            let lat = first.lat.to_degrees() + (last.lat.to_degrees() - first.lat.to_degrees()) * t;
            let lon = first.lon.to_degrees() + (last.lon.to_degrees() - first.lon.to_degrees()) * t;
            let to = LatLon::from_degrees(lat, lon);
            (*pos != to).then_some((*id, *pos, to))
        })
        .collect();
    if moves.is_empty() {
        return note("status.already_distributed");
    }
    let outcome = crate::EditController::new(crate::ToolKind::Select).move_nodes_batch(editor, &moves);
    ToolEffect::Moved(outcome)
}

/// 以选中节点包围盒中心为轴翻转纬度或经度。
pub fn mirror_selected_nodes(editor: &mut Editor, horizontal: bool) -> ToolEffect {
    let nodes = selection_nodes(editor);
    if nodes.is_empty() {
        return note("app.error.no_selection");
    }
    let center = if horizontal {
        nodes.iter().map(|(_, p)| p.lat.to_degrees()).sum::<f64>() / nodes.len() as f64
    } else {
        nodes.iter().map(|(_, p)| p.lon.to_degrees()).sum::<f64>() / nodes.len() as f64
    };
    let moves: Vec<(ObjectId, LatLon, LatLon)> = nodes
        .iter()
        .filter_map(|(id, pos)| {
            let to = if horizontal {
                LatLon::from_degrees(2.0 * center - pos.lat.to_degrees(), pos.lon.to_degrees())
            } else {
                LatLon::from_degrees(pos.lat.to_degrees(), 2.0 * center - pos.lon.to_degrees())
            };
            (*pos != to).then_some((*id, *pos, to))
        })
        .collect();
    let outcome = crate::EditController::new(crate::ToolKind::Select).move_nodes_batch(editor, &moves);
    ToolEffect::Moved(outcome)
}

/// 被多条 way 共用的节点，为其余 way 各复制一份。
pub fn unglue_selected_nodes(editor: &mut Editor) -> ToolEffect {
    let nodes = selection_nodes(editor);
    let mut commands: Vec<Box<dyn Command>> = Vec::new();
    for (node, pos) in &nodes {
        let Some(handle) = editor.data().node_handle(*node) else {
            continue;
        };
        let ways: Vec<_> = editor
            .referrers()
            .ways_of_node(handle)
            .iter()
            .filter_map(|way| editor.data().ways.id(*way))
            .collect();
        if ways.len() < 2 {
            continue;
        }
        let tags = editor
            .data()
            .node_view(handle)
            .map(|view| resolved_tags(editor.data(), view.info.tags))
            .unwrap_or_default();
        for way in ways.iter().skip(1) {
            let new_id = editor.next_local_id();
            let mut add = AddNode::new(*pos, tags.clone());
            add.id = Some(new_id);
            commands.push(Box::new(add));
            if let Some(way_handle) = editor.data().way_handle(*way) {
                let mut list = editor.data().way_node_ids(way_handle);
                for id in &mut list {
                    if *id == *node {
                        *id = new_id;
                    }
                }
                commands.push(Box::new(SetWayNodes::new(*way, list)));
            }
        }
    }
    if commands.is_empty() {
        return note("status.nothing_to_unglue");
    }
    match editor.execute_batch(commands) {
        Ok(()) => edited("status.unglued", None, None),
        Err(err) => fail(err),
    }
}

/// 沿 way 法线偏移 10 米复制一条同标签 way。
pub fn parallel_copy_way(editor: &mut Editor) -> ToolEffect {
    let Some((ObjectKind::Way, way)) = primary_target(editor) else {
        return note("app.error.no_selection");
    };
    let Some(handle) = editor.data().way_handle(way) else {
        return note("app.error.no_selection");
    };
    let ids = editor.data().way_node_ids(handle);
    let points: Vec<WorldPoint> = ids
        .iter()
        .filter_map(|id| {
            editor
                .data()
                .node_handle(*id)
                .and_then(|node| editor.data().nodes.position(node))
                .map(WorldPoint::from_latlon)
        })
        .collect();
    if points.len() < 2 {
        return note("app.error.way_too_short");
    }
    let tags = editor
        .data()
        .way_view(handle)
        .map(|view| resolved_tags(editor.data(), view.info.tags))
        .unwrap_or_default();
    let mut new_ids = Vec::with_capacity(points.len());
    let mut commands: Vec<Box<dyn Command>> = Vec::new();
    for index in 0..points.len() {
        let prev = points[index.saturating_sub(1)];
        let next = points[(index + 1).min(points.len() - 1)];
        let (nx, ny) = unit_normal(prev, next);
        let pos = WorldPoint::new(points[index].x + nx * 10.0, points[index].y + ny * 10.0).to_latlon();
        let id = editor.next_local_id();
        let mut add = AddNode::new(pos, Vec::new());
        add.id = Some(id);
        new_ids.push(id);
        commands.push(Box::new(add));
    }
    let new_way = editor.next_local_id();
    let mut add_way = AddWay::new(new_ids, tags);
    add_way.id = Some(new_way);
    commands.push(Box::new(add_way));
    match editor.execute_batch(commands) {
        Ok(()) => {
            editor.selection_mut().select_only(ObjectKind::Way, new_way);
            edited("status.parallel_created", None, None)
        }
        Err(err) => fail(err),
    }
}

/// 把选中 way 的每一段压成水平或垂直。
pub fn orthogonalize_selected(editor: &mut Editor) -> ToolEffect {
    let mut way_ids: Vec<ObjectId> = editor.selection().ways().iter().copied().collect();
    if way_ids.is_empty()
        && let Some((ObjectKind::Way, way)) = primary_target(editor)
    {
        way_ids.push(way);
    }
    if way_ids.is_empty() {
        return note("app.error.no_selection");
    }
    let mut moves: HashMap<ObjectId, (LatLon, LatLon)> = HashMap::new();
    for way in way_ids {
        let Some(handle) = editor.data().way_handle(way) else {
            continue;
        };
        let nodes = editor.data().way_node_ids(handle);
        let mut points: Vec<WorldPoint> = nodes
            .iter()
            .filter_map(|node| {
                editor
                    .data()
                    .node_handle(*node)
                    .and_then(|handle| editor.data().nodes.position(handle))
                    .map(WorldPoint::from_latlon)
            })
            .collect();
        if points.len() < 2 {
            continue;
        }
        let closed = points.first() == points.last();
        for index in 1..points.len() {
            if closed && index + 1 == points.len() {
                continue;
            }
            let a = points[index - 1];
            let b = points[index];
            if (b.x - a.x).abs() >= (b.y - a.y).abs() {
                points[index].y = a.y;
            } else {
                points[index].x = a.x;
            }
        }
        for (index, node) in nodes.iter().enumerate() {
            let Some(original) = editor
                .data()
                .node_handle(*node)
                .and_then(|handle| editor.data().nodes.position(handle))
            else {
                continue;
            };
            let target = points[index].to_latlon();
            if original != target {
                moves.entry(*node).or_insert((original, target));
            }
        }
    }
    if moves.is_empty() {
        return note("status.already_orthogonal");
    }
    let batch: Vec<(ObjectId, LatLon, LatLon)> =
        moves.into_iter().map(|(id, (from, to))| (id, from, to)).collect();
    let outcome = crate::EditController::new(crate::ToolKind::Select).move_nodes_batch(editor, &batch);
    ToolEffect::Moved(outcome)
}

/// 把多个闭合 way 收成一个 multipolygon，并清空成员标签。
pub fn join_selected_areas(editor: &mut Editor) -> ToolEffect {
    let ways: Vec<ObjectId> = editor.selection().ways().iter().copied().collect();
    if ways.len() < 2 {
        return note("app.error.need_two_ways");
    }
    let mut tags: Vec<(String, String)> = Vec::new();
    let mut members = Vec::with_capacity(ways.len());
    for way in &ways {
        let Some(handle) = editor.data().way_handle(*way) else {
            continue;
        };
        let nodes = editor.data().way_node_ids(handle);
        if nodes.len() < 4 || nodes.first() != nodes.last() {
            return note_field("app.error.area_not_closed", "way", way);
        }
        if let Some(view) = editor.data().way_view(handle) {
            for (key, value) in editor.data().tags_of(view.info.tags).iter() {
                let key = editor.data().resolve(key).to_owned();
                if key == "type" || tags.iter().any(|(existing, _)| *existing == key) {
                    continue;
                }
                tags.push((key, editor.data().resolve(value).to_owned()));
            }
        }
        members.push(RelationMemberSpec {
            kind: ObjectKind::Way,
            id: *way,
            role: "outer".to_owned(),
        });
    }
    tags.retain(|(key, _)| key != "type");
    tags.push(("type".to_owned(), "multipolygon".to_owned()));
    let relation_id = editor.next_local_id();
    let mut relation = AddRelation::new(members, tags);
    relation.id = Some(relation_id);
    let mut commands: Vec<Box<dyn Command>> = vec![Box::new(relation)];
    for way in &ways {
        commands.push(Box::new(SetTags::new(ObjectKind::Way, *way, Vec::new())));
    }
    match editor.execute_batch(commands) {
        Ok(()) => {
            editor
                .selection_mut()
                .select_only(ObjectKind::Relation, relation_id);
            edited("status.areas_joined", Some(("n", ways.len().to_string())), None)
        }
        Err(err) => fail(err),
    }
}

/// 把选中节点投影到选中 way 上，必要时插入为成员。
pub fn move_node_onto_way(editor: &mut Editor) -> ToolEffect {
    let Some(node) = editor.selection().nodes().iter().next().copied() else {
        return note("app.error.no_node_selected");
    };
    let Some(way) = editor.selection().ways().iter().next().copied() else {
        return note("app.error.no_way_selected");
    };
    let Some(handle) = editor.data().way_handle(way) else {
        return note("app.error.no_way_selected");
    };
    let nodes = editor.data().way_node_ids(handle);
    let points: Vec<WorldPoint> = nodes
        .iter()
        .filter_map(|id| {
            editor
                .data()
                .node_handle(*id)
                .and_then(|node| editor.data().nodes.position(node))
                .map(WorldPoint::from_latlon)
        })
        .collect();
    if points.len() < 2 {
        return note("app.error.way_too_short");
    }
    let Some(from) = editor
        .data()
        .node_handle(node)
        .and_then(|node| editor.data().nodes.position(node))
    else {
        return note("app.error.no_node_selected");
    };
    let world = WorldPoint::from_latlon(from);
    let mut best = (f64::MAX, 0usize, world);
    for index in 0..points.len() - 1 {
        let a = points[index];
        let b = points[index + 1];
        let dx = b.x - a.x;
        let dy = b.y - a.y;
        let len2 = dx * dx + dy * dy;
        let t = if len2 <= f64::EPSILON {
            0.0
        } else {
            (((world.x - a.x) * dx + (world.y - a.y) * dy) / len2).clamp(0.0, 1.0)
        };
        let projected = WorldPoint::new(a.x + dx * t, a.y + dy * t);
        let distance = projected.dist(world);
        if distance < best.0 {
            best = (distance, index, projected);
        }
    }
    let target = best.2.to_latlon();
    let mut commands: Vec<Box<dyn Command>> = Vec::new();
    if from != target {
        commands.push(Box::new(MoveNode::new(node, from, target)));
    }
    if !nodes.contains(&node) {
        let mut updated = nodes;
        updated.insert(best.1 + 1, node);
        commands.push(Box::new(SetWayNodes::new(way, updated)));
    }
    if commands.is_empty() {
        return note("status.node_already_on_way");
    }
    match editor.execute_batch(commands) {
        Ok(()) => edited("status.node_moved_onto_way", None, None),
        Err(err) => fail(err),
    }
}

/// 沿法线偏移 5 米，生成闭合建筑轮廓。
pub fn extrude_selected_way(editor: &mut Editor) -> ToolEffect {
    let Some((ObjectKind::Way, way)) = primary_target(editor) else {
        return note("app.error.no_selection");
    };
    let Some(handle) = editor.data().way_handle(way) else {
        return note("app.error.no_selection");
    };
    let original = editor.data().way_node_ids(handle);
    let points: Vec<WorldPoint> = original
        .iter()
        .filter_map(|id| {
            editor
                .data()
                .node_handle(*id)
                .and_then(|node| editor.data().nodes.position(node))
                .map(WorldPoint::from_latlon)
        })
        .collect();
    if points.len() < 2 {
        return note("app.error.way_too_short");
    }
    let mut tags = editor
        .data()
        .way_view(handle)
        .map(|view| resolved_tags(editor.data(), view.info.tags))
        .unwrap_or_default();
    if !tags.iter().any(|(key, _)| key == "building") {
        tags.push(("building".to_owned(), "yes".to_owned()));
    }
    let mut commands: Vec<Box<dyn Command>> = Vec::new();
    let mut offset_ids = Vec::with_capacity(points.len());
    for index in 0..points.len() {
        let prev = points[index.saturating_sub(1)];
        let next = points[(index + 1).min(points.len() - 1)];
        let (nx, ny) = unit_normal(prev, next);
        let pos = WorldPoint::new(points[index].x + nx * 5.0, points[index].y + ny * 5.0).to_latlon();
        let id = editor.next_local_id();
        let mut add = AddNode::new(pos, Vec::new());
        add.id = Some(id);
        offset_ids.push(id);
        commands.push(Box::new(add));
    }
    let mut outline = original.clone();
    outline.extend(offset_ids.iter().rev().copied());
    if let Some(first) = original.first() {
        outline.push(*first);
    }
    let new_way = editor.next_local_id();
    let mut add_way = AddWay::new(outline, tags);
    add_way.id = Some(new_way);
    commands.push(Box::new(add_way));
    match editor.execute_batch(commands) {
        Ok(()) => {
            editor.selection_mut().select_only(ObjectKind::Way, new_way);
            edited("status.extruded", None, None)
        }
        Err(err) => fail(err),
    }
}

fn unit_normal(prev: WorldPoint, next: WorldPoint) -> (f64, f64) {
    let dx = next.x - prev.x;
    let dy = next.y - prev.y;
    let len = (dx * dx + dy * dy).sqrt();
    if len > f64::EPSILON {
        (-dy / len, dx / len)
    } else {
        (1.0, 0.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use neo_store::DataSet;

    fn add_node(editor: &mut Editor, lat: f64, lon: f64) -> ObjectId {
        let id = editor.next_local_id();
        let mut cmd = AddNode::new(LatLon::from_degrees(lat, lon), Vec::new());
        cmd.id = Some(id);
        editor.execute(Box::new(cmd)).unwrap();
        id
    }

    #[test]
    fn align_moves_nodes_onto_the_long_axis() {
        let mut editor = Editor::new(DataSet::new());
        let a = add_node(&mut editor, 0.0, 0.0);
        let b = add_node(&mut editor, 1.0, 0.2);
        editor.selection_mut().insert(ObjectKind::Node, a);
        editor.selection_mut().insert(ObjectKind::Node, b);
        let effect = align_selected_nodes(&mut editor);
        assert!(matches!(effect, ToolEffect::Moved(_)));
        for id in [a, b] {
            let pos = editor
                .data()
                .node_handle(id)
                .and_then(|handle| editor.data().nodes.position(handle))
                .unwrap();
            assert!((pos.lon.to_degrees() - 0.1).abs() < 1e-9);
        }
    }
}
