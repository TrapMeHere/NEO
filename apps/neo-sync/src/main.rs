//! NEO sync sidecar: login, download, upload, notes.
//!
//! Reads JSON lines from stdin and writes JSON lines to stdout. Does not link egui.

#![forbid(unsafe_code)]

mod accounts;

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::sync::mpsc::{Receiver, TryRecvError, channel};

use accounts::{Account, AccountsFile};
use neo_api::oauth::{self as osm_oauth, OAuthConfig, Pkce};
use neo_api::{ApiClient, Note, ReqwestTransport, UploadResult};
use neo_ipc::{
    AccountSummary, ConflictDto, ConflictObjectDto, DiffEntryDto, NoteCommentDto, NoteDto, NotesAction,
    PROTOCOL_VERSION, SyncEnvelope, SyncRequest, SyncResponse,
};

struct OauthWait {
    rx: Receiver<Result<Account, String>>,
}

struct State {
    accounts: AccountsFile,
    oauth: Option<OauthWait>,
}

fn main() {
    let stdin = BufReader::new(std::io::stdin());
    let mut stdout = std::io::stdout();
    let mut state = State {
        accounts: AccountsFile::load(),
        oauth: None,
    };
    for line in stdin.lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let reply = match serde_json::from_str::<SyncEnvelope<SyncRequest>>(&line) {
            Ok(envelope) => {
                if envelope.v != PROTOCOL_VERSION {
                    SyncResponse::Hello {
                        version: PROTOCOL_VERSION,
                        ok: false,
                    }
                } else {
                    handle(&mut state, envelope.body)
                }
            }
            Err(err) => SyncResponse::Error {
                message: err.to_string(),
            },
        };
        let out = SyncEnvelope {
            v: PROTOCOL_VERSION,
            body: reply,
        };
        if writeln!(stdout, "{}", serde_json::to_string(&out).unwrap_or_default()).is_err() {
            break;
        }
        let _ = stdout.flush();
    }
}

fn handle(state: &mut State, request: SyncRequest) -> SyncResponse {
    match request {
        SyncRequest::Hello { version } => SyncResponse::Hello {
            version: PROTOCOL_VERSION,
            ok: version == PROTOCOL_VERSION,
        },
        SyncRequest::ListAccounts => accounts_response(&state.accounts),
        SyncRequest::StartOauth {
            client_id,
            redirect_uri,
        } => start_oauth(state, client_id, redirect_uri),
        SyncRequest::PollOauth => poll_oauth(state),
        SyncRequest::SelectAccount { id } => {
            if state.accounts.select(&id) {
                let _ = state.accounts.save();
                SyncResponse::Ok
            } else {
                SyncResponse::Error {
                    message: "account not found".into(),
                }
            }
        }
        SyncRequest::RemoveAccount { id } => {
            if state.accounts.remove(&id).is_some() {
                let _ = state.accounts.save();
                SyncResponse::Ok
            } else {
                SyncResponse::Error {
                    message: "account not found".into(),
                }
            }
        }
        SyncRequest::ClearCurrent => {
            state.accounts.current = None;
            let _ = state.accounts.save();
            SyncResponse::Ok
        }
        SyncRequest::SaveSettings {
            client_id,
            redirect_uri,
        } => {
            state.accounts.client_id = client_id;
            state.accounts.redirect_uri = redirect_uri;
            let _ = state.accounts.save();
            SyncResponse::Ok
        }
        SyncRequest::Download { api_base, bbox } => download(state, api_base, bbox),
        SyncRequest::OverpassDownload { endpoint, query } => download_overpass(endpoint, query),
        SyncRequest::Upload {
            api_base,
            comment,
            oscs,
        } => upload(state, api_base, comment, oscs),
        SyncRequest::Notes {
            api_base,
            bbox,
            action,
        } => notes(state, api_base, bbox, action),
        SyncRequest::Changeset { api_base, id } => changeset(state, api_base, id),
        SyncRequest::DownloadObject { api_base, kind, id } => download_object(state, api_base, kind, id),
    }
}

fn accounts_response(accounts: &AccountsFile) -> SyncResponse {
    SyncResponse::Accounts {
        current: accounts.current.clone(),
        accounts: accounts
            .accounts
            .iter()
            .map(|account| AccountSummary {
                id: account.id.clone(),
                display_name: account.display_name.clone(),
            })
            .collect(),
        client_id: accounts.client_id.clone(),
        redirect_uri: accounts.redirect_uri.clone(),
    }
}

