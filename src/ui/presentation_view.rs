use crate::data::MonsterDatabase;
use crate::model::*;
use crate::model::combat_stats::CombatStatsCache;
use crate::presentation::{PresentationState, Visibility};
use crate::presentation::combat_sim::{self, run_combat, SimResult, build_combatants_from_encounter, build_combatants_from_party};
use crate::presentation::combat_tracker::{CombatTracker, CombatantId, MonsterInstanceId, STANDARD_CONDITIONS};
use crate::ui::encounters_view::SimSide;
use crate::presentation::dice;
use crate::presentation::fog;
use crate::render::presentation::render_dm_overlay;
use crate::render::recording::replay_commands;
use crate::render::themed::RenderOptions;
use crate::ui::canvas_common::{handle_pan_zoom, ViewState, COLOR_PLACEHOLDER_TEXT};
use crate::util::{ViewTransform, GRID_PX};

use crate::render::bg_cache::BackgroundRenderCache;

pub struct SingleCombatState {
    pub side_a: SimSide,
    pub side_b: SimSide,
    pub last_result: Option<SimResult>,
}

impl Default for SingleCombatState {
    fn default() -> Self {
        Self {
            side_a: SimSide::Party,
            side_b: SimSide::default(),
            last_result: None,
        }
    }
}

pub struct PresentationViewState {
    pub view: ViewState,
    pub render_cache: BackgroundRenderCache,
    /// Canvas size from the last frame, used by the sidebar for centering.
    pub canvas_size: egui::Vec2,
    pub single_combat: SingleCombatState,
    /// Currently selected room in the presentation view.
    pub selected_room: Option<String>,
    /// True while the DM is dragging the player viewport rectangle.
    dragging_player_viewport: bool,
    /// Index of the AoE marker currently selected.
    pub selected_aoe: Option<usize>,
    /// True while dragging the selected AoE marker.
    dragging_aoe: bool,
}

impl Default for PresentationViewState {
    fn default() -> Self {
        Self {
            view: ViewState::default(),
            render_cache: BackgroundRenderCache::default(),
            canvas_size: egui::Vec2::ZERO,
            single_combat: SingleCombatState::default(),
            selected_room: None,
            dragging_player_viewport: false,
            selected_aoe: None,
            dragging_aoe: false,
        }
    }
}

impl PresentationViewState {}

pub fn render_cache_hash(layout: &SpatialLayout, theme: &Theme) -> u64 {
    presentation_input_hash(layout, theme)
}

fn presentation_input_hash(
    layout: &SpatialLayout,
    theme: &Theme,
) -> u64 {
    use std::hash::{Hash, Hasher};
    use std::collections::hash_map::DefaultHasher;
    let mut h = DefaultHasher::new();
    layout.rooms.len().hash(&mut h);
    for rl in &layout.rooms {
        rl.room_id.hash(&mut h);
        rl.x.hash(&mut h);
        rl.y.hash(&mut h);
        rl.width.hash(&mut h);
        rl.height.hash(&mut h);
    }
    layout.corridors.len().hash(&mut h);
    for c in &layout.corridors {
        c.width.hash(&mut h);
        for wp in &c.waypoints {
            wp.x.hash(&mut h);
            wp.y.hash(&mut h);
        }
    }
    theme.wall_color.hash(&mut h);
    theme.floor_color.hash(&mut h);
    theme.bg_color.hash(&mut h);
    h.finish()
}

/// AoE marker controls in the sidebar.
fn aoe_sidebar(
    ui: &mut egui::Ui,
    presentation: &PresentationState,
    dungeon: &mut Dungeon,
) {
    use crate::presentation::aoe::{AoEMarker, AoEShape};

    // Shape type selector
    let shape_idx_id = egui::Id::new("aoe_shape_type");
    let size_id = egui::Id::new("aoe_size_ft");
    let width_id = egui::Id::new("aoe_width_ft");
    let mut shape_idx: usize = ui.ctx().memory(|m| m.data.get_temp(shape_idx_id).unwrap_or(0usize));
    let mut size_ft: i32 = ui.ctx().memory(|m| m.data.get_temp(size_id).unwrap_or(20i32));
    let mut width_ft: i32 = ui.ctx().memory(|m| m.data.get_temp(width_id).unwrap_or(5i32));

    let shape_labels = ["Circle", "Square", "Line"];
    ui.horizontal(|ui| {
        ui.label("Shape:");
        egui::ComboBox::from_id_salt("aoe_shape_type")
            .selected_text(shape_labels[shape_idx.min(2)])
            .width(70.0)
            .show_ui(ui, |ui| {
                for (i, label) in shape_labels.iter().enumerate() {
                    if ui.selectable_label(i == shape_idx, *label).clicked() {
                        shape_idx = i;
                    }
                }
            });
        if shape_idx == 2 {
            // Line: length + width
            ui.label("L:");
            crate::ui::canvas_common::num_input_i32(ui, &mut size_ft, 35.0);
            ui.label("W:");
            crate::ui::canvas_common::num_input_i32(ui, &mut width_ft, 30.0);
            ui.label("ft");
        } else {
            // Circle radius or square side
            let label = if shape_idx == 0 { "R:" } else { "Size:" };
            ui.label(label);
            crate::ui::canvas_common::num_input_i32(ui, &mut size_ft, 35.0);
            ui.label("ft");
        }
    });
    size_ft = size_ft.max(5);
    width_ft = width_ft.max(5);
    ui.ctx().memory_mut(|m| {
        m.data.insert_temp(shape_idx_id, shape_idx);
        m.data.insert_temp(size_id, size_ft);
        m.data.insert_temp(width_id, width_ft);
    });

    ui.horizontal(|ui| {
        if ui.button("Add").clicked() {
            let room = presentation.party_room.as_ref()
                .and_then(|rid| dungeon.layout.as_ref().and_then(|l| l.room_by_id(rid)));
            let fallback = dungeon.layout.as_ref().and_then(|l| l.rooms.first());
            if let Some(rl) = room.or(fallback) {
                let cx = rl.x as f32 + rl.width as f32 / 2.0;
                let cy = rl.y as f32 + rl.height as f32 / 2.0;
                let color_id = egui::Id::new("aoe_color");
                let color: [u8; 4] = ui.ctx().memory(|m| m.data.get_temp(color_id).unwrap_or([255, 60, 60, 100]));
                let grid = |ft: i32| ft as f32 / 5.0;
                let shape = match shape_idx {
                    0 => AoEShape::Circle { radius: grid(size_ft) },
                    1 => AoEShape::Square { size: grid(size_ft) },
                    _ => AoEShape::Line { length: grid(size_ft), width: grid(width_ft) },
                };
                dungeon.aoe_markers.push(AoEMarker::new(shape, cx, cy, color));
            }
        }
    });

    // Color picker
    ui.horizontal(|ui| {
        ui.label("Color:");
        let color_id = egui::Id::new("aoe_color");
        let mut color: [u8; 4] = ui.ctx().memory(|m| m.data.get_temp(color_id).unwrap_or([255, 60, 60, 100]));
        let mut c3 = [color[0] as f32 / 255.0, color[1] as f32 / 255.0, color[2] as f32 / 255.0];
        if ui.color_edit_button_rgb(&mut c3).changed() {
            color[0] = (c3[0] * 255.0) as u8;
            color[1] = (c3[1] * 255.0) as u8;
            color[2] = (c3[2] * 255.0) as u8;
        }
        ui.label("Alpha:");
        let mut a = color[3] as i32;
        if crate::ui::canvas_common::num_input_i32(ui, &mut a, 35.0) {
            color[3] = a.clamp(0, 255) as u8;
        }
        ui.ctx().memory_mut(|m| m.data.insert_temp(color_id, color));
    });

    // List existing markers
    if !dungeon.aoe_markers.is_empty() {
        let mut remove_idx = None;
        for (i, marker) in dungeon.aoe_markers.iter().enumerate() {
            ui.horizontal(|ui| {
                let c = egui::Color32::from_rgba_unmultiplied(
                    marker.color[0], marker.color[1], marker.color[2], 255,
                );
                ui.colored_label(c, format!("{}", marker.shape.label()));
                ui.label(format!("({:.0},{:.0})", marker.x, marker.y));
                if ui.small_button("X").clicked() {
                    remove_idx = Some(i);
                }
            });
        }
        if let Some(idx) = remove_idx {
            dungeon.aoe_markers.remove(idx);
        }
        if ui.small_button("Clear All").clicked() {
            dungeon.aoe_markers.clear();
        }
    }
}

/// Compute the advantage state for an attack given attacker/target hidden status.
/// Attacker hidden → advantage. Target hidden → disadvantage.
/// Both → cancel out to normal.
fn compute_attack_advantage(attacker_hidden: bool, target_hidden: bool) -> dice::AdvantageState {
    match (attacker_hidden, target_hidden) {
        (true, true) => dice::AdvantageState::Normal, // cancel out
        (true, false) => dice::AdvantageState::Advantage,
        (false, true) => dice::AdvantageState::Disadvantage,
        (false, false) => dice::AdvantageState::Normal,
    }
}

/// Reusable UI for attack target selection and rolling.
fn attack_target_ui(
    ui: &mut egui::Ui,
    attacks: &[crate::model::combat_stats::ParsedAttack],
    attacker_name: &str,
    attacker_hidden: bool,
    attacker_cid: &CombatantId,
    targets: &[(CombatantId, String, u8, bool)], // (id, name, ac, hidden)
    id_salt: &str,
    attack_actions: &mut Vec<(String, String, crate::model::combat_stats::ParsedAttack, u8, dice::AdvantageState, CombatantId)>,
) {
    if attacker_hidden {
        ui.colored_label(
            egui::Color32::from_rgb(100, 180, 255),
            "Hidden (advantage on attacks)",
        );
    }

    // Target selector (shared across all attacks for this attacker)
    let target_idx_id = egui::Id::new(format!("target_idx_{}", id_salt));
    let mut selected_idx: usize = ui.ctx().memory(|m| m.data.get_temp(target_idx_id).unwrap_or(0usize));
    if selected_idx >= targets.len() && !targets.is_empty() {
        selected_idx = 0;
    }

    if targets.is_empty() {
        ui.label("No targets");
        return;
    }

    let (_, ref target_name, target_ac, target_hidden) = targets[selected_idx];
    let adv = compute_attack_advantage(attacker_hidden, target_hidden);
    let adv_label = match adv {
        dice::AdvantageState::Advantage => " [ADV]",
        dice::AdvantageState::Disadvantage => " [DIS]",
        dice::AdvantageState::Normal => "",
    };

    ui.horizontal(|ui| {
        ui.label("Target:");
        let display = format!("{} (AC {}){}", target_name, target_ac,
            if target_hidden { " [hidden]" } else { "" });
        egui::ComboBox::from_id_salt(format!("target_combo_{}", id_salt))
            .selected_text(&display)
            .width(160.0)
            .show_ui(ui, |ui| {
                for (i, (_, name, ac, hidden)) in targets.iter().enumerate() {
                    let label = format!("{} (AC {}){}", name, ac,
                        if *hidden { " [hidden]" } else { "" });
                    if ui.selectable_label(i == selected_idx, &label).clicked() {
                        selected_idx = i;
                    }
                }
            });
    });
    ui.ctx().memory_mut(|m| m.data.insert_temp(target_idx_id, selected_idx));

    for atk in attacks {
        ui.horizontal(|ui| {
            let btn_text = format!("{} (+{}){}", atk.name, atk.to_hit, adv_label);
            if ui.button(&btn_text).clicked() {
                attack_actions.push((
                    attacker_name.to_string(),
                    format!("{} (AC {})", target_name, target_ac),
                    atk.clone(),
                    target_ac,
                    adv,
                    attacker_cid.clone(),
                ));
            }
        });
    }
}

/// Get the display tag and color for a creature's awareness state.
fn awareness_tag_color(
    c: &crate::presentation::awareness::CreatureAwareness,
    surprise_color: egui::Color32,
    hidden_color: egui::Color32,
    ok_color: egui::Color32,
) -> (&'static str, egui::Color32) {
    match (c.surprised, c.hidden) {
        (true, true) => (" SURPRISED+HIDDEN", surprise_color), // edge case: cancel out on initiative
        (true, false) => (" SURPRISED", surprise_color),
        (false, true) => (" HIDDEN", hidden_color),
        (false, false) => ("", ok_color),
    }
}

