//! side 面板。

use egui::{Color32, FontId, RichText, Stroke, Ui};
use egui_phosphor::regular as ph;
use neo_core::{ObjectId, ObjectKind};
use neo_i18n::{t, tf};
use neo_store::{DataSet, Referrers};

use super::presets::preset_combo;
use super::{LayerRole, LayerView, RelationMemberEdit, TagsEditor, UiActions, UiMode, UiState};

// ---------------------------------------------------------------------------
// 右侧面板堆叠：图层 / 标签 / 选择 / 关系 / 校验 / 冲突
// ---------------------------------------------------------------------------

/// 一条校验问题（由应用层从 `neo-validate` 报告转换而来）。
#[derive(Clone, Debug)]
pub struct ValidationItem {
    /// 严重级别：2 = error，1 = warning，0 = info。
    pub severity: u8,
    /// 规则 id（`validate.rule.<id>`）。
    pub rule: &'static str,
    /// 语言无关的细节（ID/数量）。
    pub detail: String,
    /// 涉及对象。
    pub objects: Vec<(ObjectKind, ObjectId)>,
}

/// 校验结果视图。
#[derive(Clone, Debug, Default)]
pub struct ValidationView {
    /// 问题列表。
    pub items: Vec<ValidationItem>,
    /// 错误数。
    pub errors: usize,
    /// 警告数。
    pub warnings: usize,
    /// (nodes, ways, relations)
    pub checked: (usize, usize, usize),
}

fn validation_rule_text(rule: &str) -> String {
    let key: &'static str = match rule {
        "way_too_short" => "validate.rule.way_too_short",
        "way_duplicate_node" => "validate.rule.way_duplicate_node",
        "way_untagged" => "validate.rule.way_untagged",
        "building_not_closed" => "validate.rule.building_not_closed",
        "duplicate_way" => "validate.rule.duplicate_way",
        "relation_empty" => "validate.rule.relation_empty",
        "relation_no_members" => "validate.rule.relation_no_members",
        "untagged_node" => "validate.rule.untagged_node",
        "duplicate_node" => "validate.rule.duplicate_node",
        "suspicious_tag_combination" => "validate.rule.suspicious_tag_combination",
        "multipolygon_bad_role" => "validate.rule.multipolygon_bad_role",
        "self_intersecting_way" => "validate.rule.self_intersecting_way",
        "way_too_long" => "validate.rule.way_too_long",
        "closed_way_too_few_nodes" => "validate.rule.closed_way_too_few_nodes",
        "relation_type_missing" => "validate.rule.relation_type_missing",
        "duplicate_relation_member" => "validate.rule.duplicate_relation_member",
        "invalid_layer_tag" => "validate.rule.invalid_layer_tag",
        "area_not_closed" => "validate.rule.area_not_closed",
        "non_adjacent_duplicate_node" => "validate.rule.non_adjacent_duplicate_node",
        "oneway_invalid" => "validate.rule.oneway_invalid",
        "empty_name" => "validate.rule.empty_name",
        "layer_out_of_range" => "validate.rule.layer_out_of_range",
        "multipolygon_no_outer" => "validate.rule.multipolygon_no_outer",
        "multipolygon_outer_not_closed" => "validate.rule.multipolygon_outer_not_closed",
        _ => return rule.to_owned(),
    };
    t(key).to_owned()
}

