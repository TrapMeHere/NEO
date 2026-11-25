//! Application state and frame loop.
//!
//! After the Phase 3 integration, `NeoApp` no longer holds a `DataSet` directly but holds [`neo_editor::Editor`],
//! with all modifications going through commands/transactions/history/journal. The application layer only handles:
//!
//! * translating egui input into tool events (select/draw/drag/delete);
//! * translating `UiActions` into commands or IO operations;
//! * camera and scene-rebuild timing;
//! * save/export (XML, osmChange). Closing the app discards this session's edit records, keeping only user settings.

mod actions;
mod canvas;
mod core;
mod edit;
mod imagery;
mod layers;
pub(crate) mod locale;
mod open;
mod plugins;
mod session;
mod sync;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use anyhow::{Context, Result};
use eframe::egui;
use neo_core::{LatLon, ObjectId, ObjectKind};
use neo_editor::{Editor, FileJournal};
use neo_geo::{Camera, WorldPoint};
use neo_io::{SynthSpec, generate, import_pbf_with_report, read_osm_xml_file};
use neo_scene::{SceneBuilder, SceneFrame};
use neo_store::DataSet;
use neo_tools::EditController;
use neo_ui::{TagsEditor, UiActions, UiMode, UiState};

use neo_api::{ConflictReport, UploadResult};
use neo_i18n::{t, tf};
use neo_ipc::{ImagerySession, NoteDto, SyncSession};
use std::sync::mpsc::Receiver;

use serde::{Deserialize, Serialize};

/// Runtime data directory (never rely on CWD: for shortcuts under Program Files the CWD is not writable).
fn data_dir() -> std::path::PathBuf {
    if let Ok(local) = std::env::var("LOCALAPPDATA")
        && !local.is_empty()
    {
        return std::path::PathBuf::from(local).join("NEO");
    }
    if let Ok(appdata) = std::env::var("APPDATA")
        && !appdata.is_empty()
    {
        return std::path::PathBuf::from(appdata).join("NEO");
    }
    if let Ok(home) = std::env::var("HOME")
        && !home.is_empty()
    {
        return std::path::PathBuf::from(home).join(".config").join("neo");
    }
    std::env::temp_dir().join("NEO")
}

fn journal_path() -> std::path::PathBuf {
    data_dir().join("neo-journal.jsonl")
}

fn state_path() -> std::path::PathBuf {
    data_dir().join("neo-state.json")
}

fn autosave_dir() -> std::path::PathBuf {
    data_dir().join("autosave")
}

fn plugins_dir() -> std::path::PathBuf {
    data_dir().join("plugins")
}

fn layers_dir() -> std::path::PathBuf {
    data_dir().join("layers")
}

/// Build scene styles from the editor's own symbol table without handing out a mutable dataset.
pub(super) fn scene_builder(editor: &mut neo_editor::Editor) -> neo_scene::SceneBuilder {
    neo_scene::SceneBuilder::with_intern(|name| editor.intern_sym(name))
}

