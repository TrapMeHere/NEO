//! download 面板。

use egui::{Color32, RichText};
use egui_phosphor::regular as ph;
use neo_i18n::t;

use super::UiActions;

/// 下载范围（经纬度）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BboxView {
    /// 最小经度。
    pub min_lon: f64,
    /// 最小纬度。
    pub min_lat: f64,
    /// 最大经度。
    pub max_lon: f64,
    /// 最大纬度。
    pub max_lat: f64,
}

impl BboxView {
    /// 人类可读的 bbox 文本。
    pub fn text(&self) -> String {
        format!(
            "{:.5}, {:.5} — {:.5}, {:.5}",
            self.min_lon, self.min_lat, self.max_lon, self.max_lat
        )
    }

    /// Overpass QL 中的 `(south,west,north,east)`。
    pub fn overpass_bbox(&self) -> String {
        format!(
            "({:.7},{:.7},{:.7},{:.7})",
            self.min_lat, self.min_lon, self.max_lat, self.max_lon
        )
    }
}

/// 下载窗口当前标签页。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum DownloadTab {
    /// 框选后走 OSM `/map`。
    #[default]
    Bbox,
    /// Overpass QL 查询。
    Overpass,
}

/// 下载窗口状态。
#[derive(Clone, Debug)]
pub struct DownloadUiState {
    /// 当前标签页。
    pub tab: DownloadTab,
    /// 已框选的范围。
    pub bbox: Option<BboxView>,
    /// 是否正在地图上框选。
    pub selecting: bool,
    /// Overpass 服务端点。
    pub overpass_endpoint: String,
    /// Overpass QL 正文。
    pub overpass_query: String,
    /// 辅助构建：标签键。
    pub overpass_key: String,
    /// 辅助构建：标签值（空表示仅按 key 过滤）。
    pub overpass_value: String,
    /// 辅助构建：包含 node。
    pub overpass_nodes: bool,
    /// 辅助构建：包含 way。
    pub overpass_ways: bool,
    /// 辅助构建：包含 relation。
    pub overpass_relations: bool,
}

impl Default for DownloadUiState {
    fn default() -> Self {
        Self {
            tab: DownloadTab::Bbox,
            bbox: None,
            selecting: false,
            overpass_endpoint: "https://overpass-api.de/api/interpreter".to_owned(),
            overpass_query: String::new(),
            overpass_key: String::new(),
            overpass_value: String::new(),
            overpass_nodes: true,
            overpass_ways: true,
            overpass_relations: true,
        }
    }
}

impl DownloadUiState {
    /// 按当前辅助字段与框选范围生成 Overpass QL。
    pub fn build_overpass_query(&self) -> String {
        build_overpass_query(
            self.bbox,
            &self.overpass_key,
            &self.overpass_value,
            self.overpass_nodes,
            self.overpass_ways,
            self.overpass_relations,
        )
    }
}

/// 由标签过滤与 bbox 拼出基础 Overpass QL。
pub fn build_overpass_query(
    bbox: Option<BboxView>,
    key: &str,
    value: &str,
    nodes: bool,
    ways: bool,
    relations: bool,
) -> String {
    let bbox_clause = bbox
        .map(|b| b.overpass_bbox())
        .unwrap_or_else(|| "({{bbox}})".to_owned());
    let key = key.trim();
    let value = value.trim();
    let filter = if key.is_empty() {
        String::new()
    } else if value.is_empty() {
        format!("[\"{}\"]", escape_ql(key))
    } else {
        format!("[\"{}\"=\"{}\"]", escape_ql(key), escape_ql(value))
    };

    let mut lines = Vec::new();
    lines.push("[out:xml][timeout:60];".to_owned());
    lines.push("(".to_owned());
    if nodes {
        lines.push(format!("  node{filter}{bbox_clause};"));
    }
    if ways {
        lines.push(format!("  way{filter}{bbox_clause};"));
    }
    if relations {
        lines.push(format!("  relation{filter}{bbox_clause};"));
    }
    if !nodes && !ways && !relations {
        lines.push(format!("  node{filter}{bbox_clause};"));
    }
    lines.push(");".to_owned());
    lines.push("(._;>;);".to_owned());
    lines.push("out meta;".to_owned());
    lines.join("\n")
}

fn escape_ql(raw: &str) -> String {
    raw.replace('\\', "\\\\").replace('"', "\\\"")
}

/// 下载窗口：框选 OSM `/map`，或 Overpass QL 查询。
pub fn download_window(
    ctx: &egui::Context,
    actions: &mut UiActions,
    open: &mut bool,
    state: &mut DownloadUiState,
    status: &str,
) {
    egui::Window::new(format!("{} {}", ph::CLOUD_ARROW_DOWN, t("download.title")))
        .open(open)
        .collapsible(false)
        .resizable(true)
        .default_width(520.0)
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                if ui
                    .selectable_label(state.tab == DownloadTab::Bbox, t("download.tab.bbox"))
                    .clicked()
                {
                    state.tab = DownloadTab::Bbox;
                }
                if ui
                    .selectable_label(state.tab == DownloadTab::Overpass, t("download.tab.overpass"))
                    .clicked()
                {
                    state.tab = DownloadTab::Overpass;
                    if state.overpass_query.trim().is_empty() {
                        state.overpass_query = state.build_overpass_query();
                    }
                }
            });
            ui.separator();

            match state.tab {
                DownloadTab::Bbox => bbox_tab(ui, actions, state),
                DownloadTab::Overpass => overpass_tab(ui, actions, state),
            }

            ui.add_space(4.0);
            if !status.is_empty() {
                ui.label(
                    RichText::new(status)
                        .size(11.5)
                        .color(Color32::from_rgb(190, 220, 180)),
                );
            }
        });
}

