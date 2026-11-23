//! plugins 面板。

use egui::{Color32, RichText};
use egui_phosphor::regular as ph;
use neo_i18n::t;

use super::UiActions;

/// 已安装插件视图。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PluginView {
    /// 稳定 id。
    pub id: String,
    /// 名称。
    pub name: String,
    /// 版本。
    pub version: String,
    /// ABI。
    pub api: String,
    /// 能力摘要。
    pub capabilities: String,
    /// 是否已签名。
    pub signed: bool,
    /// 是否启用。
    pub enabled: bool,
    /// 包路径（只读展示）。
    pub path: String,
}

/// 插件管理窗口状态。
#[derive(Clone, Debug, Default)]
pub struct PluginUiState {
    /// 已安装插件。
    pub plugins: Vec<PluginView>,
    /// 选中的插件 id。
    pub selected: Option<String>,
    /// 状态文本。
    pub status: String,
    /// 允许加载未签名包（开发者模式）。
    pub developer_mode: bool,
}

/// 插件管理窗口。
pub fn plugins_window(
    ctx: &egui::Context,
    actions: &mut UiActions,
    open: &mut bool,
    state: &mut PluginUiState,
) {
    if !*open {
        return;
    }
    egui::Window::new(format!("{} {}", ph::PLUG, t("plugins.title")))
        .open(open)
        .resizable(true)
        .default_size([520.0, 420.0])
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                if ui
                    .button(format!("{} {}", ph::UPLOAD_SIMPLE, t("plugins.install")))
                    .clicked()
                {
                    actions.plugin_install = true;
                }
                if ui
                    .button(format!("{} {}", ph::ARROWS_CLOCKWISE, t("plugins.refresh")))
                    .clicked()
                {
                    actions.plugin_refresh = true;
                }
                ui.checkbox(&mut state.developer_mode, t("plugins.developer"));
            });
            ui.separator();
            egui::ScrollArea::vertical()
                .id_salt("neo-plugins-list")
                .max_height(260.0)
                .show(ui, |ui| {
                    for plugin in &state.plugins {
                        let label = format!(
                            "{} {} {}  v{}",
                            if plugin.enabled {
                                ph::CHECK_SQUARE
                            } else {
                                ph::SQUARE
                            },
                            plugin.name,
                            plugin.id,
                            plugin.version
                        );
                        ui.horizontal(|ui| {
                            if ui
                                .selectable_label(state.selected.as_deref() == Some(&plugin.id), label)
                                .clicked()
                            {
                                state.selected = Some(plugin.id.clone());
                            }
                            if ui
                                .add_enabled(!plugin.enabled, egui::Button::new(ph::PLAY).frame(false))
                                .clicked()
                            {
                                actions.plugin_toggle = Some((plugin.id.clone(), true));
                            }
                            if ui
                                .add_enabled(plugin.enabled, egui::Button::new(ph::PAUSE).frame(false))
                                .clicked()
                            {
                                actions.plugin_toggle = Some((plugin.id.clone(), false));
                            }
                            if ui.add(egui::Button::new(ph::TRASH).frame(false)).clicked() {
                                actions.plugin_remove = Some(plugin.id.clone());
                            }
                        });
                    }
                    if state.plugins.is_empty() {
                        ui.label(
                            RichText::new(t("plugins.empty"))
                                .size(11.5)
                                .color(Color32::from_rgb(150, 160, 174)),
                        );
                    }
                });
            if let Some(plugin) = state
                .selected
                .as_ref()
                .and_then(|id| state.plugins.iter().find(|plugin| &plugin.id == id))
            {
                ui.separator();
                ui.label(
                    RichText::new(format!(
                        "{} v{} · API {} · {}",
                        plugin.name,
                        plugin.version,
                        plugin.api,
                        if plugin.signed {
                            t("plugins.signed")
                        } else {
                            t("plugins.unsigned")
                        }
                    ))
                    .strong(),
                );
                ui.monospace(&plugin.path);
                ui.label(RichText::new(&plugin.capabilities).size(11.0));
            }
            if !state.status.is_empty() {
                ui.separator();
                ui.label(
                    RichText::new(&state.status)
                        .size(11.0)
                        .color(Color32::from_rgb(160, 200, 170)),
                );
            }
        });
}
