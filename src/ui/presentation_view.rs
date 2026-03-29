use crate::model::*;
use crate::presentation::{PresentationState, Visibility, LightSource};
use crate::presentation::fog;
use crate::render::presentation::render_dm_overlay;
use crate::render::recording::{RecordingRenderer, RenderCommand, replay_commands};
use crate::render::themed::RenderOptions;
use crate::ui::canvas_common::{handle_pan_zoom, ViewState, COLOR_PLACEHOLDER_TEXT};
use crate::util::{ViewTransform, GRID_PX};

struct PresentationRenderCache {
    commands: Vec<RenderCommand>,
    input_hash: u64,
}

pub struct PresentationViewState {
    pub view: ViewState,
    render_cache: Option<PresentationRenderCache>,
    /// Force a cache rebuild on next frame (e.g. after visibility change)
    dirty: bool,
    /// Canvas size from the last frame, used by the sidebar for centering.
    pub canvas_size: egui::Vec2,
}

impl Default for PresentationViewState {
    fn default() -> Self {
        Self {
            view: ViewState::default(),
            render_cache: None,
            dirty: false,
            canvas_size: egui::Vec2::ZERO,
        }
    }
}

impl PresentationViewState {
    pub fn mark_dirty(&mut self) {
        self.dirty = true;
    }
}