/// Dataset baseline bound to the journal: on recovery this file must be loaded first, then commands are replayed on top.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct AppState {
    /// Source file currently being edited (None means an empty/synthetic dataset)
    data_path: Option<std::path::PathBuf>,
    /// Display name
    #[serde(default)]
    label: String,
    /// Autosave interval (seconds)
    #[serde(default)]
    autosave_secs: Option<u64>,
    /// UI language saved by the user; `None` means auto-detect from the OS locale.
    #[serde(default)]
    language: Option<String>,
    /// Way simplification tolerance (pixels)
    #[serde(default)]
    simplify_px: Option<f64>,
    /// Geometry snapping while drawing
    #[serde(default)]
    draw_snap_enabled: Option<bool>,
    /// Draw-snap radius (pixels)
    #[serde(default)]
    draw_snap_radius_px: Option<f32>,
    /// OSM API base URL
    #[serde(default)]
    api_base: Option<String>,
    /// Enabled plugin ids
    #[serde(default)]
    enabled_plugins: Vec<String>,
    /// Imagery TMS template
    #[serde(default)]
    imagery_url: Option<String>,
    /// Whether imagery is shown
    #[serde(default)]
    imagery_enabled: Option<bool>,
    /// Imagery opacity
    #[serde(default)]
    imagery_opacity: Option<f32>,
    /// Imagery preset id
    #[serde(default)]
    imagery_preset: Option<String>,
    /// WMS mode
    #[serde(default)]
    imagery_wms: Option<bool>,
    /// WMS layers
    #[serde(default)]
    imagery_layers: Option<String>,
    /// `{apikey}` token
    #[serde(default)]
    imagery_api_key: Option<String>,
    /// Attribution
    #[serde(default)]
    imagery_attribution: Option<String>,
    /// WMS projection
    #[serde(default)]
    imagery_crs: Option<String>,
    /// WMS bbox latitude first
    #[serde(default)]
    imagery_lat_first: Option<bool>,
    /// Show only imagery for the current region
    #[serde(default)]
    imagery_region_only: Option<bool>,
    /// Whether the OSM imagery layer is shown
    #[serde(default)]
    imagery_osm: Option<bool>,
    /// Custom keyboard shortcuts
    #[serde(default)]
    shortcuts: Option<std::collections::BTreeMap<String, String>>,
    /// Last view-center latitude (degrees)
    #[serde(default)]
    view_lat: Option<f64>,
    /// Last view-center longitude (degrees)
    #[serde(default)]
    view_lon: Option<f64>,
    /// Last zoom. Capped at 10 on next startup.
    #[serde(default)]
    view_zoom: Option<f64>,
    /// When true, closing a layer/the app no longer prompts about unsaved changes.
    #[serde(default)]
    skip_unsaved_confirm: Option<bool>,
}

/// Action to take when the unsaved-changes confirmation is answered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum UnsavedPromptKind {
    /// Close one entry in the layer list.
    CloseLayer(usize),
    /// Exit the application.
    Exit,
}

/// Unsaved-changes confirmation state.
#[derive(Clone, Debug)]
struct UnsavedPrompt {
    kind: UnsavedPromptKind,
    /// "Do not ask again" checkbox.
    dont_ask: bool,
}

/// Maximum zoom allowed when reopening.
const MAX_RESTORED_ZOOM: f64 = 10.0;

/// Last UI camera. Only the saved center and zoom are used; whether the dataset has any features is irrelevant.
fn saved_camera(state: &AppState, viewport: [u32; 2]) -> Option<Camera> {
    let (lat, lon, zoom) = (state.view_lat?, state.view_lon?, state.view_zoom?);
    if !lat.is_finite()
        || !lon.is_finite()
        || !zoom.is_finite()
        || !(-90.0..=90.0).contains(&lat)
        || !(-180.0..=180.0).contains(&lon)
    {
        return None;
    }
    Some(Camera::new(
        WorldPoint::from_degrees(lat, lon),
        zoom.min(MAX_RESTORED_ZOOM),
        viewport,
    ))
}

