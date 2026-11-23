//! 选择集。

use std::collections::BTreeSet;

use neo_core::{ObjectId, ObjectKind};

/// 当前选中对象（按种类分组）。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Selection {
    nodes: BTreeSet<ObjectId>,
    ways: BTreeSet<ObjectId>,
    relations: BTreeSet<ObjectId>,
}

impl Selection {
    /// 空选择。
    pub fn new() -> Self {
        Self::default()
    }

    /// 清空。
    pub fn clear(&mut self) {
        self.nodes.clear();
        self.ways.clear();
        self.relations.clear();
    }

    /// 是否为空。
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty() && self.ways.is_empty() && self.relations.is_empty()
    }

    /// 总数。
    pub fn len(&self) -> usize {
        self.nodes.len() + self.ways.len() + self.relations.len()
    }

    /// 是否包含。
    pub fn contains(&self, kind: ObjectKind, id: ObjectId) -> bool {
        match kind {
            ObjectKind::Node => self.nodes.contains(&id),
            ObjectKind::Way => self.ways.contains(&id),
            ObjectKind::Relation => self.relations.contains(&id),
            ObjectKind::Changeset => false,
        }
    }

    /// 加入。
    pub fn insert(&mut self, kind: ObjectKind, id: ObjectId) {
        match kind {
            ObjectKind::Node => {
                self.nodes.insert(id);
            }
            ObjectKind::Way => {
                self.ways.insert(id);
            }
            ObjectKind::Relation => {
                self.relations.insert(id);
            }
            ObjectKind::Changeset => {}
        }
    }

    /// 移除。
    pub fn remove(&mut self, kind: ObjectKind, id: ObjectId) -> bool {
        match kind {
            ObjectKind::Node => self.nodes.remove(&id),
            ObjectKind::Way => self.ways.remove(&id),
            ObjectKind::Relation => self.relations.remove(&id),
            ObjectKind::Changeset => false,
        }
    }

    /// 切换。
    pub fn toggle(&mut self, kind: ObjectKind, id: ObjectId) {
        if !self.remove(kind, id) {
            self.insert(kind, id);
        }
    }

    /// 只选中一个对象。
    pub fn select_only(&mut self, kind: ObjectKind, id: ObjectId) {
        self.clear();
        self.insert(kind, id);
    }

    /// 迭代某类。
    pub fn iter(&self, kind: ObjectKind) -> impl Iterator<Item = ObjectId> + '_ {
        let (a, b) = match kind {
            ObjectKind::Node => (&self.nodes, &self.ways),
            ObjectKind::Way => (&self.ways, &self.relations),
            ObjectKind::Relation => (&self.relations, &self.nodes),
            ObjectKind::Changeset => (&self.nodes, &self.nodes),
        };
        let _ = b; // 保持 match 穷尽
        match kind {
            ObjectKind::Node => Box::new(a.iter().copied()) as Box<dyn Iterator<Item = ObjectId>>,
            ObjectKind::Way => Box::new(a.iter().copied()),
            ObjectKind::Relation => Box::new(a.iter().copied()),
            ObjectKind::Changeset => Box::new(std::iter::empty()),
        }
    }

    /// 节点集合。
    pub fn nodes(&self) -> &BTreeSet<ObjectId> {
        &self.nodes
    }

    /// Way 集合。
    pub fn ways(&self) -> &BTreeSet<ObjectId> {
        &self.ways
    }

    /// Relation 集合。
    pub fn relations(&self) -> &BTreeSet<ObjectId> {
        &self.relations
    }
}
