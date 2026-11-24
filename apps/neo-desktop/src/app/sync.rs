//! NeoApp::sync — 经 neo-sync 旁路完成登录/下载/上传/笔记。

use anyhow::Result;
use neo_core::{ObjectId, ObjectKind};
use neo_editor::Editor;
use neo_i18n::{t, tf};
use neo_io::{XmlWriteMode, read_osm_xml_file, write_changeset, write_osm_xml_file};
use neo_ipc::{ConflictDto, DiffEntryDto, NoteDto, NotesAction, SyncRequest, SyncResponse, SyncSession};
use neo_store::DataSet;
use std::sync::mpsc::TryRecvError;
use std::sync::{Arc, Mutex};

use super::{NeoApp, NotesOutcome, NotesRequest, SyncOutcome, data_dir};

impl NeoApp {
    pub(super) fn refresh_accounts_from_sidecar(&mut self) {
        let Some(sidecar) = &self.sync_sidecar else {
            self.settings.accounts.clear();
            self.account_current = None;
            return;
        };
        let response = {
            let Ok(mut guard) = sidecar.lock() else { return };
            guard.call(SyncRequest::ListAccounts)
        };
        match response {
            Ok(SyncResponse::Accounts {
                current,
                accounts,
                client_id,
                redirect_uri,
            }) => {
                self.account_current = current.clone();
                self.settings.selected = current;
                self.settings.client_id = client_id.clone();
                self.settings.redirect_uri = redirect_uri.clone();
                self.sidecar_client_id = client_id;
                self.sidecar_redirect_uri = redirect_uri;
                self.settings.accounts = accounts
                    .into_iter()
                    .map(|account| neo_ui::AccountView {
                        id: account.id.clone(),
                        display_name: account.display_name,
                        active: self.account_current.as_deref() == Some(account.id.as_str()),
                    })
                    .collect();
            }
            Ok(SyncResponse::Error { message }) => {
                self.settings.login_status = tf("settings.account.failed", &[("err", &message)]);
            }
            Err(err) => {
                self.settings.login_status = tf("settings.account.failed", &[("err", &err.to_string())]);
            }
            _ => {}
        }
    }

    /// 启动 OAuth2 + PKCE 登录
    pub(super) fn start_oauth(&mut self) {
        let Some(sidecar) = &self.sync_sidecar else {
            self.settings.login_status = tf("settings.account.failed", &[("err", "neo-sync unavailable")]);
            return;
        };
        let client_id = self.settings.client_id.clone();
        let redirect_uri = self.settings.redirect_uri.clone();
        let response = {
            let Ok(mut guard) = sidecar.lock() else {
                self.settings.login_status = tf("settings.account.failed", &[("err", "sync lock")]);
                return;
            };
            guard.call(SyncRequest::StartOauth {
                client_id,
                redirect_uri,
            })
        };
        match response {
            Ok(SyncResponse::OauthStarted) => {
                self.settings.login_status = t("settings.account.waiting").to_owned();
                self.oauth_pending = true;
            }
            Ok(SyncResponse::OauthFailed { error }) => {
                self.settings.login_status = tf("settings.account.failed", &[("err", &error)]);
                self.oauth_pending = false;
            }
            Ok(SyncResponse::Error { message }) => {
                self.settings.login_status = tf("settings.account.failed", &[("err", &message)]);
                self.oauth_pending = false;
            }
            Err(err) => {
                self.settings.login_status = tf("settings.account.failed", &[("err", &err.to_string())]);
                self.oauth_pending = false;
            }
            _ => {
                self.settings.login_status =
                    tf("settings.account.failed", &[("err", "unexpected oauth reply")]);
                self.oauth_pending = false;
            }
        }
    }

