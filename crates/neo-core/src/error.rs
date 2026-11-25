//! Core error types.
//!
//! Rule: library crates use concrete error types (`thiserror`); `apps/*` aggregate context with `anyhow`.

use crate::ids::{ObjectId, ObjectKind};

/// Error type shared across NEO layers.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum NeoError {
    /// Coordinate out of range (latitude > 90° or longitude > 180°).
    #[error("invalid coordinate: {0}")]
    InvalidCoordinate(String),

    /// Dangling reference: an object references a primitive that does not exist.
    #[error("dangling reference: {kind} {id} referenced by {referrer_kind} {referrer}")]
    DanglingReference {
        /// Kind of the referenced object.
        kind: ObjectKind,
        /// ID of the referenced object.
        id: ObjectId,
        /// Kind of the referrer.
        referrer_kind: ObjectKind,
        /// ID of the referrer.
        referrer: ObjectId,
    },

    /// Stale handle (the object was deleted or the slot was reused).
    #[error("stale handle for {kind} (index {index})")]
    StaleHandle {
        /// Object kind.
        kind: ObjectKind,
        /// Slot index.
        index: u32,
    },

    /// Data format/parse error (PBF, XML, OSC, MapCSS, …).
    #[error("parse error: {0}")]
    Parse(String),

    /// Data consistency error (index and storage disagree, round-trip failure, etc.).
    #[error("consistency error: {0}")]
    Consistency(String),

    /// Operation cancelled by the caller (import/export/task).
    #[error("operation cancelled")]
    Cancelled,

    /// IO error.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

/// `Result` alias for NEO library layers.
pub type Result<T> = core::result::Result<T, NeoError>;