/// 右侧面板（JOSM 风格堆叠）。
///
/// 空面板不显示占位文字：标签无内容时留空；选择/关系为空时也留空。
#[allow(clippy::too_many_arguments)]
pub fn right_side_panel(
    ui: &mut Ui,
    actions: &mut UiActions,
    data: &DataSet,
    referrers: &Referrers,
    selection: Option<(ObjectKind, ObjectId)>,
    selection_list: &[(ObjectKind, ObjectId)],
    history: &[&'static str],
    tags_editor: &mut TagsEditor,
    merge_available: bool,
    layers: &[LayerView],
    validation: Option<&ValidationView>,
) {
    egui::ScrollArea::vertical()
        .id_salt("neo-right-panel")
        .show(ui, |ui| {
            // ---- 图层：活动图层 + 停靠图层 ----
            section_header(ui, ph::STACK, t("panel.layers"), |ui| {
                if header_action(ui, ph::PLUS, t("layer.new")).clicked() {
                    actions.layer_new = true;
                }
            });
            for (index, layer) in layers.iter().enumerate() {
                if index > 0 {
                    ui.separator();
                }
                ui.horizontal(|ui| {
                    let eye = if layer.visible { ph::EYE } else { ph::EYE_SLASH };
                    if ui
                        .add(
                            egui::Button::new(RichText::new(eye).size(15.0))
                                .frame(false)
                                .min_size(egui::vec2(24.0, 22.0)),
                        )
                        .clicked()
                    {
                        actions.layer_set_visible = Some((index, !layer.visible));
                    }
                    let data_layer = layer.role == LayerRole::Data;
                    if data_layer {
                        let active_icon = if layer.active {
                            ph::CHECK_SQUARE
                        } else {
                            ph::SQUARE
                        };
                        if ui
                            .add(
                                egui::Button::new(RichText::new(active_icon).size(15.0))
                                    .frame(false)
                                    .min_size(egui::vec2(24.0, 22.0)),
                            )
                            .clicked()
                        {
                            actions.layer_activate = Some(index);
                        }
                    }
                    let name_color = if layer.active {
                        Color32::from_rgb(220, 226, 234)
                    } else {
                        Color32::from_rgb(120, 128, 140)
                    };
                    let title = if data_layer {
                        format!("[{}] {}", index + 1, layer.label)
                    } else {
                        format!("{} {}", ph::IMAGE, layer.label)
                    };
                    let response = ui.add(
                        egui::Label::new(RichText::new(title).strong().color(name_color))
                            .sense(egui::Sense::click()),
                    );
                    if data_layer && response.clicked() {
                        actions.layer_activate = Some(index);
                    }
                    if layer.active {
                        response.context_menu(|ui| {
                            if ui.button(t("layer.context.info")).clicked() {
                                actions.layer_info = true;
                                ui.close();
                            }
                            if ui.button(t("layer.context.zoom")).clicked() {
                                actions.reset_view = true;
                                ui.close();
                            }
                            if ui.button(t("layer.context.save_as")).clicked() {
                                actions.save_xml = true;
                                ui.close();
                            }
                        });
                    }
                    if ui
                        .add(egui::Button::new(RichText::new(ph::TRASH).size(12.0)).frame(false))
                        .clicked()
                    {
                        actions.layer_remove = Some(index);
                    }
                    if data_layer
                        && index > 0
                        && ui
                            .add(egui::Button::new(RichText::new(ph::ARROWS_MERGE).size(12.0)).frame(false))
                            .on_hover_text(t("layer.merge"))
                            .clicked()
                    {
                        actions.layer_import = Some(index);
                    }
                });
            }

            // ---- 标签 / 成员（各自独立 ScrollArea，避免滚轮绑在一起）----
            section_header(ui, ph::TAG, t("panel.tags"), |_| {});
            if let Some((kind, id)) = selection {
                egui::Frame::group(ui.style()).show(ui, |ui| {
                    sync_tags_editor(data, kind, id, tags_editor);
                    preset_combo(ui, tags_editor, data, kind, id);
                    if let Some(tags) = tags_editor.show(ui) {
                        actions.apply_tags = Some(tags);
                    }
                    if merge_available
                        && ui
                            .button(format!("{} {}", ph::ARROWS_MERGE, t("tags.merge_ways")))
                            .clicked()
                    {
                        actions.merge_ways = true;
                    }
                });
                membership_block(ui, actions, data, referrers, kind, id);
            }

            // ---- 选择：为空则留空 ----
            section_header(ui, ph::SELECTION, t("panel.selection"), |_| {});
            if !selection_list.is_empty() {
                egui::Frame::group(ui.style()).show(ui, |ui| {
                    for (kind, id) in selection_list.iter().take(20) {
                        let icon = match kind {
                            ObjectKind::Node => ph::CIRCLE,
                            ObjectKind::Way => ph::POLYGON,
                            ObjectKind::Relation => ph::TREE_STRUCTURE,
                            ObjectKind::Changeset => ph::CLOCK,
                        };
                        ui.monospace(format!("{icon} {kind} {id}"));
                    }
                    if selection_list.len() > 20 {
                        ui.monospace(tf(
                            "selection.more",
                            &[("n", &(selection_list.len() - 20).to_string())],
                        ));
                    }
                });
            }

            // ---- 关系：选中关系时编辑它自己的成员，下面列出全部关系 ----
            section_header(ui, ph::TREE_STRUCTURE, t("panel.relations"), |_| {});
            if let Some(relation) = relation_being_edited(selection, selection_list) {
                relation_members_editor(ui, actions, data, relation, selection_list, tags_editor);
            }
            let relations: Vec<_> = data.iter_relations().collect();
            if !relations.is_empty() {
                egui::ScrollArea::vertical().max_height(180.0).show(ui, |ui| {
                    for relation in relations.iter().take(500) {
                        let name = relation_name(data, relation.handle);
                        let text = match name {
                            Some(name) if !name.is_empty() => {
                                format!("{} {}  ({name})", ph::TREE_STRUCTURE, relation.id)
                            }
                            _ => format!("{} {}", ph::TREE_STRUCTURE, relation.id),
                        };
                        if ui
                            .selectable_label(false, RichText::new(text).size(12.0).monospace())
                            .clicked()
                        {
                            let additive =
                                ui.input(|i| i.modifiers.shift || i.modifiers.command || i.modifiers.ctrl);
                            let mut objects = if additive {
                                selection_list.to_vec()
                            } else {
                                Vec::new()
                            };
                            if !objects.contains(&(ObjectKind::Relation, relation.id)) {
                                objects.push((ObjectKind::Relation, relation.id));
                            }
                            actions.select_objects = Some(objects);
                        }
                    }
                });
            }

            // ---- 命令历史：点击某步等价于连续撤销到该步之前 ----
            section_header(ui, ph::CLOCK_COUNTER_CLOCKWISE, t("panel.history"), |_| {});
            if history.is_empty() {
                ui.label(RichText::new(t("history.empty")).size(11.0));
            } else {
                egui::ScrollArea::vertical()
                    .id_salt("neo-command-history")
                    .max_height(140.0)
                    .show(ui, |ui| {
                        for (index, label) in history.iter().take(30).enumerate() {
                            let text = format!("{}. {label}", index + 1);
                            if ui
                                .selectable_label(false, RichText::new(text).size(11.5))
                                .clicked()
                            {
                                actions.undo_to = Some(index + 1);
                            }
                        }
                    });
            }

            // ---- 校验结果 ----
            section_header(ui, ph::CHECK_SQUARE, t("panel.validation"), |ui| {
                if header_action(ui, ph::PLAY, t("validate.run")).clicked() {
                    actions.run_validation = true;
                }
            });
            if let Some(view) = validation {
                if view.items.is_empty() {
                    ui.label(
                        RichText::new(t("validate.clean"))
                            .size(11.5)
                            .color(Color32::from_rgb(140, 200, 140)),
                    );
                } else {
                    ui.label(
                        RichText::new(tf(
                            "validate.summary",
                            &[
                                ("errors", &view.errors.to_string()),
                                ("warnings", &view.warnings.to_string()),
                                ("nodes", &view.checked.0.to_string()),
                                ("ways", &view.checked.1.to_string()),
                                ("relations", &view.checked.2.to_string()),
                            ],
                        ))
                        .size(11.0)
                        .color(Color32::from_rgb(150, 160, 174)),
                    );
                    egui::ScrollArea::vertical().max_height(160.0).show(ui, |ui| {
                        for item in view.items.iter().take(120) {
                            let color = if item.severity >= 2 {
                                Color32::from_rgb(235, 110, 110)
                            } else {
                                Color32::from_rgb(235, 190, 100)
                            };
                            let icon = if item.severity >= 2 {
                                ph::X_CIRCLE
                            } else {
                                ph::WARNING
                            };
                            let message = validation_rule_text(item.rule);
                            let text = if item.detail.is_empty() {
                                message
                            } else {
                                format!("{message}  ({})", item.detail)
                            };
                            let response = ui.add(
                                egui::Label::new(
                                    RichText::new(format!("{icon} {text}")).size(11.5).color(color),
                                )
                                .sense(egui::Sense::click()),
                            );
                            if response.clicked() {
                                actions.select_objects = Some(item.objects.clone());
                            }
                        }
                    });
                }
            }

            section_header(ui, ph::WARNING, t("panel.conflict"), |_| {});
        });
}

/// 把选中对象的标签同步到标签编辑器（目标变化时重载）。
fn sync_tags_editor(data: &DataSet, kind: ObjectKind, id: ObjectId, editor: &mut TagsEditor) {
    let pairs: &[(neo_core::Sym, neo_core::Sym)] = match kind {
        ObjectKind::Node => data
            .node_handle(id)
            .and_then(|handle| data.node_view(handle))
            .map(|view| view.tag_pairs)
            .unwrap_or(&[]),
        ObjectKind::Way => data
            .way_handle(id)
            .and_then(|handle| data.way_view(handle))
            .map(|view| view.tag_pairs)
            .unwrap_or(&[]),
        ObjectKind::Relation => data
            .relation_handle(id)
            .and_then(|handle| data.relation_view(handle))
            .map(|view| view.tag_pairs)
            .unwrap_or(&[]),
        ObjectKind::Changeset => &[],
    };
    let tags: Vec<(String, String)> = pairs
        .iter()
        .map(|(key, value)| (data.resolve(*key).to_owned(), data.resolve(*value).to_owned()))
        .collect();
    editor.sync(kind, id, &tags);
}

/// 标签栏里的成员：该要素属于哪些关系，以及它在其中的角色。
fn membership_block(
    ui: &mut Ui,
    actions: &mut UiActions,
    data: &DataSet,
    referrers: &Referrers,
    kind: ObjectKind,
    id: ObjectId,
) {
    let parents = parent_relations(data, referrers, kind, id);
    if parents.is_empty() {
        return;
    }
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.label(RichText::new(t("members.of")).size(11.0).strong());
        egui::ScrollArea::vertical()
            .id_salt("neo-memberships")
            .max_height(140.0)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                for (relation, role, member_index) in parents {
                    let name = data
                        .relation_handle(relation)
                        .and_then(|handle| relation_name(data, handle))
                        .unwrap_or_default();
                    let text = match (name.is_empty(), role.is_empty()) {
                        (true, true) => format!("{} {relation}", ph::TREE_STRUCTURE),
                        (true, false) => format!("{} {relation}  [{role}]", ph::TREE_STRUCTURE),
                        (false, true) => format!("{} {relation}  {name}", ph::TREE_STRUCTURE),
                        (false, false) => format!("{} {relation}  {name}  [{role}]", ph::TREE_STRUCTURE),
                    };
                    ui.horizontal(|ui| {
                        if ui
                            .selectable_label(false, RichText::new(text).size(12.0).monospace())
                            .clicked()
                        {
                            actions.select_objects = Some(vec![(ObjectKind::Relation, relation)]);
                        }
                        if ui
                            .add(egui::Button::new(RichText::new(ph::X).size(12.0)).frame(false))
                            .on_hover_text(t("members.remove_from_relation"))
                            .clicked()
                        {
                            actions.relation_member_edit = Some(RelationMemberEdit::Remove {
                                relation,
                                index: member_index,
                            });
                        }
                    });
                }
            });
    });
}

