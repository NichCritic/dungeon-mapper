use std::hash::{Hash, Hasher};

use crate::model::*;
use crate::render::recording::replay_commands;
use crate::render::themed::RenderOptions;
use crate::ui::canvas_common::{handle_pan_zoom, ViewState, COLOR_PLACEHOLDER_TEXT};
use crate::ui::spatial_view::collect_floors;
use crate::util::{ViewTransform, GRID_PX};

use crate::render::bg_cache::BackgroundRenderCache;

pub struct DecorViewState {
    pub view: ViewState,
    pub render_cache: BackgroundRenderCache,
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
    /// Multiple selected decor items (for drag-select).
    pub selected_decor_set: std::collections::HashSet<usize>,
    /// Drag-select start position in world coords.
    drag_select_start: Option<egui::Pos2>,
    /// Search filter for decor type dropdowns.
    pub decor_search: String,
}

impl Default for DecorViewState {
    fn default() -> Self {
        Self {
            view: ViewState::default(),
            render_cache: BackgroundRenderCache::default(),
            current_floor: None,
            selected_room: None,
            dragging_decor: None,
            place_type: DecorType::Table,
            place_mode: false,
            selected_decor: None,
            selected_decor_set: std::collections::HashSet::new(),
            drag_select_start: None,
            decor_search: String::new(),
        }
    }
}