fn load_state() -> AppState {
    std::fs::read_to_string(state_path())
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

/// UI language the user explicitly selected in a previous session, if any.
///
/// `None` means "no saved choice": the app then follows the OS locale (or `NEO_LANG`).
pub(crate) fn saved_language() -> Option<neo_i18n::Lang> {
    load_state()
        .language
        .as_deref()
        .and_then(neo_i18n::Lang::from_code)
}

fn save_state(state: &AppState) {
    let path = state_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(text) = serde_json::to_string_pretty(state) {
        let tmp = path.with_extension(format!("json.{}.tmp", std::process::id()));
        if std::fs::write(&tmp, text).is_ok() && std::fs::rename(&tmp, &path).is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
    }
}

/// Number of journal auto-snapshot generations kept (Phase 4 autosave).
const AUTOSAVE_KEEP: usize = 3;

/// Selected features
/// "Draggable margin" (screen pixels): the selected feature is dragged only when the mouse is within it
const SELECTION_DRAG_MARGIN_PX: f32 = 16.0;
/// Node
/// selectable margin
/// (screen pixels)
const NODE_PICK_MARGIN_PX: f32 = 16.0;
/// way
/// selectable margin
/// (screen pixels); independent of the node margin so they do not swallow each other
const WAY_PICK_MARGIN_PX: f32 = 16.0;
/// Clickable radius of segment midpoint triangles (screen pixels).
const MIDPOINT_HIT_RADIUS_PX: f32 = 10.0;
/// Segment midpoint triangles are hidden below this screen-space length.
const MIDPOINT_MIN_SEGMENT_PX: f32 = 24.0;

/// Background import result
/// data, label, report, source path)
type OpenResult = Result<(DataSet, String, String, std::path::PathBuf), String>;

/// Background sync result
enum SyncOutcome {
    /// Download
    /// OSM XML
    Downloaded(String),
    /// Upload succeeded
    Uploaded(UploadResult),
    /// Chunked upload partially succeeded (uploaded mapping + error)
    UploadedPartial(UploadResult, String),
    /// 409 conflict
    Conflict(ConflictReport),
    /// Error
    Error(String),
}

/// Notes background result.
enum NotesOutcome {
    /// List refreshed.
    List(Vec<NoteDto>),
    /// Single create/comment/close.
    One(NoteDto),
}

/// Notes request.
enum NotesRequest {
    /// Current viewport.
    Refresh,
    /// Create.
    Create { lat: f64, lon: f64, text: String },
    /// Comment.
    Comment { id: i64, text: String },
    /// Close.
    Close { id: i64, text: String },
}

/// Copied/pasted primitives (string tags, usable across documents)
#[derive(Clone, Default)]
struct Clipboard {
    nodes: Vec<ClipboardNode>,
    ways: Vec<ClipboardWay>,
    relations: Vec<ClipboardRelation>,
}

#[derive(Clone)]
struct ClipboardNode {
    id: ObjectId,
    pos: LatLon,
    tags: Vec<(String, String)>,
}

#[derive(Clone)]
struct ClipboardWay {
    id: ObjectId,
    nodes: Vec<ObjectId>,
    tags: Vec<(String, String)>,
}

#[derive(Clone)]
struct ClipboardRelation {
    id: ObjectId,
    members: Vec<(ObjectKind, ObjectId, String)>,
    tags: Vec<(String, String)>,
}

impl Clipboard {
    fn len(&self) -> usize {
        self.nodes.len() + self.ways.len() + self.relations.len()
    }
}

struct DragState {
    /// (node ID, original position): single-node drag or selection-set drag
    nodes: Vec<(ObjectId, LatLon)>,
    /// Drag-start world coordinates
    anchor: WorldPoint,
    /// Current mouse world coordinates
    current: WorldPoint,
}

impl DragState {
    /// World-coordinate translation (meters)
    fn delta(&self) -> WorldPoint {
        WorldPoint::new(self.current.x - self.anchor.x, self.current.y - self.anchor.y)
    }

    /// Lon/lat after translating the original position by the drag delta
    fn shifted(&self, pos: LatLon) -> LatLon {
        let delta = self.delta();
        let point = WorldPoint::from_latlon(pos);
        WorldPoint::new(point.x + delta.x, point.y + delta.y).to_latlon()
    }
}

/// Basemap inside the imagery window.
const SOURCE_CUSTOM: u8 = 0;
/// Standard OSM map layer.
const SOURCE_OSM: u8 = 1;

/// One decoded imagery tile.
struct CachedTile {
    texture: egui::TextureHandle,
    /// Generation last used by the current viewport. On eviction, older ones go first.
    used: u64,
}

/// Imagery tile key.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct TileKey {
    /// 0 = basemap in the imagery window, 1 = OSM imagery layer.
    source: u8,
    z: u32,
    x: u32,
    y: u32,
}

/// Docked data layer (inactive, rendered read-only).
struct ParkedLayer {
    label: String,
    path: Option<std::path::PathBuf>,
    persist: Option<std::path::PathBuf>,
    editor: Editor,
    builder: SceneBuilder,
    visible: bool,
    /// Extent downloaded for this layer via box select/viewport (multiple downloads are unioned).
    downloaded_bboxes: Vec<neo_ui::BboxView>,
}

/// WMS background result: (key, bbox, bytes).
type WmsResult = (String, [f64; 4], Result<Vec<u8>, String>);

