//! 冲突模型（阶段 4）
//!
//! 只做**纯数据**的三方合并：base（上传前本地像）、local（当前本地像）、remote（服务端像）。
//! 不依赖 `neo-api`（409 的翻译在应用层完成），也不依赖 UI；结果是一个普通的
//! [`ObjectImage`]，由应用层转成编辑命令。
//!
//! 与 docs/06 §2.6 的 `MergeCase` / `ConflictResolution` 一一对应。

use std::collections::{BTreeMap, BTreeSet};

use neo_core::{LatLon, ObjectId, ObjectKind};
use neo_geo::WorldPoint;

/// 对象快照（用于合并的最小信息）。
#[derive(Clone, Debug, PartialEq)]
pub struct ObjectImage {
    /// 对象种类。
    pub kind: ObjectKind,
    /// 对象 id。
    pub id: ObjectId,
    /// 版本。
    pub version: i32,
    /// 是否已删除。
    pub deleted: bool,
    /// 标签。
    pub tags: BTreeMap<String, String>,
    /// 节点坐标（仅 Node）。
    pub node_pos: Option<LatLon>,
    /// 节点序列（仅 Way）。
    pub way_nodes: Vec<ObjectId>,
    /// 成员 (kind, id, role)（仅 Relation）。
    pub relation_members: Vec<(ObjectKind, ObjectId, String)>,
}

impl ObjectImage {
    /// 新对象像。
    pub fn new(kind: ObjectKind, id: ObjectId, version: i32) -> Self {
        Self {
            kind,
            id,
            version,
            deleted: false,
            tags: BTreeMap::new(),
            node_pos: None,
            way_nodes: Vec::new(),
            relation_members: Vec::new(),
        }
    }

    /// 便捷：设置标签。
    pub fn with_tags(mut self, tags: &[(&str, &str)]) -> Self {
        for (k, v) in tags {
            self.tags.insert((*k).to_owned(), (*v).to_owned());
        }
        self
    }

    /// 便捷：节点坐标。
    pub fn with_pos(mut self, pos: LatLon) -> Self {
        self.node_pos = Some(pos);
        self
    }
}

/// 合并情形。
#[derive(Clone, Debug, PartialEq)]
pub enum MergeCase {
    /// 版本不一致（服务端已有更新）。
    VersionMismatch {
        /// 本地版本。
        local: i32,
        /// 服务端版本。
        remote: i32,
    },
    /// 本地删除、远端修改。
    LocalDeletedRemoteModified,
    /// 远端删除、本地修改。
    RemoteDeletedLocalModified,
    /// 同一 tag key 双方改成不同值。
    TagDiverged {
        /// 冲突 key。
        key: String,
    },
    /// 节点双方都移动了。
    NodeMoved {
        /// 本地移动距离（米）。
        local_delta_m: f64,
        /// 远端移动距离（米）。
        remote_delta_m: f64,
    },
    /// way 节点序列双方都改了。
    WayNodeListDiverged {
        /// 本地序列。
        local: Vec<ObjectId>,
        /// 远端序列。
        remote: Vec<ObjectId>,
    },
    /// relation 成员双方都改了。
    RelationMemberDiverged,
    /// changeset 已关闭。
    ChangesetClosed,
}

/// 用户选择。
#[derive(Clone, Debug, PartialEq)]
pub enum ConflictResolution {
    /// 保留本地。
    KeepLocal,
    /// 采用远端。
    TakeRemote,
    /// 手动合并（给定最终标签；几何取本地）。
    MergeManually {
        /// 最终标签。
        tags: BTreeMap<String, String>,
    },
    /// 删除对象。
    DeleteObject,
    /// 重建为新对象（远端已删）。
    RecreateObject,
}

/// 标签合并结果。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TagMerge {
    /// 合并后的标签。
    pub merged: BTreeMap<String, String>,
    /// 无法自动决定的 key（需要人工）。
    pub conflicts: Vec<String>,
}

