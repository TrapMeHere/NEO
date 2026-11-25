//! # neo-api — OSM API 0.6 client
//!
//! This layer only handles **HTTP semantics and the XML protocol**: changeset create/close, diff upload,
//! area download, 409 conflict parsing. The transport layer is injected via [`Transport`], so unit tests need
//! no network and the application layer can choose reqwest (feature `http`) or its own mock/proxy.
//!
//! The **business merge** of conflicts (MergeCase) belongs to `neo-editor`; this crate only translates the
//! 409 XML into [`ConflictReport`] (object id/version + server message) and does not depend on the editor
//! (see docs/06 §2.6).

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod oauth;

use std::collections::VecDeque;
use std::sync::Mutex;

use neo_core::{ObjectId, ObjectKind};

/// One HTTP request (decoupled from any concrete client).
#[derive(Clone, PartialEq, Eq)]
pub struct HttpRequest {
    /// Method (GET/POST/PUT).
    pub method: String,
    /// Absolute URL.
    pub url: String,
    /// Request headers.
    pub headers: Vec<(String, String)>,
    /// Request body.
    pub body: String,
}

/// Hand-written Debug: the headers carry `Authorization: Bearer <token>`, and `{:?}` must not
/// leak long-lived credentials into logs or test output.
impl core::fmt::Debug for HttpRequest {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let headers: Vec<String> = self
            .headers
            .iter()
            .map(|(key, _)| {
                if key.eq_ignore_ascii_case("authorization") {
                    format!("{key}: <redacted>")
                } else {
                    key.clone()
                }
            })
            .collect();
        f.debug_struct("HttpRequest")
            .field("method", &self.method)
            .field("url", &self.url)
            .field("headers", &headers)
            .field("body_len", &self.body.len())
            .finish()
    }
}

/// One HTTP response.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HttpResponse {
    /// HTTP status code.
    pub status: u16,
    /// Response body.
    pub body: String,
}

/// Transport error.
#[derive(Debug, thiserror::Error)]
pub enum TransportError {
    /// Network/client error.
    #[error("transport: {0}")]
    Other(String),
}

/// Injectable HTTP transport layer.
pub trait Transport: Send + Sync {
    /// Sends a request.
    fn send(&self, request: HttpRequest) -> Result<HttpResponse, TransportError>;
}

/// API error.
#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    /// Transport failure.
    #[error(transparent)]
    Transport(#[from] TransportError),
    /// Authentication failure (401/403).
    #[error("authentication failed: {0}")]
    Unauthorized(String),
    /// 409: upload conflict.
    #[error("conflict: {0:?}")]
    Conflict(ConflictReport),
    /// 412: precondition failed (version/changeset state).
    #[error("precondition failed: {0}")]
    Precondition(String),
    /// Other HTTP errors.
    #[error("http {status}: {body}")]
    Http {
        /// Status code.
        status: u16,
        /// Response body.
        body: String,
    },
    /// Malformed response.
    #[error("protocol: {0}")]
    Protocol(String),
}

/// One object involved in a conflict.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConflictObject {
    /// Object kind.
    pub kind: ObjectKind,
    /// Local (old) id (negative for newly created objects).
    pub old_id: ObjectId,
    /// New server id (present if the server created it successfully).
    pub new_id: Option<ObjectId>,
    /// Current server version (if provided).
    pub version: Option<i32>,
}

/// 409 conflict report.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct ConflictReport {
    /// Server error message.
    pub message: String,
    /// Conflicting objects.
    pub objects: Vec<ConflictObject>,
}

impl ConflictReport {
    /// Whether it contains some objects.
    pub fn contains(&self, kind: ObjectKind, id: ObjectId) -> bool {
        self.objects
            .iter()
            .any(|o| o.kind == kind && (o.old_id == id || o.new_id == Some(id)))
    }
}

/// A diffResult entry returned by an upload.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffEntry {
    /// Object kind.
    pub kind: ObjectKind,
    /// Local id.
    pub old_id: ObjectId,
    /// Server id.
    pub new_id: Option<ObjectId>,
    /// New version.
    pub new_version: Option<i32>,
}

/// Upload result.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct UploadResult {
    /// Per-object results.
    pub entries: Vec<DiffEntry>,
}

/// One note comment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NoteComment {
    /// Time (raw server string).
    pub date: String,
    /// User name (may be empty for anonymous).
    pub user: String,
    /// Text.
    pub text: String,
    /// Action: `opened`/`commented`/`closed` etc.
    pub action: String,
}

/// One OSM note.
#[derive(Clone, Debug, PartialEq)]
pub struct Note {
    /// Note ID.
    pub id: i64,
    /// Latitude.
    pub lat: f64,
    /// Longitude.
    pub lon: f64,
    /// `open` or `closed`.
    pub status: String,
    /// Comments (the first one is from the opener).
    pub comments: Vec<NoteComment>,
}