/// NEO application state
pub struct NeoApp {
    editor: Editor,
    camera: Camera,
    builder: SceneBuilder,
    scene: Option<Arc<SceneFrame>>,
    controller: EditController,
    mode: UiMode,
    tags: TagsEditor,
    status: String,
    last_scene_micros: u64,
    data_label: String,
    /// Cascade delete awaiting confirmation: records the target set from the first delete press.
    /// Non-empty means "the same targets have already been prompted about deleting their ways too".
    pending_delete: Option<Vec<(ObjectKind, ObjectId)>>,
    drag: Option<DragState>,
    /// Box-select rectangle (world-coordinate start/end)
    box_select: Option<(WorldPoint, WorldPoint)>,
    clipboard: Clipboard,
    journal_path: PathBuf,
    data_path: Option<PathBuf>,
    /// Whether the help window is shown
    show_help: bool,
    show_search: bool,
    search: neo_ui::SearchUiState,
    show_notes: bool,
    notes: neo_ui::NotesUiState,
    notes_rx: Option<Receiver<Result<NotesOutcome, String>>>,
    show_plugins: bool,
    plugins: neo_ui::PluginUiState,
    plugin_host: Option<neo_plugin::PluginHost>,
    enabled_plugins: std::collections::HashSet<String>,
    parked_layers: Vec<ParkedLayer>,
    show_imagery: bool,
    imagery: neo_ui::ImageryUiState,
    /// Sync sidecar (login/upload/download/notes).
    sync_sidecar: Option<Arc<Mutex<SyncSession>>>,
    /// Imagery sidecar (tile download).
    imagery_sidecar: Option<Arc<ImagerySession>>,
    /// Current account id (cached by the sidecar).
    account_current: Option<String>,
    /// Whether OAuth is waiting for a callback.
    oauth_pending: bool,
    /// client_id already saved in the sidecar (used to decide whether to write it back).
    sidecar_client_id: String,
    /// redirect_uri already saved in the sidecar.
    sidecar_redirect_uri: String,
    /// Imagery zoom-level cache: `(preset+floor(zoom), level)`.
    imagery_zoom_cache: std::cell::RefCell<Option<((String, i32), u32)>>,
    /// Whether the standard OSM map layer is shown.
    osm_visible: bool,
    /// Whether the active data layer appears in the layer list. Not included at startup.
    data_listed: bool,
    /// Whether OSM imagery appears in the layer list.
    osm_listed: bool,
    /// Whether the current imagery appears in the layer list.
    imagery_listed: bool,
    show_changeset: bool,
    changeset: neo_ui::ChangesetUiState,
    imagery_tiles: std::collections::HashMap<TileKey, CachedTile>,
    /// Incremented each time the viewport uses a tile, to tell newer cache entries from older ones.
    imagery_clock: u64,
    imagery_pending: std::collections::HashSet<TileKey>,
    imagery_tx: std::sync::mpsc::Sender<(TileKey, Result<Vec<u8>, String>)>,
    imagery_rx: Receiver<(TileKey, Result<Vec<u8>, String>)>,
    wms_tx: std::sync::mpsc::Sender<WmsResult>,
    wms_rx: Receiver<WmsResult>,
    wms_texture: Option<egui::TextureHandle>,
    wms_bbox: Option<[f64; 4]>,
    wms_key: String,
    wms_pending: bool,
    imagery_offsets: neo_style::imagery::OffsetDb,
    icon_textures: std::collections::HashMap<String, Option<egui::TextureHandle>>,
    /// Plugins, imagery and preset coverage window
    show_ecosystem: bool,
    /// Whether the layer-info window is shown (layer context menu → Info).
    show_layer_info: bool,
    /// Whether the settings window is shown (Help → Settings).
    show_settings: bool,
    /// Most recent validation report
    validation: Option<neo_validate::Report>,
    /// Time of the last autosave
    last_autosave: Instant,
    /// Settings window state
    settings: neo_ui::SettingsUiState,
    /// Background result of opening a dataset (PBF/XML import does not block
    /// the UI)
    open_rx: Option<Receiver<OpenResult>>,
    /// Whether an import task is running
    open_busy: bool,
    /// Whether the autosave thread is still running (avoids stacking tasks every 60 s)
    autosave_busy: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// Whether the sync window is shown
    show_sync: bool,
    /// changeset comment
    sync_comment: String,
    /// Sync status text
    sync_status: String,
    /// Sync in progress
    sync_busy: bool,
    /// Most recent
    /// 409 conflict
    sync_conflicts: Vec<neo_ui::SyncConflict>,
    /// Most recent
    /// full 409 report (including server version)
    sync_conflict_report: Option<ConflictReport>,
    /// Whether the export window is shown
    show_export: bool,
    /// Export window state
    export_state: neo_ui::ExportUiState,
    /// Whether the download window is shown
    show_download: bool,
    /// Download window state
    download_state: neo_ui::DownloadUiState,
    /// Box-select world coordinates (start, current)
    bbox_drag: Option<(WorldPoint, WorldPoint)>,
    /// Downloaded extent of the active data layer (union of multiple downloads; outside it is grayed out for the active layer).
    downloaded_bboxes: Vec<neo_ui::BboxView>,
    /// Extent of the in-progress download; merged into `downloaded_bboxes` on success.
    pending_download_bbox: Option<neo_ui::BboxView>,
    /// Background sync result
    sync_rx: Option<Receiver<SyncOutcome>>,
    /// Measurement start (world coordinates)
    measure_start: Option<WorldPoint>,
    /// Measurement end (world coordinates)
    measure_end: Option<WorldPoint>,
    /// World coordinates of the mouse hover
    /// lon/lat
    hover_world: Option<WorldPoint>,
    hover_latlon: Option<LatLon>,
    /// Current snap hit for the drawing tool (used by the canvas overlay / point placement).
    snap_hit: Option<neo_geo::SnapHit>,
    /// Hovered way in selection mode (used for midpoint triangles).
    hover_way: Option<ObjectId>,
    /// A midpoint press already handled insert/drag, suppressing the following primary click.
    suppress_primary_click: bool,
    /// Layer visibility toggles (eye icons in the layer panel)
    layer_visible: bool,
    /// Whether the layer is the active data layer
    layer_active: bool,
    /// Skip the unsaved-changes confirmation when closing a layer/the app.
    skip_unsaved_confirm: bool,
    /// Pending unsaved-changes confirmation.
    unsaved_prompt: Option<UnsavedPrompt>,
    /// Allow the viewport to close once the user has confirmed exit.
    allow_exit: bool,
}