/// 三方合并标签：
///
/// * 只有一方改动该 key → 取改动方；
/// * 双方改成相同值 → 取该值；
/// * 双方改成不同值 → 记入 `conflicts`，默认保留本地。
pub fn merge_tags(
    base: &BTreeMap<String, String>,
    local: &BTreeMap<String, String>,
    remote: &BTreeMap<String, String>,
) -> TagMerge {
    let keys: BTreeSet<&String> = base.keys().chain(local.keys()).chain(remote.keys()).collect();
    let mut out = TagMerge::default();
    for key in keys {
        let b = base.get(key);
        let l = local.get(key);
        let r = remote.get(key);
        if l == r {
            if let Some(value) = l {
                out.merged.insert(key.clone(), value.clone());
            }
            continue;
        }
        if b == l {
            // 本地未改，取远端（可能是删除）
            if let Some(value) = r {
                out.merged.insert(key.clone(), value.clone());
            }
            continue;
        }
        if b == r {
            // 远端未改，取本地（可能是删除）
            if let Some(value) = l {
                out.merged.insert(key.clone(), value.clone());
            }
            continue;
        }
        // 双方都改了且不同
        out.conflicts.push(key.clone());
        if let Some(value) = l {
            out.merged.insert(key.clone(), value.clone());
        }
    }
    out
}

/// 三方分类（用于 UI 展示/自动合并判断）。
pub fn classify(base: &ObjectImage, local: &ObjectImage, remote: &ObjectImage) -> MergeCase {
    if local.deleted && !remote.deleted {
        return MergeCase::LocalDeletedRemoteModified;
    }
    if !local.deleted && remote.deleted {
        return MergeCase::RemoteDeletedLocalModified;
    }
    if let (Some(a), Some(b)) = (local.node_pos, remote.node_pos)
        && let Some(base_pos) = base.node_pos
    {
        let local_delta = WorldPoint::from_latlon(a).dist(WorldPoint::from_latlon(base_pos));
        let remote_delta = WorldPoint::from_latlon(b).dist(WorldPoint::from_latlon(base_pos));
        if (local_delta > 0.01 || remote_delta > 0.01) && local_delta > 0.01 && remote_delta > 0.01 {
            return MergeCase::NodeMoved {
                local_delta_m: local_delta,
                remote_delta_m: remote_delta,
            };
        }
    }
    if local.kind == ObjectKind::Way && local.way_nodes != remote.way_nodes {
        return MergeCase::WayNodeListDiverged {
            local: local.way_nodes.clone(),
            remote: remote.way_nodes.clone(),
        };
    }
    if local.kind == ObjectKind::Relation && local.relation_members != remote.relation_members {
        return MergeCase::RelationMemberDiverged;
    }
    if let Some(conflict) = merge_tags(&base.tags, &local.tags, &remote.tags)
        .conflicts
        .into_iter()
        .next()
    {
        return MergeCase::TagDiverged { key: conflict };
    }
    if local.version != remote.version {
        return MergeCase::VersionMismatch {
            local: local.version,
            remote: remote.version,
        };
    }
    MergeCase::VersionMismatch {
        local: local.version,
        remote: remote.version,
    }
}

/// 把用户选择落成最终对象像；`KeepLocal`/`MergeManually` 保留本地几何。
pub fn resolve(
    case: &MergeCase,
    local: &ObjectImage,
    remote: &ObjectImage,
    resolution: &ConflictResolution,
) -> Option<ObjectImage> {
    match resolution {
        ConflictResolution::KeepLocal => Some(local.clone()),
        ConflictResolution::TakeRemote => Some(remote.clone()),
        ConflictResolution::MergeManually { tags } => {
            let mut image = local.clone();
            image.tags = tags.clone();
            Some(image)
        }
        ConflictResolution::DeleteObject => {
            let mut image = local.clone();
            image.deleted = true;
            Some(image)
        }
        ConflictResolution::RecreateObject => {
            if !matches!(case, MergeCase::RemoteDeletedLocalModified) {
                return None;
            }
            let mut image = local.clone();
            image.id = ObjectId(-local.id.0.abs().max(1));
            image.version = 0;
            Some(image)
        }
    }
}
