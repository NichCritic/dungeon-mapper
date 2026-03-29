use crate::model::*;
use crate::ui::canvas_common::{handle_pan_zoom, ViewState};
use crate::util::{grid_to_world, world_to_grid, ViewTransform, GRID_PX};

/// What's currently being dragged in the spatial view
#[derive(Clone, Debug, Default)]
enum DragTarget {
    #[default]
    None,
    Room(String),
    /// Dragging a corridor waypoint: (corridor index, waypoint index)
    Waypoint(usize, usize),
}

pub struct SpatialViewState {
    pub view: ViewState,
    pub selected_room: Option<String>,
    /// Index of the selected corridor (if any)
    pub selected_corridor: Option<usize>,
    /// Index of the selected waypoint within the selected corridor
    pub selected_waypoint: Option<usize>,
    drag_target: DragTarget,
    drag_accum: egui::Vec2,
    pub density_gap: u32,
    pub corridor_width: u32,
}

impl Default for SpatialViewState {
    fn default() -> Self {
        Self {
            view: ViewState::default(),
            selected_room: None,
            selected_corridor: None,
            selected_waypoint: None,
            drag_target: DragTarget::None,
            drag_accum: egui::Vec2::ZERO,
            density_gap: 2,
            corridor_width: 2,
        }
    }
}

const HANDLE_RADIUS: f32 = 5.0;
const HANDLE_HIT_RADIUS: f32 = 8.0;

pub fn spatial_view(ui: &mut egui::Ui, dungeon: &mut Dungeon, state: &mut SpatialViewState) {
    let (response, painter) = ui.allocate_painter(
        ui.available_size(),
        egui::Sense::click_and_drag(),
    );
    let rect = response.rect;

    painter.rect_filled(rect, 0.0, egui::Color32::from_rgb(40, 40, 45));

    handle_pan_zoom(&response, &mut state.view);
    let transform = ViewTransform::new(state.view.offset, state.view.zoom, rect);

    if let Some(layout) = &dungeon.layout {
        draw_infinite_grid(&painter, &transform, rect);
        draw_bounds(&painter, &transform, layout);
        draw_corridors(&painter, &transform, layout, state);
        draw_rooms(&painter, &transform, layout, &dungeon.graph, state);
        draw_waypoint_handles(&painter, &transform, layout, state);
    } else if !dungeon.graph.rooms.is_empty() {
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            "Layout will be generated automatically.",
            egui::FontId::proportional(16.0),
            egui::Color32::from_rgb(150, 150, 150),
        );
    } else {
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            "Add rooms in the Graph tab first.",
            egui::FontId::proportional(16.0),
            egui::Color32::from_rgb(150, 150, 150),
        );
    }

    if dungeon.layout.is_some() {
        handle_spatial_interactions(ui, &response, &transform, dungeon, state);
    }
}

