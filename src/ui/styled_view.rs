use std::hash::{Hash, Hasher};

use std::collections::HashSet;

use crate::model::*;
use crate::render::recording::{RecordingRenderer, RenderCommand, replay_commands};
use crate::render::themed::RenderOptions;
use crate::ui::canvas_common::{handle_pan_zoom, truncate_to_fit, ViewState, COLOR_PLACEHOLDER_TEXT};
use crate::ui::spatial_view::collect_floors;
use crate::util::{grid_to_world, ViewTransform, GRID_PX};

/// Cached world-space drawing commands from render_themed.
struct RenderCache {
    commands: Vec<RenderCommand>,
    input_hash: u64,
}

pub struct StyledViewState {
    pub view: ViewState,
    pub show_grid: bool,
    pub show_labels: bool,
    pub show_notes: bool,
    pub show_secrets: bool,
    pub current_floor: Option<i32>,
    render_cache: Option<RenderCache>,
    /// Set by sidebar export buttons, consumed by app.rs to dispatch async export.
    pub export_requested: Option<bool>,
    /// Room selected on the canvas (for contextual sidebar info).
    pub selected_room: Option<String>,
}

impl Default for StyledViewState {
    fn default() -> Self {
        Self {
            view: ViewState::default(),
            show_grid: true,
            show_labels: true,
            show_notes: true,
            show_secrets: true,
            current_floor: None,
            render_cache: None,
            export_requested: None,
            selected_room: None,
        }
    }
}

