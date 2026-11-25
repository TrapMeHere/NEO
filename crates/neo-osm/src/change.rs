//! Editing state and upload action semantics.
//!
//! The two concepts must be kept separate; conflating them is the most common
//! semantic bug when restructuring a JOSM-style editor:
//!
//! * [`ChangeState`] is the **local data state** (relative to the last download/upload baseline);
//! * [`ChangeAction`] is the **action uploaded to the OSM API** (matching osmChange's create/modify/delete).
//!
//! An `Added` object is uploaded as `create`; a `Deleted` object that was never uploaded
//! must not appear in the diff at all (created locally and then deleted).

use neo_core::{ObjectId, ObjectKind};

/// Local data state.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum ChangeState {
    /// Matches the baseline.
    #[default]
    Unchanged,
    /// Newly created locally; no server version yet.
    Added,
    /// Modified on top of the server version.
    Modified,
    /// Deleted locally (still exists on the server; uploaded as delete).
    Deleted,
}

impl ChangeState {
    /// Whether this must be submitted on the next upload.
    #[inline]
    pub const fn is_dirty(self) -> bool {
        !matches!(self, Self::Unchanged)
    }

    /// Whether a server object exists (upload needs a version to modify/delete).
    #[inline]
    pub const fn has_server_object(self) -> bool {
        matches!(self, Self::Modified | Self::Deleted)
    }
}

/// osmChange action.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ChangeAction {
    /// `<create>`。
    Create,
    /// `<modify>`。
    Modify,
    /// `<delete>`。
    Delete,
}

impl ChangeAction {
    /// Derives the action from the local state and whether a server version exists; returns `None` when no upload is needed.
    pub fn from_state(state: ChangeState) -> Option<Self> {
        match state {
            ChangeState::Unchanged => None,
            ChangeState::Added => Some(Self::Create),
            ChangeState::Modified => Some(Self::Modify),
            ChangeState::Deleted => Some(Self::Delete),
        }
    }

    /// osmChange element name.
    #[inline]
    pub const fn xml_name(self) -> &'static str {
        match self {
            Self::Create => "create",
            Self::Modify => "modify",
            Self::Delete => "delete",
        }
    }
}

/// Identity of one change (used for journal / diff / conflict lookup).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ChangeRef {
    /// Object kind.
    pub kind: ObjectKind,
    /// Object ID.
    pub id: ObjectId,
    /// Action.
    pub action: ChangeAction,
}