fn parent_relations(
    data: &DataSet,
    referrers: &Referrers,
    kind: ObjectKind,
    id: ObjectId,
) -> Vec<(ObjectId, String, usize)> {
    let Some(handles) = (match kind {
        ObjectKind::Node => data
            .node_handle(id)
            .map(|handle| referrers.relations_of_node(handle)),
        ObjectKind::Way => data
            .way_handle(id)
            .map(|handle| referrers.relations_of_way(handle)),
        ObjectKind::Relation => data
            .relation_handle(id)
            .map(|handle| referrers.relations_of_relation(handle)),
        ObjectKind::Changeset => None,
    }) else {
        return Vec::new();
    };
    let mut rows = Vec::new();
    for handle in handles {
        let Some(relation) = data.relations.id(*handle) else {
            continue;
        };
        let Some(members) = data.relations.members_of(*handle) else {
            continue;
        };
        for (index, member) in members.iter().enumerate() {
            if !member_is(data, member, kind, id) {
                continue;
            }
            rows.push((relation, data.resolve(member.role).to_owned(), index));
        }
    }
    rows
}

fn member_is(data: &DataSet, member: &neo_store::StoredMember, kind: ObjectKind, id: ObjectId) -> bool {
    if member.target.kind != kind {
        return false;
    }
    match kind {
        ObjectKind::Node => member.target.as_node().and_then(|handle| data.nodes.id(handle)) == Some(id),
        ObjectKind::Way => member.target.as_way().and_then(|handle| data.ways.id(handle)) == Some(id),
        ObjectKind::Relation => {
            member
                .target
                .as_relation()
                .and_then(|handle| data.relations.id(handle))
                == Some(id)
        }
        ObjectKind::Changeset => false,
    }
}

