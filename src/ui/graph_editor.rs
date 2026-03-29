use std::collections::{HashMap, HashSet};

use crate::model::*;
use crate::ui::canvas_common::{handle_pan_zoom, ViewState};
use crate::util::{point_to_segment_dist, ViewTransform};

/// Multi-selection state for graph editor
#[derive(Clone, Debug, Default)]
pub struct Selection {
    pub rooms: HashSet<String>,
    pub connections: HashSet<String>,
    pub groups: HashSet<String>,
}

impl Selection {
    pub fn is_empty(&self) -> bool {
        self.rooms.is_empty() && self.connections.is_empty() && self.groups.is_empty()
    }

    pub fn clear(&mut self) {
        self.rooms.clear();
        self.connections.clear();
        self.groups.clear();
    }

    pub fn select_room(&mut self, id: String) {
        self.clear();
        self.rooms.insert(id);
    }

    pub fn select_connection(&mut self, id: String) {
        self.clear();
        self.connections.insert(id);
    }

    pub fn select_group(&mut self, id: String) {
        self.clear();
        self.groups.insert(id);
    }

    pub fn toggle_room(&mut self, id: &str) {
        if !self.rooms.remove(id) {
            self.rooms.insert(id.to_string());
        }
        // Clear non-room selections when toggling rooms
        self.connections.clear();
        self.groups.clear();
    }

    /// Returns the single selected group ID, if exactly one is selected.
    pub fn single_group(&self) -> Option<&str> {
        if self.groups.len() == 1 && self.rooms.is_empty() && self.connections.is_empty() {
            self.groups.iter().next().map(|s| s.as_str())
        } else {
            None
        }
    }
}

/// Drag state for graph editor interactions
#[derive(Clone, Debug, Default)]
pub enum DragState {
    #[default]
    None,
    DraggingRooms,
    ConnectingFrom(String),
    /// Marquee selection: start position in world coords
    Marquee(egui::Pos2),
}

/// State specific to the graph editor view
pub struct GraphEditorState {
    pub view: ViewState,
    pub room_positions: HashMap<String, egui::Pos2>,
    pub selection: Selection,
    pub drag_state: DragState,
    next_room_number: u32,
}

impl Default for GraphEditorState {
    fn default() -> Self {
        Self {
            view: ViewState::default(),
            room_positions: HashMap::new(),
            selection: Selection::default(),
            drag_state: DragState::None,
            next_room_number: 1,
        }
    }
}

const NODE_WIDTH: f32 = 120.0;
const NODE_HEIGHT: f32 = 50.0;
const CONNECT_HANDLE_RADIUS: f32 = 8.0;