    /// 轮询 OAuth 后台结果
    pub(super) fn poll_oauth(&mut self) {
        if !self.oauth_pending {
            return;
        }
        let Some(sidecar) = &self.sync_sidecar else {
            self.oauth_pending = false;
            return;
        };
        let response = {
            let Ok(mut guard) = sidecar.lock() else { return };
            guard.call(SyncRequest::PollOauth)
        };
        match response {
            Ok(SyncResponse::OauthPending) => {}
            Ok(SyncResponse::OauthDone { account }) => {
                self.oauth_pending = false;
                self.settings.login_status =
                    tf("settings.account.success", &[("name", &account.display_name)]);
                self.refresh_accounts_from_sidecar();
            }
            Ok(SyncResponse::OauthFailed { error }) => {
                self.oauth_pending = false;
                self.settings.login_status = tf("settings.account.failed", &[("err", &error)]);
            }
            Ok(SyncResponse::Error { message }) => {
                self.oauth_pending = false;
                self.settings.login_status = tf("settings.account.failed", &[("err", &message)]);
            }
            Err(err) => {
                self.oauth_pending = false;
                self.settings.login_status = tf("settings.account.failed", &[("err", &err.to_string())]);
            }
            _ => {}
        }
    }

    pub(super) fn api_base(&self) -> String {
        let default = "https://api.openstreetmap.org".to_owned();
        let candidate = std::env::var("NEO_API_URL")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .or_else(|| {
                let value = self.settings.api_base.trim().to_owned();
                (!value.is_empty()).then_some(value)
            })
            .unwrap_or_else(|| default.clone());
        if candidate.starts_with("https://")
            || candidate.starts_with("http://127.0.0.1")
            || candidate.starts_with("http://localhost")
        {
            candidate.trim_end_matches('/').to_owned()
        } else {
            tracing::warn!("ignoring insecure API base {candidate:?}; using OSM");
            default
        }
    }

    pub(super) fn start_api_download(&mut self, bbox: (f64, f64, f64, f64)) {
        if self.sync_busy {
            self.sync_status = t("sync.busy").to_owned();
            return;
        }
        let Some(sidecar) = self.sync_sidecar.clone() else {
            self.sync_status = tf("app.error.sync", &[("err", "neo-sync unavailable")]);
            return;
        };
        let base = self.api_base();
        let (tx, rx) = std::sync::mpsc::channel();
        self.sync_rx = Some(rx);
        self.sync_busy = true;
        self.sync_status = t("sync.busy").to_owned();
        self.pending_download_bbox = Some(neo_ui::BboxView {
            min_lon: bbox.0.min(bbox.2),
            min_lat: bbox.1.min(bbox.3),
            max_lon: bbox.0.max(bbox.2),
            max_lat: bbox.1.max(bbox.3),
        });
        std::thread::spawn(move || {
            let outcome = call_sync(
                &sidecar,
                SyncRequest::Download {
                    api_base: base,
                    bbox: [bbox.0, bbox.1, bbox.2, bbox.3],
                },
            );
            let _ = tx.send(match outcome {
                Ok(SyncResponse::Downloaded { xml }) => SyncOutcome::Downloaded(xml),
                Ok(SyncResponse::Error { message }) => SyncOutcome::Error(message),
                Ok(other) => SyncOutcome::Error(format!("unexpected: {other:?}")),
                Err(err) => SyncOutcome::Error(err),
            });
        });
    }

    /// 把一次成功下载的范围并入活动层（已包含则跳过，被新框完全覆盖的旧框剔除）。
    pub(super) fn record_download_bbox(&mut self, bbox: neo_ui::BboxView) {
        let bbox = neo_ui::BboxView {
            min_lon: bbox.min_lon.min(bbox.max_lon),
            min_lat: bbox.min_lat.min(bbox.max_lat),
            max_lon: bbox.min_lon.max(bbox.max_lon),
            max_lat: bbox.min_lat.max(bbox.max_lat),
        };
        if (bbox.max_lon - bbox.min_lon).abs() < 1e-9 || (bbox.max_lat - bbox.min_lat).abs() < 1e-9 {
            return;
        }
        if self
            .downloaded_bboxes
            .iter()
            .any(|existing| bbox_contains(existing, &bbox))
        {
            return;
        }
        self.downloaded_bboxes
            .retain(|existing| !bbox_contains(&bbox, existing));
        self.downloaded_bboxes.push(bbox);
    }