fn start_oauth(state: &mut State, client_id: String, redirect_uri: String) -> SyncResponse {
    let redirect = if redirect_uri.trim().is_empty() {
        accounts::OAUTH_REDIRECT_URI.to_owned()
    } else {
        redirect_uri
    };
    let Some((port, callback_path)) = accounts::redirect_parts(&redirect) else {
        return SyncResponse::OauthFailed {
            error: "invalid redirect URI".into(),
        };
    };
    let listener = match TcpListener::bind(("127.0.0.1", port)) {
        Ok(listener) => listener,
        Err(err) => {
            return SyncResponse::OauthFailed {
                error: format!("bind 127.0.0.1:{port}: {err}"),
            };
        }
    };
    let client_id = if client_id.trim().is_empty() {
        accounts::oauth_client_id()
    } else {
        client_id.trim().to_owned()
    };
    let config = OAuthConfig::openstreetmap(client_id, redirect);
    let pkce = Pkce::generate();
    let state_token = {
        use rand::RngCore as _;
        let mut bytes = [0u8; 32];
        rand::rng().fill_bytes(&mut bytes);
        bytes.iter().map(|b| format!("{b:02x}")).collect::<String>()
    };
    let url = osm_oauth::authorize_url(&config, &state_token, &pkce);
    let callback_host = format!("127.0.0.1:{port}");
    if let Err(err) = accounts::open_browser(&url) {
        return SyncResponse::OauthFailed {
            error: format!("open browser: {err}"),
        };
    }
    let (tx, rx) = channel();
    state.oauth = Some(OauthWait { rx });
    std::thread::spawn(move || {
        let result = (|| -> Result<Account, String> {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(180);
            listener.set_nonblocking(true).map_err(|err| err.to_string())?;
            loop {
                if std::time::Instant::now() > deadline {
                    return Err("timeout waiting for OAuth callback".to_owned());
                }
                let (mut stream, _) = match listener.accept() {
                    Ok(pair) => pair,
                    Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(std::time::Duration::from_millis(120));
                        continue;
                    }
                    Err(err) => return Err(err.to_string()),
                };
                use std::io::{Read, Write};
                let _ = stream.set_nonblocking(false);
                let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(5)));
                let mut buffer = Vec::with_capacity(2048);
                let mut chunk = [0u8; 1024];
                loop {
                    match stream.read(&mut chunk) {
                        Ok(0) => break,
                        Ok(n) => {
                            buffer.extend_from_slice(&chunk[..n]);
                            if buffer.windows(4).any(|w| w == b"\r\n\r\n") || buffer.len() > 16 * 1024 {
                                break;
                            }
                        }
                        Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => break,
                        Err(err) if err.kind() == std::io::ErrorKind::TimedOut => break,
                        Err(_) => break,
                    }
                }
                let request = String::from_utf8_lossy(&buffer);
                match accounts::parse_oauth_callback_http(
                    &request,
                    &state_token,
                    &callback_path,
                    &callback_host,
                ) {
                    accounts::OAuthCallbackParse::Ignore => {
                        let _ = write!(stream, "HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n");
                        continue;
                    }
                    accounts::OAuthCallbackParse::Failed(error) => {
                        let _ = write!(
                            stream,
                            "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nConnection: close\r\n\r\n<html><body><h3>NEO OAuth</h3><p>Sign-in failed. Return to NEO to see the error.</p></body></html>"
                        );
                        return Err(error);
                    }
                    accounts::OAuthCallbackParse::Code(code) => {
                        let _ = write!(
                            stream,
                            "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nConnection: close\r\n\r\n<html><body><h3>NEO OAuth</h3><p>Signed in. You can close this window.</p></body></html>"
                        );
                        let _ = stream.flush();
                        let transport = ReqwestTransport::new().map_err(|e| e.to_string())?;
                        let tokens = osm_oauth::exchange_code(&transport, &config, &code, &pkce)
                            .map_err(|e| accounts::format_oauth_http_error(&e.to_string()))?;
                        let user = osm_oauth::fetch_user(&transport, &config.api_base, &tokens.access_token)
                            .map_err(|e| e.to_string())?;
                        return Ok(Account::new(user.id, &user.display_name, &tokens));
                    }
                }
            }
        })();
        let _ = tx.send(result);
    });
    SyncResponse::OauthStarted
}

fn poll_oauth(state: &mut State) -> SyncResponse {
    let Some(wait) = &state.oauth else {
        return SyncResponse::Error {
            message: "no oauth in progress".into(),
        };
    };
    match wait.rx.try_recv() {
        Ok(Ok(account)) => {
            let summary = AccountSummary {
                id: account.id.clone(),
                display_name: account.display_name.clone(),
            };
            state.accounts.upsert(account);
            let _ = state.accounts.save();
            state.oauth = None;
            SyncResponse::OauthDone { account: summary }
        }
        Ok(Err(error)) => {
            state.oauth = None;
            SyncResponse::OauthFailed { error }
        }
        Err(TryRecvError::Empty) => SyncResponse::OauthPending,
        Err(TryRecvError::Disconnected) => {
            state.oauth = None;
            SyncResponse::OauthFailed {
                error: "oauth worker disconnected".into(),
            }
        }
    }
}

