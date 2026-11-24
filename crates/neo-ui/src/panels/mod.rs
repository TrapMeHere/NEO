//! JOSM 风格界面组件（现代化）。
//!
//! 布局对齐 JOSM：
//! * 顶部：菜单栏 + 常用功能工具栏；
//! * 左侧：竖排**工具栏**（选择/绘制/删除/测距 + 几何操作）；
//! * 右侧：**图层**（右键 → 信息/缩放到图层）、**标签/成员**、**选择**、**关系**、
//!   **校验结果**、**冲突** 面板堆叠；
//! * 底部：**状态栏**（经纬度、比例尺、测距结果、缩放、撤销/重做、操作提示）。
//!
//! 图标来自开源 Phosphor Icons（`egui-phosphor`，MIT）。面板均为纯函数。

use egui::{RichText, Ui};
use egui_phosphor::regular as ph;
use neo_core::{ObjectId, ObjectKind};
use neo_i18n::t;

mod changeset;
mod chrome;
mod download;
mod export;
mod imagery;
mod notes;
mod plugins;
mod presets;
mod search;
mod settings;
mod side;
mod sync;

pub use changeset::{ChangesetUiState, ChangesetView, changeset_window};
pub use chrome::{apply_modern_style, menu_bar, tool_palette, top_toolbar};
pub use download::{BboxView, DownloadTab, DownloadUiState, download_window};
pub use export::{ExportUiState, export_window};
pub use imagery::{ImageryUiState, imagery_window};
pub use notes::{NoteCommentView, NoteView, NotesUiState, notes_window};
pub use plugins::{PluginUiState, PluginView, plugins_window};
pub use presets::{ecosystem_window, help_window};
pub use search::{SearchHit, SearchUiState, search_window};
pub use settings::{AccountView, SHORTCUT_ACTIONS, SettingsTab, SettingsUiState, settings_window};
pub use side::{ValidationItem, ValidationView, layer_info_window, right_side_panel, status_bar};
pub use sync::{SyncConflict, sync_window};

/// UI 侧的工作模式。
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum UiMode {
    /// 选择/移动（工具栏无单独按钮；点/线/面均未选中时回到此模式）。
    #[default]
    Select,
    /// 创建节点。
    DrawPoint,
    /// 绘制折线。
    DrawLine,
    /// 绘制闭合面。
    DrawArea,
    /// 测距。
    Measure,
    /// 按点分割：点击路径上的节点即在该点分割经过它的路径。
    SplitPoint,
}

/// 关系成员编辑意图（由应用层转成 `SetRelationMembers` 命令）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RelationMemberEdit {
    /// 把当前选中对象加入关系（角色取标签编辑器里的“成员角色”）。
    Add {
        /// 关系 ID。
        relation: ObjectId,
    },
    /// 删除指定下标的成员。
    Remove {
        /// 关系 ID。
        relation: ObjectId,
        /// 成员下标。
        index: usize,
    },
    /// 上移成员。
    MoveUp {
        /// 关系 ID。
        relation: ObjectId,
        /// 成员下标。
        index: usize,
    },
    /// 下移成员。
    MoveDown {
        /// 关系 ID。
        relation: ObjectId,
        /// 成员下标。
        index: usize,
    },
    /// 修改成员角色。
    SetRole {
        /// 关系 ID。
        relation: ObjectId,
        /// 成员下标。
        index: usize,
        /// 新角色。
        role: String,
    },
}

/// 图层是数据还是影像。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LayerRole {
    /// 可编辑的 OSM 数据。
    Data,
    /// 影像窗口里当前选中的底图。
    Imagery,
    /// 标准 OSM 栅格地图。
    Osm,
}

/// 数据或影像图层视图。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LayerView {
    /// 名称。
    pub label: String,
    /// 是否可见。
    pub visible: bool,
    /// 是否为活动（可编辑）图层。
    pub active: bool,
    /// 数据、当前影像，或 OSM 栅格图。出现在列表里的图层都能显示和删除。
    pub role: LayerRole,
}

/// 一次标签修改：对 `targets` 每个对象应用 `set`（添加/覆盖），并删除 `remove` 里的 key。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TagEdit {
    /// 要修改的对象（kind, id）。
    pub targets: Vec<(ObjectKind, ObjectId)>,
    /// 要添加/覆盖的标签。
    pub set: Vec<(String, String)>,
    /// 要从所有对象删除的 key。
    pub remove: Vec<String>,
}