/// Changeset history (read-only metadata).
///
/// The tuple order of `bbox` matches [`ApiClient::download_map`]: `(min_lon, min_lat,
/// max_lon, max_lat)`. It is `None` when the changeset has not modified objects or the server omits the bbox attribute.
#[derive(Clone, Debug, PartialEq)]
pub struct ChangesetInfo {
    /// Changeset ID.
    pub id: i64,
    /// Author uid (may be missing for anonymous/imported changesets).
    pub uid: Option<i64>,
    /// Author display name (empty string when missing).
    pub user: String,
    /// Creation time (raw server string, ISO 8601).
    pub created_at: String,
    /// Closing time (`None` while still open).
    pub closed_at: Option<String>,
    /// Whether it is still open (when the `open` attribute is missing, inferred from whether `closed_at` is empty).
    pub open: bool,
    /// Value of the `comment` tag (empty string when there is no `comment` tag).
    pub comment: String,
    /// All tags (in order of appearance; includes `comment`).
    pub tags: Vec<(String, String)>,
    /// Number of changed objects: prefers `changes_count`, falls back to `comments_count` when missing.
    pub changes_count: Option<usize>,
    /// Changed area `(min_lon, min_lat, max_lon, max_lat)`.
    pub bbox: Option<(f64, f64, f64, f64)>,
}

/// Public OSM user information (read-only).
///
/// Unlike [`oauth::UserInfo`]: this reads the public profile from `/api/0.6/user/{uid}`,
/// which additionally contains `<description>` and the registration time; the one in `oauth` comes from `/user/details`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UserInfo {
    /// Numeric uid.
    pub id: i64,
    /// Display name.
    pub display_name: String,
    /// Personal description (text of `<description>`, possibly empty).
    pub description: String,
    /// Registration time (raw server string).
    pub account_created: String,
}

/// OSM API client.
pub struct ApiClient<T: Transport> {
    /// API root (e.g. `https://api.openstreetmap.org`).
    pub base_url: String,
    /// OAuth2 access token (optional; anonymous access can only read).
    pub token: Option<String>,
    /// Transport layer.
    pub transport: T,
}

impl<T: Transport> ApiClient<T> {
    /// Creates a client.
    pub fn new(base_url: impl Into<String>, token: Option<String>, transport: T) -> Self {
        Self {
            base_url: base_url.into().trim_end_matches('/').to_owned(),
            token,
            transport,
        }
    }

    fn request(
        &self,
        method: &str,
        path: &str,
        body: impl Into<String>,
        content_type: &str,
    ) -> Result<String, ApiError> {
        let mut headers = vec![
            (
                "User-Agent".to_owned(),
                "NEO/0.1 (+https://github.com/TrapMeHere/NEO)".to_owned(),
            ),
            ("Content-Type".to_owned(), content_type.to_owned()),
        ];
        if let Some(token) = &self.token {
            headers.push(("Authorization".to_owned(), format!("Bearer {token}")));
        }
        let response = self.transport.send(HttpRequest {
            method: method.to_owned(),
            url: format!("{}{}", self.base_url, path),
            headers,
            body: body.into(),
        })?;
        match response.status {
            200..=299 => Ok(response.body),
            401 | 403 => Err(ApiError::Unauthorized(response.body)),
            409 => Err(ApiError::Conflict(parse_conflict(&response.body))),
            412 => Err(ApiError::Precondition(response.body)),
            status => Err(ApiError::Http {
                status,
                body: response.body,
            }),
        }
    }

    /// Creates a changeset and returns its id.
    ///
    /// OSM only registers `PUT /api/0.6/changeset/create`. Using POST lands on the website's HTML 404.
    pub fn create_changeset(&self, comment: &str, created_by: &str) -> Result<i64, ApiError> {
        let body = format!(
            "<osm><changeset><tag k=\"comment\" v=\"{}\"/><tag k=\"created_by\" v=\"{}\"/></changeset></osm>",
            escape_xml(comment),
            escape_xml(created_by)
        );
        let text = self.request("PUT", "/api/0.6/changeset/create", body, "text/xml")?;
        text.trim()
            .parse::<i64>()
            .map_err(|_| ApiError::Protocol(format!("unexpected changeset id: {text}")))
    }

    /// Uploads osmChange (the caller generates the XML with `neo-io`).
    pub fn upload_diff(&self, changeset: i64, osc: &str) -> Result<UploadResult, ApiError> {
        let text = self.request(
            "POST",
            &format!("/api/0.6/changeset/{changeset}/upload"),
            osc,
            "text/xml",
        )?;
        Ok(parse_diff_result(&text))
    }

    /// Closes a changeset.
    pub fn close_changeset(&self, changeset: i64) -> Result<(), ApiError> {
        self.request(
            "PUT",
            &format!("/api/0.6/changeset/{changeset}/close"),
            "",
            "text/xml",
        )?;
        Ok(())
    }

    /// Reads changeset history (read-only): `GET /api/0.6/changeset/{id}`.
    ///
    /// The response is `<osm><changeset ...>` XML; `<discussion>` and comment bodies are ignored and only
    /// metadata plus `<tag>` are taken. Returns an error when the root element is not `<osm>` or the XML is malformed.
    /// The returned error is [`ApiError::Protocol`].
    pub fn changeset(&self, id: i64) -> Result<ChangesetInfo, ApiError> {
        let text = self.request("GET", &format!("/api/0.6/changeset/{id}"), "", "text/xml")?;
        parse_changeset(&text, id)
    }

    /// Reads the user's public information (read-only): `GET /api/0.6/user/{uid}`.
    ///
    /// No token required; XML entities in `<description>` (`&amp;` etc.) are unescaped.
    pub fn user(&self, uid: i64) -> Result<UserInfo, ApiError> {
        let text = self.request("GET", &format!("/api/0.6/user/{uid}"), "", "text/xml")?;
        parse_user(&text, uid)
    }