/// Find the corridor under a grid position, returning the connection_id.
fn corridor_at_grid(layout: &SpatialLayout, gx: i32, gy: i32) -> Option<String> {
    for corridor in &layout.corridors {
        let cw = corridor.width as i32;
        let half = cw / 2;
        for pair in corridor.waypoints.windows(2) {
            let min_gx = pair[0].x.min(pair[1].x) - half;
            let min_gy = pair[0].y.min(pair[1].y) - half;
            let max_gx = pair[0].x.max(pair[1].x) - half + cw;
            let max_gy = pair[0].y.max(pair[1].y) - half + cw;

            if gx >= min_gx && gx < max_gx && gy >= min_gy && gy < max_gy {
                return Some(corridor.connection_id.clone());
            }
        }
    }
    None
}

/// Find the room under a grid position, returning the room_id.
fn room_at_grid(layout: &SpatialLayout, gx: i32, gy: i32) -> Option<String> {
    for rl in &layout.rooms {
        if gx >= rl.x && gx < rl.x + rl.width as i32
            && gy >= rl.y && gy < rl.y + rl.height as i32
        {
            return Some(rl.room_id.clone());
        }
    }
    None
}

/// The DM's presentation canvas showing the full map with visibility overlay.
pub fn presentation_view(
    ui: &mut egui::Ui,
    dungeon: &mut Dungeon,
    presentation: &mut PresentationState,
    view_state: &mut PresentationViewState,
    player_view_state: &mut crate::ui::player_view::PlayerViewState,
) {
    let (response, painter) = ui.allocate_painter(
        ui.available_size(),
        egui::Sense::click_and_drag(),
    );
    let rect = response.rect;

    let bg = dungeon.theme.bg_color;
    painter.rect_filled(rect, 0.0, egui::Color32::from_rgba_unmultiplied(bg[0], bg[1], bg[2], bg[3]));

    handle_pan_zoom(&response, &mut view_state.view);
    view_state.canvas_size = rect.size();
    let transform = ViewTransform::new(view_state.view.offset, view_state.view.zoom, rect);

    let Some(layout) = &dungeon.layout else {
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            "Generate a layout first (Spatial tab).",
            egui::FontId::proportional(16.0),
            COLOR_PLACEHOLDER_TEXT,
        );
        return;
    };

    // Rebuild cached render commands for the full map (DM sees everything)
    let hash = presentation_input_hash(layout, &dungeon.theme);
    let options = RenderOptions {
        show_grid: true,
        show_labels: true,
        show_notes: true,
        show_secrets: true,
        show_decor: true,
    };
    let cache_ready = view_state.render_cache.ensure(
        hash, &dungeon.graph, layout, &dungeon.theme, options, "Presentation",
    );

    if cache_ready {
        if let Some(commands) = view_state.render_cache.commands() {
            replay_commands(&painter, &transform, commands);
        }
    } else {
        let msg = format!("Rendering {}...",
            view_state.render_cache.pending_label().unwrap_or("map"));
        let spinner_rect = egui::Rect::from_center_size(rect.center(), egui::vec2(200.0, 40.0));
        painter.rect_filled(spinner_rect, 8.0, egui::Color32::from_rgba_unmultiplied(0, 0, 0, 180));
        painter.text(
            spinner_rect.center(),
            egui::Align2::CENTER_CENTER,
            &msg,
            egui::FontId::proportional(14.0),
            egui::Color32::WHITE,
        );
        ui.ctx().request_repaint();
    }

    // Draw text overlay (labels/notes visible to DM)
    for rl in &layout.rooms {
        if let Some(room) = dungeon.graph.room_by_id(&rl.room_id) {
            let cx = (rl.x as f32 + rl.width as f32 / 2.0) * GRID_PX;
            let cy = (rl.y as f32 + rl.height as f32 / 2.0) * GRID_PX;
            let screen = transform.world_to_screen(egui::pos2(cx, cy));
            painter.text(
                screen,
                egui::Align2::CENTER_CENTER,
                &room.label,
                egui::FontId::monospace(10.0 * transform.zoom),
                egui::Color32::from_rgb(60, 60, 60),
            );
        }
    }

    // DM overlay (fog of war + door state indicators)
    render_dm_overlay(&painter, &transform, layout, dungeon, presentation);

    // AoE markers (visible on DM view, with center crosshairs)
    crate::presentation::aoe::render_aoe_markers(&painter, &transform, &dungeon.aoe_markers, true);

    // --- Player viewport rectangle ---
    // Compute the world-space rect the player currently sees from their view state.
    let pv_zoom = player_view_state.view.zoom;
    let pv_offset = player_view_state.view.offset;
    let pv_canvas = player_view_state.canvas_size;
    if pv_canvas.x > 0.0 && pv_canvas.y > 0.0 && pv_zoom > 0.0 {
        // Player view corners in world coords
        let pv_world_min_x = -pv_offset.x / pv_zoom;
        let pv_world_min_y = -pv_offset.y / pv_zoom;
        let pv_world_max_x = (pv_canvas.x - pv_offset.x) / pv_zoom;
        let pv_world_max_y = (pv_canvas.y - pv_offset.y) / pv_zoom;

        // Convert to DM screen coords
        let screen_min = transform.world_to_screen(egui::pos2(pv_world_min_x, pv_world_min_y));
        let screen_max = transform.world_to_screen(egui::pos2(pv_world_max_x, pv_world_max_y));
        let vp_rect = egui::Rect::from_min_max(screen_min, screen_max);

        // Draw the viewport rectangle
        painter.rect_stroke(
            vp_rect, 0.0,
            egui::Stroke::new(2.0, egui::Color32::from_rgb(50, 255, 50)),
            egui::StrokeKind::Outside,
        );

        // Drag handling: start drag when left-click lands anywhere inside the viewport rect
        if response.drag_started_by(egui::PointerButton::Primary) {
            if let Some(pos) = response.interact_pointer_pos() {
                if vp_rect.contains(pos) {
                    view_state.dragging_player_viewport = true;
                }
            }
        }

        if view_state.dragging_player_viewport && response.dragged_by(egui::PointerButton::Primary) {
            let delta_screen = response.drag_delta();
            // Convert screen delta to world delta
            let world_dx = delta_screen.x / transform.zoom;
            let world_dy = delta_screen.y / transform.zoom;
            // Shift the player view offset (world shift -> player screen shift)
            player_view_state.view.offset.x -= world_dx * pv_zoom;
            player_view_state.view.offset.y -= world_dy * pv_zoom;
        }

        if response.drag_stopped_by(egui::PointerButton::Primary) {
            view_state.dragging_player_viewport = false;
        }
    }

    // AoE: click to select, drag to move, Delete key to remove
    if response.drag_started_by(egui::PointerButton::Primary) && !view_state.dragging_player_viewport {
        if let Some(pos) = response.interact_pointer_pos() {
            if let Some(idx) = crate::presentation::aoe::marker_at_screen_pos(pos, &transform, &dungeon.aoe_markers) {
                view_state.selected_aoe = Some(idx);
                view_state.dragging_aoe = true;
            }
        }
    }
    if view_state.dragging_aoe {
        if response.dragged_by(egui::PointerButton::Primary) {
            if let Some(idx) = view_state.selected_aoe {
                let delta = response.drag_delta();
                let world_dx = delta.x / (transform.zoom * GRID_PX);
                let world_dy = delta.y / (transform.zoom * GRID_PX);
                if idx < dungeon.aoe_markers.len() {
                    dungeon.aoe_markers[idx].x += world_dx;
                    dungeon.aoe_markers[idx].y += world_dy;
                }
            }
        }
    }
    if response.drag_stopped() {
        view_state.dragging_aoe = false;
    }

    // Delete key removes selected AoE
    if view_state.selected_aoe.is_some() {
        if ui.input(|i| i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace)) {
            if let Some(idx) = view_state.selected_aoe.take() {
                if idx < dungeon.aoe_markers.len() {
                    dungeon.aoe_markers.remove(idx);
                }
            }
        }
    }

    // Validate selected_aoe index
    if let Some(idx) = view_state.selected_aoe {
        if idx >= dungeon.aoe_markers.len() {
            view_state.selected_aoe = None;
        }
    }

    // Left-click: select room or AoE, deselect on empty space
    if response.clicked() && !view_state.dragging_aoe {
        if let Some(pos) = response.interact_pointer_pos() {
            // Check AoE first
            if let Some(idx) = crate::presentation::aoe::marker_at_screen_pos(pos, &transform, &dungeon.aoe_markers) {
                view_state.selected_aoe = Some(idx);
            } else {
                view_state.selected_aoe = None;
                let world = transform.screen_to_world(pos);
                let gx = (world.x / GRID_PX).floor() as i32;
                let gy = (world.y / GRID_PX).floor() as i32;

                if let Some(room_id) = room_at_grid(layout, gx, gy) {
                    view_state.selected_room = Some(room_id);
                } else {
                    view_state.selected_room = None;
                }
            }
        }
    }

    // Draw selection highlight
    if let Some(ref sel_id) = view_state.selected_room {
        if let Some(rl) = layout.room_by_id(sel_id) {
            let min = transform.world_to_screen(egui::pos2(rl.x as f32 * GRID_PX, rl.y as f32 * GRID_PX));
            let max = transform.world_to_screen(egui::pos2(
                (rl.x as f32 + rl.width as f32) * GRID_PX,
                (rl.y as f32 + rl.height as f32) * GRID_PX,
            ));
            let sel_rect = egui::Rect::from_min_max(min, max);
            painter.rect_stroke(
                sel_rect, 0.0,
                egui::Stroke::new(2.0, egui::Color32::from_rgb(100, 200, 255)),
                egui::StrokeKind::Outside,
            );
        }
    }

    // Draw AoE selection highlight
    if let Some(idx) = view_state.selected_aoe {
        if let Some(marker) = dungeon.aoe_markers.get(idx) {
            let center = transform.world_to_screen(egui::pos2(marker.x * GRID_PX, marker.y * GRID_PX));
            let radius = match &marker.shape {
                crate::presentation::aoe::AoEShape::Circle { radius } => *radius,
                crate::presentation::aoe::AoEShape::Square { size } => *size / 2.0,
                crate::presentation::aoe::AoEShape::Line { length, .. } => *length / 2.0,
            } * GRID_PX * transform.zoom;
            painter.circle_stroke(
                center, radius + 3.0,
                egui::Stroke::new(2.0, egui::Color32::from_rgb(255, 255, 100)),
            );
        }
    }

    // Capture right-click position so the context menu doesn't change as the pointer moves
    let ctx_pos_id = egui::Id::new("pres_context_menu_pos");
    if response.secondary_clicked() {
        if let Some(pos) = response.interact_pointer_pos() {
            response.ctx.memory_mut(|m| m.data.insert_temp(ctx_pos_id, pos));
        }
    }

    // Right-click context menu
    response.context_menu(|ui| {
        if let Some(pos) = ui.ctx().memory(|m| m.data.get_temp::<egui::Pos2>(ctx_pos_id)) {
            let world = transform.screen_to_world(pos);
            let gx = (world.x / GRID_PX).floor() as i32;
            let gy = (world.y / GRID_PX).floor() as i32;

            // Check corridor first
            if let Some(conn_id) = corridor_at_grid(layout, gx, gy) {
                let edge = dungeon.graph.connections.iter()
                    .find(|e| e.connection.id == conn_id);
                let label = edge.map(|e| {
                    let src = dungeon.graph.room_by_id(&e.source_room_id)
                        .map(|r| r.label.as_str()).unwrap_or("?");
                    let tgt = dungeon.graph.room_by_id(&e.target_room_id)
                        .map(|r| r.label.as_str()).unwrap_or("?");
                    format!("{} <-> {}", src, tgt)
                }).unwrap_or_else(|| "Corridor".into());

                let is_open = presentation.is_door_open(&conn_id);
                ui.label(format!("{} ({})", label, if is_open { "Open" } else { "Closed" }));
                ui.separator();

                if ui.button(if is_open { "Close Door" } else { "Open Door" }).clicked() {
                    fog::toggle_door(&conn_id, presentation);
    
                    ui.close_menu();
                }
            } else if let Some(room_id) = room_at_grid(layout, gx, gy) {
                let label = dungeon.graph.room_by_id(&room_id)
                    .map(|r| r.label.as_str())
                    .unwrap_or("Room");
                let vis = presentation.room_visibility(&room_id);
                ui.label(format!("{} ({})", label, match vis {
                    Visibility::Hidden => "Hidden",
                    Visibility::Explored => "Explored",
                    Visibility::Visible => "Visible",
                }));
                ui.separator();

                if ui.button("Reveal").clicked() {
                    fog::reveal_room(&room_id, presentation);
    
                    ui.close_menu();
                }
                if ui.button("Explore").clicked() {
                    fog::explore_room(&room_id, presentation);
    
                    ui.close_menu();
                }
                if ui.button("Hide").clicked() {
                    fog::hide_room(&room_id, presentation);
    
                    ui.close_menu();
                }
                ui.separator();
                if ui.button("Open All Doors").clicked() {
                    fog::open_room_doors(&room_id, presentation, &dungeon.graph);
    
                    ui.close_menu();
                }
                if ui.button("Close All Doors").clicked() {
                    fog::close_room_doors(&room_id, presentation, &dungeon.graph);
    
                    ui.close_menu();
                }
                ui.separator();
                if ui.button("Reveal + Adjacent").clicked() {
                    fog::reveal_room_and_adjacent(&room_id, presentation, &dungeon.graph);
    
                    ui.close_menu();
                }
                ui.separator();
                if ui.button("Move Party Here").clicked() {
                    presentation.party_room = Some(room_id.clone());
                    ui.close_menu();
                }
            } else {
                ui.label("(no room or corridor here)");
            }
        }
    });
}