fn handle_spatial_interactions(
    ui: &egui::Ui,
    response: &egui::Response,
    transform: &ViewTransform,
    dungeon: &mut Dungeon,
    state: &mut SpatialViewState,
) {
    // === DRAG START ===
    if response.drag_started_by(egui::PointerButton::Primary) {
        if let Some(pos) = response.hover_pos() {
            let world = transform.screen_to_world(pos);

            // First check: waypoint handles (highest priority when a corridor is selected)
            if let Some(ci) = state.selected_corridor {
                if let Some(layout) = &dungeon.layout {
                    if ci < layout.corridors.len() {
                        let corridor = &layout.corridors[ci];
                        for (wi, wp) in corridor.waypoints.iter().enumerate() {
                            let wp_screen = transform.world_to_screen(
                                egui::pos2(grid_to_world(wp.x), grid_to_world(wp.y)),
                            );
                            if pos.distance(wp_screen) < HANDLE_HIT_RADIUS * state.view.zoom {
                                state.selected_waypoint = Some(wi);
                                state.drag_target = DragTarget::Waypoint(ci, wi);
                                state.drag_accum = egui::Vec2::ZERO;
                                return;
                            }
                        }
                    }
                }
            }

            // Second check: rooms
            let gx = world_to_grid(world.x);
            let gy = world_to_grid(world.y);
            if let Some(layout) = &dungeon.layout {
                for rl in &layout.rooms {
                    if gx >= rl.x
                        && gx < rl.x + rl.width as i32
                        && gy >= rl.y
                        && gy < rl.y + rl.height as i32
                    {
                        state.selected_room = Some(rl.room_id.clone());
                        state.selected_corridor = None;
                        state.selected_waypoint = None;
                        state.drag_target = DragTarget::Room(rl.room_id.clone());
                        state.drag_accum = egui::Vec2::ZERO;
                        return;
                    }
                }
            }
        }
    }

    // === DOUBLE-CLICK — insert waypoint on selected corridor segment ===
    if response.double_clicked() {
        if let Some(pos) = response.hover_pos() {
            let world = transform.screen_to_world(pos);
            if let Some(ci) = state.selected_corridor {
                if let Some(layout) = &mut dungeon.layout {
                    if ci < layout.corridors.len() {
                        let corridor = &layout.corridors[ci];
                        // Find which segment was double-clicked
                        let mut best_seg: Option<(usize, f32)> = None;
                        for (si, pair) in corridor.waypoints.windows(2).enumerate() {
                            let a = egui::pos2(grid_to_world(pair[0].x), grid_to_world(pair[0].y));
                            let b = egui::pos2(grid_to_world(pair[1].x), grid_to_world(pair[1].y));
                            let dist = point_to_segment_dist(world, a, b);
                            let threshold = (corridor.width as f32 * GRID_PX / 2.0 + 6.0) / state.view.zoom;
                            if dist < threshold {
                                if best_seg.map_or(true, |(_, bd)| dist < bd) {
                                    best_seg = Some((si, dist));
                                }
                            }
                        }
                        if let Some((si, _)) = best_seg {
                            let new_wp = GridPos {
                                x: world_to_grid(world.x),
                                y: world_to_grid(world.y),
                            };
                            layout.corridors[ci].waypoints.insert(si + 1, new_wp);
                            layout.corridors[ci].pinned_waypoints =
                                layout.corridors[ci].waypoints.clone();
                            state.selected_waypoint = Some(si + 1);
                        }
                    }
                }
            }
        }
    }

    // === CLICK (no drag) — select corridors / waypoints ===
    if response.clicked() && !response.double_clicked() {
        if let Some(pos) = response.hover_pos() {
            let world = transform.screen_to_world(pos);

            // First: if corridor selected, check waypoint handle click
            if let Some(ci) = state.selected_corridor {
                if let Some(layout) = &dungeon.layout {
                    if ci < layout.corridors.len() {
                        let corridor = &layout.corridors[ci];
                        for (wi, wp) in corridor.waypoints.iter().enumerate() {
                            let wp_screen = transform.world_to_screen(
                                egui::pos2(grid_to_world(wp.x), grid_to_world(wp.y)),
                            );
                            if pos.distance(wp_screen) < HANDLE_HIT_RADIUS * state.view.zoom {
                                state.selected_waypoint = Some(wi);
                                return;
                            }
                        }
                    }
                }
            }

            // Check corridor segment hit
            if let Some(layout) = &dungeon.layout {
                let mut hit_corridor = None;
                for (ci, corridor) in layout.corridors.iter().enumerate() {
                    for pair in corridor.waypoints.windows(2) {
                        let a = egui::pos2(grid_to_world(pair[0].x), grid_to_world(pair[0].y));
                        let b = egui::pos2(grid_to_world(pair[1].x), grid_to_world(pair[1].y));
                        let dist = point_to_segment_dist(world, a, b);
                        let threshold = (corridor.width as f32 * GRID_PX / 2.0 + 4.0) / state.view.zoom;
                        if dist < threshold {
                            hit_corridor = Some(ci);
                            break;
                        }
                    }
                    if hit_corridor.is_some() {
                        break;
                    }
                }

                if let Some(ci) = hit_corridor {
                    state.selected_corridor = Some(ci);
                    state.selected_waypoint = None;
                    state.selected_room = None;
                } else {
                    // Check room hit
                    let gx = world_to_grid(world.x);
                    let gy = world_to_grid(world.y);
                    let mut hit_room = false;
                    for rl in &layout.rooms {
                        if gx >= rl.x
                            && gx < rl.x + rl.width as i32
                            && gy >= rl.y
                            && gy < rl.y + rl.height as i32
                        {
                            state.selected_room = Some(rl.room_id.clone());
                            state.selected_corridor = None;
                            state.selected_waypoint = None;
                            hit_room = true;
                            break;
                        }
                    }
                    if !hit_room {
                        state.selected_waypoint = None;
                    }
                }
            }
        }
    }

    // === DELETE KEY — remove selected waypoint ===
    if response.has_focus() || response.hovered() {
        let delete_pressed = ui.input(|i| {
            i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace)
        });
        if delete_pressed {
            if let (Some(ci), Some(wi)) = (state.selected_corridor, state.selected_waypoint) {
                if let Some(layout) = &mut dungeon.layout {
                    if ci < layout.corridors.len() {
                        let len = layout.corridors[ci].waypoints.len();
                        // Don't delete if only 2 waypoints left (start + end)
                        // Don't delete start (0) or end (len-1)
                        if len > 2 && wi > 0 && wi < len - 1 {
                            layout.corridors[ci].waypoints.remove(wi);
                            layout.corridors[ci].pinned_waypoints =
                                layout.corridors[ci].waypoints.clone();
                            state.selected_waypoint = None;
                            layout.recheck_corridor_overlaps();
                        }
                    }
                }
            }
        }
    }

    // === DRAGGING ===
    if response.dragged_by(egui::PointerButton::Primary) {
        state.drag_accum += response.drag_delta() / state.view.zoom;

        let grid_steps_x = (state.drag_accum.x / GRID_PX).round() as i32;
        let grid_steps_y = (state.drag_accum.y / GRID_PX).round() as i32;

        if grid_steps_x != 0 || grid_steps_y != 0 {
            match &state.drag_target {
                DragTarget::Room(room_id) => {
                    let room_id = room_id.clone();

                    // Find which connections are attached to this room
                    let connected_ids: Vec<(String, bool, bool)> = dungeon.graph.connections
                        .iter()
                        .filter_map(|e| {
                            let is_src = e.source_room_id == room_id;
                            let is_tgt = e.target_room_id == room_id;
                            if is_src || is_tgt {
                                Some((e.connection.id.clone(), is_src, is_tgt))
                            } else {
                                None
                            }
                        })
                        .collect();

                    if let Some(layout) = &mut dungeon.layout {
                        // Move the room
                        if let Some(rl) = layout.room_by_id_mut(&room_id) {
                            rl.x += grid_steps_x;
                            rl.y += grid_steps_y;
                        }

                        // Shift pinned waypoints for connected corridors
                        for corridor in &mut layout.corridors {
                            for (conn_id, is_src, is_tgt) in &connected_ids {
                                if corridor.connection_id != *conn_id {
                                    continue;
                                }
                                if !corridor.pinned_waypoints.is_empty() {
                                    // Shift the start waypoint if this room is the source
                                    if *is_src {
                                        corridor.pinned_waypoints.first_mut().unwrap().x += grid_steps_x;
                                        corridor.pinned_waypoints.first_mut().unwrap().y += grid_steps_y;
                                    }
                                    // Shift the end waypoint if this room is the target
                                    if *is_tgt {
                                        corridor.pinned_waypoints.last_mut().unwrap().x += grid_steps_x;
                                        corridor.pinned_waypoints.last_mut().unwrap().y += grid_steps_y;
                                    }
                                }
                            }
                        }
                    }
                }
                DragTarget::Waypoint(ci, wi) => {
                    let ci = *ci;
                    let wi = *wi;
                    if let Some(layout) = &mut dungeon.layout {
                        if ci < layout.corridors.len() && wi < layout.corridors[ci].waypoints.len()
                        {
                            layout.corridors[ci].waypoints[wi].x += grid_steps_x;
                            layout.corridors[ci].waypoints[wi].y += grid_steps_y;
                        }
                    }
                }
                DragTarget::None => {}
            }
            state.drag_accum.x -= grid_steps_x as f32 * GRID_PX;
            state.drag_accum.y -= grid_steps_y as f32 * GRID_PX;
        }
    }

    // === DRAG STOP ===
    if response.drag_stopped_by(egui::PointerButton::Primary) {
        match &state.drag_target {
            DragTarget::Room(_) => {
                if let Some(layout) = &mut dungeon.layout {
                    layout.corridors =
                        crate::solver::corridor::route_corridors(&dungeon.graph, layout, state.corridor_width);
                    layout.recheck_corridor_overlaps();
                }
            }
            DragTarget::Waypoint(ci, _) => {
                let ci = *ci;
                if let Some(layout) = &mut dungeon.layout {
                    // Pin all current waypoints so the solver routes through them
                    if ci < layout.corridors.len() {
                        layout.corridors[ci].pinned_waypoints =
                            layout.corridors[ci].waypoints.clone();
                    }
                    layout.recheck_corridor_overlaps();
                }
            }
            DragTarget::None => {}
        }
        state.drag_target = DragTarget::None;
    }
}

