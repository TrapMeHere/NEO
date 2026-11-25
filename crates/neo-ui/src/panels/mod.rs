//! JOSM-style UI components (modernized).
//!
//! Layout follows JOSM:
//! * top: menu bar + main toolbar;
//! * left: vertical **tool palette** (select/draw/delete/measure + geometry operations);
//! * right: **Layers** (right-click → info/zoom to layer), **Tags/Members**, **Selection**, **Relations**,
//!   **Validation results**, **Conflicts** panels stacked;
//! * bottom: **status bar** (lat/lon, scale, measure result, zoom, undo/redo, action hint).
//!
//! Icons come from the open-source Phosphor Icons (`egui-phosphor`, MIT). Panels are pure functions.

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

/// UI-side working mode.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum UiMode {
    /// Select/move (no dedicated toolbar button; returns to this mode when no point/line/area is selected).
    #[default]
    Select,
    /// Create a node.
    DrawPoint,
    /// Draw a polyline.
    DrawLine,
    /// Draw a closed area.
    DrawArea,
    /// Measure.
    Measure,
    /// Split at point: clicking a node on a path splits every path through that node.
    SplitPoint,
}

/// Relation member edit intent (translated by the app layer into a `SetRelationMembers` command).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RelationMemberEdit {
    /// Add the currently selected objects to a relation (the role comes from the tag editor's "member role").
    Add {
        /// Relation ID.
        relation: ObjectId,
    },
    /// Remove the member at the given index.
    Remove {
        /// Relation ID.
        relation: ObjectId,
        /// Member index.
        index: usize,
    },
    /// Move the member up.
    MoveUp {
        /// Relation ID.
        relation: ObjectId,
        /// Member index.
        index: usize,
    },
    /// Move the member down.
    MoveDown {
        /// Relation ID.
        relation: ObjectId,
        /// Member index.
        index: usize,
    },
    /// Change the member role.
    SetRole {
        /// Relation ID.
        relation: ObjectId,
        /// Member index.
        index: usize,
        /// New role.
        role: String,
    },
}

/// Whether a layer is data or imagery.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LayerRole {
    /// Editable OSM data.
    Data,
    /// The currently selected background layer in the imagery window.
    Imagery,
    /// Standard OSM raster map.
    Osm,
}

/// A data or imagery layer view.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LayerView {
    /// Name.
    pub label: String,
    /// Whether the layer is visible.
    pub visible: bool,
    /// Whether this is the active (editable) layer.
    pub active: bool,
    /// Data, current imagery, or an OSM raster map. Every layer in the list can be shown or deleted.
    pub role: LayerRole,
}

/// One tag edit: apply `set` (add/overwrite) to every object in `targets`, and delete the keys in `remove`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TagEdit {
    /// Objects to modify (kind, id).
    pub targets: Vec<(ObjectKind, ObjectId)>,
    /// Tags to add/overwrite.
    pub set: Vec<(String, String)>,
    /// Keys to delete from all objects.
    pub remove: Vec<String>,
}

/// State of a tag value across the selected set.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TagValue {
    /// All selected objects have the same value for this key.
    Same(String),
    /// Values differ or some objects lack the key; keeps the values seen (deduplicated, in first-seen order).
    Mixed(Vec<String>),
}

/// One row of the tag editor (multiple selected objects shown as a union).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TagRow {
    /// Current key (editable).
    pub key: String,
    /// Key at sync time; empty means a row the user just added.
    pub original_key: String,
    /// Value state.
    pub value: TagValue,
}