/// 当前要编辑其自身成员的关系：主选择优先，否则取选择列表里的关系。
fn relation_being_edited(
    selection: Option<(ObjectKind, ObjectId)>,
    selection_list: &[(ObjectKind, ObjectId)],
) -> Option<ObjectId> {
    if let Some((ObjectKind::Relation, id)) = selection {
        return Some(id);
    }
    selection_list
        .iter()
        .find(|(kind, _)| *kind == ObjectKind::Relation)
        .map(|(_, id)| *id)
}

/// 关系自己的成员（增删、排序、改角色）。不放在标签栏。
fn relation_members_editor(
    ui: &mut Ui,
    actions: &mut UiActions,
    data: &DataSet,
    id: ObjectId,
    selection_list: &[(ObjectKind, ObjectId)],
    tags_editor: &mut TagsEditor,
) {
    let Some(handle) = data.relation_handle(id) else {
        return;
    };
    let Some(members) = data.relations.members_of(handle) else {
        return;
    };
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(RichText::new(t("members.relation")).size(11.0).strong());
            ui.label(t("members.role"));
            ui.add(egui::TextEdit::singleline(&mut tags_editor.relation_role).desired_width(70.0));
            if ui
                .add_enabled(
                    selection_list
                        .iter()
                        .any(|(kind, object)| !(*kind == ObjectKind::Relation && *object == id)),
                    egui::Button::new(format!("{} {}", ph::PLUS, t("members.add"))),
                )
                .clicked()
            {
                actions.relation_member_edit = Some(RelationMemberEdit::Add { relation: id });
            }
        });
        egui::ScrollArea::vertical()
            .id_salt("neo-members-relation")
            .max_height(180.0)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                for (index, member) in members.iter().take(500).enumerate() {
                    let (member_kind, member_id) = match member.target.kind {
                        ObjectKind::Node => {
                            (ph::CIRCLE, member.target.as_node().and_then(|h| data.nodes.id(h)))
                        }
                        ObjectKind::Way => {
                            (ph::POLYGON, member.target.as_way().and_then(|h| data.ways.id(h)))
                        }
                        ObjectKind::Relation => (
                            ph::TREE_STRUCTURE,
                            member.target.as_relation().and_then(|h| data.relations.id(h)),
                        ),
                        ObjectKind::Changeset => (ph::CLOCK, None),
                    };
                    let role = data.resolve(member.role);
                    let id_text = member_id
                        .map(|id| id.to_string())
                        .unwrap_or_else(|| "?".to_owned());
                    ui.horizontal(|ui| {
                        ui.monospace(format!("{member_kind} {id_text}"));
                        if let Some((_, edit_index, buffer)) = &mut tags_editor.member_role_edit
                            && *edit_index == index
                        {
                            let response = ui.add(egui::TextEdit::singleline(buffer).desired_width(70.0));
                            let commit = response.lost_focus();
                            if commit {
                                let role = buffer.clone();
                                tags_editor.member_role_edit = None;
                                actions.relation_member_edit = Some(RelationMemberEdit::SetRole {
                                    relation: id,
                                    index,
                                    role,
                                });
                            }
                            return;
                        }
                        ui.label(format!("[{role}]"));
                        if ui.small_button(ph::PENCIL_SIMPLE).clicked() {
                            tags_editor.member_role_edit = Some((id, index, role.to_owned()));
                        }
                        if ui.small_button(ph::ARROW_UP).clicked() && index > 0 {
                            actions.relation_member_edit =
                                Some(RelationMemberEdit::MoveUp { relation: id, index });
                        }
                        if ui.small_button(ph::ARROW_DOWN).clicked() && index + 1 < members.len() {
                            actions.relation_member_edit =
                                Some(RelationMemberEdit::MoveDown { relation: id, index });
                        }
                        if ui.small_button(ph::X).clicked() {
                            actions.relation_member_edit =
                                Some(RelationMemberEdit::Remove { relation: id, index });
                        }
                    });
                }
            });
    });
}

