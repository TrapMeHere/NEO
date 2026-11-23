//! # neo-ui — egui 界面层（JOSM 风格，现代化）
//!
//! 布局对齐 JOSM：顶部菜单 + 常用工具栏、左侧竖排工具栏、右侧图层/标签/选择/关系面板、
//! 底部经纬度与比例尺状态栏。图标来自开源 Phosphor Icons（MIT）。

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod panels;

pub use panels::{
    AccountView, BboxView, ChangesetUiState, ChangesetView, DownloadTab, DownloadUiState, ExportFormat,
    ExportUiState, FilterUiState, ImageryUiState, LayerRole, LayerView, NoteCommentView, NoteView,
    NotesUiState, PluginUiState, PluginView, RelationMemberEdit, SearchHit, SearchUiState, SettingsTab,
    SettingsUiState, SyncConflict, TagsEditor, UiActions, UiMode, UiState, ValidationItem, ValidationView,
    apply_modern_style, changeset_window, download_window, ecosystem_window, export_window, filter_window,
    help_window, imagery_window, layer_info_window, menu_bar, notes_window, plugins_window, right_side_panel,
    search_window, settings_window, status_bar, sync_window, tool_palette, top_toolbar,
};