    /// Downloads a single object (used by "take remote" during conflicts).
    pub fn download_object(&self, kind: ObjectKind, id: ObjectId) -> Result<String, ApiError> {
        let name = match kind {
            ObjectKind::Node => "node",
            ObjectKind::Way => "way",
            ObjectKind::Relation => "relation",
            ObjectKind::Changeset => "changeset",
        };
        self.request("GET", &format!("/api/0.6/{name}/{}", id.0), "", "text/xml")
    }

    /// Downloads a bbox area (returns OSM XML; the caller parses it with `neo-io`).
    pub fn download_map(&self, bbox: (f64, f64, f64, f64)) -> Result<String, ApiError> {
        let (min_lon, min_lat, max_lon, max_lat) = bbox;
        let path = format!("/api/0.6/map?bbox={min_lon:.7},{min_lat:.7},{max_lon:.7},{max_lat:.7}");
        self.request("GET", &path, "", "text/xml")
    }

    /// Downloads OSM XML through an Overpass QL query (`endpoint` is the full interpreter URL).
    pub fn download_overpass(&self, endpoint: &str, query: &str) -> Result<String, ApiError> {
        download_overpass(&self.transport, endpoint, query)
    }

    /// Reads notes inside the bbox (at most 100 by default, including ones closed in the last 7 days).
    pub fn notes(&self, bbox: (f64, f64, f64, f64)) -> Result<Vec<Note>, ApiError> {
        let (min_lon, min_lat, max_lon, max_lat) = bbox;
        let path = format!(
            "/api/0.6/notes.json?bbox={min_lon:.7},{min_lat:.7},{max_lon:.7},{max_lat:.7}&limit=100&closed=7"
        );
        let text = self.request("GET", &path, "", "application/json")?;
        parse_notes(&text)
    }

    /// Creates a note (requires a login token).
    pub fn create_note(&self, lat: f64, lon: f64, text: &str) -> Result<Note, ApiError> {
        let body = form_body(&[
            ("lat", format!("{lat:.7}")),
            ("lon", format!("{lon:.7}")),
            ("text", text.to_owned()),
        ]);
        let response = self.request(
            "POST",
            "/api/0.6/notes.json",
            body,
            "application/x-www-form-urlencoded",
        )?;
        parse_note_response(&response)
    }

    /// Appends a comment (requires a login token).
    pub fn comment_note(&self, id: i64, text: &str) -> Result<Note, ApiError> {
        let body = form_body(&[("text", text.to_owned())]);
        let response = self.request(
            "POST",
            &format!("/api/0.6/notes/{id}/comment.json"),
            body,
            "application/x-www-form-urlencoded",
        )?;
        parse_note_response(&response)
    }

    /// Closes a note (requires a login token).
    pub fn close_note(&self, id: i64, text: &str) -> Result<Note, ApiError> {
        let body = form_body(&[("text", text.to_owned())]);
        let response = self.request(
            "POST",
            &format!("/api/0.6/notes/{id}/close.json"),
            body,
            "application/x-www-form-urlencoded",
        )?;
        parse_note_response(&response)
    }
}

fn form_body(pairs: &[(&str, String)]) -> String {
    let mut serializer = url::form_urlencoded::Serializer::new(String::new());
    for (key, value) in pairs {
        serializer.append_pair(key, value);
    }
    serializer.finish()
}

/// Submits QL to the Overpass interpreter and returns OSM XML.
///
/// `endpoint` is typically `https://overpass-api.de/api/interpreter`.
pub fn download_overpass<T: Transport>(
    transport: &T,
    endpoint: &str,
    query: &str,
) -> Result<String, ApiError> {
    let endpoint = endpoint.trim().trim_end_matches('/');
    if endpoint.is_empty() {
        return Err(ApiError::Protocol("empty overpass endpoint".into()));
    }
    if query.trim().is_empty() {
        return Err(ApiError::Protocol("empty overpass query".into()));
    }
    let body = form_body(&[("data", query.to_owned())]);
    let response = transport.send(HttpRequest {
        method: "POST".to_owned(),
        url: endpoint.to_owned(),
        headers: vec![
            (
                "User-Agent".to_owned(),
                "NEO/0.1 (+https://github.com/TrapMeHere/NEO)".to_owned(),
            ),
            (
                "Content-Type".to_owned(),
                "application/x-www-form-urlencoded".to_owned(),
            ),
        ],
        body,
    })?;
    match response.status {
        200..=299 => Ok(response.body),
        401 | 403 => Err(ApiError::Unauthorized(response.body)),
        status => Err(ApiError::Http {
            status,
            body: response.body,
        }),
    }
}

fn parse_note_response(text: &str) -> Result<Note, ApiError> {
    let value: serde_json::Value =
        serde_json::from_str(text).map_err(|err| ApiError::Protocol(err.to_string()))?;
    parse_note_value(&value).ok_or_else(|| ApiError::Protocol("malformed note response".into()))
}

fn parse_notes(text: &str) -> Result<Vec<Note>, ApiError> {
    let value: serde_json::Value =
        serde_json::from_str(text).map_err(|err| ApiError::Protocol(err.to_string()))?;
    if let Some(features) = value.get("features").and_then(|v| v.as_array()) {
        return Ok(features.iter().filter_map(parse_note_value).collect());
    }
    // The create/comment endpoints may return a single feature directly or {note: ...}.
    if let Some(note) = parse_note_value(&value) {
        return Ok(vec![note]);
    }
    if let Some(note) = value.get("note").and_then(parse_note_value) {
        return Ok(vec![note]);
    }
    Err(ApiError::Protocol("malformed notes response".into()))
}