pub fn graph_editor(ui: &mut egui::Ui, dungeon: &mut Dungeon, state: &mut GraphEditorState) {
    let (response, painter) = ui.allocate_painter(
        ui.available_size(),
        egui::Sense::click_and_drag(),
    );
    let rect = response.rect;

    // Fill canvas background
    painter.rect_filled(rect, 0.0, egui::Color32::from_rgb(35, 35, 40));

    handle_pan_zoom(&response, &mut state.view);
    let transform = ViewTransform::new(state.view.offset, state.view.zoom, rect);

    // Load positions from saved data, or assign defaults for new rooms
    for room in &dungeon.graph.rooms {
        if !state.room_positions.contains_key(&room.id) {
            if let Some(&(x, y)) = dungeon.graph.graph_positions.get(&room.id) {
                state.room_positions.insert(room.id.clone(), egui::pos2(x, y));
            } else {
                state.room_positions.insert(room.id.clone(), egui::pos2(200.0, 200.0));
            }
        }
    }

    // Sync positions back to the model for serialization
    dungeon.graph.graph_positions = state
        .room_positions
        .iter()
        .map(|(id, pos)| (id.clone(), (pos.x, pos.y)))
        .collect();

    // Handle interactions
    handle_interactions(ui, &response, &transform, dungeon, state);

    // Draw groups (behind everything)
    draw_groups(&painter, &transform, dungeon, state);

    // Draw connections
    draw_connections(&painter, &transform, dungeon, state);

    // Draw rooms
    draw_rooms(&painter, &transform, dungeon, state);

    // Draw in-progress connection line
    if let DragState::ConnectingFrom(ref src_id) = state.drag_state {
        if let Some(&src_pos) = state.room_positions.get(src_id) {
            if let Some(pointer) = response.hover_pos() {
                let world_target = transform.screen_to_world(pointer);
                let src_rect = egui::Rect::from_center_size(
                    src_pos,
                    egui::vec2(NODE_WIDTH, NODE_HEIGHT),
                );
                let src_edge = rect_edge_intersection(src_pos, world_target, src_rect);
                let screen_src = transform.world_to_screen(src_edge);
                painter.line_segment(
                    [screen_src, pointer],
                    egui::Stroke::new(2.0, egui::Color32::from_rgb(100, 200, 255)),
                );
            }
        }
    }

    // Draw marquee selection rectangle
    if let DragState::Marquee(start) = state.drag_state {
        if let Some(pointer) = response.hover_pos() {
            let screen_start = transform.world_to_screen(start);
            let marquee = egui::Rect::from_two_pos(screen_start, pointer);
            painter.rect_filled(
                marquee,
                0.0,
                egui::Color32::from_rgba_unmultiplied(100, 150, 255, 30),
            );
            painter.rect_stroke(
                marquee,
                0.0,
                egui::Stroke::new(1.0, egui::Color32::from_rgb(100, 150, 255)),
                egui::StrokeKind::Middle,
            );
        }
    }
}

