//! 编辑核心回归测试：本地 ID 稳定性、undo 落点、事务回滚、journal 与上传回写。

use std::sync::{Arc, Mutex};

use neo_core::{LatLon, Meta, ObjectId, ObjectKind};
use neo_editor::{
    AddNode, AddRelation, AddWay, DiffMapping, Editor, JournalFrame, JournalSink, MoveNode,
    RelationMemberSpec, SetRelationMembers, SetTags,
};
use neo_osm::{Node, Tags, Way};
use neo_store::{DataSet, MissingPolicy};

struct SharedSink(Arc<Mutex<Vec<JournalFrame>>>);

impl JournalSink for SharedSink {
    fn write_line(&mut self, line: &str) -> std::io::Result<()> {
        let frame = serde_json::from_str(line)
            .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err))?;
        self.0.lock().unwrap().push(frame);
        Ok(())
    }

    fn reset(&mut self) -> std::io::Result<()> {
        self.0.lock().unwrap().clear();
        Ok(())
    }
}

fn editor_with_sink() -> (Editor, Arc<Mutex<Vec<JournalFrame>>>) {
    let frames = Arc::new(Mutex::new(Vec::new()));
    let mut editor = Editor::new(DataSet::new());
    editor.set_journal_sink(Box::new(SharedSink(frames.clone())));
    (editor, frames)
}

fn lon(id: f64) -> LatLon {
    LatLon::from_degrees(47.0, 8.0 + id * 0.001)
}

#[test]
fn take_remote_replaces_object_and_version() {
    let (mut editor, _frames) = editor_with_sink();
    let mut add = AddNode::new(lon(0.0), vec![("name".into(), "local".into())]);
    add.id = Some(ObjectId(42));
    editor.execute(Box::new(add)).unwrap();

    let mut remote = DataSet::new();
    let key = remote.intern("name");
    let value = remote.intern("remote");
    let mut tags = Tags::new();
    tags.set(key, value);
    remote
        .push_node(Node {
            id: ObjectId(42),
            pos: lon(1.0),
            tags,
            meta: Meta {
                version: 7,
                ..Meta::new_local()
            },
        })
        .unwrap();
    editor
        .replace_from_remote(ObjectKind::Node, ObjectId(42), &remote)
        .unwrap();
    let handle = editor.data().node_handle(ObjectId(42)).unwrap();
    let view = editor.data().node_view(handle).unwrap();
    assert_eq!(view.info.meta.version, 7);
    assert!(
        view.tags()
            .any(|(key, value)| editor.data().resolve(key) == "name"
                && editor.data().resolve(value) == "remote")
    );
    assert_eq!(editor.data().changes.iter(ObjectKind::Node).len(), 0);
}

#[test]
fn partial_upload_keeps_unuploaded_changes() {
    let (mut editor, _frames) = editor_with_sink();
    editor
        .execute(Box::new(AddNode::new(lon(0.0), Vec::new())))
        .unwrap();
    editor
        .execute(Box::new(AddNode::new(lon(1.0), Vec::new())))
        .unwrap();
    let ids: Vec<ObjectId> = editor.data().iter_nodes().map(|node| node.id).collect();
    let mappings = vec![DiffMapping {
        kind: ObjectKind::Node,
        old_id: ids[0],
        new_id: ObjectId(1001),
        new_version: Some(1),
    }];
    let applied = editor.apply_uploaded(&mappings, false);
    assert_eq!(applied, 1);
    assert_eq!(editor.data().node_count(), 2);
    assert_eq!(editor.data().changes.iter(ObjectKind::Node).len(), 1);
    assert!(editor.history().undo_len() == 0);
}

#[test]
fn relation_members_can_be_added_reordered_and_removed_with_undo() {
    let (mut editor, _frames) = editor_with_sink();
    editor
        .execute(Box::new(AddNode::new(lon(0.0), Vec::new())))
        .unwrap();
    editor
        .execute(Box::new(AddNode::new(lon(1.0), Vec::new())))
        .unwrap();
    let nodes: Vec<ObjectId> = editor.data().iter_nodes().map(|node| node.id).collect();
    assert_eq!(nodes.len(), 2);
    let members = vec![
        RelationMemberSpec {
            kind: ObjectKind::Node,
            id: nodes[0],
            role: "outer".into(),
        },
        RelationMemberSpec {
            kind: ObjectKind::Node,
            id: nodes[1],
            role: "inner".into(),
        },
    ];
    editor
        .execute(Box::new(AddRelation::new(members.clone(), Vec::new())))
        .unwrap();
    let relation = editor.data().iter_relations().next().unwrap().id;
    assert_eq!(editor.relation_members(relation).unwrap().len(), 2);

    // 删掉第二个成员并改第一个角色。
    let mut changed = members.clone();
    changed.remove(1);
    changed[0].role = "outer".into();
    editor
        .execute(Box::new(SetRelationMembers::new(relation, changed)))
        .unwrap();
    assert_eq!(editor.relation_members(relation).unwrap().len(), 1);
    editor.undo().unwrap();
    assert_eq!(editor.relation_members(relation).unwrap().len(), 2);
    editor.redo().unwrap();
    assert_eq!(editor.relation_members(relation).unwrap().len(), 1);
}