fn parse_note_value(value: &serde_json::Value) -> Option<Note> {
    let properties = value.get("properties").unwrap_or(value);
    let id = properties.get("id")?.as_i64()?;
    let status = properties
        .get("status")
        .and_then(|v| v.as_str())
        .unwrap_or("open")
        .to_owned();
    let coords = value
        .get("geometry")
        .and_then(|v| v.get("coordinates"))
        .and_then(|v| v.as_array());
    let mut lon = None;
    let mut lat = None;
    if let Some(coords) = coords {
        lon = coords.first().and_then(|v| v.as_f64());
        lat = coords.get(1).and_then(|v| v.as_f64());
    }
    let lat = lat.or_else(|| properties.get("lat").and_then(|v| v.as_f64()))?;
    let lon = lon.or_else(|| properties.get("lon").and_then(|v| v.as_f64()))?;
    let comments = properties
        .get("comments")
        .and_then(|v| v.as_array())
        .map(|items| {
            items
                .iter()
                .map(|comment| NoteComment {
                    date: comment
                        .get("date")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_owned(),
                    user: comment
                        .get("user")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_owned(),
                    text: comment
                        .get("text")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_owned(),
                    action: comment
                        .get("action")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_owned(),
                })
                .collect()
        })
        .unwrap_or_default();
    Some(Note {
        id,
        lat,
        lon,
        status,
        comments,
    })
}

fn escape_xml(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn attr(attrs: &[(String, String)], key: &str) -> Option<String> {
    attrs.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone())
}

fn kind_of(name: &str) -> Option<ObjectKind> {
    match name {
        "node" => Some(ObjectKind::Node),
        "way" => Some(ObjectKind::Way),
        "relation" => Some(ObjectKind::Relation),
        _ => None,
    }
}

fn attrs_of(e: &quick_xml::events::BytesStart<'_>) -> Vec<(String, String)> {
    let mut attrs = Vec::new();
    for a in e.attributes().flatten() {
        attrs.push((
            a.key.as_ref().to_owned(),
            a.normalized_value(quick_xml::XmlVersion::Explicit1_0)
                .unwrap_or_default()
                .into_owned(),
        ));
    }
    attrs
}

fn parse_diff_result(xml: &str) -> UploadResult {
    let mut result = UploadResult::default();
    let mut reader = quick_xml::Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    loop {
        match reader.read_event() {
            Ok(quick_xml::events::Event::Start(e)) | Ok(quick_xml::events::Event::Empty(e)) => {
                let name = e.name().as_ref().to_owned();
                let Some(kind) = kind_of(&name) else { continue };
                let attrs = attrs_of(&e);
                let old_id = attr(&attrs, "old_id").and_then(|s| s.parse::<i64>().ok());
                let new_id = attr(&attrs, "new_id").and_then(|s| s.parse::<i64>().ok());
                let new_version = attr(&attrs, "new_version").and_then(|s| s.parse::<i32>().ok());
                if let Some(old_id) = old_id {
                    result.entries.push(DiffEntry {
                        kind,
                        old_id: ObjectId(old_id),
                        new_id: new_id.map(ObjectId),
                        new_version,
                    });
                }
            }
            Ok(quick_xml::events::Event::Eof) | Err(_) => break,
            _ => {}
        }
    }
    result
}

/// Verifies that the document root element is `<osm>` (all OSM API 0.6 XML responses use it as the root).
fn check_osm_root(root: &mut Option<String>, name: &str) -> Result<(), ApiError> {
    if root.is_none() {
        if name != "osm" {
            return Err(ApiError::Protocol(format!(
                "expected <osm> root element, got <{name}>"
            )));
        }
        *root = Some(name.to_owned());
    }
    Ok(())
}

/// Builds a [`ChangesetInfo`] from the `<changeset>` attributes (excluding the `<tag>` child elements).
///
/// `fallback_id` is used when the response lacks an `id` attribute, falling back to the id the caller requested.
fn changeset_from_attrs(attrs: &[(String, String)], fallback_id: i64) -> ChangesetInfo {
    let closed_at = attr(attrs, "closed_at").filter(|value| !value.is_empty());
    // Old or simplified servers may lack the open attribute: having closed_at means it is closed.
    let open = match attr(attrs, "open") {
        Some(value) => matches!(value.as_str(), "true" | "1"),
        None => closed_at.is_none(),
    };
    let number = |key: &str| attr(attrs, key).and_then(|value| value.parse::<f64>().ok());
    let (min_lat, min_lon, max_lat, max_lon) = (
        number("min_lat"),
        number("min_lon"),
        number("max_lat"),
        number("max_lon"),
    );
    // All four corners must be present for a valid bbox; the order matches download_map (lon first).
    let bbox = match (min_lat, min_lon, max_lat, max_lon) {
        (Some(min_lat), Some(min_lon), Some(max_lat), Some(max_lon)) => {
            Some((min_lon, min_lat, max_lon, max_lat))
        }
        _ => None,
    };
    ChangesetInfo {
        id: attr(attrs, "id")
            .and_then(|value| value.parse::<i64>().ok())
            .unwrap_or(fallback_id),
        uid: attr(attrs, "uid").and_then(|value| value.parse::<i64>().ok()),
        user: attr(attrs, "user").unwrap_or_default(),
        created_at: attr(attrs, "created_at").unwrap_or_default(),
        closed_at,
        open,
        comment: String::new(),
        tags: Vec::new(),
        changes_count: attr(attrs, "changes_count")
            .or_else(|| attr(attrs, "comments_count"))
            .and_then(|value| value.parse::<usize>().ok()),
        bbox,
    }
}

