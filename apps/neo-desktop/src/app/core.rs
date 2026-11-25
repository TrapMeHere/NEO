//! NeoApp::core methods. Same behavior as before the app.rs split.

use anyhow::{Context, Result};
use neo_core::{ObjectId, ObjectKind};
use neo_editor::Editor;
use neo_geo::{Camera, WorldBounds, WorldPoint};
use neo_i18n::{t, tf};
use neo_render::MapGpu;
use neo_tools::{EditController, Outcome, ToolKind};
use neo_ui::{TagsEditor, UiMode, UiState};
use std::sync::Arc;
use std::time::Instant;

use super::{
    AppState, Clipboard, NeoApp, clear_editing_records, data_dir, init_editor, install_fonts, journal_path,
    load_state, node_lod_band, save_state,
};

impl NeoApp {
    /// Constructor: takes over wgpu, restores the
    /// journal (if any), and loads the initial data
    pub fn new(cc: &eframe::CreationContext<'_>) -> Result<Self> {
        let render_state = cc.wgpu_render_state.as_ref().context(t("app.error.no_wgpu"))?;
        neo_ui::apply_modern_style(&cc.egui_ctx);
        install_fonts(&cc.egui_ctx);
        let device = render_state.device.clone();
        // wgpu's default uncaught-error handler panics; log instead so the user can at least save
        // data and see the error, rather than the process vanishing with no console
        device.on_uncaptured_error(std::sync::Arc::new(|err| {
            tracing::error!("wgpu uncaptured error: {err}");
        }));
        let gpu = MapGpu::new(&device, render_state.target_format, 1);
        render_state.renderer.write().callback_resources.insert(gpu);

        clear_editing_records();
        let journal_path = journal_path();
        let state = load_state();
        let (editor, data_label) = init_editor(&journal_path)?;
        let mut editor = editor;
        let builder = super::scene_builder(&mut editor);
        let (imagery_tx, imagery_rx) = std::sync::mpsc::channel();
        let (wms_tx, wms_rx) = std::sync::mpsc::channel();
        let mut app = Self {
            editor,
            camera: Camera::new(WorldPoint::from_degrees(47.37, 8.54), 14.0, [1600, 1000]),
            builder,
            scene: None,
            controller: EditController::new(ToolKind::Select),
            mode: UiMode::Select,
            tags: TagsEditor::new(),
            status: String::new(),
            last_scene_micros: 0,
            data_label,
            pending_delete: None,
            drag: None,
            box_select: None,
            clipboard: Clipboard::default(),
            journal_path,
            data_path: None,
            show_help: std::env::var("NEO_SHOW_HELP").is_ok(),
            show_search: false,
            search: neo_ui::SearchUiState::default(),
            show_notes: false,
            notes: neo_ui::NotesUiState::default(),
            notes_rx: None,
            show_plugins: false,
            plugins: neo_ui::PluginUiState::default(),
            plugin_host: None,
            enabled_plugins: std::collections::HashSet::new(),
            parked_layers: Vec::new(),
            show_imagery: false,
            imagery: neo_ui::ImageryUiState::default(),
            sync_sidecar: crate::sidecar::open_sync().ok(),
            imagery_sidecar: crate::sidecar::open_imagery().ok(),
            account_current: None,
            oauth_pending: false,
            sidecar_client_id: String::new(),
            sidecar_redirect_uri: String::new(),
            imagery_zoom_cache: std::cell::RefCell::new(None),
            osm_visible: false,
            data_listed: false,
            osm_listed: false,
            imagery_listed: false,
            show_changeset: false,
            changeset: neo_ui::ChangesetUiState::default(),
            imagery_tiles: std::collections::HashMap::new(),
            imagery_clock: 0,
            imagery_pending: std::collections::HashSet::new(),
            imagery_tx,
            imagery_rx,
            wms_tx,
            wms_rx,
            wms_texture: None,
            wms_bbox: None,
            wms_key: String::new(),
            wms_pending: false,
            imagery_offsets: neo_style::imagery::OffsetDb::load(data_dir().join("imagery-offsets.json"))
                .map(|(db, _)| db)
                .unwrap_or_default(),
            icon_textures: std::collections::HashMap::new(),
            show_ecosystem: false,
            show_layer_info: std::env::var("NEO_SHOW_LAYER_INFO").is_ok(),
            show_settings: std::env::var("NEO_SHOW_SETTINGS").is_ok(),
            validation: None,
            last_autosave: Instant::now(),
            settings: neo_ui::SettingsUiState::default(),
            open_rx: None,
            open_busy: false,
            autosave_busy: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            show_sync: std::env::var("NEO_SHOW_SYNC").is_ok(),
            sync_comment: String::new(),
            sync_status: String::new(),
            sync_busy: false,
            sync_conflicts: Vec::new(),
            sync_conflict_report: None,
            show_export: std::env::var("NEO_SHOW_EXPORT").is_ok(),
            export_state: neo_ui::ExportUiState::default(),
            show_download: std::env::var("NEO_SHOW_DOWNLOAD").is_ok(),
            download_state: neo_ui::DownloadUiState {
                selecting: std::env::var("NEO_DOWNLOAD_SELECT").is_ok(),
                ..Default::default()
            },
            bbox_drag: None,
            downloaded_bboxes: Vec::new(),
            pending_download_bbox: None,
            sync_rx: None,
            measure_start: None,
            measure_end: None,
            hover_world: None,
            hover_latlon: None,
            snap_hit: None,
            hover_way: None,
            suppress_primary_click: false,
            layer_visible: std::env::var("NEO_LAYER_HIDDEN").is_err(),
            layer_active: std::env::var("NEO_LAYER_INACTIVE").is_err(),
            skip_unsaved_confirm: false,
            unsaved_prompt: None,
            allow_exit: false,
        };
        if std::env::var("NEO_SETTINGS_TAB").as_deref() == Ok("account") {
            app.settings.tab = neo_ui::SettingsTab::Account;
        }
        app.refresh_accounts_from_sidecar();
        if app.sync_sidecar.is_none() {
            app.settings.login_status = tf("settings.account.failed", &[("err", "neo-sync unavailable")]);
        }
        if app.imagery_sidecar.is_none() {
            tracing::warn!("neo-imagery unavailable; imagery downloads disabled");
        }
        app.settings.data_dir = data_dir().display().to_string();
        if let Some(value) = state.autosave_secs {
            app.settings.autosave_secs = value.clamp(10, 600);
        }
        if let Some(skip) = state.skip_unsaved_confirm {
            app.skip_unsaved_confirm = skip;
        }
        app.settings.confirm_unsaved = !app.skip_unsaved_confirm;
        if let Some(value) = state.simplify_px {
            app.settings.simplify_px = value.clamp(0.5, 5.0);
        }
        if let Some(value) = state.draw_snap_enabled {
            app.settings.draw_snap_enabled = value;
            app.controller.snap_enabled = value;
        }
        if let Some(value) = state.draw_snap_radius_px {
            app.settings.draw_snap_radius_px = value.clamp(4.0, 48.0);
        }
        if let Some(value) = &state.api_base {
            app.settings.api_base = value.clone();
        }
        app.enabled_plugins = state.enabled_plugins.iter().cloned().collect();
        if let Some(url) = &state.imagery_url {
            app.imagery.url = url.clone();
        }
        if let Some(opacity) = state.imagery_opacity {
            app.imagery.opacity = opacity.clamp(0.0, 1.0);
        }
        if let Some(preset) = &state.imagery_preset {
            app.imagery.preset_id = preset.clone();
        }
        if let Some(wms) = state.imagery_wms {
            app.imagery.wms = wms;
        }
        if let Some(layers) = &state.imagery_layers {
            app.imagery.wms_layers = layers.clone();
        }
        if let Some(key) = &state.imagery_api_key {
            app.imagery.api_key = key.clone();
        }
        if let Some(attribution) = &state.imagery_attribution {
            app.imagery.attribution = attribution.clone();
        }
        if let Some(crs) = &state.imagery_crs {
            app.imagery.wms_crs = crs.clone();
        }
        if let Some(lat_first) = state.imagery_lat_first {
            app.imagery.wms_lat_first = lat_first;
        }
        if let Some(region_only) = state.imagery_region_only {
            app.imagery.region_only = region_only;
        }
        if let Some(shortcuts) = &state.shortcuts {
            app.settings.shortcuts = shortcuts.clone();
        }
        if let Ok(url) = std::env::var("NEO_IMAGERY_URL") {
            app.imagery.url = url;
            app.imagery.enabled = true;
            app.imagery_listed = true;
            app.show_imagery = true;
        }
        app.data_listed = app.editor.data().node_count() > 0
            || app.editor.data().way_count() > 0
            || app.editor.data().relation_count() > 0;
        if std::env::var("NEO_NEW_LAYER").is_ok() {
            app.new_layer();
        }
        app.refresh_plugins();
        if let Some(camera) = super::saved_camera(&state, app.camera.viewport) {
            app.camera = camera;
        }
        if std::env::var("NEO_RUN_VALIDATION").is_ok() {
            let report = neo_validate::validate(app.editor.data());
            app.status = tf(
                "status.validation_done",
                &[
                    ("errors", &report.errors().to_string()),
                    ("warnings", &report.warnings().to_string()),
                ],
            );
            app.validation = Some(report);
        }
        if let Ok(mode) = std::env::var("NEO_TEST_MODE") {
            app.mode = match mode.as_str() {
                "point" | "draw_point" => UiMode::DrawPoint,
                "line" | "draw" | "draw_line" => UiMode::DrawLine,
                "area" | "draw_area" => UiMode::DrawArea,
                "measure" => UiMode::Measure,
                "split" | "split_point" => UiMode::SplitPoint,
                _ => UiMode::Select,
            };
            app.controller.set_tool(match app.mode {
                UiMode::DrawPoint => ToolKind::DrawPoint,
                UiMode::DrawLine => ToolKind::DrawLine,
                UiMode::DrawArea => ToolKind::DrawArea,
                _ => ToolKind::Select,
            });
        }
        if let Ok(value) = std::env::var("NEO_START_ZOOM")
            && let Ok(zoom) = value.parse::<f64>()
        {
            app.camera.set_zoom(zoom);
        }
        if let Ok(spec) = std::env::var("NEO_TEST_SELECT") {
            match spec.as_str() {
                "way" => {
                    if let Some(way) = app.pick_visible_way() {
                        app.editor.selection_mut().select_only(ObjectKind::Way, way);
                    }
                }
                "node" => {
                    if let Some(node) = app.pick_visible_node() {
                        app.editor.selection_mut().select_only(ObjectKind::Node, node);
                    }
                }
                _ => {}
            }
        }
        app.status = if app.editor.data().node_count() == 0
            && app.editor.data().way_count() == 0
            && app.editor.data().relation_count() == 0
        {
            t("app.status.empty_start").to_owned()
        } else {
            app.data_label.clone()
        };
        Ok(app)
    }

