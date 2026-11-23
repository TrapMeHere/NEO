//! # neo-api — OSM API 0.6 客户端
//!
//! 这一层只负责 **HTTP 语义与 XML 协议**：changeset 创建/关闭、diff 上传、区域下载、
//! 409 冲突解析。传输层通过 [`Transport`] 注入，因此单元测试不需要网络，应用层可以
//! 选择 reqwest（feature `http`）或自己的 mock/代理。
//!
//! 冲突的**业务合并**（MergeCase）属于 `neo-editor`；本 crate 只把 409 的 XML
//! 翻译成 [`ConflictReport`]（对象 id/version + 服务端消息），不依赖 editor（见 docs/06 §2.6）。

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod oauth;

use std::collections::VecDeque;
use std::sync::Mutex;

use neo_core::{ObjectId, ObjectKind};

/// 一次 HTTP 请求（与具体客户端解耦）。
#[derive(Clone, PartialEq, Eq)]
pub struct HttpRequest {
    /// 方法（GET/POST/PUT）。
    pub method: String,
    /// 绝对 URL。
    pub url: String,
    /// 请求头。
    pub headers: Vec<(String, String)>,
    /// 请求体。
    pub body: String,
}

/// 手写 Debug：请求头里有 `Authorization: Bearer <token>`，不能让 `{:?}`
/// 把长期凭据打进日志/测试输出。
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

/// 一次 HTTP 响应。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HttpResponse {
    /// HTTP 状态码。
    pub status: u16,
    /// 响应体。
    pub body: String,
}

/// 传输层错误。
#[derive(Debug, thiserror::Error)]
pub enum TransportError {
    /// 网络/客户端错误。
    #[error("transport: {0}")]
    Other(String),
}

/// 可注入的 HTTP 传输层。
pub trait Transport: Send + Sync {
    /// 发送请求。
    fn send(&self, request: HttpRequest) -> Result<HttpResponse, TransportError>;
}

/// API 错误。
#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    /// 传输层失败。
    #[error(transparent)]
    Transport(#[from] TransportError),
    /// 认证失败（401/403）。
    #[error("authentication failed: {0}")]
    Unauthorized(String),
    /// 409：上传冲突。
    #[error("conflict: {0:?}")]
    Conflict(ConflictReport),
    /// 412：前置条件失败（版本/changeset 状态）。
    #[error("precondition failed: {0}")]
    Precondition(String),
    /// 其它 HTTP 错误。
    #[error("http {status}: {body}")]
    Http {
        /// 状态码。
        status: u16,
        /// 响应体。
        body: String,
    },
    /// 响应格式错误。
    #[error("protocol: {0}")]
    Protocol(String),
}

/// 冲突中的一个对象。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConflictObject {
    /// 对象种类。
    pub kind: ObjectKind,
    /// 本地（旧）id（新建对象为负）。
    pub old_id: ObjectId,
    /// 服务端新 id（若服务端创建成功）。
    pub new_id: Option<ObjectId>,
    /// 服务端当前版本（若给出）。
    pub version: Option<i32>,
}

/// 409 冲突报告。
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct ConflictReport {
    /// 服务端错误消息。
    pub message: String,
    /// 冲突对象。
    pub objects: Vec<ConflictObject>,
}

impl ConflictReport {
    /// 是否包含某些对象。
    pub fn contains(&self, kind: ObjectKind, id: ObjectId) -> bool {
        self.objects
            .iter()
            .any(|o| o.kind == kind && (o.old_id == id || o.new_id == Some(id)))
    }
}

/// 上传返回的 diffResult 条目。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffEntry {
    /// 对象种类。
    pub kind: ObjectKind,
    /// 本地 id。
    pub old_id: ObjectId,
    /// 服务端 id。
    pub new_id: Option<ObjectId>,
    /// 新版本。
    pub new_version: Option<i32>,
}

/// 上传结果。
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct UploadResult {
    /// 逐对象结果。
    pub entries: Vec<DiffEntry>,
}

/// 一条 note 评论。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NoteComment {
    /// 时间（服务端原样字符串）。
    pub date: String,
    /// 用户名（匿名可为空）。
    pub user: String,
    /// 文本。
    pub text: String,
    /// 动作：`opened`/`commented`/`closed` 等。
    pub action: String,
}

/// 一条 OSM note。
#[derive(Clone, Debug, PartialEq)]
pub struct Note {
    /// note id。
    pub id: i64,
    /// 纬度。
    pub lat: f64,
    /// 经度。
    pub lon: f64,
    /// `open` 或 `closed`。
    pub status: String,
    /// 评论（第一条是开启者）。
    pub comments: Vec<NoteComment>,
}

