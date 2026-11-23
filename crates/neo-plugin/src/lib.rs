//! NEO 插件宿主。
//!
//! 默认构建只含清单、权限、包检查和命令翻译，不链接 wasmtime。
//! `wasm-plugins` 打开后用 wasmtime 50 跑插件：fuel、内存上限、能力求交。
//! 插件只能把命令交回 [`apply_command`]，不能写 `neo-store`。

#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod apply;
mod capability;
mod gate;
#[cfg(feature = "wasm-plugins")]
mod guests;
mod manifest;
mod package;
#[cfg(feature = "wasm-plugins")]
mod wasm_host;

pub use apply::{apply_command, object_kind_name, way_tags};
pub use capability::{Capability, CapabilitySet};
pub use gate::{HostError, HostGate, PanelDescriptor, PendingCommand};
pub use manifest::{ManifestCaps, PluginManifest, api_compatible, parse_manifest};
pub use package::{OpenedPackage, content_signature, open_package, pack_neopkg};
#[cfg(feature = "wasm-plugins")]
pub use wasm_host::{DispatchReport, LoadedPluginInfo, PluginHost, PluginHostConfig, run_builtin_demo};

use neo_query::MapQuery;

/// 插件宿主错误。
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum PluginError {
    /// 清单不合法。
    #[error("manifest: {0}")]
    Manifest(String),
    /// ABI 主版本不一致。
    #[error("incompatible api: plugin={plugin}, host={host}")]
    ApiIncompatible {
        /// 插件声明的 API。
        plugin: String,
        /// 宿主 API。
        host: String,
    },
    /// 签名缺失或对不上。
    #[error("signature invalid")]
    SignatureInvalid,
    /// 能力被拒绝。
    #[error("capability denied: {0}")]
    CapabilityDenied(String),
    /// 插件 trap。
    #[error("plugin trapped: {0}")]
    Trapped(String),
    /// 配额。
    #[error("quota exceeded: {0}")]
    QuotaExceeded(String),
    /// 命令执行失败。
    #[error("command failed: {0}")]
    Command(String),
    /// 没开 `wasm-plugins`。
    #[error("wasm plugins are disabled in this build")]
    WasmDisabled,
}

/// 只读快照的摘要。插件查询面用它，不把 `DataSet` 交进 WASM。
pub fn snapshot_facts(query: &impl MapQuery) -> SnapshotFacts {
    SnapshotFacts {
        version: query.data().version(),
        nodes: query.data().node_count() as u64,
        ways: query.data().way_count() as u64,
        relations: query.data().relation_count() as u64,
    }
}

/// [`snapshot_facts`] 的结果。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SnapshotFacts {
    /// `DataSet` 版本。
    pub version: u64,
    /// 节点数。
    pub nodes: u64,
    /// way 数。
    pub ways: u64,
    /// relation 数。
    pub relations: u64,
}

/// 当前构建能不能跑 WASM。
pub fn wasm_plugins_enabled() -> bool {
    cfg!(feature = "wasm-plugins")
}
