use crate::data::MonsterDatabase;
use crate::model::*;
use crate::model::combat_stats::CombatStatsCache;
use crate::model::monster::{
    size_label, alignment_display, damage_list_display,
    MonsterRef, CustomMonster, EncounterMonster, Monster, Feature,
    MergeStrategy, MergeConfig, merge_monsters,
    MERGE_NUMERIC_FIELDS, MERGE_LIST_FIELDS, MERGE_STRING_FIELDS,
    MonsterType, HitPoints, ChallengeRating, SpeedValue, ArmorClass,
};
use crate::presentation::combat_sim::{
    self, build_combatants_from_encounter, build_combatants_from_party,
    run_monte_carlo, MonteCarloResult,
};
use crate::render::recording::{RecordingRenderer, RenderCommand, replay_commands};
use crate::render::themed::RenderOptions;
use crate::ui::canvas_common::{handle_pan_zoom, ViewState, COLOR_PLACEHOLDER_TEXT};
use crate::util::{ViewTransform, GRID_PX};

use std::collections::HashMap;
use std::hash::{Hash, Hasher};

struct RenderCache {
    commands: Vec<RenderCommand>,
    input_hash: u64,
}

/// Which side selector in the combat simulator.
#[derive(Clone, Debug, PartialEq)]
pub enum SimSide {
    Party,
    Encounter(usize), // index into dungeon.encounters
}

impl Default for SimSide {
    fn default() -> Self { SimSide::Party }
}

pub struct SimulationState {
    pub side_a: SimSide,
    pub side_b: SimSide,
    pub monte_carlo_n: u32,
    pub last_monte_carlo: Option<MonteCarloResult>,
}

impl Default for SimulationState {
    fn default() -> Self {
        Self {
            side_a: SimSide::Party,
            side_b: SimSide::default(),
            monte_carlo_n: 100,
            last_monte_carlo: None,
        }
    }
}

pub struct EncountersViewState {
    pub view: ViewState,
    render_cache: Option<RenderCache>,
    pub sim_state: SimulationState,
    /// Room selected on the map canvas (for contextual sidebar).
    pub selected_room: Option<String>,
}

impl Default for EncountersViewState {
    fn default() -> Self {
        Self {
            view: ViewState::default(),
            render_cache: None,
            sim_state: SimulationState::default(),
            selected_room: None,
        }
    }
}

