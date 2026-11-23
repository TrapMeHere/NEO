//! chrome 面板。

use egui::{Color32, FontId, RichText, Stroke, Ui};
use egui_phosphor::regular as ph;
use neo_i18n::t;

use super::{UiActions, UiMode, UiState};

/// 深色现代主题（JOSM 蓝作强调色）。
pub fn apply_modern_style(ctx: &egui::Context) {
    let mut visuals = egui::Visuals::dark();
    let accent = Color32::from_rgb(72, 150, 240);
    let panel = Color32::from_rgb(34, 38, 44);
    visuals.panel_fill = panel;
    visuals.window_fill = Color32::from_rgb(38, 42, 49);
    visuals.extreme_bg_color = Color32::from_rgb(24, 27, 32);
    visuals.faint_bg_color = Color32::from_rgb(43, 48, 56);
    visuals.selection.bg_fill = accent.gamma_multiply(0.35);
    visuals.selection.stroke = Stroke::new(1.0, accent);
    visuals.widgets.noninteractive.bg_fill = panel;
    visuals.widgets.inactive.bg_fill = Color32::from_rgb(45, 50, 58);
    visuals.widgets.hovered.bg_fill = Color32::from_rgb(54, 61, 71);
    visuals.widgets.active.bg_fill = accent.gamma_multiply(0.55);
    visuals.widgets.inactive.fg_stroke = Stroke::new(1.0, Color32::from_rgb(205, 211, 219));
    visuals.widgets.hovered.fg_stroke = Stroke::new(1.0, Color32::from_rgb(235, 240, 246));
    visuals.hyperlink_color = accent;
    ctx.set_visuals_of(egui::Theme::Dark, visuals.clone());
    ctx.set_visuals_of(egui::Theme::Light, visuals);
    ctx.set_theme(egui::Theme::Dark);
    ctx.all_styles_mut(|style| {
        style.spacing.item_spacing = egui::vec2(8.0, 6.0);
        style.spacing.button_padding = egui::vec2(8.0, 4.0);
        style.spacing.interact_size.y = 26.0;
        style.spacing.menu_margin = egui::Margin::same(6);
        style
            .text_styles
            .insert(egui::TextStyle::Heading, FontId::proportional(15.0));
        style
            .text_styles
            .insert(egui::TextStyle::Body, FontId::proportional(13.5));
        style
            .text_styles
            .insert(egui::TextStyle::Button, FontId::proportional(13.5));
        style
            .text_styles
            .insert(egui::TextStyle::Monospace, FontId::monospace(12.5));
    });
}

// ---------------------------------------------------------------------------
// 菜单栏
// ---------------------------------------------------------------------------

