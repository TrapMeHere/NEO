//! IO regression tests: XML timestamp/action, osmChange entities and delete order, PBF round-trip.

use std::io::Cursor;

use neo_core::{LatLon, Meta, ObjectId, ObjectKind};
use neo_io::osc_read::read_osm_change;
use neo_io::pbf_write::{PbfWriteOptions, write_pbf_file};
use neo_io::xml::{XmlReadOptions, XmlWriteMode, read_osm_xml, write_osm_xml};
use neo_io::{collect_changeset, collect_changeset_selected, stamp_osmchange_changeset, write_changeset};
use neo_osm::{ChangeState, Node, Relation, RelationMember, Tags, Way};
use neo_store::{DataSet, MissingPolicy};

fn node(id: i64, lat: f64, lon: f64) -> Node {
    Node {
        id: ObjectId(id),
        pos: LatLon::from_degrees(lat, lon),
        tags: Tags::new(),
        meta: Meta::new_local(),
    }
}

fn push_tagged(data: &mut DataSet, id: i64, lat: f64, lon: f64, tags: &[(&str, &str)]) {
    let mut pairs = Vec::new();
    for (key, value) in tags {
        pairs.push((data.intern(key), data.intern(value)));
    }
    data.push_node(Node {
        id: ObjectId(id),
        pos: LatLon::from_degrees(lat, lon),
        tags: Tags::from_pairs(pairs),
        meta: Meta::new_local(),
    })
    .unwrap();
}

#[test]
fn xml_reads_iso_timestamp_and_josm_action() {
    let xml = r#"<?xml version="1.0"?>
<osm version="0.6">
  <node id="1" lat="47.0" lon="8.0" version="3" timestamp="2024-02-29T12:34:56Z" action="modify"/>
  <node id="-1" lat="47.1" lon="8.1"/>
  <node id="2" lat="47.2" lon="8.2" visible="false"/>
</osm>"#;
    let mut data = DataSet::new();
    read_osm_xml(&mut data, Cursor::new(xml), XmlReadOptions::default()).unwrap();
    let first = data.node_handle(ObjectId(1)).unwrap();
    assert_eq!(data.nodes.get(first).unwrap().meta.timestamp, 1_709_210_096);
    assert_eq!(
        data.change_state(ObjectKind::Node, ObjectId(1)),
        ChangeState::Modified
    );
    assert_eq!(
        data.change_state(ObjectKind::Node, ObjectId(-1)),
        ChangeState::Added
    );
    assert_eq!(
        data.change_state(ObjectKind::Node, ObjectId(2)),
        ChangeState::Deleted
    );
}

#[test]
fn xml_write_emits_iso_timestamp_not_epoch_integer() {
    let mut data = DataSet::new();
    push_tagged(&mut data, 1, 47.0, 8.0, &[("name", "A&B")]);
    let handle = data.node_handle(ObjectId(1)).unwrap();
    let mut meta = data.nodes.get(handle).unwrap().meta;
    meta.timestamp = 1_709_210_096;
    data.nodes.set_meta(handle, meta);
    let mut out = Vec::new();
    write_osm_xml(&data, &mut out, XmlWriteMode::Server).unwrap();
    let text = String::from_utf8(out).unwrap();
    assert!(text.contains("timestamp=\"2024-02-29T12:34:56Z\""), "{text}");
    assert!(text.contains("A&amp;B"));
    assert!(!text.contains("timestamp=\"1709210096\""));
}

#[test]
fn xml_rejects_missing_coordinates_and_truncated_files() {
    let mut data = DataSet::new();
    let err = read_osm_xml(
        &mut data,
        Cursor::new("<osm><node id=\"1\"/></osm>"),
        XmlReadOptions::default(),
    );
    assert!(err.is_err());

    let err = read_osm_xml(
        &mut data,
        Cursor::new("<osm><node id=\"1\" lat=\"47\" lon=\"8\"/>"),
        XmlReadOptions::default(),
    );
    assert!(err.is_err(), "truncated XML must not be reported as success");
}

