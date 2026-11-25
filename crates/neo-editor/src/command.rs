//! Command contract.

use std::any::Any;

use neo_core::{ObjectId, ObjectKind};
use neo_store::{DataSet, Referrers};

use crate::editor::EditContext;
use crate::journal::JournalCommand;

/// Edit error.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum CommandError {
    /// The object does not exist.
    #[error("{kind} {id} not found")]
    NotFound {
        /// Object kind.
        kind: ObjectKind,
        /// Object ID.
        id: ObjectId,
    },
    /// The object is still referenced and cannot be deleted.
    #[error("{kind} {id} is still referenced by {what}")]
    Referenced {
        /// Object kind.
        kind: ObjectKind,
        /// Object ID.
        id: ObjectId,
        /// Description of the referrers (way count / relations, etc.).
        what: String,
    },
    /// Invalid argument.
    #[error("invalid edit: {0}")]
    Invalid(String),
    /// Storage error.
    #[error("store error: {0}")]
    Store(String),
}

impl From<neo_core::NeoError> for CommandError {
    fn from(value: neo_core::NeoError) -> Self {
        Self::Store(value.to_string())
    }
}

/// Command kind.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub enum CommandKind {
    /// Add a node.
    AddNode,
    /// Add a way.
    AddWay,
    /// Add a relation.
    AddRelation,
    /// Move a node.
    MoveNode,
    /// Modify tags.
    SetTags,
    /// Modify the way's node sequence.
    SetWayNodes,
    /// Modify a relation's members.
    SetRelationMembers,
    /// Delete an object.
    Delete,
    /// Split a way.
    SplitWay,
    /// Merge ways.
    MergeWay,
    /// Reverse a way.
    ReverseWay,
}

impl CommandKind {
    /// Whether it can be merged with commands of the same kind (drag merging).
    pub fn mergeable(self) -> bool {
        matches!(self, Self::MoveNode | Self::SetTags)
    }
}

/// An undoable edit command.
pub trait Command: Send {
    /// Kind.
    fn kind(&self) -> CommandKind;
    /// Label.
    fn label(&self) -> &'static str;
    /// Precheck (no side effects).
    fn precheck(&self, _data: &DataSet, _referrers: &Referrers) -> Result<(), CommandError> {
        Ok(())
    }
    /// Apply forward.
    fn apply(&mut self, ctx: &mut EditContext<'_>) -> Result<(), CommandError>;
    /// Inverse operation.
    fn undo(&mut self, ctx: &mut EditContext<'_>) -> Result<(), CommandError>;
    /// Merge an updating command.
    fn merge(&mut self, _newer: &dyn Command) -> bool {
        false
    }
    /// Journal record.
    fn record(&self) -> JournalCommand;
    /// Downcast (for merging).
    fn as_any(&self) -> &dyn Any;
}
