//! Cascade-delete regression tests: when deleting nodes/ways/relations, referrers must be fixed up in sync (JOSM semantics).

use neo_core::{LatLon, ObjectId, ObjectKind};
use neo_editor::{AddNode, AddRelation, AddWay, Editor, RelationMemberSpec, plan_delete};
use neo_osm::ChangeState;
use neo_store::DataSet;

fn lon(id: i64) -> LatLon {
    LatLon::from_degrees(47.0, 8.0 + id as f64 * 0.001)
}

fn add_node(editor: &mut Editor, id: i64) {
    let mut cmd = AddNode::new(lon(id), Vec::new());
    cmd.id = Some(ObjectId(id));
    editor.execute(Box::new(cmd)).unwrap();
}

fn add_way(editor: &mut Editor, id: i64, nodes: &[i64]) {
    let mut cmd = AddWay::new(nodes.iter().copied().map(ObjectId).collect(), Vec::new());
    cmd.id = Some(ObjectId(id));
    editor.execute(Box::new(cmd)).unwrap();
}

fn member(kind: ObjectKind, id: i64, role: &str) -> RelationMemberSpec {
    RelationMemberSpec {
        kind,
        id: ObjectId(id),
        role: role.to_owned(),
    }
}

fn add_relation(editor: &mut Editor, id: i64, members: Vec<RelationMemberSpec>) {
    let mut cmd = AddRelation::new(members, Vec::new());
    cmd.id = Some(ObjectId(id));
    editor.execute(Box::new(cmd)).unwrap();
}

fn node_ids(editor: &Editor, way: i64) -> Vec<ObjectId> {
    let data = editor.data();
    let handle = data.way_handle(ObjectId(way)).expect("way exists");
    data.way_node_ids(handle)
}

#[test]
fn deleting_middle_vertex_resizes_way_in_one_undo_step() {
    let mut editor = Editor::new(DataSet::new());
    for id in 1..=3 {
        add_node(&mut editor, id);
    }
    add_way(&mut editor, 10, &[1, 2, 3]);

    let plan = plan_delete(&mut editor, &[(ObjectKind::Node, ObjectId(2))]).unwrap();
    assert!(
        plan.reaped_ways.is_empty(),
        "deleting 1 of 3 nodes leaves 2 nodes"
    );
    assert_eq!(plan.touched_ways, vec![ObjectId(10)]);
    let undo_before = editor.history().undo_len();
    editor.execute_batch(plan.commands).unwrap();

    assert_eq!(node_ids(&editor, 10), vec![ObjectId(1), ObjectId(3)]);
    assert_eq!(
        editor.data().change_state(ObjectKind::Node, ObjectId(2)),
        ChangeState::Deleted
    );
    // One command batch = one history entry.
    assert_eq!(editor.history().undo_len(), undo_before + 1);

    editor.undo().unwrap();
    assert_eq!(node_ids(&editor, 10), vec![ObjectId(1), ObjectId(2), ObjectId(3)]);
    assert_eq!(
        editor.data().change_state(ObjectKind::Node, ObjectId(2)),
        ChangeState::Added
    );
}

#[test]
fn deleting_vertex_of_two_node_way_reaps_the_way() {
    let mut editor = Editor::new(DataSet::new());
    add_node(&mut editor, 1);
    add_node(&mut editor, 2);
    add_way(&mut editor, 10, &[1, 2]);

    let plan = plan_delete(&mut editor, &[(ObjectKind::Node, ObjectId(1))]).unwrap();
    assert_eq!(plan.reaped_ways, vec![ObjectId(10)]);
    editor.execute_batch(plan.commands).unwrap();

    assert_eq!(
        editor.data().change_state(ObjectKind::Way, ObjectId(10)),
        ChangeState::Deleted
    );
    assert_eq!(
        editor.data().change_state(ObjectKind::Node, ObjectId(1)),
        ChangeState::Deleted
    );
    // The remaining nodes are unaffected.
    assert_eq!(
        editor.data().change_state(ObjectKind::Node, ObjectId(2)),
        ChangeState::Added
    );

    editor.undo().unwrap();
    assert_eq!(node_ids(&editor, 10), vec![ObjectId(1), ObjectId(2)]);
    assert_eq!(
        editor.data().change_state(ObjectKind::Node, ObjectId(1)),
        ChangeState::Added
    );
}

#[test]
fn node_shared_by_two_ways_is_removed_from_both() {
    let mut editor = Editor::new(DataSet::new());
    for id in 1..=5 {
        add_node(&mut editor, id);
    }
    // Shared node 3.
    add_way(&mut editor, 10, &[1, 2, 3]);
    add_way(&mut editor, 11, &[3, 4, 5]);

    let plan = plan_delete(&mut editor, &[(ObjectKind::Node, ObjectId(3))]).unwrap();
    assert_eq!(plan.touched_ways, vec![ObjectId(10), ObjectId(11)]);
    assert!(plan.reaped_ways.is_empty());
    editor.execute_batch(plan.commands).unwrap();

    assert_eq!(node_ids(&editor, 10), vec![ObjectId(1), ObjectId(2)]);
    assert_eq!(node_ids(&editor, 11), vec![ObjectId(4), ObjectId(5)]);
}

