//! changeset 面板。

use egui::{Color32, RichText};
use egui_phosphor::regular as ph;
use neo_i18n::t;

use super::UiActions;

/// Changeset 查看器状态。
#[derive(Clone, Debug, Default)]
pub struct ChangesetUiState {
    /// 输入框。
    pub id: String,
    /// 结果。
    pub info: Option<ChangesetView>,
    /// 状态/错误。
    pub status: String,
}

/// Changeset 视图。
#[derive(Clone, Debug, PartialEq)]
pub struct ChangesetView {
    /// id。
    pub id: i64,
    /// 用户。
    pub user: String,
    /// 创建时间。
    pub created_at: String,
    /// 关闭时间。
    pub closed_at: String,
    /// 是否仍打开。
    pub open: bool,
    /// 评论。
    pub comment: String,
    /// 变更数。
    pub changes: String,
    /// 标签。
    pub tags: Vec<(String, String)>,
}

/// Changeset 查看窗口。
pub fn changeset_window(
    ctx: &egui::Context,
    actions: &mut UiActions,
    open: &mut bool,
    state: &mut ChangesetUiState,
) {
    if !*open {
        return;
    }
    egui::Window::new(format!(
        "{} {}",
        ph::CLOCK_COUNTER_CLOCKWISE,
        t("changeset.title")
    ))
    .open(open)
    .resizable(true)
    .default_width(460.0)
    .show(ctx, |ui| {
        ui.horizontal(|ui| {
            ui.label(t("changeset.id"));
            ui.add(
                egui::TextEdit::singleline(&mut state.id)
                    .desired_width(120.0)
                    .hint_text("123456"),
            );
            if ui.button(t("changeset.load")).clicked() {
                actions.changeset_load = state.id.trim().parse::<i64>().ok();
            }
        });
        if let Some(info) = &state.info {
            ui.separator();
            ui.label(RichText::new(format!("#{} {}", info.id, info.user)).strong());
            ui.label(format!(
                "{} → {}  ({})",
                info.created_at,
                info.closed_at,
                if info.open {
                    t("changeset.open")
                } else {
                    t("changeset.closed")
                }
            ));
            if !info.comment.is_empty() {
                ui.label(RichText::new(&info.comment).size(12.5));
            }
            ui.label(format!("{}: {}", t("changeset.changes"), info.changes));
            for (key, value) in &info.tags {
                ui.monospace(format!("{key}={value}"));
            }
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
