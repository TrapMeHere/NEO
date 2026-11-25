//! osmChange 0.6 output (the upload diff format).
//!
//! Three spec facts worth remembering (docs/09):
//!
//! 1. The file root element is `<osmChange version="0.6">`; **there is no "OSC 0.3"**;
//! 2. Upload is a single `application/xml` request body (not multipart; multipart is only for GPX);
//! 3. Local new objects use **negative id placeholders** (in the `create` block); the server returns `<diffResult>` mapping them to positive ids.
//!
//! XML serialization is hand-written here rather than using quick-xml's writer: output is small, fully controllable, and
//! easy to golden-test byte by byte; the reading side (XML import) uses quick-xml.

use std::collections::{BTreeSet, HashMap};
use std::io::{self, Write};

use neo_core::{ObjectId, ObjectKind};
use neo_osm::{ChangeState, Element};
use neo_store::DataSet;

/// Changes needed for one upload (grouped by osmChange action).
#[derive(Clone, Default)]
pub struct ChangeSet {
    /// New objects.
    pub create: Vec<Element>,
    /// Modified objects.
    pub modify: Vec<Element>,
    /// Deleted objects.
    pub delete: Vec<Element>,
}

impl ChangeSet {
    /// Whether it is empty.
    pub fn is_empty(&self) -> bool {
        self.create.is_empty() && self.modify.is_empty() && self.delete.is_empty()
    }

    /// Total element count.
    pub fn len(&self) -> usize {
        self.create.len() + self.modify.len() + self.delete.len()
    }

    /// Splits into batches of at most `max` elements each, respecting dependencies.
    ///
    /// New objects are union-found by "way/relation references a new node/member", so a reference group is never
    /// split across two diffs; modifications/deletions are chunked in deterministic order. The deletion order is already
    /// sorted as relation → way → node in [`collect_changeset`], and chunking preserves it.
    pub fn chunks(&self, max: usize) -> Result<Vec<ChangeSet>, neo_core::NeoError> {
        use std::collections::HashMap;
        if max == 0 {
            return Err(neo_core::NeoError::Parse("chunk size must be > 0".into()));
        }
        if self.len() <= max {
            return Ok(vec![self.clone()]);
        }
        // Modified objects may reference objects newly created in the same batch (negative ids). Across diffs the
        // server has already mapped negative ids to positive ones, but later diffs writing negative ids would fail;
        // until references are rewritten per chunk, large mixed create+modify changesets are not split, to avoid
        // uploading half-broken data.
        if !self.create.is_empty() && !self.modify.is_empty() {
            return Err(neo_core::NeoError::Parse(
                "mixed create+modify changeset exceeds chunk size; split the edit".into(),
            ));
        }
        let mut out = Vec::new();
        if !self.create.is_empty() {
            let mut parent: Vec<usize> = (0..self.create.len()).collect();
            fn find(parent: &mut [usize], mut index: usize) -> usize {
                while parent[index] != index {
                    parent[index] = parent[parent[index]];
                    index = parent[index];
                }
                index
            }
            fn union(parent: &mut [usize], a: usize, b: usize) {
                let ra = find(parent, a);
                let rb = find(parent, b);
                if ra != rb {
                    parent[rb] = ra;
                }
            }
            let mut index_by = HashMap::new();
            for (index, element) in self.create.iter().enumerate() {
                index_by.insert((element.kind(), element.id()), index);
            }
            for (index, element) in self.create.iter().enumerate() {
                match element {
                    Element::Way(way) => {
                        for node in &way.nodes {
                            if let Some(target) = index_by.get(&(ObjectKind::Node, *node)).copied() {
                                union(&mut parent, index, target);
                            }
                        }
                    }
                    Element::Relation(relation) => {
                        for member in &relation.members {
                            if let Some(target) = index_by.get(&(member.kind, member.id)).copied() {
                                union(&mut parent, index, target);
                            }
                        }
                    }
                    Element::Node(_) => {}
                }
            }
            let mut groups: HashMap<usize, Vec<Element>> = HashMap::new();
            for (index, element) in self.create.iter().enumerate() {
                let root = find(&mut parent, index);
                groups.entry(root).or_default().push(element.clone());
            }
            let mut current = ChangeSet::default();
            for group in groups.into_values() {
                if group.len() > max {
                    return Err(neo_core::NeoError::Parse(
                        "one connected create group exceeds the upload chunk size".into(),
                    ));
                }
                if !current.create.is_empty() && current.create.len() + group.len() > max {
                    out.push(current);
                    current = ChangeSet::default();
                }
                current.create.extend(group);
            }
            if !current.create.is_empty() {
                out.push(current);
            }
        }
        for chunk in self.modify.chunks(max) {
            out.push(ChangeSet {
                modify: chunk.to_vec(),
                ..ChangeSet::default()
            });
        }
        for chunk in self.delete.chunks(max) {
            out.push(ChangeSet {
                delete: chunk.to_vec(),
                ..ChangeSet::default()
            });
        }
        Ok(out)
    }
}

