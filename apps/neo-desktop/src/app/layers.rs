//! NeoApp::layers methods. Same behavior as before the app.rs split.

use neo_editor::Editor;
use neo_i18n::t;
use neo_io::{XmlWriteMode, write_osm_xml_file};
use neo_store::DataSet;

use super::{NeoApp, ParkedLayer, layers_dir};

impl NeoApp {
    pub(super) fn save_parked_layer(&mut self, index: usize) {
        let Some(layer) = self.parked_layers.get_mut(index) else {
            return;
        };
        let dir = layers_dir();
        let _ = std::fs::create_dir_all(&dir);
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        let path = dir.join(format!("layer-{stamp}-{index}.osm"));
        if write_osm_xml_file(layer.editor.data(), &path, XmlWriteMode::Josm).is_ok() {
            layer.persist = Some(path);
        }
    }

    pub(super) fn layers_view(&self) -> Vec<neo_ui::LayerView> {
        let mut layers = Vec::new();
        if self.data_listed {
            layers.push(neo_ui::LayerView {
                label: self.data_label.clone(),
                visible: self.layer_visible,
                active: true,
                role: neo_ui::LayerRole::Data,
            });
            for layer in &self.parked_layers {
                layers.push(neo_ui::LayerView {
                    label: layer.label.clone(),
                    visible: layer.visible,
                    active: false,
                    role: neo_ui::LayerRole::Data,
                });
            }
        }
        if self.osm_listed {
            layers.push(neo_ui::LayerView {
                label: t("imagery.layer.osm").into(),
                visible: self.osm_visible,
                active: false,
                role: neo_ui::LayerRole::Osm,
            });
        }
        if self.imagery_listed {
            layers.push(neo_ui::LayerView {
                label: self.imagery_layer_label(),
                visible: self.imagery.enabled,
                active: false,
                role: neo_ui::LayerRole::Imagery,
            });
        }
        layers
    }

    pub(super) fn activate_listed_layer(&mut self, index: usize) {
        match self.layer_slot(index) {
            Some(LayerSlot::Data) => self.layer_active = true,
            Some(LayerSlot::Parked(position)) => self.switch_parked(position),
            _ => {}
        }
    }

    pub(super) fn set_layer_visible(&mut self, index: usize, visible: bool) {
        match self.layer_slot(index) {
            Some(LayerSlot::Data) => {
                self.layer_visible = visible;
                self.scene = None;
            }
            Some(LayerSlot::Parked(position)) => {
                if let Some(layer) = self.parked_layers.get_mut(position) {
                    layer.visible = visible;
                }
                self.scene = None;
            }
            Some(LayerSlot::Osm) => self.osm_visible = visible,
            Some(LayerSlot::Imagery) => self.imagery.enabled = visible,
            None => {}
        }
    }

    /// The parked data layer at the given view index. Used when copying into the active layer.
    pub(super) fn parked_slot(&self, index: usize) -> Option<usize> {
        match self.layer_slot(index) {
            Some(LayerSlot::Parked(position)) => Some(position),
            _ => None,
        }
    }

    fn layer_slot(&self, index: usize) -> Option<LayerSlot> {
        let mut cursor = 0usize;
        if self.data_listed {
            if cursor == index {
                return Some(LayerSlot::Data);
            }
            cursor += 1;
            for position in 0..self.parked_layers.len() {
                if cursor == index {
                    return Some(LayerSlot::Parked(position));
                }
                cursor += 1;
            }
        }
        if self.osm_listed {
            if cursor == index {
                return Some(LayerSlot::Osm);
            }
            cursor += 1;
        }
        if self.imagery_listed && cursor == index {
            return Some(LayerSlot::Imagery);
        }
        None
    }

    fn imagery_layer_label(&self) -> String {
        match self.imagery.preset_id.as_str() {
            "" => t("imagery.layer.base").into(),
            "esri-world" => t("imagery.preset.esri").into(),
            "esri-clarity" => t("imagery.preset.esri_clarity").into(),
            "bing" => t("imagery.preset.bing").into(),
            "mapbox" => t("imagery.preset.mapbox").into(),
            "tianditu" => t("imagery.preset.tianditu").into(),
            "sentinel-2025" => t("imagery.preset.sentinel2025").into(),
            "sentinel-2020" => t("imagery.preset.sentinel2020").into(),
            "osm" => t("imagery.preset.osm").into(),
            id => neo_style::imagery_presets()
                .iter()
                .find(|preset| preset.id == id)
                .map(|preset| preset.name.clone())
                .unwrap_or_else(|| id.to_string()),
        }
    }

    fn switch_parked(&mut self, position: usize) {
        if position >= self.parked_layers.len() {
            return;
        }
        let parked = self.parked_layers.remove(position);
        let ParkedLayer {
            label,
            path,
            persist: _,
            editor,
            builder,
            visible: _,
            downloaded_bboxes,
        } = parked;
        let old_editor = std::mem::replace(&mut self.editor, editor);
        let old_builder = std::mem::replace(&mut self.builder, builder);
        let old_label = std::mem::replace(&mut self.data_label, label);
        let old_path = self.data_path.take();
        let old_visible = self.layer_visible;
        let old_bboxes = std::mem::replace(&mut self.downloaded_bboxes, downloaded_bboxes);
        self.parked_layers.push(ParkedLayer {
            label: old_label,
            path: old_path,
            persist: None,
            editor: old_editor,
            builder: old_builder,
            visible: old_visible,
            downloaded_bboxes: old_bboxes,
        });
        self.save_parked_layer(self.parked_layers.len() - 1);
        self.data_path = path;
        self.layer_visible = true;
        self.layer_active = true;
        self.scene = None;
        self.validation = None;
        self.tags.clear();
        self.editor.reset_journal();
        self.data_listed = true;
        self.reselect_after_switch();
        self.persist_state();
    }