/// Compute the zoom level that makes one grid square appear as 1 physical
/// inch on a screen with the given diagonal size (in inches) and 16:9 aspect.
fn zoom_for_one_inch_square(ctx: &egui::Context, screen_diagonal_inches: f32) -> f32 {
    // Physical width of a 16:9 screen: diagonal * 16 / sqrt(16² + 9²)
    let physical_width_inches = screen_diagonal_inches * 16.0 / (337.0_f32).sqrt();
    // Screen width in egui points
    let screen_width_points = ctx.screen_rect().width();
    // Points per physical inch
    let points_per_inch = screen_width_points / physical_width_inches;
    // We want GRID_PX * zoom = points_per_inch
    points_per_inch / GRID_PX
}

/// Sidebar for the presentation view (DM controls).
/// After a tick, apply hazards and run FFA battles in rooms with multiple encounters.
fn run_autobattles(
    presentation: &mut PresentationState,
    dungeon: &Dungeon,
    monster_db: &MonsterDatabase,
    combat_stats_cache: &mut CombatStatsCache,
) {
    use rand::Rng;

    // Group living encounters by current room
    let mut rooms: std::collections::HashMap<String, Vec<usize>> = std::collections::HashMap::new();
    for (i, enc) in dungeon.encounters.iter().enumerate() {
        if presentation.defeated_encounters.contains(&enc.id) { continue; }
        let room_id = presentation.encounter_room(enc).to_string();
        rooms.entry(room_id).or_default().push(i);
    }

    for (_room_id, enc_indices) in &rooms {
        // Separate hazards from combatant encounters
        let hazard_indices: Vec<usize> = enc_indices.iter().copied()
            .filter(|&i| dungeon.encounters[i].is_hazard())
            .collect();
        let combat_indices: Vec<usize> = enc_indices.iter().copied()
            .filter(|&i| !dungeon.encounters[i].is_hazard())
            .collect();

        // Apply hazards to all non-hazard encounters in the room
        if !hazard_indices.is_empty() && !combat_indices.is_empty() {
            let mut rng = rand::thread_rng();
            for &haz_idx in &hazard_indices {
                let hazard = match &dungeon.encounters[haz_idx].hazard {
                    Some(h) => h.clone(),
                    None => continue,
                };
                for &enc_idx in &combat_indices {
                    let enc = &dungeon.encounters[enc_idx];
                    // Resolve each monster to get ability scores for saves
                    let mut total_kills = 0;
                    let mut total_monsters = 0u32;
                    for em in &enc.monsters {
                        let monster = crate::presentation::combat_tracker::resolve_monster(&em.monster_ref, monster_db, &dungeon.custom_monsters);
                        let Some(monster) = monster else { continue };
                        let ability_key = match hazard.save_ability {
                            crate::model::encounter::SaveAbility::Str => "str",
                            crate::model::encounter::SaveAbility::Dex => "dex",
                            crate::model::encounter::SaveAbility::Con => "con",
                            crate::model::encounter::SaveAbility::Int => "int",
                            crate::model::encounter::SaveAbility::Wis => "wis",
                            crate::model::encounter::SaveAbility::Cha => "cha",
                        };
                        // Use save proficiency if present, otherwise ability modifier
                        let save_mod: i32 = monster.save.get(ability_key)
                            .and_then(|s| s.trim_start_matches('+').parse::<i32>().ok())
                            .unwrap_or_else(|| {
                                let score = match hazard.save_ability {
                                    crate::model::encounter::SaveAbility::Str => monster.str_score,
                                    crate::model::encounter::SaveAbility::Dex => monster.dex_score,
                                    crate::model::encounter::SaveAbility::Con => monster.con_score,
                                    crate::model::encounter::SaveAbility::Int => monster.int_score,
                                    crate::model::encounter::SaveAbility::Wis => monster.wis_score,
                                    crate::model::encounter::SaveAbility::Cha => monster.cha_score,
                                };
                                (score as i32 - 10) / 2
                            });
                        let hp = combat_stats_cache.get_or_parse(monster).max_hp;
                        for _ in 0..em.count {
                            total_monsters += 1;
                            let saved = if let Some(dc) = hazard.save_dc {
                                let save_roll = rng.gen_range(1..=20) + save_mod as i32;
                                save_roll >= dc as i32
                            } else {
                                false
                            };
                            if !saved && !hazard.damage.is_empty() {
                                let dmg = crate::presentation::dice::roll_dice_expr(&hazard.damage);
                                if dmg >= hp {
                                    total_kills += 1;
                                }
                            }
                        }
                    }
                    if total_kills >= total_monsters as usize && total_monsters > 0 {
                        presentation.defeated_encounters.insert(enc.id.clone());
                    }
                }
            }
        }

        // Run FFA between surviving non-hazard encounters
        let live_combat: Vec<usize> = combat_indices.iter().copied()
            .filter(|i| !presentation.defeated_encounters.contains(&dungeon.encounters[*i].id))
            .collect();
        if live_combat.len() < 2 { continue; }

        let mut groups: Vec<Vec<combat_sim::SimCombatant>> = Vec::new();
        let mut side_to_enc: Vec<usize> = Vec::new();
        for (side, &enc_idx) in live_combat.iter().enumerate() {
            let enc = &dungeon.encounters[enc_idx];
            let combatants = combat_sim::build_combatants_from_encounter(
                enc, monster_db, &dungeon.custom_monsters, combat_stats_cache, side,
            );
            if !combatants.is_empty() {
                groups.push(combatants);
                side_to_enc.push(enc_idx);
            }
        }
        if groups.len() < 2 { continue; }

        let result = combat_sim::run_combat_ffa(&groups);

        // Mark wiped-out encounters as defeated
        for (side, &enc_idx) in side_to_enc.iter().enumerate() {
            let all_dead = result.combatants.iter()
                .filter(|c| c.side == side)
                .all(|c| c.current_hp <= 0);
            if all_dead {
                presentation.defeated_encounters.insert(dungeon.encounters[enc_idx].id.clone());
            }
        }
    }
}

/// Reusable party editing/display section.
fn party_section(
    ui: &mut egui::Ui,
    dungeon: &mut Dungeon,
    presentation: &mut PresentationState,
    in_combat: bool,
) {
    // Party room selector
    {
        let rooms_list: Vec<_> = dungeon.graph.rooms.iter()
            .map(|r| (r.id.clone(), r.label.clone()))
            .collect();
        let selected_label = presentation.party_room.as_ref()
            .and_then(|rid| dungeon.graph.room_by_id(rid))
            .map(|r| r.label.as_str())
            .unwrap_or("(none)");
        ui.horizontal(|ui| {
            ui.label("Room:");
            egui::ComboBox::from_id_salt("party_room_combo")
                .selected_text(selected_label)
                .width(120.0)
                .show_ui(ui, |ui| {
                    if ui.selectable_label(presentation.party_room.is_none(), "(none)").clicked() {
                        presentation.party_room = None;
                    }
                    for (rid, rlabel) in &rooms_list {
                        let selected = presentation.party_room.as_ref() == Some(rid);
                        if ui.selectable_label(selected, rlabel).clicked() {
                            presentation.party_room = Some(rid.clone());
                        }
                    }
                });
        });
    }

    if !in_combat {
        if ui.button("Add PC").clicked() {
            dungeon.party.push(PlayerCharacter::new("New Character".to_string()));
        }
    }

    let mut remove_pc_idx = None;
    for (i, pc) in dungeon.party.iter_mut().enumerate() {
        ui.push_id(format!("party_pc_{}", pc.id), |ui| {
            egui::CollapsingHeader::new(&pc.name)
                .id_salt(format!("pc_header_{}", pc.id))
                .default_open(false)
                .show(ui, |ui| {
                    if in_combat {
                        ui.label(format!("{} ({})", pc.name, pc.class));
                        ui.label(format!("AC {} | HP {}/{}", pc.ac, pc.current_hp, pc.max_hp));
                        ui.label(format!("Init mod: {:+} | PP: {}", pc.initiative_modifier, pc.passive_perception));
                    } else {
                        ui.horizontal(|ui| {
                            ui.label("Name:");
                            ui.text_edit_singleline(&mut pc.name);
                        });
                        ui.horizontal(|ui| {
                            ui.label("Class:");
                            ui.text_edit_singleline(&mut pc.class);
                        });
                        ui.horizontal(|ui| {
                            ui.label("AC:");
                            let mut ac_val = pc.ac as i32;
                            if crate::ui::canvas_common::num_input_i32(ui, &mut ac_val, 35.0) { pc.ac = ac_val as u8; }
                            ui.label("HP:");
                            crate::ui::canvas_common::num_input_i32(ui, &mut pc.max_hp, 40.0);
                        });
                        pc.current_hp = pc.current_hp.min(pc.max_hp);
                        ui.horizontal(|ui| {
                            ui.label("Current HP:");
                            crate::ui::canvas_common::num_input_i32(ui, &mut pc.current_hp, 40.0);
                        });
                        ui.horizontal(|ui| {
                            ui.label("Init mod:");
                            let mut init_mod = pc.initiative_modifier as i32;
                            crate::ui::canvas_common::num_input_i32(ui, &mut init_mod, 35.0);
                            pc.initiative_modifier = init_mod as i8;
                            ui.label("PP:");
                            let mut pp_val = pc.passive_perception as i32;
                            if crate::ui::canvas_common::num_input_i32(ui, &mut pp_val, 35.0) { pc.passive_perception = pp_val as u8; }
                        });
                        ui.horizontal(|ui| {
                            ui.label("Atk:");
                            ui.label("+");
                            let mut atk_val = pc.attack_bonus as i32;
                            if crate::ui::canvas_common::num_input_i32(ui, &mut atk_val, 35.0) { pc.attack_bonus = atk_val as i8; }
                            ui.label("Dmg:");
                            ui.add(egui::TextEdit::singleline(&mut pc.damage_dice).desired_width(80.0));
                        });
                        ui.horizontal(|ui| {
                            ui.label("Stealth:");
                            let mut stealth_val = pc.stealth_modifier as i32;
                            crate::ui::canvas_common::num_input_i32(ui, &mut stealth_val, 35.0);
                            pc.stealth_modifier = stealth_val as i8;
                        });
                        ui.horizontal(|ui| {
                            ui.label("Roll:");
                            let mut has_override = pc.stealth_override.is_some();
                            if ui.checkbox(&mut has_override, "").changed() {
                                if has_override {
                                    pc.stealth_override = Some(10);
                                } else {
                                    pc.stealth_override = None;
                                }
                            }
                            if let Some(ref mut val) = pc.stealth_override {
                                let mut v = *val;
                                crate::ui::canvas_common::num_input_i32(ui, &mut v, 35.0);
                                *val = v;
                            } else {
                                ui.label("(auto)");
                            }
                        });
                        ui.horizontal(|ui| {
                            ui.checkbox(&mut pc.senses.darkvision, "DV");
                            ui.checkbox(&mut pc.senses.blindsight, "BS");
                            ui.checkbox(&mut pc.senses.tremorsense, "TS");
                        });
                        if ui.small_button("Remove").clicked() {
                            remove_pc_idx = Some(i);
                        }
                    }
                });
        });
    }
    if let Some(idx) = remove_pc_idx {
        dungeon.party.remove(idx);
    }
}

