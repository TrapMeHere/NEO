//! 合并 / 分割对 relation 成员的级联修正（JOSM 语义）。

use neo_core::{LatLon, ObjectId, ObjectKind};
use neo_editor::{
    AddNode, AddRelation, AddWay, CommandError, Editor, MergeWay, RelationMemberSpec, SetRelationMembers,
    SplitWay,
};
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

fn add_relation(editor: &mut Editor, id: i64, members: Vec<(&str, i64, &str)>) {
    let members = members
        .into_iter()
        .map(|(kind, id, role)| RelationMemberSpec {
            kind: match kind {
                "way" => ObjectKind::Way,
                "rel" => ObjectKind::Relation,
                _ => ObjectKind::Node,
            },
            id: ObjectId(id),
            role: role.to_owned(),
        })
        .collect();
    let mut cmd = AddRelation::new(members, Vec::new());
    cmd.id = Some(ObjectId(id));
    editor.execute(Box::new(cmd)).unwrap();
}

fn member_ids(editor: &Editor, relation: i64) -> Vec<(ObjectKind, i64, String)> {
    editor
        .relation_members(ObjectId(relation))
        .unwrap()
        .into_iter()
        .map(|m| (m.kind, m.id.0, m.role))
        .collect()
}

#[test]
fn merge_moves_relation_membership_from_b_to_a() {
    let mut editor = Editor::new(DataSet::new());
    for id in 1..=3 {
        add_node(&mut editor, id);
    }
    add_way(&mut editor, 10, &[1, 2]);
    add_way(&mut editor, 11, &[2, 3]);
    add_relation(
        &mut editor,
        20,
        vec![("way", 10, "forward"), ("way", 11, "forward")],
    );

    // 修复前：MergeWay 会以 "way 11 is still referenced by relations" 直接失败。
    editor
        .execute(Box::new(MergeWay::new(ObjectId(10), ObjectId(11))))
        .unwrap();

    // b 的成员被就地换成 a，角色保留；重复的 (way 10, forward) 合并掉。
    assert_eq!(
        member_ids(&editor, 20),
        vec![(ObjectKind::Way, 10, "forward".to_owned())]
    );
    assert_eq!(
        editor.data().change_state(ObjectKind::Way, ObjectId(11)),
        ChangeState::Deleted
    );

    editor.undo().unwrap();
    assert_eq!(
        member_ids(&editor, 20),
        vec![
            (ObjectKind::Way, 10, "forward".to_owned()),
            (ObjectKind::Way, 11, "forward".to_owned())
        ]
    );
    assert_ne!(
        editor.data().change_state(ObjectKind::Way, ObjectId(11)),
        ChangeState::Deleted
    );
}

#[test]
fn merge_keeps_role_when_only_b_is_a_member() {
    let mut editor = Editor::new(DataSet::new());
    for id in 1..=3 {
        add_node(&mut editor, id);
    }
    add_way(&mut editor, 10, &[1, 2]);
    add_way(&mut editor, 11, &[2, 3]);
    add_relation(&mut editor, 20, vec![("way", 11, "inner")]);

    editor
        .execute(Box::new(MergeWay::new(ObjectId(10), ObjectId(11))))
        .unwrap();

    assert_eq!(
        member_ids(&editor, 20),
        vec![(ObjectKind::Way, 10, "inner".to_owned())]
    );
}

#[test]
fn split_appends_new_way_to_relations_with_same_role() {
    let mut editor = Editor::new(DataSet::new());
    for id in 1..=3 {
        add_node(&mut editor, id);
    }
    add_way(&mut editor, 10, &[1, 2, 3]);
    add_relation(&mut editor, 20, vec![("way", 10, "outer")]);

    editor.execute(Box::new(SplitWay::new(ObjectId(10), 1))).unwrap();

    let members = member_ids(&editor, 20);
    assert_eq!(members.len(), 2, "新 way 必须进入原 way 所在的 relation");
    assert_eq!(members[0], (ObjectKind::Way, 10, "outer".to_owned()));
    assert_eq!(members[1].0, ObjectKind::Way);
    assert_eq!(members[1].2, "outer");
    let new_way = members[1].1;
    assert_ne!(new_way, 10);

    editor.undo().unwrap();
    assert_eq!(
        member_ids(&editor, 20),
        vec![(ObjectKind::Way, 10, "outer".to_owned())]
    );
    assert!(editor.data().way_handle(ObjectId(new_way)).is_none());
}

#[test]
fn relation_membership_cycle_is_rejected() {
    let mut editor = Editor::new(DataSet::new());
    add_relation(&mut editor, 20, vec![]);
    add_relation(&mut editor, 21, vec![("rel", 20, "")]);

    // 21 已包含 20；再让 20 包含 21 就成环（服务端会 412）。
    let err = editor
        .execute(Box::new(SetRelationMembers::new(
            ObjectId(20),
            vec![RelationMemberSpec {
                kind: ObjectKind::Relation,
                id: ObjectId(21),
                role: String::new(),
            }],
        )))
        .unwrap_err();
    assert!(matches!(err, CommandError::Invalid(_)), "got {err:?}");
    assert!(editor.relation_members(ObjectId(20)).unwrap().is_empty());
}

#[test]
fn relation_cannot_contain_itself() {
    let mut editor = Editor::new(DataSet::new());
    add_relation(&mut editor, 20, vec![]);

    let err = editor
        .execute(Box::new(SetRelationMembers::new(
            ObjectId(20),
            vec![RelationMemberSpec {
                kind: ObjectKind::Relation,
                id: ObjectId(20),
                role: String::new(),
            }],
        )))
        .unwrap_err();
    assert!(matches!(err, CommandError::Invalid(_)), "got {err:?}");
}