fn relation_name(data: &DataSet, handle: neo_core::Handle<neo_store::RelationSlot>) -> Option<String> {
    let view = data.relation_view(handle)?;
    view.tag_pairs
        .iter()
        .find(|(key, _)| data.resolve(*key) == "name")
        .map(|(_, value)| data.resolve(*value).to_owned())
}

fn section_header(ui: &mut Ui, icon: &str, title: &str, add_actions: impl FnOnce(&mut Ui)) {
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.label(RichText::new(format!("{icon}  {title}")).strong().size(13.0));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), add_actions);
    });
    ui.separator();
}

fn header_action(ui: &mut Ui, icon: &str, tip: &str) -> egui::Response {
    ui.add(
        egui::Button::new(RichText::new(icon).size(14.0))
            .frame(false)
            .min_size(egui::vec2(22.0, 20.0)),
    )
    .on_hover_text(tip)
}

// ---------------------------------------------------------------------------
// 底部状态栏：经纬度 / 比例尺 / 测距 / 缩放 / 撤销重做
// ---------------------------------------------------------------------------

/// 底部状态栏。
pub fn status_bar(ui: &mut Ui, state: UiState<'_>) {
    ui.horizontal(|ui| {
        let (lat, lon) = state.mouse_latlon.unwrap_or(state.center_latlon);
        ui.label(
            RichText::new(format!("{} {lat:.6}, {lon:.6}", ph::CROSSHAIR))
                .monospace()
                .size(11.5),
        );
        ui.separator();
        scale_bar(ui, state.meters_per_px);
        ui.separator();
        ui.label(
            RichText::new(format!("{} z{:.2}", ph::MAGNIFYING_GLASS, state.zoom))
                .monospace()
                .size(11.5),
        );
        ui.separator();
        match state.measure_distance_m {
            Some(meters) => {
                let text = if meters >= 1000.0 {
                    format!("{} {:.3} km", ph::RULER, meters / 1000.0)
                } else {
                    format!("{} {:.1} m", ph::RULER, meters)
                };
                ui.label(
                    RichText::new(text)
                        .monospace()
                        .size(11.5)
                        .color(Color32::from_rgb(255, 210, 120)),
                );
            }
            None => if state.mode == UiMode::Measure {},
        }
        ui.separator();
        ui.label(
            RichText::new(format!(
                "{} {} {}",
                ph::SELECTION,
                t("status.selected_label"),
                state.selection_len
            ))
            .monospace()
            .size(11.5),
        );
        ui.separator();
        ui.label(
            RichText::new(format!(
                "{} {} {} / {} {}",
                ph::ARROW_COUNTER_CLOCKWISE,
                t("menu.edit.undo"),
                state.undo_len,
                t("menu.edit.redo"),
                state.redo_len
            ))
            .monospace()
            .size(11.5),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if !state.status.is_empty() {
                let mut text: String = state.status.chars().take(64).collect();
                if state.status.chars().count() > 64 {
                    text.push('…');
                }
                ui.label(
                    RichText::new(text)
                        .size(11.5)
                        .color(Color32::from_rgb(180, 220, 180)),
                );
            }
        });
    });
}

