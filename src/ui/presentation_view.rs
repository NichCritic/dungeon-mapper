use crate::data::MonsterDatabase;
use crate::model::*;
use crate::model::combat_stats::CombatStatsCache;
use crate::presentation::{PresentationState, Visibility, LightSource};
use crate::presentation::combat_sim::{self, SimCombatant, run_combat, SimResult, build_combatants_from_encounter, build_combatants_from_party};
use crate::presentation::combat_tracker::{CombatTracker, CombatantId, MonsterInstanceId, STANDARD_CONDITIONS};
use crate::ui::encounters_view::SimSide;
use crate::presentation::dice;
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
    render_cache: Option<PresentationRenderCache>,
    /// Force a cache rebuild on next frame (e.g. after visibility change)
    dirty: bool,
    /// Canvas size from the last frame, used by the sidebar for centering.
    pub canvas_size: egui::Vec2,
    pub single_combat: SingleCombatState,
    /// Currently selected room in the presentation view.
    pub selected_room: Option<String>,
    /// True while the DM is dragging the player viewport rectangle.
    dragging_player_viewport: bool,
}

impl Default for PresentationViewState {
    fn default() -> Self {
        Self {
            view: ViewState::default(),
            render_cache: None,
            dirty: false,
            canvas_size: egui::Vec2::ZERO,
            single_combat: SingleCombatState::default(),
            selected_room: None,
            dragging_player_viewport: false,
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
    presentation.encounter_positions.len().hash(&mut h);
    for (eid, rid) in &presentation.encounter_positions {
        eid.hash(&mut h);
        rid.hash(&mut h);
    }
    presentation.party_room.hash(&mut h);
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
            show_decor: true,
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
    render_dm_overlay(&painter, &transform, layout, dungeon, presentation);

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
        // Subtle fill so it's visible over dark areas
        painter.rect_filled(
            vp_rect, 0.0,
            egui::Color32::from_rgba_unmultiplied(50, 255, 50, 10),
        );

        // Drag handling: start drag when left-click lands on the viewport rect border/interior
        let edge_margin = 8.0; // px - hit area for edges
        let inner = vp_rect.shrink(edge_margin);
        if response.drag_started_by(egui::PointerButton::Primary) {
            if let Some(pos) = response.interact_pointer_pos() {
                // Hit if on the border band (inside rect but outside inner) or if rect is small
                if vp_rect.contains(pos) && (!inner.contains(pos) || vp_rect.width() < edge_margin * 3.0 || vp_rect.height() < edge_margin * 3.0) {
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

    // Left-click: select room (only if not dragging viewport)
    if response.clicked() {
        if let Some(pos) = response.interact_pointer_pos() {
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
                ui.separator();
                if ui.button("Move Party Here").clicked() {
                    presentation.party_room = Some(room_id.clone());
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
                view_state.mark_dirty();
            }
            if ui.selectable_label(matches!(vis, Visibility::Explored), "Explored").clicked() {
                fog::explore_room(&sel_room_id, presentation);
                view_state.mark_dirty();
            }
            if ui.selectable_label(matches!(vis, Visibility::Visible), "Visible").clicked() {
                fog::reveal_room(&sel_room_id, presentation);
                view_state.mark_dirty();
            }
        });

        // Room position/size info
        if let Some(layout) = &dungeon.layout {
            if let Some(rl) = layout.room_by_id(&sel_room_id) {
                ui.add_space(4.0);
                ui.label(format!("Position: ({}, {})", rl.x, rl.y));
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
                        view_state.mark_dirty();
                    }
                });
            }
        }

        // Encounters in this room
        let room_encounters: Vec<_> = dungeon.encounters.iter()
            .filter(|e| e.home_room_id == sel_room_id)
            .map(|e| e.name.clone())
            .collect();
        if !room_encounters.is_empty() {
            ui.add_space(4.0);
            ui.label("Encounters:");
            for name in &room_encounters {
                ui.label(format!("  {}", name));
            }
        }

        // Quick actions
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            if ui.button("Reveal + Adjacent").clicked() {
                fog::reveal_room_and_adjacent(&sel_room_id, presentation, &dungeon.graph);
                view_state.mark_dirty();
            }
            if ui.button("Move Party Here").clicked() {
                presentation.party_room = Some(sel_room_id.clone());
                view_state.mark_dirty();
            }
        });

