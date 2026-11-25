//! NEO plugin host.
//!
//! The default build only contains the manifest, permission, package checks, and command translation; it does not link wasmtime.
//! With `wasm-plugins` enabled, plugins run on wasmtime 50: fuel, memory limits, capability intersection.
//! Plugins can only hand commands back to [`apply_command`]; they cannot write to `neo-store`.

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

/// Plugin host error.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum PluginError {
    /// Invalid manifest.
    #[error("manifest: {0}")]
    Manifest(String),
    /// ABI major version mismatch.
    #[error("incompatible api: plugin={plugin}, host={host}")]
    ApiIncompatible {
        /// API declared by the plugin.
        plugin: String,
        /// Host API.
        host: String,
    },
    /// Signature missing or mismatched.
    #[error("signature invalid")]
    SignatureInvalid,
    /// Capability denied.
    #[error("capability denied: {0}")]
    CapabilityDenied(String),
    /// Plugin trap.
    #[error("plugin trapped: {0}")]
    Trapped(String),
    /// Quota.
    #[error("quota exceeded: {0}")]
    QuotaExceeded(String),
    /// Command execution failed.
    #[error("command failed: {0}")]
    Command(String),
    /// `wasm-plugins` is not enabled.
    #[error("wasm plugins are disabled in this build")]
    WasmDisabled,
}

/// Summary of the read-only snapshot. Used by the plugin query surface; `DataSet` is never handed into WASM.
pub fn snapshot_facts(query: &impl MapQuery) -> SnapshotFacts {
    SnapshotFacts {
        version: query.data().version(),
        nodes: query.data().node_count() as u64,
        ways: query.data().way_count() as u64,
        relations: query.data().relation_count() as u64,
    }
}

/// Result of [`snapshot_facts`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SnapshotFacts {
    /// `DataSet` version.
    pub version: u64,
    /// Node count.
    pub nodes: u64,
    /// Way count.
    pub ways: u64,
    /// Relation count.
    pub relations: u64,
}

/// Whether the current build can run WASM.
pub fn wasm_plugins_enabled() -> bool {
    cfg!(feature = "wasm-plugins")
}
