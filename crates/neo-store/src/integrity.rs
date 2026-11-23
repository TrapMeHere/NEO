//! 引用完整性检查（G2 验收 2.5）。
//!
//! OSM 的核心不变量：
//! * 每条 Way 的每个节点引用都必须存在（除非显式标记 incomplete）；
//! * 每个 Relation 成员都必须存在。
//!
//! 本模块对内存 `DataSet` 做 O(n+e) 扫描并返回结构化问题列表；导入/上传前可调用。
//! 注意：bbox 提取的 PBF 天然可能有不完整 relation，导入时用 `MissingPolicy::Skip`
//! 丢弃或保留为 incomplete —— 本检查用于"全量数据是否自洽"。

use neo_core::{ObjectId, ObjectKind};

use crate::dataset::DataSet;

/// 一条完整性问题。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IntegrityIssue {
    /// 问题类别（`way-node` / `relation-member`）。
    pub category: &'static str,
    /// 引用者对象。
    pub referrer: ObjectId,
    /// 引用者种类。
    pub referrer_kind: ObjectKind,
    /// 缺失对象种类。
    pub missing_kind: ObjectKind,
    /// 缺失对象 ID（若无法解析句柄时为 `ObjectId::NONE`）。
    pub missing: ObjectId,
}

/// 完整性报告。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct IntegrityReport {
    /// 扫描的节点数。
    pub nodes: usize,
    /// 扫描的 Way 数。
    pub ways: usize,
    /// 扫描的 Relation 数。
    pub relations: usize,
    /// 问题列表。
    pub issues: Vec<IntegrityIssue>,
}

impl IntegrityReport {
    /// 是否完全自洽。
    pub fn is_ok(&self) -> bool {
        self.issues.is_empty()
    }

    /// 问题数。
    pub fn issue_count(&self) -> usize {
        self.issues.len()
    }
}

impl core::fmt::Display for IntegrityReport {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "integrity: {} nodes, {} ways, {} relations, {} issues",
            self.nodes,
            self.ways,
            self.relations,
            self.issues.len()
        )
    }
}

/// 全量引用完整性检查。
pub fn check(data: &DataSet) -> IntegrityReport {
    let mut report = IntegrityReport {
        nodes: data.node_count(),
        ways: data.way_count(),
        relations: data.relation_count(),
        issues: Vec::new(),
    };

    for way in data.iter_ways() {
        let Some(refs) = data.ways.nodes(way.handle) else {
            continue;
        };
        for node in refs {
            if !data.nodes.contains(*node) {
                report.issues.push(IntegrityIssue {
                    category: "way-node",
                    referrer: way.id,
                    referrer_kind: ObjectKind::Way,
                    missing_kind: ObjectKind::Node,
                    missing: data.nodes.id(*node).unwrap_or(ObjectId::NONE),
                });
            }
        }
    }

    for relation in data.iter_relations() {
        let Some(members) = data.relations.members_of(relation.handle) else {
            continue;
        };
        for member in members {
            let valid = match member.target.kind {
                ObjectKind::Node => member
                    .target
                    .as_node()
                    .map(|h| data.nodes.contains(h))
                    .unwrap_or(false),
                ObjectKind::Way => member
                    .target
                    .as_way()
                    .map(|h| data.ways.contains(h))
                    .unwrap_or(false),
                ObjectKind::Relation => member
                    .target
                    .as_relation()
                    .map(|h| data.relations.contains(h))
                    .unwrap_or(false),
                ObjectKind::Changeset => true,
            };
            if !valid {
                report.issues.push(IntegrityIssue {
                    category: "relation-member",
                    referrer: relation.id,
                    referrer_kind: ObjectKind::Relation,
                    missing_kind: member.target.kind,
                    missing: ObjectId::NONE,
                });
            }
        }
    }

    report
}
