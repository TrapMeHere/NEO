//! Conflict model (stage 4)
//!
//! Pure-data three-way merge only: base (local image before upload), local (current local image), remote (server image).
//! It depends neither on `neo-api` (409 translation is done in the app layer) nor on the UI; the result is a plain
//! [`ObjectImage`], which the app layer turns into edit commands.
//!
//! Corresponds one-to-one with `MergeCase` / `ConflictResolution` in docs/06 §2.6.

use std::collections::{BTreeMap, BTreeSet};

use neo_core::{LatLon, ObjectId, ObjectKind};
use neo_geo::WorldPoint;

/// Object snapshot (the minimal information used for merging).
#[derive(Clone, Debug, PartialEq)]
pub struct ObjectImage {
    /// Object kind.
    pub kind: ObjectKind,
    /// Object id.
    pub id: ObjectId,
    /// Version.
    pub version: i32,
    /// Whether it is deleted.
    pub deleted: bool,
    /// Tags.
    pub tags: BTreeMap<String, String>,
    /// Node position (Node only).
    pub node_pos: Option<LatLon>,
    /// Node sequence (Way only).
    pub way_nodes: Vec<ObjectId>,
    /// Members (kind, id, role) (Relation only).
    pub relation_members: Vec<(ObjectKind, ObjectId, String)>,
}

impl ObjectImage {
    /// The new object image.
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

    /// Convenience: set tags.
    pub fn with_tags(mut self, tags: &[(&str, &str)]) -> Self {
        for (k, v) in tags {
            self.tags.insert((*k).to_owned(), (*v).to_owned());
        }
        self
    }

    /// Convenience: node position.
    pub fn with_pos(mut self, pos: LatLon) -> Self {
        self.node_pos = Some(pos);
        self
    }
}

/// Merge case.
#[derive(Clone, Debug, PartialEq)]
pub enum MergeCase {
    /// Version mismatch (the server already has a newer version).
    VersionMismatch {
        /// Local version.
        local: i32,
        /// Server version.
        remote: i32,
    },
    /// Deleted locally, modified remotely.
    LocalDeletedRemoteModified,
    /// Deleted remotely, modified locally.
    RemoteDeletedLocalModified,
    /// Both sides changed the same tag key to different values.
    TagDiverged {
        /// Conflicting key.
        key: String,
    },
    /// Both sides moved the node.
    NodeMoved {
        /// Local move distance (metres).
        local_delta_m: f64,
        /// Remote move distance (metres).
        remote_delta_m: f64,
    },
    /// Both sides changed the way's node sequence.
    WayNodeListDiverged {
        /// Local sequence.
        local: Vec<ObjectId>,
        /// Remote sequence.
        remote: Vec<ObjectId>,
    },
    /// Both sides changed the relation's members.
    RelationMemberDiverged,
    /// The changeset is already closed.
    ChangesetClosed,
}

/// User choice.
#[derive(Clone, Debug, PartialEq)]
pub enum ConflictResolution {
    /// Keep local.
    KeepLocal,
    /// Take remote.
    TakeRemote,
    /// Merge manually (given final tags; geometry stays local).
    MergeManually {
        /// Final tags.
        tags: BTreeMap<String, String>,
    },
    /// Delete the object.
    DeleteObject,
    /// Recreate as a new object (already deleted remotely).
    RecreateObject,
}

/// Tag merge result.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TagMerge {
    /// Merged tags.
    pub merged: BTreeMap<String, String>,
    /// Keys that cannot be decided automatically (need manual work).
    pub conflicts: Vec<String>,
}

/// Three-way tag merge:
///
/// * Only one side changed the key -> take the changed side;
/// * Both sides changed it to the same value -> take that value;
/// * Both sides changed it to different values -> record in `conflicts`, keep local by default.
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
            // Local unchanged, take remote (may be a deletion)
            if let Some(value) = r {
                out.merged.insert(key.clone(), value.clone());
            }
            continue;
        }
        if b == r {
            // Remote unchanged, take local (may be a deletion)
            if let Some(value) = l {
                out.merged.insert(key.clone(), value.clone());
            }
            continue;
        }
        // Both sides changed and differ
        out.conflicts.push(key.clone());
        if let Some(value) = l {
            out.merged.insert(key.clone(), value.clone());
        }
    }
    out
}

/// Three-way classification (for UI display / auto-merge decisions).
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

/// Turn the user choice into the final object image; `KeepLocal`/`MergeManually` keep local geometry.
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