/// Renders the combat tracker UI into the given `ui`.
/// Used both inline in the sidebar and in the pop-out window.
fn combat_tracker_ui(
    ui: &mut egui::Ui,
    tracker: &mut CombatTracker,
    dungeon: &Dungeon,
) {
    // Round & turn controls
    ui.horizontal(|ui| {
        ui.label(format!("Round {}", tracker.round));
        if ui.small_button("<").on_hover_text("Previous turn").clicked() {
            tracker.prev_turn();
        }
        if ui.small_button(">").on_hover_text("Next turn").clicked() {
            tracker.next_turn();
        }
    });

    // Collect deferred actions
    let mut damage_actions: Vec<(CombatantId, i32)> = Vec::new();
    let mut heal_actions: Vec<(CombatantId, i32)> = Vec::new();
    let mut condition_toggles: Vec<(CombatantId, usize)> = Vec::new();
    let mut attack_actions: Vec<(String, String, crate::model::combat_stats::ParsedAttack, u8, dice::AdvantageState, CombatantId)> = Vec::new();
    let mut hidden_toggles: Vec<CombatantId> = Vec::new();

    // Build target list for attack dropdowns (all living combatants)
    let all_targets: Vec<(CombatantId, String, u8, bool)> = tracker.attack_targets();

    // Current turn info card
    if let Some(current_id) = tracker.current_combatant_id().cloned() {
        let name = tracker.get_combatant_name(&current_id).to_string();
        let init = match &current_id {
            CombatantId::Monster(mid) => tracker.instances.get(mid).and_then(|i| i.initiative).unwrap_or(0),
            CombatantId::Player(pid) => tracker.players.get(pid).and_then(|p| p.initiative).unwrap_or(0),
        };
        ui.colored_label(
            egui::Color32::from_rgb(100, 255, 100),
            format!("Turn: {} (Init {})", name, init),
        );

        // Active combatant's controls
        ui.group(|ui| {
            match &current_id {
                CombatantId::Monster(mid) => {
                    if let Some(inst) = tracker.instances.get(&mid) {
                        let hp_frac = if inst.max_hp > 0 { inst.current_hp as f32 / inst.max_hp as f32 } else { 0.0 };
                        let bar_color = if hp_frac > 0.5 {
                            egui::Color32::from_rgb(80, 200, 80)
                        } else if hp_frac > 0.25 {
                            egui::Color32::from_rgb(220, 200, 50)
                        } else {
                            egui::Color32::from_rgb(220, 60, 60)
                        };
                        ui.label(format!("AC {} | {}/{} HP", inst.ac, inst.current_hp, inst.max_hp));
                        ui.add(egui::ProgressBar::new(hp_frac).fill(bar_color).desired_width(ui.available_width()));

                        // Conditions
                        ui.horizontal_wrapped(|ui| {
                            for (c_idx, &cond_name) in STANDARD_CONDITIONS.iter().enumerate() {
                                let active = inst.conditions.get(c_idx).copied().unwrap_or(false);
                                if active {
                                    ui.colored_label(egui::Color32::from_rgb(255, 160, 40), cond_name);
                                }
                            }
                        });

                        // Attacks
                        if !inst.attacks.is_empty() {
                            let attacks_snapshot: Vec<_> = inst.attacks.clone();
                            let attacker_name = inst.label.clone();
                            let attacker_hidden = inst.hidden;
                            let attacker_cid = current_id.clone();
                            if attacker_hidden {
                                ui.colored_label(
                                    egui::Color32::from_rgb(100, 180, 255),
                                    "Hidden (advantage on attacks)",
                                );
                            }
                            let targets: Vec<_> = all_targets.iter()
                                .filter(|(cid, _, _, _)| *cid != attacker_cid)
                                .cloned().collect();
                            egui::CollapsingHeader::new("Actions")
                                .id_salt("active_turn_attacks")
                                .default_open(true)
                                .show(ui, |ui| {
                                    attack_target_ui(ui, &attacks_snapshot, &attacker_name, attacker_hidden, &attacker_cid, &targets, "turn", &mut attack_actions);
                                });
                        }

                        // Stat block pop-out button
                        if ui.small_button("Stat Block").clicked() {
                            ui.ctx().memory_mut(|mem| {
                                mem.data.insert_temp(egui::Id::new("combat_statblock_mid"), mid.clone());
                            });
                        }
                    }
                }
                CombatantId::Player(pid) => {
                    if let Some(pc) = tracker.players.get(pid) {
                        let hp_frac = if pc.max_hp > 0 { pc.current_hp as f32 / pc.max_hp as f32 } else { 0.0 };
                        let bar_color = if hp_frac > 0.5 {
                            egui::Color32::from_rgb(80, 200, 80)
                        } else if hp_frac > 0.25 {
                            egui::Color32::from_rgb(220, 200, 50)
                        } else {
                            egui::Color32::from_rgb(220, 60, 60)
                        };
                        ui.label(format!("AC {} | {}/{} HP", pc.ac, pc.current_hp, pc.max_hp));
                        ui.add(egui::ProgressBar::new(hp_frac).fill(bar_color).desired_width(ui.available_width()));
                    }
                    if let Some(pc) = dungeon.party.iter().find(|p| p.id == *pid) {
                        ui.label(format!("{} | Atk: +{} | Dmg: {}", pc.class, pc.attack_bonus, pc.damage_dice));
                    }
                }
            }
        });
    }

    // Initiative controls
    ui.horizontal(|ui| {
        if ui.button("Roll Initiative").clicked() {
            tracker.roll_all_initiative();
        }
        if ui.button("Sort").on_hover_text("Sort by initiative").clicked() {
            tracker.sort_initiative();
        }
    });

    // Simulate combat button
    if ui.button("Simulate Combat").on_hover_text("Run combat to completion, updating all HP").clicked() {
        let mut side_monsters: Vec<combat_sim::SimCombatant> = Vec::new();
        let mut monster_ids: Vec<MonsterInstanceId> = Vec::new();
        for (mid, inst) in &tracker.instances {
            if inst.current_hp <= 0 { continue; }
            side_monsters.push(combat_sim::SimCombatant {
                name: inst.label.clone(),
                max_hp: inst.max_hp,
                current_hp: inst.current_hp,
                ac: inst.ac,
                initiative_mod: inst.dex_mod,
                attacks: inst.attacks.clone(),
                multiattack_count: 1,
                side: 0,
            });
            monster_ids.push(mid.clone());
        }

        let mut side_players: Vec<combat_sim::SimCombatant> = Vec::new();
        let mut player_ids: Vec<String> = Vec::new();
        for (pid, pc) in &tracker.players {
            if pc.current_hp <= 0 { continue; }
            let party_pc = dungeon.party.iter().find(|p| p.id == *pid);
            let (atk_bonus, dmg_dice) = party_pc
                .map(|p| (p.attack_bonus, p.damage_dice.clone()))
                .unwrap_or((5, "1d8 + 3".to_string()));
            let attack = crate::model::combat_stats::ParsedAttack {
                name: format!("{}'s Attack", pc.name),
                attack_type: "mw".to_string(),
                to_hit: atk_bonus,
                reach: Some(5),
                range: None,
                damage_dice: dmg_dice.clone(),
                damage_avg: combat_sim::estimate_dice_avg_pub(&dmg_dice),
                damage_type: "weapon".to_string(),
                extra_damage: Vec::new(),
                effect: String::new(),
            };
            side_players.push(combat_sim::SimCombatant {
                name: pc.name.clone(),
                max_hp: pc.max_hp,
                current_hp: pc.current_hp,
                ac: pc.ac,
                initiative_mod: pc.initiative_modifier,
                attacks: vec![attack],
                multiattack_count: 1,
                side: 1,
            });
            player_ids.push(pid.clone());
        }

        if !side_monsters.is_empty() && !side_players.is_empty() {
            let result = combat_sim::run_combat(&side_monsters, &side_players);

            for (i, mid) in monster_ids.iter().enumerate() {
                if let Some(inst) = tracker.instances.get_mut(mid) {
                    if let Some(final_state) = result.combatants.get(i) {
                        inst.current_hp = final_state.current_hp.max(0);
                        inst.is_dead = inst.current_hp <= 0;
                    }
                }
            }

            for (i, pid) in player_ids.iter().enumerate() {
                if let Some(pc) = tracker.players.get_mut(pid) {
                    let combatant_idx = side_monsters.len() + i;
                    if let Some(final_state) = result.combatants.get(combatant_idx) {
                        pc.current_hp = final_state.current_hp.max(0);
                    }
                }
            }

            let winner_text = match result.winner {
                Some(0) => "Monsters win",
                Some(1) => "Party wins",
                _ => "Draw",
            };
            tracker.log.log(
                format!("-- Simulation complete: {} in {} rounds --", winner_text, result.rounds),
                [255, 215, 0],
            );
            for c in &result.combatants {
                let status = if c.current_hp <= 0 { "dead" } else { "alive" };
                tracker.log.log(
                    format!("  {} ({}/{}) {}", c.name, c.current_hp.max(0), c.max_hp, status),
                    if c.current_hp <= 0 { [255, 100, 100] } else { [100, 255, 100] },
                );
            }
        }
    }

    ui.separator();

    // Per-encounter collapsible sections
    let active_enc_ids: std::collections::HashSet<&str> = tracker.instances.keys()
        .map(|mid| mid.encounter_id.as_str())
        .collect();
    let encounter_ids: Vec<_> = dungeon.encounters.iter()
        .filter(|e| active_enc_ids.contains(e.id.as_str()))
        .map(|e| (e.id.clone(), e.name.clone()))
        .collect();

    let current_turn_id = tracker.current_combatant_id().cloned();

    egui::ScrollArea::vertical().max_height(ui.available_height()).id_salt("combat_scroll").show(ui, |ui| {
        // Party section
        if !tracker.players.is_empty() {
            egui::CollapsingHeader::new("Party")
                .id_salt("combat_party")
                .default_open(true)
                .show(ui, |ui| {
                    let player_ids: Vec<String> = tracker.players.keys().cloned().collect();
                    for pid in &player_ids {
                        let Some(pc) = tracker.players.get_mut(pid) else { continue };
                        let combatant_id = CombatantId::Player(pid.clone());
                        let is_current = current_turn_id.as_ref() == Some(&combatant_id);

                        ui.push_id(format!("pc_{}", pid), |ui| {
                            let frame_color = if is_current {
                                egui::Color32::from_rgba_unmultiplied(100, 200, 255, 30)
                            } else {
                                egui::Color32::TRANSPARENT
                            };

                            egui::Frame::NONE.fill(frame_color).show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    ui.label(egui::RichText::new(&pc.name).strong());
                                    ui.label(format!("AC {}", pc.ac));
                                    ui.label("Init:");
                                    let mut init_val = pc.initiative.unwrap_or(0);
                                    if crate::ui::canvas_common::num_input_i32(ui, &mut init_val, 35.0) {
                                        pc.initiative = Some(init_val);
                                    }
                                });

                                // HP bar
                                let hp_frac = if pc.max_hp > 0 {
                                    pc.current_hp as f32 / pc.max_hp as f32
                                } else {
                                    0.0
                                };
                                let bar_color = if hp_frac > 0.5 {
                                    egui::Color32::from_rgb(80, 200, 80)
                                } else if hp_frac > 0.25 {
                                    egui::Color32::from_rgb(220, 200, 50)
                                } else {
                                    egui::Color32::from_rgb(220, 60, 60)
                                };
                                ui.label(format!("{}/{} HP", pc.current_hp, pc.max_hp));
                                let bar = egui::ProgressBar::new(hp_frac)
                                    .fill(bar_color)
                                    .desired_width(ui.available_width());
                                ui.add(bar);

                                // Damage/Heal
                                let dmg_id = egui::Id::new(format!("dmg_pc_{}", pid));
                                let mut dmg_val: i32 = ui.ctx().memory(|m| m.data.get_temp(dmg_id).unwrap_or(0));
                                ui.horizontal(|ui| {
                                    ui.label("HP:"); crate::ui::canvas_common::num_input_i32(ui, &mut dmg_val, 40.0);
                                    if ui.small_button("Dmg").clicked() && dmg_val > 0 {
                                        damage_actions.push((combatant_id.clone(), dmg_val));
                                    }
                                    if ui.small_button("Heal").clicked() && dmg_val > 0 {
                                        heal_actions.push((combatant_id.clone(), dmg_val));
                                    }
                                });
                                ui.ctx().memory_mut(|m| m.data.insert_temp(dmg_id, dmg_val));

                                // Conditions
                                ui.horizontal_wrapped(|ui| {
                                    for (c_idx, &cond_name) in STANDARD_CONDITIONS.iter().enumerate() {
                                        let active = pc.conditions.get(c_idx).copied().unwrap_or(false);
                                        let abbrev = &cond_name[..3.min(cond_name.len())];
                                        let color = if active {
                                            egui::Color32::from_rgb(255, 160, 40)
                                        } else {
                                            egui::Color32::from_rgb(120, 120, 120)
                                        };
                                        if ui.add(egui::Button::new(
                                            egui::RichText::new(abbrev).size(9.0).color(color)
                                        ).min_size(egui::vec2(0.0, 16.0))).on_hover_text(cond_name).clicked() {
                                            condition_toggles.push((combatant_id.clone(), c_idx));
                                        }
                                    }
                                    // Hidden toggle
                                    let hid_color = if pc.hidden {
                                        egui::Color32::from_rgb(100, 200, 255)
                                    } else {
                                        egui::Color32::from_rgb(120, 120, 120)
                                    };
                                    if ui.add(egui::Button::new(
                                        egui::RichText::new("Hid").size(9.0).color(hid_color)
                                    ).min_size(egui::vec2(0.0, 16.0))).on_hover_text("Hidden").clicked() {
                                        hidden_toggles.push(combatant_id.clone());
                                    }
                                });
                            });
                        });
                        ui.add_space(2.0);
                    }
                });
        }

        // Per-encounter monster sections
        for (enc_id, enc_name) in &encounter_ids {
            let (alive, dead) = tracker.counts_for_encounter(enc_id);
            let header = format!("{} ({} alive, {} dead)", enc_name, alive, dead);

            egui::CollapsingHeader::new(header)
                .id_salt(format!("combat_{}", enc_id))
                .default_open(true)
                .show(ui, |ui| {
                    let order: Vec<MonsterInstanceId> = if tracker.initiative_order.is_empty() {
                        tracker.instances.keys()
                            .filter(|id| id.encounter_id == *enc_id)
                            .cloned()
                            .collect()
                    } else {
                        tracker.initiative_order.iter()
                            .filter_map(|cid| {
                                if let CombatantId::Monster(mid) = cid {
                                    if mid.encounter_id == *enc_id {
                                        return Some(mid.clone());
                                    }
                                }
                                None
                            })
                            .collect()
                    };

                    for inst_id in order {
                        let Some(inst) = tracker.instances.get_mut(&inst_id) else { continue };
                        let combatant_id = CombatantId::Monster(inst_id.clone());
                        let is_current = current_turn_id.as_ref() == Some(&combatant_id);

                        ui.push_id(format!("inst_{}_{}_{}", inst_id.encounter_id, inst_id.monster_index, inst_id.instance), |ui| {
                            let frame_color = if is_current {
                                egui::Color32::from_rgba_unmultiplied(100, 255, 100, 30)
                            } else if inst.is_dead {
                                egui::Color32::from_rgba_unmultiplied(100, 100, 100, 20)
                            } else {
                                egui::Color32::TRANSPARENT
                            };

                            egui::Frame::NONE.fill(frame_color).show(ui, |ui| {
                                // Name + AC + initiative
                                ui.horizontal(|ui| {
                                    if inst.is_dead {
                                        ui.colored_label(egui::Color32::from_rgb(150, 150, 150), &inst.label);
                                    } else {
                                        ui.label(&inst.label);
                                    }
                                    ui.label(format!("AC {}", inst.ac));
                                    ui.label("Init:");
                                    let mut init_val = inst.initiative.unwrap_or(0);
                                    if crate::ui::canvas_common::num_input_i32(ui, &mut init_val, 35.0) {
                                        inst.initiative = Some(init_val);
                                    }
                                });

                                // HP bar
                                let hp_frac = if inst.max_hp > 0 {
                                    inst.current_hp as f32 / inst.max_hp as f32
                                } else {
                                    0.0
                                };
                                let bar_color = if hp_frac > 0.5 {
                                    egui::Color32::from_rgb(80, 200, 80)
                                } else if hp_frac > 0.25 {
                                    egui::Color32::from_rgb(220, 200, 50)
                                } else {
                                    egui::Color32::from_rgb(220, 60, 60)
                                };

                                ui.horizontal(|ui| {
                                    let hp_text = format!("{}/{}", inst.current_hp, inst.max_hp);
                                    if inst.temp_hp > 0 {
                                        ui.label(format!("{} (+{} temp)", hp_text, inst.temp_hp));
                                    } else {
                                        ui.label(&hp_text);
                                    }
                                });

                                let bar = egui::ProgressBar::new(hp_frac)
                                    .fill(bar_color)
                                    .desired_width(ui.available_width());
                                ui.add(bar);

                                // Damage/Heal controls
                                let dmg_id = egui::Id::new(format!("dmg_{}_{}_{}", inst_id.encounter_id, inst_id.monster_index, inst_id.instance));
                                let mut dmg_val: i32 = ui.ctx().memory(|m| m.data.get_temp(dmg_id).unwrap_or(0));

                                ui.horizontal(|ui| {
                                    ui.label("HP:"); crate::ui::canvas_common::num_input_i32(ui, &mut dmg_val, 40.0);
                                    if ui.small_button("Dmg").clicked() && dmg_val > 0 {
                                        damage_actions.push((combatant_id.clone(), dmg_val));
                                    }
                                    if ui.small_button("Heal").clicked() && dmg_val > 0 {
                                        heal_actions.push((combatant_id.clone(), dmg_val));
                                    }
                                });
                                ui.ctx().memory_mut(|m| m.data.insert_temp(dmg_id, dmg_val));

                                // Conditions (compact toggles)
                                ui.horizontal_wrapped(|ui| {
                                    for (c_idx, &cond_name) in STANDARD_CONDITIONS.iter().enumerate() {
                                        let active = inst.conditions.get(c_idx).copied().unwrap_or(false);
                                        let abbrev = &cond_name[..3.min(cond_name.len())];
                                        let color = if active {
                                            egui::Color32::from_rgb(255, 160, 40)
                                        } else {
                                            egui::Color32::from_rgb(120, 120, 120)
                                        };
                                        if ui.add(egui::Button::new(
                                            egui::RichText::new(abbrev).size(9.0).color(color)
                                        ).min_size(egui::vec2(0.0, 16.0))).on_hover_text(cond_name).clicked() {
                                            condition_toggles.push((combatant_id.clone(), c_idx));
                                        }
                                    }
                                    // Hidden toggle
                                    let hid_color = if inst.hidden {
                                        egui::Color32::from_rgb(100, 200, 255)
                                    } else {
                                        egui::Color32::from_rgb(120, 120, 120)
                                    };
                                    if ui.add(egui::Button::new(
                                        egui::RichText::new("Hid").size(9.0).color(hid_color)
                                    ).min_size(egui::vec2(0.0, 16.0))).on_hover_text("Hidden").clicked() {
                                        hidden_toggles.push(combatant_id.clone());
                                    }
                                });

                                // Attacks section
                                if !inst.attacks.is_empty() {
                                    let attacks_snapshot: Vec<_> = inst.attacks.clone();
                                    let attacker_name = inst.label.clone();
                                    let attacker_hidden = inst.hidden;
                                    let attacker_cid = combatant_id.clone();
                                    let targets: Vec<_> = all_targets.iter()
                                        .filter(|(cid, _, _, _)| *cid != attacker_cid)
                                        .cloned().collect();
                                    let id_salt = format!("attacks_{}_{}_{}", inst_id.encounter_id, inst_id.monster_index, inst_id.instance);
                                    egui::CollapsingHeader::new("Attacks")
                                        .id_salt(&id_salt)
                                        .default_open(false)
                                        .show(ui, |ui| {
                                            attack_target_ui(ui, &attacks_snapshot, &attacker_name, attacker_hidden, &attacker_cid, &targets, &id_salt, &mut attack_actions);
                                        });
                                }
                            });
                        });

                        ui.add_space(2.0);
                    }
                });
        }
    });

    // Apply deferred actions
    for (id, dmg) in damage_actions {
        tracker.apply_damage_to(&id, dmg);
    }
    for (id, amt) in heal_actions {
        tracker.heal_combatant(&id, amt);
    }
    for (id, c_idx) in condition_toggles {
        tracker.toggle_combatant_condition(&id, c_idx);
    }
    for id in hidden_toggles {
        tracker.toggle_hidden(&id);
    }
    // Process attack rolls
    for (attacker_name, target_desc, attack, target_ac, advantage, attacker_cid) in attack_actions {
        let result = dice::roll_attack_with_advantage(&attack, target_ac, advantage);
        tracker.log.log_attack(&attacker_name, &target_desc, &attack.name, &result, Some(&attack));
        match &attacker_cid {
            CombatantId::Monster(mid) => {
                if let Some(inst) = tracker.instances.get_mut(mid) {
                    inst.hidden = false;
                }
            }
            CombatantId::Player(pid) => {
                if let Some(pc) = tracker.players.get_mut(pid) {
                    pc.hidden = false;
                }
            }
        }
    }
}

