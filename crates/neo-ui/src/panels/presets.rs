//! presets 面板。

use egui::{Color32, RichText, Ui};
use egui_phosphor::regular as ph;
use neo_core::{ObjectId, ObjectKind};
use neo_i18n::t;
use neo_store::DataSet;

use super::TagsEditor;

// ---------------------------------------------------------------------------
// 帮助窗口
// ---------------------------------------------------------------------------

/// 阶段 5 生态窗口：三个插件语义、影像 URL、预设缺口。
pub fn ecosystem_window(ctx: &egui::Context, open: &mut bool) {
    egui::Window::new(format!("{} {}", ph::MAP_TRIFOLD, t("ecosystem.title")))
        .open(open)
        .collapsible(false)
        .resizable(true)
        .default_width(420.0)
        .show(ctx, |ui| {
            ui.label(t("ecosystem.plugins"));
            ui.monospace("todo\nutilsplugin (add-node)\nbuildings_tools (building=yes)");
            ui.separator();
            ui.label(t("ecosystem.imagery"));
            let url = neo_style::tms_url("https://tile.openstreetmap.org/{z}/{x}/{y}.png", 12, 2100, 1400);
            ui.monospace(&url);
            ui.separator();
            let missing = neo_style::preset_coverage().missing();
            ui.label(t("ecosystem.missing"));
            if missing.is_empty() {
                ui.label(t("ecosystem.missing_none"));
            } else {
                for name in missing {
                    ui.label(format!("- {name}"));
                }
            }
        });
}

/// 快捷键/关于窗口。
pub fn help_window(ctx: &egui::Context, open: &mut bool) {
    egui::Window::new(format!("{} {}", ph::QUESTION, t("help.title")))
        .open(open)
        .collapsible(false)
        .resizable(false)
        .show(ctx, |ui| {
            ui.heading(t("app.window_title"));
            ui.label(t("app.subtitle"));
            ui.label(
                RichText::new(format!("v{}", env!("CARGO_PKG_VERSION")))
                    .size(11.5)
                    .color(Color32::from_rgb(150, 160, 174)),
            );
            ui.separator();
            ui.label(
                RichText::new(t("help.license"))
                    .size(11.0)
                    .color(Color32::from_rgb(150, 160, 174)),
            );
        });
}

// ---------------------------------------------------------------------------
// 标签编辑器
// ---------------------------------------------------------------------------

