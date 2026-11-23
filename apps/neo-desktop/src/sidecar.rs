//! 旁路进程句柄：同步与影像。

use std::sync::{Arc, Mutex};

use neo_ipc::{ImagerySession, SidecarError, SyncSession};

/// 打开同步旁路；失败时返回错误（编辑器继续跑）。
pub fn open_sync() -> Result<Arc<Mutex<SyncSession>>, SidecarError> {
    SyncSession::open().map(|session| Arc::new(Mutex::new(session)))
}

/// 打开影像旁路；失败时返回错误。
pub fn open_imagery() -> Result<Arc<ImagerySession>, SidecarError> {
    ImagerySession::open().map(Arc::new)
}