/// Renders the combat tracker as a floating egui::Window.
/// Called from app.rs when combat is active and the window is popped out.
pub fn combat_tracker_window(
    ctx: &egui::Context,
    presentation: &mut PresentationState,
    dungeon: &Dungeon,
    combat_window_open: &mut bool,
) {
    if let Some(tracker) = &mut presentation.combat_tracker {
        let mut open = true;
        egui::Window::new("Combat Tracker")
            .id(egui::Id::new("combat_tracker_window"))
            .open(&mut open)
            .default_size([400.0, 600.0])
            .resizable(true)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    if ui.button("End Combat").clicked() {
                        ui.ctx().memory_mut(|mem| {
                            mem.data.insert_temp(egui::Id::new("_end_combat_flag"), true);
                        });
                    }
                    if ui.small_button("Dock").clicked() {
                        *combat_window_open = false;
                    }
                });
                ui.separator();
                egui::ScrollArea::vertical().show(ui, |ui| {
                    combat_tracker_ui(ui, tracker, dungeon);
                });
            });
        if !open {
            // Window closed via X button — dock it back to sidebar
            *combat_window_open = false;
        }
        // Check for end combat flag (set inside the window closure)
        let end_combat: bool = ctx.memory(|mem| mem.data.get_temp(egui::Id::new("_end_combat_flag")).unwrap_or(false));
        if end_combat {
            ctx.memory_mut(|mem| mem.data.remove::<bool>(egui::Id::new("_end_combat_flag")));
            presentation.combat_tracker = None;
        }
    }
}