/// Appends the direct child `<tag k="..." v="...">` elements of the changeset to [`ChangesetInfo`].
fn push_changeset_tag(info: &mut Option<ChangesetInfo>, attrs: &[(String, String)]) {
    if let (Some(key), Some(value), Some(info)) = (attr(attrs, "k"), attr(attrs, "v"), info.as_mut()) {
        info.tags.push((key, value));
    }
}

/// Parses the XML of `/api/0.6/changeset/{id}`.
fn parse_changeset(xml: &str, fallback_id: i64) -> Result<ChangesetInfo, ApiError> {
    let mut reader = quick_xml::Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut root: Option<String> = None;
    let mut root_closed = false;
    let mut depth = 0usize;
    // Depth of `<changeset>` / `<discussion>`: only `<tag>` elements that are direct children of changeset
    // count as metadata; any `<tag>` inside discussion is ignored.
    let mut changeset_depth: Option<usize> = None;
    let mut discussion_depth: Option<usize> = None;
    let mut info: Option<ChangesetInfo> = None;
    loop {
        match reader.read_event() {
            Ok(quick_xml::events::Event::Start(e)) => {
                depth += 1;
                let name = e.name().as_ref().to_owned();
                check_osm_root(&mut root, &name)?;
                match name.as_str() {
                    "changeset" if info.is_none() => {
                        info = Some(changeset_from_attrs(&attrs_of(&e), fallback_id));
                        changeset_depth = Some(depth);
                    }
                    "discussion" if changeset_depth.is_some() => {
                        discussion_depth = Some(depth);
                    }
                    "tag"
                        if changeset_depth == Some(depth.saturating_sub(1)) && discussion_depth.is_none() =>
                    {
                        push_changeset_tag(&mut info, &attrs_of(&e));
                    }
                    _ => {}
                }
            }
            Ok(quick_xml::events::Event::Empty(e)) => {
                let name = e.name().as_ref().to_owned();
                if root.is_none() {
                    check_osm_root(&mut root, &name)?;
                    // `<osm/>` is self-closing, so there is no End event.
                    root_closed = true;
                }
                match name.as_str() {
                    "changeset" if info.is_none() => {
                        info = Some(changeset_from_attrs(&attrs_of(&e), fallback_id));
                    }
                    "tag" if changeset_depth == Some(depth) && discussion_depth.is_none() => {
                        push_changeset_tag(&mut info, &attrs_of(&e));
                    }
                    _ => {}
                }
            }
            Ok(quick_xml::events::Event::End(_)) => {
                if discussion_depth == Some(depth) {
                    discussion_depth = None;
                }
                if changeset_depth == Some(depth) {
                    changeset_depth = None;
                }
                if depth == 1 {
                    root_closed = true;
                }
                depth = depth.saturating_sub(1);
            }
            Ok(quick_xml::events::Event::Eof) => break,
            Err(err) => {
                return Err(ApiError::Protocol(format!("malformed changeset XML: {err}")));
            }
            _ => {}
        }
    }
    if root.is_none() {
        return Err(ApiError::Protocol("changeset response has no <osm> root".into()));
    }
    if !root_closed {
        return Err(ApiError::Protocol(
            "truncated changeset XML: missing </osm>".into(),
        ));
    }
    let mut info = info.ok_or_else(|| ApiError::Protocol("changeset response has no <changeset>".into()))?;
    info.comment = info
        .tags
        .iter()
        .find(|(key, _)| key == "comment")
        .map(|(_, value)| value.clone())
        .unwrap_or_default();
    Ok(info)
}

/// Builds a [`UserInfo`] from the `<user>` attributes (`description` is filled from the child element).
fn user_from_attrs(attrs: &[(String, String)], fallback_uid: i64) -> UserInfo {
    UserInfo {
        id: attr(attrs, "id")
            .and_then(|value| value.parse::<i64>().ok())
            .unwrap_or(fallback_uid),
        display_name: attr(attrs, "display_name").unwrap_or_default(),
        description: String::new(),
        account_created: attr(attrs, "account_created").unwrap_or_default(),
    }
}

