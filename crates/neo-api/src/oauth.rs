//! OAuth 2.0 (PKCE) and OSM account information.
//!
//! NEO uses OSM's OAuth2 (authorization code + PKCE, public client, no client_secret):
//! 1. `authorize_url` builds the browser authorization URL;
//! 2. after the callback returns `code`, `exchange_code` swaps it for access/refresh tokens;
//! 3. `fetch_user` reads `/api/0.6/user/details` to get the user name.
//!
//! Network transport still goes through [`crate::Transport`], so tests need no real browser/network.

use base64::Engine as _;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{ApiError, HttpRequest, Transport};

/// OAuth configuration.
#[derive(Clone, Debug)]
pub struct OAuthConfig {
    /// Authorization page.
    pub authorize_url: String,
    /// Token endpoint.
    pub token_url: String,
    /// API root (used to read user info).
    pub api_base: String,
    /// Registered client_id.
    pub client_id: String,
    /// Callback URI (must match the OSM OAuth application registration).
    pub redirect_uri: String,
    /// Permission scope.
    pub scopes: String,
}

impl OAuthConfig {
    /// OSM official default configuration.
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

/// PKCE verifier/challenge.
#[derive(Clone)]
pub struct Pkce {
    /// High-entropy random string.
    pub verifier: String,
    /// base64url of S256(verifier).
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
    /// Generates a new PKCE.
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

/// Builds the authorization URL (including state).
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

/// Token set.
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TokenSet {
    /// Access token.
    pub access_token: String,
    /// Refresh token (optional).
    #[serde(default)]
    pub refresh_token: Option<String>,
    /// Validity period (seconds).
    #[serde(default)]
    pub expires_in: Option<i64>,
    /// Permission scope.
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

/// Exchanges the authorization code for tokens.
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

/// Refreshes the access token.
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

/// Current user.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UserInfo {
    /// Numeric id.
    pub id: i64,
    /// Display name.
    pub display_name: String,
}

/// Reads the currently logged-in user.
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
