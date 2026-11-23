//! 宿主调用门。WASM 导入和直接调用走同一套权限检查。

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use crate::PluginError;
use crate::capability::CapabilitySet;

/// 插件交回来、尚未进入编辑器的命令。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingCommand {
    /// 来源插件 id。
    pub plugin: String,
    /// 命令名，例如 `add-node`。
    pub name: String,
    /// JSON 参数。
    pub args_json: String,
}

/// 面板描述。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PanelDescriptor {
    /// 插件分配到的面板号（从 1 开始）。
    pub id: u32,
    /// 插件内的面板 id。
    pub panel_id: String,
    /// 标题。
    pub title: String,
    /// 最近一次状态文本。
    pub status: String,
}

/// 权限或参数错误。插件看到的是状态码，宿主保留这个枚举。
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[non_exhaustive]
pub enum HostError {
    /// 未授权。
    #[error("permission denied: {0}")]
    PermissionDenied(String),
    /// 找不到对象或文件。
    #[error("not found: {0}")]
    NotFound(String),
    /// 参数不合法。
    #[error("invalid argument: {0}")]
    Invalid(String),
    /// 超出配额。
    #[error("quota exceeded: {0}")]
    QuotaExceeded(String),
}

/// 一次宿主调用的记账。
#[derive(Debug)]
pub struct HostGate {
    /// 插件 id。
    pub plugin_id: String,
    /// 求交之后的能力。
    pub caps: CapabilitySet,
    /// 已入队的命令。
    pub commands: Vec<PendingCommand>,
    /// 面板。
    pub panels: Vec<PanelDescriptor>,
    /// 本事件剩余的命令配额。
    pub command_budget: u32,
    /// 快照版本（只读）。
    pub snapshot_version: u64,
    /// 允许读取的根目录。
    pub file_root: Option<PathBuf>,
    /// 最近一次成功读到的字节。
    pub last_file: Vec<u8>,
    /// 实际放行的 URL。被拒绝的不会记进来。
    pub http_log: Vec<String>,
    /// 放行的 HTTP 次数，方便跨插件断言。
    pub http_hits: Arc<AtomicU32>,
}

impl HostGate {
    /// 新建。`http_hits` 只在 URL 被放行时增加。
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

    /// 只读快照版本。始终允许。
    pub fn snapshot_version(&self) -> u64 {
        self.snapshot_version
    }

    /// 入队一条命令。未授权或超额都不会写入 `commands`。
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

    /// 注册面板。
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

    /// 更新面板状态文本。
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

    /// 读插件目录内的相对路径。`..`、绝对路径和无授权都拒绝，并且不访问目标。
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

    /// 记录一次放行的 HTTPS 请求。拒绝的 URL 不会增加 `http_hits`。
    pub fn http_request(&mut self, url: &str, method: &str) -> Result<Vec<u8>, HostError> {
        if !self.caps.allows_url(url) {
            return Err(HostError::PermissionDenied(format!("network {url}")));
        }
        if !method.eq_ignore_ascii_case("GET") && !method.eq_ignore_ascii_case("POST") {
            return Err(HostError::Invalid(format!("method {method}")));
        }
        // 宿主侧日志同样有配额：插件可以在 fuel 预算内发很多请求，
        // 无上限的 Vec 会绕过 guest 的内存限制把宿主撑爆。
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

/// 把 [`HostError`] 收成插件 ABI 的状态码。
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