impl eframe::App for NeoApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let frame_started = Instant::now();
        let mut actions = UiActions::default();
        // Journal writes failed (disk
        // read-only) and recording has stopped; the
        // UI must be warned explicitly,
        // otherwise users assume crash recovery still works
        if let Some(err) = self.editor.take_journal_failure() {
            self.status = tf("app.error.journal_write", &[("err", &err)]);
        }

        // Closing the window: intercept first when there are unsaved edits, then show the confirmation.
        if ui.ctx().input(|input| input.viewport().close_requested()) {
            if self.allow_exit || !self.needs_unsaved_prompt(self.any_unsaved_edits()) {
                // Allow closing
            } else {
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::CancelClose);
                if self.unsaved_prompt.is_none() {
                    self.open_unsaved_prompt(UnsavedPromptKind::Exit);
                }
            }
        }

        self.poll_oauth();
        self.poll_sync();
        self.poll_open_dataset();
        self.poll_notes();
        self.settings.selected = self.account_current.clone();
        for account in &mut self.settings.accounts {
            account.active = self.account_current.as_deref() == Some(account.id.as_str());
        }

        let typing = ui.ctx().memory(|m| m.focused().is_some());
        if !typing && self.settings.rebinding.is_none() {
            let active: Vec<String> = ui.input(|input| {
                self.settings
                    .shortcuts
                    .iter()
                    .filter(|(_, binding)| shortcut_matches(input, binding))
                    .map(|(action, _)| action.clone())
                    .collect()
            });
            for action in active {
                match action.as_str() {
                    "undo" => actions.undo = true,
                    "redo" => actions.redo = true,
                    "copy" => actions.copy_selection = true,
                    "paste" => actions.paste = true,
                    "duplicate" => actions.duplicate = true,
                    "find" => actions.toggle_search = true,
                    "save_xml" => actions.save_xml = true,
                    "finish_way" => actions.finish_way = true,
                    "delete" => actions.delete_selected = true,
                    "mode_select" => actions.mode = Some(UiMode::Select),
                    "mode_draw_point" => actions.mode = Some(UiMode::DrawPoint),
                    "mode_draw_line" | "mode_draw" => actions.mode = Some(UiMode::DrawLine),
                    "mode_draw_area" => actions.mode = Some(UiMode::DrawArea),
                    "mode_measure" => actions.mode = Some(UiMode::Measure),
                    "zoom_in" => actions.zoom_in = true,
                    "zoom_out" => actions.zoom_out = true,
                    "reset_view" => actions.reset_view = true,
                    _ => {}
                }
            }
        }

        egui::Panel::top("neo-menubar").show(ui, |ui| {
            let center = self.camera.center.to_latlon();
            let lat = center.lat.to_degrees();
            let lon = center.lon.to_degrees();
            let state = self.ui_state();
            neo_ui::menu_bar(ui, &mut actions, state, lat, lon);
        });
        egui::Panel::top("neo-toolbar").show(ui, |ui| {
            let state = self.ui_state();
            neo_ui::top_toolbar(ui, &mut actions, state);
        });
        // Left vertical toolbar (JOSM toolbox
        egui::Panel::left("neo-toolbox")
            .exact_size(48.0)
            .resizable(false)
            .show(ui, |ui| {
                let state = self.ui_state();
                neo_ui::tool_palette(ui, &mut actions, state);
            });
        let selection = self.selection_target();
        let selection_list = self.selection_list();
        let validation_view = self.validation_view();
        let merge_available = self.editor.selection().ways().len() >= 2;
        let history_labels = self.editor.history().labels();
        let layers_view = self.layers_view();
        egui::Panel::right("neo-properties")
            .default_size(340.0)
            .min_size(300.0)
            .max_size(440.0)
            .show(ui, |ui| {
                neo_ui::right_side_panel(
                    ui,
                    &mut actions,
                    self.editor.data(),
                    self.editor.referrers(),
                    selection,
                    &selection_list,
                    &history_labels,
                    &mut self.tags,
                    merge_available,
                    &layers_view,
                    validation_view.as_ref(),
                );
            });
        egui::Panel::bottom("neo-status").show(ui, |ui| {
            let state = self.ui_state();
            neo_ui::status_bar(ui, state);
        });
        egui::CentralPanel::no_frame().show(ui, |ui| {
            self.map_canvas(ui);
        });
        let ctx = ui.ctx().clone();
        // Construct UiState inline (rather than calling an &self method) so that &mut self.show_layer_info and
        // field borrows do not conflict.
        let center = self.camera.center.to_latlon();
        let layer_state = UiState {
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
        };
        neo_ui::layer_info_window(
            &ctx,
            self.editor.data(),
            layer_state,
            &mut actions,
            &mut self.show_layer_info,
        );
        let sync_account_name = self
            .settings
            .accounts
            .iter()
            .find(|account| account.active)
            .map(|account| account.display_name.clone());
        let download_status = self.sync_status.clone();
        neo_ui::download_window(
            &ctx,
            &mut actions,
            &mut self.show_download,
            &mut self.download_state,
            &download_status,
        );
        neo_ui::export_window(&ctx, &mut actions, &mut self.show_export, &mut self.export_state);
        let has_pending_changes = self.editor.data().has_pending_changes();
        neo_ui::sync_window(
            &ctx,
            &mut actions,
            &mut self.show_sync,
            sync_account_name.as_deref(),
            &mut self.sync_comment,
            &self.sync_status,
            self.sync_busy,
            has_pending_changes,
            &self.sync_conflicts,
        );
        neo_ui::settings_window(&ctx, &mut actions, &mut self.settings, &mut self.show_settings);
        self.controller.snap_enabled = self.settings.draw_snap_enabled;
        let want_skip = !self.settings.confirm_unsaved;
        if want_skip != self.skip_unsaved_confirm {
            self.skip_unsaved_confirm = want_skip;
            self.persist_state();
        }
        self.draw_unsaved_prompt(&ctx);
        neo_ui::help_window(&ctx, &mut self.show_help);
        neo_ui::search_window(
            &ctx,
            &mut actions,
            &mut self.show_search,
            &mut self.search,
            self.editor.data(),
        );
        neo_ui::notes_window(&ctx, &mut actions, &mut self.show_notes, &mut self.notes);
        neo_ui::plugins_window(&ctx, &mut actions, &mut self.show_plugins, &mut self.plugins);
        neo_ui::imagery_window(
            &ctx,
            &mut actions,
            &mut self.show_imagery,
            &mut self.imagery,
            center.lat.to_degrees(),
            center.lon.to_degrees(),
        );
        neo_ui::changeset_window(&ctx, &mut actions, &mut self.show_changeset, &mut self.changeset);
        neo_ui::ecosystem_window(&ctx, &mut self.show_ecosystem);

        let had_actions = actions != UiActions::default();
        if let Err(err) = self.apply_actions(actions) {
            self.status = tf("app.error.generic", &[("err", &err.to_string())]);
        }
        // Key point: actions run after panels are drawn, so the next frame must be requested; otherwise
        // undo/redo/edit results only appear on the next input event (the UI lags one step behind)
        self.maybe_autosave();
        // When a background task finishes the user may not produce input; while busy, request the next frame to poll results proactively
        if self.sync_busy || self.oauth_pending || self.open_busy {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(200));
        }
        if had_actions {
            ui.ctx().request_repaint();
        }
        let frame_ms = frame_started.elapsed().as_secs_f64() * 1000.0;
        if frame_ms > 8.0 {
            tracing::debug!("ui frame: {frame_ms:.1} ms, zoom {:.2}", self.camera.zoom);
        }
    }

    fn save(&mut self, _storage: &mut dyn eframe::Storage) {
        self.persist_state();
    }

    fn on_exit(&mut self) {
        // Release the journal file handle first, then delete the edit records. Settings are saved separately in persist_state.
        self.editor
            .set_journal_sink(Box::new(neo_editor::MemoryJournal::default()));
        for _ in 0..50 {
            if !self.autosave_busy.load(std::sync::atomic::Ordering::Acquire) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        clear_editing_records();
        self.persist_state();
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.09, 0.10, 0.12, 1.0]
    }
}

