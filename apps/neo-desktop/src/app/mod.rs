//! 应用状态与帧循环。
//!
//! 阶段 3 集成后，`NeoApp` 不再直接持有 `DataSet`，而是持有 [`neo_editor::Editor`]，
//! 所有修改都经命令/事务/历史/journal。应用层只负责：
//!
//! * 把 egui 输入翻译成工具事件（选择/绘制/拖拽/删除）；
//! * 把 `UiActions` 翻译成命令或 IO 操作；
//! * 相机与场景重建时机；
//! * 保存/导出（XML、osmChange）。关闭程序时丢掉本次编辑记录，只留用户设置。

mod actions;
mod canvas;
mod core;
mod edit;
mod imagery;
mod layers;
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

/// 运行期数据目录（绝不依赖 CWD：快捷方式在 Program Files 下 CWD 不可写）。
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

/// 用编辑器自己的符号表构建场景样式，不把可变数据集交出去。
pub(super) fn scene_builder(editor: &mut neo_editor::Editor) -> neo_scene::SceneBuilder {
    neo_scene::SceneBuilder::with_intern(|name| editor.intern_sym(name))
}

/// journal 绑定的数据集基线：恢复时必须先加载这份文件，再把命令重放上去。
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct AppState {
    /// 当前编辑的源文件（None 表示空数据集/合成数据）
    data_path: Option<std::path::PathBuf>,
    /// 展示名
    #[serde(default)]
    label: String,
    /// 自动保存间隔（秒）
    #[serde(default)]
    autosave_secs: Option<u64>,
    /// way 简化容差（像素）
    #[serde(default)]
    simplify_px: Option<f64>,
    /// 绘制时几何吸附
    #[serde(default)]
    draw_snap_enabled: Option<bool>,
    /// 绘制吸附半径（像素）
    #[serde(default)]
    draw_snap_radius_px: Option<f32>,
    /// OSM API 基地址
    #[serde(default)]
    api_base: Option<String>,
    /// 已启用的插件 id
    #[serde(default)]
    enabled_plugins: Vec<String>,
    /// 影像 TMS 模板
    #[serde(default)]
    imagery_url: Option<String>,
    /// 是否显示影像
    #[serde(default)]
    imagery_enabled: Option<bool>,
    /// 影像不透明度
    #[serde(default)]
    imagery_opacity: Option<f32>,
    /// 影像预设 id
    #[serde(default)]
    imagery_preset: Option<String>,
    /// WMS 模式
    #[serde(default)]
    imagery_wms: Option<bool>,
    /// WMS layers
    #[serde(default)]
    imagery_layers: Option<String>,
    /// `{apikey}` 令牌
    #[serde(default)]
    imagery_api_key: Option<String>,
    /// 署名
    #[serde(default)]
    imagery_attribution: Option<String>,
    /// WMS 投影
    #[serde(default)]
    imagery_crs: Option<String>,
    /// WMS bbox 纬度在前
    #[serde(default)]
    imagery_lat_first: Option<bool>,
    /// 影像列表只显示当前地区
    #[serde(default)]
    imagery_region_only: Option<bool>,
    /// OSM 影像图层是否显示
    #[serde(default)]
    imagery_osm: Option<bool>,
    /// 自定义快捷键
    #[serde(default)]
    shortcuts: Option<std::collections::BTreeMap<String, String>>,
    /// 上次视图中心纬度（度）
    #[serde(default)]
    view_lat: Option<f64>,
    /// 上次视图中心经度（度）
    #[serde(default)]
    view_lon: Option<f64>,
    /// 上次缩放。下次启动时不超过 10。
    #[serde(default)]
    view_zoom: Option<f64>,
    /// 为 true 时关闭图层/应用不再提示未保存。
    #[serde(default)]
    skip_unsaved_confirm: Option<bool>,
}

/// 未保存确认框要执行的动作。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum UnsavedPromptKind {
    /// 关闭图层列表中的某一项。
    CloseLayer(usize),
    /// 退出应用。
    Exit,
}

