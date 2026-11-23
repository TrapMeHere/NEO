//! OSM 账户存储与 OAuth 登录流程（应用层）。
//!
//! 令牌保存在本机配置目录（`%APPDATA%/NEO/accounts.json`，非 Windows 用 `$HOME/.config/neo/`），
//! 明文存储；OAuth2 + PKCE，使用内置公开客户端（对用户透明，无需填写）。

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// 内置 OAuth client_id 的 XOR 密文（源码中不出现明文）。
const OAUTH_CLIENT_ID_OBF: &[u8] = &[
    0x2f, 0x3f, 0x01, 0x63, 0x23, 0x5a, 0x02, 0x5c, 0x44, 0x01, 0x74, 0x0a, 0x76, 0x73, 0x66,
    0x02, 0x69, 0x67, 0x30, 0x2a, 0x5e, 0x07, 0x3f, 0x15, 0x2c, 0x64, 0x57, 0x2b, 0x54, 0x38,
    0x4a, 0x0d, 0x36, 0x30, 0x54, 0x30, 0x08, 0x15, 0x4f, 0x73, 0x2e, 0x78, 0x59,
];

/// 内置 OAuth 客户端 id（运行时解密；对用户透明，无需填写）。
pub fn oauth_client_id() -> String {
    static CACHE: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    CACHE
        .get_or_init(|| neo_core::reveal(OAUTH_CLIENT_ID_OBF))
        .clone()
}

/// 固定回调地址（须与 OSM OAuth 应用登记一致）。
pub const OAUTH_REDIRECT_URI: &str = "http://127.0.0.1:8765/callback";

/// 一个已保存的 OSM 账户。
#[derive(Clone, Serialize, Deserialize)]
pub struct Account {
    /// OSM 用户 id（字符串）。
    pub id: String,
    /// 显示名。
    pub display_name: String,
    /// access token。
    pub access_token: String,
    /// refresh token。
    #[serde(default)]
    pub refresh_token: Option<String>,
    /// 过期时间（unix 秒）。
    #[serde(default)]
    pub expires_at: Option<i64>,
    /// 授权 scope。
    #[serde(default)]
    pub scopes: Option<String>,
}

/// 手写 Debug：token 不能进日志。
impl std::fmt::Debug for Account {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Account")
            .field("id", &self.id)
            .field("display_name", &self.display_name)
            .field("access_token", &"<redacted>")
            .field(
                "refresh_token",
                &self.refresh_token.as_ref().map(|_| "<redacted>"),
            )
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

impl Account {
    /// 从 OAuth token 与用户信息构造。
    pub fn new(user_id: i64, display_name: &str, tokens: &neo_api::oauth::TokenSet) -> Self {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        Self {
            id: user_id.to_string(),
            display_name: display_name.to_owned(),
            access_token: tokens.access_token.clone(),
            refresh_token: tokens.refresh_token.clone(),
            expires_at: tokens.expires_in.map(|secs| now + secs),
            scopes: tokens.scope.clone(),
        }
    }
}

/// 账户配置文件。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AccountsFile {
    /// OAuth client_id。
    #[serde(default = "default_client_id")]
    pub client_id: String,
    /// 回调地址。
    #[serde(default = "default_redirect")]
    pub redirect_uri: String,
    /// 当前账户 id。
    #[serde(default)]
    pub current: Option<String>,
    /// 已保存账户。
    #[serde(default)]
    pub accounts: Vec<Account>,
}

fn default_redirect() -> String {
    OAUTH_REDIRECT_URI.to_owned()
}

fn default_client_id() -> String {
    oauth_client_id()
}

impl Default for AccountsFile {
    fn default() -> Self {
        Self {
            client_id: default_client_id(),
            redirect_uri: default_redirect(),
            current: None,
            accounts: Vec::new(),
        }
    }
}

/// 配置路径。
pub fn config_path() -> PathBuf {
    if let Ok(local) = std::env::var("LOCALAPPDATA")
        && !local.is_empty()
    {
        return PathBuf::from(local).join("NEO").join("accounts.json");
    }
    if let Ok(appdata) = std::env::var("APPDATA")
        && !appdata.is_empty()
    {
        return PathBuf::from(appdata).join("NEO").join("accounts.json");
    }
    if let Ok(home) = std::env::var("HOME")
        && !home.is_empty()
    {
        return PathBuf::from(home)
            .join(".config")
            .join("neo")
            .join("accounts.json");
    }
    // 绝不把明文 token 写进当前工作目录（可能是共享/同步目录）。
    std::env::temp_dir().join("NEO").join("accounts.json")
}