    pub(super) fn start_overpass_download(
        &mut self,
        endpoint: String,
        query: String,
        bbox: Option<neo_ui::BboxView>,
    ) {
        if self.sync_busy {
            self.sync_status = t("sync.busy").to_owned();
            return;
        }
        let endpoint = endpoint.trim().to_owned();
        let query = query.trim().to_owned();
        if endpoint.is_empty() || query.is_empty() {
            self.sync_status = tf("app.error.sync", &[("err", "empty overpass query")]);
            return;
        }
        let Some(sidecar) = self.sync_sidecar.clone() else {
            self.sync_status = tf("app.error.sync", &[("err", "neo-sync unavailable")]);
            return;
        };
        let (tx, rx) = std::sync::mpsc::channel();
        self.sync_rx = Some(rx);
        self.sync_busy = true;
        self.sync_status = t("sync.busy").to_owned();
        self.pending_download_bbox = bbox.map(|bbox| neo_ui::BboxView {
            min_lon: bbox.min_lon.min(bbox.max_lon),
            min_lat: bbox.min_lat.min(bbox.max_lat),
            max_lon: bbox.min_lon.max(bbox.max_lon),
            max_lat: bbox.min_lat.max(bbox.max_lat),
        });
        std::thread::spawn(move || {
            let outcome = call_sync(&sidecar, SyncRequest::OverpassDownload { endpoint, query });
            let _ = tx.send(match outcome {
                Ok(SyncResponse::Downloaded { xml }) => SyncOutcome::Downloaded(xml),
                Ok(SyncResponse::Error { message }) => SyncOutcome::Error(message),
                Ok(other) => SyncOutcome::Error(format!("unexpected: {other:?}")),
                Err(err) => SyncOutcome::Error(err),
            });
        });
    }

    pub(super) fn start_sync_upload(&mut self, selected_only: bool) {
        if self.sync_busy {
            self.sync_status = t("sync.busy").to_owned();
            return;
        }
        if self.account_current.is_none() {
            self.sync_status = t("sync.no_account").to_owned();
            return;
        }
        let Some(sidecar) = self.sync_sidecar.clone() else {
            self.sync_status = tf("app.error.sync", &[("err", "neo-sync unavailable")]);
            return;
        };
        let set = if selected_only {
            let selection = self.editor.selection();
            let mut selected = std::collections::HashSet::new();
            for id in selection.nodes() {
                selected.insert((ObjectKind::Node, *id));
            }
            for id in selection.ways() {
                selected.insert((ObjectKind::Way, *id));
            }
            for id in selection.relations() {
                selected.insert((ObjectKind::Relation, *id));
            }
            if selected.is_empty() {
                self.sync_status = t("app.error.no_selection").to_owned();
                return;
            }
            match neo_io::collect_changeset_selected(self.editor.data(), &selected) {
                Ok(set) => set,
                Err(err) => {
                    self.sync_status = tf("app.error.sync", &[("err", &err.to_string())]);
                    return;
                }
            }
        } else {
            match neo_io::collect_changeset(self.editor.data()) {
                Ok(set) => set,
                Err(err) => {
                    self.sync_status = tf("app.error.sync", &[("err", &err.to_string())]);
                    return;
                }
            }
        };
        if set.is_empty() {
            self.sync_status = t("app.error.no_selection_error").to_owned();
            return;
        }
        let chunks = match set.chunks(4000) {
            Ok(chunks) => chunks,
            Err(err) => {
                self.sync_status = tf("app.error.sync", &[("err", &err.to_string())]);
                return;
            }
        };
        let mut oscs = Vec::with_capacity(chunks.len());
        for chunk in &chunks {
            let mut buffer = Vec::new();
            if let Err(err) = write_changeset(chunk, self.editor.data(), &mut buffer) {
                self.sync_status = tf("app.error.sync", &[("err", &err.to_string())]);
                return;
            }
            oscs.push(String::from_utf8_lossy(&buffer).into_owned());
        }
        let comment = if self.sync_comment.trim().is_empty() {
            "NEO edit".to_owned()
        } else {
            self.sync_comment.clone()
        };
        let base = self.api_base();
        let (tx, rx) = std::sync::mpsc::channel();
        self.sync_rx = Some(rx);
        self.sync_busy = true;
        self.sync_status = t("sync.busy").to_owned();
        std::thread::spawn(move || {
            let outcome = call_sync(
                &sidecar,
                SyncRequest::Upload {
                    api_base: base,
                    comment,
                    oscs,
                },
            );
            let _ = tx.send(match outcome {
                Ok(SyncResponse::Uploaded { entries }) => SyncOutcome::Uploaded(entries_to_upload(entries)),
                Ok(SyncResponse::UploadedPartial { entries, error }) => {
                    SyncOutcome::UploadedPartial(entries_to_upload(entries), error)
                }
                Ok(SyncResponse::Conflict { report }) => SyncOutcome::Conflict(conflict_from_dto(report)),
                Ok(SyncResponse::Error { message }) => SyncOutcome::Error(message),
                Ok(other) => SyncOutcome::Error(format!("unexpected: {other:?}")),
                Err(err) => SyncOutcome::Error(err),
            });
        });
    }

