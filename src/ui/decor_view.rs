use std::hash::{Hash, Hasher};

use crate::model::*;
use crate::render::decor::{draw_decor, DecorPalette, PainterSink};
use crate::render::recording::replay_commands;
use crate::render::themed::RenderOptions;
use crate::ui::canvas_common::{handle_pan_zoom, ViewState, COLOR_PLACEHOLDER_TEXT};
use crate::ui::spatial_view::collect_floors;
use crate::util::{ViewTransform, DECOR_HALF_SIZE, GRID_PX};

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
    /// Offsets of the other selected items relative to the dragged one, captured at drag start.
    drag_group: Vec<(usize, f32, f32)>,
    /// Drag-select start position in world coords.
    drag_select_start: Option<egui::Pos2>,
    /// Search filter for decor type dropdowns.
    pub decor_search: String,
    /// Whether the object browser panel is open.
    pub object_browser_open: bool,
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
            drag_group: Vec::new(),
            drag_select_start: None,
            decor_search: String::new(),
            object_browser_open: false,
        }
    }
}

impl DecorViewState {
    /// Every selected decor index: the box-selected set plus the primary selection.
    pub fn decor_selection(&self) -> Vec<usize> {
        let mut sel: Vec<usize> = self.selected_decor_set.iter().copied().collect();
        if let Some(di) = self.selected_decor {
            if !sel.contains(&di) {
                sel.push(di);
            }
        }
        sel.sort_unstable();
        sel
    }

    /// Make `di` the only selected item.
    pub fn select_decor_only(&mut self, di: usize) {
        self.selected_decor_set.clear();
        self.selected_decor = Some(di);
    }

    /// Add `di` to the selection, or drop it if already selected.
    pub fn toggle_decor(&mut self, di: usize) {
        let mut sel: std::collections::HashSet<usize> =
            self.decor_selection().into_iter().collect();
        if !sel.remove(&di) {
            sel.insert(di);
        }
        self.selected_decor = sel.iter().copied().min();
        self.selected_decor_set = sel;
    }