fn handle_interactions(
    ui: &egui::Ui,
    response: &egui::Response,
    transform: &ViewTransform,
    dungeon: &mut Dungeon,
    state: &mut GraphEditorState,
) {
    let pointer = response.hover_pos();

    let modifiers = ui.input(|i| i.modifiers);
    let shift = modifiers.shift;
    let ctrl = modifiers.ctrl;

    // Double-click to create room
    if response.double_clicked() {
        if let Some(pos) = pointer {
            let world_pos = transform.screen_to_world(pos);
            if hit_test_room(world_pos, &state.room_positions).is_none() {
                let connect_and_select = ctrl;
                let connect_no_select = modifiers.alt;

                let label = format!("Room {}", state.next_room_number);
                state.next_room_number += 1;
                let room = Room::new(label);
                let new_id = room.id.clone();
                state.room_positions.insert(new_id.clone(), world_pos);
                dungeon.graph.add_room(room);

                if connect_and_select || connect_no_select {
                    // Connect all selected rooms to the new room
                    let selected: Vec<String> = state.selection.rooms.iter().cloned().collect();
                    for selected_id in &selected {
                        let conn = Connection::new(ConnectionType::Door);
                        dungeon.graph.add_connection(
                            selected_id.clone(),
                            new_id.clone(),
                            conn,
                        );
                    }
                }

                if !connect_no_select {
                    state.selection.select_room(new_id);
                }
            }
        }
    }

    // Click to select. Shift+click toggles. Ctrl+click connects.
    // Skip if double-click also fired this frame.
    if response.clicked() && !response.double_clicked() {
        if let Some(pos) = pointer {
            let world_pos = transform.screen_to_world(pos);

            if let Some(room_id) = hit_test_room(world_pos, &state.room_positions) {
                if ctrl && !state.selection.rooms.is_empty() {
                    // Connect all selected rooms to the clicked room
                    let selected: Vec<String> = state.selection.rooms.iter()
                        .filter(|id| id.as_str() != room_id)
                        .cloned()
                        .collect();
                    for selected_id in &selected {
                        let conn = Connection::new(ConnectionType::Door);
                        dungeon.graph.add_connection(
                            selected_id.clone(),
                            room_id.clone(),
                            conn,
                        );
                    }
                }
                if shift {
                    state.selection.toggle_room(&room_id);
                } else {
                    state.selection.select_room(room_id);
                }
            } else if let Some(conn_id) = hit_test_connection(world_pos, &dungeon.graph, &state.room_positions, state.view.zoom) {
                if shift {
                    if !state.selection.connections.remove(&conn_id) {
                        state.selection.connections.insert(conn_id);
                    }
                } else {
                    state.selection.select_connection(conn_id);
                }
            } else if let Some(group_id) = hit_test_group(world_pos, &dungeon.graph, &state.room_positions) {
                state.selection.select_group(group_id);
            } else if !shift && !ctrl && !modifiers.alt {
                state.selection.clear();
            }
        }
    }

    // Drag start: room drag, connect handle, or marquee
    if response.drag_started_by(egui::PointerButton::Primary) {
        if let Some(pos) = pointer {
            let world_pos = transform.screen_to_world(pos);

            if let Some(room_id) = hit_test_connect_handle(world_pos, &state.room_positions, transform) {
                state.drag_state = DragState::ConnectingFrom(room_id);
            } else if let Some(room_id) = hit_test_room(world_pos, &state.room_positions) {
                // If dragging a selected room, move all selected rooms
                if !state.selection.rooms.contains(&room_id) {
                    if !shift {
                        state.selection.select_room(room_id.clone());
                    } else {
                        state.selection.toggle_room(&room_id);
                    }
                }
                state.drag_state = DragState::DraggingRooms;
            } else {
                // Start marquee selection on empty space
                state.drag_state = DragState::Marquee(world_pos);
            }
        }
    }

    // Dragging
    if response.dragged_by(egui::PointerButton::Primary) {
        match &state.drag_state {
            DragState::DraggingRooms => {
                let delta = response.drag_delta() / state.view.zoom;
                // Move all selected rooms
                let selected: Vec<String> = state.selection.rooms.iter().cloned().collect();
                for id in &selected {
                    if let Some(pos) = state.room_positions.get_mut(id) {
                        *pos += delta;
                    }
                }
            }
            DragState::ConnectingFrom(_) | DragState::Marquee(_) => {}
            DragState::None => {}
        }
    }

    // Release drag
    if response.drag_stopped_by(egui::PointerButton::Primary) {
        match &state.drag_state {
            DragState::ConnectingFrom(src_id) => {
                let src_id = src_id.clone();
                if let Some(pos) = pointer {
                    let world_pos = transform.screen_to_world(pos);
                    if let Some(target_id) = hit_test_room(world_pos, &state.room_positions) {
                        if target_id != src_id {
                            let conn = Connection::new(ConnectionType::Door);
                            let conn_id = conn.id.clone();
                            dungeon.graph.add_connection(src_id, target_id, conn);
                            state.selection.select_connection(conn_id);
                        }
                    }
                }
            }
            DragState::Marquee(start) => {
                // Select all rooms within the marquee rectangle
                if let Some(pos) = pointer {
                    let end = transform.screen_to_world(pos);
                    let min_x = start.x.min(end.x);
                    let min_y = start.y.min(end.y);
                    let max_x = start.x.max(end.x);
                    let max_y = start.y.max(end.y);

                    if !shift {
                        state.selection.clear();
                    }

                    for (id, &room_pos) in &state.room_positions {
                        if room_pos.x >= min_x && room_pos.x <= max_x
                            && room_pos.y >= min_y && room_pos.y <= max_y
                        {
                            state.selection.rooms.insert(id.clone());
                        }
                    }
                }
            }
            _ => {}
        }
        state.drag_state = DragState::None;
    }

    // Delete key — delete all selected items
    if response.has_focus() || response.hovered() {
        let delete_pressed = ui.input(|i| {
            i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace)
        });
        if delete_pressed && !state.selection.is_empty() {
            for id in state.selection.rooms.drain() {
                state.room_positions.remove(&id);
                dungeon.graph.remove_room(&id);
            }
            for id in state.selection.connections.drain() {
                dungeon.graph.remove_connection(&id);
            }
            for id in state.selection.groups.drain() {
                dungeon.graph.groups.retain(|g| g.id != id);
            }
        }
    }
}

