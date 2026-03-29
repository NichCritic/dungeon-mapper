use std::collections::HashMap;

use crate::model::*;
use crate::ui::canvas_common::{handle_pan_zoom, ViewState};
use crate::util::ViewTransform;

/// Selection state for graph editor
#[derive(Clone, Debug, Default)]
pub enum Selection {
    #[default]
    None,
    Room(String),
    Connection(String),
}

/// Drag state for graph editor interactions
#[derive(Clone, Debug, Default)]
pub enum DragState {
    #[default]
    None,
    DraggingRoom(String),
    ConnectingFrom(String),
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
            selection: Selection::None,
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

    // Ensure all rooms have a graph view position
    for room in &dungeon.graph.rooms {
        if !state.room_positions.contains_key(&room.id) {
            state
                .room_positions
                .insert(room.id.clone(), egui::pos2(200.0, 200.0));
        }
    }

    // Handle interactions
    handle_interactions(ui, &response, &transform, dungeon, state);

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
}

fn handle_interactions(
    ui: &egui::Ui,
    response: &egui::Response,
    transform: &ViewTransform,
    dungeon: &mut Dungeon,
    state: &mut GraphEditorState,
) {
    let pointer = response.hover_pos();

    // Double-click to create room
    // Ctrl+double-click: create connected room, select new room
    // Alt+double-click: create connected room, keep current selection
    if response.double_clicked() {
        if let Some(pos) = pointer {
            let world_pos = transform.screen_to_world(pos);
            // Check we're not on an existing room
            if hit_test_room(world_pos, &state.room_positions).is_none() {
                let modifiers = ui.input(|i| i.modifiers);
                let connect_and_select = modifiers.ctrl;
                let connect_no_select = modifiers.alt;

                let label = format!("Room {}", state.next_room_number);
                state.next_room_number += 1;
                let room = Room::new(label);
                let new_id = room.id.clone();
                state.room_positions.insert(new_id.clone(), world_pos);
                dungeon.graph.add_room(room);

                if connect_and_select || connect_no_select {
                    // Connect to currently selected room if there is one
                    if let Selection::Room(ref selected_id) = state.selection {
                        let conn = Connection::new(ConnectionType::Door);
                        dungeon.graph.add_connection(
                            selected_id.clone(),
                            new_id.clone(),
                            conn,
                        );
                    }
                }

                if !connect_no_select {
                    state.selection = Selection::Room(new_id);
                }
            }
        }
    }

    // Left-click to select (only when hitting a room or connection).
    // Ctrl+click on a room connects it to the selected room.
    // Clicking empty space does nothing (double-click creates a room there).
    if response.clicked() {
        if let Some(pos) = pointer {
            let world_pos = transform.screen_to_world(pos);
            let ctrl = ui.input(|i| i.modifiers.ctrl);

            if let Some(room_id) = hit_test_room(world_pos, &state.room_positions) {
                if ctrl {
                    if let Selection::Room(ref selected_id) = state.selection {
                        if *selected_id != room_id {
                            let conn = Connection::new(ConnectionType::Door);
                            dungeon.graph.add_connection(
                                selected_id.clone(),
                                room_id.clone(),
                                conn,
                            );
                        }
                    }
                }
                state.selection = Selection::Room(room_id);
            } else if let Some(conn_id) = hit_test_connection(world_pos, &dungeon.graph, &state.room_positions, state.view.zoom) {
                state.selection = Selection::Connection(conn_id);
            }
            // No else — clicking empty space doesn't deselect
        }
    }

    // Left-drag to move rooms / draw connections
    if response.drag_started_by(egui::PointerButton::Primary) {
        if let Some(pos) = pointer {
            let world_pos = transform.screen_to_world(pos);

            // Check if clicking on a connection handle (edge of room)
            if let Some(room_id) = hit_test_connect_handle(world_pos, &state.room_positions, transform) {
                state.drag_state = DragState::ConnectingFrom(room_id);
            } else if let Some(room_id) = hit_test_room(world_pos, &state.room_positions) {
                state.selection = Selection::Room(room_id.clone());
                state.drag_state = DragState::DraggingRoom(room_id);
            } else {
            }
        }
    }

    // Dragging
    if response.dragged_by(egui::PointerButton::Primary) {
        match &state.drag_state {
            DragState::DraggingRoom(id) => {
                let delta = response.drag_delta() / state.view.zoom;
                if let Some(pos) = state.room_positions.get_mut(id) {
                    *pos += delta;
                }
            }
            DragState::ConnectingFrom(_) => {
                // Visual feedback handled in draw
            }
            DragState::None => {}
        }
    }

    // Release drag
    if response.drag_stopped_by(egui::PointerButton::Primary) {
        if let DragState::ConnectingFrom(ref src_id) = state.drag_state {
            if let Some(pos) = pointer {
                let world_pos = transform.screen_to_world(pos);
                if let Some(target_id) = hit_test_room(world_pos, &state.room_positions) {
                    if target_id != *src_id {
                        let conn = Connection::new(ConnectionType::Door);
                        let conn_id = conn.id.clone();
                        dungeon
                            .graph
                            .add_connection(src_id.clone(), target_id, conn);
                        state.selection = Selection::Connection(conn_id);
                    }
                }
            }
        }
        state.drag_state = DragState::None;
    }

    // Delete key
    if response.has_focus() || response.hovered() {
        let delete_pressed = ui.input(|i| {
            i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace)
        });
        if delete_pressed {
            match &state.selection {
                Selection::Room(id) => {
                    let id = id.clone();
                    state.room_positions.remove(&id);
                    dungeon.graph.remove_room(&id);
                    state.selection = Selection::None;
                }
                Selection::Connection(id) => {
                    let id = id.clone();
                    dungeon.graph.remove_connection(&id);
                    state.selection = Selection::None;
                }
                Selection::None => {}
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

fn point_to_segment_dist(p: egui::Pos2, a: egui::Pos2, b: egui::Pos2) -> f32 {
    let ab = b - a;
    let ap = p - a;
    let t = (ap.dot(ab) / ab.dot(ab)).clamp(0.0, 1.0);
    let closest = a + ab * t;
    p.distance(closest)
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

fn draw_connections(
    painter: &egui::Painter,
    transform: &ViewTransform,
    dungeon: &Dungeon,
    state: &GraphEditorState,
) {
    // Count connections between each room pair to offset duplicates
    let mut pair_counts: HashMap<(String, String), usize> = HashMap::new();
    let mut pair_indices: HashMap<String, usize> = HashMap::new();
    for edge in &dungeon.graph.connections {
        let key = if edge.source_room_id < edge.target_room_id {
            (edge.source_room_id.clone(), edge.target_room_id.clone())
        } else {
            (edge.target_room_id.clone(), edge.source_room_id.clone())
        };
        let idx = *pair_counts.get(&key).unwrap_or(&0);
        pair_indices.insert(edge.connection.id.clone(), idx);
        *pair_counts.entry(key).or_insert(0) += 1;
    }

    for edge in &dungeon.graph.connections {
        let src_pos = state.room_positions.get(&edge.source_room_id);
        let tgt_pos = state.room_positions.get(&edge.target_room_id);

        if let (Some(&src), Some(&tgt)) = (src_pos, tgt_pos) {
            let key = if edge.source_room_id < edge.target_room_id {
                (edge.source_room_id.clone(), edge.target_room_id.clone())
            } else {
                (edge.target_room_id.clone(), edge.source_room_id.clone())
            };
            let count = pair_counts[&key];
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

            let is_selected = matches!(&state.selection, Selection::Connection(id) if *id == edge.connection.id);

            let (color, width) = if is_selected {
                (egui::Color32::from_rgb(100, 200, 255), 3.0)
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

            let is_selected = matches!(&state.selection, Selection::Room(id) if *id == room.id);

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
