use std::hash::{Hash, Hasher};

use crate::model::*;
use crate::render::recording::{RecordingRenderer, RenderCommand, replay_commands};
use crate::render::themed::RenderOptions;
use crate::ui::canvas_common::{handle_pan_zoom, ViewState, COLOR_PLACEHOLDER_TEXT};
use crate::ui::spatial_view::collect_floors;
use crate::util::{ViewTransform, GRID_PX};

struct RenderCache {
    commands: Vec<RenderCommand>,
    input_hash: u64,
}

pub struct DecorViewState {
    pub view: ViewState,
    render_cache: Option<RenderCache>,
    pub current_floor: Option<i32>,
    /// Room selected for decor editing.
    pub selected_room: Option<String>,
    /// Decor item being dragged (room_id, decor index).
    dragging_decor: Option<(String, usize)>,
    /// Decor type to place when clicking inside a room.
    pub place_type: DecorType,
    /// Whether we're in "place mode" (click to add decor).
    pub place_mode: bool,
    /// Selected decor item within the selected room (index).
    pub selected_decor: Option<usize>,
}

impl Default for DecorViewState {
    fn default() -> Self {
        Self {
            view: ViewState::default(),
            render_cache: None,
            current_floor: None,
            selected_room: None,
            dragging_decor: None,
            place_type: DecorType::Table,
            place_mode: false,
            selected_decor: None,
        }
    }
}

fn render_input_hash(layout: &SpatialLayout, graph: &DungeonGraph, theme: &Theme, current_floor: Option<i32>) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    let mut h = DefaultHasher::new();
    layout.rooms.len().hash(&mut h);
    for rl in &layout.rooms {
        rl.room_id.hash(&mut h);
        rl.x.hash(&mut h);
        rl.y.hash(&mut h);
        rl.width.hash(&mut h);
        rl.height.hash(&mut h);
        if let Some(room) = graph.room_by_id(&rl.room_id) {
            if let Some(cave) = &room.cave_data {
                cave.generation.hash(&mut h);
            }
            room.sections.len().hash(&mut h);
            for s in &room.sections {
                s.x.to_bits().hash(&mut h);
                s.y.to_bits().hash(&mut h);
                s.width.to_bits().hash(&mut h);
                s.height.to_bits().hash(&mut h);
                std::mem::discriminant(&s.elevation).hash(&mut h);
            }
        }
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
    current_floor.hash(&mut h);
    h.finish()
}

