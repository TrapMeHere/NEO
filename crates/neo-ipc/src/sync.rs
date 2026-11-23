//! 同步旁路消息。

use serde::{Deserialize, Serialize};

/// 同步旁路协议版本。
pub const PROTOCOL_VERSION: u32 = super::PROTOCOL_VERSION;

/// 账户摘要（不含令牌）。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct AccountSummary {
    /// OSM 用户 id。
    pub id: String,
    /// 显示名。
    pub display_name: String,
}

/// 上传 diff 条目。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct DiffEntryDto {
    /// `node` / `way` / `relation`。
    pub kind: String,
    /// 本地 id。
    pub old_id: i64,
    /// 服务端 id。
    pub new_id: Option<i64>,
    /// 新版本。
    pub new_version: Option<i32>,
}

/// 冲突对象。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConflictObjectDto {
    /// `node` / `way` / `relation`。
    pub kind: String,
    /// 本地 id。
    pub old_id: i64,
    /// 服务端 id。
    pub new_id: Option<i64>,
    /// 服务端版本。
    pub version: Option<i32>,
}

/// 409 冲突。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ConflictDto {
    /// 服务端消息。
    pub message: String,
    /// 冲突对象。
    pub objects: Vec<ConflictObjectDto>,
}

/// Note 评论。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct NoteCommentDto {
    /// 时间。
    pub date: String,
    /// 用户。
    pub user: String,
    /// 文本。
    pub text: String,
}

/// Note。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct NoteDto {
    /// id。
    pub id: i64,
    /// 纬度。
    pub lat: f64,
    /// 经度。
    pub lon: f64,
    /// 状态。
    pub status: String,
    /// 评论。
    pub comments: Vec<NoteCommentDto>,
}

/// Notes 动作。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum NotesAction {
    /// 当前视口列表。
    Refresh,
    /// 新建。
    Create {
        /// 纬度。
        lat: f64,
        /// 经度。
        lon: f64,
        /// 文本。
        text: String,
    },
    /// 评论。
    Comment {
        /// note id。
        id: i64,
        /// 文本。
        text: String,
    },
    /// 关闭。
    Close {
        /// note id。
        id: i64,
        /// 文本。
        text: String,
    },
}

/// 编辑器 → 同步旁路。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum SyncRequest {
    /// 握手。
    Hello {
        /// 编辑器侧协议版本。
        version: u32,
    },
    /// 列出账户。
    ListAccounts,
    /// 开始 OAuth。
    StartOauth {
        /// 客户端 id（空则用内置）。
        client_id: String,
        /// 回调地址（空则用内置）。
        redirect_uri: String,
    },
    /// 查询 OAuth 进度。
    PollOauth,
    /// 选择当前账户。
    SelectAccount {
        /// 账户 id。
        id: String,
    },
    /// 删除账户。
    RemoveAccount {
        /// 账户 id。
        id: String,
    },
    /// 退出当前账户。
    ClearCurrent,
    /// 写回 OAuth 设置字段。
    SaveSettings {
        /// 客户端 id。
        client_id: String,
        /// 回调地址。
        redirect_uri: String,
    },
    /// 下载地图 XML。
    Download {
        /// API 根地址。
        api_base: String,
        /// `(min_lon, min_lat, max_lon, max_lat)`。
        bbox: [f64; 4],
    },
    /// 通过 Overpass QL 下载 OSM XML。
    OverpassDownload {
        /// Overpass interpreter 端点。
        endpoint: String,
        /// Overpass QL 查询正文。
        query: String,
    },
    /// 上传 osmChange 分片。
    Upload {
        /// API 根地址。
        api_base: String,
        /// changeset 注释。
        comment: String,
        /// 已写好的 osc 文本。
        oscs: Vec<String>,
    },
    /// Notes。
    Notes {
        /// API 根地址。
        api_base: String,
        /// `(min_lon, min_lat, max_lon, max_lat)`。
        bbox: [f64; 4],
        /// 动作。
        action: NotesAction,
    },
    /// 读取 changeset 元数据。
    Changeset {
        /// API 根地址。
        api_base: String,
        /// changeset id。
        id: i64,
    },
    /// 下载单个对象 XML。
    DownloadObject {
        /// API 根地址。
        api_base: String,
        /// `node` / `way` / `relation`。
        kind: String,
        /// 对象 id。
        id: i64,
    },
}

