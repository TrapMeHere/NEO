//! Reference integrity checking (G2 acceptance 2.5).
//!
//! OSM's core invariants:
//! * every node reference of every Way must exist (unless explicitly marked incomplete);
//! * every Relation member must exist.
//!
//! This module scans the in-memory `DataSet` in O(n+e) and returns a structured problem list; it can be called
//! before import/upload. Note: bbox-extracted PBFs can naturally contain incomplete relations; on import use
//! `MissingPolicy::Skip` to drop them or keep them as incomplete —— this check is for "is the full data set self-consistent".

use neo_core::{ObjectId, ObjectKind};

use crate::dataset::DataSet;

/// One integrity problem.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IntegrityIssue {
    /// Problem category (`way-node` / `relation-member`).
    pub category: &'static str,
    /// Referrer object.
    pub referrer: ObjectId,
    /// Referrer kind.
    pub referrer_kind: ObjectKind,
    /// Kind of the missing object.
    pub missing_kind: ObjectKind,
    /// ID of the missing object (`ObjectId::NONE` if the handle cannot be resolved).
    pub missing: ObjectId,
}

/// Integrity report.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct IntegrityReport {
    /// Number of nodes scanned.
    pub nodes: usize,
    /// Number of Ways scanned.
    pub ways: usize,
    /// Number of Relations scanned.
    pub relations: usize,
    /// Problem list.
    pub issues: Vec<IntegrityIssue>,
}

impl IntegrityReport {
    /// Whether the data is fully self-consistent.
    pub fn is_ok(&self) -> bool {
        self.issues.is_empty()
    }

    /// Number of problems.
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

/// Full reference-integrity check.
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