/// Parses the XML of `/api/0.6/user/{uid}`.
fn parse_user(xml: &str, fallback_uid: i64) -> Result<UserInfo, ApiError> {
    let mut reader = quick_xml::Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut root: Option<String> = None;
    let mut root_closed = false;
    let mut depth = 0usize;
    let mut user: Option<UserInfo> = None;
    loop {
        match reader.read_event() {
            Ok(quick_xml::events::Event::Start(e)) => {
                depth += 1;
                let name = e.name().as_ref().to_owned();
                check_osm_root(&mut root, &name)?;
                match name.as_str() {
                    "user" if user.is_none() => {
                        user = Some(user_from_attrs(&attrs_of(&e), fallback_uid));
                    }
                    "description" if user.is_some() => {
                        // read_text consumes </description>, so depth must be restored manually.
                        let raw = reader
                            .read_text(e.name())
                            .map_err(|err| ApiError::Protocol(format!("malformed <description>: {err}")))?;
                        let text = quick_xml::escape::unescape(&raw).map_err(|err| {
                            ApiError::Protocol(format!("invalid entity in <description>: {err}"))
                        })?;
                        if let Some(user) = user.as_mut() {
                            user.description = text.trim().to_owned();
                        }
                        depth = depth.saturating_sub(1);
                    }
                    _ => {}
                }
            }
            Ok(quick_xml::events::Event::Empty(e)) => {
                let name = e.name().as_ref().to_owned();
                if root.is_none() {
                    check_osm_root(&mut root, &name)?;
                    root_closed = true;
                }
                if name == "user" && user.is_none() {
                    user = Some(user_from_attrs(&attrs_of(&e), fallback_uid));
                }
            }
            Ok(quick_xml::events::Event::End(_)) => {
                if depth == 1 {
                    root_closed = true;
                }
                depth = depth.saturating_sub(1);
            }
            Ok(quick_xml::events::Event::Eof) => break,
            Err(err) => return Err(ApiError::Protocol(format!("malformed user XML: {err}"))),
            _ => {}
        }
    }
    if root.is_none() {
        return Err(ApiError::Protocol("user response has no <osm> root".into()));
    }
    if !root_closed {
        return Err(ApiError::Protocol("truncated user XML: missing </osm>".into()));
    }
    user.ok_or_else(|| ApiError::Protocol("user response has no <user>".into()))
}

fn parse_conflict(xml: &str) -> ConflictReport {
    let mut report = ConflictReport::default();
    if !xml.trim_start().starts_with('<') {
        // Plain-text error body (the common shape of OSM 409s).
        report.message = xml.trim().to_owned();
        parse_conflict_text(&report.message.clone(), &mut report);
        return report;
    }
    let mut reader = quick_xml::Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    loop {
        match reader.read_event() {
            Ok(quick_xml::events::Event::Start(e)) => {
                let name = e.name().as_ref().to_owned();
                if name == "error" || name == "message" {
                    if let Ok(text) = reader.read_text(e.name()) {
                        report.message.push_str(&text);
                    }
                    continue;
                }
                let Some(kind) = kind_of(&name) else { continue };
                let attrs = attrs_of(&e);
                let old_id = attr(&attrs, "old_id").and_then(|s| s.parse::<i64>().ok());
                let new_id = attr(&attrs, "new_id").and_then(|s| s.parse::<i64>().ok());
                let version = attr(&attrs, "version").and_then(|s| s.parse::<i32>().ok());
                if let Some(old_id) = old_id {
                    report.objects.push(ConflictObject {
                        kind,
                        old_id: ObjectId(old_id),
                        new_id: new_id.map(ObjectId),
                        version,
                    });
                }
            }
            Ok(quick_xml::events::Event::Empty(e)) => {
                let name = e.name().as_ref().to_owned();
                let Some(kind) = kind_of(&name) else { continue };
                let attrs = attrs_of(&e);
                let old_id = attr(&attrs, "old_id").and_then(|s| s.parse::<i64>().ok());
                let new_id = attr(&attrs, "new_id").and_then(|s| s.parse::<i64>().ok());
                let version = attr(&attrs, "version").and_then(|s| s.parse::<i32>().ok());
                if let Some(old_id) = old_id {
                    report.objects.push(ConflictObject {
                        kind,
                        old_id: ObjectId(old_id),
                        new_id: new_id.map(ObjectId),
                        version,
                    });
                }
            }
            Ok(quick_xml::events::Event::Eof) | Err(_) => break,
            _ => {}
        }
    }
    // OSM 409s often contain only plain text ("Version mismatch: Provided 2, server
    // had: 3 of Node 123") with no structured <node old_id=...>. Without this fallback the
    // conflict UI would always show 0 objects and KeepLocal would have nothing to act on.
    if report.objects.is_empty() && !report.message.is_empty() {
        let message = report.message.clone();
        parse_conflict_text(&message, &mut report);
    }
    report
}

fn parse_conflict_text(text: &str, report: &mut ConflictReport) {
    for (word, kind) in [
        ("Node", ObjectKind::Node),
        ("Way", ObjectKind::Way),
        ("Relation", ObjectKind::Relation),
    ] {
        for (pos, _) in text.match_indices(word) {
            let after = &text[pos + word.len()..];
            let digits: String = after
                .trim_start()
                .chars()
                .take_while(|ch| ch.is_ascii_digit())
                .collect();
            let Ok(id) = digits.parse::<i64>() else {
                continue;
            };
            // Look for "server had: N" in the nearby text.
            let window_start = pos.saturating_sub(200);
            let window_end = (pos + 240).min(text.len());
            let window = text.get(window_start..window_end).unwrap_or("");
            let version = window.split("server had:").nth(1).and_then(|tail| {
                tail.trim_start()
                    .chars()
                    .take_while(|ch| ch.is_ascii_digit())
                    .collect::<String>()
                    .parse::<i32>()
                    .ok()
            });
            if !report
                .objects
                .iter()
                .any(|object| object.kind == kind && object.old_id == ObjectId(id))
            {
                report.objects.push(ConflictObject {
                    kind,
                    old_id: ObjectId(id),
                    new_id: None,
                    version,
                });
            }
        }
    }
}

