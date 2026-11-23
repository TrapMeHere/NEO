//! 删除规划：把"删除对象"按 JOSM 语义展开成一批原子命令。
//!
//! OSM API 拒绝删除仍被 way 引用的节点（HTTP 412 `Node … is still used by ways …`），
//! 但允许在同一个 changeset 里**先修改 way、再删除节点** —— JOSM/iD 删除顶点就是这么做的。
//! 因此本地必须先把引用者改掉，否则用户根本无法删除线或多边形上的一个点。
//!
//! 本模块只负责**生成命令**，不执行；调用方用 [`Editor::execute_batch`] 提交，
//! 这样"引用者修正 + 删除"共享一次事务与一条 undo 记录。
//!
//! 与 `DeleteObject::precheck` 的分工：命令本身仍然保持严格（被引用就拒绝），
//! 作为最后一道安全网；级联逻辑集中在规划器里，避免命令 undo 复杂化。

use std::collections::{BTreeMap, BTreeSet};

use neo_core::{ObjectId, ObjectKind};

use crate::command::{Command, CommandError};
use crate::commands::{DeleteObject, RelationMemberSpec, SetRelationMembers, SetWayNodes};
use crate::editor::Editor;

/// 一次删除展开后的计划。
#[derive(Default)]
pub struct DeletePlan {
    /// 已按安全顺序排好的命令：引用者修正在前，删除在后。
    pub commands: Vec<Box<dyn Command>>,
    /// 会被显式删除的对象（去重、已排序）。
    pub targets: Vec<(ObjectKind, ObjectId)>,
    /// 因删到不足 2 个节点而一并删除的 way。非空时调用方应先向用户确认。
    pub reaped_ways: Vec<ObjectId>,
    /// 被摘掉节点的 way。
    pub touched_ways: Vec<ObjectId>,
    /// 被摘掉成员的 relation。
    pub touched_relations: Vec<ObjectId>,
}

impl DeletePlan {
    /// 计划是否为空（没有任何可执行命令）。
    pub fn is_empty(&self) -> bool {
        self.commands.is_empty()
    }
}

impl std::fmt::Debug for DeletePlan {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeletePlan")
            .field("targets", &self.targets)
            .field("reaped_ways", &self.reaped_ways)
            .field("touched_ways", &self.touched_ways)
            .field("touched_relations", &self.touched_relations)
            .field("commands", &self.commands.len())
            .finish()
    }
}