    pub(super) fn poll_sync(&mut self) {
        let Some(rx) = &self.sync_rx else { return };
        let outcome = match rx.try_recv() {
            Ok(outcome) => outcome,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => {
                self.sync_busy = false;
                self.sync_rx = None;
                self.pending_download_bbox = None;
                self.sync_status = tf("app.error.sync", &[("err", "background task disconnected")]);
                return;
            }
        };
        self.sync_busy = false;
        self.sync_rx = None;
        match outcome {
            SyncOutcome::Downloaded(xml) => {
                let path = data_dir().join("neo-sync-download.osm");
                if let Some(parent) = path.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                if let Err(err) = std::fs::write(&path, xml) {
                    self.pending_download_bbox = None;
                    self.sync_status = tf("app.error.sync", &[("err", &err.to_string())]);
                    return;
                }
                let mut data = DataSet::new();
                match read_osm_xml_file(&mut data, &path) {
                    Ok(report) => {
                        let nodes = data.node_count();
                        let ways = data.way_count();
                        let _ = report;
                        if self.data_listed {
                            match self.editor.merge_from_dataset(&data) {
                                Ok(stats) => {
                                    if let Some(bbox) = self.pending_download_bbox.take() {
                                        self.record_download_bbox(bbox);
                                    }
                                    self.scene = None;
                                    self.layer_visible = true;
                                    self.layer_active = true;
                                    self.sync_status = tf(
                                        "status.sync_merged",
                                        &[
                                            ("added", &stats.added.to_string()),
                                            ("updated", &stats.updated.to_string()),
                                            ("skipped", &stats.skipped.to_string()),
                                            ("nodes", &nodes.to_string()),
                                            ("ways", &ways.to_string()),
                                        ],
                                    );
                                }
                                Err(err) => {
                                    self.pending_download_bbox = None;
                                    self.sync_status = tf("app.error.sync", &[("err", &err.to_string())]);
                                }
                            }
                        } else {
                            let label = "OSM download".to_owned();
                            self.data_path = Some(path.to_path_buf());
                            self.set_editor(Editor::new(data), label);
                            if let Some(bbox) = self.pending_download_bbox.take() {
                                self.record_download_bbox(bbox);
                            }
                            self.reopen_journal();
                            self.sync_status = tf(
                                "status.sync_downloaded",
                                &[("nodes", &nodes.to_string()), ("ways", &ways.to_string())],
                            );
                        }
                    }
                    Err(err) => {
                        self.pending_download_bbox = None;
                        self.sync_status = tf("app.error.sync", &[("err", &err.to_string())]);
                    }
                }
            }
            SyncOutcome::Uploaded(result) => {
                self.sync_conflicts.clear();
                self.sync_conflict_report = None;
                let mappings: Vec<neo_editor::DiffMapping> = result
                    .entries
                    .iter()
                    .map(|entry| neo_editor::DiffMapping {
                        kind: entry.kind,
                        old_id: entry.old_id,
                        new_id: entry.new_id.unwrap_or(entry.old_id),
                        new_version: entry
                            .new_version
                            .filter(|version| *version > 0)
                            .map(|version| version as u32),
                    })
                    .collect();
                let applied = self.editor.apply_diff_result(&mappings);
                let uploaded_path = data_dir().join("neo-uploaded.osm");
                if write_osm_xml_file(self.editor.data(), &uploaded_path, XmlWriteMode::Josm).is_ok() {
                    self.data_path = Some(uploaded_path);
                    self.persist_state();
                }
                self.scene = None;
                self.sync_status = tf("status.sync_uploaded", &[("n", &applied.to_string())]);
            }
            SyncOutcome::UploadedPartial(result, err) => {
                let mappings: Vec<neo_editor::DiffMapping> = result
                    .entries
                    .iter()
                    .map(|entry| neo_editor::DiffMapping {
                        kind: entry.kind,
                        old_id: entry.old_id,
                        new_id: entry.new_id.unwrap_or(entry.old_id),
                        new_version: entry
                            .new_version
                            .filter(|version| *version > 0)
                            .map(|version| version as u32),
                    })
                    .collect();
                let applied = self.editor.apply_uploaded(&mappings, false);
                let uploaded_path = data_dir().join("neo-uploaded.osm");
                if write_osm_xml_file(self.editor.data(), &uploaded_path, XmlWriteMode::Josm).is_ok() {
                    self.data_path = Some(uploaded_path);
                    self.persist_state();
                }
                self.scene = None;
                self.sync_status = tf(
                    "status.sync_partial",
                    &[("n", &applied.to_string()), ("err", &err)],
                );
            }
            SyncOutcome::Conflict(report) => {
                self.sync_conflict_report = Some(report.clone());
                self.sync_conflicts = report
                    .objects
                    .iter()
                    .map(|object| neo_ui::SyncConflict {
                        kind: object.kind,
                        id: object.old_id,
                        version: object.version,
                    })
                    .collect();
                self.sync_status = tf(
                    "status.sync_conflict",
                    &[("n", &self.sync_conflicts.len().to_string())],
                );
            }
            SyncOutcome::Error(err) => {
                self.pending_download_bbox = None;
                self.sync_status = tf("app.error.sync", &[("err", &err)]);
            }
        }
    }