fn render_input_hash(layout: &SpatialLayout, graph: &DungeonGraph, theme: &Theme) -> u64 {
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
            room.decor.len().hash(&mut h);
            for d in &room.decor {
                d.x.to_bits().hash(&mut h);
                d.y.to_bits().hash(&mut h);
                d.rotation.to_bits().hash(&mut h);
                d.scale.to_bits().hash(&mut h);
                std::mem::discriminant(&d.decor_type).hash(&mut h);
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
    h.finish()
}

fn truncate_name(name: &str, max_len: usize) -> String {
    if name.chars().count() <= max_len {
        name.to_string()
    } else {
        let truncated: String = name.chars().take(max_len).collect();
        format!("{}...", truncated)
    }
}

pub fn encounters_view(ui: &mut egui::Ui, dungeon: &Dungeon, state: &mut EncountersViewState) {
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

    // Rebuild cached render commands if inputs changed
    let hash = render_input_hash(layout, &dungeon.graph, &dungeon.theme);
    let needs_rebuild = state.render_cache.as_ref().is_none_or(|c| c.input_hash != hash);

    if needs_rebuild {
        let mut recorder = RecordingRenderer::new();
        let options = RenderOptions {
            show_grid: true,
            show_labels: true,
            show_notes: false,
            show_secrets: false,
            show_decor: true,
        };
        crate::render::themed::render_themed(
            &mut recorder,
            &dungeon.graph,
            layout,
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

    // Draw encounter markers at their home rooms
    for enc in &dungeon.encounters {
        let Some(rl) = layout.room_by_id(&enc.home_room_id) else { continue };

        let cx = (rl.x as f32 + rl.width as f32 / 2.0) * GRID_PX;
        let cy = (rl.y as f32 + rl.height as f32 / 2.0) * GRID_PX;
        let screen = transform.world_to_screen(egui::pos2(cx, cy));

        let siblings: Vec<_> = dungeon.encounters.iter()
            .filter(|e| e.home_room_id == enc.home_room_id)
            .collect();
        let idx = siblings.iter().position(|e| e.id == enc.id).unwrap_or(0);
        let offset_y = (idx as f32 - (siblings.len() as f32 - 1.0) / 2.0) * 14.0 * transform.zoom;
        let pos = screen + egui::vec2(0.0, 16.0 * transform.zoom + offset_y);

        let (marker, color) = match enc.encounter_type {
            EncounterType::Static => ("S", egui::Color32::from_rgb(255, 80, 80)),
            EncounterType::Wandering(r) => {
                if let Some(range) = r {
                    let radius_px = range as f32 * GRID_PX * 2.0 * transform.zoom;
                    painter.circle_stroke(
                        screen,
                        radius_px,
                        egui::Stroke::new(1.0, egui::Color32::from_rgba_unmultiplied(255, 160, 40, 60)),
                    );
                }
                ("W", egui::Color32::from_rgb(255, 160, 40))
            }
        };

        let text_size = 9.0 * transform.zoom;
        let display = format!("{} {}", marker, truncate_name(&enc.name, 10));

        let galley = painter.layout_no_wrap(
            display.clone(),
            egui::FontId::monospace(text_size),
            color,
        );
        let pill_size = galley.size() + egui::vec2(6.0, 2.0);
        let pill_rect = egui::Rect::from_center_size(pos, pill_size);
        painter.rect_filled(pill_rect, 3.0, egui::Color32::from_rgba_unmultiplied(0, 0, 0, 200));

        painter.text(
            pos,
            egui::Align2::CENTER_CENTER,
            &display,
            egui::FontId::monospace(text_size),
            color,
        );
    }

    // Click to select/deselect a room
    if response.clicked() {
        if let Some(pos) = response.interact_pointer_pos() {
            let world = transform.screen_to_world(pos);
            let gx = (world.x / GRID_PX).floor() as i32;
            let gy = (world.y / GRID_PX).floor() as i32;
            let mut hit = None;
            for rl in &layout.rooms {
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
        if let Some(rl) = layout.room_by_id(sel_id) {
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
}

pub fn encounters_sidebar(
    ui: &mut egui::Ui,
    dungeon: &mut Dungeon,
    monster_db: &MonsterDatabase,
    combat_stats_cache: &mut CombatStatsCache,
    state: &mut EncountersViewState,
) {
    let sim_state = &mut state.sim_state;

    if monster_db.is_empty() {
        ui.colored_label(
            egui::Color32::from_rgb(255, 200, 100),
            "No monster database loaded",
        );
        ui.add_space(4.0);
    }

    // Contextual header: selected room
    if let Some(ref sel_room_id) = state.selected_room.clone() {
        let room_label = dungeon.graph.room_by_id(sel_room_id)
            .map(|r| r.label.clone())
            .unwrap_or_else(|| "?".to_string());
        ui.heading(&room_label);
        ui.separator();

        if ui.small_button("Deselect").clicked() {
            state.selected_room = None;
        }

        // Encounters in this room
        let room_enc_indices: Vec<usize> = dungeon.encounters.iter().enumerate()
            .filter(|(_, e)| e.home_room_id == *sel_room_id)
            .map(|(i, _)| i)
            .collect();

        if room_enc_indices.is_empty() {
            ui.add_space(4.0);
            ui.label("No encounters in this room.");
        }

        ui.add_space(4.0);
        if ui.button("Add Encounter Here").clicked() {
            dungeon.encounters.push(Encounter::new("New Encounter".to_string(), sel_room_id.clone()));
        }

        if !room_enc_indices.is_empty() {
            ui.add_space(8.0);
            encounters_list(ui, dungeon, monster_db, &room_enc_indices);
        }

        // Other rooms' encounters (collapsed)
        let other_indices: Vec<usize> = (0..dungeon.encounters.len())
            .filter(|i| !room_enc_indices.contains(i))
            .collect();
        if !other_indices.is_empty() {
            ui.add_space(12.0);
            egui::CollapsingHeader::new(format!("Other Encounters ({})", other_indices.len()))
                .default_open(false)
                .show(ui, |ui| {
                    encounters_list(ui, dungeon, monster_db, &other_indices);
                });
        }
    } else {
        ui.heading("Encounters");
        ui.separator();

        ui.add_space(4.0);

        // Add encounter
        ui.horizontal(|ui| {
            if ui.button("Add Encounter").clicked() {
                let room_id = dungeon.graph.rooms.first()
                    .map(|r| r.id.clone())
                    .unwrap_or_default();
                if !room_id.is_empty() {
                    dungeon.encounters.push(Encounter::new("New Encounter".to_string(), room_id));
                }
            }
            if ui.button("Monster Workshop").clicked() {
                ui.ctx().memory_mut(|mem| {
                    mem.data.insert_temp(egui::Id::new("monster_workshop_open"), true);
                });
            }
        });

        ui.add_space(8.0);

        let all_indices: Vec<usize> = (0..dungeon.encounters.len()).collect();
        encounters_list(ui, dungeon, monster_db, &all_indices);
    }

    // Monster browser window
    monster_browser_window(ui.ctx(), dungeon, monster_db);
    // Custom monster editor window
    custom_monster_editor_window(ui.ctx(), dungeon);
    // Monster workshop window (merge + custom editing)
    monster_workshop_window(ui.ctx(), dungeon, monster_db);

    // Monte Carlo window
    monte_carlo_window(ui.ctx(), dungeon, monster_db, combat_stats_cache, sim_state);

    ui.add_space(12.0);
    if ui.button("Monte Carlo Simulator").clicked() {
        ui.ctx().memory_mut(|mem| {
            mem.data.insert_temp(egui::Id::new("monte_carlo_open"), true);
        });
    }
}

/// Render a list of encounters by index. Handles all deferred mutations.
fn encounters_list(
    ui: &mut egui::Ui,
    dungeon: &mut Dungeon,
    monster_db: &MonsterDatabase,
    indices: &[usize],
) {

    let rooms_list: Vec<_> = dungeon.graph.rooms.iter()
        .map(|r| (r.id.clone(), r.label.clone()))
        .collect();
    let index_set: std::collections::HashSet<usize> = indices.iter().copied().collect();

    // Encounter list
    let mut remove_enc_idx = None;
    let mut add_monster_to: Option<usize> = None;
    let mut remove_monster: Option<(usize, usize)> = None;
    let mut customize_monster: Option<(usize, usize)> = None;
    let mut edit_custom_id: Option<String> = None;

    egui::ScrollArea::vertical().id_salt("enc_scroll").show(ui, |ui| {
        for (enc_idx, enc) in dungeon.encounters.iter_mut().enumerate() {
            if !index_set.contains(&enc_idx) { continue; }
            let enc_id = enc.id.clone();
            ui.push_id(&enc_id, |ui| {
                ui.group(|ui| {
                    // Header: name + delete
                    ui.horizontal(|ui| {
                        ui.text_edit_singleline(&mut enc.name);
                        if ui.small_button("X").clicked() {
                            remove_enc_idx = Some(enc_idx);
                        }
                    });

                    // Type selector
                    ui.horizontal(|ui| {
                        let is_static = enc.encounter_type == EncounterType::Static;
                        if ui.selectable_label(is_static, "Static").clicked() {
                            enc.encounter_type = EncounterType::Static;
                        }
                        let mut wander_range = match enc.encounter_type {
                            EncounterType::Wandering(r) => r,
                            _ => Some(2),
                        };
                        let is_wandering = matches!(enc.encounter_type, EncounterType::Wandering(_));
                        if ui.selectable_label(is_wandering, "Wandering").clicked() {
                            enc.encounter_type = EncounterType::Wandering(wander_range);
                        }
                        if is_wandering {
                            let is_unlimited = wander_range.is_none();
                            if is_unlimited {
                                ui.label("-");
                            } else {
                                let mut r = wander_range.unwrap_or(2);
                                if ui.add(egui::Slider::new(&mut r, 1..=20).prefix("range: ")).changed() {
                                    wander_range = Some(r);
                                    enc.encounter_type = EncounterType::Wandering(wander_range);
                                }
                            }
                            let mut unlimited = is_unlimited;
                            if ui.checkbox(&mut unlimited, "unlimited").changed() {
                                wander_range = if unlimited { None } else { Some(2) };
                                enc.encounter_type = EncounterType::Wandering(wander_range);
                            }
                        }
                    });

                    // Home room
                    egui::ComboBox::from_id_salt(format!("enc_home_{}", enc_id))
                        .selected_text(
                            dungeon.graph.room_by_id(&enc.home_room_id)
                                .map(|r| r.label.as_str())
                                .unwrap_or("Select room"),
                        )
                        .width(150.0)
                        .show_ui(ui, |ui| {
                            for (rid, rlabel) in &rooms_list {
                                ui.selectable_value(&mut enc.home_room_id, rid.clone(), rlabel);
                            }
                        });

                    // Monster list for this encounter
                    if !enc.monsters.is_empty() {
                        ui.add_space(4.0);
                        ui.label("Monsters:");
                        for (m_idx, em) in enc.monsters.iter_mut().enumerate() {
                            ui.horizontal(|ui| {
                                // Count
                                ui.label("x");
                                crate::ui::canvas_common::num_input_u32(ui, &mut em.count, 30.0);

                                // Monster name
                                let name = resolve_monster_name(&em.monster_ref, monster_db, &dungeon.custom_monsters);
                                ui.label(&name);

                                // Edit button for custom/merged monsters
                                match &em.monster_ref {
                                    MonsterRef::Custom { id } | MonsterRef::Merged { id } => {
                                        let icon = if matches!(em.monster_ref, MonsterRef::Merged { .. }) { "M" } else { "E" };
                                        if ui.small_button(icon).on_hover_text("Edit custom monster").clicked() {
                                            edit_custom_id = Some(id.clone());
                                        }
                                    }
                                    _ => {}
                                }

                                // Customize button (only for base monsters)
                                if matches!(em.monster_ref, MonsterRef::Base { .. }) {
                                    if ui.small_button("C").on_hover_text("Customize (create editable copy)").clicked() {
                                        customize_monster = Some((enc_idx, m_idx));
                                    }
                                }

                                if ui.small_button("-").clicked() {
                                    remove_monster = Some((enc_idx, m_idx));
                                }
                            });
                        }

                        // XP total
                        let total_xp: u32 = enc.monsters.iter().map(|em| {
                            resolve_monster_xp(&em.monster_ref, monster_db, &dungeon.custom_monsters) * em.count
                        }).sum();
                        if total_xp > 0 {
                            ui.label(format!("Total XP: {}", total_xp));
                        }
                    }

                    // Add monster button
                    if ui.small_button("+ Add Monster").clicked() {
                        add_monster_to = Some(enc_idx);
                    }
                });
            });
        }
    });

    // Handle deferred actions
    if let Some(idx) = remove_enc_idx {
        dungeon.encounters.remove(idx);
    }
    if let Some((enc_idx, m_idx)) = remove_monster {
        if enc_idx < dungeon.encounters.len() {
            dungeon.encounters[enc_idx].monsters.remove(m_idx);
        }
    }
    if let Some((enc_idx, m_idx)) = customize_monster {
        if enc_idx < dungeon.encounters.len() && m_idx < dungeon.encounters[enc_idx].monsters.len() {
            let em = &dungeon.encounters[enc_idx].monsters[m_idx];
            if let MonsterRef::Base { ref source, ref name } = em.monster_ref {
                if let Some(base) = monster_db.find(source, name) {
                    let custom = CustomMonster {
                        id: uuid::Uuid::new_v4().to_string(),
                        based_on: Some((source.clone(), name.clone())),
                        monster: base.clone(),
                    };
                    let custom_id = custom.id.clone();
                    dungeon.custom_monsters.push(custom);
                    dungeon.encounters[enc_idx].monsters[m_idx].monster_ref =
                        MonsterRef::Custom { id: custom_id };
                }
            }
        }
    }
    if let Some(id) = edit_custom_id {
        ui.ctx().memory_mut(|mem| {
            mem.data.insert_temp(egui::Id::new("custom_editor_id"), id);
        });
    }
    if let Some(enc_idx) = add_monster_to {
        // Open the monster browser targeted at this encounter
        ui.ctx().memory_mut(|mem| {
            mem.data.insert_temp(egui::Id::new("monster_browser_open"), true);
            mem.data.insert_temp(egui::Id::new("monster_browser_target"), enc_idx);
        });
    }
}

/// Floating Monte Carlo simulator window.
fn monte_carlo_window(
    ctx: &egui::Context,
    dungeon: &mut Dungeon,
    monster_db: &MonsterDatabase,
    combat_stats_cache: &mut CombatStatsCache,
    sim_state: &mut SimulationState,
) {
    let mut open: bool = ctx.memory(|mem|
        mem.data.get_temp(egui::Id::new("monte_carlo_open")).unwrap_or(false)
    );

    if !open { return; }

    egui::Window::new("Monte Carlo Simulator")
        .open(&mut open)
        .default_size([400.0, 400.0])
        .resizable(true)
        .show(ctx, |ui| {
            let enc_names: Vec<(usize, String)> = dungeon.encounters.iter().enumerate()
                .map(|(i, e)| (i, e.name.clone()))
                .collect();

            ui.horizontal(|ui| {
                ui.label("Side A:");
                egui::ComboBox::from_id_salt("mc_side_a")
                    .selected_text(match &sim_state.side_a {
                        SimSide::Party => "Party".to_string(),
                        SimSide::Encounter(idx) => enc_names.iter()
                            .find(|(i, _)| i == idx)
                            .map(|(_, n)| n.clone())
                            .unwrap_or_else(|| "?".to_string()),
                    })
                    .width(140.0)
                    .show_ui(ui, |ui| {
                        if ui.selectable_label(sim_state.side_a == SimSide::Party, "Party").clicked() {
                            sim_state.side_a = SimSide::Party;
                        }
                        for (i, name) in &enc_names {
                            if ui.selectable_label(sim_state.side_a == SimSide::Encounter(*i), name).clicked() {
                                sim_state.side_a = SimSide::Encounter(*i);
                            }
                        }
                    });
            });

            ui.horizontal(|ui| {
                ui.label("Side B:");
                egui::ComboBox::from_id_salt("mc_side_b")
                    .selected_text(match &sim_state.side_b {
                        SimSide::Party => "Party".to_string(),
                        SimSide::Encounter(idx) => enc_names.iter()
                            .find(|(i, _)| i == idx)
                            .map(|(_, n)| n.clone())
                            .unwrap_or_else(|| "?".to_string()),
                    })
                    .width(140.0)
                    .show_ui(ui, |ui| {
                        if ui.selectable_label(sim_state.side_b == SimSide::Party, "Party").clicked() {
                            sim_state.side_b = SimSide::Party;
                        }
                        for (i, name) in &enc_names {
                            if ui.selectable_label(sim_state.side_b == SimSide::Encounter(*i), name).clicked() {
                                sim_state.side_b = SimSide::Encounter(*i);
                            }
                        }
                    });
            });

            ui.add_space(4.0);

            let build_side = |side: &SimSide, side_idx: usize, cache: &mut CombatStatsCache| -> (Vec<combat_sim::SimCombatant>, String) {
                match side {
                    SimSide::Party => {
                        (build_combatants_from_party(&dungeon.party, side_idx), "Party".to_string())
                    }
                    SimSide::Encounter(enc_idx) => {
                        if let Some(enc) = dungeon.encounters.get(*enc_idx) {
                            let label = enc.name.clone();
                            let combatants = build_combatants_from_encounter(enc, monster_db, &dungeon.custom_monsters, cache, side_idx);
                            (combatants, label)
                        } else {
                            (Vec::new(), "?".to_string())
                        }
                    }
                }
            };

            ui.horizontal(|ui| {
                ui.label("N:");
                crate::ui::canvas_common::num_input_u32(ui, &mut sim_state.monte_carlo_n, 60.0);
                if ui.button("Run").clicked() {
                    let (side_a, label_a) = build_side(&sim_state.side_a, 0, combat_stats_cache);
                    let (side_b, label_b) = build_side(&sim_state.side_b, 1, combat_stats_cache);
                    if !side_a.is_empty() && !side_b.is_empty() {
                        sim_state.last_monte_carlo = Some(run_monte_carlo(
                            &side_a, &side_b, sim_state.monte_carlo_n,
                            label_a, label_b,
                        ));
                    }
                }
            });

            if let Some(mc) = &sim_state.last_monte_carlo {
                ui.add_space(4.0);
                ui.group(|ui| {
                    ui.label(format!("Monte Carlo ({} sims)", mc.num_sims));
                    let a_pct = mc.side_a_wins as f32 / mc.num_sims as f32 * 100.0;
                    let b_pct = mc.side_b_wins as f32 / mc.num_sims as f32 * 100.0;
                    let d_pct = mc.draws as f32 / mc.num_sims as f32 * 100.0;
                    ui.label(format!("{}: {:.1}% ({} wins)", mc.side_a_label, a_pct, mc.side_a_wins));
                    ui.label(format!("{}: {:.1}% ({} wins)", mc.side_b_label, b_pct, mc.side_b_wins));
                    if mc.draws > 0 {
                        ui.label(format!("Draws: {:.1}% ({})", d_pct, mc.draws));
                    }
                    ui.label(format!("Avg rounds: {:.1}", mc.avg_rounds));
                });
            }
        });

    ctx.memory_mut(|mem| {
        mem.data.insert_temp(egui::Id::new("monte_carlo_open"), open);
    });
}

/// Floating monster browser window.
fn monster_browser_window(
    ctx: &egui::Context,
    dungeon: &mut Dungeon,
    monster_db: &MonsterDatabase,
) {
    let mut open: bool = ctx.memory(|mem|
        mem.data.get_temp(egui::Id::new("monster_browser_open")).unwrap_or(false)
    );
    let target_enc: Option<usize> = ctx.memory(|mem|
        mem.data.get_temp(egui::Id::new("monster_browser_target"))
    );

    if !open {
        return;
    }

    // Read filter state from temp storage
    let mut search: String = ctx.memory(|mem|
        mem.data.get_temp(egui::Id::new("mb_search")).unwrap_or_default()
    );
    let mut cr_min_str: String = ctx.memory(|mem|
        mem.data.get_temp(egui::Id::new("mb_cr_min")).unwrap_or_default()
    );
    let mut cr_max_str: String = ctx.memory(|mem|
        mem.data.get_temp(egui::Id::new("mb_cr_max")).unwrap_or_default()
    );
    let mut type_filter: String = ctx.memory(|mem|
        mem.data.get_temp(egui::Id::new("mb_type_filter")).unwrap_or_default()
    );
    let selected: Option<(String, String)> = ctx.memory(|mem|
        mem.data.get_temp(egui::Id::new("mb_selected"))
    );

    egui::Window::new("Monster Browser")
        .open(&mut open)
        .default_size([500.0, 600.0])
        .resizable(true)
        .show(ctx, |ui| {
            // Search and filters
            ui.horizontal(|ui| {
                ui.label("Search:");
                ui.text_edit_singleline(&mut search);
            });
            ui.horizontal(|ui| {
                ui.label("CR:");
                ui.add(egui::TextEdit::singleline(&mut cr_min_str).desired_width(30.0).hint_text("min"));
                ui.label("-");
                ui.add(egui::TextEdit::singleline(&mut cr_max_str).desired_width(30.0).hint_text("max"));
                ui.label("Type:");
                ui.add(egui::TextEdit::singleline(&mut type_filter).desired_width(80.0));
            });

            ui.separator();

            // Build filter
            let filter = crate::data::monster_db::MonsterFilter {
                name_query: search.clone(),
                cr_min: cr_min_str.parse().ok().map(|v: f32| v),
                cr_max: cr_max_str.parse().ok().map(|v: f32| v),
                monster_type: if type_filter.is_empty() { None } else { Some(type_filter.clone()) },
                ..Default::default()
            };

            let results = if filter.is_active() {
                monster_db.filter(&filter)
            } else {
                // Show all, but cap display
                monster_db.all().iter().collect()
            };

            let total = results.len();
            ui.label(format!("{} monsters", total));

            // Split: left = list, right = detail
            ui.columns(2, |cols| {
                // Left: scrollable list
                egui::ScrollArea::vertical().id_salt("mb_list").show(&mut cols[0], |ui| {
                    let display_limit = 200;
                    for (i, m) in results.iter().take(display_limit).enumerate() {
                        let is_selected = selected.as_ref()
                            .is_some_and(|(s, n)| s == &m.source && n == &m.name);
                        let label = format!(
                            "CR {} {}",
                            m.cr.cr_string(),
                            m.name,
                        );
                        if ui.selectable_label(is_selected, &label).clicked() {
                            ui.ctx().memory_mut(|mem| {
                                mem.data.insert_temp(
                                    egui::Id::new("mb_selected"),
                                    (m.source.clone(), m.name.clone()),
                                );
                            });
                        }

                        // Double-click to add
                        if ui.ctx().input(|i| i.pointer.button_double_clicked(egui::PointerButton::Primary)) && is_selected {
                            if let Some(enc_idx) = target_enc {
                                if enc_idx < dungeon.encounters.len() {
                                    dungeon.encounters[enc_idx].monsters.push(EncounterMonster {
                                        monster_ref: MonsterRef::Base {
                                            source: m.source.clone(),
                                            name: m.name.clone(),
                                        },
                                        count: 1,
                                        notes: String::new(),
                                    });
                                }
                            }
                        }

                        let _ = i; // suppress unused warning
                    }
                    if total > display_limit {
                        ui.label(format!("... and {} more (refine search)", total - display_limit));
                    }
                });

                // Right: stat block detail
                egui::ScrollArea::vertical().id_salt("mb_detail").show(&mut cols[1], |ui| {
                    if let Some((ref src, ref name)) = selected {
                        if let Some(m) = monster_db.find(src, name) {
                            draw_stat_block(ui, m, monster_db);

                            ui.add_space(8.0);
                            if let Some(enc_idx) = target_enc {
                                if ui.button("Add to Encounter").clicked() {
                                    if enc_idx < dungeon.encounters.len() {
                                        dungeon.encounters[enc_idx].monsters.push(EncounterMonster {
                                            monster_ref: MonsterRef::Base {
                                                source: m.source.clone(),
                                                name: m.name.clone(),
                                            },
                                            count: 1,
                                            notes: String::new(),
                                        });
                                    }
                                }
                            }
                            if ui.button("Add as Custom Copy").clicked() {
                                let custom = CustomMonster {
                                    id: uuid::Uuid::new_v4().to_string(),
                                    based_on: Some((src.clone(), name.clone())),
                                    monster: m.clone(),
                                };
                                if let Some(enc_idx) = target_enc {
                                    if enc_idx < dungeon.encounters.len() {
                                        dungeon.encounters[enc_idx].monsters.push(EncounterMonster {
                                            monster_ref: MonsterRef::Custom { id: custom.id.clone() },
                                            count: 1,
                                            notes: String::new(),
                                        });
                                    }
                                }
                                dungeon.custom_monsters.push(custom);
                            }
                        }
                    } else {
                        ui.label("Select a monster to view its stats.");
                    }
                });
            });
        });

    // Persist filter state
    ctx.memory_mut(|mem| {
        mem.data.insert_temp(egui::Id::new("monster_browser_open"), open);
        mem.data.insert_temp(egui::Id::new("mb_search"), search);
        mem.data.insert_temp(egui::Id::new("mb_cr_min"), cr_min_str);
        mem.data.insert_temp(egui::Id::new("mb_cr_max"), cr_max_str);
        mem.data.insert_temp(egui::Id::new("mb_type_filter"), type_filter);
    });
}

/// Draw a formatted D&D stat block for a monster, optionally with token image.
fn draw_stat_block(ui: &mut egui::Ui, m: &Monster, monster_db: &MonsterDatabase) {
    // Token image
    if let Some(token_path) = monster_db.token_path(&m.source, &m.name) {
        let uri = format!("file://{}", token_path.display());
        ui.add(
            egui::Image::from_uri(uri)
                .fit_to_exact_size(egui::vec2(80.0, 80.0))
                .corner_radius(4.0),
        );
        ui.add_space(4.0);
    }

    // Name
    ui.heading(&m.name);

    // Size, type, alignment
    let size = m.size.iter().map(|s| size_label(s)).collect::<Vec<_>>().join("/");
    let type_str = m.monster_type.display();
    let align = alignment_display(&m.alignment);
    ui.label(format!("{} {}, {}", size, type_str, align));

    ui.separator();

    // AC
    let ac_str = m.ac.iter().map(|a| a.display()).collect::<Vec<_>>().join(", ");
    ui.label(format!("AC: {}", ac_str));

    // HP
    ui.label(format!("HP: {}", m.hp.display()));

    // Speed
    ui.label(format!("Speed: {}", m.speed.display()));

    ui.separator();

    // Ability scores
    ui.horizontal(|ui| {
        for (label, score) in [
            ("STR", m.str_score), ("DEX", m.dex_score), ("CON", m.con_score),
            ("INT", m.int_score), ("WIS", m.wis_score), ("CHA", m.cha_score),
        ] {
            ui.vertical(|ui| {
                ui.label(egui::RichText::new(label).strong().size(10.0));
                ui.label(format!("{} ({})", score, Monster::modifier_str(score)));
            });
        }
    });

    ui.separator();

    // Saves
    if !m.save.is_empty() {
        let saves: Vec<String> = m.save.iter()
            .map(|(k, v)| format!("{} {}", capitalize(k), v))
            .collect();
        ui.label(format!("Saves: {}", saves.join(", ")));
    }

    // Skills
    if !m.skill.is_empty() {
        let skills: Vec<String> = m.skill.iter()
            .map(|(k, v)| format!("{} {}", capitalize(k), v))
            .collect();
        ui.label(format!("Skills: {}", skills.join(", ")));
    }

    // Damage immunities/resistances/vulnerabilities
    if !m.immune.is_empty() {
        ui.label(format!("Damage Immunities: {}", damage_list_display(&m.immune)));
    }
    if !m.resist.is_empty() {
        ui.label(format!("Damage Resistances: {}", damage_list_display(&m.resist)));
    }
    if !m.vulnerable.is_empty() {
        ui.label(format!("Vulnerabilities: {}", damage_list_display(&m.vulnerable)));
    }
    if !m.condition_immune.is_empty() {
        let ci: Vec<String> = m.condition_immune.iter().map(|v| match v {
            serde_json::Value::String(s) => s.clone(),
            serde_json::Value::Object(obj) => {
                let mut parts = Vec::new();
                if let Some(pre) = obj.get("preNote").and_then(|v| v.as_str()) {
                    parts.push(pre.to_string());
                }
                if let Some(arr) = obj.get("conditionImmune").and_then(|v| v.as_array()) {
                    let items: Vec<&str> = arr.iter().filter_map(|v| v.as_str()).collect();
                    parts.push(items.join(", "));
                }
                if let Some(note) = obj.get("note").and_then(|v| v.as_str()) {
                    parts.push(note.to_string());
                }
                parts.join(" ")
            }
            _ => v.to_string(),
        }).collect();
        ui.label(format!("Condition Immunities: {}", ci.join("; ")));
    }

    // Senses
    if !m.senses.is_empty() || m.passive.is_some() {
        let mut senses = m.senses.join(", ");
        if let Some(pp) = m.passive {
            if !senses.is_empty() { senses.push_str(", "); }
            senses.push_str(&format!("passive Perception {}", pp));
        }
        ui.label(format!("Senses: {}", senses));
    }

    // Languages
    if !m.languages.is_empty() {
        ui.label(format!("Languages: {}", m.languages.join(", ")));
    }

    // CR
    let xp = m.cr.xp();
    ui.label(format!("CR: {} ({} XP)", m.cr.cr_string(), xp));

    // Traits
    if !m.traits.is_empty() {
        ui.separator();
        for feature in &m.traits {
            ui.label(egui::RichText::new(&feature.name).strong().italics());
            ui.label(feature.entries_text());
        }
    }

    // Actions
    if !m.action.is_empty() {
        ui.separator();
        ui.label(egui::RichText::new("Actions").strong().size(13.0));
        for feature in &m.action {
            ui.label(egui::RichText::new(&feature.name).strong().italics());
            ui.label(feature.entries_text());
        }
    }

    // Reactions
    if !m.reaction.is_empty() {
        ui.separator();
        ui.label(egui::RichText::new("Reactions").strong().size(13.0));
        for feature in &m.reaction {
            ui.label(egui::RichText::new(&feature.name).strong().italics());
            ui.label(feature.entries_text());
        }
    }

    // Legendary actions
    if !m.legendary.is_empty() {
        ui.separator();
        ui.label(egui::RichText::new("Legendary Actions").strong().size(13.0));
        for feature in &m.legendary {
            ui.label(egui::RichText::new(&feature.name).strong().italics());
            ui.label(feature.entries_text());
        }
    }
}

// --- Custom Monster Editor ---

/// Window for editing a custom monster's fields.
fn custom_monster_editor_window(
    ctx: &egui::Context,
    dungeon: &mut Dungeon,
) {
    let editor_id: Option<String> = ctx.memory(|mem|
        mem.data.get_temp(egui::Id::new("custom_editor_id"))
    );
    let Some(editing_id) = editor_id else { return };

    // Find the custom monster index
    let Some(cm_idx) = dungeon.custom_monsters.iter().position(|c| c.id == editing_id) else {
        // Not found, clear the editor
        ctx.memory_mut(|mem| {
            mem.data.remove::<String>(egui::Id::new("custom_editor_id"));
        });
        return;
    };

    let mut open = true;
    let title = format!("Edit: {}", dungeon.custom_monsters[cm_idx].monster.name);

    egui::Window::new(title)
        .id(egui::Id::new("custom_monster_editor_window"))
        .open(&mut open)
        .default_size([450.0, 600.0])
        .resizable(true)
        .show(ctx, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                let m = &mut dungeon.custom_monsters[cm_idx].monster;

                // Name
                ui.horizontal(|ui| {
                    ui.label("Name:");
                    ui.text_edit_singleline(&mut m.name);
                });

                // Size
                let size_options = ["T", "S", "M", "L", "H", "G"];
                let current_size = m.size.first().cloned().unwrap_or_else(|| "M".to_string());
                ui.horizontal(|ui| {
                    ui.label("Size:");
                    egui::ComboBox::from_id_salt("cm_size")
                        .selected_text(size_label(&current_size))
                        .show_ui(ui, |ui| {
                            for s in &size_options {
                                if ui.selectable_label(current_size == *s, size_label(s)).clicked() {
                                    m.size = vec![s.to_string()];
                                }
                            }
                        });
                });

                // Type
                let mut type_name = m.monster_type.display();
                ui.horizontal(|ui| {
                    ui.label("Type:");
                    if ui.text_edit_singleline(&mut type_name).changed() {
                        m.monster_type = MonsterType::Simple(type_name);
                    }
                });

                ui.separator();

                // AC
                ui.horizontal(|ui| {
                    ui.label("AC:");
                    let mut ac_val = m.ac.first().and_then(|a| a.value()).unwrap_or(10) as i32;
                    if crate::ui::canvas_common::num_input_i32(ui, &mut ac_val, 35.0) {
                        m.ac = vec![ArmorClass::Simple(ac_val as u8)];
                    }
                });

                // HP
                ui.horizontal(|ui| {
                    ui.label("HP:");
                    match &mut m.hp {
                        HitPoints::Formula { average, formula } => {
                            let mut avg = *average;
                            ui.label("avg:"); crate::ui::canvas_common::num_input_i32(ui, &mut avg, 40.0);
                            *average = avg;
                            ui.add(egui::TextEdit::singleline(formula).desired_width(80.0).hint_text("formula"));
                        }
                        _ => {
                            let mut avg: i32 = 0;
                            let mut formula = String::new();
                            ui.label("avg:"); crate::ui::canvas_common::num_input_i32(ui, &mut avg, 40.0);
                            ui.add(egui::TextEdit::singleline(&mut formula).desired_width(80.0).hint_text("formula"));
                            if avg > 0 || !formula.is_empty() {
                                m.hp = HitPoints::Formula { average: avg, formula };
                            }
                        }
                    }
                });

                // Speed
                ui.label("Speed:");
                ui.indent("speed_indent", |ui| {
                    speed_edit(ui, "Walk", &mut m.speed.walk, "speed_walk");
                    speed_edit(ui, "Fly", &mut m.speed.fly, "speed_fly");
                    speed_edit(ui, "Swim", &mut m.speed.swim, "speed_swim");
                    speed_edit(ui, "Climb", &mut m.speed.climb, "speed_climb");
                    speed_edit(ui, "Burrow", &mut m.speed.burrow, "speed_burrow");
                });

                ui.separator();

                // Ability scores
                ui.label(egui::RichText::new("Ability Scores").strong());
                ui.horizontal(|ui| {
                    for (label, score) in [
                        ("STR", &mut m.str_score),
                        ("DEX", &mut m.dex_score),
                        ("CON", &mut m.con_score),
                        ("INT", &mut m.int_score),
                        ("WIS", &mut m.wis_score),
                        ("CHA", &mut m.cha_score),
                    ] {
                        ui.vertical(|ui| {
                            ui.label(egui::RichText::new(label).strong().size(10.0));
                            let mut val = *score as i32;
                            if crate::ui::canvas_common::num_input_i32(ui, &mut val, 35.0) {
                                *score = val as u8;
                            }
                        });
                    }
                });

                ui.separator();

                // CR
                ui.horizontal(|ui| {
                    ui.label("CR:");
                    let mut cr_str = m.cr.cr_string().to_string();
                    if ui.add(egui::TextEdit::singleline(&mut cr_str).desired_width(50.0)).changed() {
                        m.cr = ChallengeRating::Simple(cr_str);
                    }
                });

                ui.separator();

                // Feature sections
                feature_list_editor(ui, "Traits", &mut m.traits, "cm_traits");
                feature_list_editor(ui, "Actions", &mut m.action, "cm_actions");
                feature_list_editor(ui, "Reactions", &mut m.reaction, "cm_reactions");
                feature_list_editor(ui, "Legendary Actions", &mut m.legendary, "cm_legendary");
            });
        });

    if !open {
        ctx.memory_mut(|mem| {
            mem.data.remove::<String>(egui::Id::new("custom_editor_id"));
        });
    }
}

/// Helper: edit a speed value with a drag value and clear button.
fn speed_edit(ui: &mut egui::Ui, label: &str, speed: &mut SpeedValue, _id_salt: &str) {
    ui.horizontal(|ui| {
        ui.label(format!("{}:", label));
        let mut val = speed.value().unwrap_or(0) as i32;
        let has_value = speed.value().is_some();
        if has_value {
            let mut uval = val as u32;
            let changed = crate::ui::canvas_common::num_input_u32(ui, &mut uval, 40.0);
            ui.label("ft.");
            val = uval as i32;
            if changed {
                *speed = SpeedValue::Simple(val as u32);
            }
            if ui.small_button("X").clicked() {
                *speed = SpeedValue::None;
            }
        } else {
            if ui.small_button("+").on_hover_text(format!("Add {} speed", label)).clicked() {
                *speed = SpeedValue::Simple(30);
            }
        }
    });
}

/// Editor for a list of Features (traits, actions, etc.)
fn feature_list_editor(ui: &mut egui::Ui, section_label: &str, features: &mut Vec<Feature>, id_salt: &str) {
    egui::CollapsingHeader::new(egui::RichText::new(section_label).strong())
        .id_salt(id_salt)
        .show(ui, |ui| {
            let mut remove_idx = None;
            for (i, feature) in features.iter_mut().enumerate() {
                ui.push_id(format!("{}_{}", id_salt, i), |ui| {
                    ui.group(|ui| {
                        ui.horizontal(|ui| {
                            ui.label("Name:");
                            ui.text_edit_singleline(&mut feature.name);
                            if ui.small_button("-").clicked() {
                                remove_idx = Some(i);
                            }
                        });
                        // Edit entries as multiline text
                        let mut text = feature.entries_text();
                        if ui.add(
                            egui::TextEdit::multiline(&mut text)
                                .desired_width(f32::INFINITY)
                                .desired_rows(3)
                        ).changed() {
                            // Convert back to entries: each line becomes a string entry
                            feature.entries = text.lines()
                                .map(|line| serde_json::Value::String(line.to_string()))
                                .collect();
                        }
                    });
                });
            }
            if let Some(idx) = remove_idx {
                features.remove(idx);
            }
            if ui.small_button(format!("+ Add {}", section_label)).clicked() {
                features.push(Feature {
                    name: String::new(),
                    entries: Vec::new(),
                });
            }
        });
}

// --- Monster Workshop Window (Merge + Custom Monsters) ---

fn monster_workshop_window(
    ctx: &egui::Context,
    dungeon: &mut Dungeon,
    monster_db: &MonsterDatabase,
) {
    let mut open: bool = ctx.memory(|mem|
        mem.data.get_temp(egui::Id::new("monster_workshop_open")).unwrap_or(false)
    );

    if !open {
        return;
    }

    // Merge state stored in temp memory
    let mut merge_a_src: String = ctx.memory(|mem|
        mem.data.get_temp(egui::Id::new("merge_a_src")).unwrap_or_default()
    );
    let mut merge_a_name: String = ctx.memory(|mem|
        mem.data.get_temp(egui::Id::new("merge_a_name")).unwrap_or_default()
    );
    let mut merge_b_src: String = ctx.memory(|mem|
        mem.data.get_temp(egui::Id::new("merge_b_src")).unwrap_or_default()
    );
    let mut merge_b_name: String = ctx.memory(|mem|
        mem.data.get_temp(egui::Id::new("merge_b_name")).unwrap_or_default()
    );
    let mut merge_a_custom_id: String = ctx.memory(|mem|
        mem.data.get_temp(egui::Id::new("merge_a_custom_id")).unwrap_or_default()
    );
    let mut merge_b_custom_id: String = ctx.memory(|mem|
        mem.data.get_temp(egui::Id::new("merge_b_custom_id")).unwrap_or_default()
    );

    // MergeConfig defaults (we need to serialize the overrides to temp storage)
    let mut default_numeric_idx: usize = ctx.memory(|mem|
        mem.data.get_temp(egui::Id::new("merge_def_num")).unwrap_or(1usize) // Max
    );
    let mut default_list_idx: usize = ctx.memory(|mem|
        mem.data.get_temp(egui::Id::new("merge_def_list")).unwrap_or(2usize) // ConcatA
    );
    let mut default_string_idx: usize = ctx.memory(|mem|
        mem.data.get_temp(egui::Id::new("merge_def_str")).unwrap_or(4usize) // TakeA
    );

    // Per-field overrides stored as JSON string in temp memory
    let overrides_json: String = ctx.memory(|mem|
        mem.data.get_temp(egui::Id::new("merge_overrides")).unwrap_or_else(|| "{}".to_string())
    );
    let mut overrides: HashMap<String, usize> = serde_json::from_str(&overrides_json).unwrap_or_default();

    let strategy_from_idx = |idx: usize| -> MergeStrategy {
        MergeStrategy::ALL.get(idx).cloned().unwrap_or(MergeStrategy::Max)
    };

    egui::Window::new("Monster Workshop")
        .open(&mut open)
        .default_size([550.0, 700.0])
        .resizable(true)
        .show(ctx, |ui| {
            // --- Custom Monsters List ---
            if !dungeon.custom_monsters.is_empty() {
                ui.label(egui::RichText::new("Custom Monsters").strong().size(14.0));
                let mut edit_id = None;
                for cm in &dungeon.custom_monsters {
                    ui.horizontal(|ui| {
                        ui.label(&cm.monster.name);
                        if ui.small_button("Edit").clicked() {
                            edit_id = Some(cm.id.clone());
                        }
                    });
                }
                if let Some(id) = edit_id {
                    ctx.memory_mut(|mem| {
                        mem.data.insert_temp(egui::Id::new("custom_editor_id"), id);
                    });
                }
                ui.separator();
            }

            // --- Merge Monsters ---
            ui.label(egui::RichText::new("Merge Monsters").strong().size(14.0));
            ui.add_space(4.0);

            // Monster A picker
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("Monster A:").strong());
                let a_label = if !merge_a_name.is_empty() {
                    merge_a_name.clone()
                } else if !merge_a_custom_id.is_empty() {
                    dungeon.custom_monsters.iter()
                        .find(|c| c.id == merge_a_custom_id)
                        .map(|c| format!("{} (custom)", c.monster.name))
                        .unwrap_or_else(|| "Select...".to_string())
                } else {
                    "Select...".to_string()
                };
                egui::ComboBox::from_id_salt("merge_pick_a")
                    .selected_text(&a_label)
                    .width(200.0)
                    .show_ui(ui, |ui| {
                        // Show custom monsters first
                        for cm in &dungeon.custom_monsters {
                            let label = format!("{} (custom)", cm.monster.name);
                            if ui.selectable_label(merge_a_custom_id == cm.id, &label).clicked() {
                                merge_a_custom_id = cm.id.clone();
                                merge_a_src.clear();
                                merge_a_name.clear();
                            }
                        }
                        // Show database monsters (limited to common ones based on current search)
                        // For usability, show a text hint
                        ui.separator();
                        ui.label("Database (first 50):");
                        for m in monster_db.all().iter().take(50) {
                            let label = format!("{} (CR {}, {})", m.name, m.cr.cr_string(), m.source);
                            let selected = merge_a_src == m.source && merge_a_name == m.name;
                            if ui.selectable_label(selected, &label).clicked() {
                                merge_a_src = m.source.clone();
                                merge_a_name = m.name.clone();
                                merge_a_custom_id.clear();
                            }
                        }
                    });
            });

            // Monster B picker
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("Monster B:").strong());
                let b_label = if !merge_b_name.is_empty() {
                    merge_b_name.clone()
                } else if !merge_b_custom_id.is_empty() {
                    dungeon.custom_monsters.iter()
                        .find(|c| c.id == merge_b_custom_id)
                        .map(|c| format!("{} (custom)", c.monster.name))
                        .unwrap_or_else(|| "Select...".to_string())
                } else {
                    "Select...".to_string()
                };
                egui::ComboBox::from_id_salt("merge_pick_b")
                    .selected_text(&b_label)
                    .width(200.0)
                    .show_ui(ui, |ui| {
                        for cm in &dungeon.custom_monsters {
                            let label = format!("{} (custom)", cm.monster.name);
                            if ui.selectable_label(merge_b_custom_id == cm.id, &label).clicked() {
                                merge_b_custom_id = cm.id.clone();
                                merge_b_src.clear();
                                merge_b_name.clear();
                            }
                        }
                        ui.separator();
                        ui.label("Database (first 50):");
                        for m in monster_db.all().iter().take(50) {
                            let label = format!("{} (CR {}, {})", m.name, m.cr.cr_string(), m.source);
                            let selected = merge_b_src == m.source && merge_b_name == m.name;
                            if ui.selectable_label(selected, &label).clicked() {
                                merge_b_src = m.source.clone();
                                merge_b_name = m.name.clone();
                                merge_b_custom_id.clear();
                            }
                        }
                    });
            });

            ui.separator();

            // Default strategies
            ui.label(egui::RichText::new("Default Strategies").strong());
            ui.horizontal(|ui| {
                ui.label("Numeric:");
                egui::ComboBox::from_id_salt("merge_def_num_cb")
                    .selected_text(strategy_from_idx(default_numeric_idx).label())
                    .show_ui(ui, |ui| {
                        for (i, s) in MergeStrategy::ALL.iter().enumerate() {
                            if ui.selectable_label(i == default_numeric_idx, s.label()).clicked() {
                                default_numeric_idx = i;
                            }
                        }
                    });
            });
            ui.horizontal(|ui| {
                ui.label("Lists:");
                egui::ComboBox::from_id_salt("merge_def_list_cb")
                    .selected_text(strategy_from_idx(default_list_idx).label())
                    .show_ui(ui, |ui| {
                        for (i, s) in MergeStrategy::ALL.iter().enumerate() {
                            if ui.selectable_label(i == default_list_idx, s.label()).clicked() {
                                default_list_idx = i;
                            }
                        }
                    });
            });
            ui.horizontal(|ui| {
                ui.label("Strings:");
                egui::ComboBox::from_id_salt("merge_def_str_cb")
                    .selected_text(strategy_from_idx(default_string_idx).label())
                    .show_ui(ui, |ui| {
                        for (i, s) in MergeStrategy::ALL.iter().enumerate() {
                            if ui.selectable_label(i == default_string_idx, s.label()).clicked() {
                                default_string_idx = i;
                            }
                        }
                    });
            });

            // Per-field overrides
            ui.separator();
            egui::CollapsingHeader::new("Per-Field Overrides")
                .id_salt("merge_overrides_section")
                .show(ui, |ui| {
                    ui.label(egui::RichText::new("Numeric Fields").size(11.0).strong());
                    for (field_key, field_label) in MERGE_NUMERIC_FIELDS {
                        merge_field_override(ui, field_key, field_label, &mut overrides);
                    }
                    ui.add_space(4.0);
                    ui.label(egui::RichText::new("List Fields").size(11.0).strong());
                    for (field_key, field_label) in MERGE_LIST_FIELDS {
                        merge_field_override(ui, field_key, field_label, &mut overrides);
                    }
                    ui.add_space(4.0);
                    ui.label(egui::RichText::new("String Fields").size(11.0).strong());
                    for (field_key, field_label) in MERGE_STRING_FIELDS {
                        merge_field_override(ui, field_key, field_label, &mut overrides);
                    }
                });

            // Resolve monsters A and B
            let monster_a: Option<Monster> = if !merge_a_name.is_empty() {
                monster_db.find(&merge_a_src, &merge_a_name).cloned()
            } else if !merge_a_custom_id.is_empty() {
                dungeon.custom_monsters.iter()
                    .find(|c| c.id == merge_a_custom_id)
                    .map(|c| c.monster.clone())
            } else {
                None
            };
            let monster_b: Option<Monster> = if !merge_b_name.is_empty() {
                monster_db.find(&merge_b_src, &merge_b_name).cloned()
            } else if !merge_b_custom_id.is_empty() {
                dungeon.custom_monsters.iter()
                    .find(|c| c.id == merge_b_custom_id)
                    .map(|c| c.monster.clone())
            } else {
                None
            };

            // Build config
            let mut config = MergeConfig {
                default_numeric: strategy_from_idx(default_numeric_idx),
                default_list: strategy_from_idx(default_list_idx),
                default_string: strategy_from_idx(default_string_idx),
                overrides: HashMap::new(),
            };
            for (field, idx) in &overrides {
                config.overrides.insert(field.clone(), strategy_from_idx(*idx));
            }

            // Preview
            if let (Some(ref a), Some(ref b)) = (&monster_a, &monster_b) {
                ui.separator();
                ui.label(egui::RichText::new("Preview").strong().size(14.0));
                let merged = merge_monsters(a, b, &config);
                egui::ScrollArea::vertical().id_salt("merge_preview").max_height(300.0).show(ui, |ui| {
                    draw_stat_block(ui, &merged, monster_db);
                });

                ui.add_space(8.0);
                if ui.button("Create Merged Monster").clicked() {
                    let custom = CustomMonster {
                        id: uuid::Uuid::new_v4().to_string(),
                        based_on: None,
                        monster: merged,
                    };
                    dungeon.custom_monsters.push(custom);
                    // Provide feedback - we could close the window or show a message
                }
            } else {
                ui.separator();
                ui.label("Select both Monster A and Monster B to see a preview.");
            }
        });

    // Persist state
    let overrides_json = serde_json::to_string(&overrides).unwrap_or_else(|_| "{}".to_string());
    ctx.memory_mut(|mem| {
        mem.data.insert_temp(egui::Id::new("monster_workshop_open"), open);
        mem.data.insert_temp(egui::Id::new("merge_a_src"), merge_a_src);
        mem.data.insert_temp(egui::Id::new("merge_a_name"), merge_a_name);
        mem.data.insert_temp(egui::Id::new("merge_b_src"), merge_b_src);
        mem.data.insert_temp(egui::Id::new("merge_b_name"), merge_b_name);
        mem.data.insert_temp(egui::Id::new("merge_a_custom_id"), merge_a_custom_id);
        mem.data.insert_temp(egui::Id::new("merge_b_custom_id"), merge_b_custom_id);
        mem.data.insert_temp(egui::Id::new("merge_def_num"), default_numeric_idx);
        mem.data.insert_temp(egui::Id::new("merge_def_list"), default_list_idx);
        mem.data.insert_temp(egui::Id::new("merge_def_str"), default_string_idx);
        mem.data.insert_temp(egui::Id::new("merge_overrides"), overrides_json);
    });
}