/// 生成一次删除的完整计划。
///
/// `targets` 中的对象会被删除，引用它们的 way / relation 会被同步修正：
///
/// * 节点仍被 way 引用 → 从该 way 的节点序列中摘掉；若剩余不足 2 个节点，
///   连同这条 way 一起删除（计入 [`DeletePlan::reaped_ways`]，调用方应提示用户）。
/// * 节点 / way / relation 仍被 relation 引用 → 从成员列表中剔除该成员。
/// * 顺序固定为：relation 成员修正 → way 节点修正 → 删 way → 删节点 → 删 relation。
///   这个顺序保证批量执行时每条命令的 `precheck` 都能通过。
///
/// 不存在的对象返回 [`CommandError::NotFound`]；一个有效对象都没有时返回空计划。
pub fn plan_delete(
    editor: &mut Editor,
    targets: &[(ObjectKind, ObjectId)],
) -> Result<DeletePlan, CommandError> {
    // 反查表是只读的，脏标记由写命令设置；规划前必须刷新，否则会漏掉引用者。
    editor.refresh_referrers();

    let mut nodes: BTreeSet<ObjectId> = BTreeSet::new();
    let mut ways: BTreeSet<ObjectId> = BTreeSet::new();
    let mut relations: BTreeSet<ObjectId> = BTreeSet::new();
    for (kind, id) in targets {
        match kind {
            ObjectKind::Node => {
                nodes.insert(*id);
            }
            ObjectKind::Way => {
                ways.insert(*id);
            }
            ObjectKind::Relation => {
                relations.insert(*id);
            }
            ObjectKind::Changeset => {
                return Err(CommandError::Invalid(
                    "changeset is not a deletable edit object".into(),
                ));
            }
        }
    }

    for id in &nodes {
        if editor.data().node_handle(*id).is_none() {
            return Err(CommandError::NotFound {
                kind: ObjectKind::Node,
                id: *id,
            });
        }
    }
    for id in &ways {
        if editor.data().way_handle(*id).is_none() {
            return Err(CommandError::NotFound {
                kind: ObjectKind::Way,
                id: *id,
            });
        }
    }
    for id in &relations {
        if editor.data().relation_handle(*id).is_none() {
            return Err(CommandError::NotFound {
                kind: ObjectKind::Relation,
                id: *id,
            });
        }
    }

    // 1. 找出引用被删节点、但自身不在删除集里的 way（这些 way 需要摘点或整体删除）。
    let mut shrink_candidates: BTreeSet<ObjectId> = BTreeSet::new();
    {
        let data = editor.data();
        let referrers = editor.referrers();
        for id in &nodes {
            let Some(handle) = data.node_handle(*id) else {
                continue;
            };
            for way in referrers.ways_of_node(handle) {
                if let Some(way_id) = data.ways.id(*way)
                    && !ways.contains(&way_id)
                {
                    shrink_candidates.insert(way_id);
                }
            }
        }
    }

    let mut resized: BTreeMap<ObjectId, Vec<ObjectId>> = BTreeMap::new();
    let mut reaped: BTreeSet<ObjectId> = BTreeSet::new();
    for way_id in shrink_candidates {
        let Some(handle) = editor.data().way_handle(way_id) else {
            continue;
        };
        let current = editor.data().try_way_node_ids(handle)?;
        let before = current.len();
        let kept: Vec<ObjectId> = current.into_iter().filter(|node| !nodes.contains(node)).collect();
        if kept.len() == before {
            // 反查表与节点序列不一致（理论上不应发生）：不要凭空改数据。
            continue;
        }
        if kept.len() >= 2 {
            resized.insert(way_id, kept);
        } else {
            reaped.insert(way_id);
        }
    }

    // 2. 找出需要改成员列表的 relation：引用被删节点/way/relation，或被并删 way 的。
    let mut relations_to_rewrite: BTreeSet<ObjectId> = BTreeSet::new();
    {
        let data = editor.data();
        let referrers = editor.referrers();
        for id in &nodes {
            let Some(handle) = data.node_handle(*id) else {
                continue;
            };
            for relation in referrers.relations_of_node(handle) {
                if let Some(relation_id) = data.relations.id(*relation) {
                    relations_to_rewrite.insert(relation_id);
                }
            }
        }
        for id in reaped.iter().chain(ways.iter()) {
            let Some(handle) = data.way_handle(*id) else {
                continue;
            };
            for relation in referrers.relations_of_way(handle) {
                if let Some(relation_id) = data.relations.id(*relation) {
                    relations_to_rewrite.insert(relation_id);
                }
            }
        }
        for id in &relations {
            let Some(handle) = data.relation_handle(*id) else {
                continue;
            };
            for relation in referrers.relations_of_relation(handle) {
                if let Some(relation_id) = data.relations.id(*relation) {
                    relations_to_rewrite.insert(relation_id);
                }
            }
        }
    }

    let mut member_updates: BTreeMap<ObjectId, Vec<RelationMemberSpec>> = BTreeMap::new();
    for relation_id in relations_to_rewrite {
        let members = editor.relation_members(relation_id)?;
        let before = members.len();
        let kept: Vec<RelationMemberSpec> = members
            .into_iter()
            .filter(|member| !vanishes(member.kind, member.id, &nodes, &ways, &reaped, &relations))
            .collect();
        if kept.len() != before {
            member_updates.insert(relation_id, kept);
        }
    }

    // 3. 按依赖顺序组装命令。
    let mut commands: Vec<Box<dyn Command>> = Vec::new();
    let touched_relations: Vec<ObjectId> = member_updates.keys().copied().collect();
    for (relation, members) in member_updates {
        commands.push(Box::new(SetRelationMembers::new(relation, members)));
    }
    let touched_ways: Vec<ObjectId> = resized.keys().copied().collect();
    for (way, node_ids) in resized {
        commands.push(Box::new(SetWayNodes::new(way, node_ids)));
    }
    let reaped_ways: Vec<ObjectId> = reaped.iter().copied().collect();
    for way in &reaped_ways {
        commands.push(Box::new(DeleteObject::new(ObjectKind::Way, *way)));
    }
    for way in &ways {
        commands.push(Box::new(DeleteObject::new(ObjectKind::Way, *way)));
    }
    for node in &nodes {
        commands.push(Box::new(DeleteObject::new(ObjectKind::Node, *node)));
    }
    for relation in &relations {
        commands.push(Box::new(DeleteObject::new(ObjectKind::Relation, *relation)));
    }

    let mut target_list: Vec<(ObjectKind, ObjectId)> = Vec::new();
    target_list.extend(nodes.iter().map(|id| (ObjectKind::Node, *id)));
    target_list.extend(ways.iter().map(|id| (ObjectKind::Way, *id)));
    target_list.extend(relations.iter().map(|id| (ObjectKind::Relation, *id)));
    Ok(DeletePlan {
        commands,
        targets: target_list,
        reaped_ways,
        touched_ways,
        touched_relations,
    })
}

/// 该成员是否会随着本次删除消失。
fn vanishes(
    kind: ObjectKind,
    id: ObjectId,
    nodes: &BTreeSet<ObjectId>,
    ways: &BTreeSet<ObjectId>,
    reaped: &BTreeSet<ObjectId>,
    relations: &BTreeSet<ObjectId>,
) -> bool {
    match kind {
        ObjectKind::Node => nodes.contains(&id),
        ObjectKind::Way => ways.contains(&id) || reaped.contains(&id),
        ObjectKind::Relation => relations.contains(&id),
        ObjectKind::Changeset => false,
    }
}