/// Builds the [`ChangeSet`] for osmChange from a dataset and change tracking.
///
/// Semantics:
/// * `Added` → create (local negative ids are emitted as-is; the server maps them);
/// * `Modified` → modify (must carry the server version);
/// * `Deleted` → delete (also carrying the version);
/// * `Unchanged` is not emitted.
///
/// Collects the changes of **selected objects**; dependencies of new objects (new nodes/new members) are included automatically.
///
/// The selection is filtered by `(kind, id)`; unselected modifications are not uploaded, which suits "small commits".
pub fn collect_changeset_selected(
    data: &DataSet,
    selected: &std::collections::HashSet<(ObjectKind, ObjectId)>,
) -> Result<ChangeSet, neo_core::NeoError> {
    let mut include = selected.clone();
    let mut stack: Vec<(ObjectKind, ObjectId)> = selected.iter().copied().collect();
    while let Some((kind, id)) = stack.pop() {
        if data.change_state(kind, id) != ChangeState::Added {
            continue;
        }
        match kind {
            ObjectKind::Way => {
                if let Some(handle) = data.way_handle(id) {
                    for node in data.way_node_ids(handle) {
                        if data.change_state(ObjectKind::Node, node) == ChangeState::Added
                            && include.insert((ObjectKind::Node, node))
                        {
                            stack.push((ObjectKind::Node, node));
                        }
                    }
                }
            }
            ObjectKind::Relation => {
                if let Some(handle) = data.relation_handle(id)
                    && let Some(view) = data.relation_view(handle)
                {
                    for member in view.members {
                        let member_id = match member.target.kind {
                            ObjectKind::Node => member.target.as_node().and_then(|h| data.nodes.id(h)),
                            ObjectKind::Way => member.target.as_way().and_then(|h| data.ways.id(h)),
                            ObjectKind::Relation => {
                                member.target.as_relation().and_then(|h| data.relations.id(h))
                            }
                            ObjectKind::Changeset => None,
                        };
                        if let Some(member_id) = member_id
                            && data.change_state(member.target.kind, member_id) == ChangeState::Added
                            && include.insert((member.target.kind, member_id))
                        {
                            stack.push((member.target.kind, member_id));
                        }
                    }
                }
            }
            _ => {}
        }
    }
    collect_changeset_filtered(data, &include)
}

fn collect_changeset_filtered(
    data: &DataSet,
    include: &std::collections::HashSet<(ObjectKind, ObjectId)>,
) -> Result<ChangeSet, neo_core::NeoError> {
    let mut set = ChangeSet::default();
    for kind in ObjectKind::EDITABLE {
        for (id, state) in data.changes.iter(kind) {
            if !include.contains(&(kind, id)) {
                continue;
            }
            let Some(element) = element_of(data, kind, id)? else {
                continue;
            };
            match state {
                ChangeState::Added => set.create.push(element),
                ChangeState::Modified => set.modify.push(element),
                ChangeState::Deleted if id.is_local() => {}
                ChangeState::Deleted => set.delete.push(element),
                ChangeState::Unchanged => {}
            }
        }
    }
    set.create
        .sort_by_key(|element| (kind_order(element.kind()), element.id().0));
    set.modify
        .sort_by_key(|element| (kind_order(element.kind()), element.id().0));
    sort_deletes(data, &mut set.delete);
    Ok(set)
}