/// Draw a per-field override combo box.
fn merge_field_override(ui: &mut egui::Ui, field_key: &str, field_label: &str, overrides: &mut HashMap<String, usize>) {
    ui.horizontal(|ui| {
        ui.label(format!("{}:", field_label));
        let current = overrides.get(field_key).copied();
        let label = current
            .map(|i| MergeStrategy::ALL.get(i).map(|s| s.label()).unwrap_or("Default"))
            .unwrap_or("Default");
        egui::ComboBox::from_id_salt(format!("merge_ov_{}", field_key))
            .selected_text(label)
            .width(100.0)
            .show_ui(ui, |ui| {
                if ui.selectable_label(current.is_none(), "Default").clicked() {
                    overrides.remove(field_key);
                }
                for (i, s) in MergeStrategy::ALL.iter().enumerate() {
                    if ui.selectable_label(current == Some(i), s.label()).clicked() {
                        overrides.insert(field_key.to_string(), i);
                    }
                }
            });
    });
}

/// Resolve a MonsterRef to a display name.
fn resolve_monster_name(
    mref: &MonsterRef,
    db: &MonsterDatabase,
    custom: &[CustomMonster],
) -> String {
    match mref {
        MonsterRef::Base { source, name } => {
            if let Some(m) = db.find(source, name) {
                format!("{} ({})", m.name, m.cr.cr_string())
            } else {
                format!("{} [{}]", name, source)
            }
        }
        MonsterRef::Custom { id } => {
            if let Some(cm) = custom.iter().find(|c| c.id == *id) {
                format!("{} (custom)", cm.monster.name)
            } else {
                format!("Custom [{}]", &id[..8.min(id.len())])
            }
        }
        MonsterRef::Merged { id } => {
            if let Some(cm) = custom.iter().find(|c| c.id == *id) {
                format!("{} (merged)", cm.monster.name)
            } else {
                format!("Merged [{}]", &id[..8.min(id.len())])
            }
        }
    }
}

/// Resolve a MonsterRef to XP.
fn resolve_monster_xp(
    mref: &MonsterRef,
    db: &MonsterDatabase,
    custom: &[CustomMonster],
) -> u32 {
    match mref {
        MonsterRef::Base { source, name } => {
            db.find(source, name).map(|m| m.cr.xp()).unwrap_or(0)
        }
        MonsterRef::Custom { id } | MonsterRef::Merged { id } => {
            custom.iter()
                .find(|c| c.id == *id)
                .map(|c| c.monster.cr.xp())
                .unwrap_or(0)
        }
    }
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        None => String::new(),
        Some(f) => f.to_uppercase().chain(c).collect(),
    }
}