    pub(super) fn set_editor(&mut self, mut editor: Editor, label: String) {
        // Switching datasets must reset the journal baseline; otherwise commands from the old document would be replayed onto the new data.
        // The Editor has no reset sink yet, so clear the file explicitly.
        let _ = std::fs::File::create(&self.journal_path);
        editor.reset_journal();
        self.builder = super::scene_builder(&mut editor);
        self.editor = editor;
        self.data_label = label;
        self.scene = None;
        self.tags.clear();
        self.validation = None;
        self.measure_start = None;
        self.measure_end = None;
        self.download_state.bbox = None;
        self.bbox_drag = None;
        self.downloaded_bboxes.clear();
        // Do not clear pending_download_bbox: after a successful download we may have just called set_editor and then record the extent.
        self.controller.reset_pending();
        self.drag = None;
        self.data_listed = true;
        self.persist_state();
    }

    /// Write user settings and the UI camera
    pub(super) fn persist_state(&self) {
        let (lat, lon) = self.camera.center.to_latlon().to_degrees();
        save_state(&AppState {
            data_path: None,
            label: String::new(),
            autosave_secs: Some(self.settings.autosave_secs),
            language: Some(neo_i18n::current_language().code().to_owned()),
            simplify_px: Some(self.settings.simplify_px),
            draw_snap_enabled: Some(self.settings.draw_snap_enabled),
            draw_snap_radius_px: Some(self.settings.draw_snap_radius_px),
            api_base: Some(self.settings.api_base.clone()),
            enabled_plugins: self.enabled_plugins.iter().cloned().collect(),
            imagery_url: Some(self.imagery.url.clone()),
            imagery_enabled: Some(false),
            imagery_opacity: Some(self.imagery.opacity),
            imagery_preset: Some(self.imagery.preset_id.clone()),
            imagery_wms: Some(self.imagery.wms),
            imagery_layers: Some(self.imagery.wms_layers.clone()),
            imagery_api_key: Some(self.imagery.api_key.clone()),
            imagery_attribution: Some(self.imagery.attribution.clone()),
            imagery_crs: Some(self.imagery.wms_crs.clone()),
            imagery_lat_first: Some(self.imagery.wms_lat_first),
            imagery_region_only: Some(self.imagery.region_only),
            imagery_osm: Some(false),
            shortcuts: Some(self.settings.shortcuts.clone()),
            view_lat: Some(lat),
            view_lon: Some(lon),
            view_zoom: Some(self.camera.zoom),
            skip_unsaved_confirm: Some(self.skip_unsaved_confirm),
        });
    }