        // Center camera
        if ui.button("Center Camera").clicked() {
            if let Some(layout) = &dungeon.layout {
                if let Some(rl) = layout.room_by_id(&sel_room_id) {
                    let cx = (rl.x as f32 + rl.width as f32 / 2.0) * GRID_PX;
                    let cy = (rl.y as f32 + rl.height as f32 / 2.0) * GRID_PX;
                    view_state.view.center_on(cx, cy, view_state.canvas_size);
                }
            }
        }
    } else {
        // --- General room/door lists (no selection) ---
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
    }

    ui.add_space(8.0);

    // Party Management
    ui.heading("Party");
    ui.separator();

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
            ui.label("Party room:");
            egui::ComboBox::from_id_salt("party_room_combo")
                .selected_text(selected_label)
                .width(120.0)
                .show_ui(ui, |ui| {
                    if ui.selectable_label(presentation.party_room.is_none(), "(none)").clicked() {
                        presentation.party_room = None;
                        view_state.mark_dirty();
                    }
                    for (rid, rlabel) in &rooms_list {
                        let selected = presentation.party_room.as_ref() == Some(rid);
                        if ui.selectable_label(selected, rlabel).clicked() {
                            presentation.party_room = Some(rid.clone());
                            view_state.mark_dirty();
                        }
                    }
                });
        });
    }

    ui.add_space(4.0);

    // Only allow editing when not in combat
    let in_combat = presentation.combat_tracker.is_some();

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
                        // Read-only during combat
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

    ui.add_space(8.0);

    // Encounters & Combat Tracker
    if !dungeon.encounters.is_empty() {
        ui.heading("Encounters");
        ui.separator();

        ui.horizontal(|ui| {
            if ui.button("Tick").on_hover_text("Move wandering encounters").clicked() {
                presentation.tick_encounters(dungeon);
                view_state.mark_dirty();
            }
            if ui.button("Reset Pos").on_hover_text("Return all encounters to home rooms").clicked() {
                presentation.reset_encounter_positions(dungeon);
                view_state.mark_dirty();
            }
        });

        // Combat tracker controls
        ui.horizontal(|ui| {
            if presentation.combat_tracker.is_some() {
                if ui.button("End Combat").clicked() {
                    presentation.combat_tracker = None;
                }
            } else {
                if ui.button("Start Combat").clicked() {
                    presentation.combat_tracker = Some(CombatTracker::init_with_party(
                        &dungeon.encounters,
                        monster_db,
                        &dungeon.custom_monsters,
                        combat_stats_cache,
                        &dungeon.party,
                    ));
                }
            }
        });

        if let Some(tracker) = &mut presentation.combat_tracker {
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

            // Current turn indicator
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

            // Simulate combat button — runs a full sim and writes results back
            if ui.button("Simulate Combat").on_hover_text("Run combat to completion, updating all HP").clicked() {
                // Build SimCombatants from tracker's current monster instances (side 0)
                // and player instances (side 1) — monsters attack players
                let mut side_monsters: Vec<SimCombatant> = Vec::new();
                let mut monster_ids: Vec<MonsterInstanceId> = Vec::new();
                for (mid, inst) in &tracker.instances {
                    if inst.current_hp <= 0 { continue; }
                    side_monsters.push(SimCombatant {
                        name: inst.label.clone(),
                        max_hp: inst.max_hp,
                        current_hp: inst.current_hp,
                        ac: 10, // monsters don't store AC on instance; use 10 as fallback
                        initiative_mod: inst.dex_mod,
                        attacks: inst.attacks.clone(),
                        multiattack_count: if inst.attacks.is_empty() { 1 } else {
                            // estimate from count of attacks, default 1
                            1
                        },
                        side: 0,
                    });
                    monster_ids.push(mid.clone());
                }

                let mut side_players: Vec<SimCombatant> = Vec::new();
                let mut player_ids: Vec<String> = Vec::new();
                for (pid, pc) in &tracker.players {
                    if pc.current_hp <= 0 { continue; }
                    // Find the PC's attack stats from dungeon.party
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
                    };
                    side_players.push(SimCombatant {
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

                    // Write monster results back to tracker
                    for (i, mid) in monster_ids.iter().enumerate() {
                        if let Some(inst) = tracker.instances.get_mut(mid) {
                            if let Some(final_state) = result.combatants.get(i) {
                                inst.current_hp = final_state.current_hp.max(0);
                                inst.is_dead = inst.current_hp <= 0;
                            }
                        }
                    }

                    // Write player results back to tracker
                    for (i, pid) in player_ids.iter().enumerate() {
                        if let Some(pc) = tracker.players.get_mut(pid) {
                            let combatant_idx = side_monsters.len() + i;
                            if let Some(final_state) = result.combatants.get(combatant_idx) {
                                pc.current_hp = final_state.current_hp.max(0);
                            }
                        }
                    }

                    // Log the result
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
            let encounter_ids: Vec<_> = dungeon.encounters.iter()
                .map(|e| (e.id.clone(), e.name.clone()))
                .collect();

            // Pre-compute current turn ID to avoid borrow conflicts
            let current_turn_id = tracker.current_combatant_id().cloned();

            // Collect deferred actions using CombatantId
            let mut damage_actions: Vec<(CombatantId, i32)> = Vec::new();
            let mut heal_actions: Vec<(CombatantId, i32)> = Vec::new();
            let mut condition_toggles: Vec<(CombatantId, usize)> = Vec::new();
            let mut attack_actions: Vec<(String, String, crate::model::combat_stats::ParsedAttack, u8)> = Vec::new(); // (attacker_name, target_desc, attack, target_ac)

            egui::ScrollArea::vertical().max_height(400.0).id_salt("combat_scroll").show(ui, |ui| {
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
                                            if let Some(init) = &mut pc.initiative {
                                                ui.label("Init:"); crate::ui::canvas_common::num_input_i32(ui, init, 35.0);
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
                                        // Name + initiative
                                        ui.horizontal(|ui| {
                                            if inst.is_dead {
                                                ui.colored_label(egui::Color32::from_rgb(150, 150, 150), &inst.label);
                                            } else {
                                                ui.label(&inst.label);
                                            }
                                            if let Some(init) = &mut inst.initiative {
                                                ui.label("Init:"); crate::ui::canvas_common::num_input_i32(ui, init, 35.0);
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

                                        // HP progress bar
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
                                        });

                                        // Attacks section
                                        if !inst.attacks.is_empty() {
                                            let attacks_snapshot: Vec<_> = inst.attacks.clone();
                                            let attacker_name = inst.label.clone();
                                            egui::CollapsingHeader::new("Attacks")
                                                .id_salt(format!("attacks_{}_{}_{}", inst_id.encounter_id, inst_id.monster_index, inst_id.instance))
                                                .default_open(false)
                                                .show(ui, |ui| {
                                                    for atk in &attacks_snapshot {
                                                        let ac_id = egui::Id::new(format!("ac_{}_{}_{}_{}", inst_id.encounter_id, inst_id.monster_index, inst_id.instance, atk.name));
                                                        let mut target_ac: u8 = ui.ctx().memory(|m| m.data.get_temp(ac_id).unwrap_or(10u8));
                                                        ui.horizontal(|ui| {
                                                            let btn_text = format!("{} (+{})", atk.name, atk.to_hit);
                                                            if ui.button(&btn_text).clicked() {
                                                                attack_actions.push((attacker_name.clone(), format!("AC {}", target_ac), atk.clone(), target_ac));
                                                            }
                                                            ui.label("vs AC");
                                                            let mut tac = target_ac as i32;
                                                            if crate::ui::canvas_common::num_input_i32(ui, &mut tac, 35.0) { target_ac = tac as u8; }
                                                        });
                                                        ui.ctx().memory_mut(|m| m.data.insert_temp(ac_id, target_ac));
                                                    }
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
            // Process attack rolls
            for (attacker_name, target_desc, attack, target_ac) in attack_actions {
                let result = dice::roll_attack(&attack, target_ac);
                tracker.log.log_attack(&attacker_name, &target_desc, &attack.name, &result);
            }
        } else {
            // No combat active — just show encounter locations
            egui::ScrollArea::vertical().max_height(150.0).id_salt("enc_pres_scroll").show(ui, |ui| {
                for enc in &dungeon.encounters {
                    let current_room_id = presentation.encounter_room(enc);
                    let current_label = dungeon.graph.room_by_id(current_room_id)
                        .map(|r| r.label.as_str())
                        .unwrap_or("?");
                    let type_marker = match enc.encounter_type {
                        EncounterType::Static => "S",
                        EncounterType::Wandering(_) => "W",
                    };
                    ui.label(format!("[{}] {} - {}", type_marker, enc.name, current_label));
                }
            });
        }
    }

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
    ui.checkbox(&mut presentation.show_labels_player, "Show labels to players");

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

    // --- Single Combat Simulator ---
    ui.add_space(12.0);
    ui.heading("Combat Simulator");
    ui.separator();

    let sim = &mut view_state.single_combat;
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
        let side_a_combatants = match &sim.side_a {
            SimSide::Party => build_combatants_from_party(&dungeon.party, 0),
            SimSide::Encounter(idx) => {
                if let Some(enc) = dungeon.encounters.get(*idx) {
                    build_combatants_from_encounter(enc, monster_db, &dungeon.custom_monsters, combat_stats_cache, 0)
                } else { Vec::new() }
            }
        };
        let side_b_combatants = match &sim.side_b {
            SimSide::Party => build_combatants_from_party(&dungeon.party, 1),
            SimSide::Encounter(idx) => {
                if let Some(enc) = dungeon.encounters.get(*idx) {
                    build_combatants_from_encounter(enc, monster_db, &dungeon.custom_monsters, combat_stats_cache, 1)
                } else { Vec::new() }
            }
        };
        if !side_a_combatants.is_empty() && !side_b_combatants.is_empty() {
            let result = run_combat(&side_a_combatants, &side_b_combatants);

            // Persist results to combat tracker if active
            if let Some(tracker) = &mut presentation.combat_tracker {
                for final_c in &result.combatants {
                    // Try to match to tracker monster instances by name
                    for (_, inst) in tracker.instances.iter_mut() {
                        if inst.label == final_c.name {
                            inst.current_hp = final_c.current_hp.max(0);
                            inst.is_dead = inst.current_hp <= 0;
                            break;
                        }
                    }
                    // Try to match to tracker players by name
                    for (_, pc) in tracker.players.iter_mut() {
                        if pc.name == final_c.name {
                            pc.current_hp = final_c.current_hp.max(0);
                            break;
                        }
                    }
                }
            }

            sim.last_result = Some(result);
        }
    }

    if let Some(result) = &sim.last_result {
        ui.add_space(4.0);
        ui.group(|ui| {
            let winner_text = match result.winner {
                Some(0) => "Side A wins",
                Some(1) => "Side B wins",
                _ => "Draw (timeout)",
            };
            ui.label(format!("{} in {} rounds", winner_text, result.rounds));

            for side in 0..=1 {
                let side_combatants: Vec<_> = result.combatants.iter()
                    .filter(|c| c.side == side)
                    .collect();
                if side_combatants.is_empty() { continue; }
                ui.label(if side == 0 { "Side A:" } else { "Side B:" });
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

    ui.add_space(8.0);

    // Web server controls
    ui.heading("Web Server");
    ui.separator();
}

/// Actions the sidebar can request from the app regarding the server.
pub enum ServerAction {
    None,
}
