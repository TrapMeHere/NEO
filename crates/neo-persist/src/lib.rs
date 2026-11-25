//! NEO persistence: SQLite WAL workspace, command journal, autosave, crash recovery.
//!
//! Skeleton placeholder: implementation lands with the milestones; see docs/ and docs/16-roadmap-and-risks.md.

pub mod session;
pub mod snapshot;
pub use snapshot::{SnapshotMeta, SnapshotStore};

pub use session::{
    SESSION_VERSION, Session, SessionError, SessionLayer, SessionSelection, SessionViewport, read_jos,
    read_jos_file, read_joz, write_jos, write_jos_file, write_joz,
};
