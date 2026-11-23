//! NeoApp::canvas 方法。行为与拆分前的 app.rs 相同。

use crate::map_callback::MapCallback;
use eframe::egui;
use neo_core::{LatLon, ObjectId, ObjectKind};
use neo_geo::{HitTester, SnapHit, SnapTarget, WorldBounds, WorldPoint, resolve_snap};
use neo_i18n::{t, tf};
use neo_tools::Pick;
use neo_ui::UiMode;

use super::{
    DragState, MIDPOINT_HIT_RADIUS_PX, MIDPOINT_MIN_SEGMENT_PX, NODE_PICK_MARGIN_PX, NeoApp,
    SELECTION_DRAG_MARGIN_PX, WAY_PICK_MARGIN_PX, point_segment_distance,
};

impl NeoApp {
    /// 测试/演示用：选视口内第一个可见（可拾取）的节点
    pub(super) fn pick_visible_node(&self) -> Option<ObjectId> {
        let visible = self.camera.visible_bounds();
        self.editor
            .data()
            .iter_nodes()
            .find(|node| visible.contains(WorldPoint::from_latlon(node.pos)) && self.node_selectable(node.id))
            .map(|node| node.id)
    }

    /// 测试/演示用：选一条视口内节点最多的 way
    pub(super) fn pick_visible_way(&self) -> Option<ObjectId> {
        let visible = self.camera.visible_bounds();
        let mut best: Option<(usize, ObjectId)> = None;
        for way in self.editor.data().iter_ways() {
            let Some(nodes) = self.editor.data().ways.nodes(way.handle) else {
                continue;
            };
            let mut inside = false;
            let mut count = 0usize;
            for node in nodes {
                count += 1;
                if let Some(position) = self.editor.data().nodes.position(*node)
                    && visible.contains(WorldPoint::from_latlon(position))
                {
                    inside = true;
                }
            }
            if inside && count >= 2 && best.map(|(c, _)| count > c).unwrap_or(true) {
                best = Some((count, way.id));
            }
        }
        best.map(|(_, id)| id)
    }

    pub(super) fn pick_at(&self, screen: [f32; 2]) -> Pick {
        // 节点优先。内圈其次，这样点在面上选的是面，而不是旁边的线。
        // 最后才是线的外边距。隐藏节点不能截胡它下面的面或线。
        let node_hit = {
            let tester = HitTester::new(self.editor.data(), self.editor.index());
            tester.pick_node(&self.camera, screen, NODE_PICK_MARGIN_PX)
        };
        if let Some(hit) = node_hit
            && hit
                .node
                .and_then(|handle| self.editor.data().nodes.id(handle))
                .is_some_and(|id| self.node_selectable(id) && !self.hidden.contains(&(ObjectKind::Node, id)))
        {
            return Pick {
                node: hit.node.and_then(|handle| self.editor.data().nodes.id(handle)),
                way: None,
                relation: None,
            };
        }
        if let Some(pick) = self.pick_area_band(screen) {
            return pick;
        }
        let way_hit = {
            let tester = HitTester::new(self.editor.data(), self.editor.index());
            tester.pick_way(&self.camera, screen, WAY_PICK_MARGIN_PX)
        };
        let Some(hit) = way_hit else {
            return Pick::default();
        };
        let way = hit.way.and_then(|handle| self.editor.data().ways.id(handle));
        if way.is_some_and(|id| self.hidden.contains(&(ObjectKind::Way, id))) {
            return Pick::default();
        }
        Pick {
            node: None,
            way,
            relation: None,
        }
    }

    /// 点在面的内圈上时选中该面。重叠时取面积更小的那个。
    fn pick_area_band(&self, screen: [f32; 2]) -> Option<Pick> {
        let scene = self.scene.as_ref()?;
        let world = self.camera.screen_to_world(screen);
        let mut best: Option<(f64, ObjectKind, ObjectId)> = None;
        for band in &scene.area_bands {
            if self.hidden.contains(&(band.kind, band.id)) {
                continue;
            }
            let inside = if band.solid {
                neo_geo::geometry::point_in_ring(world, &band.outer)
                    && !band
                        .holes
                        .iter()
                        .any(|hole| neo_geo::geometry::point_in_ring(world, hole))
            } else {
                neo_geo::geometry::point_in_inner_band(
                    world,
                    &band.outer,
                    &band.holes,
                    band.inset_m,
                    band.width_m,
                )
            };
            if !inside {
                continue;
            }
            let area = neo_geo::geometry::signed_area(&band.outer).abs();
            if best.as_ref().map(|(have, _, _)| area < *have).unwrap_or(true) {
                best = Some((area, band.kind, band.id));
            }
        }
        let (_, kind, id) = best?;
        Some(match kind {
            ObjectKind::Way => Pick {
                node: None,
                way: Some(id),
                relation: None,
            },
            ObjectKind::Relation => Pick {
                node: None,
                way: None,
                relation: Some(id),
            },
            ObjectKind::Node | ObjectKind::Changeset => return None,
        })
    }

    /// 当前缩放是否允许「选中/悬停 way 时强制显示成员节点」（与节点 LOD 下限一致：z≥17）。
    pub(super) fn way_member_nodes_force_zoom(&self) -> bool {
        let zoom = self
            .scene
            .as_ref()
            .map(|scene| scene.camera.zoom)
            .unwrap_or(self.camera.zoom);
        zoom >= neo_scene::NODE_MIN_ZOOM
    }

    /// 节点是否因选中/悬停 way 而强制显示（等同高缩放下全部节点可见；仅 z≥17）。
    pub(super) fn node_forced_visible(&self, node: ObjectId) -> bool {
        if !self.way_member_nodes_force_zoom() {
            return false;
        }
        if self.hidden.contains(&(ObjectKind::Node, node)) {
            return false;
        }
        if self.editor.data().node_handle(node).is_none() {
            return false;
        }
        if let Some(way) = self.hover_way
            && !self.hidden.contains(&(ObjectKind::Way, way))
            && let Some(way_handle) = self.editor.data().way_handle(way)
            && self.editor.data().way_node_ids(way_handle).contains(&node)
        {
            return true;
        }
        for way in self.editor.selection().ways() {
            if self.hidden.contains(&(ObjectKind::Way, *way)) {
                continue;
            }
            let Some(way_handle) = self.editor.data().way_handle(*way) else {
                continue;
            };
            if self.editor.data().way_node_ids(way_handle).contains(&node) {
                return true;
            }
        }
        false
    }

    /// 节点在当
    /// zoom 下是否可
    /// 可拾取（
    /// renderer 共用 `node_visible_at` 规则；选中/悬停 way 的成员节点始终可选）
    pub(super) fn node_selectable(&self, node: ObjectId) -> bool {
        if self.hidden.contains(&(ObjectKind::Node, node)) {
            return false;
        }
        if self.node_forced_visible(node) {
            return true;
        }
        let Some(handle) = self.editor.data().node_handle(node) else {
            return false;
        };
        let refs = self.editor.referrers().ways_of_node(handle).len();
        // 用场景构建时
        // zoom，而不是实
        // zoom：画面还没重建时两者不一致，
        // 否则会出
        // 看不见的节点能选中"
        // 看得见的节点点不

        let zoom = self
            .scene
            .as_ref()
            .map(|scene| scene.camera.zoom)
            .unwrap_or(self.camera.zoom);
        neo_scene::node_visible_at(zoom, refs)
    }

    /// 缩放后清理已不可见的节点选择（way/relation 选择保留）
    pub(super) fn prune_hidden_node_selection(&mut self) {
        let hidden: Vec<ObjectId> = self
            .editor
            .selection()
            .nodes()
            .iter()
            .copied()
            .filter(|id| !self.node_selectable(*id))
            .collect();
        for id in hidden {
            self.editor.selection_mut().remove(ObjectKind::Node, id);
        }
    }