pub fn decor_view(ui: &mut egui::Ui, dungeon: &mut Dungeon, state: &mut DecorViewState) {
    let (response, painter) = ui.allocate_painter(
        ui.available_size(),
        egui::Sense::click_and_drag(),
    );
    let rect = response.rect;

    let bg = dungeon.theme.bg_color;
    painter.rect_filled(rect, 0.0, egui::Color32::from_rgba_unmultiplied(bg[0], bg[1], bg[2], bg[3]));

    handle_pan_zoom(&response, &mut state.view);
    let transform = ViewTransform::new(state.view.offset, state.view.zoom, rect);

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

    // Build floor-filtered layout
    let filtered_layout;
    let render_layout = if let Some(floor) = state.current_floor {
        let visible_room_ids: std::collections::HashSet<&str> = dungeon.graph.rooms.iter()
            .filter(|r| r.floor.visible_on(floor))
            .map(|r| r.id.as_str())
            .collect();
        filtered_layout = SpatialLayout {
            rooms: layout.rooms.iter()
                .filter(|rl| visible_room_ids.contains(rl.room_id.as_str()))
                .cloned()
                .collect(),
            corridors: layout.corridors.iter()
                .filter(|c| {
                    dungeon.graph.connections.iter()
                        .find(|e| e.connection.id == c.connection_id)
                        .is_some_and(|e| {
                            visible_room_ids.contains(e.source_room_id.as_str())
                                || visible_room_ids.contains(e.target_room_id.as_str())
                        })
                })
                .cloned()
                .collect(),
            bounds: layout.bounds.clone(),
        };
        &filtered_layout
    } else {
        layout
    };

    // Rebuild cached render commands if inputs changed
    let hash = render_input_hash(layout, &dungeon.graph, &dungeon.theme, state.current_floor);
    let needs_rebuild = state.render_cache.as_ref().is_none_or(|c| c.input_hash != hash);

    if needs_rebuild {
        let mut recorder = RecordingRenderer::new();
        let options = RenderOptions {
            show_grid: true,
            show_labels: true,
            show_notes: false,
            show_secrets: false,
        };
        crate::render::themed::render_themed(
            &mut recorder,
            &dungeon.graph,
            render_layout,
            &dungeon.theme,
            &options,
        );
        state.render_cache = Some(RenderCache {
            commands: recorder.commands,
            input_hash: hash,
        });
    }

    if let Some(cache) = &state.render_cache {
        replay_commands(&painter, &transform, &cache.commands);
    }

    // Draw room labels
    for rl in &render_layout.rooms {
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

    // Highlight selected room
    if let Some(ref sel_id) = state.selected_room {
        if let Some(rl) = render_layout.room_by_id(sel_id) {
            let min = transform.world_to_screen(egui::pos2(
                rl.x as f32 * GRID_PX, rl.y as f32 * GRID_PX,
            ));
            let max = transform.world_to_screen(egui::pos2(
                (rl.x as f32 + rl.width as f32) * GRID_PX,
                (rl.y as f32 + rl.height as f32) * GRID_PX,
            ));
            painter.rect_stroke(
                egui::Rect::from_min_max(min, max),
                0.0,
                egui::Stroke::new(2.0, egui::Color32::from_rgb(100, 180, 255)),
                egui::StrokeKind::Middle,
            );
        }
    }

    // Draw decor interaction handles for all rooms
    // (larger, more visible handles than the base render for interaction)
    for rl in &render_layout.rooms {
        if let Some(room) = dungeon.graph.room_by_id(&rl.room_id) {
            let room_px_x = rl.x as f32 * GRID_PX;
            let room_px_y = rl.y as f32 * GRID_PX;
            let is_selected_room = state.selected_room.as_deref() == Some(&rl.room_id);

            for (di, decor) in room.decor.iter().enumerate() {
                let wx = room_px_x + decor.x * GRID_PX;
                let wy = room_px_y + decor.y * GRID_PX;
                let screen = transform.world_to_screen(egui::pos2(wx, wy));
                let handle_r = (6.0 * transform.zoom).max(4.0);

                if is_selected_room {
                    // Draw selection ring for selected decor
                    let is_sel = state.selected_decor == Some(di);
                    let ring_color = if is_sel {
                        egui::Color32::from_rgb(255, 200, 50)
                    } else {
                        egui::Color32::from_rgb(100, 180, 255)
                    };
                    painter.circle_stroke(screen, handle_r + 2.0, egui::Stroke::new(1.5, ring_color));
                }
            }
        }
    }

    // Handle interactions
    let pointer_pos = response.interact_pointer_pos().or(response.hover_pos());

    // Dragging decor
    if let Some((ref drag_room_id, drag_idx)) = state.dragging_decor {
        if response.dragged_by(egui::PointerButton::Primary) {
            if let Some(pos) = pointer_pos {
                let world = transform.screen_to_world(pos);
                // Find the room layout to get room origin
                if let Some(rl) = render_layout.room_by_id(drag_room_id) {
                    let room_px_x = rl.x as f32 * GRID_PX;
                    let room_px_y = rl.y as f32 * GRID_PX;
                    let new_x = (world.x - room_px_x) / GRID_PX;
                    let new_y = (world.y - room_px_y) / GRID_PX;
                    // Clamp to room bounds
                    let new_x = new_x.clamp(0.0, rl.width as f32);
                    let new_y = new_y.clamp(0.0, rl.height as f32);
                    if let Some(room) = dungeon.graph.room_by_id_mut(drag_room_id) {
                        if drag_idx < room.decor.len() {
                            room.decor[drag_idx].x = new_x;
                            room.decor[drag_idx].y = new_y;
                        }
                    }
                }
            }
        }
        if response.drag_stopped() {
            state.dragging_decor = None;
        }
    }

    // Click handling
    if response.clicked() {
        if let Some(pos) = response.interact_pointer_pos() {
            let world = transform.screen_to_world(pos);
            let gx = (world.x / GRID_PX).floor() as i32;
            let gy = (world.y / GRID_PX).floor() as i32;

            // First, check if we clicked on an existing decor item in the selected room
            let mut clicked_decor = None;
            if let Some(ref sel_id) = state.selected_room {
                if let Some(rl) = render_layout.room_by_id(sel_id) {
                    if let Some(room) = dungeon.graph.room_by_id(sel_id) {
                        let room_px_x = rl.x as f32 * GRID_PX;
                        let room_px_y = rl.y as f32 * GRID_PX;
                        let hit_radius = GRID_PX * 0.5;
                        for (di, decor) in room.decor.iter().enumerate() {
                            let dx = world.x - (room_px_x + decor.x * GRID_PX);
                            let dy = world.y - (room_px_y + decor.y * GRID_PX);
                            if (dx * dx + dy * dy).sqrt() < hit_radius {
                                clicked_decor = Some(di);
                                break;
                            }
                        }
                    }
                }
            }

            if let Some(di) = clicked_decor {
                state.selected_decor = Some(di);
            } else if state.place_mode {
                // Place new decor if clicking inside a room
                if let Some(ref sel_id) = state.selected_room.clone() {
                    if let Some(rl) = render_layout.room_by_id(sel_id) {
                        let room_px_x = rl.x as f32 * GRID_PX;
                        let room_px_y = rl.y as f32 * GRID_PX;
                        let room_w = rl.width as f32 * GRID_PX;
                        let room_h = rl.height as f32 * GRID_PX;
                        if world.x >= room_px_x && world.x <= room_px_x + room_w
                            && world.y >= room_px_y && world.y <= room_px_y + room_h
                        {
                            let dx = (world.x - room_px_x) / GRID_PX;
                            let dy = (world.y - room_px_y) / GRID_PX;
                            let new_decor = RoomDecor::new(state.place_type, dx, dy);
                            if let Some(room) = dungeon.graph.room_by_id_mut(sel_id) {
                                room.decor.push(new_decor);
                                state.selected_decor = Some(room.decor.len() - 1);
                            }
                        }
                    }
                }
            } else {
                // Click to select room
                state.selected_decor = None;
                let mut hit = None;
                for rl in &render_layout.rooms {
                    if gx >= rl.x && gx < rl.x + rl.width as i32
                        && gy >= rl.y && gy < rl.y + rl.height as i32
                    {
                        hit = Some(rl.room_id.clone());
                        break;
                    }
                }
                state.selected_room = hit;
            }
        }
    }

    // Start drag on primary button drag start over a decor item
    if response.drag_started_by(egui::PointerButton::Primary) && state.dragging_decor.is_none() && !state.place_mode {
        if let Some(pos) = pointer_pos {
            let world = transform.screen_to_world(pos);
            if let Some(ref sel_id) = state.selected_room {
                if let Some(rl) = render_layout.room_by_id(sel_id) {
                    if let Some(room) = dungeon.graph.room_by_id(sel_id) {
                        let room_px_x = rl.x as f32 * GRID_PX;
                        let room_px_y = rl.y as f32 * GRID_PX;
                        let hit_radius = GRID_PX * 0.5;
                        for (di, decor) in room.decor.iter().enumerate() {
                            let dx = world.x - (room_px_x + decor.x * GRID_PX);
                            let dy = world.y - (room_px_y + decor.y * GRID_PX);
                            if (dx * dx + dy * dy).sqrt() < hit_radius {
                                state.dragging_decor = Some((sel_id.clone(), di));
                                state.selected_decor = Some(di);
                                break;
                            }
                        }
                    }
                }
            }
        }
    }

    // Place mode cursor
    if state.place_mode && response.hovered() {
        if let Some(pos) = response.hover_pos() {
            painter.circle_stroke(
                pos,
                8.0,
                egui::Stroke::new(1.5, egui::Color32::from_rgb(100, 255, 100)),
            );
            painter.text(
                pos + egui::vec2(12.0, -12.0),
                egui::Align2::LEFT_BOTTOM,
                state.place_type.label(),
                egui::FontId::proportional(11.0),
                egui::Color32::from_rgb(100, 255, 100),
            );
        }
    }
}

pub fn decor_sidebar(ui: &mut egui::Ui, dungeon: &mut Dungeon, state: &mut DecorViewState) {
    if let Some(ref sel_room_id) = state.selected_room.clone() {
        let room_label = dungeon.graph.room_by_id(sel_room_id)
            .map(|r| r.label.clone())
            .unwrap_or_else(|| "?".to_string());
        ui.heading(&room_label);
        ui.separator();

        if ui.small_button("Deselect").clicked() {
            state.selected_room = None;
            state.selected_decor = None;
            state.place_mode = false;
            return;
        }

        ui.add_space(8.0);

        // Place mode toggle
        ui.horizontal(|ui| {
            ui.label("Place:");
            egui::ComboBox::from_id_salt("decor_place_type")
                .selected_text(state.place_type.label())
                .width(90.0)
                .show_ui(ui, |ui| {
                    for dt in DecorType::ALL {
                        ui.selectable_value(&mut state.place_type, dt, dt.label());
                    }
                });
        });
        let place_label = if state.place_mode { "Stop Placing" } else { "Start Placing" };
        if ui.button(place_label).clicked() {
            state.place_mode = !state.place_mode;
        }

        ui.add_space(8.0);
        ui.separator();

        // List decor items in this room
        let decor_count = dungeon.graph.room_by_id(sel_room_id)
            .map(|r| r.decor.len())
            .unwrap_or(0);

        if decor_count == 0 {
            ui.label("No decorations. Use 'Start Placing' to add items.");
        } else {
            ui.label(format!("{} decoration(s):", decor_count));
            ui.add_space(4.0);

            let mut remove_idx = None;
            // Snapshot decor info to avoid borrow issues
            let decor_info: Vec<(usize, String, DecorType, f32, f32)> = dungeon.graph.room_by_id(sel_room_id)
                .map(|r| r.decor.iter().enumerate()
                    .map(|(i, d)| (i, d.id.clone(), d.decor_type, d.x, d.y))
                    .collect())
                .unwrap_or_default();

            for (di, _id, dt, _dx, _dy) in &decor_info {
                let is_sel = state.selected_decor == Some(*di);
                let frame = if is_sel {
                    egui::Frame::NONE
                        .inner_margin(4.0)
                        .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(255, 200, 50)))
                        .corner_radius(3.0)
                } else {
                    egui::Frame::NONE.inner_margin(4.0)
                };
                frame.show(ui, |ui| {
                    ui.horizontal(|ui| {
                        if ui.selectable_label(is_sel, dt.label()).clicked() {
                            state.selected_decor = Some(*di);
                        }
                        if ui.small_button("X").clicked() {
                            remove_idx = Some(*di);
                        }
                    });
                });
            }

            if let Some(idx) = remove_idx {
                if let Some(room) = dungeon.graph.room_by_id_mut(sel_room_id) {
                    room.decor.remove(idx);
                }
                if state.selected_decor == Some(idx) {
                    state.selected_decor = None;
                } else if let Some(sel) = state.selected_decor {
                    if sel > idx {
                        state.selected_decor = Some(sel - 1);
                    }
                }
            }
        }

        // Selected decor properties
        if let Some(sel_idx) = state.selected_decor {
            ui.add_space(8.0);
            ui.separator();
            ui.label("Properties:");

            if let Some(room) = dungeon.graph.room_by_id_mut(sel_room_id) {
                let (rw, rh) = room.grid_size();
                if sel_idx < room.decor.len() {
                    let decor = &mut room.decor[sel_idx];
                    ui.horizontal(|ui| {
                        ui.label("Type:");
                        egui::ComboBox::from_id_salt("decor_sel_type")
                            .selected_text(decor.decor_type.label())
                            .width(90.0)
                            .show_ui(ui, |ui| {
                                for dt in DecorType::ALL {
                                    ui.selectable_value(&mut decor.decor_type, dt, dt.label());
                                }
                            });
                    });
                    ui.horizontal(|ui| {
                        ui.add(egui::DragValue::new(&mut decor.x).range(0.0..=rw as f32).speed(0.1).prefix("x: "));
                        ui.add(egui::DragValue::new(&mut decor.y).range(0.0..=rh as f32).speed(0.1).prefix("y: "));
                    });
                    ui.add(egui::Slider::new(&mut decor.rotation, 0.0..=360.0).text("Rotation"));
                }
            }
        }
    } else {
        ui.heading("Decorations");
        ui.separator();
        ui.label("Select a room on the map to edit its decorations.");

        ui.add_space(12.0);

        // Summary of rooms with decor
        let rooms_with_decor: Vec<_> = dungeon.graph.rooms.iter()
            .filter(|r| !r.decor.is_empty())
            .map(|r| (r.id.clone(), r.label.clone(), r.decor.len()))
            .collect();

        if rooms_with_decor.is_empty() {
            ui.label("No rooms have decorations yet.");
        } else {
            ui.label(format!("{} room(s) with decorations:", rooms_with_decor.len()));
            for (id, label, count) in &rooms_with_decor {
                if ui.button(format!("{} ({})", label, count)).clicked() {
                    state.selected_room = Some(id.clone());
                }
            }
        }
    }

    // Floor selector (always available)
    ui.add_space(16.0);
    ui.separator();
    ui.label("Floor:");
    {
        let floors = collect_floors(&dungeon.graph);
        let label = match state.current_floor {
            None => "All Floors".to_string(),
            Some(f) => format!("Floor {}", f),
        };
        egui::ComboBox::from_id_salt("decor_floor_select")
            .selected_text(&label)
            .show_ui(ui, |ui| {
                if ui.selectable_value(&mut state.current_floor, None, "All Floors").changed() {}
                for f in &floors {
                    let mut val = Some(*f);
                    if ui.selectable_value(&mut val, Some(*f), format!("Floor {}", f)).clicked() {
                        state.current_floor = Some(*f);
                    }
                }
            });
    }
}