/// Collects the changes of the whole dataset (create/modify/delete grouping, deterministic ordering).
pub fn collect_changeset(data: &DataSet) -> Result<ChangeSet, neo_core::NeoError> {
    let mut set = ChangeSet::default();
    for kind in ObjectKind::EDITABLE {
        for (id, state) in data.changes.iter(kind) {
            let Some(element) = element_of(data, kind, id)? else {
                continue;
            };
            match state {
                ChangeState::Added => set.create.push(element),
                ChangeState::Modified => set.modify.push(element),
                // Objects created locally and then deleted never existed on the server: uploading `<delete>` with a
                // negative id would fail the whole changeset, so they must be dropped from the diff.
                ChangeState::Deleted if id.is_local() => {}
                ChangeState::Deleted => set.delete.push(element),
                ChangeState::Unchanged => {}
            }
        }
    }
    // Deterministic output: the same edit set exports identical bytes every time (golden test/review/cache).
    set.create
        .sort_by_key(|element| (kind_order(element.kind()), element.id().0));
    set.modify
        .sort_by_key(|element| (kind_order(element.kind()), element.id().0));
    // Deletion must follow dependency order; the server executes in document order, so deleting an object that is
    // still referenced first makes the whole request fail with 412.
    sort_deletes(data, &mut set.delete);
    Ok(set)
}

fn kind_order(kind: ObjectKind) -> u8 {
    match kind {
        ObjectKind::Node => 0,
        ObjectKind::Way => 1,
        ObjectKind::Relation => 2,
        ObjectKind::Changeset => 3,
    }
}

fn delete_order(kind: ObjectKind) -> u8 {
    match kind {
        ObjectKind::Relation => 0,
        ObjectKind::Way => 1,
        ObjectKind::Node => 2,
        ObjectKind::Changeset => 3,
    }
}

/// Ordering of the delete block: by kind first (relation → way → node), then within a relation by **ancestor depth, descending**.
///
/// The server executes `<delete>` in document order: if a child relation is deleted before its parent, the server
/// returns 412 because it is still referenced by the parent (with `if-unused` it becomes "silently not deleted"). So parents must come before children.
/// Equal depth falls back to id, keeping the exported bytes stable.
fn sort_deletes(data: &DataSet, elements: &mut [Element]) {
    let deleted_relations: BTreeSet<ObjectId> = elements
        .iter()
        .filter(|element| element.kind() == ObjectKind::Relation)
        .map(|element| element.id())
        .collect();
    let mut memo: HashMap<ObjectId, u32> = HashMap::new();
    let mut ranks: HashMap<ObjectId, u32> = HashMap::new();
    for id in &deleted_relations {
        let mut visiting: BTreeSet<ObjectId> = BTreeSet::new();
        let depth = relation_depth(data, *id, &deleted_relations, &mut memo, &mut visiting);
        ranks.insert(*id, depth);
    }
    elements.sort_by_key(|element| {
        let rank = if element.kind() == ObjectKind::Relation {
            u32::MAX - ranks.get(&element.id()).copied().unwrap_or(0)
        } else {
            0
        };
        (delete_order(element.kind()), rank, element.id().0)
    });
}

/// Ancestor depth of a relation within the "deleted set": a parent relation's depth is greater than its child's.
///
/// Returns 0 on cycles (doesn't block export; the server reports the error), and `visiting` guarantees termination.
fn relation_depth(
    data: &DataSet,
    id: ObjectId,
    deleted: &BTreeSet<ObjectId>,
    memo: &mut HashMap<ObjectId, u32>,
    visiting: &mut BTreeSet<ObjectId>,
) -> u32 {
    if let Some(depth) = memo.get(&id) {
        return *depth;
    }
    if !visiting.insert(id) {
        return 0;
    }
    let mut depth = 0;
    if let Some(handle) = data.relation_handle(id)
        && let Some(view) = data.relation_view(handle)
    {
        for member in view.members {
            if member.target.kind != ObjectKind::Relation {
                continue;
            }
            let Some(child) = member.target.as_relation().and_then(|h| data.relations.id(h)) else {
                continue;
            };
            if child != id && deleted.contains(&child) {
                depth = depth.max(1 + relation_depth(data, child, deleted, memo, visiting));
            }
        }
    }
    visiting.remove(&id);
    memo.insert(id, depth);
    depth
}