    /// User-initiated "zoom to data". Startup and dataset switches do not call this.
    pub(super) fn reset_view(&mut self) {
        // Fit using the 2%–98% percentiles: sparse
        // ways outside the boundary (cross-border roads etc.) should not drag the initial view out to z11
        let mut xs: Vec<f64> = Vec::new();
        let mut ys: Vec<f64> = Vec::new();
        for node in self.editor.data().iter_nodes() {
            let point = WorldPoint::from_latlon(node.pos);
            xs.push(point.x);
            ys.push(point.y);
        }
        if xs.is_empty() {
            self.camera = Camera::new(WorldPoint::from_degrees(47.37, 8.54), 14.0, self.camera.viewport);
            self.scene = None;
            return;
        }
        xs.sort_by(f64::total_cmp);
        ys.sort_by(f64::total_cmp);
        let percentile = |values: &[f64], p: f64| -> f64 {
            let index = ((values.len() as f64 - 1.0) * p).round() as usize;
            values[index]
        };
        let lo_x = percentile(&xs, 0.02);
        let hi_x = percentile(&xs, 0.98);
        let lo_y = percentile(&ys, 0.02);
        let hi_y = percentile(&ys, 0.98);
        let mut bounds = WorldBounds::EMPTY;
        bounds.expand_point(WorldPoint::new(lo_x, lo_y));
        bounds.expand_point(WorldPoint::new(hi_x, hi_y));
        self.camera = Camera::new(bounds.center(), 14.0, self.camera.viewport);
        self.camera.fit_bounds(&bounds, 40.0);
        self.scene = None;
    }