/// 同步旁路 → 编辑器。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum SyncResponse {
    /// 握手结果。
    Hello {
        /// 旁路协议版本。
        version: u32,
        /// 是否兼容。
        ok: bool,
    },
    /// 账户列表。
    Accounts {
        /// 当前账户 id。
        current: Option<String>,
        /// 账户摘要。
        accounts: Vec<AccountSummary>,
        /// 客户端 id。
        client_id: String,
        /// 回调地址。
        redirect_uri: String,
    },
    /// OAuth 已启动。
    OauthStarted,
    /// OAuth 仍在等回调。
    OauthPending,
    /// OAuth 成功。
    OauthDone {
        /// 新登录账户。
        account: AccountSummary,
    },
    /// OAuth 失败。
    OauthFailed {
        /// 错误。
        error: String,
    },
    /// 下载成功。
    Downloaded {
        /// OSM XML。
        xml: String,
    },
    /// 上传成功。
    Uploaded {
        /// diff 条目。
        entries: Vec<DiffEntryDto>,
    },
    /// 部分上传成功。
    UploadedPartial {
        /// 已成功条目。
        entries: Vec<DiffEntryDto>,
        /// 后续错误。
        error: String,
    },
    /// 409 冲突。
    Conflict {
        /// 冲突报告。
        report: ConflictDto,
    },
    /// Notes 列表。
    NotesList {
        /// notes。
        notes: Vec<NoteDto>,
    },
    /// 单条 note。
    NoteOne {
        /// note。
        note: NoteDto,
    },
    /// Changeset 元数据。
    ChangesetInfo {
        /// id。
        id: i64,
        /// 用户。
        user: String,
        /// 创建时间。
        created_at: String,
        /// 关闭时间。
        closed_at: String,
        /// 是否仍打开。
        open: bool,
        /// 注释。
        comment: String,
        /// 修改计数（文本）。
        changes: String,
        /// 标签。
        tags: Vec<(String, String)>,
    },
    /// 通用成功。
    Ok,
    /// 错误。
    Error {
        /// 消息。
        message: String,
    },
}

/// 同步信封。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct SyncEnvelope<T> {
    /// 协议版本。
    pub v: u32,
    /// 载荷。
    #[serde(flatten)]
    pub body: T,
}

/// 同步会话：一行请求、一行应答。
pub struct SyncSession {
    inner: super::client::SidecarProcess,
    ready: bool,
}

impl SyncSession {
    /// 启动并握手。
    pub fn open() -> Result<Self, super::client::SidecarError> {
        let inner = super::client::SidecarProcess::spawn("neo-sync")?;
        let mut session = Self { inner, ready: false };
        session.handshake()?;
        Ok(session)
    }

    fn handshake(&mut self) -> Result<(), super::client::SidecarError> {
        match self.call(SyncRequest::Hello {
            version: PROTOCOL_VERSION,
        })? {
            SyncResponse::Hello { ok: true, .. } => {
                self.ready = true;
                Ok(())
            }
            SyncResponse::Hello { version, ok: false } => Err(super::client::SidecarError::Version {
                expected: PROTOCOL_VERSION,
                got: version,
            }),
            other => Err(super::client::SidecarError::Protocol(format!(
                "unexpected hello: {other:?}"
            ))),
        }
    }

    /// 是否可用。
    pub fn ready(&self) -> bool {
        self.ready
    }

    /// 发请求并读应答。
    pub fn call(&mut self, request: SyncRequest) -> Result<SyncResponse, super::client::SidecarError> {
        let envelope = SyncEnvelope {
            v: PROTOCOL_VERSION,
            body: request,
        };
        let line = serde_json::to_string(&envelope)
            .map_err(|err| super::client::SidecarError::Protocol(err.to_string()))?;
        let reply = self.inner.exchange(&line)?;
        let parsed: SyncEnvelope<SyncResponse> = serde_json::from_str(&reply)
            .map_err(|err| super::client::SidecarError::Protocol(format!("{err}: {reply}")))?;
        if parsed.v != PROTOCOL_VERSION {
            return Err(super::client::SidecarError::Version {
                expected: PROTOCOL_VERSION,
                got: parsed.v,
            });
        }
        Ok(parsed.body)
    }
}