/// Intent produced by the UI, translated by the app layer into edit commands/IO/camera operations.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UiActions {
    /// Open an OSM data file (XML/PBF), always as a new layer.
    pub open_file: bool,
    /// Open PBF (legacy entry point; same as [`Self::open_file`]).
    pub open_pbf: bool,
    /// Open OSM XML (legacy entry point; same as [`Self::open_file`]).
    pub open_xml: bool,
    /// Import GPX.
    pub open_gpx: bool,
    /// Export GPX.
    pub save_gpx: bool,
    /// Save as OSM XML.
    pub save_xml: bool,
    /// Export osmChange.
    pub save_osc: bool,
    /// Export PBF.
    pub save_pbf: bool,
    /// Save session (.jos/.joz).
    pub save_session: bool,
    /// Open session.
    pub open_session: bool,
    /// Reset view (zoom to data).
    pub reset_view: bool,
    /// Zoom in.
    pub zoom_in: bool,
    /// Zoom out.
    pub zoom_out: bool,
    /// Undo.
    pub undo: bool,
    /// Redo.
    pub redo: bool,
    /// Undo N steps (clicked in the command history panel).
    pub undo_to: Option<usize>,
    /// Copy selected objects.
    pub copy_selection: bool,
    /// Paste clipboard.
    pub paste: bool,
    /// Copy and offset-paste selected objects.
    pub duplicate: bool,
    /// Switch mode.
    pub mode: Option<UiMode>,
    /// Finish drawing the current way.
    pub finish_way: bool,
    /// Tag edit (produced on focus loss or when a dropdown value is picked).
    pub tag_edit: Option<TagEdit>,
    /// Split the selected ways.
    pub split_way: bool,
    /// Reverse the selected ways.
    pub reverse_way: bool,
    /// Merge the two selected ways.
    pub merge_ways: bool,
    /// Simplify the selected ways (RDP).
    pub simplify_way: bool,
    /// Glue the selected nodes into one (JOSM glue).
    pub merge_nodes: bool,
    /// Align nodes (along their main distribution axis).
    pub align_nodes: bool,
    /// Distribute nodes evenly.
    pub distribute_nodes: bool,
    /// Mirror horizontally (flip latitude).
    pub mirror_horizontal: bool,
    /// Mirror vertically (flip longitude).
    pub mirror_vertical: bool,
    /// Unglue selected nodes (each referencing way gets its own copy).
    pub unglue: bool,
    /// Copy the selected ways and offset them into a parallel line.
    pub parallel_way: bool,
    /// Make the selected way's segments orthogonal (horizontal/vertical).
    pub orthogonalize: bool,
    /// Merge the selected closed ways into one multipolygon relation.
    pub join_areas: bool,
    /// Move the selected nodes onto the selected way and join them.
    pub move_node_onto_way: bool,
    /// Extrude the selected way into a closed building outline.
    pub extrude: bool,
    /// Open the Phase 5 plugins / imagery / preset override windows.
    pub toggle_ecosystem: bool,
    /// Delete the selected objects.
    pub delete_selected: bool,
    /// Recover from the journal.
    pub recover_journal: bool,
    /// Open/close the help window.
    pub toggle_help: bool,
    /// Open the layer info window (layer context menu).
    pub layer_info: bool,
    /// Toggle layer visibility (eye icon).
    pub toggle_layer_visible: bool,
    /// Toggle the active data layer.
    pub toggle_layer_active: bool,
    /// Switch to a specific layer (0 = current active layer; others are parked layer index + 1).
    pub layer_activate: Option<usize>,
    /// Toggle visibility of a specific data layer.
    pub layer_set_visible: Option<(usize, bool)>,
    /// Toggle imagery layer visibility.
    pub imagery_layer_visible: Option<(LayerRole, bool)>,
    /// Create a new empty layer.
    pub layer_new: bool,
    /// Remove a parked layer (index + 1).
    pub layer_remove: Option<usize>,
    /// Merge a parked layer into the active layer.
    pub layer_import: Option<usize>,
    /// Switch the UI language.
    pub set_language: Option<neo_i18n::Lang>,
    /// Open/close the settings window.
    pub toggle_settings: bool,
    /// Open the data directory in the file manager.
    pub open_data_dir: bool,
    /// Clear the autosave cache.
    pub clear_autosave: bool,
    /// Run validation.
    pub run_validation: bool,
    /// Select the objects involved in a validation issue.
    pub select_objects: Option<Vec<(ObjectKind, ObjectId)>>,
    /// Start OAuth login.
    pub oauth_login: bool,
    /// Log out the current account (keep the record).
    pub oauth_logout: bool,
    /// Switch the current account.
    pub oauth_select: Option<String>,
    /// Delete an account record.
    pub oauth_remove: Option<String>,
    /// Relation member edit (add/remove/reorder/change roles).
    pub relation_member_edit: Option<RelationMemberEdit>,
    /// Open/close the search window.
    pub toggle_search: bool,
    /// Open/close the Notes window.
    pub toggle_notes: bool,
    /// Refresh notes in the current area.
    pub notes_refresh: bool,
    /// Create a note.
    pub note_create: bool,
    /// Comment on the selected note.
    pub note_comment: Option<i64>,
    /// Close the selected note.
    pub note_close: Option<i64>,
    /// Navigate to a note.
    pub note_focus: Option<(f64, f64)>,
    /// Open/close the changeset viewer.
    pub toggle_changeset: bool,
    /// Load a changeset id.
    pub changeset_load: Option<i64>,
    /// Open/close the plugin manager window.
    pub toggle_plugins: bool,
    /// Refresh the plugin list.
    pub plugin_refresh: bool,
    /// Install a plugin bundle.
    pub plugin_install: bool,
    /// Remove a plugin (id).
    pub plugin_remove: Option<String>,
    /// Enable/disable a plugin (id, enabled).
    pub plugin_toggle: Option<(String, bool)>,
    /// Open/close the imagery window.
    pub toggle_imagery: bool,
    /// Add the standard OSM map to the layer list and show it.
    pub imagery_add_osm: bool,
    /// Add imagery by preset id to the layer list and show it.
    pub imagery_pick: Option<String>,
    /// Apply imagery settings.
    pub imagery_apply: bool,
    /// Clear the imagery tile cache.
    pub imagery_clear: bool,
    /// Import an imagery offset database JSON.
    pub imagery_offset_load: bool,
    /// Export the current offset database.
    pub imagery_offset_save: bool,
    /// Apply the most recent offset.
    pub imagery_apply_offset: bool,
    /// Select and navigate to a search result.
    pub focus_object: Option<(ObjectKind, ObjectId)>,
    /// Open/close the upload window (formerly the "sync" window; now upload-only, downloads happen in a separate window).
    pub toggle_sync: bool,
    /// Upload local changes.
    pub sync_upload: bool,
    /// Upload the changes of the selected objects only.
    pub sync_upload_selected: bool,
    /// Keep local after a conflict (align the version and retry the upload).
    pub sync_keep_local: bool,
    /// Overwrite a single local conflicting object with the server object.
    pub sync_take_remote: Option<(ObjectKind, ObjectId)>,
    /// Merge tags that only exist on the remote (local geometry/existing tags are kept).
    pub sync_merge_remote: Option<(ObjectKind, ObjectId)>,
    /// Take the remote geometry/members but keep local tags.
    pub sync_merge_geometry: Option<(ObjectKind, ObjectId)>,
    /// Open/close the export window.
    pub toggle_export: bool,
    /// The user chose a format in the export window and confirmed.
    pub export_now: Option<ExportFormat>,
    /// Open/close the download window.
    pub toggle_download: bool,
    /// Start selecting the download area on the map.
    pub begin_bbox: bool,
    /// Download the selected bbox.
    pub download_bbox: bool,
    /// Download via Overpass.
    pub download_overpass: bool,
    /// Clear the bbox.
    pub clear_bbox: bool,
}