/// 比例尺：按当前米/像素画一段 80px 左右的标尺并标注距离。
fn scale_bar(ui: &mut Ui, meters_per_px: f64) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(120.0, 20.0), egui::Sense::hover());
    let painter = ui.painter();
    let target_px = 90.0_f64;
    let raw = meters_per_px * target_px;
    // 取 1/2/5 × 10^n 的"漂亮"距离
    let exponent = raw.log10().floor();
    let base = 10f64.powf(exponent);
    let mut nice = base;
    for factor in [1.0, 2.0, 5.0, 10.0] {
        if base * factor >= raw {
            nice = base * factor;
            break;
        }
    }
    let length_px = (nice / meters_per_px).clamp(20.0, 118.0) as f32;
    let y = rect.bottom() - 6.0;
    let x0 = rect.left() + 2.0;
    let stroke = Stroke::new(1.5, Color32::from_rgb(200, 206, 214));
    painter.line_segment([egui::pos2(x0, y), egui::pos2(x0 + length_px, y)], stroke);
    painter.line_segment([egui::pos2(x0, y - 5.0), egui::pos2(x0, y + 1.0)], stroke);
    painter.line_segment(
        [
            egui::pos2(x0 + length_px, y - 5.0),
            egui::pos2(x0 + length_px, y + 1.0),
        ],
        stroke,
    );
    let label = if nice >= 1000.0 {
        format!("{:.0} km", nice / 1000.0)
    } else {
        format!("{nice:.0} m")
    };
    painter.text(
        egui::pos2(x0, y - 8.0),
        egui::Align2::LEFT_BOTTOM,
        label,
        FontId::monospace(10.0),
        Color32::from_rgb(200, 206, 214),
    );
}