    pub(super) fn map_canvas(&mut self, ui: &mut egui::Ui) {
        let (rect, response) = ui.allocate_exact_size(ui.available_size(), egui::Sense::click_and_drag());
        let ppp = ui.ctx().pixels_per_point();
        let viewport_px = [
            (rect.width() * ppp).round().max(1.0) as u32,
            (rect.height() * ppp).round().max(1.0) as u32,
        ];
        self.camera.set_viewport(viewport_px);
        self.ensure_imagery_tiles(ui.ctx());
        self.poll_imagery(ui.ctx());
        self.draw_imagery(ui.painter(), rect, ppp);

        let pointer_screen = |pos: egui::Pos2| [(pos.x - rect.min.x) * ppp, (pos.y - rect.min.y) * ppp];
        let editable =
            self.data_listed && self.layer_active && self.layer_visible && !self.download_state.selecting;

        // 左键拖拽：命中节

        // 移动该节点；否则若已有选中集合
        // 直接拖动选中元素
        // （不需要长按：选中后任意位置左键拖拽即可）
        // 下载框选：左键拖出矩形（不进入编辑拖拽）
        if self.download_state.selecting {
            if response.drag_started_by(egui::PointerButton::Primary) {
                let press_origin = ui.input(|i| i.pointer.press_origin());
                if let Some(pos) = press_origin.or_else(|| response.interact_pointer_pos()) {
                    let world = self.camera.screen_to_world(pointer_screen(pos));
                    self.bbox_drag = Some((world, world));
                }
            }
            if response.dragged_by(egui::PointerButton::Primary)
                && let (Some((start, _)), Some(pos)) = (self.bbox_drag, response.interact_pointer_pos())
            {
                let world = self.camera.screen_to_world(pointer_screen(pos));
                self.bbox_drag = Some((start, world));
            }
            if response.drag_stopped_by(egui::PointerButton::Primary)
                && let Some((start, end)) = self.bbox_drag.take()
            {
                let a = start.to_latlon();
                let b = end.to_latlon();
                let min_lat = a.lat.to_degrees().min(b.lat.to_degrees());
                let max_lat = a.lat.to_degrees().max(b.lat.to_degrees());
                let min_lon = a.lon.to_degrees().min(b.lon.to_degrees());
                let max_lon = a.lon.to_degrees().max(b.lon.to_degrees());
                if (max_lon - min_lon).abs() > 1e-9 && (max_lat - min_lat).abs() > 1e-9 {
                    self.download_state.bbox = Some(neo_ui::BboxView {
                        min_lon,
                        min_lat,
                        max_lon,
                        max_lat,
                    });
                }
                self.download_state.selecting = false;
                ui.ctx().request_repaint();
            }
        }

        if editable && response.drag_started_by(egui::PointerButton::Primary) && self.mode == UiMode::Select {
            //
            // 按下原点"而不是当前指针位置来判定命中/外边距：拖拽回调触发时指针已经移动了几像素
            let press_origin = ui.input(|i| i.pointer.press_origin());
            if let Some(pos) = press_origin.or_else(|| response.interact_pointer_pos()) {
                let screen = pointer_screen(pos);
                let world = self.camera.screen_to_world(screen);
                if let Some((way, seg)) = self.pick_midpoint_handle(screen) {
                    // 按下中点即插入节点，并立即进入拖动该新点。
                    match self.controller.insert_midpoint_node(&mut self.editor, way, seg) {
                        Ok(node) => {
                            let from = self
                                .editor
                                .data()
                                .node_handle(node)
                                .and_then(|h| self.editor.data().nodes.position(h))
                                .unwrap_or_else(|| world.to_latlon());
                            let anchor = WorldPoint::from_latlon(from);
                            self.drag = Some(DragState {
                                nodes: vec![(node, from)],
                                anchor,
                                current: world,
                            });
                            self.suppress_primary_click = true;
                            self.apply_outcome(neo_tools::Outcome::Edited(format!("在中点创建节点 {node}")));
                            ui.ctx().request_repaint();
                        }
                        Err(err) => {
                            self.status = err;
                        }
                    }
                } else {
                    let pick = self.pick_at(screen);
                    // 能拖动必须先选中：按下时命中未选中的要

                    // 先把它设为选中（红框立即出现）
                    if let Some(node) = pick.node {
                        if !self.editor.selection().contains(ObjectKind::Node, node) {
                            self.editor.selection_mut().select_only(ObjectKind::Node, node);
                        }
                    } else if let Some(way) = pick.way
                        && !self.editor.selection().contains(ObjectKind::Way, way)
                    {
                        self.editor.selection_mut().select_only(ObjectKind::Way, way);
                    }
                    // 拖动范围 = 当前选中的节
                    // + 选中 way 的全部节点
                    // （未命中任何要素时，只有指针在选中要素外边距内才允许拖动。）
                    let hit_feature = pick.node.is_some() || pick.way.is_some();
                    let within_selection_margin = !self.editor.selection().is_empty()
                        && self
                            .selection_screen_distance(screen)
                            .map(|distance| distance <= SELECTION_DRAG_MARGIN_PX)
                            .unwrap_or(false);
                    if hit_feature || within_selection_margin {
                        let nodes = self.collect_selection_nodes();
                        if nodes
                            .iter()
                            .any(|(_, pos)| pos.lat.to_degrees().abs() > neo_geo::MAX_MERCATOR_LAT)
                        {
                            // WebMercator
                            // |lat|>85.05° 压到边界；拖动会
                            // clamp 后的
                            // 纬度写回数据（对南极数据是静默损坏）
                            self.status = t("app.error.polar_drag").to_owned();
                        } else if !nodes.is_empty() {
                            self.drag = Some(DragState {
                                nodes,
                                anchor: world,
                                current: world,
                            });
                            ui.ctx().request_repaint();
                        }
                    } else {
                        // 空白处左键拖
                        // = 框选（JOSM 的选择工具行为）
                        self.box_select = Some((world, world));
                    }
                }
            }
        }

        // 左键拖动：直接预览写入坐标，要素跟随鼠标（释放时再记一条 MoveNode 历史）。
        if editable
            && response.dragged_by(egui::PointerButton::Primary)
            && let Some(pos) = response.interact_pointer_pos()
            && self.drag.is_some()
        {
            let world = self.camera.screen_to_world(pointer_screen(pos));
            if let Some(drag) = &mut self.drag {
                drag.current = world;
            }
            if let Some(drag) = &self.drag {
                for (id, from) in &drag.nodes {
                    let to = drag.shifted(*from);
                    let _ = self.editor.preview_move_node(*id, to);
                }
                self.scene = None;
            }
        }
        if response.dragged_by(egui::PointerButton::Primary)
            && let Some((start, _)) = self.box_select
            && let Some(pos) = response.interact_pointer_pos()
        {
            self.box_select = Some((start, self.camera.screen_to_world(pointer_screen(pos))));
        }

        // 右键拖动：平移地图（背景）
        if response.dragged_by(egui::PointerButton::Secondary) {
            let delta = response.drag_delta();
            self.camera.pan_by_screen(delta.x * ppp, delta.y * ppp);
        }

        if response.drag_stopped_by(egui::PointerButton::Primary)
            && let Some(drag) = self.drag.take()
            && editable
        {
            // 只有活动图层才提交移
            // 释放时才写入 DataSet：一次批量移
            // = 一次重
            // + 一条历史
            let moves: Vec<(ObjectId, LatLon, LatLon)> = drag
                .nodes
                .iter()
                .map(|(id, from)| (*id, *from, drag.shifted(*from)))
                .collect();
            let outcome = self.controller.move_nodes_batch(&mut self.editor, &moves);
            self.apply_outcome(outcome);
            // 有实质拖动时 egui 通常不再发 click；清掉中点按下的抑制标记。
            self.suppress_primary_click = false;
            ui.ctx().request_repaint();
        }

        if response.drag_stopped_by(egui::PointerButton::Primary)
            && let Some((start, end)) = self.box_select.take()
        {
            let additive = ui.input(|i| i.modifiers.command);
            self.select_in_box(start, end, additive);
            ui.ctx().request_repaint();
        }

        if response.clicked()
            && editable
            && let Some(pos) = response.interact_pointer_pos()
        {
            if self.suppress_primary_click {
                self.suppress_primary_click = false;
            } else {
                let screen = pointer_screen(pos);
                let world = self.camera.screen_to_world(screen);
                if self.mode == UiMode::Measure {
                    self.measure_click(world);
                } else if self.mode == UiMode::Select {
                    let pick = self.pick_at(screen);
                    let additive = ui.input(|i| i.modifiers.command);
                    let outcome =
                        self.controller
                            .on_click_multi(&mut self.editor, pick, world.to_latlon(), additive);
                    self.apply_outcome(outcome);
                } else if self.controller.tool.is_draw() {
                    let additive = ui.input(|i| i.modifiers.command);
                    let alt = ui.input(|i| i.modifiers.alt);
                    self.place_draw_at(screen, world, additive, alt);
                }
            }
        }

        // 绘制：空格 = 左键落点。
        if editable
            && self.controller.tool.is_draw()
            && response.hovered()
            && ui.input(|i| i.key_pressed(egui::Key::Space))
            && let Some(pointer) = ui
                .input(|i| i.pointer.hover_pos())
                .or(response.interact_pointer_pos())
        {
            let screen = pointer_screen(pointer);
            let world = self.camera.screen_to_world(screen);
            let additive = ui.input(|i| i.modifiers.command);
            let alt = ui.input(|i| i.modifiers.alt);
            self.place_draw_at(screen, world, additive, alt);
        }

        // 绘制：右键单击回退上一步绘制（右键拖平移仍可用）。
        if editable && self.controller.tool.is_draw() && response.clicked_by(egui::PointerButton::Secondary) {
            let outcome = self.controller.undo_last_draw(&mut self.editor);
            if outcome != neo_tools::Outcome::None {
                self.apply_outcome(outcome);
                self.scene = None;
                ui.ctx().request_repaint();
            }
        }

        if response.hovered() {
            let scroll = ui.input(|i| i.smooth_scroll_delta.y);
            if scroll.abs() > 0.01
                && let Some(pointer) = ui.input(|i| i.pointer.hover_pos())
            {
                let anchor = pointer_screen(pointer);
                self.camera.zoom_about(anchor, f64::from(scroll) / 240.0);
            }
        }

        // 鼠标悬停位置（状态栏经纬
        // + 测距实时终点）+ 绘制吸附预览 + 中点三角候选 way
        if response.hovered() {
            if let Some(pointer) = ui.input(|i| i.pointer.hover_pos())
                && rect.contains(pointer)
            {
                let screen = pointer_screen(pointer);
                let world = self.camera.screen_to_world(screen);
                self.hover_world = Some(world);
                self.hover_latlon = Some(world.to_latlon());
                let alt = ui.input(|i| i.modifiers.alt);
                self.snap_hit = self.resolve_draw_snap(screen, alt);
                if let Some(hit) = &self.snap_hit
                    && let Some(snap_world) = hit.world_point(self.editor.data())
                {
                    self.hover_world = Some(snap_world);
                    self.hover_latlon = Some(snap_world.to_latlon());
                }
                self.hover_way = if self.mode == UiMode::Select {
                    // 中点优先：悬停在三角上时仍保留对应 way。
                    self.pick_midpoint_handle(screen)
                        .map(|(way, _)| way)
                        .or_else(|| self.pick_at(screen).way)
                } else {
                    None
                };
            }
        } else {
            self.hover_world = None;
            self.hover_latlon = None;
            self.snap_hit = None;
            self.hover_way = None;
        }

        self.prune_hidden_node_selection();
        if self.data_listed && self.layer_visible {
            self.ensure_scene();
        }

        if self.data_listed
            && self.layer_visible
            && let Some(scene) = &self.scene
        {
            let callback = MapCallback {
                scene: scene.clone(),
                camera: self.camera,
                viewport_px,
                grey: !self.layer_active,
            };
            ui.painter()
                .add(egui_wgpu::Callback::new_paint_callback(rect, callback));
        }
        if self.data_listed && self.layer_visible && self.scene.is_some() {
            self.draw_icons(ui, rect, ppp);
            if let Some(scene) = &self.scene {
                paint_scene_labels(ui.painter(), scene, &self.camera, rect, ppp);
            }
        }
        // 活动数据层：已下载范围外置灰（多次框选取并集挖洞）。
        if self.data_listed && self.layer_active && self.layer_visible && !self.downloaded_bboxes.is_empty() {
            draw_undownloaded_mask(ui.painter(), rect, &self.camera, ppp, &self.downloaded_bboxes);
        }
        if let Some((start, end)) = self.box_select {
            let ppp = ui.ctx().pixels_per_point();
            let to_screen = |world: WorldPoint| {
                let point = self.camera.world_to_screen(world);
                egui::pos2(rect.min.x + point[0] / ppp, rect.min.y + point[1] / ppp)
            };
            // CAD：左→右窗选（蓝），右→左相交选（绿虚线感：半透明填充 + 描边）。
            let crossing = neo_geo::box_select_mode(start, end) == neo_geo::BoxSelectMode::Crossing;
            let (stroke, fill) = if crossing {
                (
                    egui::Color32::from_rgb(70, 190, 110),
                    egui::Color32::from_rgba_unmultiplied(70, 190, 110, 35),
                )
            } else {
                (
                    egui::Color32::from_rgb(90, 170, 255),
                    egui::Color32::from_rgba_unmultiplied(90, 170, 255, 28),
                )
            };
            ui.painter().rect(
                egui::Rect::from_two_pos(to_screen(start), to_screen(end)),
                0.0,
                fill,
                egui::Stroke::new(1.5, stroke),
                egui::StrokeKind::Inside,
            );
        }
        // 选中高亮 + 选中/悬停 way 的成员节点（普通节点样式）
        self.draw_way_member_nodes_overlay(ui, rect, ppp);
        self.draw_selection_overlay(ui, rect, ppp);
        self.draw_midpoint_handles(ui, rect, ppp);
        self.draw_snap_overlay(ui, rect, ppp);
        self.draw_pending_draw_overlay(ui, rect, ppp);
        // 测距叠层：起

        // 终点（或鼠标位置），并显示距离
        if let Some(start) = self.measure_start
            && let Some(end) = self.measure_end.or(self.hover_world)
        {
            let to_screen = |world: WorldPoint| -> egui::Pos2 {
                let screen = self.camera.world_to_screen(world);
                rect.min + egui::vec2(screen[0] / ppp, screen[1] / ppp)
            };
            let p0 = to_screen(start);
            let p1 = to_screen(end);
            let yellow = egui::Color32::from_rgb(255, 205, 90);
            let painter = ui.painter();
            painter.line_segment([p0, p1], egui::Stroke::new(2.0, yellow));
            painter.circle_filled(p0, 4.0, yellow);
            painter.circle_filled(p1, 4.0, yellow);
            let meters = start.dist(end);
            let label = if meters >= 1000.0 {
                format!("{:.3} km", meters / 1000.0)
            } else {
                format!("{:.1} m", meters)
            };
            painter.text(
                egui::pos2((p0.x + p1.x) * 0.5, (p0.y + p1.y) * 0.5 - 6.0),
                egui::Align2::CENTER_BOTTOM,
                label,
                egui::FontId::monospace(12.0),
                yellow,
            );
        }
        // 下载框选矩形（拖拽
        // + 已选范围）
        let bbox_corners = if let Some((start, current)) = self.bbox_drag {
            Some((start, current))
        } else {
            self.download_state.bbox.map(|bbox| {
                (
                    WorldPoint::from_latlon(LatLon::from_degrees(bbox.min_lat, bbox.min_lon)),
                    WorldPoint::from_latlon(LatLon::from_degrees(bbox.max_lat, bbox.max_lon)),
                )
            })
        };
        if let Some((a, b)) = bbox_corners {
            let to_screen = |world: WorldPoint| -> egui::Pos2 {
                let screen = self.camera.world_to_screen(world);
                rect.min + egui::vec2(screen[0] / ppp, screen[1] / ppp)
            };
            let p0 = to_screen(a);
            let p1 = to_screen(b);
            let bbox_rect = egui::Rect::from_two_pos(p0, p1);
            let color = egui::Color32::from_rgb(90, 200, 160);
            ui.painter().rect_filled(
                bbox_rect,
                0.0,
                egui::Color32::from_rgba_unmultiplied(90, 200, 160, 30),
            );
            ui.painter().rect_stroke(
                bbox_rect,
                0.0,
                egui::Stroke::new(2.0, color),
                egui::StrokeKind::Inside,
            );
        }
        ui.painter().rect_stroke(
            rect,
            0.0,
            egui::Stroke::new(1.0, egui::Color32::from_gray(70)),
            egui::StrokeKind::Inside,
        );
    }