/// Read-only state snapshot the panels need.
#[derive(Clone, Copy)]
pub struct UiState<'a> {
    /// Current tool mode.
    pub mode: UiMode,
    /// Can undo.
    pub can_undo: bool,
    /// Can redo.
    pub can_redo: bool,
    /// Has unsaved changes.
    pub dirty: bool,
    /// There are pending OSM data changes waiting to upload (used to enable/grey out the sync entry).
    pub has_pending_changes: bool,
    /// Undo stack length.
    pub undo_len: usize,
    /// Redo stack length.
    pub redo_len: usize,
    /// Current zoom.
    pub zoom: f64,
    /// Data source label.
    pub label: &'a str,
    /// Latest action status.
    pub status: &'a str,
    /// Number of selected objects.
    pub selection_len: usize,
    /// Camera center (latitude, longitude).
    pub center_latlon: (f64, f64),
    /// Lat/lon under the mouse (while hovering the map).
    pub mouse_latlon: Option<(f64, f64)>,
    /// Meters per pixel at the current zoom (scale bar).
    pub meters_per_px: f64,
    /// Measure result (meters).
    pub measure_distance_m: Option<f64>,
    /// Whether the current layer is visible.
    pub layer_visible: bool,
    /// Whether the current layer is an active data layer.
    pub layer_active: bool,
}

