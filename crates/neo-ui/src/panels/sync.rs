//! Upload panel (formerly the "Sync" window): only responsible for uploading local changes to OSM.
//!
//! Download is a separate feature (the download window in `download.rs`); this panel no longer offers any bbox
//! download entry, avoiding the mixed semantics of "Sync" doing both download and upload. Conflict handling keeps
//! only the actions that resolve upload 409s.

use egui::{Color32, RichText};
use egui_phosphor::regular as ph;
use neo_core::{ObjectId, ObjectKind};
use neo_i18n::{t, tf};

use super::UiActions;

/// Sync conflict (from a 409).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SyncConflict {
    /// Object kind.
    pub kind: ObjectKind,
    /// Object id.
    pub id: ObjectId,
    /// Server version.
    pub version: Option<i32>,
}

/// Upload window.
#[allow(clippy::too_many_arguments)]
pub fn sync_window(
    ctx: &egui::Context,
    actions: &mut UiActions,
    open: &mut bool,
    account: Option<&str>,
    comment: &mut String,
    status: &str,
    busy: bool,
    has_pending_changes: bool,
    conflicts: &[SyncConflict],
) {
    egui::Window::new(format!("{} {}", ph::CLOUD_ARROW_UP, t("sync.title")))
        .open(open)
        .collapsible(false)
        .resizable(false)
        .default_width(520.0)
        .show(ctx, |ui| {
            if let Some(name) = account {
                ui.label(name);
            } else {
                ui.label(RichText::new(t("sync.no_account")).color(Color32::from_rgb(235, 170, 100)));
            }
            ui.add_space(4.0);
            ui.add(
                egui::TextEdit::singleline(comment)
                    .desired_width(480.0)
                    .hint_text(t("sync.comment")),
            );
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(
                        account.is_some() && !busy && has_pending_changes,
                        egui::Button::new(format!("{} {}", ph::CLOUD_ARROW_UP, t("sync.upload"))),
                    )
                    .clicked()
                {
                    actions.sync_upload = true;
                }
                if ui
                    .add_enabled(
                        account.is_some() && !busy && has_pending_changes,
                        egui::Button::new(format!("{} {}", ph::SELECTION, t("sync.upload_selected"))),
                    )
                    .clicked()
                {
                    actions.sync_upload_selected = true;
                }
            });
            if busy {
                ui.label(
                    RichText::new(t("sync.busy"))
                        .size(11.5)
                        .color(Color32::from_rgb(150, 190, 240)),
                );
            }
            if !status.is_empty() {
                ui.label(
                    RichText::new(status)
                        .size(11.5)
                        .color(Color32::from_rgb(190, 220, 180)),
                );
            }
            if !conflicts.is_empty() {
                ui.separator();
                ui.label(
                    RichText::new(tf("sync.conflicts", &[("n", &conflicts.len().to_string())]))
                        .color(Color32::from_rgb(235, 130, 110)),
                );
                egui::ScrollArea::vertical().max_height(120.0).show(ui, |ui| {
                    for conflict in conflicts.iter().take(100) {
                        let version = conflict.version.map(|v| format!(" v{v}")).unwrap_or_default();
                        ui.horizontal(|ui| {
                            ui.monospace(format!("{:?} {}{}", conflict.kind, conflict.id, version));
                            if ui.small_button(t("sync.take_remote")).clicked() {
                                actions.sync_take_remote = Some((conflict.kind, conflict.id));
                            }
                            if ui.small_button(t("sync.merge_remote")).clicked() {
                                actions.sync_merge_remote = Some((conflict.kind, conflict.id));
                            }
                            if ui.small_button(t("sync.merge_geometry")).clicked() {
                                actions.sync_merge_geometry = Some((conflict.kind, conflict.id));
                            }
                        });
                    }
                });
                ui.horizontal(|ui| {
                    if ui
                        .button(format!("{} {}", ph::CHECK, t("sync.keep_local")))
                        .clicked()
                    {
                        actions.sync_keep_local = true;
                    }
                });
            }
            ui.add_space(4.0);
        });
}