pub fn presentation_sidebar(
    ui: &mut egui::Ui,
    dungeon: &mut Dungeon,
    presentation: &mut PresentationState,
    view_state: &mut PresentationViewState,
    player_view_state: &mut crate::ui::player_view::PlayerViewState,
    player_viewport_open: &mut bool,
    combat_window_open: &mut bool,
    _server_action: &mut ServerAction,
    monster_db: &MonsterDatabase,
    combat_stats_cache: &mut CombatStatsCache,
) {
    ui.heading("Presentation Mode");
    ui.separator();

    // Quick actions
    ui.horizontal(|ui| {
        if ui.button("Reveal All").clicked() {
            for room in &dungeon.graph.rooms {
                fog::reveal_room(&room.id, presentation);
            }
            for edge in &dungeon.graph.connections {
                fog::open_door(&edge.connection.id, presentation);
            }
        }
        if ui.button("Hide All").clicked() {
            for room in &dungeon.graph.rooms {
                fog::hide_room(&room.id, presentation);
            }
            for edge in &dungeon.graph.connections {
                fog::close_door(&edge.connection.id, presentation);
            }
        }
        if ui.button("1\"/sq").on_hover_text("Zoom: 1 inch per square on 40\" screen").clicked() {
            let target_zoom = zoom_for_one_inch_square(ui.ctx(), 40.0);
            let canvas_center = view_state.canvas_size / 2.0;
            let world_center_x = (canvas_center.x - view_state.view.offset.x) / view_state.view.zoom;
            let world_center_y = (canvas_center.y - view_state.view.offset.y) / view_state.view.zoom;
            view_state.view.zoom = target_zoom;
            view_state.view.center_on(world_center_x, world_center_y, view_state.canvas_size);
        }
    });

    ui.add_space(8.0);

    let in_combat = presentation.combat_tracker.is_some();

    if let Some(sel_room_id) = view_state.selected_room.clone() {
        // --- Contextual sidebar for selected room ---
        let room_label = dungeon.graph.room_by_id(&sel_room_id)
            .map(|r| r.label.clone())
            .unwrap_or_else(|| "?".to_string());
        ui.heading(&room_label);
        ui.separator();

        if ui.small_button("Deselect").clicked() {
            view_state.selected_room = None;
        }

        // Visibility control
        let vis = presentation.room_visibility(&sel_room_id).clone();
        ui.add_space(4.0);
        ui.label("Visibility:");
        ui.horizontal(|ui| {
            if ui.selectable_label(matches!(vis, Visibility::Hidden), "Hidden").clicked() {
                fog::hide_room(&sel_room_id, presentation);

            }
            if ui.selectable_label(matches!(vis, Visibility::Explored), "Explored").clicked() {
                fog::explore_room(&sel_room_id, presentation);

            }
            if ui.selectable_label(matches!(vis, Visibility::Visible), "Visible").clicked() {
                fog::reveal_room(&sel_room_id, presentation);

            }
        });

        // Room position/size info
        if let Some(layout) = &dungeon.layout {
            if let Some(rl) = layout.room_by_id(&sel_room_id) {
                ui.add_space(4.0);
                ui.label(format!("Size: {}x{} ({}x{} ft)", rl.width, rl.height, rl.width * 5, rl.height * 5));
            }
        }

        // Doors for this room
        let room_doors: Vec<_> = dungeon.graph.connections.iter()
            .filter(|e| e.source_room_id == sel_room_id || e.target_room_id == sel_room_id)
            .map(|e| {
                let other = if e.source_room_id == sel_room_id { &e.target_room_id } else { &e.source_room_id };
                let other_label = dungeon.graph.room_by_id(other)
                    .map(|r| r.label.as_str()).unwrap_or("?");
                (e.connection.id.clone(), other_label.to_string())
            })
            .collect();
        if !room_doors.is_empty() {
            ui.add_space(4.0);
            ui.label("Doors:");
            for (conn_id, other_label) in &room_doors {
                ui.horizontal(|ui| {
                    let is_open = presentation.is_door_open(conn_id);
                    let (state_label, state_color) = if is_open {
                        ("O", egui::Color32::from_rgb(100, 255, 100))
                    } else {
                        ("C", egui::Color32::from_rgb(255, 100, 100))
                    };
                    ui.colored_label(state_color, state_label);
                    if ui.button(other_label).clicked() {
                        fog::toggle_door(conn_id, presentation);
        
                    }
                });
            }
        }

        // Quick actions
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            if ui.button("Reveal + Adjacent").clicked() {
                fog::reveal_room_and_adjacent(&sel_room_id, presentation, &dungeon.graph);

            }
            if ui.button("Move Party Here").clicked() {
                presentation.party_room = Some(sel_room_id.clone());
            }
        });
        if ui.button("Center Camera").clicked() {
            if let Some(layout) = &dungeon.layout {
                if let Some(rl) = layout.room_by_id(&sel_room_id) {
                    let cx = (rl.x as f32 + rl.width as f32 / 2.0) * GRID_PX;
                    let cy = (rl.y as f32 + rl.height as f32 / 2.0) * GRID_PX;
                    view_state.view.center_on(cx, cy, view_state.canvas_size);
                }
            }
        }

        // Party — only if party is in this room
        let party_here = presentation.party_room.as_ref() == Some(&sel_room_id);
        if party_here {
            ui.add_space(8.0);
            ui.heading("Party");
            ui.separator();
            party_section(ui, dungeon, presentation, in_combat);
        }

        // Encounters in this room
        let room_encounter_indices: Vec<usize> = dungeon.encounters.iter().enumerate()
            .filter(|(_, e)| presentation.encounter_room(e) == sel_room_id)
            .map(|(i, _)| i)
            .collect();
        if !room_encounter_indices.is_empty() {
            ui.add_space(8.0);
            ui.heading("Encounters Here");
            ui.separator();

            // Collect display info first, then render with mutable access
            struct MonsterDisplayInfo {
                m_idx: usize,
                name: String,
                ac: Option<u8>,
                hp: Option<i32>,
            }
            struct EncDisplayInfo {
                enc_idx: usize,
                type_marker: &'static str,
                name: String,
                monsters: Vec<MonsterDisplayInfo>,
            }
            let enc_infos: Vec<EncDisplayInfo> = room_encounter_indices.iter().map(|&i| {
                let enc = &dungeon.encounters[i];
                let type_marker = match enc.encounter_type {
                    EncounterType::Static => "S",
                    EncounterType::Wandering(_) => "W",
                };
                let monsters: Vec<MonsterDisplayInfo> = enc.monsters.iter().enumerate().map(|(m_idx, em)| {
                    let monster = crate::presentation::combat_tracker::resolve_monster(
                        &em.monster_ref, monster_db, &dungeon.custom_monsters,
                    );
                    let name = monster.map(|m| m.name.clone()).unwrap_or_else(|| "?".to_string());
                    let stats = monster.map(|m| crate::model::combat_stats::parse_combat_stats(m));
                    let ac = stats.as_ref().and_then(|s| s.ac);
                    let hp = stats.as_ref().map(|s| s.max_hp);
                    MonsterDisplayInfo { m_idx, name, ac, hp }
                }).collect();
                EncDisplayInfo { enc_idx: i, type_marker, name: enc.name.clone(), monsters }
            }).collect();

            for info in &enc_infos {
                ui.label(format!("[{}] {}", info.type_marker, info.name));
                for minfo in &info.monsters {
                    ui.horizontal(|ui| {
                        let stats_str = match (minfo.ac, minfo.hp) {
                            (Some(ac), Some(hp)) => format!("  {} (AC {} HP {}) x", minfo.name, ac, hp),
                            _ => format!("  {} x", minfo.name),
                        };
                        ui.label(&stats_str);
                        let mut count = dungeon.encounters[info.enc_idx].monsters[minfo.m_idx].count;
                        if crate::ui::canvas_common::num_input_u32(ui, &mut count, 35.0) {
                            dungeon.encounters[info.enc_idx].monsters[minfo.m_idx].count = count;
                        }
                    });
                }
                if ui.small_button("+ Add Monster").clicked() {
                    ui.ctx().memory_mut(|mem| {
                        mem.data.insert_temp(egui::Id::new("monster_browser_open"), true);
                        mem.data.insert_temp(egui::Id::new("monster_browser_target"), info.enc_idx);
                    });
                }
            }

            // Rebuild room_encounters for Start Combat / Add to Combat buttons
            let room_encounters: Vec<&crate::model::Encounter> = room_encounter_indices.iter()
                .map(|&i| &dungeon.encounters[i])
                .collect();
            if !in_combat {
                // Awareness check for encounters in this room (party also here)
                if party_here && !dungeon.party.is_empty() {
                    if ui.button("Awareness Check").clicked() {
                        let mut results = Vec::new();
                        for enc in &room_encounters {
                            let result = crate::presentation::awareness::run_awareness_check(
                                dungeon, enc, &sel_room_id, &sel_room_id, monster_db,
                            );
                            results.push(result);
                        }
                        presentation.last_awareness_results = results;
                    }
                }

                let room_encounter_slice: Vec<crate::model::Encounter> = room_encounters.iter()
                    .map(|e| (*e).clone())
                    .collect();

                // Start combat button, applying per-creature surprise from awareness
                if ui.button(format!("Start Combat in {}", room_label)).clicked() {
                    let mut tracker = CombatTracker::init_with_party(
                        &room_encounter_slice,
                        monster_db,
                        &dungeon.custom_monsters,
                        combat_stats_cache,
                        &dungeon.party,
                    );
                    for result in &presentation.last_awareness_results {
                        tracker.apply_awareness(result);
                    }
                    presentation.combat_tracker = Some(tracker);
                    presentation.last_awareness_results.clear();
                }
            } else {
                // Add encounters to existing combat
                if ui.button("Add to Combat").clicked() {
                    if let Some(tracker) = &mut presentation.combat_tracker {
                        for enc in &room_encounters {
                            tracker.add_encounter(enc, monster_db, &dungeon.custom_monsters, combat_stats_cache);
                        }
                    }
                }
            }
        }

        // Nearby encounters (not in this room but within detection range)
        if !in_combat {
            let distances = crate::presentation::bfs_distances(&sel_room_id, &dungeon.graph);
            let mut nearby_encounters: Vec<(&crate::model::Encounter, u32, Option<f32>)> = Vec::new();
            for enc in &dungeon.encounters {
                if presentation.defeated_encounters.contains(&enc.id) { continue; }
                let enc_room = presentation.encounter_room(enc).to_string();
                if enc_room == sel_room_id { continue; }
                if let Some(&hops) = distances.get(&enc_room) {
                    let feet = dungeon.layout.as_ref()
                        .and_then(|layout| crate::presentation::awareness::encounter_distance_feet(
                            &sel_room_id, &enc_room, layout));
                    nearby_encounters.push((enc, hops, feet));
                }
            }
            nearby_encounters.sort_by_key(|(_, hops, _)| *hops);

            if !nearby_encounters.is_empty() {
                ui.add_space(8.0);
                ui.heading("Nearby Encounters");
                ui.separator();
                for (enc, hops, feet) in &nearby_encounters {
                    let dist_str = if let Some(ft) = feet {
                        format!("{} room{}, ~{:.0} ft", hops, if *hops != 1 { "s" } else { "" }, ft)
                    } else {
                        format!("{} room{}", hops, if *hops != 1 { "s" } else { "" })
                    };
                    let type_marker = match enc.encounter_type {
                        EncounterType::Static => "S",
                        EncounterType::Wandering(_) => "W",
                    };
                    ui.horizontal(|ui| {
                        ui.label(format!("[{}] {} ({})", type_marker, enc.name, dist_str));
                        if party_here && !dungeon.party.is_empty() {
                            let enc_room = presentation.encounter_room(enc).to_string();
                            if ui.small_button("Check").on_hover_text("Run awareness check").clicked() {
                                let result = crate::presentation::awareness::run_awareness_check(
                                    dungeon, enc, &enc_room, &sel_room_id, monster_db,
                                );
                                // Replace any existing result for this encounter
                                presentation.last_awareness_results.retain(|r| r.encounter_id != enc.id);
                                presentation.last_awareness_results.push(result);
                            }
                        }
                    });
                }
            }
        }

        // Awareness check results
        if !presentation.last_awareness_results.is_empty() {
            ui.add_space(8.0);
            ui.heading("Awareness Results");
            ui.separator();
            if ui.small_button("Clear").clicked() {
                presentation.last_awareness_results.clear();
            }
            let results = presentation.last_awareness_results.clone();
            for result in &results {
                egui::CollapsingHeader::new(&result.encounter_name)
                    .id_salt(format!("awareness_{}", result.encounter_id))
                    .default_open(true)
                    .show(ui, |ui| {
                        // Distance & light
                        if let Some(ft) = result.distance_feet {
                            ui.label(format!("Distance: {} rooms, ~{:.0} ft", result.distance_rooms, ft));
                        } else {
                            ui.label(format!("Distance: {} rooms", result.distance_rooms));
                        }
                        ui.label(format!(
                            "Light: encounter {}, party {}",
                            result.encounter_light.label(),
                            result.party_light.label(),
                        ));

                        let surprise_color = egui::Color32::from_rgb(255, 200, 50);
                        let ok_color = egui::Color32::from_rgb(100, 255, 100);
                        let hidden_color = egui::Color32::from_rgb(100, 180, 255);

                        // Monster stealth rolls & state
                        ui.add_space(4.0);
                        ui.label("Monsters:");
                        for m in &result.monsters {
                            let (tag, color) = awareness_tag_color(m, surprise_color, hidden_color, ok_color);
                            ui.colored_label(color, format!(
                                "  {} - Stealth {} | PP {}{}",
                                m.name, m.stealth_roll, m.passive_perception, tag,
                            ));
                        }

                        // Party stealth rolls & state
                        ui.add_space(4.0);
                        ui.label("Party:");
                        for pc in &result.party {
                            let (tag, color) = awareness_tag_color(pc, surprise_color, hidden_color, ok_color);
                            ui.colored_label(color, format!(
                                "  {} - Stealth {} | PP {}{}",
                                pc.name, pc.stealth_roll, pc.passive_perception, tag,
                            ));
                        }

                        // Summary
                        ui.add_space(4.0);
                        let n_party_surprised = result.party.iter().filter(|c| c.surprised).count();
                        let n_party_hidden = result.party.iter().filter(|c| c.hidden).count();
                        let n_monster_surprised = result.monsters.iter().filter(|c| c.surprised).count();
                        let n_monster_hidden = result.monsters.iter().filter(|c| c.hidden).count();

                        if n_party_surprised > 0 {
                            ui.colored_label(surprise_color, format!(
                                "{}/{} PCs surprised (disadv. initiative)",
                                n_party_surprised, result.party.len(),
                            ));
                        }
                        if n_party_hidden > 0 {
                            ui.colored_label(hidden_color, format!(
                                "{}/{} PCs hidden (adv. initiative)",
                                n_party_hidden, result.party.len(),
                            ));
                        }
                        if n_monster_surprised > 0 {
                            ui.colored_label(surprise_color, format!(
                                "{}/{} monsters surprised (disadv. initiative)",
                                n_monster_surprised, result.monsters.len(),
                            ));
                        }
                        if n_monster_hidden > 0 {
                            ui.colored_label(hidden_color, format!(
                                "{}/{} monsters hidden (adv. initiative)",
                                n_monster_hidden, result.monsters.len(),
                            ));
                        }
                        if n_party_surprised + n_party_hidden + n_monster_surprised + n_monster_hidden == 0 {
                            ui.label("No surprise or hidden - all aware");
                        }
                    });
            }
        }

    } else {
        // --- General room/door lists (no selection) ---
        egui::CollapsingHeader::new("Rooms")
            .id_salt("gen_rooms")
            .default_open(false)
            .show(ui, |ui| {
            egui::ScrollArea::vertical().max_height(200.0).id_salt("rooms_scroll").show(ui, |ui| {
                let rooms: Vec<_> = dungeon.graph.rooms.iter().map(|r| (r.id.clone(), r.label.clone())).collect();
                for (room_id, label) in rooms {
                    ui.horizontal(|ui| {
                        let vis = presentation.room_visibility(&room_id).clone();
                        let vis_label = match vis {
                            Visibility::Hidden => "H",
                            Visibility::Explored => "E",
                            Visibility::Visible => "V",
                        };
                        let vis_color = match vis {
                            Visibility::Hidden => egui::Color32::from_rgb(255, 100, 100),
                            Visibility::Explored => egui::Color32::from_rgb(255, 200, 100),
                            Visibility::Visible => egui::Color32::from_rgb(100, 255, 100),
                        };
                        ui.colored_label(vis_color, vis_label);
                        if ui.button(&label).clicked() {
                            fog::cycle_room_visibility(&room_id, presentation);
                        }
                        if ui.small_button("\u{2316}").on_hover_text("Center on room").clicked() {
                            if let Some(layout) = &dungeon.layout {
                                if let Some(rl) = layout.room_by_id(&room_id) {
                                    let cx = (rl.x as f32 + rl.width as f32 / 2.0) * GRID_PX;
                                    let cy = (rl.y as f32 + rl.height as f32 / 2.0) * GRID_PX;
                                    view_state.view.center_on(cx, cy, view_state.canvas_size);
                                }
                            }
                        }
                    });
                }
            });
        });

        egui::CollapsingHeader::new("Doors")
            .id_salt("gen_doors")
            .default_open(false)
            .show(ui, |ui| {
            egui::ScrollArea::vertical().max_height(200.0).id_salt("doors_scroll").show(ui, |ui| {
                let edges: Vec<_> = dungeon.graph.connections.iter().map(|e| {
                    let src = dungeon.graph.room_by_id(&e.source_room_id)
                        .map(|r| r.label.as_str()).unwrap_or("?");
                    let tgt = dungeon.graph.room_by_id(&e.target_room_id)
                        .map(|r| r.label.as_str()).unwrap_or("?");
                    (e.connection.id.clone(), format!("{} <-> {}", src, tgt))
                }).collect();
                for (conn_id, label) in edges {
                    ui.horizontal(|ui| {
                        let is_open = presentation.is_door_open(&conn_id);
                        let (state_label, state_color) = if is_open {
                            ("O", egui::Color32::from_rgb(100, 255, 100))
                        } else {
                            ("C", egui::Color32::from_rgb(255, 100, 100))
                        };
                        ui.colored_label(state_color, state_label);
                        if ui.button(&label).clicked() {
                            fog::toggle_door(&conn_id, presentation);
                        }
                    });
                }
            });
        });

        // Party Management (full view when no room selected)
        egui::CollapsingHeader::new("Party")
            .id_salt("gen_party")
            .default_open(!in_combat)
            .show(ui, |ui| {
            party_section(ui, dungeon, presentation, in_combat);
        });

        // Encounters grouped by room
        if !dungeon.encounters.is_empty() {
            egui::CollapsingHeader::new("Encounters")
                .id_salt("gen_encounters")
                .default_open(!in_combat)
                .show(ui, |ui| {

            ui.horizontal(|ui| {
                if ui.button("Tick").on_hover_text("Move wandering encounters").clicked() {
                    presentation.tick_encounters(dungeon);
                    if presentation.autobattle {
                        run_autobattles(presentation, dungeon, monster_db, combat_stats_cache);
                    }
                }
                if ui.button("Reset Pos").on_hover_text("Return all encounters to home rooms").clicked() {
                    presentation.reset_encounter_positions(dungeon);
                }
                if ui.button("Reset Stats").on_hover_text("Clear all defeated encounters").clicked() {
                    presentation.defeated_encounters.clear();
                }
            });
            ui.checkbox(&mut presentation.autobattle, "Autobattle")
                .on_hover_text("Encounters sharing a room after a tick automatically fight");

            {
                let mut rooms_with_encounters: Vec<(String, String, Vec<&crate::model::Encounter>)> = Vec::new();
                for enc in &dungeon.encounters {
                    let room_id = presentation.encounter_room(enc).to_string();
                    let room_label = dungeon.graph.room_by_id(&room_id)
                        .map(|r| r.label.clone())
                        .unwrap_or_else(|| "?".to_string());
                    if let Some(entry) = rooms_with_encounters.iter_mut().find(|(rid, _, _)| *rid == room_id) {
                        entry.2.push(enc);
                    } else {
                        rooms_with_encounters.push((room_id, room_label, vec![enc]));
                    }
                }
                for (_, room_label, room_encs) in &rooms_with_encounters {
                    egui::CollapsingHeader::new(format!("{} ({})", room_label, room_encs.len()))
                        .id_salt(format!("enc_room_gen_{}", room_label))
                        .default_open(false)
                        .show(ui, |ui| {
                            for enc in room_encs {
                                let type_marker = match enc.encounter_type {
                                    EncounterType::Static => "S",
                                    EncounterType::Wandering(_) => "W",
                                };
                                ui.label(format!("[{}] {}", type_marker, enc.name));
                            }
                            let room_encounter_slice: Vec<crate::model::Encounter> = room_encs.iter()
                                .map(|e| (*e).clone())
                                .collect();
                            if !in_combat {
                                if ui.button(format!("Start Combat in {}", room_label)).clicked() {
                                    presentation.combat_tracker = Some(CombatTracker::init_with_party(
                                        &room_encounter_slice,
                                        monster_db,
                                        &dungeon.custom_monsters,
                                        combat_stats_cache,
                                        &dungeon.party,
                                    ));
                                }
                            } else {
                                if ui.button("Add to Combat").clicked() {
                                    if let Some(tracker) = &mut presentation.combat_tracker {
                                        for enc in &room_encounter_slice {
                                            tracker.add_encounter(enc, monster_db, &dungeon.custom_monsters, combat_stats_cache);
                                        }
                                    }
                                }
                            }
                        });
                }
            }

            }); // end Encounters collapsing header
        }
    }

    // Combat tracker — always visible when active (regardless of room selection)
    if presentation.combat_tracker.is_some() {
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.heading("Combat");
            let pop_label = if *combat_window_open { "Dock" } else { "Pop Out" };
            if ui.small_button(pop_label).clicked() {
                *combat_window_open = !*combat_window_open;
            }
        });
        ui.separator();
        if ui.button("End Combat").clicked() {
            presentation.combat_tracker = None;
        }
    }

    if !*combat_window_open {
    if let Some(tracker) = &mut presentation.combat_tracker {
            combat_tracker_ui(ui, tracker, dungeon);
        }
    } // end if !combat_window_open


    ui.add_space(8.0);

    // AoE markers
    egui::CollapsingHeader::new("Area of Effect")
        .id_salt("aoe_section")
        .default_open(true)
        .show(ui, |ui| {
        aoe_sidebar(ui, presentation, dungeon);
    });

    ui.add_space(8.0);

    // Player window
    egui::CollapsingHeader::new("Player View")
        .id_salt("player_view_section")
        .default_open(true)
        .show(ui, |ui| {

    if ui.button(if *player_viewport_open { "Close Player Window" } else { "Open Player Window" }).clicked() {
        *player_viewport_open = !*player_viewport_open;
    }
    ui.checkbox(&mut presentation.show_labels_player, "Show labels to players");
    ui.checkbox(&mut player_view_state.locked, "Lock player view (no scroll/pan)");

    // Player view zoom: 1 inch per square on 40" screen
    if ui.button("Player: 1\"/square (40\" screen)").clicked() {
        let target_zoom = zoom_for_one_inch_square(ui.ctx(), 40.0);
        let canvas_center = player_view_state.canvas_size / 2.0;
        if canvas_center.x > 0.0 && canvas_center.y > 0.0 {
            let world_center_x = (canvas_center.x - player_view_state.view.offset.x) / player_view_state.view.zoom;
            let world_center_y = (canvas_center.y - player_view_state.view.offset.y) / player_view_state.view.zoom;
            player_view_state.view.zoom = target_zoom;
            player_view_state.view.center_on(world_center_x, world_center_y, player_view_state.canvas_size);
        } else {
            player_view_state.view.zoom = target_zoom;
        }
    }

    // Sync player view to DM's current view center
    if ui.button("Sync player to DM view").on_hover_text("Center the player view on what the DM is looking at").clicked() {
        let dm_center_world_x = (view_state.canvas_size.x / 2.0 - view_state.view.offset.x) / view_state.view.zoom;
        let dm_center_world_y = (view_state.canvas_size.y / 2.0 - view_state.view.offset.y) / view_state.view.zoom;
        player_view_state.view.center_on(dm_center_world_x, dm_center_world_y, player_view_state.canvas_size);
    }

    // Center player view on room
    if let Some(layout) = &dungeon.layout {
        if let Some(ref sel_id) = view_state.selected_room {
            let room_label = dungeon.graph.room_by_id(sel_id)
                .map(|r| r.label.as_str())
                .unwrap_or("room");
            if ui.button(format!("Center player on {}", room_label)).clicked() {
                if let Some(rl) = layout.room_by_id(sel_id) {
                    let cx = (rl.x as f32 + rl.width as f32 / 2.0) * GRID_PX;
                    let cy = (rl.y as f32 + rl.height as f32 / 2.0) * GRID_PX;
                    player_view_state.view.center_on(cx, cy, player_view_state.canvas_size);
                }
            }
        } else {
            let rooms: Vec<_> = dungeon.graph.rooms.iter()
                .filter(|r| *presentation.room_visibility(&r.id) != Visibility::Hidden)
                .map(|r| (r.id.clone(), r.label.clone()))
                .collect();
            if !rooms.is_empty() {
                egui::ComboBox::from_id_salt("player_center_room")
                    .selected_text("Center player on...")
                    .show_ui(ui, |ui| {
                        for (room_id, label) in &rooms {
                            if ui.selectable_label(false, label).clicked() {
                                if let Some(rl) = layout.room_by_id(room_id) {
                                    let cx = (rl.x as f32 + rl.width as f32 / 2.0) * GRID_PX;
                                    let cy = (rl.y as f32 + rl.height as f32 / 2.0) * GRID_PX;
                                    player_view_state.view.center_on(cx, cy, player_view_state.canvas_size);
                                }
                            }
                        }
                    });
            }
        }
    }

    }); // end Player View collapsing header

    // Stat block pop-out window
    {
        let stat_mid: Option<MonsterInstanceId> = ui.ctx().memory(|mem|
            mem.data.get_temp(egui::Id::new("combat_statblock_mid"))
        );
        if let Some(mid) = stat_mid {
            let mut open = true;
            let monster = dungeon.encounters.iter()
                .find(|e| e.id == mid.encounter_id)
                .and_then(|enc| enc.monsters.get(mid.monster_index))
                .and_then(|em| crate::presentation::combat_tracker::resolve_monster(
                    &em.monster_ref, monster_db, &dungeon.custom_monsters,
                ));
            if let Some(m) = monster {
                egui::Window::new(format!("Stat Block: {}", m.name))
                    .id(egui::Id::new("combat_statblock_window"))
                    .open(&mut open)
                    .default_size([400.0, 500.0])
                    .resizable(true)
                    .show(ui.ctx(), |ui| {
                        egui::ScrollArea::vertical().show(ui, |ui| {
                            crate::ui::encounters_view::draw_stat_block(ui, m, monster_db);
                        });
                    });
            }
            if !open {
                ui.ctx().memory_mut(|mem| {
                    mem.data.remove::<MonsterInstanceId>(egui::Id::new("combat_statblock_mid"));
                });
            }
        }
    }

    // --- Single Combat Simulator ---
    ui.add_space(12.0);
    egui::CollapsingHeader::new("Combat Simulator")
        .id_salt("combat_sim_section")
        .default_open(false)
        .show(ui, |ui| {

    let sim = &mut view_state.single_combat;

    // Build and run combat sim
    let mut sim_combatants: Option<(Vec<combat_sim::SimCombatant>, Vec<combat_sim::SimCombatant>)> = None;

    if let Some(ref sel_id) = view_state.selected_room {
        // Room-scoped: encounters in this room fight each other
        let room_enc_indices: Vec<(usize, String)> = dungeon.encounters.iter().enumerate()
            .filter(|(_, e)| presentation.encounter_room(e) == sel_id.as_str())
            .map(|(i, e)| (i, e.name.clone()))
            .collect();
        if room_enc_indices.is_empty() {
            ui.label("No encounters in this room.");
        } else if room_enc_indices.len() < 2 {
            ui.label("Need at least 2 encounters to simulate.");
        } else {
            ui.horizontal(|ui| {
                ui.label("Side A:");
                egui::ComboBox::from_id_salt("pres_room_sim_a")
                    .selected_text(room_enc_indices.iter()
                        .find(|(i, _)| SimSide::Encounter(*i) == sim.side_a)
                        .map(|(_, n)| n.as_str())
                        .unwrap_or("Select..."))
                    .width(140.0)
                    .show_ui(ui, |ui| {
                        for (i, name) in &room_enc_indices {
                            if ui.selectable_label(sim.side_a == SimSide::Encounter(*i), name).clicked() {
                                sim.side_a = SimSide::Encounter(*i);
                            }
                        }
                    });
            });
            ui.horizontal(|ui| {
                ui.label("Side B:");
                egui::ComboBox::from_id_salt("pres_room_sim_b")
                    .selected_text(room_enc_indices.iter()
                        .find(|(i, _)| SimSide::Encounter(*i) == sim.side_b)
                        .map(|(_, n)| n.as_str())
                        .unwrap_or("Select..."))
                    .width(140.0)
                    .show_ui(ui, |ui| {
                        for (i, name) in &room_enc_indices {
                            if ui.selectable_label(sim.side_b == SimSide::Encounter(*i), name).clicked() {
                                sim.side_b = SimSide::Encounter(*i);
                            }
                        }
                    });
            });
            ui.horizontal(|ui| {
                if ui.button("Simulate 1v1").clicked() {
                    let side_a = if let SimSide::Encounter(idx) = &sim.side_a {
                        if let Some(enc) = dungeon.encounters.get(*idx) {
                            build_combatants_from_encounter(enc, monster_db, &dungeon.custom_monsters, combat_stats_cache, 0)
                        } else { Vec::new() }
                    } else { Vec::new() };
                    let side_b = if let SimSide::Encounter(idx) = &sim.side_b {
                        if let Some(enc) = dungeon.encounters.get(*idx) {
                            build_combatants_from_encounter(enc, monster_db, &dungeon.custom_monsters, combat_stats_cache, 1)
                        } else { Vec::new() }
                    } else { Vec::new() };
                    if !side_a.is_empty() && !side_b.is_empty() {
                        sim_combatants = Some((side_a, side_b));
                    }
                }
                if ui.button("Free-for-all").clicked() {
                    let mut groups: Vec<Vec<combat_sim::SimCombatant>> = Vec::new();
                    for (side, (idx, _)) in room_enc_indices.iter().enumerate() {
                        if let Some(enc) = dungeon.encounters.get(*idx) {
                            let combatants = build_combatants_from_encounter(
                                enc, monster_db, &dungeon.custom_monsters, combat_stats_cache, side,
                            );
                            if !combatants.is_empty() {
                                groups.push(combatants);
                            }
                        }
                    }
                    if groups.len() >= 2 {
                        let result = combat_sim::run_combat_ffa(&groups);
                        // Mark wiped-out encounters as defeated
                        for (side, (idx, _)) in room_enc_indices.iter().enumerate() {
                            let all_dead = result.combatants.iter()
                                .filter(|c| c.side == side)
                                .all(|c| c.current_hp <= 0);
                            if all_dead {
                                if let Some(enc) = dungeon.encounters.get(*idx) {
                                    presentation.defeated_encounters.insert(enc.id.clone());
                                }
                            }
                        }
                        sim.last_result = Some(result);
                    }
                }
            });
        }
    } else {
        // Full side picker
        let enc_names: Vec<(usize, String)> = dungeon.encounters.iter().enumerate()
            .map(|(i, e)| (i, e.name.clone()))
            .collect();

        ui.horizontal(|ui| {
            ui.label("Side A:");
            egui::ComboBox::from_id_salt("pres_sim_side_a")
                .selected_text(match &sim.side_a {
                    SimSide::Party => "Party".to_string(),
                    SimSide::Encounter(idx) => enc_names.iter()
                        .find(|(i, _)| i == idx)
                        .map(|(_, n)| n.clone())
                        .unwrap_or_else(|| "?".to_string()),
                })
                .width(140.0)
                .show_ui(ui, |ui| {
                    if ui.selectable_label(sim.side_a == SimSide::Party, "Party").clicked() {
                        sim.side_a = SimSide::Party;
                    }
                    for (i, name) in &enc_names {
                        if ui.selectable_label(sim.side_a == SimSide::Encounter(*i), name).clicked() {
                            sim.side_a = SimSide::Encounter(*i);
                        }
                    }
                });
        });

        ui.horizontal(|ui| {
            ui.label("Side B:");
            egui::ComboBox::from_id_salt("pres_sim_side_b")
                .selected_text(match &sim.side_b {
                    SimSide::Party => "Party".to_string(),
                    SimSide::Encounter(idx) => enc_names.iter()
                        .find(|(i, _)| i == idx)
                        .map(|(_, n)| n.clone())
                        .unwrap_or_else(|| "?".to_string()),
                })
                .width(140.0)
                .show_ui(ui, |ui| {
                    if ui.selectable_label(sim.side_b == SimSide::Party, "Party").clicked() {
                        sim.side_b = SimSide::Party;
                    }
                    for (i, name) in &enc_names {
                        if ui.selectable_label(sim.side_b == SimSide::Encounter(*i), name).clicked() {
                            sim.side_b = SimSide::Encounter(*i);
                        }
                    }
                });
        });

        ui.add_space(4.0);

        if ui.button("Run Single Combat").clicked() {
            let side_a = match &sim.side_a {
                SimSide::Party => build_combatants_from_party(&dungeon.party, 0),
                SimSide::Encounter(idx) => {
                    if let Some(enc) = dungeon.encounters.get(*idx) {
                        build_combatants_from_encounter(enc, monster_db, &dungeon.custom_monsters, combat_stats_cache, 0)
                    } else { Vec::new() }
                }
            };
            let side_b = match &sim.side_b {
                SimSide::Party => build_combatants_from_party(&dungeon.party, 1),
                SimSide::Encounter(idx) => {
                    if let Some(enc) = dungeon.encounters.get(*idx) {
                        build_combatants_from_encounter(enc, monster_db, &dungeon.custom_monsters, combat_stats_cache, 1)
                    } else { Vec::new() }
                }
            };
            if !side_a.is_empty() && !side_b.is_empty() {
                sim_combatants = Some((side_a, side_b));
            }
        }
    }

    if let Some((side_a_combatants, side_b_combatants)) = sim_combatants {
        let result = run_combat(&side_a_combatants, &side_b_combatants);

        // Persist results to combat tracker if active
        if let Some(tracker) = &mut presentation.combat_tracker {
            for final_c in &result.combatants {
                for (_, inst) in tracker.instances.iter_mut() {
                    if inst.label == final_c.name {
                        inst.current_hp = final_c.current_hp.max(0);
                        inst.is_dead = inst.current_hp <= 0;
                        break;
                    }
                }
                for (_, pc) in tracker.players.iter_mut() {
                    if pc.name == final_c.name {
                        pc.current_hp = final_c.current_hp.max(0);
                        break;
                    }
                }
            }
        }

        // Mark wiped-out encounters as defeated
        let check_side = |side_enum: &SimSide, side_idx: usize| {
            if let SimSide::Encounter(enc_idx) = side_enum {
                let all_dead = result.combatants.iter()
                    .filter(|c| c.side == side_idx)
                    .all(|c| c.current_hp <= 0);
                if all_dead {
                    if let Some(enc) = dungeon.encounters.get(*enc_idx) {
                        return Some(enc.id.clone());
                    }
                }
            }
            None
        };
        if let Some(id) = check_side(&sim.side_a.clone(), 0) {
            presentation.defeated_encounters.insert(id);
        }
        if let Some(id) = check_side(&sim.side_b.clone(), 1) {
            presentation.defeated_encounters.insert(id);
        }

        sim.last_result = Some(result);
    }

    if let Some(result) = &sim.last_result {
        ui.add_space(4.0);
        ui.group(|ui| {
            // Collect all distinct sides
            let mut sides: Vec<usize> = result.combatants.iter().map(|c| c.side).collect();
            sides.sort();
            sides.dedup();

            let winner_text = match result.winner {
                Some(s) => format!("Side {} wins", s + 1),
                None => "Draw (timeout)".to_string(),
            };
            ui.label(format!("{} in {} rounds", winner_text, result.rounds));

            for side in &sides {
                let side_combatants: Vec<_> = result.combatants.iter()
                    .filter(|c| c.side == *side)
                    .collect();
                if side_combatants.is_empty() { continue; }
                let is_winner = result.winner == Some(*side);
                let header_color = if is_winner {
                    egui::Color32::from_rgb(100, 255, 100)
                } else {
                    egui::Color32::from_rgb(180, 180, 180)
                };
                ui.colored_label(header_color, format!("Side {}:", side + 1));
                for c in &side_combatants {
                    let status = if c.current_hp <= 0 { "DEAD" } else { "alive" };
                    let hp_display = if c.current_hp <= 0 {
                        format!("0/{}", c.max_hp)
                    } else {
                        format!("{}/{}", c.current_hp, c.max_hp)
                    };
                    let color = if c.current_hp <= 0 {
                        egui::Color32::from_rgb(255, 100, 100)
                    } else {
                        egui::Color32::from_rgb(100, 255, 100)
                    };
                    ui.colored_label(color, format!("  {} ({}) - {}", c.name, hp_display, status));
                }
            }
        });
    }

    }); // end Combat Simulator collapsing header

    ui.add_space(8.0);

    // Monster browser window (shared with encounters view, driven by egui temp memory)
    crate::ui::encounters_view::monster_browser_window(ui.ctx(), dungeon, monster_db, &mut None);

    // Web server controls
    ui.heading("Web Server");
    ui.separator();
}

/// Actions the sidebar can request from the app regarding the server.
pub enum ServerAction {
    None,
}
