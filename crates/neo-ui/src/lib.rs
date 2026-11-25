//! # neo-ui — egui interface layer (JOSM style, modernized)
//!
//! Layout aligned with JOSM: top menu + common toolbar, left vertical tool palette, right layer/tag/selection/relation panels,
//! and a bottom status bar with lat/lon and scale. Icons from open-source Phosphor Icons (MIT).

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod panels;

pub use panels::{
    AccountView, BboxView, ChangesetUiState, ChangesetView, DownloadTab, DownloadUiState, ExportFormat,
    ExportUiState, ImageryUiState, LayerRole, LayerView, NoteCommentView, NoteView, NotesUiState,
    PluginUiState, PluginView, RelationMemberEdit, SearchHit, SearchUiState, SettingsTab, SettingsUiState,
    SyncConflict, TagEdit, TagsEditor, UiActions, UiMode, UiState, ValidationItem, ValidationView,
    apply_modern_style, changeset_window, download_window, ecosystem_window, export_window, help_window,
    imagery_window, layer_info_window, menu_bar, notes_window, plugins_window, right_side_panel,
    search_window, settings_window, status_bar, sync_window, tool_palette, top_toolbar,
};