fn hit_test_room(
    world_pos: egui::Pos2,
    room_positions: &HashMap<String, egui::Pos2>,
) -> Option<String> {
    for (id, &pos) in room_positions {
        let room_rect = egui::Rect::from_min_size(
            pos - egui::vec2(NODE_WIDTH / 2.0, NODE_HEIGHT / 2.0),
            egui::vec2(NODE_WIDTH, NODE_HEIGHT),
        );
        if room_rect.contains(world_pos) {
            return Some(id.clone());
        }
    }
    None
}

fn hit_test_connect_handle(
    world_pos: egui::Pos2,
    room_positions: &HashMap<String, egui::Pos2>,
    _transform: &ViewTransform,
) -> Option<String> {
    for (id, &pos) in room_positions {
        // Handle on the right edge of the room
        let handle_center = pos + egui::vec2(NODE_WIDTH / 2.0, 0.0);
        if world_pos.distance(handle_center) < CONNECT_HANDLE_RADIUS * 2.0 {
            return Some(id.clone());
        }
    }
    None
}

fn hit_test_group(
    world_pos: egui::Pos2,
    graph: &DungeonGraph,
    room_positions: &HashMap<String, egui::Pos2>,
) -> Option<String> {
    let padding = 20.0;
    for group in &graph.groups {
        if group.room_ids.is_empty() {
            continue;
        }
        let mut min_x = f32::MAX;
        let mut min_y = f32::MAX;
        let mut max_x = f32::MIN;
        let mut max_y = f32::MIN;
        for room_id in &group.room_ids {
            if let Some(&pos) = room_positions.get(room_id) {
                min_x = min_x.min(pos.x - NODE_WIDTH / 2.0);
                min_y = min_y.min(pos.y - NODE_HEIGHT / 2.0);
                max_x = max_x.max(pos.x + NODE_WIDTH / 2.0);
                max_y = max_y.max(pos.y + NODE_HEIGHT / 2.0);
            }
        }
        if min_x > max_x {
            continue;
        }
        let rect = egui::Rect::from_min_max(
            egui::pos2(min_x - padding, min_y - padding),
            egui::pos2(max_x + padding, max_y + padding),
        );
        if rect.contains(world_pos) {
            return Some(group.id.clone());
        }
    }
    None
}

fn hit_test_connection(
    world_pos: egui::Pos2,
    graph: &DungeonGraph,
    room_positions: &HashMap<String, egui::Pos2>,
    zoom: f32,
) -> Option<String> {
    // Threshold in screen pixels, divided by zoom to get world units.
    // This keeps the click area consistent regardless of zoom level.
    let threshold = 12.0 / zoom;

    for edge in &graph.connections {
        if let (Some(&src), Some(&tgt)) = (
            room_positions.get(&edge.source_room_id),
            room_positions.get(&edge.target_room_id),
        ) {
            // Test against the visible edge-to-edge line, not center-to-center
            let src_rect = egui::Rect::from_center_size(
                src,
                egui::vec2(NODE_WIDTH, NODE_HEIGHT),
            );
            let tgt_rect = egui::Rect::from_center_size(
                tgt,
                egui::vec2(NODE_WIDTH, NODE_HEIGHT),
            );
            let src_edge = rect_edge_intersection(src, tgt, src_rect);
            let tgt_edge = rect_edge_intersection(tgt, src, tgt_rect);

            let dist = point_to_segment_dist(world_pos, src_edge, tgt_edge);
            if dist < threshold {
                return Some(edge.connection.id.clone());
            }
        }
    }
    None
}