    /// 构造校验面板视图
    pub(super) fn validation_view(&self) -> Option<neo_ui::ValidationView> {
        self.validation.as_ref().map(|report| neo_ui::ValidationView {
            items: report
                .issues
                .iter()
                .map(|issue| neo_ui::ValidationItem {
                    severity: match issue.severity {
                        neo_validate::Severity::Error => 2,
                        neo_validate::Severity::Warning => 1,
                        neo_validate::Severity::Info => 0,
                    },
                    rule: issue.rule.id(),
                    detail: issue.detail.clone(),
                    objects: issue.objects.clone(),
                })
                .collect(),
            errors: report.errors(),
            warnings: report.warnings(),
            checked: (report.nodes, report.ways, report.relations),
        })
    }

    pub(super) fn notes_bbox(&self) -> (f64, f64, f64, f64) {
        let bounds = self.camera.visible_bounds();
        let a = bounds.min.to_latlon();
        let b = bounds.max.to_latlon();
        (
            a.lon.to_degrees().min(b.lon.to_degrees()),
            a.lat.to_degrees().min(b.lat.to_degrees()),
            a.lon.to_degrees().max(b.lon.to_degrees()),
            a.lat.to_degrees().max(b.lat.to_degrees()),
        )
    }

    pub(super) fn note_view(note: &NoteDto) -> neo_ui::NoteView {
        neo_ui::NoteView {
            id: note.id,
            lat: note.lat,
            lon: note.lon,
            status: note.status.clone(),
            comments: note
                .comments
                .iter()
                .map(|comment| neo_ui::NoteCommentView {
                    date: comment.date.clone(),
                    user: comment.user.clone(),
                    text: comment.text.clone(),
                })
                .collect(),
        }
    }