    /// 按当前过滤器重算隐藏集合（用户点“应用”时才扫描）。
    pub(super) fn recompute_filter(&mut self) {
        self.hidden.clear();
        if !self.filter.active {
            self.builder.set_hidden(self.hidden.clone());
            self.scene = None;
            return;
        }
        let query = self.filter.query.trim().to_lowercase();
        let id_query = query
            .strip_prefix("id:")
            .and_then(|value| value.trim().parse::<i64>().ok());
        let tag_query = query
            .split_once('=')
            .map(|(key, value)| (key.trim().to_lowercase(), value.trim().to_lowercase()));
        let data = self.editor.data();
        let matches = |id: ObjectId, pairs: &[(neo_core::Sym, neo_core::Sym)]| -> bool {
            if query.is_empty() {
                return true;
            }
            if let Some(wanted) = id_query {
                return id.0 == wanted;
            }
            if let Some((key, value)) = &tag_query {
                return pairs.iter().any(|(k, v)| {
                    data.resolve(*k).eq_ignore_ascii_case(key) && data.resolve(*v).eq_ignore_ascii_case(value)
                });
            }
            id.0.to_string().contains(&query)
                || pairs.iter().any(|(key, value)| {
                    data.resolve(*key).to_lowercase().contains(&query)
                        || data.resolve(*value).to_lowercase().contains(&query)
                })
        };
        if self.filter.nodes {
            for node in data.iter_nodes() {
                if !matches(node.id, data.tag_pairs(node.tags)) {
                    self.hidden.insert((ObjectKind::Node, node.id));
                }
            }
        } else {
            for node in data.iter_nodes() {
                self.hidden.insert((ObjectKind::Node, node.id));
            }
        }
        if self.filter.ways {
            for way in data.iter_ways() {
                if !matches(way.id, data.tag_pairs(way.tags)) {
                    self.hidden.insert((ObjectKind::Way, way.id));
                }
            }
        } else {
            for way in data.iter_ways() {
                self.hidden.insert((ObjectKind::Way, way.id));
            }
        }
        if self.filter.relations {
            for relation in data.iter_relations() {
                if !matches(relation.id, data.tag_pairs(relation.tags)) {
                    self.hidden.insert((ObjectKind::Relation, relation.id));
                }
            }
        } else {
            for relation in data.iter_relations() {
                self.hidden.insert((ObjectKind::Relation, relation.id));
            }
        }
        let count = self.hidden.len();
        self.builder.set_hidden(self.hidden.clone());
        self.scene = None;
        self.status = tf("status.filter_applied", &[("n", &count.to_string())]);
    }

