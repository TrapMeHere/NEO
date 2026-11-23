//! OAuth 2.0（PKCE）与 OSM 账户信息。
//!
//! NEO 使用 OSM 的 OAuth2（授权码 + PKCE，公开客户端，无 client_secret）：
//! 1. `authorize_url` 生成浏览器授权地址；
//! 2. 回调拿到 `code` 后 `exchange_code` 换 access/refresh token；
//! 3. `fetch_user` 读取 `/api/0.6/user/details` 得到用户名。
//!
//! 网络传输仍走 [`crate::Transport`]，因此测试无需真实浏览器/网络。

use base64::Engine as _;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{ApiError, HttpRequest, Transport};

/// OAuth 配置。
#[derive(Clone, Debug)]
pub struct OAuthConfig {
    /// 授权页。
    pub authorize_url: String,
    /// token 端点。
    pub token_url: String,
    /// API 根（用于读取用户信息）。
    pub api_base: String,
    /// 注册的 client_id。
    pub client_id: String,
    /// 回调地址（必须与 OSM OAuth 应用登记一致）。
    pub redirect_uri: String,
    /// 权限范围。
    pub scopes: String,
}

impl OAuthConfig {
    /// OSM 官方默认配置。
    pub fn openstreetmap(client_id: impl Into<String>, redirect_uri: impl Into<String>) -> Self {
        Self {
            authorize_url: "https://www.openstreetmap.org/oauth2/authorize".to_owned(),
            token_url: "https://www.openstreetmap.org/oauth2/token".to_owned(),
            api_base: "https://api.openstreetmap.org".to_owned(),
            client_id: client_id.into(),
            redirect_uri: redirect_uri.into(),
            scopes: "read_prefs write_api".to_owned(),
        }
    }
}

/// PKCE verifier/challenge。
#[derive(Clone)]
pub struct Pkce {
    /// 高熵随机串。
    pub verifier: String,
    /// S256(verifier) 的 base64url。
    pub challenge: String,
}

impl core::fmt::Debug for Pkce {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Pkce")
            .field("verifier", &"<redacted>")
            .field("challenge", &self.challenge)
            .finish()
    }
}

impl Pkce {
    /// 生成新的 PKCE。
    pub fn generate() -> Self {
        use rand::RngCore;
        let mut bytes = [0u8; 32];
        rand::rng().fill_bytes(&mut bytes);
        let verifier = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes);
        let digest = Sha256::digest(verifier.as_bytes());
        let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest);
        Self { verifier, challenge }
    }
}

/// 生成授权 URL（含 state）。
pub fn authorize_url(config: &OAuthConfig, state: &str, pkce: &Pkce) -> String {
    let mut url = url::Url::parse(&config.authorize_url).expect("valid authorize url");
    url.query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("client_id", &config.client_id)
        .append_pair("redirect_uri", &config.redirect_uri)
        .append_pair("scope", &config.scopes)
        .append_pair("state", state)
        .append_pair("code_challenge", &pkce.challenge)
        .append_pair("code_challenge_method", "S256");
    url.into()
}

/// token 集合。
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TokenSet {
    /// access token。
    pub access_token: String,
    /// refresh token（可选）。
    #[serde(default)]
    pub refresh_token: Option<String>,
    /// 有效期（秒）。
    #[serde(default)]
    pub expires_in: Option<i64>,
    /// 权限范围。
    #[serde(default)]
    pub scope: Option<String>,
}

impl core::fmt::Debug for TokenSet {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("TokenSet")
            .field("access_token", &"<redacted>")
            .field(
                "refresh_token",
                &self.refresh_token.as_ref().map(|_| "<redacted>"),
            )
            .field("expires_in", &self.expires_in)
            .field("scope", &self.scope)
            .finish()
    }
}