    pub(super) fn spawn_notes(&mut self, request: NotesRequest) {
        if self.notes_rx.is_some() {
            return;
        }
        let Some(sidecar) = self.sync_sidecar.clone() else {
            self.notes.status = tf("notes.error", &[("err", "neo-sync unavailable")]);
            return;
        };
        let needs_auth = !matches!(request, NotesRequest::Refresh);
        if needs_auth && self.account_current.is_none() {
            self.notes.status = t("notes.login_required").to_owned();
            return;
        }
        let base = self.api_base();
        let bbox = self.notes_bbox();
        let action = match request {
            NotesRequest::Refresh => NotesAction::Refresh,
            NotesRequest::Create { lat, lon, text } => NotesAction::Create { lat, lon, text },
            NotesRequest::Comment { id, text } => NotesAction::Comment { id, text },
            NotesRequest::Close { id, text } => NotesAction::Close { id, text },
        };
        let (tx, rx) = std::sync::mpsc::channel();
        self.notes_rx = Some(rx);
        self.notes.busy = true;
        std::thread::spawn(move || {
            let outcome = call_sync(
                &sidecar,
                SyncRequest::Notes {
                    api_base: base,
                    bbox: [bbox.0, bbox.1, bbox.2, bbox.3],
                    action,
                },
            );
            let _ = tx.send(match outcome {
                Ok(SyncResponse::NotesList { notes }) => Ok(NotesOutcome::List(notes)),
                Ok(SyncResponse::NoteOne { note }) => Ok(NotesOutcome::One(note)),
                Ok(SyncResponse::Error { message }) => Err(message),
                Ok(other) => Err(format!("unexpected: {other:?}")),
                Err(err) => Err(err),
            });
        });
    }

    pub(super) fn poll_notes(&mut self) {
        let Some(rx) = &self.notes_rx else { return };
        let result = match rx.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => Err("notes worker disconnected".to_owned()),
        };
        self.notes_rx = None;
        self.notes.busy = false;
        match result {
            Ok(NotesOutcome::List(notes)) => {
                self.notes.notes = notes.iter().map(Self::note_view).collect();
                if self
                    .notes
                    .selected
                    .is_none_or(|id| !self.notes.notes.iter().any(|note| note.id == id))
                {
                    self.notes.selected = self.notes.notes.first().map(|note| note.id);
                }
                self.notes.status = tf("notes.loaded", &[("n", &self.notes.notes.len().to_string())]);
            }
            Ok(NotesOutcome::One(note)) => {
                let view = Self::note_view(&note);
                if let Some(existing) = self
                    .notes
                    .notes
                    .iter_mut()
                    .find(|existing| existing.id == view.id)
                {
                    *existing = view.clone();
                } else {
                    self.notes.notes.push(view.clone());
                }
                self.notes.notes.sort_by_key(|note| note.id);
                self.notes.selected = Some(view.id);
                self.notes.text.clear();
                self.notes.status = tf("notes.updated", &[("id", &view.id.to_string())]);
            }
            Err(err) => {
                self.notes.status = tf("notes.error", &[("err", &err)]);
            }
        }
    }

    pub(super) fn account_logout(&mut self) {
        if let Some(sidecar) = &self.sync_sidecar
            && let Ok(mut guard) = sidecar.lock()
        {
            let _ = guard.call(SyncRequest::ClearCurrent);
        }
        self.refresh_accounts_from_sidecar();
        self.settings.login_status = t("settings.account.logged_out").to_owned();
    }

    pub(super) fn account_select(&mut self, id: &str) {
        if let Some(sidecar) = &self.sync_sidecar
            && let Ok(mut guard) = sidecar.lock()
        {
            let _ = guard.call(SyncRequest::SelectAccount { id: id.to_owned() });
        }
        self.refresh_accounts_from_sidecar();
        let name = self
            .settings
            .accounts
            .iter()
            .find(|account| account.id == id)
            .map(|account| account.display_name.clone())
            .unwrap_or_default();
        self.settings.login_status = tf("settings.account.selected", &[("name", &name)]);
    }

    pub(super) fn account_remove(&mut self, id: &str) {
        let name = self
            .settings
            .accounts
            .iter()
            .find(|account| account.id == id)
            .map(|account| account.display_name.clone())
            .unwrap_or_default();
        if let Some(sidecar) = &self.sync_sidecar
            && let Ok(mut guard) = sidecar.lock()
        {
            let _ = guard.call(SyncRequest::RemoveAccount { id: id.to_owned() });
        }
        self.refresh_accounts_from_sidecar();
        self.settings.login_status = tf("settings.account.removed", &[("name", &name)]);
    }

    /// 经旁路下载单个对象的 OSM XML。
    pub(super) fn download_object_xml(&mut self, kind: ObjectKind, id: ObjectId) -> Result<String, String> {
        let Some(sidecar) = &self.sync_sidecar else {
            return Err("neo-sync unavailable".into());
        };
        if self.account_current.is_none() {
            return Err(t("sync.no_account").to_owned());
        }
        let kind_name = match kind {
            ObjectKind::Node => "node",
            ObjectKind::Way => "way",
            ObjectKind::Relation => "relation",
            ObjectKind::Changeset => "changeset",
        };
        let mut guard = sidecar.lock().map_err(|_| "sync lock poisoned".to_owned())?;
        match guard.call(SyncRequest::DownloadObject {
            api_base: self.api_base(),
            kind: kind_name.to_owned(),
            id: id.raw(),
        }) {
            Ok(SyncResponse::Downloaded { xml }) => Ok(xml),
            Ok(SyncResponse::Error { message }) => Err(message),
            Ok(other) => Err(format!("unexpected: {other:?}")),
            Err(err) => Err(err.to_string()),
        }
    }

    pub(super) fn persist_account_settings(&mut self) {
        if self.settings.client_id == self.sidecar_client_id
            && self.settings.redirect_uri == self.sidecar_redirect_uri
        {
            return;
        }
        if let Some(sidecar) = &self.sync_sidecar
            && let Ok(mut guard) = sidecar.lock()
        {
            let _ = guard.call(SyncRequest::SaveSettings {
                client_id: self.settings.client_id.clone(),
                redirect_uri: self.settings.redirect_uri.clone(),
            });
        }
        self.sidecar_client_id = self.settings.client_id.clone();
        self.sidecar_redirect_uri = self.settings.redirect_uri.clone();
    }
}