/// 标签值在选中集合中的状态。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TagValue {
    /// 所有选中对象该 key 的值一致。
    Same(String),
    /// 值不一致或部分对象缺少该 key；保存出现过的值（去重，按首次出现顺序）。
    Mixed(Vec<String>),
}

/// 标签编辑器的一行（多个选中对象按并集展示）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TagRow {
    /// 当前 key（可编辑）。
    pub key: String,
    /// 同步时的 key；空表示用户新加的行。
    pub original_key: String,
    /// 值状态。
    pub value: TagValue,
}

/// UI 产生的意图，由应用层翻译成编辑命令/IO/相机操作。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UiActions {
    /// 打开 OSM 数据文件（XML/PBF），始终作为新图层。
    pub open_file: bool,
    /// 打开 PBF（兼容旧入口；与 [`Self::open_file`] 相同）。
    pub open_pbf: bool,
    /// 打开 OSM XML（兼容旧入口；与 [`Self::open_file`] 相同）。
    pub open_xml: bool,
    /// 导入 GPX。
    pub open_gpx: bool,
    /// 导出 GPX。
    pub save_gpx: bool,
    /// 保存为 OSM XML。
    pub save_xml: bool,
    /// 导出 osmChange。
    pub save_osc: bool,
    /// 导出 PBF。
    pub save_pbf: bool,
    /// 保存会话（.jos/.joz）。
    pub save_session: bool,
    /// 打开会话。
    pub open_session: bool,
    /// 重置视图（缩放到数据）。
    pub reset_view: bool,
    /// 放大。
    pub zoom_in: bool,
    /// 缩小。
    pub zoom_out: bool,
    /// 撤销。
    pub undo: bool,
    /// 重做。
    pub redo: bool,
    /// 撤销 N 步（命令历史面板点击）。
    pub undo_to: Option<usize>,
    /// 复制选中对象。
    pub copy_selection: bool,
    /// 粘贴剪贴板。
    pub paste: bool,
    /// 复制并偏移粘贴选中对象。
    pub duplicate: bool,
    /// 切换模式。
    pub mode: Option<UiMode>,
    /// 结束当前绘制 way。
    pub finish_way: bool,
    /// 标签修改（失焦/选择下拉值时产生）。
    pub tag_edit: Option<TagEdit>,
    /// 分割选中的 way。
    pub split_way: bool,
    /// 反转选中的 way。
    pub reverse_way: bool,
    /// 合并选中的两条 way。
    pub merge_ways: bool,
    /// 简化选中的 way（RDP）。
    pub simplify_way: bool,
    /// 把选中的多个节点粘合成一个（JOSM glue）。
    pub merge_nodes: bool,
    /// 节点对齐（按分布主轴）。
    pub align_nodes: bool,
    /// 节点等距分布。
    pub distribute_nodes: bool,
    /// 水平镜像（翻转纬度）。
    pub mirror_horizontal: bool,
    /// 垂直镜像（翻转经度）。
    pub mirror_vertical: bool,
    /// 解粘选中节点（每条引用 way 得到独立副本）。
    pub unglue: bool,
    /// 复制选中 way 并平移出平行线。
    pub parallel_way: bool,
    /// 使选中 way 的线段正交（水平/垂直）。
    pub orthogonalize: bool,
    /// 把选中的闭合 way 合并成一个 multipolygon 关系。
    pub join_areas: bool,
    /// 把选中节点移动到选中 way 上并接入。
    pub move_node_onto_way: bool,
    /// 把选中 way 挤出成闭合建筑轮廓。
    pub extrude: bool,
    /// 打开阶段 5 的插件 / 影像 / 预设覆盖窗口。
    pub toggle_ecosystem: bool,
    /// 删除选中的对象。
    pub delete_selected: bool,
    /// 从 journal 恢复。
    pub recover_journal: bool,
    /// 打开/关闭帮助窗口。
    pub toggle_help: bool,
    /// 打开图层信息窗口（图层右键菜单）。
    pub layer_info: bool,
    /// 切换图层显示（眼睛图标）。
    pub toggle_layer_visible: bool,
    /// 切换活动数据图层。
    pub toggle_layer_active: bool,
    /// 切换到指定图层（0 = 当前活动图层；其余为停靠图层下标 + 1）。
    pub layer_activate: Option<usize>,
    /// 切换指定数据图层可见性。
    pub layer_set_visible: Option<(usize, bool)>,
    /// 切换影像图层可见性。
    pub imagery_layer_visible: Option<(LayerRole, bool)>,
    /// 新建空图层。
    pub layer_new: bool,
    /// 删除停靠图层（下标 + 1）。
    pub layer_remove: Option<usize>,
    /// 把停靠图层合并进活动图层。
    pub layer_import: Option<usize>,
    /// 切换界面语言。
    pub set_language: Option<neo_i18n::Lang>,
    /// 打开/关闭设置窗口。
    pub toggle_settings: bool,
    /// 在文件管理器中打开数据目录。
    pub open_data_dir: bool,
    /// 清理自动保存缓存。
    pub clear_autosave: bool,
    /// 运行校验。
    pub run_validation: bool,
    /// 选中校验问题涉及的对象。
    pub select_objects: Option<Vec<(ObjectKind, ObjectId)>>,
    /// 开始 OAuth 登录。
    pub oauth_login: bool,
    /// 退出当前账户（保留记录）。
    pub oauth_logout: bool,
    /// 切换当前账户。
    pub oauth_select: Option<String>,
    /// 删除账户记录。
    pub oauth_remove: Option<String>,
    /// 关系成员编辑（增删/排序/改角色）。
    pub relation_member_edit: Option<RelationMemberEdit>,
    /// 打开/关闭查找窗口。
    pub toggle_search: bool,
    /// 打开/关闭 Notes 窗口。
    pub toggle_notes: bool,
    /// 刷新当前区域的 notes。
    pub notes_refresh: bool,
    /// 新建 note。
    pub note_create: bool,
    /// 评论选中的 note。
    pub note_comment: Option<i64>,
    /// 关闭选中的 note。
    pub note_close: Option<i64>,
    /// 定位到 note。
    pub note_focus: Option<(f64, f64)>,
    /// 打开/关闭 changeset 查看器。
    pub toggle_changeset: bool,
    /// 加载 changeset id。
    pub changeset_load: Option<i64>,
    /// 打开/关闭插件管理窗口。
    pub toggle_plugins: bool,
    /// 刷新插件列表。
    pub plugin_refresh: bool,
    /// 安装插件包。
    pub plugin_install: bool,
    /// 删除插件（id）。
    pub plugin_remove: Option<String>,
    /// 启用/禁用插件（id, enabled）。
    pub plugin_toggle: Option<(String, bool)>,
    /// 打开/关闭影像窗口。
    pub toggle_imagery: bool,
    /// 把 OSM 标准地图加进图层列表并显示。
    pub imagery_add_osm: bool,
    /// 按预设 id 把影像加进图层列表并显示。
    pub imagery_pick: Option<String>,
    /// 应用影像设置。
    pub imagery_apply: bool,
    /// 清除影像瓦片缓存。
    pub imagery_clear: bool,
    /// 导入影像偏移库 JSON。
    pub imagery_offset_load: bool,
    /// 导出当前偏移库。
    pub imagery_offset_save: bool,
    /// 应用最近偏移。
    pub imagery_apply_offset: bool,
    /// 选中查找结果并定位。
    pub focus_object: Option<(ObjectKind, ObjectId)>,
    /// 打开/关闭上传窗口（原“同步”窗口，现只负责上传；下载在独立窗口）。
    pub toggle_sync: bool,
    /// 上传本地修改。
    pub sync_upload: bool,
    /// 只上传选中对象的修改。
    pub sync_upload_selected: bool,
    /// 冲突后保留本地（对齐版本并重试上传）。
    pub sync_keep_local: bool,
    /// 用服务端对象覆盖单个本地冲突对象。
    pub sync_take_remote: Option<(ObjectKind, ObjectId)>,
    /// 合并远端独有标签（本地几何/已有标签保留）。
    pub sync_merge_remote: Option<(ObjectKind, ObjectId)>,
    /// 采用远端几何/成员，保留本地标签。
    pub sync_merge_geometry: Option<(ObjectKind, ObjectId)>,
    /// 打开/关闭导出窗口。
    pub toggle_export: bool,
    /// 用户在导出窗口选择格式并确认。
    pub export_now: Option<ExportFormat>,
    /// 打开/关闭下载窗口。
    pub toggle_download: bool,
    /// 开始在地图上框选下载范围。
    pub begin_bbox: bool,
    /// 下载已选 bbox。
    pub download_bbox: bool,
    /// 通过 Overpass 下载。
    pub download_overpass: bool,
    /// 清除 bbox。
    pub clear_bbox: bool,
}

