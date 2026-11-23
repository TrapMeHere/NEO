//! notes 面板。

use egui::{Color32, RichText};
use egui_phosphor::regular as ph;
use neo_i18n::t;

use super::UiActions;

/// 一条 note 评论（UI 视图）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NoteCommentView {
    /// 时间。
    pub date: String,
    /// 用户。
    pub user: String,
    /// 文本。
    pub text: String,
}

/// 一条 note（UI 视图）。
#[derive(Clone, Debug, PartialEq)]
pub struct NoteView {
    /// id。
    pub id: i64,
    /// 纬度。
    pub lat: f64,
    /// 经度。
    pub lon: f64,
    /// `open`/`closed`。
    pub status: String,
    /// 评论。
    pub comments: Vec<NoteCommentView>,
}

/// Notes 窗口状态。
#[derive(Clone, Debug, Default)]
pub struct NotesUiState {
    /// 当前 bbox 内的 notes。
    pub notes: Vec<NoteView>,
    /// 选中的 note id。
    pub selected: Option<i64>,
    /// 输入框（新建/评论/关闭共用）。
    pub text: String,
    /// 状态文本。
    pub status: String,
    /// 后台请求进行中。
    pub busy: bool,
}

/// Notes 窗口。
pub fn notes_window(ctx: &egui::Context, actions: &mut UiActions, open: &mut bool, state: &mut NotesUiState) {
    if !*open {
        return;
    }
    egui::Window::new(format!("{} {}", ph::CHAT_CIRCLE, t("notes.title")))
        .open(open)
        .resizable(true)
        .default_size([460.0, 420.0])
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(
                        !state.busy,
                        egui::Button::new(format!("{} {}", ph::ARROWS_CLOCKWISE, t("notes.refresh"))),
                    )
                    .clicked()
                {
                    actions.notes_refresh = true;
                }
                if ui
                    .add_enabled(
                        !state.busy && !state.text.trim().is_empty(),
                        egui::Button::new(format!("{} {}", ph::PLUS, t("notes.new"))),
                    )
                    .clicked()
                {
                    actions.note_create = true;
                }
                if state.busy {
                    ui.spinner();
                }
            });
            ui.separator();
            ui.horizontal_top(|ui| {
                ui.vertical(|ui| {
                    ui.set_min_width(180.0);
                    egui::ScrollArea::vertical()
                        .id_salt("neo-notes-list")
                        .max_height(300.0)
                        .show(ui, |ui| {
                            for note in &state.notes {
                                let first = note
                                    .comments
                                    .first()
                                    .map(|comment| comment.text.as_str())
                                    .unwrap_or("");
                                let label = format!(
                                    "{} {}  {}{}",
                                    if note.status == "open" {
                                        ph::CHAT_CIRCLE
                                    } else {
                                        ph::CHECK
                                    },
                                    note.id,
                                    first.chars().take(34).collect::<String>(),
                                    if first.chars().count() > 34 { "…" } else { "" }
                                );
                                if ui
                                    .selectable_label(state.selected == Some(note.id), label)
                                    .clicked()
                                {
                                    state.selected = Some(note.id);
                                    actions.note_focus = Some((note.lat, note.lon));
                                }
                            }
                        });
                });
                ui.separator();
                ui.vertical(|ui| {
                    ui.set_min_width(230.0);
                    if let Some(note) = state
                        .selected
                        .and_then(|id| state.notes.iter().find(|note| note.id == id))
                        .cloned()
                    {
                        ui.label(format!("{} #{}  {}", ph::CHAT_CIRCLE, note.id, note.status));
                        egui::ScrollArea::vertical()
                            .id_salt("neo-note-comments")
                            .max_height(190.0)
                            .show(ui, |ui| {
                                for comment in &note.comments {
                                    ui.label(
                                        RichText::new(format!("{} · {}", comment.user, comment.date))
                                            .size(10.5)
                                            .color(Color32::from_rgb(150, 160, 174)),
                                    );
                                    ui.label(RichText::new(&comment.text).size(12.0));
                                    ui.add_space(4.0);
                                }
                            });
                        ui.add(
                            egui::TextEdit::multiline(&mut state.text)
                                .desired_rows(3)
                                .desired_width(220.0)
                                .hint_text(t("notes.text")),
                        );
                        ui.horizontal(|ui| {
                            if ui
                                .add_enabled(
                                    !state.busy && !state.text.trim().is_empty(),
                                    egui::Button::new(t("notes.comment")),
                                )
                                .clicked()
                            {
                                actions.note_comment = Some(note.id);
                            }
                            if ui
                                .add_enabled(
                                    !state.busy && note.status == "open",
                                    egui::Button::new(t("notes.close")),
                                )
                                .clicked()
                            {
                                actions.note_close = Some(note.id);
                            }
                        });
                    } else {
                        ui.label(
                            RichText::new(t("notes.empty"))
                                .size(11.5)
                                .color(Color32::from_rgb(150, 160, 174)),
                        );
                        ui.add(
                            egui::TextEdit::multiline(&mut state.text)
                                .desired_rows(3)
                                .desired_width(220.0)
                                .hint_text(t("notes.text")),
                        );
                    }
                });
            });
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