fn active_token(accounts: &mut AccountsFile) -> Result<String, String> {
    let mut account = accounts
        .active()
        .cloned()
        .ok_or_else(|| "no account".to_owned())?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let expiring = account
        .expires_at
        .map(|expires| expires - now < 60)
        .unwrap_or(false);
    if expiring {
        let refresh = account
            .refresh_token
            .clone()
            .ok_or_else(|| "token expired; please sign in again".to_owned())?;
        let transport = ReqwestTransport::new().map_err(|e| e.to_string())?;
        let config = OAuthConfig::openstreetmap(
            accounts::oauth_client_id(),
            accounts::OAUTH_REDIRECT_URI.to_owned(),
        );
        let tokens = osm_oauth::refresh_token(&transport, &config, &refresh).map_err(|e| e.to_string())?;
        account.access_token = tokens.access_token;
        if tokens.refresh_token.is_some() {
            account.refresh_token = tokens.refresh_token;
        }
        account.expires_at = tokens.expires_in.map(|secs| now + secs);
        account.scopes = tokens.scope.or(account.scopes);
        accounts.upsert(account.clone());
        let _ = accounts.save();
    }
    Ok(account.access_token)
}

fn download(state: &mut State, api_base: String, bbox: [f64; 4]) -> SyncResponse {
    let token = active_token(&mut state.accounts).ok();
    let transport = match ReqwestTransport::new() {
        Ok(transport) => transport,
        Err(err) => {
            return SyncResponse::Error {
                message: err.to_string(),
            };
        }
    };
    let client = ApiClient::new(api_base, token, transport);
    let bbox = (bbox[0], bbox[1], bbox[2], bbox[3]);
    match client.download_map(bbox) {
        Ok(xml) => SyncResponse::Downloaded { xml },
        Err(err) => SyncResponse::Error {
            message: err.to_string(),
        },
    }
}

fn download_overpass(endpoint: String, query: String) -> SyncResponse {
    let transport = match ReqwestTransport::new() {
        Ok(transport) => transport,
        Err(err) => {
            return SyncResponse::Error {
                message: err.to_string(),
            };
        }
    };
    match neo_api::download_overpass(&transport, &endpoint, &query) {
        Ok(xml) => SyncResponse::Downloaded { xml },
        Err(err) => SyncResponse::Error {
            message: err.to_string(),
        },
    }
}

fn upload(state: &mut State, api_base: String, comment: String, oscs: Vec<String>) -> SyncResponse {
    let token = match active_token(&mut state.accounts) {
        Ok(token) => token,
        Err(err) => {
            return SyncResponse::Error { message: err };
        }
    };
    let result = (|| -> Result<UploadResult, (UploadResult, neo_api::ApiError)> {
        let transport = ReqwestTransport::new().map_err(|e| {
            (
                UploadResult::default(),
                neo_api::ApiError::Transport(neo_api::TransportError::Other(e.to_string())),
            )
        })?;
        let client = ApiClient::new(api_base, Some(token), transport);
        let changeset = client
            .create_changeset(&comment, "NEO/0.1")
            .map_err(|err| (UploadResult::default(), err))?;
        let mut merged = UploadResult::default();
        let mut failure = None;
        for osc in &oscs {
            let stamped = neo_io::stamp_osmchange_changeset(osc, changeset);
            match client.upload_diff(changeset, &stamped) {
                Ok(result) => merged.entries.extend(result.entries),
                Err(err) => {
                    failure = Some(err);
                    break;
                }
            }
        }
        let close = client.close_changeset(changeset);
        if let Some(err) = failure {
            return Err((merged, err));
        }
        if let Err(err) = close {
            tracing::warn!("close changeset failed: {err}");
        }
        Ok(merged)
    })();
    match result {
        Ok(result) => SyncResponse::Uploaded {
            entries: result.entries.iter().map(diff_dto).collect(),
        },
        Err((partial, neo_api::ApiError::Conflict(report))) => {
            if partial.entries.is_empty() {
                SyncResponse::Conflict {
                    report: ConflictDto {
                        message: report.message,
                        objects: report
                            .objects
                            .iter()
                            .map(|object| ConflictObjectDto {
                                kind: kind_name(object.kind),
                                old_id: object.old_id.raw(),
                                new_id: object.new_id.map(|id| id.raw()),
                                version: object.version,
                            })
                            .collect(),
                    },
                }
            } else {
                SyncResponse::UploadedPartial {
                    entries: partial.entries.iter().map(diff_dto).collect(),
                    error: format!("conflict after partial upload: {report:?}"),
                }
            }
        }
        Err((partial, err)) => {
            if partial.entries.is_empty() {
                SyncResponse::Error {
                    message: err.to_string(),
                }
            } else {
                SyncResponse::UploadedPartial {
                    entries: partial.entries.iter().map(diff_dto).collect(),
                    error: err.to_string(),
                }
            }
        }
    }
}