#[test]
fn osc_decodes_entities_and_orders_delete_by_dependency() {
    let osc = r#"<?xml version="1.0"?>
<osmChange version="0.6">
  <modify>
    <node id="1" lat="47.0" lon="8.0" version="2">
      <tag k="name" v="Fish &amp; Chips"/>
    </node>
  </modify>
</osmChange>"#;
    let mut data = DataSet::new();
    push_tagged(&mut data, 1, 47.0, 8.0, &[]);
    read_osm_change(&mut data, Cursor::new(osc), MissingPolicy::Error).unwrap();
    let name_key = data.intern("name");
    let handle = data.node_handle(ObjectId(1)).unwrap();
    let view = data.node_view(handle).unwrap();
    let name = data.resolve(view.tag(name_key).unwrap());
    assert_eq!(name, "Fish & Chips");

    // Delete order: relation → way → node.
    let mut data = DataSet::new();
    push_tagged(&mut data, 1, 47.0, 8.0, &[]);
    data.push_node(node(2, 47.1, 8.1)).unwrap();
    data.push_way(
        Way {
            id: ObjectId(10),
            nodes: vec![ObjectId(1), ObjectId(2)].into(),
            tags: Tags::new(),
            meta: Meta::new_local(),
        },
        MissingPolicy::Error,
    )
    .unwrap();
    data.set_change_state_visible(ObjectKind::Way, ObjectId(10), ChangeState::Deleted, false);
    data.set_change_state_visible(ObjectKind::Node, ObjectId(1), ChangeState::Deleted, false);
    let set = collect_changeset(&data).unwrap();
    let mut out = Vec::new();
    write_changeset(&set, &data, &mut out).unwrap();
    let text = String::from_utf8(out).unwrap();
    let way_pos = text.find("<way").expect("way delete");
    let node_pos = text.find("<node").expect("node delete");
    assert!(way_pos < node_pos, "delete must be relation→way→node:\n{text}");
    assert!(text.contains("if-unused=\"true\""));
}

#[test]
fn nested_relation_delete_emits_parent_before_child() {
    // The parent relation has a larger id than the child: sorting only by (kind, id) would emit the child first,
    // and the server returns 412 because "the child is still referenced by the parent" (with if-unused it is silently skipped).
    let mut data = DataSet::new();
    let role = data.intern("");
    data.push_relation(
        Relation {
            id: ObjectId(20),
            members: Vec::new().into(),
            tags: Tags::new(),
            meta: Meta::new_local(),
        },
        MissingPolicy::Error,
    )
    .unwrap();
    data.push_relation(
        Relation {
            id: ObjectId(30),
            members: vec![RelationMember {
                kind: ObjectKind::Relation,
                id: ObjectId(20),
                role,
            }]
            .into(),
            tags: Tags::new(),
            meta: Meta::new_local(),
        },
        MissingPolicy::Error,
    )
    .unwrap();
    data.set_change_state_visible(ObjectKind::Relation, ObjectId(20), ChangeState::Deleted, false);
    data.set_change_state_visible(ObjectKind::Relation, ObjectId(30), ChangeState::Deleted, false);

    let set = collect_changeset(&data).unwrap();
    let mut out = Vec::new();
    write_changeset(&set, &data, &mut out).unwrap();
    let text = String::from_utf8(out).unwrap();
    let parent = text.find("<relation id=\"30\"").expect("parent delete");
    let child = text.find("<relation id=\"20\"").expect("child delete");
    assert!(
        parent < child,
        "parent relation must be deleted before child relation:\n{text}"
    );
}

#[test]
fn stamp_osmchange_writes_changeset_on_each_object() {
    let osc = "<?xml version=\"1.0\"?>\n\
<osmChange version=\"0.6\" generator=\"NEO\">\n\
  <create>\n\
    <node id=\"-7\" lat=\"30.1\" lon=\"104.1\">\n\
      <tag k=\"amenity\" v=\"bench\"/>\n\
    </node>\n\
    <way id=\"-8\" version=\"1\">\n\
      <nd ref=\"-7\"/>\n\
    </way>\n\
  </create>\n\
</osmChange>\n";
    let stamped = stamp_osmchange_changeset(osc, 4242);
    assert!(stamped.contains("<node id=\"-7\" lat=\"30.1\" lon=\"104.1\" changeset=\"4242\">"));
    assert!(stamped.contains("<way id=\"-8\" version=\"1\" changeset=\"4242\">"));
    let again = stamp_osmchange_changeset(&stamped, 99);
    assert!(again.contains("changeset=\"99\""));
    assert!(!again.contains("changeset=\"4242\""));
}