    pub(super) fn ensure_scene(&mut self) {
        let mut rebuild = self.scene.is_none();
        let mut reasons = String::new();
        if let Some(scene) = &self.scene {
            if scene.version != self.editor.data().version() {
                rebuild = true;
                reasons.push_str("version ");
            }
            let rebase_dist = self.camera.center.dist(scene.origin);
            if rebase_dist > neo_geo::REBASE_THRESHOLD_M {
                rebuild = true;
                reasons.push_str("rebase ");
            }
            // Zooming changes the visible extent and LOD, so a rebuild is required; otherwise "zoom in then out" keeps
            // the culling result from the high zoom level and the map goes blank (a real bug reported by users).
            let zoom_delta = (scene.camera.zoom - self.camera.zoom).abs();
            if zoom_delta > 0.25 || node_lod_band(scene.camera.zoom) != node_lod_band(self.camera.zoom) {
                rebuild = true;
                reasons.push_str("zoom ");
            }
            // A window resize changes the culling extent; without a rebuild you get unrendered blank edges
            if scene.camera.viewport != self.camera.viewport {
                rebuild = true;
                reasons.push_str("viewport ");
            }
            let mpp = self.camera.meters_per_px();
            let view_w = f64::from(self.camera.viewport[0]) * mpp;
            let view_h = f64::from(self.camera.viewport[1]) * mpp;
            // The pan threshold must be smaller than cull_margin (5%), otherwise newly entering viewport edges show a white strip.
            if (scene.camera.center.x - self.camera.center.x).abs() > view_w * 0.04
                || (scene.camera.center.y - self.camera.center.y).abs() > view_h * 0.04
            {
                rebuild = true;
                reasons.push_str("pan ");
            }
            if rebuild {
                tracing::debug!(
                    "rebuild reasons=[{}] rebase_dist={:.1}m view={:.0}x{:.0}m center=({:.3},{:.3}) scene_center=({:.3},{:.3}) zoom={:.2}",
                    reasons,
                    rebase_dist,
                    view_w,
                    view_h,
                    self.camera.center.x,
                    self.camera.center.y,
                    scene.camera.center.x,
                    scene.camera.center.y,
                    self.camera.zoom
                );
            }
        }
        if rebuild {
            let t = Instant::now();
            self.editor.refresh_referrers();
            let mut frame = self.builder.build(
                self.editor.data(),
                self.editor.index(),
                self.editor.referrers(),
                self.editor.world_cache(),
                &self.camera,
            );
            for layer in &mut self.parked_layers {
                if !layer.visible {
                    continue;
                }
                let extra = layer.builder.build(
                    layer.editor.data(),
                    layer.editor.index(),
                    layer.editor.referrers(),
                    layer.editor.world_cache(),
                    &self.camera,
                );
                frame.stats.vertices += extra.stats.vertices;
                frame.stats.triangles += extra.stats.triangles;
                frame.stats.ways += extra.stats.ways;
                frame.stats.skipped += extra.stats.skipped;
                frame.batches.extend(extra.batches);
                frame.labels.extend(extra.labels);
            }
            self.last_scene_micros = t.elapsed().as_micros() as u64;
            tracing::debug!(
                "scene build: {:.2} ms, {} verts, {} tris, zoom {:.2}",
                self.last_scene_micros as f64 / 1000.0,
                frame.stats.vertices,
                frame.stats.triangles,
                self.camera.zoom
            );
            self.scene = Some(Arc::new(frame));
        }
    }

