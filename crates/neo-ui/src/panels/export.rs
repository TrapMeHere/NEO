//! export 面板。

use egui_phosphor::regular as ph;
use neo_i18n::t;

use super::{ExportFormat, UiActions};

/// 导出窗口状态。
#[derive(Clone, Debug, Default)]
pub struct ExportUiState {
    /// 选择的格式。
    pub format: Option<ExportFormat>,
}

/// 统一导出窗口：用户选择格式。
pub fn export_window(
    ctx: &egui::Context,
    actions: &mut UiActions,
    open: &mut bool,
    state: &mut ExportUiState,
) {
    egui::Window::new(format!("{} {}", ph::EXPORT, t("export.title")))
        .open(open)
        .collapsible(false)
        .resizable(false)
        .default_width(360.0)
        .show(ctx, |ui| {
            let current = state.format.unwrap_or(ExportFormat::Xml);
            for (format, key) in [
                (ExportFormat::Xml, "export.xml"),
                (ExportFormat::Osc, "export.osc"),
                (ExportFormat::Pbf, "export.pbf"),
                (ExportFormat::Session, "export.session"),
            ] {
                if ui.selectable_label(current == format, t(key)).clicked() {
                    state.format = Some(format);
                }
            }
            ui.add_space(8.0);
            if ui
                .button(format!("{} {}", ph::EXPORT, t("export.action")))
                .clicked()
            {
                actions.export_now = Some(state.format.unwrap_or(ExportFormat::Xml));
            }
            ui.add_space(4.0);
        });
}