/// 顶部菜单栏（文件/编辑/视图/影像/工具/帮助）。
pub fn menu_bar(ui: &mut Ui, actions: &mut UiActions, state: UiState<'_>, lat: f64, lon: f64) {
    egui::MenuBar::new().ui(ui, |ui| {
        ui.menu_button(t("menu.file"), |ui| {
            if ui
                .button(format!("{} {}", ph::FOLDER_OPEN, t("menu.file.open")))
                .clicked()
            {
                actions.open_file = true;
                ui.close();
            }
            if ui
                .button(format!("{} {}", ph::MAP_TRIFOLD, t("menu.file.open_gpx")))
                .clicked()
            {
                actions.open_gpx = true;
                ui.close();
            }
            if ui
                .button(format!("{} {}", ph::MAP_TRIFOLD, t("menu.file.save_gpx")))
                .clicked()
            {
                actions.save_gpx = true;
                ui.close();
            }
            ui.separator();
            if ui
                .button(format!("{} {}", ph::FLOPPY_DISK, t("menu.file.save_xml")))
                .clicked()
            {
                actions.save_xml = true;
                ui.close();
            }
            if ui
                .button(format!("{} {}", ph::CLOUD_ARROW_UP, t("menu.file.sync")))
                .clicked()
            {
                actions.toggle_sync = true;
                ui.close();
            }
            ui.separator();
            if ui
                .button(format!("{} {}", ph::EXPORT, t("menu.file.export")))
                .clicked()
            {
                actions.toggle_export = true;
                ui.close();
            }
            if ui
                .button(format!("{} {}", ph::FOLDER_SIMPLE, t("menu.file.open_session")))
                .clicked()
            {
                actions.open_session = true;
                ui.close();
            }
        });

        ui.menu_button(t("menu.edit"), |ui| {
            if ui
                .add_enabled(
                    state.can_undo,
                    egui::Button::new(format!("{} {}", ph::ARROW_COUNTER_CLOCKWISE, t("menu.edit.undo"))),
                )
                .clicked()
            {
                actions.undo = true;
                ui.close();
            }
            if ui
                .add_enabled(
                    state.can_redo,
                    egui::Button::new(format!("{} {}", ph::ARROW_CLOCKWISE, t("menu.edit.redo"))),
                )
                .clicked()
            {
                actions.redo = true;
                ui.close();
            }
            ui.separator();
            if ui
                .add_enabled(
                    state.selection_len > 0,
                    egui::Button::new(format!("{} {}", ph::COPY, t("menu.edit.copy"))),
                )
                .clicked()
            {
                actions.copy_selection = true;
                ui.close();
            }
            if ui
                .button(format!("{} {}", ph::CLIPBOARD, t("menu.edit.paste")))
                .clicked()
            {
                actions.paste = true;
                ui.close();
            }
            if ui
                .add_enabled(
                    state.selection_len > 0,
                    egui::Button::new(format!("{} {}", ph::COPY_SIMPLE, t("menu.edit.duplicate"))),
                )
                .clicked()
            {
                actions.duplicate = true;
                ui.close();
            }
            if ui
                .button(format!("{} {}", ph::MAGNIFYING_GLASS, t("menu.edit.find")))
                .clicked()
            {
                actions.toggle_search = true;
                ui.close();
            }
            ui.separator();
            if ui
                .button(format!("{} {}", ph::TRASH, t("menu.edit.delete")))
                .clicked()
            {
                actions.delete_selected = true;
                ui.close();
            }
            if ui
                .button(format!(
                    "{} {}",
                    ph::CLOCK_COUNTER_CLOCKWISE,
                    t("menu.edit.recover")
                ))
                .clicked()
            {
                actions.recover_journal = true;
                ui.close();
            }
        });

        ui.menu_button(t("menu.view"), |ui| {
            if ui
                .button(format!(
                    "{} {}",
                    ph::MAGNIFYING_GLASS_PLUS,
                    t("menu.view.zoom_in")
                ))
                .clicked()
            {
                actions.zoom_in = true;
                ui.close();
            }
            if ui
                .button(format!(
                    "{} {}",
                    ph::MAGNIFYING_GLASS_MINUS,
                    t("menu.view.zoom_out")
                ))
                .clicked()
            {
                actions.zoom_out = true;
                ui.close();
            }
            if ui
                .button(format!("{} {}", ph::ARROWS_OUT_SIMPLE, t("menu.view.reset")))
                .clicked()
            {
                actions.reset_view = true;
                ui.close();
            }
            ui.separator();
            if ui
                .button(format!("{} {}", ph::FUNNEL, t("menu.view.filter")))
                .clicked()
            {
                actions.toggle_filter = true;
                ui.close();
            }
            if ui
                .button(format!(
                    "{} {}",
                    ph::ARROW_COUNTER_CLOCKWISE,
                    t("menu.view.filter_clear")
                ))
                .clicked()
            {
                actions.clear_filter = true;
                ui.close();
            }
            ui.separator();
            if ui
                .button(format!("{} {}", ph::CHAT_CIRCLE, t("menu.view.notes")))
                .clicked()
            {
                actions.toggle_notes = true;
                ui.close();
            }
            if ui
                .button(format!(
                    "{} {}",
                    ph::CLOCK_COUNTER_CLOCKWISE,
                    t("menu.view.changeset")
                ))
                .clicked()
            {
                actions.toggle_changeset = true;
                ui.close();
            }
        });

        ui.menu_button(t("menu.imagery"), |ui| {
            if ui.button(t("imagery.layer.osm")).clicked() {
                actions.imagery_add_osm = true;
                ui.close();
            }
            for (id, label) in super::imagery::quick_imagery_choices(lat, lon) {
                if ui.button(label).clicked() {
                    actions.imagery_pick = Some(id);
                    ui.close();
                }
            }
            ui.separator();
            if ui.button(t("menu.imagery.more")).clicked() {
                actions.toggle_imagery = true;
                ui.close();
            }
        });

        ui.menu_button(t("menu.tools"), |ui| {
            for (mode, icon, label) in [
                (UiMode::DrawPoint, ph::MAP_PIN, t("tool.point")),
                (UiMode::DrawLine, ph::LINE_SEGMENT, t("tool.line")),
                (UiMode::DrawArea, ph::POLYGON, t("tool.area")),
                (UiMode::Measure, ph::RULER, t("tool.measure")),
            ] {
                let selected = state.mode == mode;
                if ui.selectable_label(selected, format!("{icon} {label}")).clicked() {
                    // 再点一次取消，回到选择。
                    actions.mode = Some(if selected { UiMode::Select } else { mode });
                    ui.close();
                }
            }
            if ui
                .selectable_label(
                    state.mode == UiMode::Select,
                    format!("{} {}", ph::CURSOR_CLICK, t("tool.select")),
                )
                .clicked()
            {
                actions.mode = Some(UiMode::Select);
                ui.close();
            }
            ui.separator();
            if ui
                .button(format!("{} {}", ph::CHECK, t("menu.tools.finish")))
                .clicked()
            {
                actions.finish_way = true;
                ui.close();
            }
            ui.separator();
            if ui
                .button(format!("{} {}", ph::SCISSORS, t("menu.tools.split")))
                .clicked()
            {
                actions.split_way = true;
                ui.close();
            }
            if ui
                .button(format!("{} {}", ph::ARROWS_LEFT_RIGHT, t("menu.tools.reverse")))
                .clicked()
            {
                actions.reverse_way = true;
                ui.close();
            }
            if ui
                .button(format!("{} {}", ph::ARROWS_MERGE, t("menu.tools.merge")))
                .clicked()
            {
                actions.merge_ways = true;
                ui.close();
            }
            if ui
                .button(format!("{} {}", ph::WAVE_SAWTOOTH, t("menu.tools.simplify")))
                .clicked()
            {
                actions.simplify_way = true;
                ui.close();
            }
            if ui
                .add_enabled(
                    state.selection_len >= 2,
                    egui::Button::new(format!("{} {}", ph::PLUG, t("menu.tools.merge_nodes"))),
                )
                .clicked()
            {
                actions.merge_nodes = true;
                ui.close();
            }
            ui.separator();
            if ui
                .add_enabled(
                    state.selection_len >= 2,
                    egui::Button::new(format!("{} {}", ph::ALIGN_LEFT, t("menu.tools.align"))),
                )
                .clicked()
            {
                actions.align_nodes = true;
                ui.close();
            }
            if ui
                .add_enabled(
                    state.selection_len >= 3,
                    egui::Button::new(format!(
                        "{} {}",
                        ph::ARROWS_HORIZONTAL,
                        t("menu.tools.distribute")
                    )),
                )
                .clicked()
            {
                actions.distribute_nodes = true;
                ui.close();
            }
            if ui
                .add_enabled(
                    state.selection_len >= 1,
                    egui::Button::new(format!("{} {}", ph::FLIP_HORIZONTAL, t("menu.tools.mirror_h"))),
                )
                .clicked()
            {
                actions.mirror_horizontal = true;
                ui.close();
            }
            if ui
                .add_enabled(
                    state.selection_len >= 1,
                    egui::Button::new(format!("{} {}", ph::FLIP_VERTICAL, t("menu.tools.mirror_v"))),
                )
                .clicked()
            {
                actions.mirror_vertical = true;
                ui.close();
            }
            ui.separator();
            if ui
                .add_enabled(
                    state.selection_len >= 1,
                    egui::Button::new(format!("{} {}", ph::SCISSORS, t("menu.tools.unglue"))),
                )
                .clicked()
            {
                actions.unglue = true;
                ui.close();
            }
            if ui
                .add_enabled(
                    state.selection_len >= 1,
                    egui::Button::new(format!("{} {}", ph::COPY_SIMPLE, t("menu.tools.parallel"))),
                )
                .clicked()
            {
                actions.parallel_way = true;
                ui.close();
            }
            if ui
                .button(format!("{} {}", ph::SQUARE, t("menu.tools.orthogonalize")))
                .clicked()
            {
                actions.orthogonalize = true;
                ui.close();
            }
            if ui
                .add_enabled(
                    state.selection_len >= 2,
                    egui::Button::new(format!("{} {}", ph::TREE_STRUCTURE, t("menu.tools.join_areas"))),
                )
                .clicked()
            {
                actions.join_areas = true;
                ui.close();
            }
            if ui
                .add_enabled(
                    state.selection_len >= 2,
                    egui::Button::new(format!("{} {}", ph::CURSOR_CLICK, t("menu.tools.move_onto"))),
                )
                .clicked()
            {
                actions.move_node_onto_way = true;
                ui.close();
            }
            if ui
                .button(format!("{} {}", ph::BUILDING, t("menu.tools.extrude")))
                .clicked()
            {
                actions.extrude = true;
                ui.close();
            }
            ui.separator();
            if ui
                .button(format!("{} {}", ph::MAP_TRIFOLD, t("menu.tools.ecosystem")))
                .clicked()
            {
                actions.toggle_ecosystem = true;
                ui.close();
            }
        });

        ui.menu_button(t("menu.help"), |ui| {
            if ui
                .button(format!("{} {}", ph::PLUG, t("menu.help.plugins")))
                .clicked()
            {
                actions.toggle_plugins = true;
                ui.close();
            }
            ui.separator();
            if ui
                .button(format!("{} {}", ph::GEAR, t("menu.help.settings")))
                .clicked()
            {
                actions.toggle_settings = true;
                ui.close();
            }
            ui.separator();
            if ui
                .button(format!("{} {}", ph::QUESTION, t("menu.help.about")))
                .clicked()
            {
                actions.toggle_help = true;
                ui.close();
            }
        });

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(
                RichText::new(t("app.tagline"))
                    .size(12.0)
                    .color(Color32::from_rgb(150, 160, 174)),
            );
        });
    });
}