    pub(super) fn apply_outcome(&mut self, outcome: Outcome) {
        if let Some(msg) = outcome.message() {
            self.status = msg.to_owned();
        }
        self.resync_tags();
    }

    /// Read one object's tags (string pairs); returns `None` when the object does not exist.
    pub(super) fn object_tag_strings(&self, kind: ObjectKind, id: ObjectId) -> Option<Vec<(String, String)>> {
        let data = self.editor.data();
        let pairs: &[(neo_core::Sym, neo_core::Sym)] = match kind {
            ObjectKind::Node => data
                .node_handle(id)
                .and_then(|handle| data.nodes.get(handle))
                .map(|view| data.tag_pairs(view.tags))?,
            ObjectKind::Way => data
                .way_handle(id)
                .and_then(|handle| data.ways.get(handle))
                .map(|view| data.tag_pairs(view.tags))?,
            ObjectKind::Relation => data
                .relation_handle(id)
                .and_then(|handle| data.relations.get(handle))
                .map(|view| data.tag_pairs(view.tags))?,
            ObjectKind::Changeset => return Some(Vec::new()),
        };
        Some(
            pairs
                .iter()
                .map(|(key, value)| (data.resolve(*key).to_owned(), data.resolve(*value).to_owned()))
                .collect(),
        )
    }

    /// Make the tag editor recompute from current data next frame (called after undo/redo/deletion/external changes).
    ///
    /// The row model is synced by the panel every frame from "selection + data version"; here we only need to invalidate the cache.
    pub(super) fn resync_tags(&mut self) {
        self.tags.invalidate();
    }