/// Find where a ray from `from` toward `to` exits `rect`.
/// Returns the intersection point on the rectangle edge.
fn rect_edge_intersection(from: egui::Pos2, to: egui::Pos2, rect: egui::Rect) -> egui::Pos2 {
    let dx = to.x - from.x;
    let dy = to.y - from.y;

    if dx == 0.0 && dy == 0.0 {
        return from;
    }

    let mut best_t = f32::MAX;

    // Check intersection with each edge
    // Right edge: x = rect.max.x
    if dx != 0.0 {
        let t = (rect.max.x - from.x) / dx;
        if t > 0.0 && t < best_t {
            let y = from.y + dy * t;
            if y >= rect.min.y && y <= rect.max.y {
                best_t = t;
            }
        }
    }
    // Left edge: x = rect.min.x
    if dx != 0.0 {
        let t = (rect.min.x - from.x) / dx;
        if t > 0.0 && t < best_t {
            let y = from.y + dy * t;
            if y >= rect.min.y && y <= rect.max.y {
                best_t = t;
            }
        }
    }
    // Bottom edge: y = rect.max.y
    if dy != 0.0 {
        let t = (rect.max.y - from.y) / dy;
        if t > 0.0 && t < best_t {
            let x = from.x + dx * t;
            if x >= rect.min.x && x <= rect.max.x {
                best_t = t;
            }
        }
    }
    // Top edge: y = rect.min.y
    if dy != 0.0 {
        let t = (rect.min.y - from.y) / dy;
        if t > 0.0 && t < best_t {
            let x = from.x + dx * t;
            if x >= rect.min.x && x <= rect.max.x {
                best_t = t;
            }
        }
    }

    if best_t == f32::MAX {
        from
    } else {
        egui::pos2(from.x + dx * best_t, from.y + dy * best_t)
    }
}

fn draw_groups(
    painter: &egui::Painter,
    transform: &ViewTransform,
    dungeon: &Dungeon,
    state: &GraphEditorState,
) {
    for group in &dungeon.graph.groups {
        if group.room_ids.is_empty() {
            continue;
        }

        // Compute bounding rect of all rooms in this group
        let mut min_x = f32::MAX;
        let mut min_y = f32::MAX;
        let mut max_x = f32::MIN;
        let mut max_y = f32::MIN;

        for room_id in &group.room_ids {
            if let Some(&pos) = state.room_positions.get(room_id) {
                min_x = min_x.min(pos.x - NODE_WIDTH / 2.0);
                min_y = min_y.min(pos.y - NODE_HEIGHT / 2.0);
                max_x = max_x.max(pos.x + NODE_WIDTH / 2.0);
                max_y = max_y.max(pos.y + NODE_HEIGHT / 2.0);
            }
        }

        if min_x > max_x {
            continue;
        }

        let padding = 20.0;
        let screen_min = transform.world_to_screen(egui::pos2(min_x - padding, min_y - padding));
        let screen_max = transform.world_to_screen(egui::pos2(max_x + padding, max_y + padding));
        let rect = egui::Rect::from_min_max(screen_min, screen_max);

        let is_selected = state.selection.groups.contains(&group.id);
        let c = group.color;
        let fill = egui::Color32::from_rgba_unmultiplied(c[0], c[1], c[2], c[3]);
        let border_color = if is_selected {
            egui::Color32::from_rgb(100, 200, 255)
        } else {
            egui::Color32::from_rgba_unmultiplied(c[0], c[1], c[2], (c[3] as u16 * 3).min(255) as u8)
        };

        painter.rect_filled(rect, 8.0, fill);
        painter.rect_stroke(rect, 8.0, egui::Stroke::new(1.5, border_color), egui::StrokeKind::Middle);

        // Label at top
        painter.text(
            egui::pos2(rect.center().x, screen_min.y + 12.0),
            egui::Align2::CENTER_CENTER,
            &group.label,
            egui::FontId::monospace(11.0 * transform.zoom),
            border_color,
        );
    }
}