#[test]
fn osc_omits_locally_created_then_deleted_objects() {
    let mut data = DataSet::new();
    data.push_node(node(-1, 47.0, 8.0)).unwrap();
    data.set_change_state_visible(ObjectKind::Node, ObjectId(-1), ChangeState::Deleted, false);
    let set = collect_changeset(&data).unwrap();
    assert!(set.delete.is_empty(), "local-only objects must not be uploaded");
}

#[test]
fn pbf_roundtrip_preserves_coordinates_tags_and_visibility() {
    let mut data = DataSet::new();
    push_tagged(&mut data, 1, 47.1234567, 8.7654321, &[("highway", "primary")]);
    push_tagged(&mut data, 2, 47.2, 8.8, &[]);
    data.push_way(
        Way {
            id: ObjectId(10),
            nodes: vec![ObjectId(1), ObjectId(2)].into(),
            tags: Tags::new(),
            meta: Meta::new_local(),
        },
        MissingPolicy::Error,
    )
    .unwrap();
    // Historical object: visible=false.
    data.set_change_state_visible(ObjectKind::Node, ObjectId(2), ChangeState::Deleted, false);

    let path = std::env::temp_dir().join(format!(
        "neo-pbf-test-{}-{}.pbf",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    write_pbf_file(&data, &path, PbfWriteOptions::default()).unwrap();
    let (back, report) = neo_io::import_pbf_with_report(&path).unwrap();
    let _ = std::fs::remove_file(&path);
    assert_eq!(report.nodes, 2);
    assert_eq!(report.ways, 1);
    let first = back.node_handle(ObjectId(1)).unwrap();
    assert_eq!(
        back.nodes.get(first).unwrap().pos,
        LatLon::from_degrees(47.1234567, 8.7654321)
    );
    let view = back.node_view(first).unwrap();
    assert!(
        view.tags()
            .any(|(key, value)| back.resolve(key) == "highway" && back.resolve(value) == "primary"),
        "tags must survive the PBF roundtrip"
    );
    assert!(
        !back
            .nodes
            .get(back.node_handle(ObjectId(2)).unwrap())
            .unwrap()
            .meta
            .visible
    );
}

#[test]
fn selected_changeset_pulls_in_new_dependencies() {
    let mut data = DataSet::new();
    for id in [-1, -2] {
        data.push_node(node(id, 47.0, 8.0)).unwrap();
        data.set_change_state(ObjectKind::Node, ObjectId(id), ChangeState::Added);
    }
    data.push_way(
        Way {
            id: ObjectId(-3),
            nodes: vec![ObjectId(-1), ObjectId(-2)].into(),
            tags: Tags::new(),
            meta: Meta::new_local(),
        },
        MissingPolicy::Error,
    )
    .unwrap();
    data.set_change_state(ObjectKind::Way, ObjectId(-3), ChangeState::Added);

    let mut selected = std::collections::HashSet::new();
    selected.insert((ObjectKind::Way, ObjectId(-3)));
    let set = collect_changeset_selected(&data, &selected).unwrap();
    assert_eq!(set.create.len(), 3);

    let mut only_node = std::collections::HashSet::new();
    only_node.insert((ObjectKind::Node, ObjectId(-1)));
    let set = collect_changeset_selected(&data, &only_node).unwrap();
    assert_eq!(set.create.len(), 1);
}

#[test]
fn chunking_keeps_create_dependencies_together() {
    let mut data = DataSet::new();
    for id in [-1, -2, -3, -4] {
        data.push_node(node(id, 47.0, 8.0)).unwrap();
        data.set_change_state(ObjectKind::Node, ObjectId(id), ChangeState::Added);
    }
    let set = collect_changeset(&data).unwrap();
    let chunks = set.chunks(2).unwrap();
    assert_eq!(chunks.len(), 2);
    assert_eq!(chunks[0].create.len(), 2);
    assert_eq!(chunks[1].create.len(), 2);

    data.push_way(
        Way {
            id: ObjectId(-5),
            nodes: vec![ObjectId(-1), ObjectId(-2)].into(),
            tags: Tags::new(),
            meta: Meta::new_local(),
        },
        MissingPolicy::Error,
    )
    .unwrap();
    data.set_change_state(ObjectKind::Way, ObjectId(-5), ChangeState::Added);
    let set = collect_changeset(&data).unwrap();
    // 4 nodes + 1 way; the way and two of the nodes must share a block, so 2 cannot fit.
    assert!(set.chunks(2).is_err());
    let chunks = set.chunks(5).unwrap();
    assert_eq!(chunks.len(), 1);
}