/// 未保存确认框状态。
#[derive(Clone, Debug)]
struct UnsavedPrompt {
    kind: UnsavedPromptKind,
    /// 「不再提示」勾选。
    dont_ask: bool,
}

/// 重新打开时允许恢复的最大缩放。
const MAX_RESTORED_ZOOM: f64 = 10.0;

/// 上次界面相机。只看保存的中心和缩放，不看数据集里有没有要素。
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

/// journal 自动快照保留代数（阶段 4 自动保存）。
const AUTOSAVE_KEEP: usize = 3;

/// 选中要素
/// 可拖动外边距"（屏幕像素）：鼠标在此范围内才拖动选中要素
const SELECTION_DRAG_MARGIN_PX: f32 = 16.0;
/// 节点
/// 可选择外边
/// （屏幕像素）
const NODE_PICK_MARGIN_PX: f32 = 16.0;
/// way
/// 可选择外边
/// （屏幕像素）；与节点独立，避免互相侵占
const WAY_PICK_MARGIN_PX: f32 = 16.0;
/// 线段中点三角可点击半径（屏幕像素）。
const MIDPOINT_HIT_RADIUS_PX: f32 = 10.0;
/// 线段屏幕长度低于此值时不显示中点三角。
const MIDPOINT_MIN_SEGMENT_PX: f32 = 24.0;

/// 后台导入结果
/// data, label, report, source path)
type OpenResult = Result<(DataSet, String, String, std::path::PathBuf), String>;

/// 同步后台结果
enum SyncOutcome {
    /// 下载
    /// OSM XML
    Downloaded(String),
    /// 上传成功
    Uploaded(UploadResult),
    /// 分块上传部分成功（已上传映射 + 错误）
    UploadedPartial(UploadResult, String),
    /// 409 冲突
    Conflict(ConflictReport),
    /// 错误
    Error(String),
}

/// Notes 后台结果。
enum NotesOutcome {
    /// 列表刷新。
    List(Vec<NoteDto>),
    /// 单条新建/评论/关闭。
    One(NoteDto),
}

/// Notes 请求。
enum NotesRequest {
    /// 当前视口。
    Refresh,
    /// 新建。
    Create { lat: f64, lon: f64, text: String },
    /// 评论。
    Comment { id: i64, text: String },
    /// 关闭。
    Close { id: i64, text: String },
}

/// 复制/粘贴的图元（字符串标签，跨文档可用）
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
    /// (节点 ID, 原始位置)：单节点拖动或选中集合拖动
    nodes: Vec<(ObjectId, LatLon)>,
    /// 拖拽起点世界坐标
    anchor: WorldPoint,
    /// 当前鼠标世界坐标
    current: WorldPoint,
}

impl DragState {
    /// 世界坐标平移量（米）
    fn delta(&self) -> WorldPoint {
        WorldPoint::new(self.current.x - self.anchor.x, self.current.y - self.anchor.y)
    }

    /// 把原始位置按拖动量平移后的经纬度
    fn shifted(&self, pos: LatLon) -> LatLon {
        let delta = self.delta();
        let point = WorldPoint::from_latlon(pos);
        WorldPoint::new(point.x + delta.x, point.y + delta.y).to_latlon()
    }
}

/// 影像窗口里的底图。
const SOURCE_CUSTOM: u8 = 0;
/// OSM 标准地图图层。
const SOURCE_OSM: u8 = 1;

/// 一张已解码的影像瓦片。
struct CachedTile {
    texture: egui::TextureHandle,
    /// 最近一次被当前视口用到的代数。淘汰时旧的先让路。
    used: u64,
}

/// 影像瓦片键。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct TileKey {
    /// 0 = 影像窗口里的底图，1 = OSM 影像图层。
    source: u8,
    z: u32,
    x: u32,
    y: u32,
}

/// 停靠的数据图层（非活动，只读渲染）。
struct ParkedLayer {
    label: String,
    path: Option<std::path::PathBuf>,
    persist: Option<std::path::PathBuf>,
    editor: Editor,
    builder: SceneBuilder,
    visible: bool,
    /// 该层已框选/视口下载过的范围（多次下载取并集）。
    downloaded_bboxes: Vec<neo_ui::BboxView>,
}