    /// 框选（CAD / JOSM）：左→右仅整体在框内；右→左选相交或在框内。
    pub(super) fn select_in_box(&mut self, start: WorldPoint, end: WorldPoint, additive: bool) {
        let bounds = WorldBounds::from_points(start, end);
        let mode = neo_geo::box_select_mode(start, end);
        let mut nodes = Vec::new();
        let mut ways = Vec::new();
        self.editor.index().for_each_node(bounds, |entry| {
            let Some(handle) = entry.as_node() else {
                return;
            };
            let Some(pos) = self.editor.data().nodes.position(handle) else {
                return;
            };
            if bounds.contains(WorldPoint::from_latlon(pos))
                && let Some(id) = self.editor.data().nodes.id(handle)
                && self.node_selectable(id)
                && !self.hidden.contains(&(ObjectKind::Node, id))
            {
                nodes.push(id);
            }
        });
        self.editor.index().for_each_way(bounds, |entry| {
            let Some(handle) = entry.as_way() else {
                return;
            };
            let points = neo_geo::geometry::way_world_points(self.editor.data(), handle);
            if neo_geo::way_matches_box(&points, &bounds, mode)
                && let Some(id) = self.editor.data().ways.id(handle)
                && !self.hidden.contains(&(ObjectKind::Way, id))
            {
                ways.push(id);
            }
        });

        if !additive {
            self.editor.selection_mut().clear();
        }
        let total = nodes.len() + ways.len();
        for id in nodes {
            self.editor.selection_mut().insert(ObjectKind::Node, id);
        }
        for id in ways {
            self.editor.selection_mut().insert(ObjectKind::Way, id);
        }
        self.status = tf("status.box_selected", &[("n", &total.to_string())]);
        self.resync_tags();
    }

    /// 鼠标屏幕坐标到"选中要素"几何的最近距离（像素）；没有选中要素时返回 None。
    ///
    /// 节点按点距离；way 按折线线段距离。用于判断是否在可拖动外边距内。
    pub(super) fn selection_screen_distance(&self, screen: [f32; 2]) -> Option<f32> {
        let selection = self.editor.selection();
        if selection.is_empty() {
            return None;
        }
        let mut best = f32::INFINITY;
        let to_screen =
            |pos: LatLon| -> [f32; 2] { self.camera.world_to_screen(WorldPoint::from_latlon(pos)) };
        for node in selection.nodes() {
            let Some(handle) = self.editor.data().node_handle(*node) else {
                continue;
            };
            let Some(pos) = self.editor.data().nodes.position(handle) else {
                continue;
            };
            let p = to_screen(pos);
            let distance = ((p[0] - screen[0]).powi(2) + (p[1] - screen[1]).powi(2)).sqrt();
            best = best.min(distance);
        }
        for way in selection.ways() {
            let Some(handle) = self.editor.data().way_handle(*way) else {
                continue;
            };
            let points: Vec<[f32; 2]> = self
                .editor
                .data()
                .way_node_ids(handle)
                .iter()
                .filter_map(|id| {
                    let h = self.editor.data().node_handle(*id)?;
                    let p = self.editor.data().nodes.position(h)?;
                    Some(to_screen(p))
                })
                .collect();
            for pair in points.windows(2) {
                best = best.min(point_segment_distance(screen, pair[0], pair[1]));
            }
        }
        best.is_finite().then_some(best)
    }

    /// 绘制工具在屏幕位置落点（左键 / 空格共用）。
    pub(super) fn place_draw_at(&mut self, screen: [f32; 2], world: WorldPoint, additive: bool, alt: bool) {
        let pick = self.pick_at(screen);
        let snap = self.resolve_draw_snap(screen, alt);
        let place = snap
            .as_ref()
            .and_then(|hit| hit.latlon(self.editor.data()))
            .unwrap_or_else(|| world.to_latlon());
        let outcome = self
            .controller
            .on_click_multi_with_snap(&mut self.editor, pick, place, additive, snap);
        self.apply_outcome(outcome);
        if self.controller.take_finished_feature() {
            self.switch_to_select_tool();
        }
    }

    /// 绘制完成后恢复选择工具。
    pub(super) fn switch_to_select_tool(&mut self) {
        self.mode = UiMode::Select;
        self.controller.set_tool(neo_tools::ToolKind::Select);
        self.status = tf("app.status.tool", &[("tool", self.mode_label())]);
    }

    /// 选择模式下应显示/可点中点手柄的 way：选中集合 ∪ 悬停 way。
    fn midpoint_candidate_ways(&self) -> Vec<ObjectId> {
        let mut ways: Vec<ObjectId> = self.editor.selection().ways().iter().copied().collect();
        if let Some(way) = self.hover_way
            && !ways.contains(&way)
            && !self.hidden.contains(&(ObjectKind::Way, way))
        {
            ways.push(way);
        }
        ways
    }

    /// 候选 way 上可见的线段中点手柄（屏幕坐标足够长才出现）。
    fn iter_midpoint_handles(
        &self,
        rect: egui::Rect,
        ppp: f32,
    ) -> Vec<(ObjectId, usize, egui::Pos2, egui::Vec2)> {
        let mut out = Vec::new();
        if !self.data_listed || !self.layer_visible || !self.layer_active {
            return out;
        }
        let to_pos = |latlon: LatLon| -> egui::Pos2 {
            let screen = self.camera.world_to_screen(WorldPoint::from_latlon(latlon));
            rect.min + egui::vec2(screen[0] / ppp, screen[1] / ppp)
        };
        for way in self.midpoint_candidate_ways() {
            if self.hidden.contains(&(ObjectKind::Way, way)) {
                continue;
            }
            let Some(handle) = self.editor.data().way_handle(way) else {
                continue;
            };
            let nodes = self.editor.data().way_node_ids(handle);
            for seg in 0..nodes.len().saturating_sub(1) {
                let Some(pa) = self
                    .editor
                    .data()
                    .node_handle(nodes[seg])
                    .and_then(|h| self.editor.data().nodes.position(h))
                else {
                    continue;
                };
                let Some(pb) = self
                    .editor
                    .data()
                    .node_handle(nodes[seg + 1])
                    .and_then(|h| self.editor.data().nodes.position(h))
                else {
                    continue;
                };
                let a = to_pos(pa);
                let b = to_pos(pb);
                let along = b - a;
                if along.length() < MIDPOINT_MIN_SEGMENT_PX {
                    continue;
                }
                let mid = egui::pos2((a.x + b.x) * 0.5, (a.y + b.y) * 0.5);
                if !rect.expand(MIDPOINT_HIT_RADIUS_PX).contains(mid) {
                    continue;
                }
                out.push((way, seg, mid, along));
            }
        }
        out
    }