/// 面板需要的只读状态快照。
#[derive(Clone, Copy)]
pub struct UiState<'a> {
    /// 当前工具模式。
    pub mode: UiMode,
    /// 可撤销。
    pub can_undo: bool,
    /// 可重做。
    pub can_redo: bool,
    /// 有未保存修改。
    pub dirty: bool,
    /// 有待上传的 OSM 数据变更（用于同步入口启用/置灰）。
    pub has_pending_changes: bool,
    /// 撤销栈长度。
    pub undo_len: usize,
    /// 重做栈长度。
    pub redo_len: usize,
    /// 当前缩放。
    pub zoom: f64,
    /// 数据源标签。
    pub label: &'a str,
    /// 最近操作状态。
    pub status: &'a str,
    /// 选中对象数。
    pub selection_len: usize,
    /// 相机中心（纬度, 经度）。
    pub center_latlon: (f64, f64),
    /// 鼠标所在经纬度（悬停地图时）。
    pub mouse_latlon: Option<(f64, f64)>,
    /// 当前缩放下的米/像素（比例尺）。
    pub meters_per_px: f64,
    /// 测距结果（米）。
    pub measure_distance_m: Option<f64>,
    /// 当前图层是否显示。
    pub layer_visible: bool,
    /// 当前图层是否为活动数据图层。
    pub layer_active: bool,
}