// ---------------------------------------------------------------------------
// 顶部常用功能工具栏
// ---------------------------------------------------------------------------

fn icon_button(ui: &mut Ui, icon: &str, tip: &str, enabled: bool) -> bool {
    let button = egui::Button::new(RichText::new(icon).size(17.0)).min_size(egui::vec2(27.0, 24.0));
    ui.add_enabled(enabled, button).on_hover_text(tip).clicked()
}

fn tool_button(ui: &mut Ui, icon: &str, tip: &str, selected: bool) -> bool {
    ui.selectable_label(selected, RichText::new(icon).size(17.0))
        .on_hover_text(tip)
        .clicked()
}

/// 顶部工具栏：文件 / 编辑 / 视图 常用操作。
pub fn top_toolbar(ui: &mut Ui, actions: &mut UiActions, state: UiState<'_>) {
    ui.horizontal(|ui| {
        if icon_button(ui, ph::CLOUD_ARROW_DOWN, t("menu.file.download"), true) {
            actions.toggle_download = true;
        }
        if icon_button(
            ui,
            ph::CLOUD_ARROW_UP,
            t("menu.file.sync"),
            state.has_pending_changes,
        ) {
            actions.toggle_sync = true;
        }
        if icon_button(ui, ph::FOLDER_OPEN, t("toolbar.open"), true) {
            actions.open_file = true;
        }
        if icon_button(ui, ph::EXPORT, t("menu.file.export"), true) {
            actions.toggle_export = true;
        }
        if icon_button(ui, ph::FOLDER_SIMPLE, t("toolbar.open_session"), true) {
            actions.open_session = true;
        }
        ui.separator();
        if icon_button(ui, ph::ARROW_COUNTER_CLOCKWISE, t("toolbar.undo"), state.can_undo) {
            actions.undo = true;
        }
        if icon_button(ui, ph::ARROW_CLOCKWISE, t("toolbar.redo"), state.can_redo) {
            actions.redo = true;
        }
        if icon_button(ui, ph::TRASH, t("toolbar.delete"), true) {
            actions.delete_selected = true;
        }
        ui.separator();
        if icon_button(ui, ph::MAGNIFYING_GLASS_PLUS, t("toolbar.zoom_in"), true) {
            actions.zoom_in = true;
        }
        if icon_button(ui, ph::MAGNIFYING_GLASS_MINUS, t("toolbar.zoom_out"), true) {
            actions.zoom_out = true;
        }
        if icon_button(ui, ph::ARROWS_OUT_SIMPLE, t("toolbar.reset"), true) {
            actions.reset_view = true;
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if state.dirty {
                ui.label(
                    RichText::new(format!("{} {}", ph::WARNING, t("toolbar.dirty")))
                        .color(Color32::from_rgb(240, 190, 90)),
                );
            } else {
                ui.label(
                    RichText::new(format!("{} {}", ph::CHECK, t("toolbar.clean")))
                        .color(Color32::from_rgb(140, 200, 140)),
                );
            }
        });
    });
}