// ---------------------------------------------------------------------------
// Startup and recovery
// ---------------------------------------------------------------------------

/// Every start begins with empty data. The previous downloads, opened files and journal are not restored.
fn init_editor(journal_path: &Path) -> Result<(Editor, String)> {
    let (base, label) = load_initial_data()?;
    let mut editor = Editor::new(base);
    if let Some(parent) = journal_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::File::create(journal_path);
    if let Ok(sink) = FileJournal::open(journal_path) {
        editor.set_journal_sink(Box::new(sink));
    }
    Ok((editor, label))
}

/// Delete the files left by this editing session. User settings, accounts, plugins and imagery offsets are not included.
fn clear_editing_records() {
    let dir = data_dir();
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            let session_file = name.starts_with("neo-journal")
                || name == "neo-sync-download.osm"
                || name == "neo-uploaded.osm"
                || name == "neo-recovered.osm";
            if session_file {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
    let _ = std::fs::remove_dir_all(autosave_dir());
    let _ = std::fs::remove_dir_all(layers_dir());
}

/// Install fonts: Phosphor + system CJK + external OFL fonts (without `include_bytes!`)
///
/// Look up `assets/fonts/` (or `NEO_ASSET_DIR`). System fonts remain available when files are missing
fn install_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    egui_phosphor::add_to_fonts(&mut fonts, egui_phosphor::Variant::Regular);

    let push_family = |fonts: &mut egui::FontDefinitions, name: &str, front: bool| {
        for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
            if let Some(list) = fonts.families.get_mut(&family) {
                if front && family == egui::FontFamily::Proportional {
                    list.insert(0, name.to_owned());
                } else {
                    list.push(name.to_owned());
                }
            }
        }
    };

    // Prefer system CJK so Chinese does not become tofu boxes when the distribution lacks fonts
    let system_cjk = [
        r"C:\Windows\Fonts\msyh.ttc",
        r"C:\Windows\Fonts\simhei.ttf",
        r"C:\Windows\Fonts\Deng.ttf",
        r"C:\Windows\Fonts\simsun.ttc",
    ];
    for path in system_cjk {
        let Ok(bytes) = std::fs::read(path) else {
            continue;
        };
        fonts.font_data.insert(
            "sys-cjk".to_owned(),
            std::sync::Arc::new(egui::FontData::from_owned(bytes)),
        );
        push_family(&mut fonts, "sys-cjk", true);
        break;
    }

    // External OFL fonts (shipped with the installer, not embedded in the exe)
    let bundled = [
        ("neo-cjk", "fonts/NotoSansCJKsc-Regular.otf", true),
        ("neo-unifont", "fonts/unifont.otf", false),
        ("neo-unifont-upper", "fonts/unifont_upper.otf", false),
    ];
    for (name, relative, front) in bundled {
        let Some(bytes) = neo_style::read_asset(relative) else {
            continue;
        };
        fonts.font_data.insert(
            name.to_owned(),
            std::sync::Arc::new(egui::FontData::from_owned(bytes)),
        );
        push_family(&mut fonts, name, front);
    }

    ctx.set_fonts(fonts);
}

