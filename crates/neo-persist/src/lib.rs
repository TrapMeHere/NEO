//! NEO persistence: SQLite WAL workspace, command journal, autosave, crash recovery.
//!
//! 骨架占位：实现随里程碑推进，见 docs/ 与 docs/16-roadmap-and-risks.md。

pub mod session;
pub mod snapshot;
pub use snapshot::{SnapshotMeta, SnapshotStore};

pub use session::{
    SESSION_VERSION, Session, SessionError, SessionLayer, SessionSelection, SessionViewport, read_jos,
    read_jos_file, read_joz, write_jos, write_jos_file, write_joz,
};
