//! # neo-ipc — line protocol between the editor and the sidecar processes
//!
//! One JSON object per line. `v` is the protocol version; on mismatch the editor disables the corresponding sidecar.
//! The imagery sidecar uses `id` for concurrent replies; the sync sidecar handles one request at a time.

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

/// Major protocol version shared by sync and imagery.
pub const PROTOCOL_VERSION: u32 = 1;