/// 预设菜单：一行下拉选分组；匹配预设时默认打开其分组并置顶。
///
/// 几何或搜索过滤后没有预设的分组不显示。
pub(super) fn preset_combo(
    ui: &mut Ui,
    tags_editor: &mut TagsEditor,
    data: &DataSet,
    kind: ObjectKind,
    id: ObjectId,
) {
    let geometry = preset_geometry_filter(data, kind, id);
    let concrete = tags_editor.concrete_tags();
    let matched = neo_style::best_preset_for_tags(&concrete, geometry);
    let zh = neo_i18n::current_language() == neo_i18n::Lang::ZhCn;
    let button_label = matched
        .map(|preset| {
            if zh {
                preset.name_zh.as_str()
            } else {
                preset.name_en.as_str()
            }
        })
        .unwrap_or_else(|| t("preset.label"));
    let mut chosen: Option<&'static neo_style::SchemaPreset> = None;
    let button = ui.add_sized(
        [220.0, ui.spacing().interact_size.y],
        egui::Button::new(button_label),
    );
    egui::Popup::menu(&button)
        .width(320.0)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut tags_editor.preset_search)
                    .desired_width(ui.available_width())
                    .hint_text(t("preset.search")),
            );

            let query = tags_editor.preset_search.to_lowercase();
            let categories = neo_style::preset_categories();
            let visible = visible_preset_groups(geometry, &query, zh, categories);
            let matched_group = matched.and_then(|preset| {
                neo_style::categories_for_preset(&preset.id)
                    .first()
                    .copied()
                    .map(str::to_owned)
            });
            if !tags_editor.preset_pinned
                && let Some(group) = matched_group.as_deref()
                && visible.iter().any(|(category, _)| category.id == group)
            {
                tags_editor.preset_category = group.to_owned();
            }
            if !tags_editor.preset_category.is_empty()
                && !visible
                    .iter()
                    .any(|(category, _)| category.id == tags_editor.preset_category)
            {
                tags_editor.preset_category.clear();
                tags_editor.preset_pinned = false;
                if let Some(group) = matched_group.as_deref()
                    && visible.iter().any(|(category, _)| category.id == group)
                {
                    tags_editor.preset_category = group.to_owned();
                }
            }

            let selected_name = if tags_editor.preset_category.is_empty() {
                t("preset.category_all").to_owned()
            } else {
                visible
                    .iter()
                    .find(|(category, _)| category.id == tags_editor.preset_category)
                    .map(|(category, _)| category_label(category, zh).to_owned())
                    .unwrap_or_else(|| tags_editor.preset_category.clone())
            };
            if ui
                .selectable_label(tags_editor.preset_picking, format!("{selected_name} ▾"))
                .clicked()
            {
                tags_editor.preset_picking = !tags_editor.preset_picking;
            }

            ui.separator();
            let category_id = tags_editor.preset_category.clone();
            if tags_editor.preset_picking {
                egui::ScrollArea::vertical()
                    .id_salt("neo-preset-cat-menu")
                    .max_height(280.0)
                    .show(ui, |ui| {
                        if ui
                            .selectable_label(category_id.is_empty(), t("preset.category_all"))
                            .clicked()
                        {
                            tags_editor.preset_category.clear();
                            tags_editor.preset_pinned = true;
                            tags_editor.preset_picking = false;
                        }
                        for (category, count) in &visible {
                            let name = category_label(category, zh);
                            let selected = category_id == category.id;
                            if ui
                                .selectable_label(selected, format!("{name} ({count})"))
                                .clicked()
                            {
                                tags_editor.preset_category = category.id.clone();
                                tags_editor.preset_pinned = true;
                                tags_editor.preset_picking = false;
                            }
                        }
                    });
                return;
            }
            let category_id = category_id.as_str();
            let matched_id = matched.map(|preset| preset.id.as_str());
            egui::ScrollArea::vertical()
                .id_salt("neo-preset-list")
                .max_height(280.0)
                .show(ui, |ui| {
                    let mut shown = 0usize;
                    let mut emit = |preset: &'static neo_style::SchemaPreset, pin: bool| {
                        if shown >= 80 {
                            return;
                        }
                        if !preset_list_hit(preset, geometry, category_id, &query, zh, categories) {
                            return;
                        }
                        let name = if zh {
                            preset.name_zh.as_str()
                        } else {
                            preset.name_en.as_str()
                        };
                        let label = if pin {
                            format!("★ {name}")
                        } else if preset.suggestion {
                            format!("{name} · {}", t("preset.suggestion"))
                        } else if category_id.is_empty() {
                            let group = neo_style::categories_for_preset(&preset.id)
                                .first()
                                .copied()
                                .unwrap_or("");
                            let group_name = categories
                                .iter()
                                .find(|category| category.id == group)
                                .map(|category| category_label(category, zh))
                                .unwrap_or(group);
                            if group_name.is_empty() {
                                name.to_owned()
                            } else {
                                format!("{name} · {group_name}")
                            }
                        } else {
                            name.to_owned()
                        };
                        if ui.selectable_label(pin, label).clicked() {
                            chosen = Some(preset);
                            ui.close();
                        }
                        shown += 1;
                    };

                    if let Some(preset) = matched {
                        emit(preset, true);
                    }
                    let schema = neo_style::schema_presets();
                    for preset in schema {
                        if matched_id == Some(preset.id.as_str()) {
                            continue;
                        }
                        emit(preset, false);
                    }
                    if !query.is_empty() {
                        for preset in neo_style::suggestion_presets() {
                            if matched_id == Some(preset.id.as_str()) {
                                continue;
                            }
                            emit(preset, false);
                        }
                    }
                    if shown == 0 {
                        ui.label(
                            RichText::new(t("preset.empty"))
                                .size(11.0)
                                .color(Color32::from_rgb(150, 160, 174)),
                        );
                    }
                });
        });
    if let Some(preset) = chosen {
        tags_editor.apply_preset_owned(&preset.tags, &preset.remove_keys);
    }
}