    /// 命中线段中点三角；返回 `(way, seg)`。
    pub(super) fn pick_midpoint_handle(&self, screen: [f32; 2]) -> Option<(ObjectId, usize)> {
        let mut ways = self.midpoint_candidate_ways();
        // 点击瞬间也纳入指针下方的 way，避免仅悬停未写入 hover_way 时点不中。
        if let Some(way) = self.pick_at(screen).way
            && !ways.contains(&way)
        {
            ways.push(way);
        }
        let mut best: Option<(f32, ObjectId, usize)> = None;
        for way in ways {
            if self.hidden.contains(&(ObjectKind::Way, way)) {
                continue;
            }
            let Some(handle) = self.editor.data().way_handle(way) else {
                continue;
            };
            let nodes = self.editor.data().way_node_ids(handle);
            for seg in 0..nodes.len().saturating_sub(1) {
                let Some(pa) = self
                    .editor
                    .data()
                    .node_handle(nodes[seg])
                    .and_then(|h| self.editor.data().nodes.position(h))
                else {
                    continue;
                };
                let Some(pb) = self
                    .editor
                    .data()
                    .node_handle(nodes[seg + 1])
                    .and_then(|h| self.editor.data().nodes.position(h))
                else {
                    continue;
                };
                let sa = self.camera.world_to_screen(WorldPoint::from_latlon(pa));
                let sb = self.camera.world_to_screen(WorldPoint::from_latlon(pb));
                let dx = sb[0] - sa[0];
                let dy = sb[1] - sa[1];
                let len = (dx * dx + dy * dy).sqrt();
                if len < MIDPOINT_MIN_SEGMENT_PX {
                    continue;
                }
                let mx = (sa[0] + sb[0]) * 0.5;
                let my = (sa[1] + sb[1]) * 0.5;
                let dist = ((screen[0] - mx).powi(2) + (screen[1] - my).powi(2)).sqrt();
                if dist <= MIDPOINT_HIT_RADIUS_PX && best.as_ref().map(|(d, _, _)| dist < *d).unwrap_or(true)
                {
                    best = Some((dist, way, seg));
                }
            }
        }
        best.map(|(_, way, seg)| (way, seg))
    }

    /// 线段中点三角标记（选中或悬停的 way；颜色/可见性与普通节点一致）。
    pub(super) fn draw_midpoint_handles(&self, ui: &egui::Ui, rect: egui::Rect, ppp: f32) {
        if self.mode != UiMode::Select || !self.layer_active {
            return;
        }
        let zoom = self
            .scene
            .as_ref()
            .map(|scene| scene.camera.zoom)
            .unwrap_or(self.camera.zoom);
        // 与普通节点同一 LOD：z < 17 不显示。
        if !neo_scene::node_visible_at(zoom, 2) {
            return;
        }
        let painter = ui.painter();
        // 与默认节点色一致（非高亮）。
        let fill = egui::Color32::from_rgba_unmultiplied(250, 250, 250, 255);
        let stroke = egui::Stroke::new(1.0, egui::Color32::from_rgb(60, 60, 60));
        let size = 3.5; // 约等于默认 node_radius_px
        for (_, _, mid, along) in self.iter_midpoint_handles(rect, ppp) {
            let len = along.length().max(1.0);
            let tangent = along / len;
            let normal = egui::vec2(-tangent.y, tangent.x);
            let p1 = mid + normal * size;
            let p2 = mid - normal * (size * 0.55) + tangent * (size * 0.75);
            let p3 = mid - normal * (size * 0.55) - tangent * (size * 0.75);
            painter.add(egui::Shape::convex_polygon(vec![p1, p2, p3], fill, stroke));
        }
    }

    /// 绘制工具下解析吸附（设置关闭或按住 Alt 时返回 None）。
    pub(super) fn resolve_draw_snap(&self, screen: [f32; 2], alt_held: bool) -> Option<SnapHit> {
        if !self.controller.tool.is_draw()
            || !self.settings.draw_snap_enabled
            || !self.controller.snap_enabled
            || alt_held
            || !self.data_listed
            || !self.layer_visible
            || !self.layer_active
        {
            return None;
        }
        let radius = self.settings.draw_snap_radius_px.max(1.0);
        resolve_snap(
            self.editor.data(),
            self.editor.index(),
            &self.camera,
            screen,
            radius,
            |id| self.node_selectable(id) && !self.hidden.contains(&(ObjectKind::Node, id)),
            |id| !self.hidden.contains(&(ObjectKind::Way, id)),
        )
    }

    /// 吸附 / 可续画端点预览（仅画布，不写状态栏）。
    pub(super) fn draw_snap_overlay(&self, ui: &egui::Ui, rect: egui::Rect, ppp: f32) {
        if !self.data_listed || !self.layer_visible || !self.controller.tool.is_draw() {
            return;
        }
        let accent = egui::Color32::from_rgb(70, 190, 255);
        let to_screen = |latlon: LatLon| -> egui::Pos2 {
            let screen = self.camera.world_to_screen(WorldPoint::from_latlon(latlon));
            rect.min + egui::vec2(screen[0] / ppp, screen[1] / ppp)
        };
        let painter = ui.painter();
        let Some(hit) = &self.snap_hit else {
            return;
        };
        match hit.target {
            SnapTarget::Node { id } => {
                let Some(handle) = self.editor.data().node_handle(id) else {
                    return;
                };
                let Some(pos) = self.editor.data().nodes.position(handle) else {
                    return;
                };
                let screen = to_screen(pos);
                let continuable = self.controller.tool == neo_tools::ToolKind::DrawLine
                    && self.controller.pending_len() == 0
                    && is_open_way_endpoint(self, id);
                let radius = if continuable { 11.0 } else { 9.0 };
                painter.circle_stroke(screen, radius, egui::Stroke::new(2.5, accent));
                if continuable {
                    painter.circle_stroke(
                        screen,
                        radius + 4.0,
                        egui::Stroke::new(1.5, egui::Color32::from_rgb(240, 250, 255)),
                    );
                }
                // 十字标记
                let arm = 6.0;
                painter.line_segment(
                    [screen + egui::vec2(-arm, 0.0), screen + egui::vec2(arm, 0.0)],
                    egui::Stroke::new(1.5, accent),
                );
                painter.line_segment(
                    [screen + egui::vec2(0.0, -arm), screen + egui::vec2(0.0, arm)],
                    egui::Stroke::new(1.5, accent),
                );
            }
            SnapTarget::WaySegment { way, seg, at } => {
                let Some(handle) = self.editor.data().way_handle(way) else {
                    return;
                };
                let nodes = self.editor.data().way_node_ids(handle);
                if let (Some(a), Some(b)) = (nodes.get(seg), nodes.get(seg + 1)) {
                    let pa = self
                        .editor
                        .data()
                        .node_handle(*a)
                        .and_then(|h| self.editor.data().nodes.position(h));
                    let pb = self
                        .editor
                        .data()
                        .node_handle(*b)
                        .and_then(|h| self.editor.data().nodes.position(h));
                    if let (Some(pa), Some(pb)) = (pa, pb) {
                        painter.line_segment([to_screen(pa), to_screen(pb)], egui::Stroke::new(4.0, accent));
                    }
                }
                let screen = to_screen(at);
                painter.circle_filled(screen, 5.0, accent);
                painter.circle_stroke(
                    screen,
                    7.0,
                    egui::Stroke::new(1.5, egui::Color32::from_rgb(240, 250, 255)),
                );
            }
        }
    }

