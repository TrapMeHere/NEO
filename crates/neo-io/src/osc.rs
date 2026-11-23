//! osmChange 0.6 输出（上传 diff 格式）。
//!
//! 三个必须记住的规范事实（docs/09）：
//!
//! 1. 文件根元素是 `<osmChange version="0.6">`，**不存在 "OSC 0.3"**；
//! 2. 上传是单个 `application/xml` 请求体（不是 multipart；multipart 只用于 GPX）；
//! 3. 本地新对象使用**负数 ID 占位**（`create` 块），服务端返回 `<diffResult>` 映射到正 ID。
//!
//! 这里手写 XML 序列化而不是依赖 quick-xml 的 writer：输出量小、可控性强，且便于
//! 逐字节 golden test；读取侧（XML 导入）用到 quick-xml。

use std::io::{self, Write};

use neo_core::{ObjectId, ObjectKind};
use neo_osm::{ChangeState, Element};
use neo_store::DataSet;

/// 一次上传所需的变更（按 osmChange 动作分组）。
#[derive(Clone, Default)]
pub struct ChangeSet {
    /// 新建对象。
    pub create: Vec<Element>,
    /// 修改对象。
    pub modify: Vec<Element>,
    /// 删除对象。
    pub delete: Vec<Element>,
}

impl ChangeSet {
    /// 是否为空。
    pub fn is_empty(&self) -> bool {
        self.create.is_empty() && self.modify.is_empty() && self.delete.is_empty()
    }

    /// 元素总数。
    pub fn len(&self) -> usize {
        self.create.len() + self.modify.len() + self.delete.len()
    }

    /// 按依赖关系切分成每块不超过 `max` 个元素的批次。
    ///
    /// 新建对象按“way/relation 引用新建节点/成员”做并查集，保证一个引用组不会被
    /// 拆到两个 diff 里；修改/删除按确定性顺序切块。删除顺序已经在
    /// [`collect_changeset`] 里按 relation → way → node 排好，切块保持该顺序。
    pub fn chunks(&self, max: usize) -> Result<Vec<ChangeSet>, neo_core::NeoError> {
        use std::collections::HashMap;
        if max == 0 {
            return Err(neo_core::NeoError::Parse("chunk size must be > 0".into()));
        }
        if self.len() <= max {
            return Ok(vec![self.clone()]);
        }
        // 修改对象可能引用同一批新建对象（负 ID）。跨 diff 时服务端已把负 ID
        // 映射成正 ID，但后续 diff 仍写负 ID 会失败；没有逐块重写引用前，
        // 混合 create+modify 的大变更拒绝切块，避免上传出半坏数据。
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

/// 从数据集与变更跟踪生成 osmChange 的 [`ChangeSet`]。
///
/// 语义要点：
/// * `Added` → create（本地负 ID 原样输出，服务端负责映射）；
/// * `Modified` → modify（必须带服务端 version）；
/// * `Deleted` → delete（同样带 version）；
/// * `Unchanged` 不输出。
///
/// 收集**选中对象**的变更集；新建对象的依赖（新节点/新成员）自动带上。
///
/// 选中集合按 `(kind, id)` 过滤；未选中的修改不会上传，适合“小步提交”。
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
    set.delete
        .sort_by_key(|element| (delete_order(element.kind()), element.id().0));
    Ok(set)
}

/// 收集整个数据集的变更（create/modify/delete 分组、确定性排序）。
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
                // 本地新建后又删除的对象从未在服务端存在过：上传 `<delete>` 负 ID
                // 会让整个 changeset 失败，必须从 diff 里剔除。
                ChangeState::Deleted if id.is_local() => {}
                ChangeState::Deleted => set.delete.push(element),
                ChangeState::Unchanged => {}
            }
        }
    }
    // 确定性输出：同一编辑集每次导出的字节一致（golden test/审阅/缓存）。
    set.create
        .sort_by_key(|element| (kind_order(element.kind()), element.id().0));
    set.modify
        .sort_by_key(|element| (kind_order(element.kind()), element.id().0));
    // 删除必须按 relation → way → node 的依赖序；服务端按文档顺序执行，
    // 先删 node 会因仍被 way 引用而 412 整包失败。
    set.delete
        .sort_by_key(|element| (delete_order(element.kind()), element.id().0));
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

/// 把变更集写成 osmChange 0.6 XML。
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
        // 让服务端只删除"仍未被引用"的对象：框外引用/他人 relation 不应让整包 412。
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

/// 给 osmChange 里每个 `<node>` / `<way>` / `<relation>` 写上 `changeset="…"`。
///
/// OSM API 要求上传的每个对象都带打开中的 changeset id；本地导出可以没有这个属性。
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