/// Export format.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExportFormat {
    /// OSM XML (.osm).
    Xml,
    /// osmChange (.osc).
    Osc,
    /// PBF (.osm.pbf).
    Pbf,
    /// JOSM session (.jos/.joz).
    Session,
}

/// Tag editor state: single and multi selection share one row model (multi selection shows the union).
#[derive(Default)]
pub struct TagsEditor {
    targets: Vec<(ObjectKind, ObjectId)>,
    rows: Vec<TagRow>,
    /// Data version the row model corresponds to; resync when the version changes (undo/redo/external edit).
    synced_version: Option<u64>,
    preset_search: String,
    /// Current preset category id; empty means all categories.
    preset_category: String,
    /// Once the user picks a group manually, the editor stops following the match result.
    preset_pinned: bool,
    /// Currently picking a group inside the same list (no separate dropdown).
    preset_picking: bool,
    /// Role used when adding relation members.
    relation_role: String,
    /// Member role being edited: (relation, index, buffer).
    member_role_edit: Option<(ObjectId, usize, String)>,
    /// Tag edit waiting to be committed after a preset is applied (returned by the next [`Self::show`]).
    pending: Option<TagEdit>,
}

impl TagsEditor {
    /// Empty editor.
    pub fn new() -> Self {
        Self::default()
    }

    /// Current edit target set.
    pub fn targets(&self) -> &[(ObjectKind, ObjectId)] {
        &self.targets
    }

    /// Role used when adding relation members.
    pub fn relation_role(&self) -> &str {
        &self.relation_role
    }

    /// Whether the union must be rescanned because the target set or data version changed.
    pub fn needs_sync(&self, targets: &[(ObjectKind, ObjectId)], version: u64) -> bool {
        self.targets != targets || self.synced_version != Some(version)
    }

    /// Force a resync on the next call (called after undo/redo/delete).
    pub fn invalidate(&mut self) {
        self.synced_version = None;
    }

    /// Clear (when nothing is selected).
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

    /// Sync the editor from the dataset's current union row model.
    pub fn sync(&mut self, targets: &[(ObjectKind, ObjectId)], version: u64, rows: &[TagRow]) {
        let targets_changed = self.targets != targets;
        // Draft rows the user has not unfocused (new rows) are kept across data-version changes,
        // so other rows committing do not wipe the row the user is typing.
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

    /// Concrete tags that can be committed from the row model (used for preset matching/diff computation).
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

    /// Apply a schema or brand preset: first drop the existing `area` and the keys in `remove`, then write the tags.
    ///
    /// The change is staged as one pending [`TagEdit`], returned by the next [`Self::show`].
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

    /// Draw the editor. Returns a tag edit when the user finishes an edit (focus loss/selecting a value/deleting a row).
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
                        // Keep the key read-only when values differ: renaming across objects has no well-defined semantics.
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
                                            // New row: only write when it has a value; clearing it produces no deletion.
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

/// Union of the tags of multiple selected objects (for a single selection it is just its own tags).
///
/// Key order follows first appearance across object order; when all values are identical it is [`TagValue::Same`], otherwise
/// [`TagValue::Mixed`] (listing the values seen).
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
        // Only the first object has surface: also mixed, the dropdown lists the values that exist.
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
        let edit = editor
            .pending
            .clone()
            .expect("preset should produce a pending edit");
        assert_eq!(edit.targets, targets());
        assert!(edit.set.contains(&("building".to_owned(), "yes".to_owned())));
        // The old implementation first removed the `area` tag written automatically while drawing; here it is likewise committed as a deletion.
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