    /// 绘制中：路径加粗、节点放大，并从末点拉出到鼠标的橡皮筋。
    pub(super) fn draw_pending_draw_overlay(&self, ui: &egui::Ui, rect: egui::Rect, ppp: f32) {
        if !self.data_listed || !self.layer_visible || !self.controller.tool.is_draw() {
            return;
        }
        let pending = self.controller.pending_nodes();
        if pending.is_empty() {
            return;
        }
        let accent = egui::Color32::from_rgb(70, 190, 255);
        let to_screen = |latlon: LatLon| -> egui::Pos2 {
            let screen = self.camera.world_to_screen(WorldPoint::from_latlon(latlon));
            rect.min + egui::vec2(screen[0] / ppp, screen[1] / ppp)
        };
        let painter = ui.painter();
        if let Some(way) = self.controller.pending_way() {
            self.paint_way_highlight(painter, way, egui::Stroke::new(4.5, accent), &to_screen);
        } else if pending.len() >= 2 {
            let points: Vec<egui::Pos2> = pending
                .iter()
                .filter_map(|id| {
                    let handle = self.editor.data().node_handle(*id)?;
                    let pos = self.editor.data().nodes.position(handle)?;
                    Some(to_screen(pos))
                })
                .collect();
            if points.len() >= 2 {
                painter.add(egui::Shape::line(points, egui::Stroke::new(4.5, accent)));
            }
        }
        for (index, node) in pending.iter().enumerate() {
            let Some(handle) = self.editor.data().node_handle(*node) else {
                continue;
            };
            let Some(pos) = self.editor.data().nodes.position(handle) else {
                continue;
            };
            let screen = to_screen(pos);
            let radius = if index == 0 { 8.0 } else { 6.5 };
            painter.circle_filled(screen, radius, accent);
            painter.circle_stroke(
                screen,
                radius + 1.5,
                egui::Stroke::new(1.5, egui::Color32::from_rgb(240, 250, 255)),
            );
        }
        if let Some(last) = pending.last()
            && let Some(handle) = self.editor.data().node_handle(*last)
            && let Some(pos) = self.editor.data().nodes.position(handle)
            && let Some(hover) = self.hover_world
        {
            // 面绘制时若已回到起点附近，橡皮筋可暗示即将闭合；这里统一连到指针。
            painter.line_segment(
                [to_screen(pos), to_screen(hover.to_latlon())],
                egui::Stroke::new(2.5, egui::Color32::from_rgba_unmultiplied(70, 190, 255, 180)),
            );
        }
    }

    /// 选中或悬停 way 的全部成员节点：外观与高缩放下普通节点一致（非高亮），便于点选。
    pub(super) fn draw_way_member_nodes_overlay(&self, ui: &egui::Ui, rect: egui::Rect, ppp: f32) {
        if !self.data_listed || !self.layer_visible || !self.layer_active {
            return;
        }
        if !self.way_member_nodes_force_zoom() {
            return;
        }
        let mut ways: Vec<ObjectId> = self.editor.selection().ways().iter().copied().collect();
        if let Some(way) = self.hover_way
            && !ways.contains(&way)
        {
            ways.push(way);
        }
        if ways.is_empty() {
            return;
        }
        let to_screen = |latlon: LatLon| -> egui::Pos2 {
            let screen = self.camera.world_to_screen(WorldPoint::from_latlon(latlon));
            rect.min + egui::vec2(screen[0] / ppp, screen[1] / ppp)
        };
        let painter = ui.painter();
        let fill = egui::Color32::from_rgba_unmultiplied(250, 250, 250, 255);
        let stroke = egui::Stroke::new(1.0, egui::Color32::from_rgb(60, 60, 60));
        let radius = 3.0;
        let mut seen = std::collections::HashSet::new();
        for way in ways {
            if self.hidden.contains(&(ObjectKind::Way, way)) {
                continue;
            }
            let Some(handle) = self.editor.data().way_handle(way) else {
                continue;
            };
            for node in self.editor.data().way_node_ids(handle) {
                if !seen.insert(node) {
                    continue;
                }
                if self.hidden.contains(&(ObjectKind::Node, node)) {
                    continue;
                }
                // 已选中的节点由红色高亮层绘制，这里只补「普通可见」。
                if self.editor.selection().contains(ObjectKind::Node, node) {
                    continue;
                }
                let Some(nh) = self.editor.data().node_handle(node) else {
                    continue;
                };
                let Some(pos) = self.editor.data().nodes.position(nh) else {
                    continue;
                };
                let screen = to_screen(pos);
                painter.circle_filled(screen, radius, fill);
                painter.circle_stroke(screen, radius, stroke);
            }
        }
    }

    /// 选中的点/线用红色。选中的关系用紫色标出它的一级成员，不展开子关系。
    pub(super) fn draw_selection_overlay(&self, ui: &egui::Ui, rect: egui::Rect, ppp: f32) {
        if !self.data_listed || !self.layer_visible {
            return;
        }
        let selection = self.editor.selection();
        if selection.is_empty() {
            return;
        }
        let red = egui::Color32::from_rgb(255, 60, 60);
        let purple = egui::Color32::from_rgb(176, 96, 240);
        let to_screen = |latlon: LatLon| -> egui::Pos2 {
            let screen = self.camera.world_to_screen(WorldPoint::from_latlon(latlon));
            rect.min + egui::vec2(screen[0] / ppp, screen[1] / ppp)
        };
        let painter = ui.painter();
        for relation in selection.relations() {
            let Some(handle) = self.editor.data().relation_handle(*relation) else {
                continue;
            };
            let Some(members) = self.editor.data().relations.members_of(handle) else {
                continue;
            };
            for member in members {
                match member.target.kind {
                    ObjectKind::Way => {
                        if let Some(way) = member
                            .target
                            .as_way()
                            .and_then(|handle| self.editor.data().ways.id(handle))
                        {
                            self.paint_way_highlight(
                                painter,
                                way,
                                egui::Stroke::new(2.5, purple),
                                &to_screen,
                            );
                        }
                    }
                    ObjectKind::Node => {
                        if let Some(node) = member
                            .target
                            .as_node()
                            .and_then(|handle| self.editor.data().nodes.id(handle))
                        {
                            self.paint_node_highlight(
                                painter,
                                node,
                                6.0,
                                egui::Stroke::new(2.0, purple),
                                &to_screen,
                            );
                        }
                    }
                    ObjectKind::Relation | ObjectKind::Changeset => {}
                }
            }
        }
        for way in selection.ways() {
            self.paint_way_highlight(painter, *way, egui::Stroke::new(2.5, red), &to_screen);
        }
        for node in selection.nodes() {
            self.paint_node_highlight(painter, *node, 6.0, egui::Stroke::new(2.0, red), &to_screen);
        }
    }

    fn paint_way_highlight(
        &self,
        painter: &egui::Painter,
        way: ObjectId,
        stroke: egui::Stroke,
        to_screen: &impl Fn(LatLon) -> egui::Pos2,
    ) {
        let Some(handle) = self.editor.data().way_handle(way) else {
            return;
        };
        let points: Vec<egui::Pos2> = self
            .editor
            .data()
            .way_node_ids(handle)
            .iter()
            .filter_map(|id| {
                let handle = self.editor.data().node_handle(*id)?;
                let pos = self.editor.data().nodes.position(handle)?;
                Some(to_screen(pos))
            })
            .collect();
        if points.len() >= 2 {
            painter.add(egui::Shape::line(points, stroke));
        }
    }

    fn paint_node_highlight(
        &self,
        painter: &egui::Painter,
        node: ObjectId,
        radius: f32,
        stroke: egui::Stroke,
        to_screen: &impl Fn(LatLon) -> egui::Pos2,
    ) {
        let Some(handle) = self.editor.data().node_handle(node) else {
            return;
        };
        let Some(pos) = self.editor.data().nodes.position(handle) else {
            return;
        };
        painter.circle_stroke(to_screen(pos), radius, stroke);
    }

    pub(super) fn draw_icons(&mut self, ui: &egui::Ui, rect: egui::Rect, ppp: f32) {
        let Some(scene) = &self.scene else {
            return;
        };
        let icons = scene.icons.clone();
        for icon in icons.into_iter().take(400) {
            let point = self.camera.world_to_screen(icon.pos);
            let pos = egui::pos2(rect.min.x + point[0] / ppp, rect.min.y + point[1] / ppp);
            let target = egui::Rect::from_center_size(pos, egui::vec2(icon.width_px, icon.width_px));
            if !rect.intersects(target) {
                continue;
            }
            if let Some(kind) = icon.image.strip_prefix("poi:") {
                paint_poi(
                    ui.painter(),
                    target,
                    kind,
                    egui::Color32::from_rgba_unmultiplied(
                        icon.color[0],
                        icon.color[1],
                        icon.color[2],
                        icon.color[3],
                    ),
                );
                continue;
            }
            if icon.image.contains("..")
                || icon.image.contains(':')
                || icon.image.starts_with('/')
                || icon.image.starts_with('\\')
            {
                continue;
            }
            let texture = self.icon_textures.entry(icon.image.clone()).or_insert_with(|| {
                let path = neo_style::assets::find_asset(&icon.image)?;
                let image = image::open(path).ok()?.to_rgba8();
                let size = [image.width() as usize, image.height() as usize];
                let color = egui::ColorImage::from_rgba_unmultiplied(size, image.as_raw());
                Some(ui.ctx().load_texture(
                    format!("neo-icon-{}", icon.image),
                    color,
                    egui::TextureOptions::LINEAR,
                ))
            });
            let Some(texture) = texture else {
                continue;
            };
            if rect.intersects(target) {
                ui.painter().image(
                    texture.id(),
                    target,
                    egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    egui::Color32::WHITE,
                );
            }
        }
    }