#[test]
fn deleting_node_drops_direct_relation_membership() {
    let mut editor = Editor::new(DataSet::new());
    for id in 1..=2 {
        add_node(&mut editor, id);
    }
    add_relation(
        &mut editor,
        20,
        vec![
            member(ObjectKind::Node, 1, "label"),
            member(ObjectKind::Node, 2, "label"),
        ],
    );

    let plan = plan_delete(&mut editor, &[(ObjectKind::Node, ObjectId(1))]).unwrap();
    assert_eq!(plan.touched_relations, vec![ObjectId(20)]);
    editor.execute_batch(plan.commands).unwrap();

    let members = editor.relation_members(ObjectId(20)).unwrap();
    assert_eq!(members.len(), 1);
    assert_eq!(members[0].id, ObjectId(2));
    // This relation was newly created locally (change_state = Added); SetRelationMembers only
    // promotes it to Modified when Unchanged; assert here that it was not wrongly marked Deleted.
    assert_ne!(
        editor.data().change_state(ObjectKind::Relation, ObjectId(20)),
        ChangeState::Deleted
    );
}

#[test]
fn deleting_way_drops_parent_relation_membership_instead_of_refusing() {
    let mut editor = Editor::new(DataSet::new());
    add_node(&mut editor, 1);
    add_node(&mut editor, 2);
    add_way(&mut editor, 10, &[1, 2]);
    add_relation(&mut editor, 20, vec![member(ObjectKind::Way, 10, "outer")]);

    // Before the fix: DeleteObject would fail outright with "way 10 is still referenced by relations".
    let plan = plan_delete(&mut editor, &[(ObjectKind::Way, ObjectId(10))]).unwrap();
    assert_eq!(plan.touched_relations, vec![ObjectId(20)]);
    editor.execute_batch(plan.commands).unwrap();

    assert!(editor.relation_members(ObjectId(20)).unwrap().is_empty());
    assert_eq!(
        editor.data().change_state(ObjectKind::Way, ObjectId(10)),
        ChangeState::Deleted
    );
}

#[test]
fn deleting_relation_drops_parent_relation_membership() {
    let mut editor = Editor::new(DataSet::new());
    add_relation(&mut editor, 20, Vec::new());
    add_relation(&mut editor, 21, vec![member(ObjectKind::Relation, 20, "sub")]);

    let plan = plan_delete(&mut editor, &[(ObjectKind::Relation, ObjectId(20))]).unwrap();
    assert_eq!(plan.touched_relations, vec![ObjectId(21)]);
    editor.execute_batch(plan.commands).unwrap();

    assert!(editor.relation_members(ObjectId(21)).unwrap().is_empty());
    assert_eq!(
        editor.data().change_state(ObjectKind::Relation, ObjectId(20)),
        ChangeState::Deleted
    );
}

#[test]
fn selecting_node_together_with_its_way_deletes_both() {
    // The real-world "delete 0/2" case: select a node together with its way.
    let mut editor = Editor::new(DataSet::new());
    add_node(&mut editor, 1);
    add_node(&mut editor, 2);
    add_way(&mut editor, 10, &[1, 2]);

    let plan = plan_delete(
        &mut editor,
        &[(ObjectKind::Node, ObjectId(1)), (ObjectKind::Way, ObjectId(10))],
    )
    .unwrap();
    // The way itself is in the delete set, so it must not have nodes removed again or be deleted twice.
    assert!(plan.reaped_ways.is_empty());
    assert!(plan.touched_ways.is_empty());
    let undo_before = editor.history().undo_len();
    editor.execute_batch(plan.commands).unwrap();

    assert_eq!(
        editor.data().change_state(ObjectKind::Way, ObjectId(10)),
        ChangeState::Deleted
    );
    assert_eq!(
        editor.data().change_state(ObjectKind::Node, ObjectId(1)),
        ChangeState::Deleted
    );
    assert_eq!(editor.history().undo_len(), undo_before + 1);
}

#[test]
fn missing_target_reports_not_found_without_mutating() {
    let mut editor = Editor::new(DataSet::new());
    add_node(&mut editor, 1);

    let err = plan_delete(&mut editor, &[(ObjectKind::Node, ObjectId(999))]).unwrap_err();
    assert!(matches!(err, neo_editor::CommandError::NotFound { .. }));
    assert_eq!(editor.history().undo_len(), 1);
}