fn draw_connections(
    painter: &egui::Painter,
    transform: &ViewTransform,
    dungeon: &Dungeon,
    state: &GraphEditorState,
) {
    // Count connections between each room pair to offset duplicates
    // Count connections per room pair to offset duplicate lines
    let room_pair_key = |e: &StoredEdge| -> (String, String) {
        if e.source_room_id < e.target_room_id {
            (e.source_room_id.clone(), e.target_room_id.clone())
        } else {
            (e.target_room_id.clone(), e.source_room_id.clone())
        }
    };

    let mut pair_counts: HashMap<(String, String), usize> = HashMap::new();
    let mut pair_indices: HashMap<String, usize> = HashMap::new();
    for edge in &dungeon.graph.connections {
        let key = room_pair_key(edge);
        let idx = *pair_counts.get(&key).unwrap_or(&0);
        pair_indices.insert(edge.connection.id.clone(), idx);
        *pair_counts.entry(key).or_insert(0) += 1;
    }

    for edge in &dungeon.graph.connections {
        let src_pos = state.room_positions.get(&edge.source_room_id);
        let tgt_pos = state.room_positions.get(&edge.target_room_id);

        if let (Some(&src), Some(&tgt)) = (src_pos, tgt_pos) {
            let count = pair_counts[&room_pair_key(edge)];
            let idx = pair_indices[&edge.connection.id];

            // Compute perpendicular offset for duplicate connections.
            // Always use the same direction (sorted room pair) so the
            // perpendicular doesn't flip when source/target are swapped.
            let offset = if count > 1 {
                let spread = 16.0; // world-space distance between parallel lines
                let center_offset = (idx as f32) - (count as f32 - 1.0) / 2.0;
                // Use consistent direction based on sorted room IDs
                let (dir_from, dir_to) = if edge.source_room_id < edge.target_room_id {
                    (src, tgt)
                } else {
                    (tgt, src)
                };
                let dir = dir_to - dir_from;
                let len = dir.length();
                if len > 0.1 {
                    let perp = egui::vec2(-dir.y / len, dir.x / len);
                    perp * center_offset * spread
                } else {
                    egui::Vec2::ZERO
                }
            } else {
                egui::Vec2::ZERO
            };

            // Clip line endpoints to room rectangle edges
            let src_offset = src + offset;
            let tgt_offset = tgt + offset;
            let src_rect = egui::Rect::from_center_size(
                src,
                egui::vec2(NODE_WIDTH, NODE_HEIGHT),
            );
            let tgt_rect = egui::Rect::from_center_size(
                tgt,
                egui::vec2(NODE_WIDTH, NODE_HEIGHT),
            );
            let src_edge = rect_edge_intersection(src_offset, tgt_offset, src_rect);
            let tgt_edge = rect_edge_intersection(tgt_offset, src_offset, tgt_rect);

            let screen_src = transform.world_to_screen(src_edge);
            let screen_tgt = transform.world_to_screen(tgt_edge);

            let is_selected = state.selection.connections.contains(&edge.connection.id);
            let has_constraints = edge.connection.min_length.is_some() || edge.connection.max_length.is_some();

            let (color, width) = if is_selected {
                (egui::Color32::from_rgb(100, 200, 255), 3.0)
            } else if has_constraints {
                // Subtle amber tint for constrained connections
                match edge.connection.connection_type {
                    ConnectionType::Open => (egui::Color32::from_rgb(210, 180, 100), 2.0),
                    ConnectionType::Door => (egui::Color32::from_rgb(220, 190, 110), 2.0),
                    ConnectionType::Locked => (egui::Color32::from_rgb(220, 190, 110), 3.0),
                    ConnectionType::Secret => (egui::Color32::from_rgb(180, 120, 180), 2.0),
                    ConnectionType::OneWay => (egui::Color32::from_rgb(220, 190, 110), 2.0),
                }
            } else {
                match edge.connection.connection_type {
                    ConnectionType::Open => (egui::Color32::from_rgb(180, 180, 180), 2.0),
                    ConnectionType::Door => (egui::Color32::from_rgb(200, 200, 200), 2.0),
                    ConnectionType::Locked => (egui::Color32::from_rgb(200, 200, 200), 3.0),
                    ConnectionType::Secret => (egui::Color32::from_rgb(160, 80, 200), 2.0),
                    ConnectionType::OneWay => (egui::Color32::from_rgb(200, 200, 200), 2.0),
                }
            };

            let stroke = egui::Stroke::new(width, color);

            match edge.connection.connection_type {
                ConnectionType::Secret if !is_selected => {
                    // Draw dashed line
                    draw_dashed_line(painter, screen_src, screen_tgt, stroke, 8.0, 4.0);
                }
                ConnectionType::OneWay => {
                    painter.line_segment([screen_src, screen_tgt], stroke);
                    // Draw arrow at target
                    draw_arrow_head(painter, screen_src, screen_tgt, color);
                }
                _ => {
                    painter.line_segment([screen_src, screen_tgt], stroke);
                }
            }
        }
    }
}

