//! Ordering helper for batch object creation.
//!
//! `AddRelation`'s `precheck` requires member objects to already exist, so relations
//! created in the same batch must be sorted so that "member relations come before
//! referrers"; otherwise, when a parent relation references a child relation created in
//! the same batch, `execute_batch` rolls the whole thing back because the member is
//! missing (paste/import silently breaks).

use neo_core::ObjectId;

/// Order relations so that "member relations come before referrers".
///
/// `deps` returns the relation-typed members of each relation; members not in `ids` are
/// ignored (they are not part of this batch anyway, so the caller guarantees their
/// dependencies separately).
///
/// On cyclic references (corrupt data, or user-constructed), the remaining items are
/// appended in input order and left to the batch's `precheck` to report an error and roll
/// back the whole batch — objects are never silently dropped here.
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
        // 30's member is 20; the input order puts the parent first, but the output must create the child first.
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
        // 3 → 2 → 1.
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
