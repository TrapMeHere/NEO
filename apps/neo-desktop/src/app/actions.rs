//! NeoApp::actions 方法。行为与拆分前的 app.rs 相同。

use anyhow::{Context, Result};
use neo_core::{LatLon, ObjectId, ObjectKind};
use neo_editor::{Editor, RelationMemberSpec, SetRelationMembers};
use neo_geo::{Camera, WorldPoint};
use neo_i18n::{t, tf};
use neo_io::{
    PbfWriteOptions, XmlWriteMode, collect_changeset, write_changeset, write_gpx, write_osm_xml_file,
    write_pbf_file,
};
use neo_ipc::{SyncRequest, SyncResponse};
use neo_persist::{
    Session, SessionLayer, SessionViewport, read_jos_file, read_joz, write_jos_file, write_joz,
};
use neo_store::DataSet;
use neo_tools::ToolKind;
use neo_ui::{RelationMemberEdit, UiActions, UiMode};
use std::path::PathBuf;

use super::{NeoApp, NotesRequest, autosave_dir, data_dir, kind_from_name, load_dataset_file};

/// 把 ID 列表压成一行提示文本（最多列 5 个，其余省略）。
fn id_list(ids: &[ObjectId]) -> String {
    const MAX: usize = 5;
    let mut shown: Vec<String> = ids.iter().take(MAX).map(|id| id.0.to_string()).collect();
    if ids.len() > MAX {
        shown.push("…".to_owned());
    }
    shown.join(", ")
}