/// changeset 历史（只读元数据）。
///
/// `bbox` 的元组顺序与 [`ApiClient::download_map`] 一致：`(min_lon, min_lat,
/// max_lon, max_lat)`。changeset 没有改动过对象、或服务端省略 bbox 属性时为 `None`。
#[derive(Clone, Debug, PartialEq)]
pub struct ChangesetInfo {
    /// changeset id。
    pub id: i64,
    /// 作者 uid（匿名/导入 changeset 可能缺失）。
    pub uid: Option<i64>,
    /// 作者显示名（缺失时为空串）。
    pub user: String,
    /// 创建时间（服务端原样字符串，ISO 8601）。
    pub created_at: String,
    /// 关闭时间（仍打开时为 `None`）。
    pub closed_at: Option<String>,
    /// 是否仍打开（缺少 `open` 属性时按 `closed_at` 是否为空推断）。
    pub open: bool,
    /// `comment` 标签的值（没有 `comment` 标签时为空串）。
    pub comment: String,
    /// 全部标签（按出现顺序；含 `comment`）。
    pub tags: Vec<(String, String)>,
    /// 改动对象数：优先 `changes_count`，缺失时回退 `comments_count`。
    pub changes_count: Option<usize>,
    /// 改动范围 `(min_lon, min_lat, max_lon, max_lat)`。
    pub bbox: Option<(f64, f64, f64, f64)>,
}

/// OSM 用户的公开信息（只读）。
///
/// 与 [`oauth::UserInfo`] 不同：这里读取的是 `/api/0.6/user/{uid}` 的公开资料，
/// 额外包含 `<description>` 与注册时间；`oauth` 那份来自 `/user/details`。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UserInfo {
    /// 数字 uid。
    pub id: i64,
    /// 显示名。
    pub display_name: String,
    /// 个人描述（`<description>` 的文本，可能为空）。
    pub description: String,
    /// 注册时间（服务端原样字符串）。
    pub account_created: String,
}

/// OSM API 客户端。
pub struct ApiClient<T: Transport> {
    /// API 根（例如 `https://api.openstreetmap.org`）。
    pub base_url: String,
    /// OAuth2 access token（可选，匿名只能读）。
    pub token: Option<String>,
    /// 传输层。
    pub transport: T,
}

impl<T: Transport> ApiClient<T> {
    /// 创建客户端。
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

    /// 创建 changeset，返回 changeset id。
    ///
    /// OSM 只注册了 `PUT /api/0.6/changeset/create`。用 POST 会落到网站的 HTML 404。
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

    /// 上传 osmChange（调用方用 `neo-io` 生成 XML）。
    pub fn upload_diff(&self, changeset: i64, osc: &str) -> Result<UploadResult, ApiError> {
        let text = self.request(
            "POST",
            &format!("/api/0.6/changeset/{changeset}/upload"),
            osc,
            "text/xml",
        )?;
        Ok(parse_diff_result(&text))
    }

    /// 关闭 changeset。
    pub fn close_changeset(&self, changeset: i64) -> Result<(), ApiError> {
        self.request(
            "PUT",
            &format!("/api/0.6/changeset/{changeset}/close"),
            "",
            "text/xml",
        )?;
        Ok(())
    }

    /// 读取 changeset 历史（只读）：`GET /api/0.6/changeset/{id}`。
    ///
    /// 响应是 `<osm><changeset ...>` XML；`<discussion>` 与注释正文被忽略，
    /// 只取元数据与 `<tag>`。根元素不是 `<osm>`、或 XML 结构损坏时返回
    /// [`ApiError::Protocol`]。
    pub fn changeset(&self, id: i64) -> Result<ChangesetInfo, ApiError> {
        let text = self.request("GET", &format!("/api/0.6/changeset/{id}"), "", "text/xml")?;
        parse_changeset(&text, id)
    }

    /// 读取用户公开信息（只读）：`GET /api/0.6/user/{uid}`。
    ///
    /// 不需要 token；`<description>` 中的 XML 实体（`&amp;` 等）会被反转义。
    pub fn user(&self, uid: i64) -> Result<UserInfo, ApiError> {
        let text = self.request("GET", &format!("/api/0.6/user/{uid}"), "", "text/xml")?;
        parse_user(&text, uid)
    }

    /// 下载单个对象（冲突时“采用远端”用）。
    pub fn download_object(&self, kind: ObjectKind, id: ObjectId) -> Result<String, ApiError> {
        let name = match kind {
            ObjectKind::Node => "node",
            ObjectKind::Way => "way",
            ObjectKind::Relation => "relation",
            ObjectKind::Changeset => "changeset",
        };
        self.request("GET", &format!("/api/0.6/{name}/{}", id.0), "", "text/xml")
    }

    /// 下载 bbox 区域（返回 OSM XML；调用方用 `neo-io` 解析）。
    pub fn download_map(&self, bbox: (f64, f64, f64, f64)) -> Result<String, ApiError> {
        let (min_lon, min_lat, max_lon, max_lat) = bbox;
        let path = format!("/api/0.6/map?bbox={min_lon:.7},{min_lat:.7},{max_lon:.7},{max_lat:.7}");
        self.request("GET", &path, "", "text/xml")
    }

