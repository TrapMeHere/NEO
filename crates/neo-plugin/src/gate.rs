//! Host call gate. WASM imports and direct calls go through the same permission checks.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use crate::PluginError;
use crate::capability::CapabilitySet;

/// A command a plugin handed back that has not yet entered the editor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingCommand {
    /// Originating plugin id.
    pub plugin: String,
    /// Command name, e.g. `add-node`.
    pub name: String,
    /// JSON arguments.
    pub args_json: String,
}

/// Panel description.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PanelDescriptor {
    /// Panel number assigned to the plugin (starting at 1).
    pub id: u32,
    /// Panel id inside the plugin.
    pub panel_id: String,
    /// Title.
    pub title: String,
    /// Most recent status text.
    pub status: String,
}

/// Permission or argument error. Plugins see a status code; the host keeps this enum.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[non_exhaustive]
pub enum HostError {
    /// Unauthorized.
    #[error("permission denied: {0}")]
    PermissionDenied(String),
    /// Object or file not found.
    #[error("not found: {0}")]
    NotFound(String),
    /// Invalid argument.
    #[error("invalid argument: {0}")]
    Invalid(String),
    /// Quota exceeded.
    #[error("quota exceeded: {0}")]
    QuotaExceeded(String),
}

/// Accounting for one host call.
#[derive(Debug)]
pub struct HostGate {
    /// Plugin id.
    pub plugin_id: String,
    /// Capabilities after intersection.
    pub caps: CapabilitySet,
    /// Queued commands.
    pub commands: Vec<PendingCommand>,
    /// Panels.
    pub panels: Vec<PanelDescriptor>,
    /// Remaining command quota for this event.
    pub command_budget: u32,
    /// Snapshot version (read-only).
    pub snapshot_version: u64,
    /// Root directory allowed for reading.
    pub file_root: Option<PathBuf>,
    /// Bytes from the most recent successful read.
    pub last_file: Vec<u8>,
    /// URLs actually allowed. Denied ones are not recorded.
    pub http_log: Vec<String>,
    /// Number of allowed HTTP requests, for cross-plugin assertions.
    pub http_hits: Arc<AtomicU32>,
}

impl HostGate {
    /// New. `http_hits` only increases when a URL is allowed.
    pub fn new(plugin_id: impl Into<String>, caps: CapabilitySet, http_hits: Arc<AtomicU32>) -> Self {
        Self {
            plugin_id: plugin_id.into(),
            caps,
            commands: Vec::new(),
            panels: Vec::new(),
            command_budget: 20,
            snapshot_version: 0,
            file_root: None,
            last_file: Vec::new(),
            http_log: Vec::new(),
            http_hits,
        }
    }

    /// Read-only snapshot version. Always allowed.
    pub fn snapshot_version(&self) -> u64 {
        self.snapshot_version
    }

    /// Queues a command. Unauthorized or over-quota commands never write to `commands`.
    pub fn submit_command(&mut self, name: &str, args_json: &str) -> Result<u64, HostError> {
        if !self.caps.command {
            return Err(HostError::PermissionDenied("command".into()));
        }
        if name.is_empty() || name.len() > 64 {
            return Err(HostError::Invalid("command name".into()));
        }
        if self.command_budget == 0 {
            return Err(HostError::QuotaExceeded("commands per event".into()));
        }
        self.command_budget -= 1;
        self.commands.push(PendingCommand {
            plugin: self.plugin_id.clone(),
            name: name.to_owned(),
            args_json: args_json.to_owned(),
        });
        Ok(self.commands.len() as u64)
    }

    /// Registers a panel.
    pub fn register_panel(&mut self, id: &str, title: &str) -> Result<u32, HostError> {
        if !self.caps.ui {
            return Err(HostError::PermissionDenied("ui".into()));
        }
        if self.panels.len() >= 8 {
            return Err(HostError::QuotaExceeded("panels".into()));
        }
        let panel = (self.panels.len() as u32) + 1;
        self.panels.push(PanelDescriptor {
            id: panel,
            panel_id: id.to_owned(),
            title: title.to_owned(),
            status: String::new(),
        });
        Ok(panel)
    }

