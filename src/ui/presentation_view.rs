use crate::data::MonsterDatabase;
use crate::model::*;
use crate::model::combat_stats::CombatStatsCache;
use crate::presentation::{PresentationState, Visibility};
use crate::presentation::combat_sim::{self, SimCombatant, run_combat, SimResult, build_combatants_from_encounter, build_combatants_from_party};
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
        }
        if ui.button("Hide All").clicked() {
            for room in &dungeon.graph.rooms {
                fog::hide_room(&room.id, presentation);
            }
            for edge in &dungeon.graph.connections {
                fog::close_door(&edge.connection.id, presentation);
            }
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
        let room_encounters: Vec<&crate::model::Encounter> = dungeon.encounters.iter()
            .filter(|e| presentation.encounter_room(e) == sel_room_id)
            .collect();
        if !room_encounters.is_empty() {
            ui.add_space(8.0);
            ui.heading("Encounters");
            ui.separator();
            for enc in &room_encounters {
                let type_marker = match enc.encounter_type {
                    EncounterType::Static => "S",
                    EncounterType::Wandering(_) => "W",
                };
                ui.label(format!("[{}] {}", type_marker, enc.name));
            }
            if !in_combat {
                let room_encounter_slice: Vec<crate::model::Encounter> = room_encounters.iter()
                    .map(|e| (*e).clone())
                    .collect();
                if ui.button(format!("Start Combat in {}", room_label)).clicked() {
                    presentation.combat_tracker = Some(CombatTracker::init_with_party(
                        &room_encounter_slice,
                        monster_db,
                        &dungeon.custom_monsters,
                        combat_stats_cache,
                        &dungeon.party,
                    ));
                }
            }
        }

    } else {
        // --- General room/door lists (no selection) ---
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

        ui.add_space(8.0);

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
        
                    }
                });
            }
        });

        ui.add_space(8.0);

        // Party Management (full view when no room selected)
        ui.heading("Party");
        ui.separator();
        party_section(ui, dungeon, presentation, in_combat);

        ui.add_space(8.0);

        // Encounters grouped by room
        if !dungeon.encounters.is_empty() {
            ui.heading("Encounters");
            ui.separator();

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

            if !in_combat {
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
                            if ui.button(format!("Start Combat in {}", room_label)).clicked() {
                                presentation.combat_tracker = Some(CombatTracker::init_with_party(
                                    &room_encounter_slice,
                                    monster_db,
                                    &dungeon.custom_monsters,
                                    combat_stats_cache,
                                    &dungeon.party,
                                ));
                            }
                        });
                }
            }
        }
    }

    // Combat tracker — always visible when active (regardless of room selection)
    if presentation.combat_tracker.is_some() {
        ui.add_space(8.0);
        ui.heading("Combat");
        ui.separator();
        if ui.button("End Combat").clicked() {
            presentation.combat_tracker = None;
        }
    }

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

            // Collect deferred actions — declared early so the turn card can use them
            let mut damage_actions: Vec<(CombatantId, i32)> = Vec::new();
            let mut heal_actions: Vec<(CombatantId, i32)> = Vec::new();
            let mut condition_toggles: Vec<(CombatantId, usize)> = Vec::new();
            let mut attack_actions: Vec<(String, String, crate::model::combat_stats::ParsedAttack, u8)> = Vec::new();

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

                // Duplicate of the active combatant's controls at the top for quick access
                ui.group(|ui| {
                    match &current_id {
                        CombatantId::Monster(mid) => {
                            if let Some(inst) = tracker.instances.get(&mid) {
                                // HP
                                let hp_frac = if inst.max_hp > 0 { inst.current_hp as f32 / inst.max_hp as f32 } else { 0.0 };
                                let bar_color = if hp_frac > 0.5 {
                                    egui::Color32::from_rgb(80, 200, 80)
                                } else if hp_frac > 0.25 {
                                    egui::Color32::from_rgb(220, 200, 50)
                                } else {
                                    egui::Color32::from_rgb(220, 60, 60)
                                };
                                ui.label(format!("{}/{} HP", inst.current_hp, inst.max_hp));
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

                                // Attacks (open by default here)
                                if !inst.attacks.is_empty() {
                                    let attacks_snapshot: Vec<_> = inst.attacks.clone();
                                    let attacker_name = inst.label.clone();
                                    egui::CollapsingHeader::new("Actions")
                                        .id_salt("active_turn_attacks")
                                        .default_open(true)
                                        .show(ui, |ui| {
                                            for atk in &attacks_snapshot {
                                                let ac_id = egui::Id::new(format!("turn_ac_{}", atk.name));
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
                        effect: String::new(),
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

            // Per-encounter collapsible sections (only encounters with instances in this combat)
            let active_enc_ids: std::collections::HashSet<&str> = tracker.instances.keys()
                .map(|mid| mid.encounter_id.as_str())
                .collect();
            let encounter_ids: Vec<_> = dungeon.encounters.iter()
                .filter(|e| active_enc_ids.contains(e.id.as_str()))
                .map(|e| (e.id.clone(), e.name.clone()))
                .collect();

            // Pre-compute current turn ID to avoid borrow conflicts
            let current_turn_id = tracker.current_combatant_id().cloned();

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
                tracker.log.log_attack(&attacker_name, &target_desc, &attack.name, &result, Some(&attack));
            }
        }

    ui.add_space(8.0);

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
    ui.heading("Combat Simulator");
    ui.separator();

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

    ui.add_space(8.0);

    // Web server controls
    ui.heading("Web Server");
    ui.separator();
}

/// Actions the sidebar can request from the app regarding the server.
pub enum ServerAction {
    None,
}