fn draw_dashed_line(
    painter: &egui::Painter,
    from: egui::Pos2,
    to: egui::Pos2,
    stroke: egui::Stroke,
    dash_len: f32,
    gap_len: f32,
) {
    let dir = to - from;
    let total_len = dir.length();
    if total_len < 1.0 {
        return;
    }
    let dir_norm = dir / total_len;
    let mut d = 0.0;
    while d < total_len {
        let seg_start = from + dir_norm * d;
        let seg_end = from + dir_norm * (d + dash_len).min(total_len);
        painter.line_segment([seg_start, seg_end], stroke);
        d += dash_len + gap_len;
    }
}

fn draw_arrow_head(painter: &egui::Painter, from: egui::Pos2, to: egui::Pos2, color: egui::Color32) {
    let dir = (to - from).normalized();
    let perp = egui::vec2(-dir.y, dir.x);
    let arrow_size = 10.0;

    let tip = to;
    let left = tip - dir * arrow_size + perp * arrow_size * 0.5;
    let right = tip - dir * arrow_size - perp * arrow_size * 0.5;

    painter.add(egui::Shape::convex_polygon(
        vec![tip, left, right],
        color,
        egui::Stroke::NONE,
    ));
}

fn draw_rooms(
    painter: &egui::Painter,
    transform: &ViewTransform,
    dungeon: &Dungeon,
    state: &GraphEditorState,
) {
    for room in &dungeon.graph.rooms {
        if let Some(&world_pos) = state.room_positions.get(&room.id) {
            let screen_pos = transform.world_to_screen(world_pos);
            let w = NODE_WIDTH * transform.zoom;
            let h = NODE_HEIGHT * transform.zoom;

            let node_rect = egui::Rect::from_center_size(screen_pos, egui::vec2(w, h));

            let is_selected = state.selection.rooms.contains(&room.id);

            // Background
            let bg_color = room.primary_color().linear_multiply(0.3);
            painter.rect_filled(node_rect, 6.0, bg_color);

            // Border
            let border_color = if is_selected {
                egui::Color32::from_rgb(100, 200, 255)
            } else {
                room.primary_color()
            };
            let border_width = if is_selected { 2.5 } else { 1.5 };
            painter.rect_stroke(node_rect, 6.0, egui::Stroke::new(border_width, border_color), egui::StrokeKind::Middle);

            // Label
            let font = egui::FontId::monospace(12.0 * transform.zoom);
            painter.text(
                screen_pos,
                egui::Align2::CENTER_CENTER,
                &room.label,
                font,
                egui::Color32::WHITE,
            );

            // Connection handle (small circle on right edge)
            let handle_pos = egui::pos2(node_rect.max.x, screen_pos.y);
            painter.circle_filled(
                handle_pos,
                CONNECT_HANDLE_RADIUS * transform.zoom * 0.5,
                egui::Color32::from_rgb(100, 200, 255).linear_multiply(0.5),
            );
        }
    }
}