/// 导出格式。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExportFormat {
    /// OSM XML（.osm）。
    Xml,
    /// osmChange（.osc）。
    Osc,
    /// PBF（.osm.pbf）。
    Pbf,
    /// JOSM 会话（.jos/.joz）。
    Session,
}

/// 标签编辑器状态：单选与多选共用同一套行模型（多选按并集显示）。
#[derive(Default)]
pub struct TagsEditor {
    targets: Vec<(ObjectKind, ObjectId)>,
    rows: Vec<TagRow>,
    /// 行模型对应的数据版本；版本变化（撤销/重做/外部修改）时重新同步。
    synced_version: Option<u64>,
    preset_search: String,
    /// 当前预设分类 id；空表示全部分类。
    preset_category: String,
    /// 用户手动选过分组后不再跟随匹配结果。
    preset_pinned: bool,
    /// 正在同一列表里选分组（不另开下拉）。
    preset_picking: bool,
    /// 添加关系成员时使用的角色。
    relation_role: String,
    /// 正在编辑的成员角色：(relation, index, buffer)。
    member_role_edit: Option<(ObjectId, usize, String)>,
    /// 预设应用后待提交的标签修改（下一次 [`Self::show`] 返回）。
    pending: Option<TagEdit>,
}

impl TagsEditor {
    /// 空编辑器。
    pub fn new() -> Self {
        Self::default()
    }

    /// 当前编辑目标集合。
    pub fn targets(&self) -> &[(ObjectKind, ObjectId)] {
        &self.targets
    }

    /// 添加关系成员时使用的角色。
    pub fn relation_role(&self) -> &str {
        &self.relation_role
    }

    /// 目标集合或数据版本变化时需要重新扫描并集。
    pub fn needs_sync(&self, targets: &[(ObjectKind, ObjectId)], version: u64) -> bool {
        self.targets != targets || self.synced_version != Some(version)
    }

    /// 强制下一次同步重算（撤销/重做/删除后调用）。
    pub fn invalidate(&mut self) {
        self.synced_version = None;
    }

    /// 清空（没有选中对象时）。
    pub fn clear(&mut self) {
        self.targets.clear();
        self.rows.clear();
        self.synced_version = None;
        self.preset_search.clear();
        self.preset_category.clear();
        self.preset_pinned = false;
        self.preset_picking = false;
        self.member_role_edit = None;
        self.pending = None;
    }

