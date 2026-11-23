//! 批量建对象的顺序工具。
//!
//! `AddRelation` 的 `precheck` 要求成员对象已经存在，所以同一批新建的 relation 必须按
//! "成员 relation 先于引用者"排序；否则父 relation 引用同批新建的子 relation 时，
//! `execute_batch` 会因找不到成员而整体回滚（粘贴/导入直接失效）。

use neo_core::ObjectId;

/// 把 relation 排成"成员 relation 先于引用者"的顺序。
///
/// `deps` 返回每个 relation 的 relation 型成员；不在 `ids` 里的成员会被忽略
/// （它们本来就不在这一批里，依赖由调用方另行保证）。
///
/// 存在循环引用时（数据损坏，或用户刻意构造）按输入顺序追加剩余项，交给批处理的
/// `precheck` 报错并整体回滚 —— 不在这里静默丢弃对象。
pub fn order_relations_by_dependency(
    ids: &[ObjectId],
    mut deps: impl FnMut(ObjectId) -> Vec<ObjectId>,
) -> Vec<ObjectId> {
    let in_batch: std::collections::BTreeSet<ObjectId> = ids.iter().copied().collect();
    let dependency_map: Vec<(ObjectId, Vec<ObjectId>)> = ids
        .iter()
        .map(|id| {
            let members: Vec<ObjectId> = deps(*id)
                .into_iter()
                .filter(|member| in_batch.contains(member))
                .collect();
            (*id, members)
        })
        .collect();

    let mut ordered: Vec<ObjectId> = Vec::with_capacity(ids.len());
    let mut remaining: Vec<ObjectId> = ids.to_vec();
    while !remaining.is_empty() {
        let mut progressed = false;
        let mut deferred: Vec<ObjectId> = Vec::with_capacity(remaining.len());
        for id in remaining {
            let ready = dependency_map
                .iter()
                .find(|(owner, _)| *owner == id)
                .is_some_and(|(_, members)| members.iter().all(|member| ordered.contains(member)));
            if ready {
                ordered.push(id);
                progressed = true;
            } else {
                deferred.push(id);
            }
        }
        if !progressed {
            ordered.extend(deferred);
            break;
        }
        remaining = deferred;
    }
    ordered
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn members_come_before_referrers() {
        // 30 的成员是 20；输入顺序父在前，输出必须先把子建出来。
        let ordered = order_relations_by_dependency(&[ObjectId(30), ObjectId(20)], |id| {
            if id == ObjectId(30) {
                vec![ObjectId(20)]
            } else {
                Vec::new()
            }
        });
        assert_eq!(ordered, vec![ObjectId(20), ObjectId(30)]);
    }

    #[test]
    fn chain_is_ordered_transitively() {
        // 3 → 2 → 1。
        let ids = [ObjectId(3), ObjectId(1), ObjectId(2)];
        let ordered = order_relations_by_dependency(&ids, |id| {
            let next = id.0 - 1;
            if next >= 1 {
                vec![ObjectId(next)]
            } else {
                Vec::new()
            }
        });
        assert_eq!(ordered, vec![ObjectId(1), ObjectId(2), ObjectId(3)]);
    }

    #[test]
    fn cycle_falls_back_to_input_order_without_losing_objects() {
        let ordered = order_relations_by_dependency(&[ObjectId(1), ObjectId(2)], |id| {
            if id == ObjectId(1) {
                vec![ObjectId(2)]
            } else {
                vec![ObjectId(1)]
            }
        });
        assert_eq!(ordered.len(), 2);
        assert!(ordered.contains(&ObjectId(1)) && ordered.contains(&ObjectId(2)));
    }

    #[test]
    fn members_outside_the_batch_are_ignored() {
        let ordered = order_relations_by_dependency(&[ObjectId(5)], |_| vec![ObjectId(999)]);
        assert_eq!(ordered, vec![ObjectId(5)]);
    }
}