    pub(super) fn focus_object(&mut self, kind: ObjectKind, id: ObjectId) {
        let mut pos: Option<LatLon> = None;
        match kind {
            ObjectKind::Node => {
                if let Some(handle) = self.editor.data().node_handle(id)
                    && let Some(view) = self.editor.data().node_view(handle)
                {
                    pos = Some(view.pos());
                }
            }
            ObjectKind::Way => {
                if let Some(handle) = self.editor.data().way_handle(id)
                    && let Some(view) = self.editor.data().way_view(handle)
                    && let Some(node) = view.node_refs.first()
                    && let Some(node_view) = self.editor.data().node_view(*node)
                {
                    pos = Some(node_view.pos());
                }
            }
            ObjectKind::Relation => {
                if let Some(handle) = self.editor.data().relation_handle(id)
                    && let Some(view) = self.editor.data().relation_view(handle)
                {
                    for member in view.members {
                        if let Some(node) = member.target.as_node()
                            && let Some(node_view) = self.editor.data().node_view(node)
                        {
                            pos = Some(node_view.pos());
                            break;
                        }
                    }
                }
            }
            ObjectKind::Changeset => {}
        }
        if let Some(pos) = pos {
            self.camera.center = WorldPoint::from_latlon(pos);
            if self.camera.zoom < 16.0 {
                self.camera.set_zoom(17.0);
            }
            self.scene = None;
        }
    }

    /// 当前测距结果（米）
    pub(super) fn measure_distance(&self) -> Option<f64> {
        let start = self.measure_start?;
        let end = self.measure_end.or(self.hover_world)?;
        Some(start.dist(end))
    }

    /// 测距工具点击：起点 → 终点 → 重新开始。
    pub(super) fn measure_click(&mut self, world: WorldPoint) {
        if self.measure_start.is_none() {
            self.measure_start = Some(world);
            self.measure_end = None;
            self.status = t("app.status.measure_pick_end").into();
        } else if self.measure_end.is_none() {
            self.measure_end = Some(world);
            self.status = match self.measure_distance() {
                Some(meters) if meters >= 1000.0 => tf(
                    "app.status.measure_km",
                    &[("n", &format!("{:.3}", meters / 1000.0))],
                ),
                Some(meters) => tf("app.status.measure_m", &[("n", &format!("{:.1}", meters))]),
                None => t("app.status.measure_pick_start").into(),
            };
        } else {
            self.measure_start = Some(world);
            self.measure_end = None;
            self.status = t("app.status.measure_pick_end").into();
        }
    }
}

/// 按优先级画名称：先占位的留下，后来的避开已有名字和图标。过长的名字折成最多三行。
fn paint_scene_labels(
    painter: &egui::Painter,
    scene: &neo_scene::SceneFrame,
    camera: &neo_geo::Camera,
    viewport: egui::Rect,
    ppp: f32,
) {
    let mut occupied = Vec::with_capacity(scene.icons.len() + 32);
    for icon in &scene.icons {
        let pos = world_to_ui(camera, icon.pos, viewport, ppp);
        let target = egui::Rect::from_center_size(pos, egui::vec2(icon.width_px, icon.width_px));
        if viewport.intersects(target) {
            occupied.push(target.expand(2.0));
        }
    }
    let mut placed = 0u32;
    for label in &scene.labels {
        if placed >= 160 {
            break;
        }
        let pos = world_to_ui(camera, label.pos, viewport, ppp);
        if !viewport.expand(240.0).contains(pos) {
            continue;
        }
        let galley = layout_name(painter, &label.text, label.size_px, label.wrap_px);
        let (anchor, pivot, angle) = label_anchor(label.place, pos);
        let bounds = label_screen_rect(pivot, &galley, anchor, angle).expand(2.0);
        if !viewport.intersects(bounds) || occupied.iter().any(|rect| rect.intersects(bounds)) {
            continue;
        }
        paint_feature_name(painter, pivot, galley, label.color, anchor, angle);
        occupied.push(bounds);
        placed += 1;
    }
}

fn world_to_ui(
    camera: &neo_geo::Camera,
    world: neo_geo::WorldPoint,
    viewport: egui::Rect,
    ppp: f32,
) -> egui::Pos2 {
    let point = camera.world_to_screen(world);
    egui::pos2(viewport.min.x + point[0] / ppp, viewport.min.y + point[1] / ppp)
}

fn label_anchor(place: neo_scene::LabelPlace, pos: egui::Pos2) -> (egui::Align2, egui::Pos2, f32) {
    match place {
        neo_scene::LabelPlace::Center => (egui::Align2::CENTER_CENTER, pos, 0.0),
        neo_scene::LabelPlace::Along { angle } => (egui::Align2::CENTER_CENTER, pos, angle),
        neo_scene::LabelPlace::Right { gap_px } => {
            (egui::Align2::LEFT_CENTER, pos + egui::vec2(gap_px, 0.0), 0.0)
        }
    }
}

fn layout_name(
    painter: &egui::Painter,
    text: &str,
    size_px: f32,
    wrap_px: f32,
) -> std::sync::Arc<egui::Galley> {
    let font = egui::FontId::proportional(size_px);
    let wrap = wrap_px.max(72.0);
    let max_h = size_px * 3.3;
    let galley = painter.layout(text.to_owned(), font.clone(), egui::Color32::PLACEHOLDER, wrap);
    if galley.rect.height() <= max_h + 0.5 {
        return galley;
    }
    let chars: Vec<char> = text.chars().collect();
    let mut lo = 1usize;
    let mut hi = chars.len();
    let mut best = painter.layout("…".to_owned(), font.clone(), egui::Color32::PLACEHOLDER, wrap);
    while lo <= hi {
        let mid = (lo + hi) / 2;
        let mut shortened: String = chars[..mid].iter().collect();
        shortened.push('…');
        let candidate = painter.layout(shortened, font.clone(), egui::Color32::PLACEHOLDER, wrap);
        if candidate.rect.height() <= max_h {
            best = candidate;
            lo = mid + 1;
        } else if mid == 0 {
            break;
        } else {
            hi = mid - 1;
        }
    }
    best
}

fn label_screen_rect(
    pivot: egui::Pos2,
    galley: &egui::Galley,
    anchor: egui::Align2,
    angle: f32,
) -> egui::Rect {
    let local = anchor.pos_in_rect(&galley.rect);
    let rect = galley.rect.translate((pivot - local.to_vec2()).to_vec2());
    if angle.abs() < 1e-4 {
        return rect;
    }
    let rot = egui::emath::Rot2::from_angle(angle);
    let mut min = egui::pos2(f32::MAX, f32::MAX);
    let mut max = egui::pos2(f32::MIN, f32::MIN);
    for corner in [
        rect.left_top(),
        rect.right_top(),
        rect.right_bottom(),
        rect.left_bottom(),
    ] {
        let turned = pivot + rot * (corner - pivot);
        min.x = min.x.min(turned.x);
        min.y = min.y.min(turned.y);
        max.x = max.x.max(turned.x);
        max.y = max.y.max(turned.y);
    }
    egui::Rect::from_min_max(min, max)
}

/// 名称跟要素颜色，浅色字配深描边，深色字配浅描边。
fn paint_feature_name(
    painter: &egui::Painter,
    pivot: egui::Pos2,
    galley: std::sync::Arc<egui::Galley>,
    ink: [u8; 4],
    anchor: egui::Align2,
    angle: f32,
) {
    let halo = halo_color(ink);
    let ink = egui::Color32::from_rgba_unmultiplied(ink[0], ink[1], ink[2], ink[3]);
    const RING: [(f32, f32); 8] = [
        (-1.6, 0.0),
        (1.6, 0.0),
        (0.0, -1.6),
        (0.0, 1.6),
        (-1.15, -1.15),
        (1.15, -1.15),
        (-1.15, 1.15),
        (1.15, 1.15),
    ];
    for (dx, dy) in RING {
        painter.add(rotated_text(
            pivot + egui::vec2(dx, dy),
            galley.clone(),
            halo,
            anchor,
            angle,
        ));
    }
    painter.add(rotated_text(pivot, galley, ink, anchor, angle));
}