/// Blocking reqwest transport (feature `http`).
#[cfg(feature = "http")]
pub struct ReqwestTransport {
    client: reqwest::blocking::Client,
}

#[cfg(feature = "http")]
impl ReqwestTransport {
    /// Creates an HTTP transport.
    pub fn new() -> Result<Self, TransportError> {
        let client = reqwest::blocking::Client::builder()
            // Uploads and token requests are non-idempotent POSTs: never follow redirects automatically
            // (301/302 would rewrite POST into GET, 307/308 would replay the body).
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(std::time::Duration::from_secs(10))
            .timeout(std::time::Duration::from_secs(180))
            .user_agent(format!("NEO/{}", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|err| TransportError::Other(err.to_string()))?;
        Ok(Self { client })
    }
}

#[cfg(feature = "http")]
impl Default for ReqwestTransport {
    fn default() -> Self {
        // Construction failure means the TLS backend is unavailable; return a client that reports errors
        // explicitly instead of panicking inside library code.
        Self::new().unwrap_or_else(|_| Self {
            client: reqwest::blocking::Client::new(),
        })
    }
}

#[cfg(feature = "http")]
impl Transport for ReqwestTransport {
    fn send(&self, request: HttpRequest) -> Result<HttpResponse, TransportError> {
        let method = reqwest::Method::from_bytes(request.method.as_bytes())
            .map_err(|err| TransportError::Other(err.to_string()))?;
        let mut builder = self.client.request(method, &request.url);
        for (key, value) in &request.headers {
            builder = builder.header(key, value);
        }
        if !request.body.is_empty() {
            builder = builder.body(request.body);
        }
        let response = builder
            .send()
            .map_err(|err| TransportError::Other(err.to_string()))?;
        let status = response.status().as_u16();
        let body = response
            .text()
            .map_err(|err| TransportError::Other(err.to_string()))?;
        Ok(HttpResponse { status, body })
    }
}

/// Test mock transport: returns responses from a queue and records requests.
#[derive(Default)]
pub struct MockTransport {
    responses: Mutex<VecDeque<HttpResponse>>,
    requests: Mutex<Vec<HttpRequest>>,
}

impl MockTransport {
    /// New mock.
    pub fn new() -> Self {
        Self::default()
    }

    /// Appends one response.
    pub fn push(&self, status: u16, body: impl Into<String>) {
        self.responses.lock().unwrap().push_back(HttpResponse {
            status,
            body: body.into(),
        });
    }

    /// Requests received so far.
    pub fn requests(&self) -> Vec<HttpRequest> {
        self.requests.lock().unwrap().clone()
    }
}

impl Transport for MockTransport {
    fn send(&self, request: HttpRequest) -> Result<HttpResponse, TransportError> {
        self.requests.lock().unwrap().push(request);
        self.responses
            .lock()
            .unwrap()
            .pop_front()
            .ok_or_else(|| TransportError::Other("mock: no response".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_notes_geojson() {
        let text = r#"{"type":"FeatureCollection","features":[{"type":"Feature","geometry":{"type":"Point","coordinates":[8.54,47.37]},"properties":{"id":42,"status":"open","comments":[{"date":"2026-01-01T00:00:00Z","user":"alice","text":"hello","action":"opened"}]}}]}"#;
        let notes = parse_notes(text).unwrap();
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].id, 42);
        assert!((notes[0].lat - 47.37).abs() < 1e-9);
        assert_eq!(notes[0].comments[0].user, "alice");
    }

    #[test]
    fn create_note_posts_form_body() {
        let mock = MockTransport::new();
        mock.push(
            200,
            r#"{"type":"Feature","geometry":{"type":"Point","coordinates":[8.0,47.0]},"properties":{"id":7,"status":"open","comments":[]}}"#,
        );
        let client = ApiClient::new("https://api.example", Some("token".into()), mock);
        let note = client.create_note(47.0, 8.0, "a & b").unwrap();
        assert_eq!(note.id, 7);
        let requests = client.transport.requests();
        assert_eq!(requests.len(), 1);
        assert!(requests[0].url.ends_with("/api/0.6/notes.json"));
        assert!(requests[0].body.contains("text=a+%26+b"));
    }

    const CHANGESET_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<osm version="0.6" generator="openstreetmap-cgimap 2.0.0">
  <changeset id="123" created_at="2026-01-02T03:04:05Z" closed_at="2026-01-02T04:05:06Z"
             open="false" comments_count="3" changes_count="7"
             min_lat="47.0" min_lon="8.0" max_lat="47.5" max_lon="8.5"
             uid="42" user="alice &amp; bob">
    <tag k="comment" v="fix &amp; tidy"/>
    <tag k="created_by" v="NEO/0.1"/>
    <discussion>
      <comment date="2026-01-02T05:00:00Z" uid="7">
        <text>looks good</text>
        <tag k="discussion_tag" v="ignored"/>
      </comment>
    </discussion>
  </changeset>
</osm>"#;

    #[test]
    fn parses_changeset_xml() {
        let mock = MockTransport::new();
        mock.push(200, CHANGESET_XML);
        let client = ApiClient::new("https://api.example", None, mock);
        let info = client.changeset(123).unwrap();
        assert_eq!(info.id, 123);
        assert_eq!(info.uid, Some(42));
        assert_eq!(info.user, "alice & bob");
        assert_eq!(info.created_at, "2026-01-02T03:04:05Z");
        assert_eq!(info.closed_at.as_deref(), Some("2026-01-02T04:05:06Z"));
        assert!(!info.open);
        assert_eq!(info.comment, "fix & tidy");
        assert!(
            info.tags
                .contains(&("created_by".to_owned(), "NEO/0.1".to_owned()))
        );
        assert!(
            !info.tags.iter().any(|(key, _)| key == "discussion_tag"),
            "discussion tags must be ignored"
        );
        assert_eq!(info.changes_count, Some(7));
        // bbox order matches download_map: (min_lon, min_lat, max_lon, max_lat).
        assert_eq!(info.bbox, Some((8.0, 47.0, 8.5, 47.5)));
        let requests = client.transport.requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].method, "GET");
        assert_eq!(requests[0].url, "https://api.example/api/0.6/changeset/123");
        assert!(requests[0].body.is_empty());
    }