#[test]
fn add_relation_with_missing_member_is_rejected() {
    let (mut editor, _frames) = editor_with_sink();
    let result = editor.execute(Box::new(AddRelation::new(
        vec![RelationMemberSpec {
            kind: ObjectKind::Node,
            id: ObjectId(999),
            role: String::new(),
        }],
        Vec::new(),
    )));
    assert!(result.is_err());
    assert_eq!(editor.data().relation_count(), 0);
}

#[test]
fn add_node_keeps_id_across_undo_and_redo() {
    let (mut editor, _frames) = editor_with_sink();
    editor
        .execute(Box::new(AddNode::new(lon(0.0), Vec::new())))
        .unwrap();
    let id = editor.data().iter_nodes().next().unwrap().id;
    editor.undo().unwrap();
    assert_eq!(editor.data().node_count(), 0);
    editor.redo().unwrap();
    assert_eq!(editor.data().iter_nodes().next().unwrap().id, id);
}

#[test]
fn move_node_undo_returns_to_constructed_origin() {
    let (mut editor, _frames) = editor_with_sink();
    editor
        .execute(Box::new(AddNode::new(lon(0.0), Vec::new())))
        .unwrap();
    let id = editor.data().iter_nodes().next().unwrap().id;
    let from = lon(0.0);
    let to = lon(5.0);
    editor.execute(Box::new(MoveNode::new(id, from, to))).unwrap();
    assert_eq!(editor.data().iter_nodes().next().unwrap().pos, to);
    editor.undo().unwrap();
    assert_eq!(
        editor.data().iter_nodes().next().unwrap().pos,
        from,
        "undo must use MoveNode::from, not the position at apply time"
    );
}

#[test]
fn duplicate_add_is_rejected_without_side_effects() {
    let (mut editor, _frames) = editor_with_sink();
    editor
        .execute(Box::new(AddNode::new(lon(0.0), Vec::new())))
        .unwrap();
    let id = editor.data().iter_nodes().next().unwrap().id;
    let mut duplicate = AddNode::new(lon(1.0), Vec::new());
    duplicate.id = Some(id);
    assert!(editor.execute(Box::new(duplicate)).is_err());
    assert_eq!(editor.data().node_count(), 1);
    assert_eq!(editor.history().undo_len(), 1);
}

#[test]
fn journal_records_real_auto_id() {
    let (mut editor, frames) = editor_with_sink();
    editor
        .execute(Box::new(AddNode::new(lon(0.0), Vec::new())))
        .unwrap();
    let id = editor.data().iter_nodes().next().unwrap().id;
    let frames = frames.lock().unwrap();
    let recorded = frames.iter().find_map(|frame| match frame {
        JournalFrame::Cmd {
            command: neo_editor::JournalCommand::AddNode { id, .. },
        } => Some(*id),
        _ => None,
    });
    assert_eq!(recorded, Some(id.0), "journal must not write id 0");
}

#[test]
fn undo_rebases_journal_so_crash_cannot_resurrect_edits() {
    let (mut editor, frames) = editor_with_sink();
    editor
        .execute(Box::new(AddNode::new(lon(0.0), Vec::new())))
        .unwrap();
    editor.undo().unwrap();
    let frames = frames.lock().unwrap();
    assert!(
        matches!(frames.last(), Some(JournalFrame::Rebase { commands }) if commands.is_empty()),
        "undo must replace the journal with the current applied history"
    );
}

#[test]
fn set_tags_undo_restores_change_state() {
    let (mut editor, _frames) = editor_with_sink();
    let node = Node {
        id: ObjectId(1),
        pos: lon(0.0),
        tags: Tags::new(),
        meta: neo_core::Meta::new_local(),
    };
    editor.import_node(node).unwrap();
    editor
        .execute(Box::new(SetTags::new(
            ObjectKind::Node,
            ObjectId(1),
            vec![("name".into(), "A".into())],
        )))
        .unwrap();
    assert_eq!(
        editor.data().change_state(ObjectKind::Node, ObjectId(1)),
        neo_osm::ChangeState::Modified
    );
    editor.undo().unwrap();
    assert_eq!(
        editor.data().change_state(ObjectKind::Node, ObjectId(1)),
        neo_osm::ChangeState::Unchanged
    );
}

#[test]
fn apply_diff_result_remaps_local_ids_and_clears_changes() {
    let (mut editor, _frames) = editor_with_sink();
    editor
        .execute(Box::new(AddNode::new(lon(0.0), Vec::new())))
        .unwrap();
    let local = editor.data().iter_nodes().next().unwrap().id;
    assert!(local.is_local());
    editor.apply_diff_result(&[DiffMapping {
        kind: ObjectKind::Node,
        old_id: local,
        new_id: ObjectId(9001),
        new_version: Some(3),
    }]);
    let view = editor.data().iter_nodes().next().unwrap();
    assert_eq!(view.id, ObjectId(9001));
    assert_eq!(view.meta.version, 3);
    assert!(!editor.data().has_pending_changes());
    assert!(editor.data().node_handle(local).is_none());
}