fn element_of(
    data: &DataSet,
    kind: ObjectKind,
    id: neo_core::ObjectId,
) -> Result<Option<Element>, neo_core::NeoError> {
    let element = match kind {
        ObjectKind::Node => {
            let Some(handle) = data.node_handle(id) else {
                return Ok(None);
            };
            let Some(view) = data.node_view(handle) else {
                return Ok(None);
            };
            Element::Node(neo_osm::Node {
                id: view.info.id,
                pos: view.info.pos,
                tags: data.tags_of(view.info.tags),
                meta: view.info.meta,
            })
        }
        ObjectKind::Way => {
            let Some(handle) = data.way_handle(id) else {
                return Ok(None);
            };
            let Some(view) = data.way_view(handle) else {
                return Ok(None);
            };
            Element::Way(neo_osm::Way {
                id: view.info.id,
                nodes: data.try_way_node_ids(handle)?.into(),
                tags: data.tags_of(view.info.tags),
                meta: view.info.meta,
            })
        }
        ObjectKind::Relation => {
            let Some(handle) = data.relation_handle(id) else {
                return Ok(None);
            };
            let Some(view) = data.relation_view(handle) else {
                return Ok(None);
            };
            let mut members = Vec::with_capacity(view.members.len());
            for m in view.members {
                let member = match m.target.kind {
                    ObjectKind::Node => data.nodes.id(m
                        .target
                        .as_node()
                        .ok_or_else(|| neo_core::NeoError::Consistency("bad node member".into()))?),
                    ObjectKind::Way => data.ways.id(m
                        .target
                        .as_way()
                        .ok_or_else(|| neo_core::NeoError::Consistency("bad way member".into()))?),
                    ObjectKind::Relation => data.relations.id(m
                        .target
                        .as_relation()
                        .ok_or_else(|| neo_core::NeoError::Consistency("bad relation member".into()))?),
                    ObjectKind::Changeset => None,
                };
                let Some(member) = member else {
                    return Err(neo_core::NeoError::Consistency(format!(
                        "relation {} has a dangling member",
                        id.0
                    )));
                };
                members.push(neo_osm::RelationMember {
                    kind: m.target.kind,
                    id: member,
                    role: m.role,
                });
            }
            Element::Relation(neo_osm::Relation {
                id: view.info.id,
                members: members.into(),
                tags: data.tags_of(view.info.tags),
                meta: view.info.meta,
            })
        }
        ObjectKind::Changeset => return Ok(None),
    };
    Ok(Some(element))
}

/// Writes a changeset as osmChange 0.6 XML.
pub fn write_changeset<W: Write>(set: &ChangeSet, data: &DataSet, out: &mut W) -> io::Result<()> {
    writeln!(out, "<?xml version=\"1.0\" encoding=\"UTF-8\"?>")?;
    writeln!(out, "<osmChange version=\"0.6\" generator=\"NEO\">")?;
    write_block(&set.create, data, out, "create")?;
    write_block(&set.modify, data, out, "modify")?;
    write_block(&set.delete, data, out, "delete")?;
    writeln!(out, "</osmChange>")?;
    Ok(())
}

fn write_block<W: Write>(elements: &[Element], data: &DataSet, out: &mut W, action: &str) -> io::Result<()> {
    if elements.is_empty() {
        return Ok(());
    }
    if action == "delete" {
        // Ask the server to delete only objects that are still unreferenced: out-of-bbox references or third-party
        // relations should not fail the whole request with 412.
        writeln!(out, "  <delete if-unused=\"true\">")?;
    } else {
        writeln!(out, "  <{action}>")?;
    }
    for element in elements {
        write_element(element, data, out, action)?;
    }
    writeln!(out, "  </{action}>")?;
    Ok(())
}