    /// 用数据集当前的并集行模型同步编辑器。
    pub fn sync(&mut self, targets: &[(ObjectKind, ObjectId)], version: u64, rows: &[TagRow]) {
        let targets_changed = self.targets != targets;
        // 未失焦的新增行（草稿）在数据版本变化时保留，避免别的行提交后
        // 把用户正在输入的新行冲掉。
        let drafts: Vec<TagRow> = if targets_changed {
            Vec::new()
        } else {
            self.rows
                .iter()
                .filter(|row| row.original_key.is_empty())
                .cloned()
                .collect()
        };
        self.targets = targets.to_vec();
        self.synced_version = Some(version);
        self.rows = rows.to_vec();
        if targets_changed {
            self.preset_category.clear();
            self.preset_pinned = false;
            self.preset_picking = false;
        }
        for draft in drafts {
            let keep = !draft.key.trim().is_empty()
                || !matches!(&draft.value, TagValue::Same(value) if value.is_empty());
            if keep {
                self.rows.push(draft);
            }
        }
        self.pending = None;
    }

    /// 行模型里可落地的具体标签（用于预设匹配/差异计算）。
    pub fn concrete_tags(&self) -> Vec<(String, String)> {
        self.rows
            .iter()
            .filter_map(|row| match &row.value {
                TagValue::Same(value) if !row.key.trim().is_empty() && !value.trim().is_empty() => {
                    Some((row.key.trim().to_owned(), value.trim().to_owned()))
                }
                _ => None,
            })
            .collect()
    }

    fn upsert_concrete(&mut self, key: &str, value: &str) {
        if let Some(row) = self.rows.iter_mut().find(|row| row.key == key) {
            row.value = TagValue::Same(value.to_owned());
        } else {
            self.rows.push(TagRow {
                key: key.to_owned(),
                original_key: key.to_owned(),
                value: TagValue::Same(value.to_owned()),
            });
        }
    }

    /// 应用 schema 或品牌预设：先去掉现有 `area` 与 `remove` 里的 key，再写入标签。
    ///
    /// 修改暂存为一次待提交的 [`TagEdit`]，由下一次 [`Self::show`] 返回。
    pub fn apply_preset_owned(&mut self, tags: &[(String, String)], remove: &[String]) {
        if self.targets.is_empty() {
            return;
        }
        let before = self.concrete_tags();
        self.rows
            .retain(|row| row.key != "area" && !remove.iter().any(|gone| gone == &row.key));
        for (key, value) in tags {
            self.upsert_concrete(key, value);
        }
        let after = self.concrete_tags();
        let set: Vec<(String, String)> = after
            .iter()
            .filter(|(key, value)| {
                !before
                    .iter()
                    .any(|(old_key, old_value)| old_key == key && old_value == value)
            })
            .cloned()
            .collect();
        let remove_keys: Vec<String> = before
            .iter()
            .map(|(key, _)| key.clone())
            .filter(|key| !after.iter().any(|(new_key, _)| new_key == key))
            .collect();
        if !set.is_empty() || !remove_keys.is_empty() {
            self.pending = Some(TagEdit {
                targets: self.targets.clone(),
                set,
                remove: remove_keys,
            });
        }
    }