pub fn render_cache_hash(layout: &SpatialLayout, graph: &DungeonGraph, theme: &Theme, current_floor: Option<i32>) -> u64 {
    render_input_hash(layout, graph, theme, current_floor)
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
                s.length.to_bits().hash(&mut h);
                s.height.to_bits().hash(&mut h);
                std::mem::discriminant(&s.elevation).hash(&mut h);
            }
            // NOTE: decor is intentionally excluded from this hash.
            // Decor is drawn as a live overlay in the decor view so that
            // dragging doesn't trigger expensive cache rebuilds every frame.
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
    let options = RenderOptions {
        show_grid: true,
        show_labels: true,
        show_notes: false,
        show_secrets: false,
        show_decor: false, // decor drawn as live overlay for smooth dragging
    };
    let cache_ready = state.render_cache.ensure(
        hash, &dungeon.graph, render_layout, &dungeon.theme, options, "Decor",
    );

    if cache_ready {
        if let Some(commands) = state.render_cache.commands() {
            replay_commands(&painter, &transform, commands);
        }
    } else {
        let msg = format!("Rendering {}...",
            state.render_cache.pending_label().unwrap_or("map"));
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

    // Live decor overlay (not cached, so dragging is smooth)
    let decor_color = egui::Color32::from_rgb(
        dungeon.theme.wall_color[0], dungeon.theme.wall_color[1], dungeon.theme.wall_color[2],
    );
    for rl in &render_layout.rooms {
        if let Some(room) = dungeon.graph.room_by_id(&rl.room_id) {
            let room_px_x = rl.x as f32 * GRID_PX;
            let room_px_y = rl.y as f32 * GRID_PX;
            for decor in &room.decor {
                let wx = room_px_x + decor.x * GRID_PX;
                let wy = room_px_y + decor.y * GRID_PX;
                let screen_center = transform.world_to_screen(egui::pos2(wx, wy));
                let s = GRID_PX * 0.4 * decor.scale * transform.zoom;
                let deg = decor.rotation;

                draw_decor_symbol(&painter, screen_center, s, deg, decor.decor_type, decor_color);
            }
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
                    // Draw selection ring and type label for selected room's decor
                    let is_sel = state.selected_decor == Some(di) || state.selected_decor_set.contains(&di);
                    let ring_color = if is_sel {
                        egui::Color32::from_rgb(255, 200, 50)
                    } else {
                        egui::Color32::from_rgb(100, 180, 255)
                    };
                    painter.circle_stroke(screen, handle_r + 2.0, egui::Stroke::new(1.5, ring_color));
                    // Type label only for selected item
                    if is_sel {
                        painter.text(
                            screen + egui::vec2(0.0, -handle_r - 6.0),
                            egui::Align2::CENTER_BOTTOM,
                            decor.decor_type.label(),
                            egui::FontId::proportional((9.0 * transform.zoom).max(7.0)),
                            ring_color,
                        );
                    }
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

    // Delete selected decor with Delete or Backspace key
    if state.selected_room.is_some() && (!state.selected_decor_set.is_empty() || state.selected_decor.is_some()) {
        let delete_pressed = ui.ctx().input(|i| {
            i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace)
        });
        if delete_pressed {
            let sel_id = state.selected_room.clone().unwrap();
            if let Some(room) = dungeon.graph.room_by_id_mut(&sel_id) {
                // Collect all indices to remove
                let mut to_remove: Vec<usize> = state.selected_decor_set.iter().copied().collect();
                if let Some(di) = state.selected_decor {
                    if !to_remove.contains(&di) { to_remove.push(di); }
                }
                to_remove.sort_unstable_by(|a, b| b.cmp(a)); // reverse order
                for idx in to_remove {
                    if idx < room.decor.len() {
                        room.decor.remove(idx);
                    }
                }
                state.selected_decor = None;
                state.selected_decor_set.clear();
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

    // Drag-select: secondary button (right-drag) to rubber-band select
    if !state.place_mode && state.dragging_decor.is_none() {
        if response.drag_started_by(egui::PointerButton::Secondary) {
            if let Some(pos) = response.interact_pointer_pos() {
                state.drag_select_start = Some(transform.screen_to_world(pos));
            }
        }
        if let Some(start) = state.drag_select_start {
            if response.dragged_by(egui::PointerButton::Secondary) {
                if let Some(pos) = pointer_pos {
                    let current = transform.screen_to_world(pos);
                    let min = egui::pos2(start.x.min(current.x), start.y.min(current.y));
                    let max = egui::pos2(start.x.max(current.x), start.y.max(current.y));
                    let screen_min = transform.world_to_screen(min);
                    let screen_max = transform.world_to_screen(max);
                    painter.rect_stroke(
                        egui::Rect::from_min_max(screen_min, screen_max),
                        0.0,
                        egui::Stroke::new(1.0, egui::Color32::from_rgb(100, 200, 255)),
                        egui::StrokeKind::Outside,
                    );
                }
            }
            if response.drag_stopped_by(egui::PointerButton::Secondary) {
                if let Some(pos) = pointer_pos {
                    let end = transform.screen_to_world(pos);
                    let min_x = start.x.min(end.x);
                    let min_y = start.y.min(end.y);
                    let max_x = start.x.max(end.x);
                    let max_y = start.y.max(end.y);
                    // Select all decor items within the rectangle
                    state.selected_decor_set.clear();
                    if let Some(ref sel_id) = state.selected_room {
                        if let Some(rl) = render_layout.room_by_id(sel_id) {
                            if let Some(room) = dungeon.graph.room_by_id(sel_id) {
                                let room_px_x = rl.x as f32 * GRID_PX;
                                let room_px_y = rl.y as f32 * GRID_PX;
                                for (di, decor) in room.decor.iter().enumerate() {
                                    let wx = room_px_x + decor.x * GRID_PX;
                                    let wy = room_px_y + decor.y * GRID_PX;
                                    if wx >= min_x && wx <= max_x && wy >= min_y && wy <= max_y {
                                        state.selected_decor_set.insert(di);
                                    }
                                }
                            }
                        }
                    }
                    if let Some(&first) = state.selected_decor_set.iter().next() {
                        state.selected_decor = Some(first);
                    }
                }
                state.drag_select_start = None;
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
            decor_type_combo(ui, "decor_place_type", &mut state.place_type, &mut state.decor_search);
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
                if sel_idx < room.decor.len() {
                    let decor = &mut room.decor[sel_idx];
                    ui.horizontal(|ui| {
                        ui.label("Type:");
                        decor_type_combo(ui, "decor_sel_type", &mut decor.decor_type, &mut state.decor_search);
                    });
                    ui.horizontal(|ui| {
                        ui.label("x:");
                        crate::ui::canvas_common::num_input_f32(ui, &mut decor.x, 35.0);
                        ui.label("y:");
                        crate::ui::canvas_common::num_input_f32(ui, &mut decor.y, 35.0);
                    });
                    ui.add(egui::Slider::new(&mut decor.rotation, 0.0..=360.0).text("Rotation"));
                    ui.add(egui::Slider::new(&mut decor.scale, 0.2..=3.0).text("Size"));
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

    // Lighting
    ui.add_space(12.0);
    ui.heading("Lighting");
    ui.separator();

    ui.add(egui::Slider::new(&mut dungeon.ambient_light, 0.0..=1.0).text("Ambient"));

    if let Some(ref sel_room_id) = state.selected_room.clone() {
        if ui.button("Add Light Here").clicked() {
            dungeon.light_sources.push(crate::model::LightSource {
                id: uuid::Uuid::new_v4().to_string(),
                room_id: sel_room_id.clone(),
                radius: 5.0,
                intensity: 1.0,
                color: [255, 200, 100],
            });
        }
        let room_light_indices: Vec<usize> = dungeon.light_sources.iter().enumerate()
            .filter(|(_, l)| l.room_id == *sel_room_id)
            .map(|(i, _)| i)
            .collect();
        let mut remove_light = None;
        for &li in &room_light_indices {
            let light = &mut dungeon.light_sources[li];
            ui.horizontal(|ui| {
                ui.add(egui::Slider::new(&mut light.radius, 1.0..=20.0).text("R"));
                ui.add(egui::Slider::new(&mut light.intensity, 0.0..=1.0).text("I"));
                if ui.small_button("X").clicked() {
                    remove_light = Some(li);
                }
            });
        }
        if let Some(idx) = remove_light {
            dungeon.light_sources.remove(idx);
        }
    } else {
        if ui.button("Add Light Source").clicked() {
            let room_id = dungeon.graph.rooms.first()
                .map(|r| r.id.clone())
                .unwrap_or_default();
            if !room_id.is_empty() {
                dungeon.light_sources.push(crate::model::LightSource {
                    id: uuid::Uuid::new_v4().to_string(),
                    room_id,
                    radius: 5.0,
                    intensity: 1.0,
                    color: [255, 200, 100],
                });
            }
        }
        let mut remove_idx = None;
        for (i, light) in dungeon.light_sources.iter_mut().enumerate() {
            ui.horizontal(|ui| {
                let room_label = dungeon.graph.room_by_id(&light.room_id)
                    .map(|r| r.label.as_str())
                    .unwrap_or("?");
                ui.label(format!("Light in {}", room_label));
                if ui.small_button("X").clicked() {
                    remove_idx = Some(i);
                }
            });
            ui.add(egui::Slider::new(&mut light.radius, 1.0..=20.0).text("Radius"));
            ui.add(egui::Slider::new(&mut light.intensity, 0.0..=1.0).text("Intensity"));
            let rooms: Vec<_> = dungeon.graph.rooms.iter().map(|r| (r.id.clone(), r.label.clone())).collect();
            egui::ComboBox::from_id_salt(format!("light_room_{}", light.id))
                .selected_text(
                    dungeon.graph.room_by_id(&light.room_id)
                        .map(|r| r.label.as_str())
                        .unwrap_or("Select room"),
                )
                .show_ui(ui, |ui| {
                    for (rid, rlabel) in &rooms {
                        ui.selectable_value(&mut light.room_id, rid.clone(), rlabel);
                    }
                });
            ui.separator();
        }
        if let Some(idx) = remove_idx {
            dungeon.light_sources.remove(idx);
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

/// Fuzzy-searchable dropdown for DecorType selection.
fn decor_type_combo(ui: &mut egui::Ui, id: &str, value: &mut DecorType, search: &mut String) {
    egui::ComboBox::from_id_salt(id)
        .selected_text(value.label())
        .width(110.0)
        .show_ui(ui, |ui| {
            ui.add(egui::TextEdit::singleline(search).hint_text("Search...").desired_width(100.0));
            let filter = search.to_lowercase();
            egui::ScrollArea::vertical().max_height(200.0).show(ui, |ui| {
                for dt in DecorType::ALL {
                    if !filter.is_empty() && !dt.label().to_lowercase().contains(&filter) {
                        continue;
                    }
                    if ui.selectable_value(value, dt, dt.label()).clicked() {
                        search.clear();
                    }
                }
            });
        });
}

/// Rotate a screen-space point around a center by deg degrees.
fn rot_screen(px: f32, py: f32, cx: f32, cy: f32, deg: f32) -> egui::Pos2 {
    let rad = deg.to_radians();
    let cos = rad.cos();
    let sin = rad.sin();
    let dx = px - cx;
    let dy = py - cy;
    egui::pos2(cx + dx * cos - dy * sin, cy + dx * sin + dy * cos)
}

fn rot_line_screen(
    painter: &egui::Painter, x1: f32, y1: f32, x2: f32, y2: f32,
    cx: f32, cy: f32, deg: f32, stroke: egui::Stroke,
) {
    let a = rot_screen(x1, y1, cx, cy, deg);
    let b = rot_screen(x2, y2, cx, cy, deg);
    painter.line_segment([a, b], stroke);
}

/// Draw a decor symbol in screen space using egui painter primitives.
fn draw_decor_symbol(
    painter: &egui::Painter,
    center: egui::Pos2,
    s: f32, // half-size in screen pixels
    deg: f32,
    decor_type: DecorType,
    color: egui::Color32,
) {
    let cx = center.x;
    let cy = center.y;
    let stroke = egui::Stroke::new(1.5, color);
    let thin = egui::Stroke::new(0.8, color);
    match decor_type {
        DecorType::Table => {
            let hw = s;
            let hh = s * 0.6;
            // Tabletop
            rot_line_screen(painter, cx - hw, cy - hh, cx + hw, cy - hh, cx, cy, deg, stroke);
            rot_line_screen(painter, cx + hw, cy - hh, cx + hw, cy + hh, cx, cy, deg, stroke);
            rot_line_screen(painter, cx + hw, cy + hh, cx - hw, cy + hh, cx, cy, deg, stroke);
            rot_line_screen(painter, cx - hw, cy + hh, cx - hw, cy - hh, cx, cy, deg, stroke);
            // Legs
            let lr = s * 0.08;
            for &(lx, ly) in &[(-hw + lr * 2.0, -hh + lr * 2.0), (hw - lr * 2.0, -hh + lr * 2.0),
                                 (-hw + lr * 2.0, hh - lr * 2.0), (hw - lr * 2.0, hh - lr * 2.0)] {
                let p = rot_screen(cx + lx, cy + ly, cx, cy, deg);
                painter.circle_filled(p, lr, color);
            }
        }
        DecorType::Chest => {
            let cs = s * 0.6;
            rot_line_screen(painter, cx - cs, cy - cs, cx + cs, cy - cs, cx, cy, deg, stroke);
            rot_line_screen(painter, cx + cs, cy - cs, cx + cs, cy + cs, cx, cy, deg, stroke);
            rot_line_screen(painter, cx + cs, cy + cs, cx - cs, cy + cs, cx, cy, deg, stroke);
            rot_line_screen(painter, cx - cs, cy + cs, cx - cs, cy - cs, cx, cy, deg, stroke);
            // Lid line
            rot_line_screen(painter, cx - cs, cy - cs * 0.2, cx + cs, cy - cs * 0.2, cx, cy, deg, thin);
            // Clasp
            let clasp = rot_screen(cx, cy + cs, cx, cy, deg);
            painter.circle_filled(clasp, s * 0.08, color);
        }
        DecorType::Pillar => {
            painter.circle_filled(center, s * 0.5, color);
            let light = egui::Color32::from_rgba_unmultiplied(
                color.r().saturating_add(40), color.g().saturating_add(40), color.b().saturating_add(40), color.a());
            painter.circle_stroke(center, s * 0.5, egui::Stroke::new(1.0, light));
            painter.circle_stroke(center, s * 0.35, egui::Stroke::new(0.5, light));
        }
        DecorType::StairsUp | DecorType::StairsDown => {
            // Box
            rot_line_screen(painter, cx - s, cy - s, cx + s, cy - s, cx, cy, deg, stroke);
            rot_line_screen(painter, cx + s, cy - s, cx + s, cy + s, cx, cy, deg, stroke);
            rot_line_screen(painter, cx + s, cy + s, cx - s, cy + s, cx, cy, deg, stroke);
            rot_line_screen(painter, cx - s, cy + s, cx - s, cy - s, cx, cy, deg, stroke);
            // Treads
            let steps = 5;
            for i in 1..steps {
                let y = cy - s + (i as f32 / steps as f32) * s * 2.0;
                rot_line_screen(painter, cx - s, y, cx + s, y, cx, cy, deg, thin);
            }
            // Arrow
            let (arrow_tip, arrow_l, arrow_r) = if decor_type == DecorType::StairsUp {
                (cy - s * 0.9, cy - s * 0.5, cy - s * 0.5)
            } else {
                (cy + s * 0.9, cy + s * 0.5, cy + s * 0.5)
            };
            rot_line_screen(painter, cx, arrow_tip, cx - s * 0.3, arrow_l, cx, cy, deg, stroke);
            rot_line_screen(painter, cx, arrow_tip, cx + s * 0.3, arrow_r, cx, cy, deg, stroke);
        }
        DecorType::Altar => {
            // Platform
            rot_line_screen(painter, cx - s * 0.8, cy + s * 0.6, cx + s * 0.8, cy + s * 0.6, cx, cy, deg, stroke);
            rot_line_screen(painter, cx - s * 0.8, cy + s * 0.8, cx + s * 0.8, cy + s * 0.8, cx, cy, deg, stroke);
            rot_line_screen(painter, cx - s * 0.8, cy + s * 0.6, cx - s * 0.8, cy + s * 0.8, cx, cy, deg, stroke);
            rot_line_screen(painter, cx + s * 0.8, cy + s * 0.6, cx + s * 0.8, cy + s * 0.8, cx, cy, deg, stroke);
            // Cross
            let t = s * 0.2;
            rot_line_screen(painter, cx - t, cy - s, cx + t, cy - s, cx, cy, deg, thin);
            rot_line_screen(painter, cx + t, cy - s, cx + t, cy + s * 0.5, cx, cy, deg, thin);
            rot_line_screen(painter, cx + t, cy + s * 0.5, cx - t, cy + s * 0.5, cx, cy, deg, thin);
            rot_line_screen(painter, cx - t, cy + s * 0.5, cx - t, cy - s, cx, cy, deg, thin);
            rot_line_screen(painter, cx - s * 0.6, cy - t * 1.5, cx + s * 0.6, cy - t * 1.5, cx, cy, deg, thin);
            rot_line_screen(painter, cx - s * 0.6, cy + t * 0.5, cx + s * 0.6, cy + t * 0.5, cx, cy, deg, thin);
            rot_line_screen(painter, cx - s * 0.6, cy - t * 1.5, cx - s * 0.6, cy + t * 0.5, cx, cy, deg, thin);
            rot_line_screen(painter, cx + s * 0.6, cy - t * 1.5, cx + s * 0.6, cy + t * 0.5, cx, cy, deg, thin);
        }
        DecorType::Fountain => {
            painter.circle_stroke(center, s * 0.85, stroke);
            painter.circle_stroke(center, s * 0.55, thin);
            painter.circle_stroke(center, s * 0.25, thin);
            painter.circle_filled(center, s * 0.08, color);
        }
        DecorType::Trap => {
            // Triangle with X
            rot_line_screen(painter, cx, cy - s * 0.7, cx + s * 0.7, cy + s * 0.5, cx, cy, deg, stroke);
            rot_line_screen(painter, cx + s * 0.7, cy + s * 0.5, cx - s * 0.7, cy + s * 0.5, cx, cy, deg, stroke);
            rot_line_screen(painter, cx - s * 0.7, cy + s * 0.5, cx, cy - s * 0.7, cx, cy, deg, stroke);
            rot_line_screen(painter, cx - s * 0.25, cy - s * 0.1, cx + s * 0.25, cy + s * 0.35, cx, cy, deg, thin);
            rot_line_screen(painter, cx + s * 0.25, cy - s * 0.1, cx - s * 0.25, cy + s * 0.35, cx, cy, deg, thin);
        }
        DecorType::Rubble => {
            let rocks: [(f32, f32, f32); 7] = [
                (0.0, 0.0, 0.18), (-0.45, -0.35, 0.14), (0.35, -0.25, 0.16),
                (-0.25, 0.4, 0.12), (0.4, 0.35, 0.13), (-0.15, -0.45, 0.1),
                (0.2, 0.15, 0.11),
            ];
            for &(dx, dy, r) in &rocks {
                let p = rot_screen(cx + dx * s, cy + dy * s, cx, cy, deg);
                painter.circle_filled(p, r * s, color);
            }
        }
        DecorType::Chair => {
            let cs = s * 0.4;
            rot_line_screen(painter, cx - cs, cy - cs, cx + cs, cy - cs, cx, cy, deg, stroke);
            rot_line_screen(painter, cx + cs, cy - cs, cx + cs, cy + cs, cx, cy, deg, stroke);
            rot_line_screen(painter, cx + cs, cy + cs, cx - cs, cy + cs, cx, cy, deg, stroke);
            rot_line_screen(painter, cx - cs, cy + cs, cx - cs, cy - cs, cx, cy, deg, stroke);
            rot_line_screen(painter, cx - cs, cy - cs, cx + cs, cy - cs, cx, cy, deg, egui::Stroke::new(2.5, color));
        }
        DecorType::Bench => {
            let hw = s * 0.9;
            let hh = s * 0.25;
            rot_line_screen(painter, cx - hw, cy - hh, cx + hw, cy - hh, cx, cy, deg, stroke);
            rot_line_screen(painter, cx + hw, cy - hh, cx + hw, cy + hh, cx, cy, deg, stroke);
            rot_line_screen(painter, cx + hw, cy + hh, cx - hw, cy + hh, cx, cy, deg, stroke);
            rot_line_screen(painter, cx - hw, cy + hh, cx - hw, cy - hh, cx, cy, deg, stroke);
        }
        DecorType::Barrel => {
            painter.circle_stroke(center, s * 0.55, stroke);
            rot_line_screen(painter, cx - s * 0.5, cy - s * 0.15, cx + s * 0.5, cy - s * 0.15, cx, cy, deg, thin);
            rot_line_screen(painter, cx - s * 0.5, cy + s * 0.15, cx + s * 0.5, cy + s * 0.15, cx, cy, deg, thin);
        }
        DecorType::Crate => {
            let cs = s * 0.55;
            rot_line_screen(painter, cx - cs, cy - cs, cx + cs, cy - cs, cx, cy, deg, stroke);
            rot_line_screen(painter, cx + cs, cy - cs, cx + cs, cy + cs, cx, cy, deg, stroke);
            rot_line_screen(painter, cx + cs, cy + cs, cx - cs, cy + cs, cx, cy, deg, stroke);
            rot_line_screen(painter, cx - cs, cy + cs, cx - cs, cy - cs, cx, cy, deg, stroke);
            rot_line_screen(painter, cx - cs, cy - cs, cx + cs, cy + cs, cx, cy, deg, thin);
            rot_line_screen(painter, cx + cs, cy - cs, cx - cs, cy + cs, cx, cy, deg, thin);
        }
        DecorType::Ladder => {
            let hw = s * 0.3;
            rot_line_screen(painter, cx - hw, cy - s, cx - hw, cy + s, cx, cy, deg, stroke);
            rot_line_screen(painter, cx + hw, cy - s, cx + hw, cy + s, cx, cy, deg, stroke);
            for i in 0..5 {
                let y = cy - s + (i as f32 + 0.5) * s * 2.0 / 5.0;
                rot_line_screen(painter, cx - hw, y, cx + hw, y, cx, cy, deg, thin);
            }
        }
        DecorType::Well => {
            painter.circle_stroke(center, s * 0.7, stroke);
            painter.circle_stroke(center, s * 0.5, thin);
            rot_line_screen(painter, cx - s * 0.7, cy, cx + s * 0.7, cy, cx, cy, deg, thin);
            rot_line_screen(painter, cx, cy - s * 0.7, cx, cy + s * 0.7, cx, cy, deg, thin);
        }
        DecorType::Brazier => {
            painter.circle_stroke(center, s * 0.4, stroke);
            painter.circle_filled(center, s * 0.25, color);
            rot_line_screen(painter, cx - s * 0.3, cy + s * 0.3, cx - s * 0.5, cy + s * 0.7, cx, cy, deg, thin);
            rot_line_screen(painter, cx + s * 0.3, cy + s * 0.3, cx + s * 0.5, cy + s * 0.7, cx, cy, deg, thin);
            rot_line_screen(painter, cx, cy + s * 0.4, cx, cy + s * 0.7, cx, cy, deg, thin);
        }
        DecorType::Fireplace => {
            rot_line_screen(painter, cx - s * 0.7, cy - s * 0.6, cx - s * 0.7, cy + s * 0.6, cx, cy, deg, egui::Stroke::new(2.0, color));
            rot_line_screen(painter, cx + s * 0.7, cy - s * 0.6, cx + s * 0.7, cy + s * 0.6, cx, cy, deg, egui::Stroke::new(2.0, color));
            rot_line_screen(painter, cx - s * 0.7, cy + s * 0.6, cx + s * 0.7, cy + s * 0.6, cx, cy, deg, egui::Stroke::new(2.0, color));
            rot_line_screen(painter, cx, cy + s * 0.3, cx - s * 0.15, cy - s * 0.2, cx, cy, deg, thin);
            rot_line_screen(painter, cx, cy + s * 0.3, cx + s * 0.15, cy - s * 0.2, cx, cy, deg, thin);
        }
        DecorType::Statue => {
            let p = rot_screen(cx, cy - s * 0.15, cx, cy, deg);
            painter.circle_filled(p, s * 0.4, color);
            let bs = s * 0.5;
            rot_line_screen(painter, cx - bs, cy + s * 0.3, cx + bs, cy + s * 0.3, cx, cy, deg, stroke);
            rot_line_screen(painter, cx + bs, cy + s * 0.3, cx + bs, cy + s * 0.7, cx, cy, deg, stroke);
            rot_line_screen(painter, cx + bs, cy + s * 0.7, cx - bs, cy + s * 0.7, cx, cy, deg, stroke);
            rot_line_screen(painter, cx - bs, cy + s * 0.7, cx - bs, cy + s * 0.3, cx, cy, deg, stroke);
        }
        DecorType::Throne => {
            let cs = s * 0.5;
            rot_line_screen(painter, cx - cs, cy - cs, cx + cs, cy - cs, cx, cy, deg, thin);
            rot_line_screen(painter, cx + cs, cy - cs, cx + cs, cy + cs, cx, cy, deg, thin);
            rot_line_screen(painter, cx + cs, cy + cs, cx - cs, cy + cs, cx, cy, deg, thin);
            rot_line_screen(painter, cx - cs, cy + cs, cx - cs, cy - cs, cx, cy, deg, thin);
            rot_line_screen(painter, cx - cs, cy - cs, cx - cs, cy - s * 0.9, cx, cy, deg, egui::Stroke::new(2.0, color));
            rot_line_screen(painter, cx + cs, cy - cs, cx + cs, cy - s * 0.9, cx, cy, deg, egui::Stroke::new(2.0, color));
            rot_line_screen(painter, cx - cs, cy - s * 0.9, cx + cs, cy - s * 0.9, cx, cy, deg, egui::Stroke::new(2.0, color));
            rot_line_screen(painter, cx - cs, cy, cx - s * 0.8, cy, cx, cy, deg, stroke);
            rot_line_screen(painter, cx + cs, cy, cx + s * 0.8, cy, cx, cy, deg, stroke);
        }
        DecorType::Bed => {
            let hw = s * 0.6;
            let hh = s * 0.9;
            rot_line_screen(painter, cx - hw, cy - hh, cx + hw, cy - hh, cx, cy, deg, thin);
            rot_line_screen(painter, cx + hw, cy - hh, cx + hw, cy + hh, cx, cy, deg, thin);
            rot_line_screen(painter, cx + hw, cy + hh, cx - hw, cy + hh, cx, cy, deg, thin);
            rot_line_screen(painter, cx - hw, cy + hh, cx - hw, cy - hh, cx, cy, deg, thin);
            rot_line_screen(painter, cx - hw, cy - hh, cx + hw, cy - hh, cx, cy, deg, egui::Stroke::new(3.0, color));
            rot_line_screen(painter, cx - hw * 0.6, cy - hh + s * 0.3, cx + hw * 0.6, cy - hh + s * 0.3, cx, cy, deg, thin);
        }
        DecorType::Bookshelf => {
            let hw = s * 0.8;
            let hh = s * 0.4;
            rot_line_screen(painter, cx - hw, cy - hh, cx + hw, cy - hh, cx, cy, deg, stroke);
            rot_line_screen(painter, cx + hw, cy - hh, cx + hw, cy + hh, cx, cy, deg, stroke);
            rot_line_screen(painter, cx + hw, cy + hh, cx - hw, cy + hh, cx, cy, deg, stroke);
            rot_line_screen(painter, cx - hw, cy + hh, cx - hw, cy - hh, cx, cy, deg, stroke);
            for i in 1..3 {
                let y = cy - hh + (i as f32 / 3.0) * hh * 2.0;
                rot_line_screen(painter, cx - hw, y, cx + hw, y, cx, cy, deg, thin);
            }
        }
        DecorType::Bones => {
            rot_line_screen(painter, cx - s * 0.6, cy - s * 0.4, cx + s * 0.6, cy + s * 0.4, cx, cy, deg, stroke);
            rot_line_screen(painter, cx + s * 0.6, cy - s * 0.4, cx - s * 0.6, cy + s * 0.4, cx, cy, deg, stroke);
            for &(bx, by) in &[(-0.6f32, -0.4f32), (0.6, 0.4), (0.6, -0.4), (-0.6, 0.4)] {
                let p = rot_screen(cx + bx * s, cy + by * s, cx, cy, deg);
                painter.circle_filled(p, s * 0.1, color);
            }
            let skull = rot_screen(cx, cy - s * 0.1, cx, cy, deg);
            painter.circle_stroke(skull, s * 0.2, thin);
        }
        DecorType::Web => {
            for i in 0..6 {
                let angle = (i as f32 / 6.0) * std::f32::consts::TAU;
                let ex = cx + s * 0.8 * angle.cos();
                let ey = cy + s * 0.8 * angle.sin();
                rot_line_screen(painter, cx, cy, ex, ey, cx, cy, deg, thin);
            }
            for ring in 1..3 {
                let r = s * 0.8 * ring as f32 / 3.0;
                for i in 0..6 {
                    let a1 = (i as f32 / 6.0) * std::f32::consts::TAU;
                    let a2 = ((i + 1) as f32 / 6.0) * std::f32::consts::TAU;
                    let p1 = rot_screen(cx + r * a1.cos(), cy + r * a1.sin(), cx, cy, deg);
                    let p2 = rot_screen(cx + r * a2.cos(), cy + r * a2.sin(), cx, cy, deg);
                    painter.line_segment([p1, p2], egui::Stroke::new(0.5, color));
                }
            }
        }
        DecorType::Door => {
            let hw = s * 0.05;
            rot_line_screen(painter, cx - hw, cy - s * 0.7, cx - hw, cy + s * 0.7, cx, cy, deg, egui::Stroke::new(2.0, color));
            rot_line_screen(painter, cx - hw, cy - s * 0.7, cx + s * 0.7, cy - s * 0.7, cx, cy, deg, stroke);
            rot_line_screen(painter, cx + s * 0.7, cy - s * 0.7, cx + s * 0.7, cy + s * 0.7, cx, cy, deg, stroke);
            rot_line_screen(painter, cx + s * 0.7, cy + s * 0.7, cx - hw, cy + s * 0.7, cx, cy, deg, stroke);
        }
        DecorType::Gate => {
            for i in 0..5 {
                let x = cx - s * 0.6 + (i as f32 / 4.0) * s * 1.2;
                rot_line_screen(painter, x, cy - s * 0.7, x, cy + s * 0.7, cx, cy, deg, thin);
            }
            rot_line_screen(painter, cx - s * 0.6, cy - s * 0.2, cx + s * 0.6, cy - s * 0.2, cx, cy, deg, thin);
            rot_line_screen(painter, cx - s * 0.6, cy + s * 0.2, cx + s * 0.6, cy + s * 0.2, cx, cy, deg, thin);
            rot_line_screen(painter, cx - s * 0.7, cy - s * 0.8, cx + s * 0.7, cy - s * 0.8, cx, cy, deg, egui::Stroke::new(2.0, color));
            rot_line_screen(painter, cx - s * 0.7, cy - s * 0.8, cx - s * 0.7, cy + s * 0.7, cx, cy, deg, egui::Stroke::new(2.0, color));
            rot_line_screen(painter, cx + s * 0.7, cy - s * 0.8, cx + s * 0.7, cy + s * 0.7, cx, cy, deg, egui::Stroke::new(2.0, color));
        }
        DecorType::Vines => {
            let tendrils: [(f32, f32, f32, f32, f32, f32); 5] = [
                (0.0, 0.0, -0.4, -0.6, -0.7, -0.8),
                (0.0, 0.0,  0.5, -0.4,  0.8, -0.7),
                (0.0, 0.0, -0.6,  0.3, -0.8,  0.6),
                (0.0, 0.0,  0.3,  0.5,  0.6,  0.8),
                (0.0, 0.0,  0.1, -0.3, -0.2, -0.9),
            ];
            for &(sx, sy, cx1, cy1, ex, ey) in &tendrils {
                let steps = 6;
                let mut prev = rot_screen(cx + sx * s, cy + sy * s, cx, cy, deg);
                for i in 1..=steps {
                    let t = i as f32 / steps as f32;
                    let it = 1.0 - t;
                    let nx = cx + (it * it * sx + 2.0 * it * t * cx1 + t * t * ex) * s;
                    let ny = cy + (it * it * sy + 2.0 * it * t * cy1 + t * t * ey) * s;
                    let cur = rot_screen(nx, ny, cx, cy, deg);
                    painter.line_segment([prev, cur], thin);
                    prev = cur;
                }
                // Leaf near end
                let leaf_t = 0.7;
                let it = 1.0 - leaf_t;
                let lx = cx + (it * it * sx + 2.0 * it * leaf_t * cx1 + leaf_t * leaf_t * ex) * s;
                let ly = cy + (it * it * sy + 2.0 * it * leaf_t * cy1 + leaf_t * leaf_t * ey) * s;
                let ls = s * 0.12;
                rot_line_screen(painter, lx - ls, ly, lx, ly - ls, cx, cy, deg, thin);
                rot_line_screen(painter, lx, ly - ls, lx + ls, ly, cx, cy, deg, thin);
                rot_line_screen(painter, lx - ls, ly, lx + ls, ly, cx, cy, deg, egui::Stroke::new(0.5, color));
            }
        }
        DecorType::Scales => {
            // Central stand
            rot_line_screen(painter, cx, cy - s * 0.9, cx, cy + s * 0.8, cx, cy, deg, stroke);
            // Base
            rot_line_screen(painter, cx - s * 0.4, cy + s * 0.8, cx + s * 0.4, cy + s * 0.8, cx, cy, deg, egui::Stroke::new(2.0, color));
            // Crossbeam
            rot_line_screen(painter, cx - s * 0.8, cy - s * 0.5, cx + s * 0.8, cy - s * 0.7, cx, cy, deg, stroke);
            // Pivot triangle
            rot_line_screen(painter, cx - s * 0.1, cy - s * 0.9, cx + s * 0.1, cy - s * 0.9, cx, cy, deg, thin);
            rot_line_screen(painter, cx - s * 0.1, cy - s * 0.9, cx, cy - s * 0.7, cx, cy, deg, thin);
            rot_line_screen(painter, cx + s * 0.1, cy - s * 0.9, cx, cy - s * 0.7, cx, cy, deg, thin);
            // Left chain + pan
            rot_line_screen(painter, cx - s * 0.8, cy - s * 0.5, cx - s * 0.8, cy + s * 0.1, cx, cy, deg, thin);
            rot_line_screen(painter, cx - s * 1.05, cy + s * 0.1, cx - s * 0.55, cy + s * 0.1, cx, cy, deg, thin);
            for i in 0..6 {
                let a0 = std::f32::consts::PI * (i as f32 / 6.0);
                let a1 = std::f32::consts::PI * ((i + 1) as f32 / 6.0);
                let pr = s * 0.25;
                let pcx = cx - s * 0.8;
                let pcy = cy + s * 0.1;
                rot_line_screen(painter,
                    pcx + pr * a0.cos(), pcy + pr * a0.sin(),
                    pcx + pr * a1.cos(), pcy + pr * a1.sin(),
                    cx, cy, deg, thin);
            }
            // Right chain + pan
            rot_line_screen(painter, cx + s * 0.8, cy - s * 0.7, cx + s * 0.8, cy - s * 0.1, cx, cy, deg, thin);
            rot_line_screen(painter, cx + s * 0.55, cy - s * 0.1, cx + s * 1.05, cy - s * 0.1, cx, cy, deg, thin);
            for i in 0..6 {
                let a0 = std::f32::consts::PI * (i as f32 / 6.0);
                let a1 = std::f32::consts::PI * ((i + 1) as f32 / 6.0);
                let pr = s * 0.25;
                let pcx = cx + s * 0.8;
                let pcy = cy - s * 0.1;
                rot_line_screen(painter,
                    pcx + pr * a0.cos(), pcy + pr * a0.sin(),
                    pcx + pr * a1.cos(), pcy + pr * a1.sin(),
                    cx, cy, deg, thin);
            }
        }
        DecorType::OfferingMouth => {
            // Face outline
            painter.circle_stroke(egui::pos2(cx, cy), s * 0.85, stroke);
            // Eyes
            let le = rot_screen(cx - s * 0.3, cy - s * 0.25, cx, cy, deg);
            let re = rot_screen(cx + s * 0.3, cy - s * 0.25, cx, cy, deg);
            painter.circle_filled(le, s * 0.12, color);
            painter.circle_filled(re, s * 0.12, color);
            // Open mouth (oval via line segments)
            let mouth_cx = cx;
            let mouth_cy = cy + s * 0.3;
            let mouth_rx = s * 0.35;
            let mouth_ry = s * 0.25;
            let segments = 16;
            for i in 0..segments {
                let a0 = (i as f32 / segments as f32) * std::f32::consts::TAU;
                let a1 = ((i + 1) as f32 / segments as f32) * std::f32::consts::TAU;
                let x0 = mouth_cx + mouth_rx * a0.cos();
                let y0 = mouth_cy + mouth_ry * a0.sin();
                let x1 = mouth_cx + mouth_rx * a1.cos();
                let y1 = mouth_cy + mouth_ry * a1.sin();
                rot_line_screen(painter, x0, y0, x1, y1, cx, cy, deg, stroke);
            }
            // Dark mouth interior
            let mc = rot_screen(mouth_cx, mouth_cy, cx, cy, deg);
            painter.circle_filled(mc, mouth_ry * 0.6, color.linear_multiply(0.3));
            // Brow ridges
            rot_line_screen(painter, cx - s * 0.5, cy - s * 0.45, cx - s * 0.1, cy - s * 0.5, cx, cy, deg, stroke);
            rot_line_screen(painter, cx + s * 0.5, cy - s * 0.45, cx + s * 0.1, cy - s * 0.5, cx, cy, deg, stroke);
            // Nose hint
            rot_line_screen(painter, cx, cy - s * 0.1, cx - s * 0.08, cy + s * 0.05, cx, cy, deg, thin);
            rot_line_screen(painter, cx, cy - s * 0.1, cx + s * 0.08, cy + s * 0.05, cx, cy, deg, thin);
        }
    }
}
