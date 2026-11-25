//! OSM account storage and OAuth login flow (application layer).
//!
//! Tokens are stored in the local config directory (`%APPDATA%/NEO/accounts.json`, or `$HOME/.config/neo/` on non-Windows),
//! in plain text; OAuth2 + PKCE with a built-in public client (transparent to the user, no input required).

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// XOR-obfuscated built-in OAuth client_id (no plaintext in the source).
const OAUTH_CLIENT_ID_OBF: &[u8] = &[
    0x2f, 0x3f, 0x01, 0x63, 0x23, 0x5a, 0x02, 0x5c, 0x44, 0x01, 0x74, 0x0a, 0x76, 0x73, 0x66, 0x02, 0x69,
    0x67, 0x30, 0x2a, 0x5e, 0x07, 0x3f, 0x15, 0x2c, 0x64, 0x57, 0x2b, 0x54, 0x38, 0x4a, 0x0d, 0x36, 0x30,
    0x54, 0x30, 0x08, 0x15, 0x4f, 0x73, 0x2e, 0x78, 0x59,
];

/// Built-in OAuth client id (decrypted at runtime; transparent to the user, no input required).
pub fn oauth_client_id() -> String {
    static CACHE: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    CACHE
        .get_or_init(|| neo_core::reveal(OAUTH_CLIENT_ID_OBF))
        .clone()
}

/// Fixed callback URI (must match the OSM OAuth application registration).
pub const OAUTH_REDIRECT_URI: &str = "http://127.0.0.1:8765/callback";

/// A saved OSM account.
#[derive(Clone, Serialize, Deserialize)]
pub struct Account {
    /// OSM user id (string).
    pub id: String,
    /// Display name.
    pub display_name: String,
    /// Access token.
    pub access_token: String,
    /// Refresh token.
    #[serde(default)]
    pub refresh_token: Option<String>,
    /// Expiry time (unix seconds).
    #[serde(default)]
    pub expires_at: Option<i64>,
    /// Authorized scope.
    #[serde(default)]
    pub scopes: Option<String>,
}

/// Hand-written Debug: tokens must never reach the logs.
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
    /// Builds from OAuth tokens and user info.
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

/// Account config file.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AccountsFile {
    /// OAuth client_id.
    #[serde(default = "default_client_id")]
    pub client_id: String,
    /// Callback URI.
    #[serde(default = "default_redirect")]
    pub redirect_uri: String,
    /// Current account id.
    #[serde(default)]
    pub current: Option<String>,
    /// Saved accounts.
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

/// Config path.
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
    // Never write a plaintext token into the current working directory (it may be a shared/synced directory).
    std::env::temp_dir().join("NEO").join("accounts.json")
}

impl AccountsFile {
    /// Load (missing file returns the default; a corrupt file is backed up and left diagnosable).
    pub fn load() -> Self {
        let path = config_path();
        let Ok(text) = std::fs::read_to_string(&path) else {
            return Self::default();
        };
        let mut file: Self = match serde_json::from_str(&text) {
            Ok(file) => file,
            Err(err) => {
                // Never silently overwrite a corrupt file: back it up first so the user can at least recover the token by hand.
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
        // Always use the built-in client; avoids a stale id lingering in an old config file.
        file.client_id = default_client_id();
        if file.redirect_uri.trim().is_empty() {
            file.redirect_uri = default_redirect();
        }
        file
    }

    /// Save (temp file + rename, so a crash never leaves half-written JSON).
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

    /// Current account.
    pub fn active(&self) -> Option<&Account> {
        let id = self.current.as_ref()?;
        self.accounts.iter().find(|a| &a.id == id)
    }

    /// Adds or updates an account and makes it current.
    pub fn upsert(&mut self, account: Account) {
        self.current = Some(account.id.clone());
        if let Some(existing) = self.accounts.iter_mut().find(|a| a.id == account.id) {
            *existing = account;
        } else {
            self.accounts.push(account);
        }
    }

    /// Selects the current account.
    pub fn select(&mut self, id: &str) -> bool {
        if self.accounts.iter().any(|a| a.id == id) {
            self.current = Some(id.to_owned());
            true
        } else {
            false
        }
    }

    /// Removes an account (clears `current` if the current account is removed).
    pub fn remove(&mut self, id: &str) -> Option<Account> {
        let idx = self.accounts.iter().position(|a| a.id == id)?;
        let removed = self.accounts.remove(idx);
        if self.current.as_deref() == Some(id) {
            self.current = None;
        }
        Some(removed)
    }
}

/// Parses the port and path out of the callback URI.
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

/// Opens the system browser.
pub fn open_browser(url: &str) -> Result<(), String> {
    // Uses `webbrowser` (ShellExecuteW on Windows, `open` on macOS, `xdg-open` on Linux).
    // Avoids going through cmd.exe, so `&` in the URL is not treated as a command separator; also avoids treating the URL as a file path.
    webbrowser::open(url).map(|_| ()).map_err(|err| err.to_string())
}

/// Parse result of a local callback request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OAuthCallbackParse {
    /// Not an OAuth request (favicon / empty probe etc.); keep waiting.
    Ignore,
    /// Got the authorization code.
    Code(String),
    /// OAuth error or state mismatch.
    Failed(String),
}

/// Parses the raw HTTP request the browser sends to the local listening port.
///
/// Browsers often first hit `/favicon.ico` or open empty connections; these must be ignored and must not count as a state failure.
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
    // Host header check: prevents DNS rebinding (a browser can visit 127.0.0.1 with an arbitrary Host).
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
        // Any web page can send a request with a mismatched state to the local port; treating it as fatal
        // would let an attacker remotely interrupt the login. Keep waiting until the timeout.
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

/// Turns the error body from the OSM token endpoint into a readable string.
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