    /// 绘制编辑器。用户完成一次编辑（失焦/选择值/删除行）时返回标签修改。
    pub fn show(&mut self, ui: &mut Ui) -> Option<TagEdit> {
        if let Some(edit) = self.pending.take() {
            return Some(edit);
        }
        if self.targets.is_empty() {
            return None;
        }
        let targets = self.targets.clone();
        let mut edit: Option<TagEdit> = None;
        let mut remove_row: Option<usize> = None;
        let mut add_row = false;
        egui::ScrollArea::vertical()
            .id_salt("neo-tags-editor")
            .max_height(180.0)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                for (idx, row) in self.rows.iter_mut().enumerate() {
                    ui.horizontal(|ui| {
                        // 值不一致时 key 只读：跨对象改名没有确定语义。
                        if matches!(row.value, TagValue::Mixed(_)) {
                            ui.add_sized(
                                [90.0, ui.spacing().interact_size.y],
                                egui::Label::new(RichText::new(row.key.as_str()).monospace()).truncate(),
                            );
                        } else {
                            let key_resp =
                                ui.add(egui::TextEdit::singleline(&mut row.key).desired_width(90.0));
                            if key_resp.lost_focus() {
                                let new_key = row.key.trim().to_owned();
                                let value = match &row.value {
                                    TagValue::Same(value) => value.trim().to_owned(),
                                    TagValue::Mixed(_) => String::new(),
                                };
                                if !new_key.is_empty() {
                                    if row.original_key.is_empty() {
                                        if !value.is_empty() {
                                            edit = Some(TagEdit {
                                                targets: targets.clone(),
                                                set: vec![(new_key.clone(), value)],
                                                remove: Vec::new(),
                                            });
                                            row.original_key = new_key;
                                        }
                                    } else if new_key != row.original_key && !value.is_empty() {
                                        edit = Some(TagEdit {
                                            targets: targets.clone(),
                                            set: vec![(new_key.clone(), value)],
                                            remove: vec![row.original_key.clone()],
                                        });
                                        row.original_key = new_key;
                                    }
                                }
                            }
                        }
                        ui.label("=");
                        match &mut row.value {
                            TagValue::Same(value) => {
                                let value_resp =
                                    ui.add(egui::TextEdit::singleline(value).desired_width(112.0));
                                if value_resp.lost_focus() {
                                    let key = if row.original_key.is_empty() {
                                        row.key.trim().to_owned()
                                    } else {
                                        row.original_key.clone()
                                    };
                                    if !key.is_empty() {
                                        let new_value = value.trim().to_owned();
                                        let empty = new_value.is_empty();
                                        if row.original_key.is_empty() {
                                            // 新行：只在有值时才写入；清空不产生删除。
                                            if !empty {
                                                row.original_key = key.clone();
                                                edit = Some(TagEdit {
                                                    targets: targets.clone(),
                                                    set: vec![(key, new_value)],
                                                    remove: Vec::new(),
                                                });
                                            }
                                        } else {
                                            edit = Some(TagEdit {
                                                targets: targets.clone(),
                                                set: if empty {
                                                    Vec::new()
                                                } else {
                                                    vec![(key.clone(), new_value)]
                                                },
                                                remove: if empty { vec![key] } else { Vec::new() },
                                            });
                                        }
                                    }
                                }
                            }
                            TagValue::Mixed(values) => {
                                let button = ui.add_sized(
                                    [112.0, ui.spacing().interact_size.y],
                                    egui::Button::new("……"),
                                );
                                egui::Popup::menu(&button).width(160.0).show(|ui| {
                                    for value in values.iter() {
                                        if ui.button(value).clicked() {
                                            edit = Some(TagEdit {
                                                targets: targets.clone(),
                                                set: vec![(row.key.clone(), value.clone())],
                                                remove: Vec::new(),
                                            });
                                            ui.close();
                                        }
                                    }
                                });
                            }
                        }
                        if ui
                            .add(egui::Button::new(RichText::new(ph::X).size(12.0)).frame(false))
                            .clicked()
                        {
                            remove_row = Some(idx);
                        }
                    });
                }
            });
        if ui.button(format!("{} {}", ph::PLUS, t("tags.add"))).clicked() {
            add_row = true;
        }
        if let Some(idx) = remove_row {
            let row = self.rows.remove(idx);
            let key = if row.original_key.is_empty() {
                row.key.trim().to_owned()
            } else {
                row.original_key
            };
            if !key.is_empty() {
                let removal = TagEdit {
                    targets,
                    set: Vec::new(),
                    remove: vec![key],
                };
                edit = Some(match edit {
                    Some(mut current) => {
                        current.remove.extend(removal.remove);
                        current
                    }
                    None => removal,
                });
            }
        }
        if add_row {
            self.rows.push(TagRow {
                key: String::new(),
                original_key: String::new(),
                value: TagValue::Same(String::new()),
            });
        }
        edit
    }
}

