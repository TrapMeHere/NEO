//! Sidecar process handles: sync and imagery.

use std::sync::{Arc, Mutex};

use neo_ipc::{ImagerySession, SidecarError, SyncSession};

/// Opens the sync sidecar; returns an error on failure (the editor keeps running).
pub fn open_sync() -> Result<Arc<Mutex<SyncSession>>, SidecarError> {
    SyncSession::open().map(|session| Arc::new(Mutex::new(session)))
}

/// Opens the imagery sidecar; returns an error on failure.
pub fn open_imagery() -> Result<Arc<ImagerySession>, SidecarError> {
    ImagerySession::open().map(Arc::new)
}