    pub(super) fn reselect_after_switch(&mut self) {
        self.editor.selection_mut().clear();
    }

    pub(super) fn new_layer(&mut self) {
        if !self.data_listed {
            self.data_listed = true;
            self.layer_visible = true;
            self.layer_active = true;
            self.data_label = format!("Layer {}", self.parked_layers.len() + 1);
            self.scene = None;
            return;
        }
        self.park_active_layer();
        let label = format!("Layer {}", self.parked_layers.len() + 1);
        self.layer_visible = true;
        self.layer_active = true;
        self.set_editor(Editor::new(DataSet::new()), label);
        self.reopen_journal();
    }

    /// Parks the active data layer into `parked_layers`, leaving the active slot for the data loaded next.
    pub(super) fn park_active_layer(&mut self) {
        let mut editor = Editor::new(DataSet::new());
        let builder = super::scene_builder(&mut editor);
        let old_editor = std::mem::replace(&mut self.editor, editor);
        let old_builder = std::mem::replace(&mut self.builder, builder);
        let old_label = std::mem::take(&mut self.data_label);
        let old_path = self.data_path.take();
        let old_visible = self.layer_visible;
        let old_bboxes = std::mem::take(&mut self.downloaded_bboxes);
        self.parked_layers.push(ParkedLayer {
            label: old_label,
            path: old_path,
            persist: None,
            editor: old_editor,
            builder: old_builder,
            visible: old_visible,
            downloaded_bboxes: old_bboxes,
        });
        self.save_parked_layer(self.parked_layers.len() - 1);
    }

    /// Whether any data layer has unsaved edits.
    pub(super) fn any_unsaved_edits(&self) -> bool {
        if self.data_listed && self.editor.history().is_dirty() {
            return true;
        }
        self.parked_layers
            .iter()
            .any(|layer| layer.editor.history().is_dirty())
    }

    /// Whether a layer list item has unsaved edits (data layers only).
    pub(super) fn layer_has_unsaved(&self, index: usize) -> bool {
        match self.layer_slot(index) {
            Some(LayerSlot::Data) => self.editor.history().is_dirty(),
            Some(LayerSlot::Parked(position)) => self
                .parked_layers
                .get(position)
                .is_some_and(|layer| layer.editor.history().is_dirty()),
            _ => false,
        }
    }

    pub(super) fn remove_layer(&mut self, index: usize) {
        match self.layer_slot(index) {
            Some(LayerSlot::Data) => self.close_active_layer(),
            Some(LayerSlot::Parked(position)) => {
                let layer = self.parked_layers.remove(position);
                if let Some(path) = layer.persist {
                    let _ = std::fs::remove_file(path);
                }
                self.scene = None;
                self.status = t("layer.removed").to_owned();
            }
            Some(LayerSlot::Osm) => {
                self.osm_listed = false;
                self.osm_visible = false;
                self.drop_imagery_source(super::SOURCE_OSM);
                self.status = t("layer.removed").into();
            }
            Some(LayerSlot::Imagery) => {
                self.imagery_listed = false;
                self.imagery.enabled = false;
                self.drop_imagery_source(super::SOURCE_CUSTOM);
                self.status = t("layer.removed").into();
            }
            None => {}
        }
    }

    /// Closes the data layer currently being edited. If other data layers exist, switch to one of them; otherwise go back to empty data
    /// and forget the file path, so this download is not reopened on the next start.
    fn close_active_layer(&mut self) {
        if self.parked_layers.is_empty() {
            self.data_path = None;
            self.layer_visible = true;
            self.set_editor(Editor::new(DataSet::new()), t("app.data.empty").into());
            self.data_listed = false;
            self.reopen_journal();
            self.status = t("layer.removed").into();
            return;
        }
        let parked = self.parked_layers.remove(0);
        let super::ParkedLayer {
            label,
            path,
            persist,
            editor,
            builder: _,
            visible: _,
            downloaded_bboxes,
        } = parked;
        self.data_path = adopt_layer_path(path, persist);
        self.layer_visible = true;
        self.set_editor(editor, label);
        self.downloaded_bboxes = downloaded_bboxes;
        self.reopen_journal();
        self.status = t("layer.removed").into();
    }
}

/// Snapshots of parked layers live in `layers/`. They must be moved once promoted to the active layer, otherwise another copy is loaded on the next start.
fn adopt_layer_path(
    path: Option<std::path::PathBuf>,
    persist: Option<std::path::PathBuf>,
) -> Option<std::path::PathBuf> {
    match (path, persist) {
        (Some(path), Some(persist)) => {
            let _ = std::fs::remove_file(persist);
            Some(path)
        }
        (None, Some(persist)) => {
            let name = persist
                .file_name()
                .unwrap_or_else(|| std::ffi::OsStr::new("parked.osm"));
            let kept = super::data_dir().join(name);
            if persist != kept
                && std::fs::rename(&persist, &kept).is_err()
                && std::fs::copy(&persist, &kept).is_ok()
            {
                let _ = std::fs::remove_file(&persist);
            }
            Some(if kept.exists() { kept } else { persist })
        }
        (path, None) => path,
    }
}

#[derive(Clone, Copy)]
enum LayerSlot {
    Data,
    Parked(usize),
    Osm,
    Imagery,
}
