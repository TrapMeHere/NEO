//! # neo-ipc — 编辑器与旁路进程的行协议
//!
//! 每行一条 JSON。`v` 是协议版本；不一致时编辑器停用对应旁路。
//! 影像旁路用 `id` 做并发应答；同步旁路一次一事。

#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod client;
mod imagery;
mod sync;

pub use client::{SidecarError, SidecarProcess, resolve_sidecar};
pub use imagery::{
    ImageryEnvelope, ImageryRequest, ImageryResponse, ImagerySession, PROTOCOL_VERSION as IMAGERY_VERSION,
};
pub use sync::{
    AccountSummary, ConflictDto, ConflictObjectDto, DiffEntryDto, NoteCommentDto, NoteDto, NotesAction,
    PROTOCOL_VERSION as SYNC_VERSION, SyncEnvelope, SyncRequest, SyncResponse, SyncSession,
};

/// 同步与影像共用的协议主版本。
pub const PROTOCOL_VERSION: u32 = 1;
