//! Sync sidecar messages.

use serde::{Deserialize, Serialize};

/// Sync sidecar protocol version.
pub const PROTOCOL_VERSION: u32 = super::PROTOCOL_VERSION;

/// Account summary (without tokens).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct AccountSummary {
    /// OSM user id.
    pub id: String,
    /// Display name.
    pub display_name: String,
}

/// Upload diff entry.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct DiffEntryDto {
    /// `node` / `way` / `relation`.
    pub kind: String,
    /// Local id.
    pub old_id: i64,
    /// Server id.
    pub new_id: Option<i64>,
    /// New version.
    pub new_version: Option<i32>,
}

/// Conflicting object.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConflictObjectDto {
    /// `node` / `way` / `relation`.
    pub kind: String,
    /// Local id.
    pub old_id: i64,
    /// Server id.
    pub new_id: Option<i64>,
    /// Server version.
    pub version: Option<i32>,
}

/// 409 conflict.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ConflictDto {
    /// Server message.
    pub message: String,
    /// Conflicting objects.
    pub objects: Vec<ConflictObjectDto>,
}

/// Note comment.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct NoteCommentDto {
    /// Time.
    pub date: String,
    /// User.
    pub user: String,
    /// Text.
    pub text: String,
}

/// Note.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct NoteDto {
    /// Id.
    pub id: i64,
    /// Latitude.
    pub lat: f64,
    /// Longitude.
    pub lon: f64,
    /// Status.
    pub status: String,
    /// Comments.
    pub comments: Vec<NoteCommentDto>,
}

/// Notes action.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum NotesAction {
    /// Refreshes the current viewport list.
    Refresh,
    /// Creates a new one.
    Create {
        /// Latitude.
        lat: f64,
        /// Longitude.
        lon: f64,
        /// Text.
        text: String,
    },
    /// Comments.
    Comment {
        /// Note id.
        id: i64,
        /// Text.
        text: String,
    },
    /// Closes.
    Close {
        /// Note id.
        id: i64,
        /// Text.
        text: String,
    },
}

/// Editor → sync sidecar.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum SyncRequest {
    /// Handshake.
    Hello {
        /// Editor-side protocol version.
        version: u32,
    },
    /// Lists accounts.
    ListAccounts,
    /// Starts OAuth.
    StartOauth {
        /// Client id (empty uses the built-in one).
        client_id: String,
        /// Callback URI (empty uses the built-in one).
        redirect_uri: String,
    },
    /// Polls OAuth progress.
    PollOauth,
    /// Selects the current account.
    SelectAccount {
        /// Account id.
        id: String,
    },
    /// Removes an account.
    RemoveAccount {
        /// Account id.
        id: String,
    },
    /// Logs out of the current account.
    ClearCurrent,
    /// Writes back the OAuth settings fields.
    SaveSettings {
        /// Client id.
        client_id: String,
        /// Callback URI.
        redirect_uri: String,
    },
    /// Downloads map XML.
    Download {
        /// API root URL.
        api_base: String,
        /// `(min_lon, min_lat, max_lon, max_lat)`.
        bbox: [f64; 4],
    },
    /// Downloads OSM XML through an Overpass QL query.
    OverpassDownload {
        /// Overpass interpreter endpoint.
        endpoint: String,
        /// Overpass QL query body.
        query: String,
    },
    /// Uploads osmChange chunks.
    Upload {
        /// API root URL.
        api_base: String,
        /// Changeset comment.
        comment: String,
        /// Already-serialized osc text.
        oscs: Vec<String>,
    },
    /// Notes.
    Notes {
        /// API root URL.
        api_base: String,
        /// `(min_lon, min_lat, max_lon, max_lat)`.
        bbox: [f64; 4],
        /// Action.
        action: NotesAction,
    },
    /// Reads changeset metadata.
    Changeset {
        /// API root URL.
        api_base: String,
        /// Changeset id.
        id: i64,
    },
    /// Downloads a single object's XML.
    DownloadObject {
        /// API root URL.
        api_base: String,
        /// `node` / `way` / `relation`.
        kind: String,
        /// Object id.
        id: i64,
    },
}

/// Sync sidecar → editor.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum SyncResponse {
    /// Handshake result.
    Hello {
        /// Sidecar protocol version.
        version: u32,
        /// Whether compatible.
        ok: bool,
    },
    /// Account list.
    Accounts {
        /// Current account id.
        current: Option<String>,
        /// Account summaries.
        accounts: Vec<AccountSummary>,
        /// Client id.
        client_id: String,
        /// Callback URI.
        redirect_uri: String,
    },
    /// OAuth has started.
    OauthStarted,
    /// OAuth is still waiting for the callback.
    OauthPending,
    /// OAuth succeeded.
    OauthDone {
        /// Newly logged-in account.
        account: AccountSummary,
    },
    /// OAuth failed.
    OauthFailed {
        /// Error.
        error: String,
    },
    /// Download succeeded.
    Downloaded {
        /// OSM XML.
        xml: String,
    },
    /// Upload succeeded.
    Uploaded {
        /// Diff entries.
        entries: Vec<DiffEntryDto>,
    },
    /// Partial upload succeeded.
    UploadedPartial {
        /// Succeeded entries.
        entries: Vec<DiffEntryDto>,
        /// Subsequent error.
        error: String,
    },
    /// 409 conflict.
    Conflict {
        /// Conflict report.
        report: ConflictDto,
    },
    /// Notes list.
    NotesList {
        /// Notes.
        notes: Vec<NoteDto>,
    },
    /// A single note.
    NoteOne {
        /// Note.
        note: NoteDto,
    },
    /// Changeset metadata.
    ChangesetInfo {
        /// Id.
        id: i64,
        /// User.
        user: String,
        /// Creation time.
        created_at: String,
        /// Closing time.
        closed_at: String,
        /// Whether it is still open.
        open: bool,
        /// Comment.
        comment: String,
        /// Change count (text).
        changes: String,
        /// Tags.
        tags: Vec<(String, String)>,
    },
    /// Generic success.
    Ok,
    /// Error.
    Error {
        /// Message.
        message: String,
    },
}

/// Sync envelope.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct SyncEnvelope<T> {
    /// Protocol version.
    pub v: u32,
    /// Payload.
    #[serde(flatten)]
    pub body: T,
}

/// Sync session: one request line, one reply line.
pub struct SyncSession {
    inner: super::client::SidecarProcess,
    ready: bool,
}

impl SyncSession {
    /// Starts and performs the handshake.
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

    /// Whether usable.
    pub fn ready(&self) -> bool {
        self.ready
    }

    /// Sends a request and reads the reply.
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