fn kind_from_name(name: &str) -> Option<ObjectKind> {
    match name {
        "node" => Some(ObjectKind::Node),
        "way" => Some(ObjectKind::Way),
        "relation" => Some(ObjectKind::Relation),
        _ => None,
    }
}

/// Node LOD tier; crossing tiers requires a scene rebuild, otherwise
/// rendering and picking rules would disagree
fn node_lod_band(zoom: f64) -> u8 {
    if zoom < neo_scene::NODE_MIN_ZOOM {
        0
    } else if zoom < neo_scene::NODE_ALL_ZOOM {
        1
    } else {
        2
    }
}

fn load_dataset_file(path: &Path) -> Result<(DataSet, String)> {
    let extension = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if extension == "pbf" {
        let (data, _report) = import_pbf_with_report(path)
            .with_context(|| tf("app.error.import", &[("path", &path.display().to_string())]))?;
        Ok((
            data,
            path.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.display().to_string()),
        ))
    } else {
        let mut data = DataSet::new();
        let _report = read_osm_xml_file(&mut data, path)
            .with_context(|| tf("app.error.read", &[("path", &path.display().to_string())]))?;
        Ok((
            data,
            path.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.display().to_string()),
        ))
    }
}

/// Only enabled
// Performance tests can set `NEO_SYNTH_N=<node count>`
// to force synthetic data generation
fn load_initial_data() -> Result<(DataSet, String)> {
    if let Ok(value) = std::env::var("NEO_SYNTH_N")
        && let Ok(nodes) = value.parse::<usize>()
        && nodes > 0
    {
        let (data, _, _) = generate(&SynthSpec::with_nodes(nodes))?;
        return Ok((data, format!("synth:{nodes}")));
    }
    Ok((DataSet::new(), t("app.data.empty").to_owned()))
}