fn category_label(category: &neo_style::PresetCategory, zh: bool) -> &str {
    if zh {
        category.name_zh.as_str()
    } else {
        category.name_en.as_str()
    }
}

fn preset_list_hit(
    preset: &neo_style::SchemaPreset,
    geometry: neo_style::PresetGeometryFilter,
    category_id: &str,
    query: &str,
    zh: bool,
    categories: &[neo_style::PresetCategory],
) -> bool {
    if !geometry.allows(preset) || !neo_style::preset_in_category(&preset.id, category_id) {
        return false;
    }
    if query.is_empty() {
        return true;
    }
    let name = if zh {
        preset.name_zh.as_str()
    } else {
        preset.name_en.as_str()
    };
    if name.to_lowercase().contains(query) || preset.id.to_lowercase().contains(query) {
        return true;
    }
    let group = neo_style::categories_for_preset(&preset.id)
        .first()
        .copied()
        .unwrap_or("");
    if group.to_lowercase().contains(query) {
        return true;
    }
    categories
        .iter()
        .find(|category| category.id == group)
        .is_some_and(|category| category_label(category, zh).to_lowercase().contains(query))
}

fn visible_preset_groups<'a>(
    geometry: neo_style::PresetGeometryFilter,
    query: &str,
    zh: bool,
    categories: &'a [neo_style::PresetCategory],
) -> Vec<(&'a neo_style::PresetCategory, usize)> {
    let mut counts: Vec<usize> = vec![0; categories.len()];
    let schema = neo_style::schema_presets();
    let mut tally = |preset: &neo_style::SchemaPreset| {
        if !preset_list_hit(preset, geometry, "", query, zh, categories) {
            return;
        }
        let Some(group) = neo_style::categories_for_preset(&preset.id).first().copied() else {
            return;
        };
        if let Some(index) = categories.iter().position(|category| category.id == group) {
            counts[index] += 1;
        }
    };
    for preset in schema {
        tally(preset);
    }
    if !query.is_empty() {
        for preset in neo_style::suggestion_presets() {
            tally(preset);
        }
    }
    categories
        .iter()
        .zip(counts)
        .filter(|(_, count)| *count > 0)
        .collect()
}

fn preset_geometry_filter(data: &DataSet, kind: ObjectKind, id: ObjectId) -> neo_style::PresetGeometryFilter {
    match kind {
        ObjectKind::Node => neo_style::PresetGeometryFilter::Node,
        ObjectKind::Way => {
            let Some(view) = data.way_handle(id).and_then(|handle| data.way_view(handle)) else {
                return neo_style::PresetGeometryFilter::OpenWay;
            };
            if !view.is_closed() {
                return neo_style::PresetGeometryFilter::OpenWay;
            }
            // 闭合不等于面：有线预设可匹配时仍按线（如闭合 highway=service）。
            let tags: Vec<(String, String)> = view
                .tag_pairs
                .iter()
                .map(|(k, v)| (data.resolve(*k).to_owned(), data.resolve(*v).to_owned()))
                .collect();
            let tag_refs: Vec<(&str, &str)> = tags.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
            if neo_style::tags_imply_area(&tag_refs) {
                neo_style::PresetGeometryFilter::ClosedWay
            } else {
                neo_style::PresetGeometryFilter::OpenWay
            }
        }
        ObjectKind::Relation => neo_style::PresetGeometryFilter::Relation,
        ObjectKind::Changeset => neo_style::PresetGeometryFilter::Node,
    }
}