fn point_to_segment_dist(p: egui::Pos2, a: egui::Pos2, b: egui::Pos2) -> f32 {
    let ab = b - a;
    let ap = p - a;
    let len_sq = ab.dot(ab);
    if len_sq < 0.001 {
        return p.distance(a);
    }
    let t = (ap.dot(ab) / len_sq).clamp(0.0, 1.0);
    let closest = a + ab * t;
    p.distance(closest)
}

/// Draw an infinite grid based on the visible viewport.
fn draw_infinite_grid(painter: &egui::Painter, transform: &ViewTransform, canvas_rect: egui::Rect) {
    let light = egui::Color32::from_rgba_premultiplied(80, 80, 80, 40);
    let heavy = egui::Color32::from_rgba_premultiplied(100, 100, 100, 60);

    let top_left = transform.screen_to_world(canvas_rect.min);
    let bottom_right = transform.screen_to_world(canvas_rect.max);

    let min_gx = (top_left.x / GRID_PX).floor() as i32 - 1;
    let max_gx = (bottom_right.x / GRID_PX).ceil() as i32 + 1;
    let min_gy = (top_left.y / GRID_PX).floor() as i32 - 1;
    let max_gy = (bottom_right.y / GRID_PX).ceil() as i32 + 1;

    for x in min_gx..=max_gx {
        let color = if x % 5 == 0 { heavy } else { light };
        let from = transform.world_to_screen(egui::pos2(grid_to_world(x), grid_to_world(min_gy)));
        let to = transform.world_to_screen(egui::pos2(grid_to_world(x), grid_to_world(max_gy)));
        painter.line_segment([from, to], egui::Stroke::new(1.0, color));
    }
    for y in min_gy..=max_gy {
        let color = if y % 5 == 0 { heavy } else { light };
        let from = transform.world_to_screen(egui::pos2(grid_to_world(min_gx), grid_to_world(y)));
        let to = transform.world_to_screen(egui::pos2(grid_to_world(max_gx), grid_to_world(y)));
        painter.line_segment([from, to], egui::Stroke::new(1.0, color));
    }
}