fn form_request(url: &str, form: &[(&str, String)]) -> HttpRequest {
    let body = form
        .iter()
        .map(|(k, v)| format!("{}={}", k, urlencode(v)))
        .collect::<Vec<_>>()
        .join("&");
    HttpRequest {
        method: "POST".to_owned(),
        url: url.to_owned(),
        headers: vec![
            (
                "Content-Type".to_owned(),
                "application/x-www-form-urlencoded".to_owned(),
            ),
            ("Accept".to_owned(), "application/json".to_owned()),
        ],
        body,
    }
}

fn urlencode(value: &str) -> String {
    let mut out = String::new();
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(byte as char),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

fn send<T: Transport>(transport: &T, request: HttpRequest) -> Result<String, ApiError> {
    let response = transport.send(request)?;
    match response.status {
        200..=299 => Ok(response.body),
        401 | 403 => Err(ApiError::Unauthorized(response.body)),
        status => Err(ApiError::Http {
            status,
            body: response.body,
        }),
    }
}

/// 用授权码换 token。
pub fn exchange_code<T: Transport>(
    transport: &T,
    config: &OAuthConfig,
    code: &str,
    pkce: &Pkce,
) -> Result<TokenSet, ApiError> {
    let form = [
        ("grant_type", "authorization_code".to_owned()),
        ("code", code.to_owned()),
        ("client_id", config.client_id.clone()),
        ("redirect_uri", config.redirect_uri.clone()),
        ("code_verifier", pkce.verifier.clone()),
    ];
    let body = send(transport, form_request(&config.token_url, &form))?;
    serde_json::from_str(&body).map_err(|err| ApiError::Protocol(format!("token response: {err}: {body}")))
}

/// 刷新 access token。
pub fn refresh_token<T: Transport>(
    transport: &T,
    config: &OAuthConfig,
    refresh: &str,
) -> Result<TokenSet, ApiError> {
    let form = [
        ("grant_type", "refresh_token".to_owned()),
        ("refresh_token", refresh.to_owned()),
        ("client_id", config.client_id.clone()),
    ];
    let body = send(transport, form_request(&config.token_url, &form))?;
    serde_json::from_str(&body).map_err(|err| ApiError::Protocol(format!("refresh response: {err}: {body}")))
}

/// 当前用户。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UserInfo {
    /// 数字 id。
    pub id: i64,
    /// 显示名。
    pub display_name: String,
}

/// 读取当前登录用户。
pub fn fetch_user<T: Transport>(
    transport: &T,
    api_base: &str,
    access_token: &str,
) -> Result<UserInfo, ApiError> {
    let request = HttpRequest {
        method: "GET".to_owned(),
        url: format!("{}/api/0.6/user/details", api_base.trim_end_matches('/')),
        headers: vec![
            ("Authorization".to_owned(), format!("Bearer {access_token}")),
            ("Accept".to_owned(), "application/xml".to_owned()),
        ],
        body: String::new(),
    };
    let body = send(transport, request)?;
    parse_user(&body)
}

fn parse_user(xml: &str) -> Result<UserInfo, ApiError> {
    let mut reader = quick_xml::Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    loop {
        match reader.read_event() {
            Ok(quick_xml::events::Event::Start(e)) | Ok(quick_xml::events::Event::Empty(e)) => {
                if e.name().as_ref() != "user" {
                    continue;
                }
                let mut id = None;
                let mut name = None;
                for attr in e.attributes().flatten() {
                    let key = attr.key.as_ref();
                    let value = attr
                        .normalized_value(quick_xml::XmlVersion::Explicit1_0)
                        .unwrap_or_default()
                        .into_owned();
                    match key {
                        "id" => id = value.parse::<i64>().ok(),
                        "display_name" => name = Some(value),
                        _ => {}
                    }
                }
                if let (Some(id), Some(display_name)) = (id, name) {
                    return Ok(UserInfo { id, display_name });
                }
            }
            Ok(quick_xml::events::Event::Eof) | Err(_) => break,
            _ => {}
        }
    }
    Err(ApiError::Protocol("no <user> in details response".into()))
}