    /// Updates a panel's status text.
    pub fn set_panel_status(&mut self, panel: u32, text: &str) -> Result<(), HostError> {
        if !self.caps.ui {
            return Err(HostError::PermissionDenied("ui".into()));
        }
        let Some(slot) = self.panels.iter_mut().find(|item| item.id == panel) else {
            return Err(HostError::NotFound(format!("panel {panel}")));
        };
        slot.status = text.to_owned();
        Ok(())
    }

    /// Reads a relative path inside the plugin directory. `..`, absolute paths, and missing grants are rejected without touching the target.
    pub fn read_file(&mut self, rel: &str) -> Result<Vec<u8>, HostError> {
        if !self.caps.filesystem {
            return Err(HostError::PermissionDenied("filesystem".into()));
        }
        if !is_safe_relative(rel) {
            return Err(HostError::PermissionDenied(format!("path {rel}")));
        }
        let Some(root) = &self.file_root else {
            return Err(HostError::PermissionDenied("no plugin directory".into()));
        };
        let root = root
            .canonicalize()
            .map_err(|err| HostError::NotFound(err.to_string()))?;
        let path = root.join(rel);
        let path = path
            .canonicalize()
            .map_err(|err| HostError::NotFound(err.to_string()))?;
        if !path.starts_with(&root) {
            return Err(HostError::PermissionDenied(
                "path escaped plugin directory".into(),
            ));
        }
        let mut file = std::fs::File::open(&path).map_err(|err| HostError::NotFound(err.to_string()))?;
        if let Ok(meta) = file.metadata()
            && meta.len() > MAX_PLUGIN_FILE_BYTES as u64
        {
            return Err(HostError::PermissionDenied(format!(
                "file too large ({} > {MAX_PLUGIN_FILE_BYTES})",
                meta.len()
            )));
        }
        let mut bytes = Vec::new();
        use std::io::Read;
        file.by_ref()
            .take(MAX_PLUGIN_FILE_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|err| HostError::NotFound(err.to_string()))?;
        if bytes.len() > MAX_PLUGIN_FILE_BYTES {
            return Err(HostError::PermissionDenied("file too large".into()));
        }
        self.last_file = bytes.clone();
        Ok(bytes)
    }

    /// Records one allowed HTTPS request. Denied URLs do not increase `http_hits`.
    pub fn http_request(&mut self, url: &str, method: &str) -> Result<Vec<u8>, HostError> {
        if !self.caps.allows_url(url) {
            return Err(HostError::PermissionDenied(format!("network {url}")));
        }
        if !method.eq_ignore_ascii_case("GET") && !method.eq_ignore_ascii_case("POST") {
            return Err(HostError::Invalid(format!("method {method}")));
        }
        // Host-side logging has a quota too: a plugin can issue many requests within its fuel budget, and
        // an unbounded Vec would bypass the guest memory limit and blow up the host.
        if self.http_log.len() >= MAX_HTTP_LOG {
            self.http_log.remove(0);
        }
        self.http_log.push(url.chars().take(MAX_URL_CHARS).collect());
        self.http_hits.fetch_add(1, Ordering::Relaxed);
        Ok(Vec::new())
    }
}

const MAX_PLUGIN_FILE_BYTES: usize = 8 * 1024 * 1024;
const MAX_HTTP_LOG: usize = 64;
const MAX_URL_CHARS: usize = 4096;

fn is_safe_relative(rel: &str) -> bool {
    let path = Path::new(rel);
    !rel.is_empty()
        && !path.is_absolute()
        && !rel.contains("..")
        && !rel.contains('\\')
        && !rel.starts_with('/')
}

/// Collapses [`HostError`] into the plugin ABI status code.
#[cfg(feature = "wasm-plugins")]
pub fn host_code(err: HostError) -> i32 {
    match err {
        HostError::PermissionDenied(_) => 1,
        HostError::NotFound(_) => 2,
        HostError::Invalid(_) => 3,
        HostError::QuotaExceeded(_) => 4,
    }
}

impl From<HostError> for PluginError {
    fn from(value: HostError) -> Self {
        PluginError::CapabilityDenied(value.to_string())
    }
}