#[allow(unused_imports)]
use neo_editor::History as _History;

/// Parse and match a shortcut string (`Ctrl+Shift+Z`, `Delete`, `F5`).
fn shortcut_matches(input: &egui::InputState, binding: &str) -> bool {
    let mut ctrl = false;
    let mut shift = false;
    let mut alt = false;
    let mut key = None;
    for part in binding.split('+').map(str::trim).filter(|part| !part.is_empty()) {
        match part {
            "Ctrl" | "Command" | "Cmd" => ctrl = true,
            "Shift" => shift = true,
            "Alt" => alt = true,
            other => key = egui::Key::from_name(other),
        }
    }
    let Some(key) = key else {
        return false;
    };
    let command = input.modifiers.command || input.modifiers.ctrl;
    input.key_pressed(key) && command == ctrl && input.modifiers.shift == shift && input.modifiers.alt == alt
}

/// Point-to-segment distance in screen space (pixels)
fn point_segment_distance(p: [f32; 2], a: [f32; 2], b: [f32; 2]) -> f32 {
    let ab = [b[0] - a[0], b[1] - a[1]];
    let ap = [p[0] - a[0], p[1] - a[1]];
    let len_sq = ab[0] * ab[0] + ab[1] * ab[1];
    if len_sq <= f32::EPSILON {
        return (ap[0] * ap[0] + ap[1] * ap[1]).sqrt();
    }
    let t = ((ap[0] * ab[0] + ap[1] * ab[1]) / len_sq).clamp(0.0, 1.0);
    let closest = [a[0] + ab[0] * t, a[1] + ab[1] * t];
    let dx = p[0] - closest[0];
    let dy = p[1] - closest[1];
    (dx * dx + dy * dy).sqrt()
}

#[cfg(test)]
mod tests {
    use super::AppState;
    use super::saved_camera;

    #[test]
    fn saved_camera_keeps_center_and_caps_zoom() {
        let state = AppState {
            view_lat: Some(30.6),
            view_lon: Some(104.0),
            view_zoom: Some(12.5),
            ..AppState::default()
        };
        let camera = saved_camera(&state, [800, 600]).expect("saved view");
        let (lat, lon) = camera.center.to_latlon().to_degrees();
        assert!((lat - 30.6).abs() < 1e-4);
        assert!((lon - 104.0).abs() < 1e-4);
        assert_eq!(camera.zoom, 10.0);
    }

    #[test]
    fn saved_camera_is_absent_without_a_stored_view() {
        assert!(saved_camera(&AppState::default(), [800, 600]).is_none());
    }
}