impl NeoApp {
    pub(super) fn apply_actions(&mut self, mut actions: UiActions) -> Result<()> {
        if self.imagery.enabled {
            self.imagery_listed = true;
        }
        // 打开/下载均合并进图层，不覆盖当前编辑，故不进 destructive。
        let destructive = actions.open_session || actions.recover_journal;
        if destructive && !self.confirm_discard() {
            return Ok(());
        }
        // 非活动图层是只读的：画布已拦截，但菜
        // 快捷
        // 标签面板也必须拦截。没有数据图层时同样不能改数据。
        if !self.data_listed || !self.layer_active {
            actions.undo = false;
            actions.redo = false;
            actions.undo_to = None;
            actions.finish_way = false;
            actions.tag_edit = None;
            actions.relation_member_edit = None;
            actions.split_way = false;
            actions.reverse_way = false;
            actions.merge_ways = false;
            actions.simplify_way = false;
            actions.merge_nodes = false;
            actions.align_nodes = false;
            actions.distribute_nodes = false;
            actions.mirror_horizontal = false;
            actions.mirror_vertical = false;
            actions.unglue = false;
            actions.parallel_way = false;
            actions.orthogonalize = false;
            actions.join_areas = false;
            actions.move_node_onto_way = false;
            actions.extrude = false;
            actions.delete_selected = false;
            actions.run_validation = false;
        }
        self.persist_account_settings();
        if actions.toggle_download {
            self.show_download = !self.show_download;
        }
        if actions.begin_bbox {
            self.download_state.selecting = true;
            self.download_state.bbox = None;
            self.bbox_drag = None;
        }
        if actions.clear_bbox {
            self.download_state.bbox = None;
            self.download_state.selecting = false;
            self.bbox_drag = None;
        }
        if actions.download_bbox
            && let Some(bbox) = self.download_state.bbox
        {
            self.start_api_download((bbox.min_lon, bbox.min_lat, bbox.max_lon, bbox.max_lat));
            self.show_download = false;
            self.download_state.bbox = None;
            self.download_state.selecting = false;
            self.bbox_drag = None;
        }
        if actions.download_overpass {
            let endpoint = self.download_state.overpass_endpoint.clone();
            let mut query = self.download_state.overpass_query.clone();
            let overpass_bbox = self.download_state.bbox;
            if let Some(bbox) = overpass_bbox {
                let coords = format!(
                    "{:.7},{:.7},{:.7},{:.7}",
                    bbox.min_lat, bbox.min_lon, bbox.max_lat, bbox.max_lon
                );
                query = query.replace("{{bbox}}", &coords);
            }
            self.start_overpass_download(endpoint, query, overpass_bbox);
            self.show_download = false;
            self.download_state.bbox = None;
            self.download_state.selecting = false;
            self.bbox_drag = None;
        }
        if actions.toggle_export {
            self.show_export = !self.show_export;
        }
        if let Some(format) = actions.export_now.take() {
            match format {
                neo_ui::ExportFormat::Xml => actions.save_xml = true,
                neo_ui::ExportFormat::Osc => actions.save_osc = true,
                neo_ui::ExportFormat::Pbf => actions.save_pbf = true,
                neo_ui::ExportFormat::Session => actions.save_session = true,
            }
        }
        if actions.toggle_sync {
            self.show_sync = !self.show_sync;
        }
        if actions.sync_upload {
            self.start_sync_upload(false);
        }
        if actions.sync_upload_selected {
            self.start_sync_upload(true);
        }
        if let Some((kind, id)) = actions.sync_take_remote {
            if self.sync_busy {
                self.sync_status = t("sync.busy").to_owned();
            } else {
                let result = (|| -> Result<(), String> {
                    let xml = self.download_object_xml(kind, id)?;
                    let mut remote = DataSet::new();
                    neo_io::read_osm_xml(&mut remote, xml.as_bytes(), neo_io::XmlReadOptions::default())
                        .map_err(|err| err.to_string())?;
                    self.editor
                        .replace_from_remote(kind, id, &remote)
                        .map_err(|err| err.to_string())?;
                    self.sync_conflicts
                        .retain(|conflict| !(conflict.kind == kind && conflict.id == id));
                    if let Some(report) = &mut self.sync_conflict_report {
                        report
                            .objects
                            .retain(|object| !(object.kind == kind && object.old_id == id));
                    }
                    Ok(())
                })();
                match result {
                    Ok(()) => {
                        self.scene = None;
                        self.sync_status = tf("status.take_remote", &[("id", &id.to_string())]);
                    }
                    Err(err) => {
                        self.sync_status = tf("app.error.sync", &[("err", &err)]);
                    }
                }
            }
        }
        if let Some((kind, id)) = actions.sync_merge_remote {
            if self.sync_busy {
                self.sync_status = t("sync.busy").to_owned();
            } else {
                let result = (|| -> Result<(), String> {
                    let xml = self.download_object_xml(kind, id)?;
                    let mut remote = DataSet::new();
                    neo_io::read_osm_xml(&mut remote, xml.as_bytes(), neo_io::XmlReadOptions::default())
                        .map_err(|err| err.to_string())?;
                    self.editor
                        .merge_from_remote(kind, id, &remote)
                        .map_err(|err| err.to_string())?;
                    self.sync_conflicts
                        .retain(|conflict| !(conflict.kind == kind && conflict.id == id));
                    if let Some(report) = &mut self.sync_conflict_report {
                        report
                            .objects
                            .retain(|object| !(object.kind == kind && object.old_id == id));
                    }
                    Ok(())
                })();
                match result {
                    Ok(()) => {
                        self.scene = None;
                        self.sync_status = tf("status.take_remote", &[("id", &id.to_string())]);
                    }
                    Err(err) => {
                        self.sync_status = tf("app.error.sync", &[("err", &err)]);
                    }
                }
            }
        }
        if let Some((kind, id)) = actions.sync_merge_geometry {
            if self.sync_busy {
                self.sync_status = t("sync.busy").to_owned();
            } else {
                let result = (|| -> Result<(), String> {
                    let xml = self.download_object_xml(kind, id)?;
                    let mut remote = DataSet::new();
                    neo_io::read_osm_xml(&mut remote, xml.as_bytes(), neo_io::XmlReadOptions::default())
                        .map_err(|err| err.to_string())?;
                    self.editor
                        .merge_geometry_from_remote(kind, id, &remote)
                        .map_err(|err| err.to_string())?;
                    self.sync_conflicts
                        .retain(|conflict| !(conflict.kind == kind && conflict.id == id));
                    if let Some(report) = &mut self.sync_conflict_report {
                        report
                            .objects
                            .retain(|object| !(object.kind == kind && object.old_id == id));
                    }
                    Ok(())
                })();
                match result {
                    Ok(()) => {
                        self.scene = None;
                        self.sync_status = tf("status.take_remote", &[("id", &id.to_string())]);
                    }
                    Err(err) => {
                        self.sync_status = tf("app.error.sync", &[("err", &err)]);
                    }
                }
            }
        }
        if actions.sync_keep_local {
            let report = self.sync_conflict_report.clone();
            if let Some(report) = report {
                let mut aligned = 0usize;
                for object in &report.objects {
                    let Some(version) = object.version else { continue };
                    if version > 0
                        && self
                            .editor
                            .set_object_version(object.kind, object.old_id, version as u32)
                    {
                        aligned += 1;
                    }
                }
                self.sync_conflicts.clear();
                self.sync_conflict_report = None;
                self.sync_status = tf("status.sync_keep_local", &[("n", &aligned.to_string())]);
                self.start_sync_upload(false);
            }
        }
        if actions.oauth_login {
            self.start_oauth();
        }
        if actions.oauth_logout {
            self.account_logout();
        }
        if let Some(id) = actions.oauth_select {
            self.account_select(&id);
        }
        if let Some(id) = actions.oauth_remove {
            self.account_remove(&id);
        }
        if actions.toggle_help {
            self.show_help = !self.show_help;
        }
        if actions.toggle_search {
            self.show_search = !self.show_search;
        }
        if actions.toggle_notes {
            self.show_notes = !self.show_notes;
            if self.show_notes && self.notes.notes.is_empty() && !self.notes.busy {
                self.spawn_notes(NotesRequest::Refresh);
            }
        }
        if actions.toggle_plugins {
            self.show_plugins = !self.show_plugins;
            if self.show_plugins {
                self.refresh_plugins();
            }
        }
        if actions.toggle_imagery {
            self.show_imagery = !self.show_imagery;
            self.persist_state();
        }
        if actions.toggle_changeset {
            self.show_changeset = !self.show_changeset;
        }
        if let Some(id) = actions.changeset_load {
            let response = self.sync_sidecar.as_ref().and_then(|sidecar| {
                sidecar.lock().ok().and_then(|mut guard| {
                    guard
                        .call(SyncRequest::Changeset {
                            api_base: self.api_base(),
                            id,
                        })
                        .ok()
                })
            });
            match response {
                Some(SyncResponse::ChangesetInfo {
                    id,
                    user,
                    created_at,
                    closed_at,
                    open,
                    comment,
                    changes,
                    tags,
                }) => {
                    self.changeset.info = Some(neo_ui::ChangesetView {
                        id,
                        user,
                        created_at,
                        closed_at,
                        open,
                        comment,
                        changes,
                        tags,
                    });
                    self.changeset.status.clear();
                }
                Some(SyncResponse::Error { message }) => {
                    self.changeset.status = tf("changeset.error", &[("err", &message)]);
                }
                Some(_) => {
                    self.changeset.status = tf("changeset.error", &[("err", "unexpected sync reply")]);
                }
                None => {
                    self.changeset.status = tf("changeset.error", &[("err", "neo-sync unavailable")]);
                }
            }
        }
        if actions.imagery_apply {
            if !self.imagery.url.trim().is_empty() && !self.imagery_valid() {
                self.imagery.status = t("imagery.invalid_url").to_owned();
            } else {
                self.imagery_tiles
                    .retain(|key, _| key.source != super::SOURCE_CUSTOM);
                self.imagery_pending
                    .retain(|key| key.source != super::SOURCE_CUSTOM);
                self.wms_texture = None;
                self.wms_key.clear();
                self.wms_bbox = None;
                self.imagery.enabled = true;
                self.imagery_listed = true;
                self.imagery.status = t("imagery.loading").replace("{n}", "0");
                self.persist_state();
            }
        }
        if actions.imagery_clear {
            self.imagery_tiles.clear();
            self.imagery_pending.clear();
            self.wms_texture = None;
            self.wms_key.clear();
            self.wms_bbox = None;
            self.imagery.cached = 0;
            self.imagery.status.clear();
        }
        if actions.imagery_offset_load
            && let Some(path) = rfd::FileDialog::new().add_filter("JSON", &["json"]).pick_file()
        {
            match neo_style::imagery::OffsetDb::load(&path) {
                Ok((db, skipped)) => {
                    self.imagery_offsets = db;
                    self.imagery.status = tf(
                        "imagery.offset_loaded",
                        &[
                            ("n", &self.imagery_offsets.entries.len().to_string()),
                            ("skipped", &skipped.to_string()),
                        ],
                    );
                }
                Err(err) => {
                    self.imagery.status = tf("imagery.error", &[("err", &err)]);
                }
            }
        }
        if actions.imagery_offset_save
            && let Some(path) = rfd::FileDialog::new()
                .set_file_name("imagery-offsets.json")
                .add_filter("JSON", &["json"])
                .save_file()
        {
            match self.imagery_offsets.save(&path) {
                Ok(()) => {
                    self.imagery.status =
                        tf("imagery.offset_saved", &[("path", &path.display().to_string())]);
                }
                Err(err) => {
                    self.imagery.status = tf("imagery.error", &[("err", &err)]);
                }
            }
        }
        if actions.imagery_apply_offset {
            let center = self.camera.center.to_latlon();
            let lat = center.lat.to_degrees();
            let lon = center.lon.to_degrees();
            let source = self.imagery.url.clone();
            if let Some(offset) = self.imagery_offsets.nearest(&source, lat, lon) {
                self.imagery.offset_x = offset.dx_px as f32;
                self.imagery.offset_y = offset.dy_px as f32;
                self.imagery.status = t("imagery.offset_applied").to_owned();
            } else {
                self.imagery.status = t("imagery.offset_none").to_owned();
            }
        }
        if actions.plugin_refresh {
            self.refresh_plugins();
        }
        if actions.plugin_install
            && let Some(path) = rfd::FileDialog::new()
                .add_filter("NEO plugin", &["neopkg"])
                .pick_file()
        {
            self.install_plugin(&path);
        }
        if let Some(id) = actions.plugin_remove {
            self.remove_plugin(&id);
        }
        if let Some((id, enabled)) = actions.plugin_toggle {
            if enabled {
                self.enabled_plugins.insert(id.clone());
            } else {
                self.enabled_plugins.remove(&id);
            }
            if let Some(plugin) = self.plugins.plugins.iter_mut().find(|plugin| plugin.id == id) {
                plugin.enabled = enabled;
            }
            if enabled {
                self.run_plugin(&id);
            } else {
                self.plugin_host = None;
            }
            self.persist_state();
        }
        if actions.notes_refresh {
            self.spawn_notes(NotesRequest::Refresh);
        }
        if actions.note_create {
            let text = self.notes.text.clone();
            if !text.trim().is_empty() {
                let center = self.camera.center.to_latlon();
                self.spawn_notes(NotesRequest::Create {
                    lat: center.lat.to_degrees(),
                    lon: center.lon.to_degrees(),
                    text,
                });
            }
        }
        if let Some(id) = actions.note_comment {
            let text = self.notes.text.clone();
            if !text.trim().is_empty() {
                self.spawn_notes(NotesRequest::Comment { id, text });
            }
        }
        if let Some(id) = actions.note_close {
            let text = self.notes.text.clone();
            self.spawn_notes(NotesRequest::Close { id, text });
        }
        if let Some((lat, lon)) = actions.note_focus
            && let Some(pos) = LatLon::try_from_degrees(lat, lon)
        {
            self.camera.center = WorldPoint::from_latlon(pos);
            if self.camera.zoom < 17.0 {
                self.camera.set_zoom(17.0);
            }
            self.scene = None;
        }
        if actions.toggle_ecosystem {
            self.show_ecosystem = !self.show_ecosystem;
        }
        if actions.layer_info {
            self.show_layer_info = true;
        }
        if actions.toggle_layer_visible {
            self.layer_visible = !self.layer_visible;
        }
        if actions.toggle_layer_active {
            self.layer_active = !self.layer_active;
            if !self.layer_active {
                self.editor.selection_mut().clear();
                self.drag = None;
                self.measure_start = None;
                self.measure_end = None;
                self.status = t("status.layer_inactive").to_owned();
            } else {
                self.status = t("status.layer_active").to_owned();
            }
        }
        if actions.imagery_add_osm {
            self.show_osm_layer();
        }
        if let Some(id) = actions.imagery_pick {
            self.show_imagery_preset(&id);
        }
        if let Some(index) = actions.layer_activate {
            self.activate_listed_layer(index);
        }
        if let Some((index, visible)) = actions.layer_set_visible {
            self.set_layer_visible(index, visible);
        }
        if actions.layer_new {
            self.new_layer();
        }
        if let Some(index) = actions.layer_remove {
            if self.needs_unsaved_prompt(self.layer_has_unsaved(index)) {
                self.open_unsaved_prompt(super::UnsavedPromptKind::CloseLayer(index));
            } else {
                self.remove_layer(index);
            }
        }
        if let Some(index) = actions.layer_import
            && let Some(position) = self.parked_slot(index)
            && position < self.parked_layers.len()
        {
            // 临时取出停靠层，避免与活动 editor 同时借用。
            let layer = self.parked_layers.remove(position);
            let result = self.editor.merge_from_dataset(layer.editor.data());
            let imported_bboxes = layer.downloaded_bboxes.clone();
            self.parked_layers.insert(position, layer);
            match result {
                Ok(stats) => {
                    for bbox in imported_bboxes {
                        self.record_download_bbox(bbox);
                    }
                    self.scene = None;
                    self.status = tf(
                        "status.layer_merged",
                        &[
                            ("added", &stats.added.to_string()),
                            ("updated", &stats.updated.to_string()),
                            ("skipped", &stats.skipped.to_string()),
                        ],
                    );
                }
                Err(err) => {
                    self.status = tf("app.error.generic", &[("err", &err.to_string())]);
                }
            }
        }
        if actions.toggle_settings {
            self.show_settings = !self.show_settings;
            if !self.show_settings {
                self.persist_state();
            }
        }
        if actions.open_data_dir {
            let dir = data_dir();
            let _ = std::fs::create_dir_all(&dir);
            #[cfg(target_os = "windows")]
            let _ = std::process::Command::new("explorer").arg(&dir).spawn();
            #[cfg(target_os = "macos")]
            let _ = std::process::Command::new("open").arg(&dir).spawn();
            #[cfg(all(unix, not(target_os = "macos")))]
            let _ = std::process::Command::new("xdg-open").arg(&dir).spawn();
        }
        if actions.clear_autosave {
            let dir = autosave_dir();
            let mut removed = 0usize;
            if let Ok(entries) = std::fs::read_dir(&dir) {
                for entry in entries.flatten() {
                    if std::fs::remove_file(entry.path()).is_ok() {
                        removed += 1;
                    }
                }
            }
            self.status = tf("status.autosave_cleared", &[("n", &removed.to_string())]);
        }
        if actions.run_validation {
            let report = neo_validate::validate(self.editor.data());
            self.status = tf(
                "status.validation_done",
                &[
                    ("errors", &report.errors().to_string()),
                    ("warnings", &report.warnings().to_string()),
                ],
            );
            self.validation = Some(report);
        }
        if let Some(objects) = actions.select_objects
            && let Some((kind, id)) = objects.first().copied()
        {
            self.editor.selection_mut().clear();
            for (kind, id) in &objects {
                self.editor.selection_mut().insert(*kind, *id);
            }
            self.focus_object(kind, id);
            self.status = tf("status.selected", &[("n", &objects.len().to_string())]);
        }
        if let Some((kind, id)) = actions.focus_object {
            self.focus_object(kind, id);
        }
        if let Some(lang) = actions.set_language {
            neo_i18n::set_language(lang);
        }
        if (actions.open_file || actions.open_pbf || actions.open_xml)
            && let Some(path) = rfd::FileDialog::new()
                .add_filter("OSM data", &["osm", "xml", "pbf"])
                .pick_file()
        {
            let pbf = path
                .extension()
                .and_then(|ext| ext.to_str())
                .is_some_and(|ext| ext.eq_ignore_ascii_case("pbf"));
            self.start_open_dataset(path, pbf);
        }
        if actions.open_gpx
            && let Some(path) = rfd::FileDialog::new().add_filter("GPX", &["gpx"]).pick_file()
        {
            self.import_gpx(&path);
        }
        if actions.save_gpx
            && let Some(path) = rfd::FileDialog::new()
                .set_file_name("neo-track.gpx")
                .add_filter("GPX", &["gpx"])
                .save_file()
        {
            let text = write_gpx(self.editor.data());
            match std::fs::write(&path, text) {
                Ok(()) => {
                    self.status = tf("app.status.gpx_saved", &[("path", &path.display().to_string())]);
                }
                Err(err) => {
                    self.status = tf("app.error.gpx", &[("err", &err.to_string())]);
                }
            }
        }
        if actions.save_xml
            && let Some(path) = rfd::FileDialog::new()
                .set_file_name("neo-export.osm")
                .add_filter("OSM XML", &["osm"])
                .save_file()
        {
            write_osm_xml_file(self.editor.data(), &path, XmlWriteMode::Josm)
                .with_context(|| tf("app.error.save", &[("path", &path.display().to_string())]))?;
            self.editor.history_mut().clear_dirty();
            // 保存后的文件成为新的恢复基线，journal 从零开始
            self.data_path = Some(path.clone());
            self.editor.reset_journal();
            self.persist_state();
            self.status = tf("app.status.saved_xml", &[("path", &path.display().to_string())]);
        }
        if actions.save_osc
            && let Some(path) = rfd::FileDialog::new()
                .set_file_name("neo-changes.osc")
                .add_filter("osmChange", &["osc"])
                .save_file()
        {
            let set = collect_changeset(self.editor.data())?;
            let mut buffer = Vec::new();
            write_changeset(&set, self.editor.data(), &mut buffer)?;
            std::fs::write(&path, buffer)?;
            self.status = tf(
                "app.status.exported_osc",
                &[
                    ("n", &set.len().to_string()),
                    ("path", &path.display().to_string()),
                ],
            );
        }
        if actions.save_pbf
            && let Some(path) = rfd::FileDialog::new()
                .set_file_name("neo-export.pbf")
                .add_filter("OSM PBF", &["pbf"])
                .save_file()
        {
            let report = write_pbf_file(self.editor.data(), &path, PbfWriteOptions::default())
                .with_context(|| tf("app.error.export_pbf", &[("path", &path.display().to_string())]))?;
            self.editor.history_mut().clear_dirty();
            self.data_path = Some(path.clone());
            self.editor.reset_journal();
            self.persist_state();
            self.status = format!("{report} -> {}", path.display());
        }
        if actions.save_session {
            let default_name = if self.data_path.is_some() {
                "neo-session.joz"
            } else {
                "neo-session.jos"
            };
            if let Some(path) = rfd::FileDialog::new()
                .set_file_name(default_name)
                .add_filter("JOSM session", &["jos", "joz"])
                .save_file()
            {
                // 会话必须包含当前内存中的编辑（不能只引用磁盘上的旧文件）
                let data_file = path.with_file_name(format!(
                    "{}.data.osm",
                    path.file_stem()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_else(|| "neo-session".into())
                ));
                let layer_file = match write_osm_xml_file(self.editor.data(), &data_file, XmlWriteMode::Josm)
                {
                    Ok(()) => Some(data_file),
                    Err(err) => {
                        self.status = tf("app.error.session_save", &[("err", &err.to_string())]);
                        return Ok(());
                    }
                };
                let center = self.camera.center.to_latlon();
                let session = Session {
                    layers: vec![SessionLayer {
                        name: "data".into(),
                        layer_type: "osm-data".into(),
                        file: layer_file,
                    }],
                    viewport: Some(SessionViewport {
                        center_lat: center.lat.to_degrees(),
                        center_lon: center.lon.to_degrees(),
                        zoom: self.camera.zoom,
                    }),
                    active_layer: Some(0),
                    selection: self.selection_items(),
                };
                let is_joz = path
                    .extension()
                    .and_then(|e| e.to_str())
                    .map(|e| e.eq_ignore_ascii_case("joz"))
                    .unwrap_or(false);
                let result = if is_joz {
                    write_joz(&session, &path)
                } else {
                    write_jos_file(&session, &path)
                };
                self.status = match result {
                    Ok(()) => tf(
                        "app.status.session_saved",
                        &[("path", &path.display().to_string())],
                    ),
                    Err(err) => tf("app.error.session_save", &[("err", &err.to_string())]),
                };
            }
        }
        if actions.open_session
            && let Some(path) = rfd::FileDialog::new()
                .add_filter("JOSM session", &["jos", "joz"])
                .pick_file()
        {
            let is_joz = path
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| e.eq_ignore_ascii_case("joz"))
                .unwrap_or(false);
            let session = if is_joz {
                let name = path
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "session".into());
                match read_joz(&path, PathBuf::from("out/session-extract").join(name)) {
                    Ok(session) => session,
                    Err(err) => {
                        self.status = tf("app.error.read_joz", &[("err", &err.to_string())]);
                        return Ok(());
                    }
                }
            } else {
                match read_jos_file(&path) {
                    Ok(session) => session,
                    Err(err) => {
                        self.status = tf("app.error.read_jos", &[("err", &err.to_string())]);
                        return Ok(());
                    }
                }
            };
            let Some(layer) = session.first_data_layer().cloned() else {
                self.status = t("app.error.session_no_layer").into();
                return Ok(());
            };
            let Some(file) = layer.file else {
                self.status = t("app.error.session_no_file").into();
                return Ok(());
            };
            match load_dataset_file(&file) {
                Ok((data, label)) => {
                    self.data_path = Some(file.clone());
                    self.set_editor(Editor::new(data), label);
                    self.reopen_journal();
                    if let Some(viewport) = session.viewport {
                        self.camera = Camera::new(
                            WorldPoint::from_latlon(LatLon::from_degrees(
                                viewport.center_lat,
                                viewport.center_lon,
                            )),
                            viewport.zoom,
                            self.camera.viewport,
                        );
                        self.scene = None;
                    }
                    self.restore_selection(&session.selection);
                    self.status = tf(
                        "app.status.session_opened",
                        &[("path", &path.display().to_string())],
                    );
                }
                Err(err) => self.status = tf("app.error.session_load", &[("err", &err.to_string())]),
            }
        }
        if actions.reset_view {
            self.reset_view();
        }
        if actions.zoom_in {
            self.camera.set_zoom(self.camera.zoom + 1.0);
        }
        if actions.zoom_out {
            self.camera.set_zoom(self.camera.zoom - 1.0);
        }
        if let Some(mode) = actions.mode {
            self.mode = mode;
            self.controller.set_tool(match mode {
                UiMode::Select | UiMode::Measure | UiMode::SplitPoint => ToolKind::Select,
                UiMode::DrawPoint => ToolKind::DrawPoint,
                UiMode::DrawLine => ToolKind::DrawLine,
                UiMode::DrawArea => ToolKind::DrawArea,
            });
            if mode == UiMode::Measure {
                // 进入测距时重置起点
                self.measure_start = None;
                self.measure_end = None;
            }
            self.status = tf("app.status.tool", &[("tool", self.mode_label())]);
        }
        if actions.finish_way {
            let outcome = self.controller.finish_way(&mut self.editor);
            self.apply_outcome(outcome);
            if self.controller.take_finished_feature() {
                self.switch_to_select_tool();
            }
        }
        if actions.undo {
            match self.editor.undo() {
                Ok(true) => self.status = t("app.status.undone").into(),
                Ok(false) => self.status = t("app.status.nothing_to_undo").into(),
                Err(err) => self.status = tf("app.error.undo", &[("err", &err.to_string())]),
            }
            self.resync_tags();
            self.scene = None;
        }
        if actions.redo {
            match self.editor.redo() {
                Ok(true) => self.status = t("app.status.redone").into(),
                Ok(false) => self.status = t("app.status.nothing_to_redo").into(),
                Err(err) => self.status = tf("app.error.redo", &[("err", &err.to_string())]),
            }
            self.resync_tags();
            self.scene = None;
        }
        if let Some(steps) = actions.undo_to {
            let mut done = 0usize;
            for _ in 0..steps {
                match self.editor.undo() {
                    Ok(true) => done += 1,
                    Ok(false) => break,
                    Err(err) => {
                        self.status = tf("app.error.undo", &[("err", &err.to_string())]);
                        break;
                    }
                }
            }
            if done > 0 {
                self.status = tf("status.undone_n", &[("n", &done.to_string())]);
                self.resync_tags();
                self.scene = None;
            }
        }
        if actions.copy_selection {
            self.copy_selection();
        }
        if actions.paste {
            self.paste_clipboard(15.0);
        }
        if actions.duplicate {
            if self.editor.selection().is_empty() {
                self.status = t("app.error.no_selection").into();
            } else {
                self.copy_selection();
                self.paste_clipboard(20.0);
            }
        }
        if let Some(edit) = actions.relation_member_edit.take() {
            let relation = match &edit {
                RelationMemberEdit::Add { relation }
                | RelationMemberEdit::Remove { relation, .. }
                | RelationMemberEdit::MoveUp { relation, .. }
                | RelationMemberEdit::MoveDown { relation, .. }
                | RelationMemberEdit::SetRole { relation, .. } => *relation,
            };
            match self.editor.relation_members(relation) {
                Ok(mut members) => {
                    let mut changed = true;
                    match edit {
                        RelationMemberEdit::Add { .. } => {
                            let role = self.tags.relation_role().to_owned();
                            let selection = self.editor.selection();
                            let mut added = 0usize;
                            for id in selection.nodes() {
                                members.push(RelationMemberSpec {
                                    kind: ObjectKind::Node,
                                    id: *id,
                                    role: role.clone(),
                                });
                                added += 1;
                            }
                            for id in selection.ways() {
                                members.push(RelationMemberSpec {
                                    kind: ObjectKind::Way,
                                    id: *id,
                                    role: role.clone(),
                                });
                                added += 1;
                            }
                            for id in selection.relations() {
                                if *id != relation {
                                    members.push(RelationMemberSpec {
                                        kind: ObjectKind::Relation,
                                        id: *id,
                                        role: role.clone(),
                                    });
                                    added += 1;
                                }
                            }
                            if added == 0 {
                                self.status = t("app.error.no_selection").to_owned();
                                changed = false;
                            }
                        }
                        RelationMemberEdit::Remove { index, .. } => {
                            if index < members.len() {
                                members.remove(index);
                            } else {
                                changed = false;
                            }
                        }
                        RelationMemberEdit::MoveUp { index, .. } => {
                            if index > 0 && index < members.len() {
                                members.swap(index, index - 1);
                            } else {
                                changed = false;
                            }
                        }
                        RelationMemberEdit::MoveDown { index, .. } => {
                            if index + 1 < members.len() {
                                members.swap(index, index + 1);
                            } else {
                                changed = false;
                            }
                        }
                        RelationMemberEdit::SetRole { index, role, .. } => {
                            if let Some(member) = members.get_mut(index) {
                                member.role = role;
                            } else {
                                changed = false;
                            }
                        }
                    }
                    if changed {
                        match self
                            .editor
                            .execute(Box::new(SetRelationMembers::new(relation, members)))
                        {
                            Ok(()) => {
                                self.status = t("status.members_updated").to_owned();
                                self.scene = None;
                            }
                            Err(err) => {
                                self.status = tf("app.error.members_update", &[("err", &err.to_string())]);
                            }
                        }
                    }
                }
                Err(err) => {
                    self.status = tf("app.error.members_update", &[("err", &err.to_string())]);
                }
            }
        }
        if actions.delete_selected {
            let targets = self.selection_items();
            if targets.is_empty() {
                self.status = t("app.error.no_selection").into();
            } else {
                let mut wanted: Vec<(ObjectKind, ObjectId)> = Vec::new();
                for item in &targets {
                    let Some(kind) = kind_from_name(item.kind) else {
                        continue;
                    };
                    let id = ObjectId(item.id);
                    if !wanted.contains(&(kind, id)) {
                        wanted.push((kind, id));
                    }
                }
                // 级联删除（JOSM 语义）：节点被 way 引用时不再直接拒绝，而是把引用它的
                // way 一并修正；若 way 会因此不足 2 个节点，则整条删除 —— 这种破坏性
                // 结果需要用户再按一次删除确认，避免误删一条道路。
                match neo_editor::plan_delete(&mut self.editor, &wanted) {
                    Ok(plan) => {
                        let needs_confirm = !plan.reaped_ways.is_empty()
                            && self.pending_delete.as_deref() != Some(wanted.as_slice());
                        if needs_confirm {
                            self.status = tf(
                                "app.status.delete_confirm_ways",
                                &[
                                    ("n", &plan.reaped_ways.len().to_string()),
                                    ("ways", &id_list(&plan.reaped_ways)),
                                ],
                            );
                            self.pending_delete = Some(wanted);
                        } else {
                            self.pending_delete = None;
                            let reaped = plan.reaped_ways.len();
                            let total = wanted.len();
                            match self.editor.execute_batch(plan.commands) {
                                Ok(()) => {
                                    for (kind, id) in &wanted {
                                        self.editor.selection_mut().remove(*kind, *id);
                                    }
                                    self.status = if reaped == 0 {
                                        tf("app.status.deleted", &[("ok", &total.to_string())])
                                    } else {
                                        tf(
                                            "app.status.deleted_cascade",
                                            &[("ok", &total.to_string()), ("ways", &reaped.to_string())],
                                        )
                                    };
                                    self.resync_tags();
                                    self.scene = None;
                                }
                                Err(err) => self.status = err.to_string(),
                            }
                        }
                    }
                    Err(err) => self.status = err.to_string(),
                }
            }
        }
        if let Some(edit) = actions.tag_edit {
            self.apply_tag_edit(edit);
        }
        if actions.simplify_way
            && let Some((ObjectKind::Way, way)) = self.selection_target()
            && let Some(handle) = self.editor.data().way_handle(way)
        {
            // 反查表必须是新的：下面要判断哪些节点被 relation / 其它 way 引用。
            self.editor.refresh_referrers();
            let nodes = self.editor.data().way_node_ids(handle);
            let mut points: Vec<WorldPoint> = Vec::with_capacity(nodes.len());
            for id in &nodes {
                let Some(node_handle) = self.editor.data().node_handle(*id) else {
                    break;
                };
                let Some(pos) = self.editor.data().nodes.position(node_handle) else {
                    break;
                };
                points.push(WorldPoint::from_latlon(pos));
            }
            // 位置缺失时 points 会比 nodes 短，RDP 下标就会与 nodes 错位 —— 宁可不简化。
            if points.len() == nodes.len() && points.len() > 2 {
                let tolerance = self.camera.meters_per_px() * self.settings.simplify_px.max(0.5);
                let keep = neo_geo::geometry::simplify_rdp_indices(&points, tolerance);
                // 简化不得删掉"有语义"的节点：带 tag 的、被 relation 引用的、与其它
                // way 共享的 —— 否则交叉口/公交站/门会静默消失，几何也会被扯断。
                let mut keep_set: std::collections::BTreeSet<usize> = keep.iter().copied().collect();
                for (index, id) in nodes.iter().enumerate() {
                    if keep_set.contains(&index) {
                        continue;
                    }
                    let Some(node_handle) = self.editor.data().node_handle(*id) else {
                        continue;
                    };
                    let tagged = self
                        .editor
                        .data()
                        .node_view(node_handle)
                        .is_some_and(|view| !view.tag_pairs.is_empty());
                    let in_relation = !self.editor.referrers().relations_of_node(node_handle).is_empty();
                    let shared = self.editor.referrers().ways_of_node(node_handle).len() > 1;
                    if tagged || in_relation || shared {
                        keep_set.insert(index);
                    }
                }
                let simplified: Vec<ObjectId> = keep_set
                    .iter()
                    .filter_map(|index| nodes.get(*index).copied())
                    .collect();
                if simplified.len() >= 2 && simplified.len() < nodes.len() {
                    match self
                        .editor
                        .execute(Box::new(neo_editor::SetWayNodes::new(way, simplified)))
                    {
                        Ok(()) => {
                            self.status = t("status.way_simplified").to_owned();
                            self.scene = None;
                        }
                        Err(err) => {
                            self.status = tf("app.error.generic", &[("err", &err.to_string())]);
                        }
                    }
                } else {
                    self.status = t("status.way_already_simple").to_owned();
                }
            }
        }
        if actions.merge_nodes {
            self.merge_selected_nodes();
        }
        if actions.align_nodes {
            self.align_selected_nodes();
        }
        if actions.distribute_nodes {
            self.distribute_selected_nodes();
        }
        if actions.mirror_horizontal {
            self.mirror_selected_nodes(true);
        }
        if actions.mirror_vertical {
            self.mirror_selected_nodes(false);
        }
        if actions.unglue {
            self.unglue_selected_nodes();
        }
        if actions.parallel_way {
            self.parallel_copy_way();
        }
        if actions.orthogonalize {
            self.orthogonalize_selected();
        }
        if actions.join_areas {
            self.join_selected_areas();
        }
        if actions.move_node_onto_way {
            self.move_node_onto_way();
        }
        if actions.extrude {
            self.extrude_selected_way();
        }
        if actions.reverse_way
            && let Some((ObjectKind::Way, way)) = self.selection_target()
        {
            let outcome = self.controller.reverse_way(&mut self.editor, way);
            self.apply_outcome(outcome);
            self.scene = None;
        }
        if actions.merge_ways {
            let ways: Vec<ObjectId> = self.editor.selection().ways().iter().copied().collect();
            let outcome = self.controller.merge_chain_ways(&mut self.editor, &ways);
            self.apply_outcome(outcome);
            self.scene = None;
        }
        if actions.recover_journal {
            self.recover_from_journal()?;
        }
        Ok(())
    }
}
