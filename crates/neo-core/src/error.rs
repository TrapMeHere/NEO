//! 核心错误类型。
//!
//! 规则：库 crate 用具体错误类型（`thiserror`），`apps/*` 用 `anyhow` 汇总上下文。

use crate::ids::{ObjectId, ObjectKind};

/// NEO 各层共用的错误类型。
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum NeoError {
    /// 坐标越界（纬度 > 90° 或经度 > 180°）。
    #[error("invalid coordinate: {0}")]
    InvalidCoordinate(String),

    /// 悬空引用：对象引用了不存在的图元。
    #[error("dangling reference: {kind} {id} referenced by {referrer_kind} {referrer}")]
    DanglingReference {
        /// 被引用对象的种类。
        kind: ObjectKind,
        /// 被引用对象的 ID。
        id: ObjectId,
        /// 引用者种类。
        referrer_kind: ObjectKind,
        /// 引用者 ID。
        referrer: ObjectId,
    },

    /// 句柄失效（对象已被删除或槽位已复用）。
    #[error("stale handle for {kind} (index {index})")]
    StaleHandle {
        /// 对象种类。
        kind: ObjectKind,
        /// 槽位下标。
        index: u32,
    },

    /// 数据格式/解析错误（PBF、XML、OSC、MapCSS…）。
    #[error("parse error: {0}")]
    Parse(String),

    /// 数据一致性错误（索引与存储不一致、往返失败等）。
    #[error("consistency error: {0}")]
    Consistency(String),

    /// 操作被调用方取消（导入/导出/任务）。
    #[error("operation cancelled")]
    Cancelled,

    /// IO 错误。
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

/// NEO 库层 `Result` 别名。
pub type Result<T> = core::result::Result<T, NeoError>;