fn notes(state: &mut State, api_base: String, bbox: [f64; 4], action: NotesAction) -> SyncResponse {
    let needs_auth = !matches!(action, NotesAction::Refresh);
    let token = if needs_auth {
        match active_token(&mut state.accounts) {
            Ok(token) => Some(token),
            Err(err) => {
                return SyncResponse::Error { message: err };
            }
        }
    } else {
        active_token(&mut state.accounts).ok()
    };
    let transport = match ReqwestTransport::new() {
        Ok(transport) => transport,
        Err(err) => {
            return SyncResponse::Error {
                message: err.to_string(),
            };
        }
    };
    let client = ApiClient::new(api_base, token, transport);
    let bbox = (bbox[0], bbox[1], bbox[2], bbox[3]);
    let outcome = match action {
        NotesAction::Refresh => client.notes(bbox).map(NotesOut::List),
        NotesAction::Create { lat, lon, text } => client.create_note(lat, lon, &text).map(NotesOut::One),
        NotesAction::Comment { id, text } => client.comment_note(id, &text).map(NotesOut::One),
        NotesAction::Close { id, text } => client.close_note(id, &text).map(NotesOut::One),
    };
    match outcome {
        Ok(NotesOut::List(notes)) => SyncResponse::NotesList {
            notes: notes.iter().map(note_dto).collect(),
        },
        Ok(NotesOut::One(note)) => SyncResponse::NoteOne {
            note: note_dto(&note),
        },
        Err(err) => SyncResponse::Error {
            message: err.to_string(),
        },
    }
}

enum NotesOut {
    List(Vec<Note>),
    One(Note),
}

fn download_object(state: &mut State, api_base: String, kind: String, id: i64) -> SyncResponse {
    let token = active_token(&mut state.accounts).ok();
    let transport = match ReqwestTransport::new() {
        Ok(transport) => transport,
        Err(err) => {
            return SyncResponse::Error {
                message: err.to_string(),
            };
        }
    };
    let object_kind = match kind.as_str() {
        "way" => neo_core::ObjectKind::Way,
        "relation" => neo_core::ObjectKind::Relation,
        _ => neo_core::ObjectKind::Node,
    };
    let client = ApiClient::new(api_base, token, transport);
    match client.download_object(object_kind, neo_core::ObjectId::new(id)) {
        Ok(xml) => SyncResponse::Downloaded { xml },
        Err(err) => SyncResponse::Error {
            message: err.to_string(),
        },
    }
}

fn changeset(state: &mut State, api_base: String, id: i64) -> SyncResponse {
    let token = active_token(&mut state.accounts).ok();
    let transport = match ReqwestTransport::new() {
        Ok(transport) => transport,
        Err(err) => {
            return SyncResponse::Error {
                message: err.to_string(),
            };
        }
    };
    let client = ApiClient::new(api_base, token, transport);
    match client.changeset(id) {
        Ok(info) => SyncResponse::ChangesetInfo {
            id: info.id,
            user: info.user,
            created_at: info.created_at,
            closed_at: info.closed_at.unwrap_or_default(),
            open: info.open,
            comment: info.comment.clone(),
            changes: info
                .changes_count
                .map(|count| count.to_string())
                .unwrap_or_else(|| "?".to_owned()),
            tags: info.tags,
        },
        Err(err) => SyncResponse::Error {
            message: err.to_string(),
        },
    }
}

fn diff_dto(entry: &neo_api::DiffEntry) -> DiffEntryDto {
    DiffEntryDto {
        kind: kind_name(entry.kind),
        old_id: entry.old_id.raw(),
        new_id: entry.new_id.map(|id| id.raw()),
        new_version: entry.new_version,
    }
}

fn note_dto(note: &Note) -> NoteDto {
    NoteDto {
        id: note.id,
        lat: note.lat,
        lon: note.lon,
        status: note.status.clone(),
        comments: note
            .comments
            .iter()
            .map(|comment| NoteCommentDto {
                date: comment.date.clone(),
                user: comment.user.clone(),
                text: comment.text.clone(),
            })
            .collect(),
    }
}

fn kind_name(kind: neo_core::ObjectKind) -> String {
    match kind {
        neo_core::ObjectKind::Node => "node".into(),
        neo_core::ObjectKind::Way => "way".into(),
        neo_core::ObjectKind::Relation => "relation".into(),
        neo_core::ObjectKind::Changeset => "changeset".into(),
    }
}