    /// One tag edit (same for single/multi selection): apply "add/overwrite + delete" to each target object.
    ///
    /// All changes are committed with one `execute_batch`, counting as one undo step.
    pub(super) fn apply_tag_edit(&mut self, edit: neo_ui::TagEdit) {
        let mut commands: Vec<Box<dyn neo_editor::Command>> = Vec::new();
        for &(kind, id) in &edit.targets {
            let Some(current) = self.object_tag_strings(kind, id) else {
                continue;
            };
            let next = merged_tags(&current, &edit.set, &edit.remove);
            if same_tags(&current, &next) {
                continue;
            }
            commands.push(Box::new(neo_editor::SetTags::new(kind, id, next)));
        }
        if commands.is_empty() {
            return;
        }
        match self.editor.execute_batch(commands) {
            Ok(()) => {
                self.resync_tags();
                self.scene = None;
            }
            Err(err) => {
                self.status = tf("app.error.tags_update", &[("err", &err.to_string())]);
                self.resync_tags();
            }
        }
    }

    pub(super) fn ui_state(&self) -> UiState<'_> {
        let center = self.camera.center.to_latlon();
        UiState {
            mode: self.mode,
            can_undo: self.editor.history().undo_len() > 0,
            can_redo: self.editor.history().redo_len() > 0,
            dirty: self.editor.history().is_dirty(),
            has_pending_changes: self.editor.data().has_pending_changes(),
            undo_len: self.editor.history().undo_len(),
            redo_len: self.editor.history().redo_len(),
            zoom: self.camera.zoom,
            label: &self.data_label,
            status: &self.status,
            selection_len: self.editor.selection().len(),
            center_latlon: (center.lat.to_degrees(), center.lon.to_degrees()),
            mouse_latlon: self
                .hover_latlon
                .map(|ll| (ll.lat.to_degrees(), ll.lon.to_degrees())),
            meters_per_px: self.camera.meters_per_px(),
            measure_distance_m: self.measure_distance(),
            layer_visible: self.layer_visible,
            layer_active: self.layer_active,
        }
    }

    /// Selected-object list (right
    /// "Selection" panel)
    pub(super) fn selection_list(&self) -> Vec<(ObjectKind, ObjectId)> {
        let selection = self.editor.selection();
        let mut out = Vec::with_capacity(selection.len());
        out.extend(selection.nodes().iter().map(|id| (ObjectKind::Node, *id)));
        out.extend(selection.ways().iter().map(|id| (ObjectKind::Way, *id)));
        out.extend(selection.relations().iter().map(|id| (ObjectKind::Relation, *id)));
        out
    }

    pub(super) fn selection_target(&self) -> Option<(ObjectKind, ObjectId)> {
        let sel = self.editor.selection();
        if let Some(id) = sel.nodes().iter().next() {
            return Some((ObjectKind::Node, *id));
        }
        if let Some(id) = sel.ways().iter().next() {
            return Some((ObjectKind::Way, *id));
        }
        if let Some(id) = sel.relations().iter().next() {
            return Some((ObjectKind::Relation, *id));
        }
        None
    }

    pub(super) fn mode_label(&self) -> &'static str {
        match self.mode {
            UiMode::Select => t("app.mode.select"),
            UiMode::DrawPoint => t("app.mode.point"),
            UiMode::DrawLine => t("app.mode.line"),
            UiMode::DrawArea => t("app.mode.area"),
            UiMode::Measure => t("app.mode.measure"),
            UiMode::SplitPoint => t("app.mode.split"),
        }
    }

    /// Confirm unsaved changes before replacing the current dataset (shared by open/download/recovery)
    pub(super) fn confirm_discard(&self) -> bool {
        if self.skip_unsaved_confirm || !self.editor.history().is_dirty() {
            return true;
        }
        rfd::MessageDialog::new()
            .set_title(t("app.window_title"))
            .set_description(t("app.confirm.discard_edits"))
            .set_buttons(rfd::MessageButtons::OkCancel)
            .show()
            == rfd::MessageDialogResult::Ok
    }

    /// Whether to show the unsaved-changes confirmation before closing a layer/the app.
    pub(super) fn needs_unsaved_prompt(&self, dirty: bool) -> bool {
        dirty && !self.skip_unsaved_confirm
    }

    pub(super) fn open_unsaved_prompt(&mut self, kind: super::UnsavedPromptKind) {
        self.unsaved_prompt = Some(super::UnsavedPrompt {
            kind,
            dont_ask: false,
        });
    }

    /// Draw the unsaved-changes confirmation and, after confirmation, close the layer or exit.
    pub(super) fn draw_unsaved_prompt(&mut self, ctx: &egui::Context) {
        let Some(mut prompt) = self.unsaved_prompt.take() else {
            return;
        };
        let mut confirm = false;
        let mut cancel = false;
        egui::Window::new(t("app.confirm.unsaved_title"))
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                ui.set_min_width(320.0);
                ui.label(t("app.confirm.unsaved_body"));
                ui.add_space(8.0);
                ui.checkbox(&mut prompt.dont_ask, t("app.confirm.dont_ask"));
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    if ui
                        .add(
                            egui::Button::new(t("app.confirm.discard"))
                                .fill(egui::Color32::from_rgb(160, 70, 70)),
                        )
                        .clicked()
                    {
                        confirm = true;
                    }
                    if ui.button(t("app.confirm.cancel")).clicked() {
                        cancel = true;
                    }
                });
            });
        if confirm {
            if prompt.dont_ask {
                self.skip_unsaved_confirm = true;
                self.settings.confirm_unsaved = false;
                self.persist_state();
            }
            match prompt.kind {
                super::UnsavedPromptKind::CloseLayer(index) => self.remove_layer(index),
                super::UnsavedPromptKind::Exit => {
                    self.allow_exit = true;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
        } else if !cancel {
            self.unsaved_prompt = Some(prompt);
        }
    }
}