/// Writes `changeset="…"` on every `<node>` / `<way>` / `<relation>` in the osmChange.
///
/// The OSM API requires every uploaded object to carry the id of an open changeset; local exports may omit this attribute.
pub fn stamp_osmchange_changeset(osc: &str, changeset: i64) -> String {
    let mut out = String::with_capacity(osc.len().saturating_add(64));
    for (index, line) in osc.lines().enumerate() {
        if index > 0 {
            out.push('\n');
        }
        let trimmed = line.trim_start();
        if trimmed.starts_with("<node ") || trimmed.starts_with("<way ") || trimmed.starts_with("<relation ")
        {
            out.push_str(&stamp_open_tag(line, changeset));
        } else {
            out.push_str(line);
        }
    }
    if osc.ends_with('\n') {
        out.push('\n');
    }
    out
}

fn stamp_open_tag(line: &str, changeset: i64) -> String {
    if let Some(start) = line.find("changeset=\"") {
        let value_start = start + "changeset=\"".len();
        if let Some(rel_end) = line[value_start..].find('"') {
            let end = value_start + rel_end;
            let mut stamped = String::with_capacity(line.len() + 16);
            stamped.push_str(&line[..value_start]);
            stamped.push_str(&changeset.to_string());
            stamped.push_str(&line[end..]);
            return stamped;
        }
    }
    match line.rfind('>') {
        Some(pos) => {
            let mut stamped = String::with_capacity(line.len() + 24);
            stamped.push_str(&line[..pos]);
            stamped.push_str(&format!(" changeset=\"{changeset}\""));
            stamped.push_str(&line[pos..]);
            stamped
        }
        None => line.to_owned(),
    }
}

fn write_element<W: Write>(element: &Element, data: &DataSet, out: &mut W, action: &str) -> io::Result<()> {
    let is_create = action == "create";
    let mut meta = String::new();
    let meta_ref = element.meta();
    if !is_create {
        meta.push_str(&format!(" version=\"{}\"", meta_ref.version));
    }
    if meta_ref.timestamp != 0 {
        meta.push_str(&format!(
            " timestamp=\"{}\"",
            crate::time::format_timestamp(meta_ref.timestamp)
        ));
    }
    let user = data.resolve(meta_ref.user);
    if !user.is_empty() {
        meta.push_str(&format!(" user=\"{}\"", escape(user)));
    }
    if meta_ref.uid != 0 {
        meta.push_str(&format!(" uid=\"{}\"", meta_ref.uid));
    }
    match element {
        Element::Node(node) => {
            let (lat, lon) = node.pos.to_degrees();
            writeln!(
                out,
                "    <node id=\"{}\" lat=\"{lat:.7}\" lon=\"{lon:.7}\"{meta}>",
                node.id.0,
            )?;
            write_tags(&node.tags, data, out)?;
            writeln!(out, "    </node>")?;
        }
        Element::Way(way) => {
            writeln!(out, "    <way id=\"{}\"{meta}>", way.id.0)?;
            for node in &way.nodes {
                writeln!(out, "      <nd ref=\"{}\"/>", node.0)?;
            }
            write_tags(&way.tags, data, out)?;
            writeln!(out, "    </way>")?;
        }
        Element::Relation(rel) => {
            writeln!(out, "    <relation id=\"{}\"{meta}>", rel.id.0)?;
            for member in &rel.members {
                writeln!(
                    out,
                    "      <member type=\"{}\" ref=\"{}\" role=\"{}\"/>",
                    member.kind.type_name(),
                    member.id.0,
                    escape(data.resolve(member.role)),
                )?;
            }
            write_tags(&rel.tags, data, out)?;
            writeln!(out, "    </relation>")?;
        }
    }
    Ok(())
}

fn write_tags<W: Write>(tags: &neo_osm::Tags, data: &DataSet, out: &mut W) -> io::Result<()> {
    for (k, v) in tags.iter() {
        writeln!(
            out,
            "      <tag k=\"{}\" v=\"{}\"/>",
            escape(data.resolve(k)),
            escape(data.resolve(v)),
        )?;
    }
    Ok(())
}

fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(ch),
        }
    }
    out
}