/// WMS 后台结果：(key, bbox, bytes)。
type WmsResult = (String, [f64; 4], Result<Vec<u8>, String>);

/// NEO 应用状态
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
    /// 等待二次确认的级联删除：记录首次按下删除时的目标集合。
    /// 非空表示"同一批目标已被提示过会连带删除 way"。
    pending_delete: Option<Vec<(ObjectKind, ObjectId)>>,
    drag: Option<DragState>,
    /// 框选矩形（世界坐标起止）
    box_select: Option<(WorldPoint, WorldPoint)>,
    clipboard: Clipboard,
    journal_path: PathBuf,
    data_path: Option<PathBuf>,
    /// 是否显示帮助窗口
    show_help: bool,
    show_search: bool,
    search: neo_ui::SearchUiState,
    show_filter: bool,
    filter: neo_ui::FilterUiState,
    hidden: std::collections::HashSet<(ObjectKind, ObjectId)>,
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
    /// 同步旁路（登录/上下传/笔记）。
    sync_sidecar: Option<Arc<Mutex<SyncSession>>>,
    /// 影像旁路（瓦片下载）。
    imagery_sidecar: Option<Arc<ImagerySession>>,
    /// 当前账户 id（旁路缓存）。
    account_current: Option<String>,
    /// OAuth 是否在等回调。
    oauth_pending: bool,
    /// 旁路里已保存的 client_id（用于判断是否要回写）。
    sidecar_client_id: String,
    /// 旁路里已保存的 redirect_uri。
    sidecar_redirect_uri: String,
    /// 影像缩放级别缓存：`(preset+floor(zoom), level)`。
    imagery_zoom_cache: std::cell::RefCell<Option<((String, i32), u32)>>,
    /// OSM 标准地图图层是否显示。
    osm_visible: bool,
    /// 活动数据层是否出现在图层列表里。启动时不放入。
    data_listed: bool,
    /// OSM 影像是否出现在图层列表里。
    osm_listed: bool,
    /// 当前影像是否出现在图层列表里。
    imagery_listed: bool,
    show_changeset: bool,
    changeset: neo_ui::ChangesetUiState,
    imagery_tiles: std::collections::HashMap<TileKey, CachedTile>,
    /// 视口每用一次瓦片就加一，用来区分缓存里的新旧。
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
    /// 插件、影像和预设覆盖窗口
    show_ecosystem: bool,
    /// 是否显示图层信息窗口（图层右键 → 信息）。
    show_layer_info: bool,
    /// 是否显示设置窗口（帮助 → 设置）。
    show_settings: bool,
    /// 最近一次校验报告
    validation: Option<neo_validate::Report>,
    /// 上次自动保存时间
    last_autosave: Instant,
    /// 设置窗口状态
    settings: neo_ui::SettingsUiState,
    /// 打开数据集的后台结果（PBF/XML 导入不阻
    /// UI）
    open_rx: Option<Receiver<OpenResult>>,
    /// 是否有导入任务在跑
    open_busy: bool,
    /// 自动保存线程是否仍在跑（避免 60s 叠多个任务）
    autosave_busy: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// 是否显示同步窗口
    show_sync: bool,
    /// changeset 注释
    sync_comment: String,
    /// 同步状态文本
    sync_status: String,
    /// 同步进行中
    sync_busy: bool,
    /// 最
    /// 409 冲突
    sync_conflicts: Vec<neo_ui::SyncConflict>,
    /// 最
    /// 409 完整报告（含服务端版本）
    sync_conflict_report: Option<ConflictReport>,
    /// 是否显示导出窗口
    show_export: bool,
    /// 导出窗口状态
    export_state: neo_ui::ExportUiState,
    /// 是否显示下载窗口
    show_download: bool,
    /// 下载窗口状态
    download_state: neo_ui::DownloadUiState,
    /// 框选中的世界坐标（start, current）
    bbox_drag: Option<(WorldPoint, WorldPoint)>,
    /// 活动数据层已下载范围（多次下载取并集；活动层时框外置灰）。
    downloaded_bboxes: Vec<neo_ui::BboxView>,
    /// 进行中的下载对应范围；成功后并入 `downloaded_bboxes`。
    pending_download_bbox: Option<neo_ui::BboxView>,
    /// 同步后台结果
    sync_rx: Option<Receiver<SyncOutcome>>,
    /// 测距起点（世界坐标）
    measure_start: Option<WorldPoint>,
    /// 测距终点（世界坐标）
    measure_end: Option<WorldPoint>,
    /// 鼠标悬停位置的世界坐
    /// 经纬度
    hover_world: Option<WorldPoint>,
    hover_latlon: Option<LatLon>,
    /// 绘制工具当前吸附命中（画布 overlay / 落点用）。
    snap_hit: Option<neo_geo::SnapHit>,
    /// 选择模式下悬停的 way（用于中点三角）。
    hover_way: Option<ObjectId>,
    /// 中点按下已处理插入/拖动，抑制随后的 primary click。
    suppress_primary_click: bool,
    /// 图层显示开关（图层面板眼睛图标）
    layer_visible: bool,
    /// 图层是否为活动数据图层
    layer_active: bool,
    /// 关闭图层/应用时跳过未保存确认。
    skip_unsaved_confirm: bool,
    /// 待处理的未保存确认框。
    unsaved_prompt: Option<UnsavedPrompt>,
    /// 用户已确认退出后允许视口关闭。
    allow_exit: bool,
}