    /// 通过 Overpass QL 下载 OSM XML（`endpoint` 为完整 interpreter URL）。
    pub fn download_overpass(&self, endpoint: &str, query: &str) -> Result<String, ApiError> {
        download_overpass(&self.transport, endpoint, query)
    }

    /// 读取 bbox 内的 notes（默认最多 100 条，含最近 7 天关闭的）。
    pub fn notes(&self, bbox: (f64, f64, f64, f64)) -> Result<Vec<Note>, ApiError> {
        let (min_lon, min_lat, max_lon, max_lat) = bbox;
        let path = format!(
            "/api/0.6/notes.json?bbox={min_lon:.7},{min_lat:.7},{max_lon:.7},{max_lat:.7}&limit=100&closed=7"
        );
        let text = self.request("GET", &path, "", "application/json")?;
        parse_notes(&text)
    }

    /// 新建 note（需要登录 token）。
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

    /// 追加评论（需要登录 token）。
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

    /// 关闭 note（需要登录 token）。
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

/// 向 Overpass interpreter 提交 QL，返回 OSM XML。
///
/// `endpoint` 一般为 `https://overpass-api.de/api/interpreter`。
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
    // 新建/评论接口可能直接返回单个 feature 或 {note: ...}。
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

/// 校验文档根元素是 `<osm>`（OSM API 0.6 的所有 XML 响应都以它为根）。
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

/// 从 `<changeset>` 的属性构造 [`ChangesetInfo`]（不含子元素 `<tag>`）。
///
/// `fallback_id` 用于响应缺少 `id` 属性时回退到调用方请求的 id。
fn changeset_from_attrs(attrs: &[(String, String)], fallback_id: i64) -> ChangesetInfo {
    let closed_at = attr(attrs, "closed_at").filter(|value| !value.is_empty());
    // 老服务端/简化响应可能没有 open 属性：有 closed_at 就是已关闭。
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
    // 四个角必须齐全才算有效 bbox；顺序与 download_map 一致（lon 在前）。
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

/// 把 changeset 的直接子 `<tag k="..." v="...">` 追加到 [`ChangesetInfo`]。
fn push_changeset_tag(info: &mut Option<ChangesetInfo>, attrs: &[(String, String)]) {
    if let (Some(key), Some(value), Some(info)) = (attr(attrs, "k"), attr(attrs, "v"), info.as_mut()) {
        info.tags.push((key, value));
    }
}

/// 解析 `/api/0.6/changeset/{id}` 的 XML。
fn parse_changeset(xml: &str, fallback_id: i64) -> Result<ChangesetInfo, ApiError> {
    let mut reader = quick_xml::Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut root: Option<String> = None;
    let mut root_closed = false;
    let mut depth = 0usize;
    // `<changeset>` / `<discussion>` 所在深度：只有 changeset 的直接子 `<tag>`
    // 才算元数据，discussion 里的 `<tag>` 一律忽略。
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
                    // `<osm/>` 自身闭合，不会有 End 事件。
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

/// 从 `<user>` 的属性构造 [`UserInfo`]（`description` 由子元素填充）。
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

/// 解析 `/api/0.6/user/{uid}` 的 XML。
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
                        // read_text 会消费掉 </description>，所以要手动补一次 depth。
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
        // 纯文本错误体（OSM 的 409 常见形态）。
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
    // OSM 的 409 往往只有一段纯文本（"Version mismatch: Provided 2, server
    // had: 3 of Node 123"），没有结构化的 <node old_id=...>。没有这条回退，
    // 冲突 UI 永远显示 0 条、KeepLocal 无从下手。
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
            // 在附近文本里找 "server had: N"。
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

/// reqwest 阻塞传输（feature `http`）。
#[cfg(feature = "http")]
pub struct ReqwestTransport {
    client: reqwest::blocking::Client,
}

#[cfg(feature = "http")]
impl ReqwestTransport {
    /// 创建 HTTP 传输。
    pub fn new() -> Result<Self, TransportError> {
        let client = reqwest::blocking::Client::builder()
            // 上传/token 都是非幂等 POST：绝不自动跟随重定向（301/302 会把 POST
            // 改写成 GET，307/308 会重放 body）。
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
        // 构造失败说明 TLS 后端不可用；此时返回一个会明确报错的客户端，
        // 而不是在库代码里 panic。
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

/// 测试用 mock 传输：按队列返回响应，并记录请求。
#[derive(Default)]
pub struct MockTransport {
    responses: Mutex<VecDeque<HttpResponse>>,
    requests: Mutex<Vec<HttpRequest>>,
}

impl MockTransport {
    /// 新 mock。
    pub fn new() -> Self {
        Self::default()
    }

    /// 追加一个响应。
    pub fn push(&self, status: u16, body: impl Into<String>) {
        self.responses.lock().unwrap().push_back(HttpResponse {
            status,
            body: body.into(),
        });
    }

    /// 已收到的请求。
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
        // bbox 顺序与 download_map 一致：(min_lon, min_lat, max_lon, max_lat)。
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
        // 没有 open 属性且没有 closed_at：按“仍打开”推断。
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