fn select_bbox_button(ui: &mut egui::Ui, selecting: bool) -> egui::Response {
    ui.add(egui::Button::new(format!("{} {}", ph::CURSOR_CLICK, t("download.select"))).selected(selecting))
}

fn bbox_tab(ui: &mut egui::Ui, actions: &mut UiActions, state: &mut DownloadUiState) {
    if let Some(bbox) = state.bbox {
        ui.monospace(bbox.text());
    } else {
        ui.label(
            RichText::new(t("download.bbox_hint"))
                .size(12.0)
                .color(Color32::from_rgb(150, 160, 174)),
        );
    }
    ui.horizontal(|ui| {
        if select_bbox_button(ui, state.selecting).clicked() {
            actions.begin_bbox = true;
        }
        if ui
            .add_enabled(
                state.bbox.is_some() && !state.selecting,
                egui::Button::new(format!("{} {}", ph::CLOUD_ARROW_DOWN, t("download.download"))),
            )
            .clicked()
        {
            actions.download_bbox = true;
        }
        if state.bbox.is_some() && ui.button(format!("{} {}", ph::X, t("download.clear"))).clicked() {
            actions.clear_bbox = true;
        }
    });
}

fn overpass_tab(ui: &mut egui::Ui, actions: &mut UiActions, state: &mut DownloadUiState) {
    ui.label(t("download.overpass.endpoint"));
    ui.add(
        egui::TextEdit::singleline(&mut state.overpass_endpoint)
            .desired_width(480.0)
            .hint_text("https://overpass-api.de/api/interpreter"),
    );
    ui.add_space(4.0);

    ui.label(t("download.overpass.helper"));
    ui.horizontal(|ui| {
        ui.label(t("download.overpass.key"));
        ui.add(
            egui::TextEdit::singleline(&mut state.overpass_key)
                .desired_width(120.0)
                .hint_text("amenity"),
        );
        ui.label(t("download.overpass.value"));
        ui.add(
            egui::TextEdit::singleline(&mut state.overpass_value)
                .desired_width(120.0)
                .hint_text("cafe"),
        );
    });
    ui.horizontal(|ui| {
        ui.checkbox(&mut state.overpass_nodes, t("download.overpass.nodes"));
        ui.checkbox(&mut state.overpass_ways, t("download.overpass.ways"));
        ui.checkbox(&mut state.overpass_relations, t("download.overpass.relations"));
    });
    ui.horizontal(|ui| {
        if select_bbox_button(ui, state.selecting).clicked() {
            actions.begin_bbox = true;
        }
        if ui
            .button(format!("{} {}", ph::MAGIC_WAND, t("download.overpass.build")))
            .clicked()
        {
            state.overpass_query = state.build_overpass_query();
        }
        if let Some(bbox) = state.bbox {
            ui.label(
                RichText::new(bbox.text())
                    .size(11.0)
                    .monospace()
                    .color(Color32::from_rgb(150, 160, 174)),
            );
        }
    });

    ui.add_space(4.0);
    ui.label(t("download.overpass.query"));
    ui.add(
        egui::TextEdit::multiline(&mut state.overpass_query)
            .desired_width(480.0)
            .desired_rows(10)
            .code_editor()
            .font(egui::TextStyle::Monospace),
    );

    ui.horizontal(|ui| {
        let can_download = !state.overpass_query.trim().is_empty()
            && !state.overpass_endpoint.trim().is_empty()
            && !state.selecting;
        if ui
            .add_enabled(
                can_download,
                egui::Button::new(format!("{} {}", ph::CLOUD_ARROW_DOWN, t("download.download"))),
            )
            .clicked()
        {
            actions.download_overpass = true;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_query_with_tag_and_bbox() {
        let q = build_overpass_query(
            Some(BboxView {
                min_lon: 116.0,
                min_lat: 39.0,
                max_lon: 117.0,
                max_lat: 40.0,
            }),
            "amenity",
            "cafe",
            true,
            true,
            false,
        );
        assert!(q.contains("[out:xml]"));
        assert!(q.contains(r#"node["amenity"="cafe"](39"#));
        assert!(q.contains(r#"way["amenity"="cafe"](39"#));
        assert!(!q.contains("relation["));
        assert!(q.contains("out meta;"));
    }

    #[test]
    fn build_query_key_only_uses_placeholder_without_bbox() {
        let q = build_overpass_query(None, "highway", "", true, false, false);
        assert!(q.contains(r#"node["highway"]({{bbox}})"#));
    }
}