// ---------------------------------------------------------------------------
// 左侧竖排工具栏（JOSM toolbox）
// ---------------------------------------------------------------------------

/// 左侧竖排工具栏：创建点/线/面 + 测距 + 几何操作。
///
/// 点/线/面互斥且可取消选中；三者都未选中时即为选择工具。
pub fn tool_palette(ui: &mut Ui, actions: &mut UiActions, state: UiState<'_>) {
    ui.vertical_centered(|ui| {
        ui.add_space(4.0);
        for (mode, icon, tip) in [
            (UiMode::DrawPoint, ph::MAP_PIN, t("tool.point")),
            (UiMode::DrawLine, ph::LINE_SEGMENT, t("tool.line")),
            (UiMode::DrawArea, ph::POLYGON, t("tool.area")),
            (UiMode::Measure, ph::RULER, t("tool.measure")),
        ] {
            let selected = state.mode == mode;
            if tool_button(ui, icon, tip, selected) {
                actions.mode = Some(if selected { UiMode::Select } else { mode });
            }
            ui.add_space(2.0);
        }
        ui.separator();
        let can_finish = matches!(state.mode, UiMode::DrawLine | UiMode::DrawArea);
        if icon_button(ui, ph::CHECK, t("tool.finish"), can_finish) {
            actions.finish_way = true;
        }
        ui.separator();
        if icon_button(ui, ph::SCISSORS, t("tool.split"), true) {
            actions.split_way = true;
        }
        if icon_button(ui, ph::ARROWS_LEFT_RIGHT, t("tool.reverse"), true) {
            actions.reverse_way = true;
        }
        if icon_button(ui, ph::ARROWS_MERGE, t("tool.merge"), true) {
            actions.merge_ways = true;
        }
    });
}