#[test]
fn add_way_with_missing_node_does_not_mutate() {
    let (mut editor, _frames) = editor_with_sink();
    let result = editor.execute(Box::new(AddWay::new(vec![ObjectId(1), ObjectId(2)], Vec::new())));
    assert!(result.is_err());
    assert_eq!(editor.data().way_count(), 0);
    assert!(!editor.data().has_pending_changes());
}

#[test]
fn import_from_dataset_copies_selected_objects_with_remapped_ids() {
    let (mut editor, _frames) = editor_with_sink();
    assert_eq!(editor.data().node_count(), 0);

    // 远端：2 个节点 + 引用它们的 1 条 way。
    let mut remote = DataSet::new();
    remote
        .push_node(Node {
            id: ObjectId(10),
            pos: lon(0.0),
            tags: Tags::new(),
            meta: Meta::new_local(),
        })
        .unwrap();
    remote
        .push_node(Node {
            id: ObjectId(11),
            pos: lon(1.0),
            tags: Tags::new(),
            meta: Meta::new_local(),
        })
        .unwrap();
    remote
        .push_way(
            Way {
                id: ObjectId(20),
                nodes: vec![ObjectId(10), ObjectId(11)].into(),
                tags: Tags::new(),
                meta: Meta::new_local(),
            },
            MissingPolicy::Error,
        )
        .unwrap()
        .unwrap();

    let created = editor
        .import_from_dataset(
            &remote,
            &[
                (ObjectKind::Node, ObjectId(10)),
                (ObjectKind::Node, ObjectId(11)),
                (ObjectKind::Way, ObjectId(20)),
            ],
            0.0,
        )
        .unwrap();
    assert_eq!(created, 3);
    assert_eq!(editor.data().node_count(), 2);
    assert_eq!(editor.data().way_count(), 1);
    // 一个命令批 = 一条历史记录。
    assert_eq!(editor.history().undo_len(), 1);

    let mut new_nodes: Vec<ObjectId> = editor.data().iter_nodes().map(|node| node.id).collect();
    assert!(new_nodes.iter().all(|id| id.is_local()));
    let way = editor.data().iter_ways().next().unwrap();
    assert!(way.id.is_local());
    let way_view = editor.data().way_view(way.handle).unwrap();
    let mut referenced: Vec<ObjectId> = way_view
        .node_refs
        .iter()
        .filter_map(|handle| editor.data().nodes.id(*handle))
        .collect();
    assert_eq!(referenced.len(), 2);
    new_nodes.sort_unstable_by_key(|id| id.0);
    referenced.sort_unstable_by_key(|id| id.0);
    assert_eq!(referenced, new_nodes);
    // 远端 ID 没有被直接带进活动数据集。
    assert!(editor.data().node_handle(ObjectId(10)).is_none());
    assert!(editor.data().node_handle(ObjectId(11)).is_none());
    assert!(editor.data().way_handle(ObjectId(20)).is_none());
}

#[test]
fn merge_from_dataset_keeps_local_edits_and_adds_new_ids() {
    let (mut editor, _frames) = editor_with_sink();
    let mut add = AddNode::new(lon(0.0), vec![("name".into(), "local".into())]);
    add.id = Some(ObjectId(1));
    editor.execute(Box::new(add)).unwrap();
    // 弄脏本地对象：合并时应跳过。
    editor
        .execute(Box::new(SetTags::new(
            ObjectKind::Node,
            ObjectId(1),
            vec![("name".into(), "edited".into())],
        )))
        .unwrap();

    let mut remote = DataSet::new();
    let key = remote.intern("name");
    let local_name = remote.intern("remote-1");
    let mut tags = Tags::new();
    tags.set(key, local_name);
    remote
        .push_node(Node {
            id: ObjectId(1),
            pos: lon(9.0),
            tags,
            meta: {
                let mut meta = Meta::new_local();
                meta.version = 3;
                meta
            },
        })
        .unwrap();
    let mut tags2 = Tags::new();
    tags2.set(key, remote.intern("new"));
    remote
        .push_node(Node {
            id: ObjectId(2),
            pos: lon(1.0),
            tags: tags2,
            meta: {
                let mut meta = Meta::new_local();
                meta.version = 1;
                meta
            },
        })
        .unwrap();

    let stats = editor.merge_from_dataset(&remote).unwrap();
    assert_eq!(stats.added, 1);
    assert_eq!(stats.skipped, 1);
    assert_eq!(stats.updated, 0);
    assert_eq!(editor.data().node_count(), 2);
    assert!(editor.data().node_handle(ObjectId(2)).is_some());
    // 本地脏对象保留编辑后的标签。
    let handle = editor.data().node_handle(ObjectId(1)).unwrap();
    let view = editor.data().node_view(handle).unwrap();
    let name = view
        .tag_pairs
        .iter()
        .find(|(k, _)| editor.data().resolve(*k) == "name")
        .map(|(_, v)| editor.data().resolve(*v));
    assert_eq!(name, Some("edited"));
}