/// 多个选中对象的标签并集（单选即其自身标签）。
///
/// key 顺序按对象顺序首次出现排列；值完全一致时是 [`TagValue::Same`]，否则
/// [`TagValue::Mixed`]（列出出现过的值）。
pub fn tag_rows_from(lists: &[Vec<(String, String)>]) -> Vec<TagRow> {
    if lists.is_empty() {
        return Vec::new();
    }
    let mut order: Vec<String> = Vec::new();
    for list in lists {
        for (key, _) in list {
            if !order.iter().any(|existing| existing == key) {
                order.push(key.clone());
            }
        }
    }
    order
        .into_iter()
        .map(|key| {
            let values: Vec<&str> = lists
                .iter()
                .filter_map(|list| {
                    list.iter()
                        .find(|(k, _)| *k == key)
                        .map(|(_, value)| value.as_str())
                })
                .collect();
            let all_same = values.len() == lists.len() && values.windows(2).all(|pair| pair[0] == pair[1]);
            let value = if all_same {
                TagValue::Same(values[0].to_owned())
            } else {
                let mut distinct: Vec<String> = Vec::new();
                for value in values {
                    if !distinct.iter().any(|seen| seen == value) {
                        distinct.push(value.to_owned());
                    }
                }
                TagValue::Mixed(distinct)
            };
            TagRow {
                key: key.clone(),
                original_key: key,
                value,
            }
        })
        .collect()
}

#[cfg(test)]
mod tags_editor_tests {
    use super::{TagValue, TagsEditor, tag_rows_from};
    use neo_core::{ObjectId, ObjectKind};

    fn tags(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
            .collect()
    }

    fn targets() -> Vec<(ObjectKind, ObjectId)> {
        vec![(ObjectKind::Way, ObjectId(1)), (ObjectKind::Way, ObjectId(2))]
    }

    #[test]
    fn union_rows_keep_common_values_and_mark_mixed() {
        let rows = tag_rows_from(&[
            tags(&[("highway", "residential"), ("name", "A"), ("surface", "asphalt")]),
            tags(&[("highway", "residential"), ("name", "B")]),
        ]);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].key, "highway");
        assert_eq!(rows[0].value, TagValue::Same("residential".into()));
        assert_eq!(rows[1].key, "name");
        assert_eq!(rows[1].value, TagValue::Mixed(vec!["A".into(), "B".into()]));
        // 只有第一个对象有 surface：也是混合，下拉里列出已有的值。
        assert_eq!(rows[2].key, "surface");
        assert_eq!(rows[2].value, TagValue::Mixed(vec!["asphalt".into()]));
    }

    #[test]
    fn single_object_rows_are_all_same() {
        let rows = tag_rows_from(&[tags(&[("building", "yes")])]);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].value, TagValue::Same("yes".into()));
        assert_eq!(rows[0].original_key, "building");
    }

    #[test]
    fn sync_only_reloads_on_target_or_version_change() {
        let mut editor = TagsEditor::new();
        let rows = tag_rows_from(&[tags(&[("highway", "residential")])]);
        let targets = targets();
        assert!(editor.needs_sync(&targets, 7));
        editor.sync(&targets, 7, &rows);
        assert!(!editor.needs_sync(&targets, 7));
        assert!(editor.needs_sync(&targets, 8));
        assert!(editor.needs_sync(&targets[..1], 7));
        editor.invalidate();
        assert!(editor.needs_sync(&targets, 7));
    }

    #[test]
    fn preset_change_returns_one_edit_for_all_targets() {
        let mut editor = TagsEditor::new();
        let rows = tag_rows_from(&[tags(&[("area", "yes")])]);
        editor.sync(&targets(), 7, &rows);
        editor.apply_preset_owned(&[("building".into(), "yes".into())], &[]);
        let edit = editor.pending.clone().expect("预设应产生待提交修改");
        assert_eq!(edit.targets, targets());
        assert!(edit.set.contains(&("building".to_owned(), "yes".to_owned())));
        // 旧实现会先清掉绘制时自动写入的 area，这里同样作为删除提交。
        assert_eq!(edit.remove, vec!["area".to_owned()]);
    }

    #[test]
    fn concrete_tags_skips_mixed_rows() {
        let mut editor = TagsEditor::new();
        let rows = tag_rows_from(&[
            tags(&[("highway", "residential"), ("name", "A")]),
            tags(&[("highway", "residential"), ("name", "B")]),
        ]);
        editor.sync(&targets(), 7, &rows);
        assert_eq!(
            editor.concrete_tags(),
            vec![("highway".to_owned(), "residential".to_owned())]
        );
    }
}