impl AccountsFile {
    /// 读取（不存在返回默认；损坏时备份原文件并保留可诊断信息）。
    pub fn load() -> Self {
        let path = config_path();
        let Ok(text) = std::fs::read_to_string(&path) else {
            return Self::default();
        };
        let mut file: Self = match serde_json::from_str(&text) {
            Ok(file) => file,
            Err(err) => {
                // 绝不静默覆盖损坏文件：先备份，用户至少能手工找回 token。
                let backup = path.with_extension(format!(
                    "corrupt-{}",
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_secs())
                        .unwrap_or(0)
                ));
                let _ = std::fs::rename(&path, &backup);
                tracing::warn!("accounts.json is corrupt ({err}); moved to {}", backup.display());
                return Self::default();
            }
        };
        // 始终使用内置 client；避免旧配置文件里残留失效 id。
        file.client_id = default_client_id();
        if file.redirect_uri.trim().is_empty() {
            file.redirect_uri = default_redirect();
        }
        file
    }

    /// 保存（临时文件 + rename，崩溃不会留下半截 JSON）。
    pub fn save(&self) -> std::io::Result<()> {
        let path = config_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let text =
            serde_json::to_string_pretty(self).map_err(|err| std::io::Error::other(err.to_string()))?;
        let tmp = path.with_extension(format!("json.{}.tmp", std::process::id()));
        std::fs::write(&tmp, text)?;
        match std::fs::rename(&tmp, &path) {
            Ok(()) => Ok(()),
            Err(first) => {
                if std::fs::remove_file(&path).is_ok() {
                    std::fs::rename(&tmp, &path)
                } else {
                    let _ = std::fs::remove_file(&tmp);
                    Err(first)
                }
            }
        }
    }

    /// 当前账户。
    pub fn active(&self) -> Option<&Account> {
        let id = self.current.as_ref()?;
        self.accounts.iter().find(|a| &a.id == id)
    }

    /// 添加或更新账户并设为当前。
    pub fn upsert(&mut self, account: Account) {
        self.current = Some(account.id.clone());
        if let Some(existing) = self.accounts.iter_mut().find(|a| a.id == account.id) {
            *existing = account;
        } else {
            self.accounts.push(account);
        }
    }

    /// 选择当前账户。
    pub fn select(&mut self, id: &str) -> bool {
        if self.accounts.iter().any(|a| a.id == id) {
            self.current = Some(id.to_owned());
            true
        } else {
            false
        }
    }

    /// 删除账户（若删除当前账户则清空 current）。
    pub fn remove(&mut self, id: &str) -> Option<Account> {
        let idx = self.accounts.iter().position(|a| a.id == id)?;
        let removed = self.accounts.remove(idx);
        if self.current.as_deref() == Some(id) {
            self.current = None;
        }
        Some(removed)
    }
}

/// 从回调地址解析端口与路径。
pub fn redirect_parts(redirect_uri: &str) -> Option<(u16, String)> {
    let url = url::Url::parse(redirect_uri).ok()?;
    let port = url.port_or_known_default()?;
    let path = if url.path().is_empty() {
        "/".to_owned()
    } else {
        url.path().to_owned()
    };
    Some((port, path))
}

/// 打开系统浏览器。
pub fn open_browser(url: &str) -> Result<(), String> {
    // 用 `webbrowser`（Windows 走 ShellExecuteW，macOS 走 `open`，Linux 走 `xdg-open`）。
    // 不经过 cmd.exe，避免 URL 里的 `&` 被当成命令分隔符；也避免把 URL 当文件路径。
    webbrowser::open(url).map(|_| ()).map_err(|err| err.to_string())
}

/// 本地回调请求的解析结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OAuthCallbackParse {
    /// 非 OAuth 请求（favicon / 空探测等），应继续等待。
    Ignore,
    /// 拿到授权码。
    Code(String),
    /// OAuth 错误或 state 不匹配。
    Failed(String),
}

/// 解析浏览器打到本地监听端口的原始 HTTP 请求。
///
/// 浏览器常会先打 `/favicon.ico` 或空连接；这些必须忽略，不能当成 state 失败。
pub fn parse_oauth_callback_http(
    request: &str,
    expected_state: &str,
    expected_path: &str,
    expected_host: &str,
) -> OAuthCallbackParse {
    let mut lines = request.lines();
    let request_line = lines.next().unwrap_or_default();
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or_default();
    let target = parts.next().unwrap_or("/");
    if method != "GET" {
        return OAuthCallbackParse::Ignore;
    }
    // Host 头校验：阻止 DNS rebinding（浏览器可带任意 Host 访问 127.0.0.1）。
    let host_ok = lines.clone().any(|line| {
        line.split_once(':').is_some_and(|(name, value)| {
            name.eq_ignore_ascii_case("host") && value.trim().eq_ignore_ascii_case(expected_host)
        })
    });
    if !host_ok {
        return OAuthCallbackParse::Ignore;
    }
    let Ok(parsed) = url::Url::parse(&format!("http://127.0.0.1{target}")) else {
        return OAuthCallbackParse::Ignore;
    };
    if parsed.path() != expected_path {
        return OAuthCallbackParse::Ignore;
    }
    let mut code = None;
    let mut got_state = None;
    let mut error = None;
    let mut error_desc = None;
    for (key, value) in parsed.query_pairs() {
        match key.as_ref() {
            "code" => code = Some(value.into_owned()),
            "state" => got_state = Some(value.into_owned()),
            "error" => error = Some(value.into_owned()),
            "error_description" => error_desc = Some(value.into_owned()),
            _ => {}
        }
    }
    if code.is_none() && error.is_none() {
        return OAuthCallbackParse::Ignore;
    }
    if got_state.as_deref() != Some(expected_state) {
        // 任意网页都能向本地端口发一个 state 不匹配的请求；如果把它当成致命错误，
        // 攻击者就能远程打断登录。继续等待，直到超时。
        return OAuthCallbackParse::Ignore;
    }
    if let Some(error) = error {
        let detail = error_desc
            .filter(|d| !d.is_empty())
            .map(|d| format!("{error}: {d}"))
            .unwrap_or(error);
        return OAuthCallbackParse::Failed(detail);
    }
    match code {
        Some(code) => OAuthCallbackParse::Code(code),
        None => OAuthCallbackParse::Failed("missing code in callback".to_owned()),
    }
}

/// 把 OSM token 端点的错误体整理成可读字符串。
pub fn format_oauth_http_error(err: &str) -> String {
    let body = err
        .rsplit_once(": ")
        .map(|(_, body)| body.trim())
        .unwrap_or(err.trim());
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(body) {
        let code = value
            .get("error")
            .and_then(|v| v.as_str())
            .unwrap_or("oauth_error");
        let desc = value
            .get("error_description")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if desc.is_empty() {
            return code.to_owned();
        }
        return format!("{code}: {desc}");
    }
    err.to_owned()
}