fn presentation_input_hash(
    layout: &SpatialLayout,
    theme: &Theme,
    presentation: &PresentationState,
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
    // Hash room visibility
    for (room_id, vis) in &presentation.room_visibility {
        room_id.hash(&mut h);
        std::mem::discriminant(vis).hash(&mut h);
    }
    // Hash door states
    presentation.doors_open.len().hash(&mut h);
    for conn_id in &presentation.doors_open {
        conn_id.hash(&mut h);
    }
    presentation.light_sources.len().hash(&mut h);
    for light in &presentation.light_sources {
        light.id.hash(&mut h);
        light.radius.to_bits().hash(&mut h);
        light.intensity.to_bits().hash(&mut h);
    }
    presentation.ambient_light.to_bits().hash(&mut h);
    h.finish()
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
    dungeon: &Dungeon,
    presentation: &mut PresentationState,
    view_state: &mut PresentationViewState,
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
    let hash = presentation_input_hash(layout, &dungeon.theme, presentation);
    let needs_rebuild = view_state.dirty
        || view_state.render_cache.as_ref().is_none_or(|c| c.input_hash != hash);

    if needs_rebuild {
        let mut recorder = RecordingRenderer::new();
        let options = RenderOptions {
            show_grid: true,
            show_labels: true,
            show_notes: true,
            show_secrets: true,
        };
        crate::render::themed::render_themed(
            &mut recorder,
            &dungeon.graph,
            layout,
            &dungeon.theme,
            &options,
        );
        view_state.render_cache = Some(PresentationRenderCache {
            commands: recorder.commands,
            input_hash: hash,
        });
        view_state.dirty = false;
    }

    // Replay the full map
    if let Some(cache) = &view_state.render_cache {
        replay_commands(&painter, &transform, &cache.commands);
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
    render_dm_overlay(&painter, &transform, layout, &dungeon.graph, presentation);

    // Left-click: room → cycle visibility, corridor → toggle door
    if response.clicked() {
        if let Some(pos) = response.interact_pointer_pos() {
            let world = transform.screen_to_world(pos);
            let gx = (world.x / GRID_PX).floor() as i32;
            let gy = (world.y / GRID_PX).floor() as i32;

            // Prefer corridor click (toggle door)
            if let Some(conn_id) = corridor_at_grid(layout, gx, gy) {
                fog::toggle_door(&conn_id, presentation);
                view_state.mark_dirty();
            } else if let Some(room_id) = room_at_grid(layout, gx, gy) {
                fog::cycle_room_visibility(&room_id, presentation);
                view_state.mark_dirty();
            }
        }
    }

    // Right-click context menu
    response.context_menu(|ui| {
        if let Some(pos) = ui.ctx().pointer_latest_pos() {
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
                    view_state.mark_dirty();
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
                    view_state.mark_dirty();
                    ui.close_menu();
                }
                if ui.button("Explore").clicked() {
                    fog::explore_room(&room_id, presentation);
                    view_state.mark_dirty();
                    ui.close_menu();
                }
                if ui.button("Hide").clicked() {
                    fog::hide_room(&room_id, presentation);
                    view_state.mark_dirty();
                    ui.close_menu();
                }
                ui.separator();
                if ui.button("Open All Doors").clicked() {
                    fog::open_room_doors(&room_id, presentation, &dungeon.graph);
                    view_state.mark_dirty();
                    ui.close_menu();
                }
                if ui.button("Close All Doors").clicked() {
                    fog::close_room_doors(&room_id, presentation, &dungeon.graph);
                    view_state.mark_dirty();
                    ui.close_menu();
                }
                ui.separator();
                if ui.button("Reveal + Adjacent").clicked() {
                    fog::reveal_room_and_adjacent(&room_id, presentation, &dungeon.graph);
                    view_state.mark_dirty();
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
pub fn presentation_sidebar(
    ui: &mut egui::Ui,
    dungeon: &mut Dungeon,
    presentation: &mut PresentationState,
    view_state: &mut PresentationViewState,
    player_view_state: &mut crate::ui::player_view::PlayerViewState,
    player_viewport_open: &mut bool,
    _server_action: &mut ServerAction,
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
            view_state.mark_dirty();
        }
        if ui.button("Hide All").clicked() {
            for room in &dungeon.graph.rooms {
                fog::hide_room(&room.id, presentation);
            }
            for edge in &dungeon.graph.connections {
                fog::close_door(&edge.connection.id, presentation);
            }
            view_state.mark_dirty();
        }
    });

    // Zoom: 1 inch per grid square on a 40" screen
    if ui.button("Zoom: 1\"/square (40\" screen)").clicked() {
        let target_zoom = zoom_for_one_inch_square(ui.ctx(), 40.0);
        // Preserve the current center point while changing zoom
        let canvas_center = view_state.canvas_size / 2.0;
        let world_center_x = (canvas_center.x - view_state.view.offset.x) / view_state.view.zoom;
        let world_center_y = (canvas_center.y - view_state.view.offset.y) / view_state.view.zoom;
        view_state.view.zoom = target_zoom;
        view_state.view.center_on(world_center_x, world_center_y, view_state.canvas_size);
    }

    ui.add_space(8.0);

    // Room list with visibility + center button
    ui.heading("Rooms");
    ui.separator();
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
                    view_state.mark_dirty();
                }
                // Center camera on this room
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

    ui.add_space(8.0);

    // Door list with open/closed state
    ui.heading("Doors");
    ui.separator();
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
                    view_state.mark_dirty();
                }
            });
        }
    });

    ui.add_space(8.0);

    // Lighting
    ui.heading("Lighting");
    ui.separator();

    ui.add(egui::Slider::new(&mut presentation.ambient_light, 0.0..=1.0).text("Ambient"));
    if presentation.ambient_light != 0.0 || !presentation.light_sources.is_empty() {
        view_state.mark_dirty();
    }

    // Add light source
    if ui.button("Add Light Source").clicked() {
        let room_id = dungeon.graph.rooms.first()
            .map(|r| r.id.clone())
            .unwrap_or_default();
        if !room_id.is_empty() {
            presentation.light_sources.push(LightSource {
                id: uuid::Uuid::new_v4().to_string(),
                room_id,
                radius: 5.0,
                intensity: 1.0,
                color: [255, 200, 100],
            });
            view_state.mark_dirty();
        }
    }

    // List light sources
    let mut remove_idx = None;
    for (i, light) in presentation.light_sources.iter_mut().enumerate() {
        ui.horizontal(|ui| {
            let room_label = dungeon.graph.room_by_id(&light.room_id)
                .map(|r| r.label.as_str())
                .unwrap_or("?");
            ui.label(format!("Light in {}", room_label));
            if ui.small_button("X").clicked() {
                remove_idx = Some(i);
            }
        });
        ui.horizontal(|ui| {
            ui.add(egui::Slider::new(&mut light.radius, 1.0..=20.0).text("Radius"));
        });
        ui.horizontal(|ui| {
            ui.add(egui::Slider::new(&mut light.intensity, 0.0..=1.0).text("Intensity"));
        });

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
        presentation.light_sources.remove(idx);
        view_state.mark_dirty();
    }

    ui.add_space(16.0);

    // Player window
    ui.heading("Player View");
    ui.separator();

    if ui.button(if *player_viewport_open { "Close Player Window" } else { "Open Player Window" }).clicked() {
        *player_viewport_open = !*player_viewport_open;
    }

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

    // Center player view on room
    if let Some(layout) = &dungeon.layout {
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

    ui.add_space(8.0);

    // Web server controls
    ui.heading("Web Server");
    ui.separator();
}

/// Actions the sidebar can request from the app regarding the server.
pub enum ServerAction {
    None,
}