fn draw_bounds(painter: &egui::Painter, transform: &ViewTransform, layout: &SpatialLayout) {
    let color = egui::Color32::from_rgb(200, 160, 60);

    for b in &layout.bounds {
        let min = transform.world_to_screen(egui::pos2(grid_to_world(b.x), grid_to_world(b.y)));
        let max = transform.world_to_screen(egui::pos2(
            grid_to_world(b.x + b.width as i32),
            grid_to_world(b.y + b.height as i32),
        ));

        let stroke = egui::Stroke::new(2.0, color);
        let dash = 8.0;
        let gap = 4.0;

        draw_dashed_line(painter, egui::pos2(min.x, min.y), egui::pos2(max.x, min.y), stroke, dash, gap);
        draw_dashed_line(painter, egui::pos2(max.x, min.y), egui::pos2(max.x, max.y), stroke, dash, gap);
        draw_dashed_line(painter, egui::pos2(max.x, max.y), egui::pos2(min.x, max.y), stroke, dash, gap);
        draw_dashed_line(painter, egui::pos2(min.x, max.y), egui::pos2(min.x, min.y), stroke, dash, gap);

        if !b.label.is_empty() {
            painter.text(
                egui::pos2(min.x + 4.0, min.y - 12.0),
                egui::Align2::LEFT_BOTTOM,
                &b.label,
                egui::FontId::monospace(11.0 * transform.zoom),
                color,
            );
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

fn draw_corridors(
    painter: &egui::Painter,
    transform: &ViewTransform,
    layout: &SpatialLayout,
    state: &SpatialViewState,
) {
    for (ci, corridor) in layout.corridors.iter().enumerate() {
        let is_selected = state.selected_corridor == Some(ci);
        let color = if corridor.invalid {
            egui::Color32::from_rgb(220, 50, 50)
        } else if is_selected {
            egui::Color32::from_rgb(130, 200, 255)
        } else {
            egui::Color32::from_rgb(180, 180, 180)
        };

        let w = corridor.width as i32;
        let half = w / 2; // integer: same offset used by solver

        // Draw each segment as a filled rectangle on the grid.
        // Waypoints are center coords. The block covers cells
        // from (center - half) to (center - half + w).
        for pair in corridor.waypoints.windows(2) {
            let x1 = pair[0].x;
            let y1 = pair[0].y;
            let x2 = pair[1].x;
            let y2 = pair[1].y;

            let min_x = x1.min(x2) - half;
            let min_y = y1.min(y2) - half;
            let max_x = x1.max(x2) - half + w;
            let max_y = y1.max(y2) - half + w;

            let screen_min = transform.world_to_screen(egui::pos2(
                grid_to_world(min_x),
                grid_to_world(min_y),
            ));
            let screen_max = transform.world_to_screen(egui::pos2(
                grid_to_world(max_x),
                grid_to_world(max_y),
            ));

            let rect = egui::Rect::from_min_max(screen_min, screen_max);
            painter.rect_filled(rect, 0.0, color);
        }
    }
}

/// Draw draggable handles at each waypoint of the selected corridor.
fn draw_waypoint_handles(
    painter: &egui::Painter,
    transform: &ViewTransform,
    layout: &SpatialLayout,
    state: &SpatialViewState,
) {
    let Some(ci) = state.selected_corridor else {
        return;
    };
    let Some(corridor) = layout.corridors.get(ci) else {
        return;
    };

    let handle_r = HANDLE_RADIUS * state.view.zoom;

    for (wi, wp) in corridor.waypoints.iter().enumerate() {
        let screen = transform.world_to_screen(
            egui::pos2(grid_to_world(wp.x), grid_to_world(wp.y)),
        );

        let is_endpoint = wi == 0 || wi == corridor.waypoints.len() - 1;
        let is_dragging = matches!(state.drag_target, DragTarget::Waypoint(c, w) if c == ci && w == wi);
        let is_selected = state.selected_waypoint == Some(wi);

        let fill = if is_dragging {
            egui::Color32::from_rgb(255, 220, 80)
        } else if is_selected {
            egui::Color32::from_rgb(255, 180, 50)
        } else if is_endpoint {
            egui::Color32::from_rgb(80, 220, 120)
        } else {
            egui::Color32::from_rgb(100, 180, 255)
        };

        let stroke_color = egui::Color32::WHITE;

        if is_endpoint {
            // Diamond shape for endpoints
            let s = handle_r * 1.2;
            let points = vec![
                egui::pos2(screen.x, screen.y - s),
                egui::pos2(screen.x + s, screen.y),
                egui::pos2(screen.x, screen.y + s),
                egui::pos2(screen.x - s, screen.y),
            ];
            painter.add(egui::Shape::convex_polygon(
                points,
                fill,
                egui::Stroke::new(1.5, stroke_color),
            ));
        } else {
            // Circle for mid-waypoints
            painter.circle(screen, handle_r, fill, egui::Stroke::new(1.5, stroke_color));
        }
    }
}

fn draw_rooms(
    painter: &egui::Painter,
    transform: &ViewTransform,
    layout: &SpatialLayout,
    graph: &DungeonGraph,
    state: &SpatialViewState,
) {
    for rl in &layout.rooms {
        let min = transform.world_to_screen(egui::pos2(grid_to_world(rl.x), grid_to_world(rl.y)));
        let max = transform.world_to_screen(egui::pos2(
            grid_to_world(rl.x + rl.width as i32),
            grid_to_world(rl.y + rl.height as i32),
        ));
        let rect = egui::Rect::from_min_max(min, max);

        let is_selected = state.selected_room.as_deref() == Some(&rl.room_id);
        let room = graph.room_by_id(&rl.room_id);
        let is_circle = room.map_or(false, |r| r.shape == RoomShape::Circle);

        let fill = egui::Color32::from_rgb(220, 220, 220);
        let border_color = if is_selected {
            egui::Color32::from_rgb(100, 200, 255)
        } else {
            egui::Color32::from_rgb(60, 60, 60)
        };
        let stroke = egui::Stroke::new(2.0, border_color);

        if is_circle {
            let center = rect.center();
            let radius = rect.width().min(rect.height()) / 2.0;
            painter.circle_filled(center, radius, fill);
            painter.circle_stroke(center, radius, stroke);
        } else {
            painter.rect_filled(rect, 0.0, fill);
            painter.rect_stroke(rect, 0.0, stroke, egui::StrokeKind::Middle);
        }

        if let Some(room) = room {
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                &room.label,
                egui::FontId::monospace(11.0 * transform.zoom),
                egui::Color32::from_rgb(30, 30, 30),
            );
        }
    }
}

pub fn spatial_sidebar(ui: &mut egui::Ui, dungeon: &mut Dungeon, state: &mut SpatialViewState) {
    ui.heading("Spatial Layout");
    ui.separator();

    ui.label("Density gap:");
    ui.add(egui::Slider::new(&mut state.density_gap, 1..=6));

    ui.add_space(8.0);
    ui.label("Default corridor width:");
    ui.add(egui::Slider::new(&mut state.corridor_width, 1..=4).suffix(" sq"));

    // Bounds management
    ui.add_space(16.0);
    ui.heading("Bounds");
    ui.separator();

    if ui.button("Add Bounds Rectangle").clicked() {
        if let Some(layout) = &mut dungeon.layout {
            let (min_x, min_y, max_x, max_y) = layout.extents();
            let margin = 2;
            layout.bounds.push(BoundsRect {
                label: format!("Bounds {}", layout.bounds.len() + 1),
                x: min_x - margin,
                y: min_y - margin,
                width: (max_x - min_x + margin * 2) as u32,
                height: (max_y - min_y + margin * 2) as u32,
            });
        }
    }

    if let Some(layout) = &mut dungeon.layout {
        let mut to_remove = None;
        for (i, b) in layout.bounds.iter_mut().enumerate() {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.text_edit_singleline(&mut b.label);
                if ui.small_button("X").clicked() {
                    to_remove = Some(i);
                }
            });
            ui.horizontal(|ui| {
                ui.add(egui::DragValue::new(&mut b.x).prefix("x: "));
                ui.add(egui::DragValue::new(&mut b.y).prefix("y: "));
            });
            ui.horizontal(|ui| {
                ui.add(egui::DragValue::new(&mut b.width).range(1..=500).prefix("w: "));
                ui.add(egui::DragValue::new(&mut b.height).range(1..=500).prefix("h: "));
            });
        }
        if let Some(i) = to_remove {
            layout.bounds.remove(i);
        }
    }

    // Selected room info
    if let Some(ref room_id) = state.selected_room {
        ui.add_space(16.0);
        ui.separator();
        if let Some(layout) = &dungeon.layout {
            if let Some(rl) = layout.room_by_id(room_id) {
                ui.label(format!("Position: ({}, {})", rl.x, rl.y));
                ui.label(format!(
                    "Size: {}x{} ({}x{} ft)",
                    rl.width,
                    rl.height,
                    rl.width * 5,
                    rl.height * 5
                ));
            }
        }
    }

    // Selected corridor info
    if let Some(ci) = state.selected_corridor {
        ui.add_space(16.0);
        ui.separator();
        if let Some(layout) = &dungeon.layout {
            if let Some(corridor) = layout.corridors.get(ci) {
                ui.label(format!("Corridor: {} waypoints", corridor.waypoints.len()));
                if corridor.invalid {
                    ui.colored_label(egui::Color32::from_rgb(220, 50, 50), "Invalid (overlapping)");
                }
            }
        }
    }
}