fn halo_color(ink: [u8; 4]) -> egui::Color32 {
    let luma = u32::from(ink[0]) * 30 + u32::from(ink[1]) * 59 + u32::from(ink[2]) * 11;
    if luma > 16_000 {
        egui::Color32::from_rgba_unmultiplied(16, 18, 22, 220)
    } else {
        egui::Color32::from_rgb(255, 255, 255)
    }
}

fn paint_poi(painter: &egui::Painter, rect: egui::Rect, kind: &str, color: egui::Color32) {
    let center = rect.center();
    let radius = rect.width() * 0.5;
    painter.circle_filled(center, radius, color);
    painter.circle_stroke(center, radius, egui::Stroke::new(1.0, egui::Color32::WHITE));
    let white = egui::Color32::WHITE;
    let mark = radius * 0.62;
    match kind {
        "hospital" | "pharmacy" => paint_cross(painter, center, mark, white),
        "fuel" => {
            let top = center + egui::vec2(0.0, -mark);
            let left = center + egui::vec2(-mark * 0.55, mark * 0.5);
            let right = center + egui::vec2(mark * 0.55, mark * 0.5);
            painter.add(egui::Shape::convex_polygon(
                vec![top, right, left],
                white,
                egui::Stroke::NONE,
            ));
        }
        "worship" => {
            let up = center + egui::vec2(0.0, -mark);
            let right = center + egui::vec2(mark, 0.0);
            let down = center + egui::vec2(0.0, mark);
            let left = center + egui::vec2(-mark, 0.0);
            painter.add(egui::Shape::convex_polygon(
                vec![up, right, down, left],
                white,
                egui::Stroke::NONE,
            ));
        }
        "tree" | "park" => {
            painter.circle_filled(center + egui::vec2(0.0, -radius * 0.12), radius * 0.38, white);
            painter.rect_filled(
                egui::Rect::from_center_size(
                    center + egui::vec2(0.0, radius * 0.38),
                    egui::vec2(radius * 0.16, radius * 0.36),
                ),
                0.0,
                white,
            );
        }
        "restaurant" => paint_fork(painter, center, mark, white),
        "cafe" => paint_glyph(painter, center, radius, "C", white),
        "bar" => paint_glyph(painter, center, radius, "B", white),
        "school" => paint_glyph(painter, center, radius, "S", white),
        "parking" => paint_glyph(painter, center, radius, "P", white),
        "bus" => paint_glyph(painter, center, radius, "B", white),
        "bank" => paint_glyph(painter, center, radius, "$", white),
        "shop" => paint_glyph(painter, center, radius, "S", white),
        "toilets" => paint_glyph(painter, center, radius, "W", white),
        "tourism" => paint_glyph(painter, center, radius, "i", white),
        _ => {}
    }
}

fn paint_glyph(painter: &egui::Painter, center: egui::Pos2, radius: f32, text: &str, color: egui::Color32) {
    painter.text(
        center,
        egui::Align2::CENTER_CENTER,
        text,
        egui::FontId::proportional(radius * 1.15),
        color,
    );
}

fn paint_cross(painter: &egui::Painter, center: egui::Pos2, arm: f32, color: egui::Color32) {
    let thick = arm * 0.38;
    painter.rect_filled(
        egui::Rect::from_center_size(center, egui::vec2(thick, arm * 2.0)),
        1.0,
        color,
    );
    painter.rect_filled(
        egui::Rect::from_center_size(center, egui::vec2(arm * 2.0, thick)),
        1.0,
        color,
    );
}

fn paint_fork(painter: &egui::Painter, center: egui::Pos2, mark: f32, color: egui::Color32) {
    let stroke = egui::Stroke::new(1.4, color);
    let top = center + egui::vec2(0.0, -mark);
    let bottom = center + egui::vec2(0.0, mark * 0.85);
    painter.line_segment([top, bottom], stroke);
    painter.line_segment([top, top + egui::vec2(-mark * 0.45, mark * 0.5)], stroke);
    painter.line_segment([top, top + egui::vec2(mark * 0.45, mark * 0.5)], stroke);
}

/// 把字的锚点钉在 `pivot` 上再顺时针旋转。`TextShape` 默认绕左上角转，这里先把锚点对齐。
fn rotated_text(
    pivot: egui::Pos2,
    galley: std::sync::Arc<egui::Galley>,
    color: egui::Color32,
    anchor: egui::Align2,
    angle: f32,
) -> egui::Shape {
    let local = anchor.pos_in_rect(&galley.rect);
    let mut shape = egui::epaint::TextShape::new(pivot - local.to_vec2(), galley, color);
    if angle != 0.0 {
        shape = shape.with_angle_and_anchor(angle, anchor);
    }
    egui::Shape::Text(shape)
}

/// 视口减去已下载 bbox 并集：框外半透明置灰。
fn draw_undownloaded_mask(
    painter: &egui::Painter,
    viewport: egui::Rect,
    camera: &neo_geo::Camera,
    ppp: f32,
    bboxes: &[neo_ui::BboxView],
) {
    let to_screen = |world: WorldPoint| -> egui::Pos2 {
        let point = camera.world_to_screen(world);
        egui::pos2(viewport.min.x + point[0] / ppp, viewport.min.y + point[1] / ppp)
    };
    let mut regions = vec![viewport];
    for bbox in bboxes {
        let a = to_screen(WorldPoint::from_latlon(LatLon::from_degrees(
            bbox.min_lat,
            bbox.min_lon,
        )));
        let b = to_screen(WorldPoint::from_latlon(LatLon::from_degrees(
            bbox.max_lat,
            bbox.max_lon,
        )));
        let hole = egui::Rect::from_two_pos(a, b).intersect(viewport);
        if hole.width() <= 0.5 || hole.height() <= 0.5 {
            continue;
        }
        regions = regions
            .into_iter()
            .flat_map(|region| subtract_rect(region, hole))
            .collect();
    }
    let tint = egui::Color32::from_rgba_unmultiplied(18, 22, 28, 140);
    for region in regions {
        if region.width() > 0.5 && region.height() > 0.5 {
            painter.rect_filled(region, 0.0, tint);
        }
    }
}

/// 节点是否为某条未闭合 way 的端点（用于续画高亮）。
fn is_open_way_endpoint(app: &NeoApp, node: ObjectId) -> bool {
    let Some(handle) = app.editor.data().node_handle(node) else {
        return false;
    };
    app.editor
        .referrers()
        .ways_of_node(handle)
        .iter()
        .any(|way_handle| {
            let nodes = app.editor.data().way_node_ids(*way_handle);
            if nodes.len() < 2 {
                return false;
            }
            let first = nodes[0];
            let last = nodes[nodes.len() - 1];
            first != last && (first == node || last == node)
        })
}

/// `outer` 减去 `hole` 后的矩形碎片（最多 4 块）。
fn subtract_rect(outer: egui::Rect, hole: egui::Rect) -> Vec<egui::Rect> {
    let clip = outer.intersect(hole);
    if clip.width() <= 0.0 || clip.height() <= 0.0 {
        return vec![outer];
    }
    let mut out = Vec::with_capacity(4);
    // 上
    if clip.min.y > outer.min.y {
        out.push(egui::Rect::from_min_max(
            outer.min,
            egui::pos2(outer.max.x, clip.min.y),
        ));
    }
    // 下
    if clip.max.y < outer.max.y {
        out.push(egui::Rect::from_min_max(
            egui::pos2(outer.min.x, clip.max.y),
            outer.max,
        ));
    }
    // 左（中间条）
    if clip.min.x > outer.min.x {
        out.push(egui::Rect::from_min_max(
            egui::pos2(outer.min.x, clip.min.y),
            egui::pos2(clip.min.x, clip.max.y),
        ));
    }
    // 右（中间条）
    if clip.max.x < outer.max.x {
        out.push(egui::Rect::from_min_max(
            egui::pos2(clip.max.x, clip.min.y),
            egui::pos2(outer.max.x, clip.max.y),
        ));
    }
    out
}