/// Compute a hash over all inputs that affect the cached render commands.
/// Text (labels, notes, secret "S" markers) is drawn as a live overlay
/// so show_labels/show_notes don't trigger a cache rebuild.
fn render_input_hash(layout: &SpatialLayout, graph: &DungeonGraph, theme: &Theme, show_grid: bool, current_floor: Option<i32>) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    let mut h = DefaultHasher::new();
    layout.rooms.len().hash(&mut h);
    for rl in &layout.rooms {
        rl.room_id.hash(&mut h);
        rl.x.hash(&mut h);
        rl.y.hash(&mut h);
        rl.width.hash(&mut h);
        rl.height.hash(&mut h);
        // Hash cave generation counter so cell edits invalidate cache
        if let Some(room) = graph.room_by_id(&rl.room_id) {
            if let Some(cave) = &room.cave_data {
                cave.generation.hash(&mut h);
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
    (theme.exterior_shading as u8).hash(&mut h);
    theme.shading_radius.to_bits().hash(&mut h);
    (theme.shading_style as u8).hash(&mut h);
    theme.hatching_density.to_bits().hash(&mut h);
    (theme.corridor_chamfer as u8).hash(&mut h);
    show_grid.hash(&mut h);
    current_floor.hash(&mut h);
    h.finish()
}

pub fn styled_view(ui: &mut egui::Ui, dungeon: &Dungeon, state: &mut StyledViewState) {
    let (response, painter) = ui.allocate_painter(
        ui.available_size(),
        egui::Sense::click_and_drag(),
    );
    let rect = response.rect;

    let bg = dungeon.theme.bg_color;
    painter.rect_filled(rect, 0.0, egui::Color32::from_rgba_unmultiplied(bg[0], bg[1], bg[2], bg[3]));

    handle_pan_zoom(&response, &mut state.view);
    let transform = ViewTransform::new(state.view.offset, state.view.zoom, rect);

    if let Some(layout) = &dungeon.layout {
        // Build floor-filtered layout if a floor is selected
        let filtered_layout;
        let render_layout = if let Some(floor) = state.current_floor {
            let visible_room_ids: HashSet<&str> = dungeon.graph.rooms.iter()
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
        let hash = render_input_hash(layout, &dungeon.graph, &dungeon.theme, state.show_grid, state.current_floor);
        let needs_rebuild = state.render_cache.as_ref()
            .is_none_or(|c| c.input_hash != hash);

        if needs_rebuild {
            let mut recorder = RecordingRenderer::new();
            let options = RenderOptions {
                show_grid: state.show_grid,
                show_labels: true,
                show_notes: true,
                show_secrets: true,
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

        // Replay cached commands through egui painter with current transform
        if let Some(cache) = &state.render_cache {
            replay_commands(&painter, &transform, &cache.commands);
        }

        // Draw lower-floor room/corridor silhouettes as dark semi-transparent shapes
        if let Some(floor) = state.current_floor {
            let ghost_fill = egui::Color32::from_rgba_unmultiplied(50, 50, 60, 70);
            let ghost_stroke = egui::Stroke::new(1.5, egui::Color32::from_rgba_unmultiplied(70, 70, 80, 90));
            // Lower-floor rooms
            for rl in &layout.rooms {
                if let Some(room) = dungeon.graph.room_by_id(&rl.room_id) {
                    if !room.floor.visible_on(floor) && room.floor.floors().iter().all(|f| *f < floor) {
                        let min = transform.world_to_screen(egui::pos2(
                            rl.x as f32 * GRID_PX,
                            rl.y as f32 * GRID_PX,
                        ));
                        let max = transform.world_to_screen(egui::pos2(
                            (rl.x + rl.width as i32) as f32 * GRID_PX,
                            (rl.y + rl.height as i32) as f32 * GRID_PX,
                        ));
                        let r = egui::Rect::from_min_max(min, max);
                        match room.shape {
                            crate::model::RoomShape::Circle => {
                                let center = r.center();
                                let radius = r.width().min(r.height()) / 2.0;
                                painter.circle_filled(center, radius, ghost_fill);
                                painter.circle_stroke(center, radius, ghost_stroke);
                            }
                            _ => {
                                painter.rect_filled(r, 0.0, ghost_fill);
                                painter.rect_stroke(r, 0.0, ghost_stroke, egui::StrokeKind::Middle);
                            }
                        }
                        // Ghost label
                        let cx = (rl.x as f32 + rl.width as f32 / 2.0) * GRID_PX;
                        let cy = (rl.y as f32 + rl.height as f32 / 2.0) * GRID_PX;
                        let screen = transform.world_to_screen(egui::pos2(cx, cy));
                        let max_width = rl.width as f32 * GRID_PX * transform.zoom;
                        let font = egui::FontId::monospace(10.0 * transform.zoom);
                        let label = truncate_to_fit(&painter, &room.label, &font, max_width);
                        painter.text(
                            screen,
                            egui::Align2::CENTER_CENTER,
                            &label,
                            font,
                            egui::Color32::from_rgba_unmultiplied(80, 80, 90, 100),
                        );
                    }
                }
            }
            // Lower-floor corridors
            for corridor in &layout.corridors {
                if let Some(edge) = dungeon.graph.connections.iter().find(|e| e.connection.id == corridor.connection_id) {
                    let src_on = dungeon.graph.room_by_id(&edge.source_room_id)
                        .is_some_and(|r| r.floor.visible_on(floor));
                    let tgt_on = dungeon.graph.room_by_id(&edge.target_room_id)
                        .is_some_and(|r| r.floor.visible_on(floor));
                    if src_on || tgt_on { continue; }
                    let src_lower = dungeon.graph.room_by_id(&edge.source_room_id)
                        .is_some_and(|r| r.floor.floors().iter().all(|f| *f < floor));
                    let tgt_lower = dungeon.graph.room_by_id(&edge.target_room_id)
                        .is_some_and(|r| r.floor.floors().iter().all(|f| *f < floor));
                    if !src_lower && !tgt_lower { continue; }

                    let w = corridor.width as i32;
                    let half = w / 2;
                    for pair in corridor.waypoints.windows(2) {
                        let x1 = pair[0].x;
                        let y1 = pair[0].y;
                        let x2 = pair[1].x;
                        let y2 = pair[1].y;
                        let min_x = x1.min(x2) - half;
                        let min_y = y1.min(y2) - half;
                        let max_x = x1.max(x2) - half + w;
                        let max_y = y1.max(y2) - half + w;
                        let smin = transform.world_to_screen(egui::pos2(
                            grid_to_world(min_x), grid_to_world(min_y),
                        ));
                        let smax = transform.world_to_screen(egui::pos2(
                            grid_to_world(max_x), grid_to_world(max_y),
                        ));
                        painter.rect_filled(
                            egui::Rect::from_min_max(smin, smax),
                            0.0,
                            ghost_fill,
                        );
                    }
                }
            }
        }

        // Live text overlay (labels, notes, secret door markers)
        draw_text_overlay(&painter, &transform, &dungeon.graph, render_layout, &dungeon.theme, state);

        // Click to select/deselect a room
        if response.clicked() {
            if let Some(pos) = response.interact_pointer_pos() {
                let world = transform.screen_to_world(pos);
                let gx = (world.x / GRID_PX).floor() as i32;
                let gy = (world.y / GRID_PX).floor() as i32;
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
    } else {
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            "Generate a layout first (Spatial tab).",
            egui::FontId::proportional(16.0),
            COLOR_PLACEHOLDER_TEXT,
        );
    }
}

/// Draw text elements that the recording renderer skips:
/// room labels, notes, and secret door "S" markers.
fn draw_text_overlay(
    painter: &egui::Painter,
    transform: &ViewTransform,
    graph: &DungeonGraph,
    layout: &SpatialLayout,
    theme: &Theme,
    state: &StyledViewState,
) {
    // Secret door "S" markers
    if state.show_secrets {
        for edge in &graph.connections {
            if edge.connection.connection_type != ConnectionType::Secret {
                continue;
            }
            let corridor = layout.corridors.iter().find(|c| c.connection_id == edge.connection.id);
            let Some(corridor) = corridor else { continue };
            if corridor.waypoints.len() < 2 { continue; }

            let room_ids = [&edge.source_room_id, &edge.target_room_id];
            let wp_ends = [&corridor.waypoints[0], corridor.waypoints.last().unwrap()];

            for (room_id, wp) in room_ids.iter().zip(wp_ends.iter()) {
                let Some(rl) = layout.room_by_id(room_id) else { continue };
                let wp_cx = wp.x as f32;
                let wp_cy = wp.y as f32;
                let dist_right = (wp_cx - (rl.x + rl.width as i32) as f32).abs();
                let dist_left = (wp_cx - rl.x as f32).abs();
                let dist_bottom = (wp_cy - (rl.y + rl.height as i32) as f32).abs();
                let dist_top = (wp_cy - rl.y as f32).abs();
                let min_dist = dist_right.min(dist_left).min(dist_bottom).min(dist_top);

                let (cx, cy) = if min_dist == dist_right {
                    let wall_x = (rl.x + rl.width as i32) as f32;
                    (wall_x, wp_cy)
                } else if min_dist == dist_left {
                    (rl.x as f32, wp_cy)
                } else if min_dist == dist_bottom {
                    let wall_y = (rl.y + rl.height as i32) as f32;
                    (wp_cx, wall_y)
                } else {
                    (wp_cx, rl.y as f32)
                };

                let screen = transform.world_to_screen(egui::pos2(cx * GRID_PX, cy * GRID_PX));
                let wc = theme.wall_color;
                painter.text(
                    screen,
                    egui::Align2::CENTER_CENTER,
                    "S",
                    egui::FontId::monospace((6.0 * transform.zoom).max(4.0)),
                    egui::Color32::from_rgba_unmultiplied(wc[0], wc[1], wc[2], wc[3]),
                );
            }
        }
    }

    // Room labels and notes
    if state.show_labels {
        for rl in &layout.rooms {
            if let Some(room) = graph.room_by_id(&rl.room_id) {
                let cx = (rl.x as f32 + rl.width as f32 / 2.0) * GRID_PX;
                let cy = (rl.y as f32 + rl.height as f32 / 2.0) * GRID_PX;
                let screen = transform.world_to_screen(egui::pos2(cx, cy));
                let max_width = rl.width as f32 * GRID_PX * transform.zoom;
                let font = egui::FontId::monospace(10.0 * transform.zoom);
                let label = truncate_to_fit(painter, &room.label, &font, max_width);

                painter.text(
                    screen,
                    egui::Align2::CENTER_CENTER,
                    &label,
                    font.clone(),
                    egui::Color32::from_rgb(60, 60, 60),
                );

                if state.show_notes && !room.notes.is_empty() {
                    let notes_screen = transform.world_to_screen(egui::pos2(cx, cy + 14.0));
                    let notes_font = egui::FontId::monospace(7.0 * transform.zoom);
                    let notes = truncate_to_fit(painter, &room.notes, &notes_font, max_width);
                    painter.text(
                        notes_screen,
                        egui::Align2::CENTER_CENTER,
                        &notes,
                        notes_font,
                        egui::Color32::from_rgb(120, 120, 120),
                    );
                }
            }
        }
    }
}


pub fn styled_sidebar(ui: &mut egui::Ui, dungeon: &mut Dungeon, state: &mut StyledViewState) {
    // Contextual: selected room info
    if let Some(ref sel_room_id) = state.selected_room.clone() {
        let room_label = dungeon.graph.room_by_id(sel_room_id)
            .map(|r| r.label.clone())
            .unwrap_or_else(|| "?".to_string());
        ui.heading(&room_label);
        ui.separator();

        if ui.small_button("Deselect").clicked() {
            state.selected_room = None;
        }

        if let Some(room) = dungeon.graph.room_by_id(sel_room_id) {
            let (w, h) = room.grid_size();
            ui.label(format!("{}x{} ({}x{} ft)", w, h, w * 5, h * 5));
            ui.label(format!("Shape: {}", room.shape.label()));

            if !room.tags.is_empty() {
                let tags_str: Vec<_> = room.tags.iter().map(|t| t.label()).collect();
                ui.label(format!("Tags: {}", tags_str.join(", ")));
            }

            if !room.notes.is_empty() {
                ui.add_space(4.0);
                ui.label("Notes:");
                ui.label(&room.notes);
            }
        }

        // Connections
        let connections: Vec<_> = dungeon.graph.connections.iter()
            .filter(|e| e.source_room_id == *sel_room_id || e.target_room_id == *sel_room_id)
            .map(|e| {
                let other_id = if e.source_room_id == *sel_room_id { &e.target_room_id } else { &e.source_room_id };
                let other_label = dungeon.graph.room_by_id(other_id)
                    .map(|r| r.label.as_str()).unwrap_or("?");
                (e.connection.connection_type.label(), other_label.to_string())
            })
            .collect();
        if !connections.is_empty() {
            ui.add_space(4.0);
            ui.label("Connections:");
            for (conn_type, other) in &connections {
                ui.label(format!("  {} \u{2192} {}", conn_type, other));
            }
        }

        // Encounters
        let room_encounters: Vec<_> = dungeon.encounters.iter()
            .filter(|e| e.home_room_id == *sel_room_id)
            .map(|e| e.name.clone())
            .collect();
        if !room_encounters.is_empty() {
            ui.add_space(4.0);
            ui.label("Encounters:");
            for name in &room_encounters {
                ui.label(format!("  {}", name));
            }
        }

        ui.add_space(12.0);
    }

    // Rendering options (always visible)
    egui::CollapsingHeader::new("Rendering")
        .default_open(state.selected_room.is_none())
        .show(ui, |ui| {
            ui.label("Theme:");
            ui.label(&dungeon.theme.name);

            ui.add_space(8.0);
            ui.checkbox(&mut state.show_grid, "Grid lines");
            ui.checkbox(&mut state.show_labels, "Room labels");
            ui.checkbox(&mut state.show_notes, "DM notes");
            ui.checkbox(&mut state.show_secrets, "Show secrets");
            ui.checkbox(&mut dungeon.theme.exterior_shading, "Exterior shading");
            if dungeon.theme.exterior_shading {
                ui.add(egui::Slider::new(&mut dungeon.theme.shading_radius, 0.2..=3.0).text("Radius"));
                egui::ComboBox::from_id_salt("shading_style")
                    .selected_text(dungeon.theme.shading_style.label())
                    .show_ui(ui, |ui| {
                        for s in ShadingStyle::ALL {
                            ui.selectable_value(&mut dungeon.theme.shading_style, s, s.label());
                        }
                    });
                if dungeon.theme.shading_style == ShadingStyle::Hatched
                    || dungeon.theme.shading_style == ShadingStyle::Stippled
                {
                    ui.add(egui::Slider::new(&mut dungeon.theme.hatching_density, 0.3..=3.0).text("Density"));
                }
            }

            ui.add_space(8.0);
            ui.label("Corridor corners:");
            egui::ComboBox::from_id_salt("chamfer_style")
                .selected_text(dungeon.theme.corridor_chamfer.label())
                .show_ui(ui, |ui| {
                    for s in ChamferStyle::ALL {
                        ui.selectable_value(&mut dungeon.theme.corridor_chamfer, s, s.label());
                    }
                });

            // Floor selector
            ui.add_space(8.0);
            {
                let floors = collect_floors(&dungeon.graph);
                let label = match state.current_floor {
                    None => "All Floors".to_string(),
                    Some(f) => format!("Floor {}", f),
                };
                egui::ComboBox::from_id_salt("styled_floor_select")
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
        });

    ui.add_space(16.0);
    ui.heading("Export");
    ui.separator();

    if ui.button("Export DM Map (PNG)").clicked() {
        state.export_requested = Some(true);
    }
    if ui.button("Export Player Map (PNG)").clicked() {
        state.export_requested = Some(false);
    }
}
