//! filter 面板。

use egui_phosphor::regular as ph;
use neo_i18n::t;

use super::UiActions;

/// 过滤器状态。
#[derive(Clone, Debug)]
pub struct FilterUiState {
    /// 是否启用过滤（过滤开启时，不匹配的对象被隐藏）。
    pub active: bool,
    /// 查询串，语法同查找。
    pub query: String,
    /// 保留节点。
    pub nodes: bool,
    /// 保留 way。
    pub ways: bool,
    /// 保留 relation。
    pub relations: bool,
    /// 是否把当前选择加入隐藏集合。
    pub hide_selected: bool,
}

impl Default for FilterUiState {
    fn default() -> Self {
        Self {
            active: false,
            query: String::new(),
            nodes: true,
            ways: true,
            relations: true,
            hide_selected: false,
        }
    }
}

/// 过滤器窗口。
pub fn filter_window(
    ctx: &egui::Context,
    actions: &mut UiActions,
    open: &mut bool,
    state: &mut FilterUiState,
) {
    if !*open {
        return;
    }
    egui::Window::new(format!("{} {}", ph::FUNNEL, t("filter.title")))
        .open(open)
        .resizable(false)
        .default_width(420.0)
        .show(ctx, |ui| {
            ui.checkbox(&mut state.active, t("filter.active"));
            ui.add(egui::TextEdit::singleline(&mut state.query).desired_width(380.0));
            ui.horizontal(|ui| {
                ui.checkbox(&mut state.nodes, t("search.nodes"));
                ui.checkbox(&mut state.ways, t("search.ways"));
                ui.checkbox(&mut state.relations, t("search.relations"));
            });
            ui.horizontal(|ui| {
                if ui
                    .button(format!("{} {}", ph::CHECK, t("filter.apply")))
                    .clicked()
                {
                    actions.apply_filter = true;
                }
                if ui.button(format!("{} {}", ph::X, t("filter.clear"))).clicked() {
                    state.active = false;
                    state.query.clear();
                    actions.clear_filter = true;
                }
                if ui
                    .button(format!("{} {}", ph::EYE_SLASH, t("filter.hide_selected")))
                    .clicked()
                {
                    actions.hide_selected = true;
                }
            });
        });
}