/// Apply one batch edit onto `current`: first remove the keys in `remove`, then write `set`
/// (existing keys get their values overwritten, new keys are appended).
fn merged_tags(
    current: &[(String, String)],
    set: &[(String, String)],
    remove: &[String],
) -> Vec<(String, String)> {
    let mut next: Vec<(String, String)> = current
        .iter()
        .filter(|(key, _)| !remove.iter().any(|gone| gone == key))
        .cloned()
        .collect();
    for (key, value) in set {
        if let Some(row) = next.iter_mut().find(|(existing, _)| existing == key) {
            row.1 = value.clone();
        } else {
            next.push((key.clone(), value.clone()));
        }
    }
    next
}

/// Whether two tag sets are equivalent (order-independent; for duplicate keys the last occurrence wins).
fn same_tags(left: &[(String, String)], right: &[(String, String)]) -> bool {
    fn normalized(tags: &[(String, String)]) -> Vec<(&str, &str)> {
        let mut rows: Vec<(&str, &str)> = tags
            .iter()
            .map(|(key, value)| (key.as_str(), value.as_str()))
            .collect();
        rows.sort_unstable();
        rows
    }
    normalized(left) == normalized(right)
}

#[cfg(test)]
mod bulk_tag_tests {
    use super::{merged_tags, same_tags};

    fn tags(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
            .collect()
    }

    #[test]
    fn merged_tags_adds_overwrites_and_removes() {
        let current = tags(&[("highway", "residential"), ("surface", "asphalt")]);
        let next = merged_tags(
            &current,
            &tags(&[("highway", "primary"), ("name", "Main")]),
            &["surface".to_owned()],
        );
        assert_eq!(next, tags(&[("highway", "primary"), ("name", "Main")]));
    }

    #[test]
    fn merged_tags_is_noop_for_an_identical_edit() {
        let current = tags(&[("highway", "residential")]);
        assert!(same_tags(&current, &merged_tags(&current, &[], &[])));
        assert!(same_tags(
            &current,
            &merged_tags(&current, &tags(&[("highway", "residential")]), &[])
        ));
        assert!(!same_tags(
            &current,
            &merged_tags(&current, &tags(&[("highway", "primary")]), &[])
        ));
    }

    #[test]
    fn same_tags_ignores_order() {
        let left = tags(&[("a", "1"), ("b", "2")]);
        let right = tags(&[("b", "2"), ("a", "1")]);
        assert!(same_tags(&left, &right));
        assert!(!same_tags(&left, &tags(&[("a", "1")])));
    }
}