    #[test]
    fn create_changeset_is_a_put() {
        let mock = MockTransport::new();
        mock.push(200, "42");
        let client = ApiClient::new("https://api.openstreetmap.org", Some("token".into()), mock);
        assert_eq!(client.create_changeset("edit", "NEO/0.1").unwrap(), 42);
        let requests = client.transport.requests();
        assert_eq!(requests[0].method, "PUT");
        assert_eq!(
            requests[0].url,
            "https://api.openstreetmap.org/api/0.6/changeset/create"
        );
    }

    #[test]
    fn parses_changeset_missing_optional_attributes() {
        let mock = MockTransport::new();
        mock.push(200, r#"<osm><changeset id="9"/></osm>"#);
        let client = ApiClient::new("https://api.example", None, mock);
        let info = client.changeset(9).unwrap();
        assert_eq!(info.id, 9);
        assert_eq!(info.uid, None);
        assert_eq!(info.user, "");
        assert_eq!(info.created_at, "");
        assert_eq!(info.closed_at, None);
        // No open attribute and no closed_at: infer "still open".
        assert!(info.open);
        assert_eq!(info.comment, "");
        assert!(info.tags.is_empty());
        assert_eq!(info.changes_count, None);
        assert_eq!(info.bbox, None);
    }

    #[test]
    fn parses_user_xml() {
        let mock = MockTransport::new();
        mock.push(
            200,
            r#"<osm version="0.6" generator="openstreetmap-cgimap 2.0.0">
  <user id="42" display_name="alice" account_created="2008-01-01T00:00:00Z">
    <description>Mapping &amp; more</description>
    <languages><lang>en</lang></languages>
  </user>
</osm>"#,
        );
        let client = ApiClient::new("https://api.example", None, mock);
        let info = client.user(42).unwrap();
        assert_eq!(info.id, 42);
        assert_eq!(info.display_name, "alice");
        assert_eq!(info.description, "Mapping & more");
        assert_eq!(info.account_created, "2008-01-01T00:00:00Z");
        let requests = client.transport.requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].method, "GET");
        assert_eq!(requests[0].url, "https://api.example/api/0.6/user/42");
    }

    #[test]
    fn overpass_posts_form_encoded_query() {
        let mock = MockTransport::new();
        mock.push(200, r#"<?xml version="1.0"?><osm version="0.6"/>"#);
        let xml = download_overpass(
            &mock,
            "https://overpass-api.de/api/interpreter",
            r#"[out:xml];node["amenity"="cafe"](1,2,3,4);out;"#,
        )
        .unwrap();
        assert!(xml.contains("<osm"));
        let requests = mock.requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].method, "POST");
        assert_eq!(requests[0].url, "https://overpass-api.de/api/interpreter");
        assert!(requests[0].body.contains("data="));
        assert!(requests[0].body.contains("amenity"));
    }

    #[test]
    fn malformed_xml_is_protocol_error() {
        let mock = MockTransport::new();
        mock.push(200, r#"<osm><changeset id="1"></not-osm></osm>"#);
        let client = ApiClient::new("https://api.example", None, mock);
        assert!(matches!(client.changeset(1), Err(ApiError::Protocol(_))));

        let mock = MockTransport::new();
        mock.push(200, "<osm><user id=\"42\">");
        let client = ApiClient::new("https://api.example", None, mock);
        assert!(matches!(client.user(42), Err(ApiError::Protocol(_))));
    }

    #[test]
    fn wrong_root_is_protocol_error() {
        let mock = MockTransport::new();
        mock.push(200, r#"<note><changeset id="1"/></note>"#);
        let client = ApiClient::new("https://api.example", None, mock);
        assert!(matches!(client.changeset(1), Err(ApiError::Protocol(_))));

        let mock = MockTransport::new();
        mock.push(200, r#"<note><user id="42" display_name="a"/></note>"#);
        let client = ApiClient::new("https://api.example", None, mock);
        assert!(matches!(client.user(42), Err(ApiError::Protocol(_))));
    }

    #[test]
    fn empty_document_is_protocol_error() {
        let mock = MockTransport::new();
        mock.push(200, "");
        let client = ApiClient::new("https://api.example", None, mock);
        assert!(matches!(client.changeset(1), Err(ApiError::Protocol(_))));

        let mock = MockTransport::new();
        mock.push(200, "");
        let client = ApiClient::new("https://api.example", None, mock);
        assert!(matches!(client.user(1), Err(ApiError::Protocol(_))));
    }
}