impl eframe::App for NeoApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let frame_started = Instant::now();
        let mut actions = UiActions::default();
        // journal 写失败（磁盘
        // 只读）后已停止记录；必须
        // UI 明确告警
        // 否则用户以为崩溃恢复仍然有效
        if let Some(err) = self.editor.take_journal_failure() {
            self.status = tf("app.error.journal_write", &[("err", &err)]);
        }

        // 关闭窗口：有未保存编辑时先拦下，弹确认框。
        if ui.ctx().input(|input| input.viewport().close_requested()) {
            if self.allow_exit || !self.needs_unsaved_prompt(self.any_unsaved_edits()) {
                // 允许关闭
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
        // 左侧竖排工具栏（JOSM toolbox
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
        let merge_available = self.editor.selection().ways().len() == 2;
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
        // 直接内联构造 UiState（而不是调用 &self 方法），保证 &mut self.show_layer_info 与
        // 字段借用互不冲突。
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
        let sync_bbox = self.current_bbox().1;
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
            &sync_bbox,
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
        neo_ui::filter_window(&ctx, &mut actions, &mut self.show_filter, &mut self.filter);
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
        // 关键：动作在面板绘制之后才执行，必须请求下一帧重绘，否则
        // 撤销/重做/编辑的结果要等到下一个输入事件才会显示（UI 滞后一步）
        self.maybe_autosave();
        // 后台任务完成时用户可能没有输入；忙时主动请求下一帧轮询结果
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
        // 先松开 journal 文件句柄，再删编辑记录。设置在 persist_state 里另存。
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
// 启动与恢
// ---------------------------------------------------------------------------

/// 每次启动都是空数据。上次的下载、打开的文件和 journal 不恢复。
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

/// 删掉本次编辑留下的文件。用户设置、账户、插件和影像偏移不在这里。
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

/// 安装字体：Phosphor + 系统 CJK + 外置 OFL 字体（不 `include_bytes!`）
///
/// 查找 `assets/fonts/`（或 `NEO_ASSET_DIR`）。缺文件时仍可用系统字体
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

    // 系统 CJK 优先，避免发行包缺字体时中文变方框
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

    // 外置 OFL 字体（随安装包分发，不进 exe）
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

/// 节点 LOD 档位；跨档时必须重建场景，否则渲
/// 拾取规则会不一致
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

/// 仅开
// 性能测试可设 `NEO_SYNTH_N=<节点
// ` 强制生成合成数据
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

/// 解析并匹配快捷键字符串（`Ctrl+Shift+Z`、`Delete`、`F5`）。
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

/// 屏幕空间中点到线段的距离（像素）
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