fn bbox_contains(outer: &neo_ui::BboxView, inner: &neo_ui::BboxView) -> bool {
    outer.min_lon <= inner.min_lon + 1e-9
        && outer.min_lat <= inner.min_lat + 1e-9
        && outer.max_lon >= inner.max_lon - 1e-9
        && outer.max_lat >= inner.max_lat - 1e-9
}

fn call_sync(sidecar: &Arc<Mutex<SyncSession>>, request: SyncRequest) -> Result<SyncResponse, String> {
    let mut guard = sidecar.lock().map_err(|_| "sync lock poisoned".to_owned())?;
    guard.call(request).map_err(|err| err.to_string())
}

fn kind_from_name(name: &str) -> ObjectKind {
    match name {
        "way" => ObjectKind::Way,
        "relation" => ObjectKind::Relation,
        "changeset" => ObjectKind::Changeset,
        _ => ObjectKind::Node,
    }
}

fn entries_to_upload(entries: Vec<DiffEntryDto>) -> neo_api::UploadResult {
    neo_api::UploadResult {
        entries: entries
            .into_iter()
            .map(|entry| neo_api::DiffEntry {
                kind: kind_from_name(&entry.kind),
                old_id: ObjectId::new(entry.old_id),
                new_id: entry.new_id.map(ObjectId::new),
                new_version: entry.new_version,
            })
            .collect(),
    }
}

fn conflict_from_dto(report: ConflictDto) -> neo_api::ConflictReport {
    neo_api::ConflictReport {
        message: report.message,
        objects: report
            .objects
            .into_iter()
            .map(|object| neo_api::ConflictObject {
                kind: kind_from_name(&object.kind),
                old_id: ObjectId::new(object.old_id),
                new_id: object.new_id.map(ObjectId::new),
                version: object.version,
            })
            .collect(),
    }
}