// ---------------------------------------------------------------------------
// 图层信息窗口（右键图层 → 信息）
// ---------------------------------------------------------------------------

/// 图层信息窗口（JOSM：图层右键 → 信息）。
pub fn layer_info_window(
    ctx: &egui::Context,
    data: &DataSet,
    state: UiState<'_>,
    actions: &mut UiActions,
    open: &mut bool,
) {
    egui::Window::new(format!("{} {}", ph::INFO, t("layer_info.title")))
        .open(open)
        .collapsible(false)
        .resizable(false)
        .show(ctx, |ui| {
            egui::Grid::new("layer_info")
                .num_columns(2)
                .spacing([16.0, 4.0])
                .show(ui, |ui| {
                    for (key, value) in [
                        (t("layer_info.name"), state.label.to_owned()),
                        (t("layer_info.nodes"), data.node_count().to_string()),
                        (t("layer_info.ways"), data.way_count().to_string()),
                        (t("layer_info.relations"), data.relation_count().to_string()),
                        (t("layer_info.version"), format!("v{}", data.version())),
                        (t("layer_info.dirty_objects"), data.changes.len().to_string()),
                        (
                            t("layer_info.unsaved"),
                            if state.dirty {
                                t("layer_info.yes")
                            } else {
                                t("layer_info.no")
                            }
                            .to_owned(),
                        ),
                        (
                            t("layer_info.center"),
                            format!("{:.6}, {:.6}", state.center_latlon.0, state.center_latlon.1),
                        ),
                        (t("layer_info.zoom"), format!("z{:.2}", state.zoom)),
                        (t("layer_info.selected"), state.selection_len.to_string()),
                    ] {
                        ui.strong(key);
                        ui.monospace(value);
                        ui.end_row();
                    }
                });
            ui.separator();
            ui.horizontal(|ui| {
                if ui
                    .button(format!(
                        "{} {}",
                        ph::ARROWS_OUT_SIMPLE,
                        t("layer_info.zoom_to_layer")
                    ))
                    .clicked()
                {
                    actions.reset_view = true;
                    ui.close();
                }
            });
        });
}