    pub fn clear_decor_selection(&mut self) {
        self.selected_decor = None;
        self.selected_decor_set.clear();
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
        show_lighting: true,
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
                let s = DECOR_HALF_SIZE * transform.zoom;
                let deg = decor.rotation;

                draw_decor_symbol(&painter, screen_center, s, decor.scale_x, decor.scale_y, deg, decor.decor_type, decor_color);
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
                egui::Stroke::new(2.0_f32, egui::Color32::from_rgb(100, 180, 255)),
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
                    painter.circle_stroke(screen, handle_r + 2.0, egui::Stroke::new(1.5_f32, ring_color));
                    // Hitbox used for cover / light blocking (selected item only)
                    if is_sel && decor.cover_kind() != crate::model::CoverKind::None {
                        let (ex, ey) = decor.decor_type.local_extent();
                        let hx = ex * decor.scale_x * crate::util::DECOR_HALF_SIZE;
                        let hy = ey * decor.scale_y * crate::util::DECOR_HALF_SIZE;
                        let (s, c) = decor.rotation.to_radians().sin_cos();
                        let corner = |lx: f32, ly: f32| transform.world_to_screen(egui::pos2(wx + lx * c - ly * s, wy + lx * s + ly * c));
                        let pts = [corner(-hx, -hy), corner(hx, -hy), corner(hx, hy), corner(-hx, hy)];
                        let hb_color = egui::Color32::from_rgba_unmultiplied(255, 120, 60, 160);
                        for i in 0..4 {
                            painter.line_segment([pts[i], pts[(i + 1) % 4]], egui::Stroke::new(1.0_f32, hb_color));
                        }
                    }
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
                            for &(i, ox, oy) in &state.drag_group {
                                if i < room.decor.len() {
                                    room.decor[i].x = (new_x + ox).clamp(0.0, rl.width as f32);
                                    room.decor[i].y = (new_y + oy).clamp(0.0, rl.height as f32);
                                }
                            }
                        }
                    }
                }
            }
        }
        if response.drag_stopped() {
            state.dragging_decor = None;
            state.drag_group.clear();
        }
    }

    // Click handling
    let ctrl_held = ui.ctx().input(|i| i.modifiers.command);
    let shift_held = ui.ctx().input(|i| i.modifiers.shift);
    if response.clicked() {
        if let Some(pos) = response.interact_pointer_pos() {
            let world = transform.screen_to_world(pos);
            let gx = (world.x / GRID_PX).floor() as i32;
            let gy = (world.y / GRID_PX).floor() as i32;

            // First, check if we clicked on an existing decor item in the selected room
            let mut clicked_decor = None;
            if !ctrl_held {
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
            }

            if ctrl_held && state.selected_room.is_some() && state.selected_decor.is_some() {
                // Ctrl+click: clone selected decor at click position
                let sel_id = state.selected_room.clone().unwrap();
                let di = state.selected_decor.unwrap();
                if let Some(rl) = render_layout.room_by_id(&sel_id) {
                    let room_px_x = rl.x as f32 * GRID_PX;
                    let room_px_y = rl.y as f32 * GRID_PX;
                    let new_x = ((world.x - room_px_x) / GRID_PX).clamp(0.0, rl.width as f32);
                    let new_y = ((world.y - room_px_y) / GRID_PX).clamp(0.0, rl.height as f32);
                    if let Some(room) = dungeon.graph.room_by_id_mut(&sel_id) {
                        if di < room.decor.len() {
                            let mut cloned = room.decor[di].clone();
                            cloned.id = uuid::Uuid::new_v4().to_string();
                            cloned.x = new_x;
                            cloned.y = new_y;
                            room.decor.push(cloned);
                            let new_idx = room.decor.len() - 1;
                            state.select_decor_only(new_idx);
                        }
                    }
                }
            } else if let Some(di) = clicked_decor {
                if shift_held {
                    state.toggle_decor(di);
                } else {
                    state.select_decor_only(di);
                }
            } else if state.place_mode {
                // Place new decor — auto-select room under cursor if needed
                let target_room = state.selected_room.clone().or_else(|| {
                    render_layout.room_at_grid(&dungeon.graph, gx, gy).map(|rl| rl.room_id.clone())
                });
                if let Some(sel_id) = target_room {
                    if let Some(rl) = render_layout.room_by_id(&sel_id) {
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
                            if let Some(room) = dungeon.graph.room_by_id_mut(&sel_id) {
                                room.decor.push(new_decor);
                                let new_idx = room.decor.len() - 1;
                                state.select_decor_only(new_idx);
                            }
                            state.selected_room = Some(sel_id);
                        }
                    }
                }
            } else {
                // Click to select room
                state.clear_decor_selection();
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

    // Delete selected decor with Delete or Backspace key (not while editing text)
    if state.selected_room.is_some() && (!state.selected_decor_set.is_empty() || state.selected_decor.is_some()) && !ui.ctx().wants_keyboard_input() {
        let delete_pressed = ui.ctx().input(|i| {
            i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace)
        });
        if delete_pressed {
            let sel_id = state.selected_room.clone().unwrap();
            if let Some(room) = dungeon.graph.room_by_id_mut(&sel_id) {
                // Collect all indices to remove
                let mut to_remove = state.decor_selection();
                to_remove.sort_unstable_by(|a, b| b.cmp(a)); // reverse order
                for idx in to_remove {
                    if idx < room.decor.len() {
                        room.decor.remove(idx);
                    }
                }
                state.clear_decor_selection();
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
                                if !state.decor_selection().contains(&di) {
                                    state.select_decor_only(di);
                                }
                                // Move the whole selection together, keeping relative offsets
                                state.drag_group = state
                                    .decor_selection()
                                    .into_iter()
                                    .filter(|&i| i != di && i < room.decor.len())
                                    .map(|i| (i, room.decor[i].x - decor.x, room.decor[i].y - decor.y))
                                    .collect();
                                break;
                            }
                        }
                    }
                }
            }
        }
    }

    // Drag-select: left-drag on empty space rubber-bands a selection
    // (a left-drag that starts on an item moves it instead, handled above)
    if !state.place_mode && state.dragging_decor.is_none() {
        if response.drag_started_by(egui::PointerButton::Primary) {
            if let Some(pos) = response.interact_pointer_pos() {
                state.drag_select_start = Some(transform.screen_to_world(pos));
            }
        }
        if let Some(start) = state.drag_select_start {
            if response.dragged_by(egui::PointerButton::Primary) {
                if let Some(pos) = pointer_pos {
                    let current = transform.screen_to_world(pos);
                    let min = egui::pos2(start.x.min(current.x), start.y.min(current.y));
                    let max = egui::pos2(start.x.max(current.x), start.y.max(current.y));
                    let screen_min = transform.world_to_screen(min);
                    let screen_max = transform.world_to_screen(max);
                    painter.rect_stroke(
                        egui::Rect::from_min_max(screen_min, screen_max),
                        0.0,
                        egui::Stroke::new(1.0_f32, egui::Color32::from_rgb(100, 200, 255)),
                        egui::StrokeKind::Outside,
                    );
                }
            }
            if response.drag_stopped_by(egui::PointerButton::Primary) {
                if let Some(pos) = pointer_pos {
                    let end = transform.screen_to_world(pos);
                    let min_x = start.x.min(end.x);
                    let min_y = start.y.min(end.y);
                    let max_x = start.x.max(end.x);
                    let max_y = start.y.max(end.y);
                    // Select all decor items within the rectangle (shift adds to the selection)
                    if !shift_held {
                        state.selected_decor_set.clear();
                    } else if let Some(di) = state.selected_decor {
                        state.selected_decor_set.insert(di);
                    }
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
                    state.selected_decor = state.selected_decor_set.iter().copied().min();
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
                egui::Stroke::new(1.5_f32, egui::Color32::from_rgb(100, 255, 100)),
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

    // Ctrl+hover ghost preview for clone
    if ctrl_held && !state.place_mode && state.selected_room.is_some() && state.selected_decor.is_some() && response.hovered() {
        if let Some(pos) = response.hover_pos() {
            let sel_id = state.selected_room.as_ref().unwrap();
            let di = state.selected_decor.unwrap();
            if let Some(room) = dungeon.graph.room_by_id(sel_id) {
                if di < room.decor.len() {
                    let decor = &room.decor[di];
                    let s = DECOR_HALF_SIZE * transform.zoom;
                    let ghost_color = egui::Color32::from_rgba_unmultiplied(
                        decor_color.r(), decor_color.g(), decor_color.b(), 100,
                    );
                    draw_decor_symbol(&painter, pos, s, decor.scale_x, decor.scale_y, decor.rotation, decor.decor_type, ghost_color);
                }
            }
        }
    }
}

pub fn decor_sidebar(ui: &mut egui::Ui, dungeon: &mut Dungeon, state: &mut DecorViewState) {
    // Object browser (shown when open, replaces other sidebar content)
    if state.object_browser_open {
        // Paint floor color as background for the entire browser area
        let fc = dungeon.theme.floor_color;
        let floor_bg = egui::Color32::from_rgba_unmultiplied(fc[0], fc[1], fc[2], fc[3]);
        let browser_rect = ui.available_rect_before_wrap();
        ui.painter().rect_filled(browser_rect, 0.0, floor_bg);

        ui.heading("Object Browser");
        ui.separator();

        if ui.small_button("Close Browser").clicked() {
            state.object_browser_open = false;
            state.decor_search.clear();
            return;
        }

        ui.add_space(4.0);

        // Search field
        let search_response = ui.add(
            egui::TextEdit::singleline(&mut state.decor_search)
                .hint_text("Search objects...")
                .desired_width(ui.available_width()),
        );
        // Auto-focus the search field when browser opens
        if search_response.gained_focus() || !search_response.lost_focus() {
            search_response.request_focus();
        }

        ui.add_space(4.0);

        if state.place_mode {
            ui.colored_label(
                egui::Color32::from_rgb(100, 255, 100),
                format!("Placing: {}", state.place_type.label()),
            );
            if ui.small_button("Stop Placing").clicked() {
                state.place_mode = false;
            }
            ui.add_space(4.0);
        }

        let filter = state.decor_search.to_lowercase();
        let wall_color = dungeon.theme.wall_color;
        let preview_color = egui::Color32::from_rgb(wall_color[0], wall_color[1], wall_color[2]);
        let available_width = ui.available_width();

        egui::ScrollArea::vertical().show(ui, |ui| {
            for dt in DecorType::ALL {
                if !filter.is_empty() && !dt.label().to_lowercase().contains(&filter) {
                    continue;
                }

                let is_active = state.place_mode && state.place_type == dt;
                let frame = if is_active {
                    egui::Frame::NONE
                        .inner_margin(6.0)
                        .stroke(egui::Stroke::new(1.5_f32, egui::Color32::from_rgb(100, 255, 100)))
                        .corner_radius(4.0)
                        .fill(egui::Color32::from_rgba_unmultiplied(100, 255, 100, 15))
                } else {
                    egui::Frame::NONE
                        .inner_margin(6.0)
                        .corner_radius(4.0)
                };

                let resp = frame.show(ui, |ui| {
                    ui.set_min_width(available_width - 16.0);
                    ui.horizontal(|ui| {
                        // Preview icon
                        let (icon_rect, _) = ui.allocate_exact_size(
                            egui::vec2(28.0, 28.0),
                            egui::Sense::hover(),
                        );
                        draw_decor_symbol(
                            ui.painter(),
                            icon_rect.center(),
                            10.0,
                            1.0,
                            1.0,
                            0.0,
                            dt,
                            preview_color,
                        );

                        ui.label(dt.label());
                    });
                }).response;

                if resp.interact(egui::Sense::click()).clicked() {
                    state.place_type = dt;
                    state.place_mode = true;
                    state.selected_decor = None;
                }

                if resp.interact(egui::Sense::hover()).hovered() && !is_active {
                    ui.painter().rect_stroke(
                        resp.rect,
                        4.0,
                        egui::Stroke::new(1.0_f32, egui::Color32::from_rgb(100, 180, 255)),
                        egui::StrokeKind::Middle,
                    );
                }
            }
        });

        // Skip the rest of sidebar when browser is open
        return;
    }

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

        // Object browser button + current placement status
        if ui.button("Object Browser").clicked() {
            state.object_browser_open = true;
            state.decor_search.clear();
        }
        if state.place_mode {
            ui.horizontal(|ui| {
                ui.colored_label(
                    egui::Color32::from_rgb(100, 255, 100),
                    format!("Placing: {}", state.place_type.label()),
                );
                if ui.small_button("Stop").clicked() {
                    state.place_mode = false;
                }
            });
        }

        ui.add_space(8.0);
        ui.separator();

        // List decor items in this room
        let decor_count = dungeon.graph.room_by_id(sel_room_id)
            .map(|r| r.decor.len())
            .unwrap_or(0);

        if decor_count == 0 {
            ui.label("No decorations. Use the Object Browser to add items.");
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

            let selection = state.decor_selection();
            for (di, _id, dt, _dx, _dy) in &decor_info {
                let is_sel = selection.contains(di);
                let frame = if is_sel {
                    egui::Frame::NONE
                        .inner_margin(4.0)
                        .stroke(egui::Stroke::new(1.0_f32, egui::Color32::from_rgb(255, 200, 50)))
                        .corner_radius(3.0)
                } else {
                    egui::Frame::NONE.inner_margin(4.0)
                };
                frame.show(ui, |ui| {
                    ui.horizontal(|ui| {
                        if ui.selectable_label(is_sel, dt.label()).clicked() {
                            if ui.input(|i| i.modifiers.shift) {
                                state.toggle_decor(*di);
                            } else {
                                state.select_decor_only(*di);
                            }
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
                let shift_down = |i: usize| if i > idx { Some(i - 1) } else if i == idx { None } else { Some(i) };
                state.selected_decor = state.selected_decor.and_then(shift_down);
                state.selected_decor_set = state.selected_decor_set.iter().filter_map(|&i| shift_down(i)).collect();
            }
        }

        // Properties of the whole selection, or of the single selected item
        let sel_all = state.decor_selection();
        if sel_all.len() > 1 {
            ui.add_space(8.0);
            ui.separator();
            ui.label(format!("Properties ({} selected):", sel_all.len()));
            if let Some(room) = dungeon.graph.room_by_id_mut(sel_room_id) {
                let items: Vec<usize> = sel_all.into_iter().filter(|&i| i < room.decor.len()).collect();
                if !items.is_empty() {
                    multi_decor_properties(ui, room, &items, &mut state.decor_search);
                }
            }
        } else if let Some(sel_idx) = state.selected_decor {
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
                    ui.horizontal(|ui| {
                        ui.label("W");
                        ui.add(egui::DragValue::new(&mut decor.scale_x).speed(0.05).range(0.01..=f32::MAX));
                        ui.label("H");
                        ui.add(egui::DragValue::new(&mut decor.scale_y).speed(0.05).range(0.01..=f32::MAX));
                    });
                    ui.horizontal(|ui| {
                        ui.label("Cover:");
                        let default_kind = decor.decor_type.default_cover();
                        let text = match decor.cover {
                            None => format!("Default ({})", default_kind.label()),
                            Some(k) => k.label().to_string(),
                        };
                        egui::ComboBox::from_id_salt("decor_cover")
                            .selected_text(text)
                            .show_ui(ui, |ui| {
                                if ui.selectable_label(decor.cover.is_none(), format!("Default ({})", default_kind.label())).clicked() {
                                    decor.cover = None;
                                }
                                for k in crate::model::CoverKind::ALL {
                                    if ui.selectable_label(decor.cover == Some(k), k.label()).clicked() {
                                        decor.cover = Some(k);
                                    }
                                }
                            });
                    }).response.on_hover_text("Most cover this object can grant; Full also blocks light");
                }
            }
        }
    } else {
        ui.heading("Decorations");
        ui.separator();

        // Object browser button available even without room selected
        if ui.button("Object Browser").clicked() {
            state.object_browser_open = true;
            state.decor_search.clear();
        }

        ui.add_space(8.0);
        ui.label("Select a room on the map to place decorations.");

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
                pos: None,
                dim_radius: None,
                carrier: None,
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
                ui.add(egui::Slider::new(&mut light.radius, 1.0..=20.0).text("Bright"));
                if ui.small_button("X").clicked() {
                    remove_light = Some(li);
                }
            });
            light_extra_controls(ui, light, &dungeon.party);
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
                    pos: None,
                    dim_radius: None,
                    carrier: None,
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
            ui.add(egui::Slider::new(&mut light.radius, 1.0..=20.0).text("Bright"));
            light_extra_controls(ui, light, &dungeon.party);
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

/// The value shared by every item, or `None` when they differ.
fn common_value<T: PartialEq + Copy>(mut vals: impl Iterator<Item = T>) -> Option<T> {
    let first = vals.next()?;
    vals.all(|v| v == first).then_some(first)
}

/// Property editor applying to every selected decor item at once. Controls show the
/// shared value, or "Mixed" when the selection disagrees; editing one writes it to all.
fn multi_decor_properties(ui: &mut egui::Ui, room: &mut Room, items: &[usize], search: &mut String) {
    let common_type = common_value(items.iter().map(|&i| room.decor[i].decor_type));
    ui.horizontal(|ui| {
        ui.label("Type:");
        let mut chosen = None;
        egui::ComboBox::from_id_salt("decor_multi_type")
            .selected_text(common_type.map(|t| t.label()).unwrap_or("Mixed"))
            .width(110.0)
            .show_ui(ui, |ui| {
                ui.add(egui::TextEdit::singleline(search).hint_text("Search...").desired_width(100.0));
                let filter = search.to_lowercase();
                egui::ScrollArea::vertical().max_height(200.0).show(ui, |ui| {
                    for dt in DecorType::ALL {
                        if !filter.is_empty() && !dt.label().to_lowercase().contains(&filter) {
                            continue;
                        }
                        if ui.selectable_label(common_type == Some(dt), dt.label()).clicked() {
                            chosen = Some(dt);
                            search.clear();
                        }
                    }
                });
            });
        if let Some(dt) = chosen {
            for &i in items {
                room.decor[i].decor_type = dt;
            }
        }
    });

    ui.horizontal(|ui| {
        ui.label("Nudge:");
        let step = 0.25;
        let mut d = (0.0f32, 0.0f32);
        if ui.small_button("\u{2190}").clicked() { d.0 -= step; }
        if ui.small_button("\u{2192}").clicked() { d.0 += step; }
        if ui.small_button("\u{2191}").clicked() { d.1 -= step; }
        if ui.small_button("\u{2193}").clicked() { d.1 += step; }
        if d != (0.0, 0.0) {
            for &i in items {
                room.decor[i].x += d.0;
                room.decor[i].y += d.1;
            }
        }
    }).response.on_hover_text("Move the whole selection by a quarter cell (or drag it on the map)");

    let common_rot = common_value(items.iter().map(|&i| room.decor[i].rotation));
    let mut rot = common_rot.unwrap_or(0.0);
    let rot_label = if common_rot.is_some() { "Rotation" } else { "Rotation (mixed)" };
    if ui.add(egui::Slider::new(&mut rot, 0.0..=360.0).text(rot_label)).changed() {
        for &i in items {
            room.decor[i].rotation = rot;
        }
    }

    ui.horizontal(|ui| {
        ui.label("W");
        let common_w = common_value(items.iter().map(|&i| room.decor[i].scale_x));
        let mut w = common_w.unwrap_or(1.0);
        if ui.add(egui::DragValue::new(&mut w).speed(0.05).range(0.01..=f32::MAX)).changed() {
            for &i in items {
                room.decor[i].scale_x = w;
            }
        }
        ui.label("H");
        let common_h = common_value(items.iter().map(|&i| room.decor[i].scale_y));
        let mut h = common_h.unwrap_or(1.0);
        if ui.add(egui::DragValue::new(&mut h).speed(0.05).range(0.01..=f32::MAX)).changed() {
            for &i in items {
                room.decor[i].scale_y = h;
            }
        }
        if common_w.is_none() || common_h.is_none() {
            ui.label("(mixed)");
        }
    });

    let common_cover = common_value(items.iter().map(|&i| room.decor[i].cover));
    let common_default = common_value(items.iter().map(|&i| room.decor[i].decor_type.default_cover()));
    ui.horizontal(|ui| {
        ui.label("Cover:");
        let default_text = match common_default {
            Some(k) => format!("Default ({})", k.label()),
            None => "Default (per type)".to_string(),
        };
        let text = match common_cover {
            Some(None) => default_text.clone(),
            Some(Some(k)) => k.label().to_string(),
            None => "Mixed".to_string(),
        };
        let mut chosen: Option<Option<CoverKind>> = None;
        egui::ComboBox::from_id_salt("decor_multi_cover")
            .selected_text(text)
            .show_ui(ui, |ui| {
                if ui.selectable_label(common_cover == Some(None), default_text).clicked() {
                    chosen = Some(None);
                }
                for k in CoverKind::ALL {
                    if ui.selectable_label(common_cover == Some(Some(k)), k.label()).clicked() {
                        chosen = Some(Some(k));
                    }
                }
            });
        if let Some(c) = chosen {
            for &i in items {
                room.decor[i].cover = c;
            }
        }
    }).response.on_hover_text("Most cover these objects can grant; Full also blocks light");
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

/// Draw a decor symbol in screen space using egui painter primitives.
fn draw_decor_symbol(
    painter: &egui::Painter,
    center: egui::Pos2,
    s: f32, // base half-size in screen pixels (without per-decor scale)
    sclx: f32, // horizontal scale factor
    scly: f32, // vertical scale factor
    deg: f32,
    decor_type: DecorType,
    color: egui::Color32,
) {
    let palette = DecorPalette::from_ink(color.to_srgba_unmultiplied());
    let mut sink = PainterSink { painter };
    draw_decor(&mut sink, decor_type, center.x, center.y, s, sclx, scly, deg, &palette);
}

/// Dim radius, explicit position, and carrier controls shared by both light lists.
fn light_extra_controls(ui: &mut egui::Ui, light: &mut crate::model::LightSource, party: &[crate::model::PlayerCharacter]) {
    ui.horizontal(|ui| {
        let mut dim = light.dim_radius();
        if ui.add(egui::Slider::new(&mut dim, 0.0..=40.0).text("Dim")).changed() {
            light.dim_radius = Some(dim.max(light.radius));
        }
        ui.add(egui::Slider::new(&mut light.intensity, 0.0..=1.0).text("I"));
    });
    ui.horizontal(|ui| {
        ui.label("Pos:");
        match light.pos {
            Some((mut x, mut y)) => {
                let cx = crate::ui::canvas_common::num_input_f32(ui, &mut x, 40.0);
                let cy = crate::ui::canvas_common::num_input_f32(ui, &mut y, 40.0);
                if cx || cy {
                    light.pos = Some((x, y));
                }
                if ui.small_button("Room center").clicked() {
                    light.pos = None;
                }
            }
            None => {
                ui.weak("room center");
                if ui.small_button("Set").on_hover_text("Give this light an exact grid position").clicked() {
                    light.pos = Some((0.0, 0.0));
                }
            }
        }
        if !party.is_empty() {
            ui.label("Carrier:");
            let current = match &light.carrier {
                Some(crate::model::TokenKind::Player(pid)) => party.iter().find(|p| p.id == *pid).map(|p| p.name.as_str()).unwrap_or("?"),
                Some(_) => "monster",
                None => "None",
            };
            egui::ComboBox::from_id_salt(format!("light_carrier_{}", light.id))
                .selected_text(current)
                .width(100.0)
                .show_ui(ui, |ui| {
                    if ui.selectable_label(light.carrier.is_none(), "None").clicked() {
                        light.carrier = None;
                    }
                    for pc in party {
                        let kind = crate::model::TokenKind::Player(pc.id.clone());
                        if ui.selectable_label(light.carrier.as_ref() == Some(&kind), &pc.name).clicked() {
                            light.carrier = Some(kind);
                        }
                    }
                });
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selection_combines_set_and_primary_without_duplicates() {
        let mut state = DecorViewState::default();
        state.selected_decor_set.extend([2, 0]);
        state.selected_decor = Some(2);
        assert_eq!(state.decor_selection(), vec![0, 2]);

        state.selected_decor = Some(5);
        assert_eq!(state.decor_selection(), vec![0, 2, 5]);
    }

    #[test]
    fn toggle_adds_and_removes_including_the_primary_item() {
        let mut state = DecorViewState::default();
        state.select_decor_only(3);
        state.toggle_decor(1);
        assert_eq!(state.decor_selection(), vec![1, 3]);

        // Toggling the primary item off leaves the rest selected
        state.toggle_decor(3);
        assert_eq!(state.decor_selection(), vec![1]);
        assert_eq!(state.selected_decor, Some(1));

        state.toggle_decor(1);
        assert!(state.decor_selection().is_empty());
        assert_eq!(state.selected_decor, None);
    }

    #[test]
    fn select_only_replaces_a_multi_selection() {
        let mut state = DecorViewState::default();
        state.selected_decor_set.extend([0, 1, 2]);
        state.select_decor_only(4);
        assert_eq!(state.decor_selection(), vec![4]);
    }
}
