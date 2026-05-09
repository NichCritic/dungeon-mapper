use crate::model::*;
use crate::presentation::PresentationState;
use crate::presentation::combat_tracker::CombatantId;
use crate::render::bg_cache::BackgroundRenderCache;
use crate::render::recording::replay_commands;
use crate::render::themed::RenderOptions;
use crate::ui::canvas_common::{handle_pan_zoom, truncate_to_fit, ViewState};
use crate::util::{ViewTransform, GRID_PX};

pub struct PlayerViewState {
    pub view: ViewState,
    pub render_cache: BackgroundRenderCache,
    /// Canvas size from the last frame, used by sidebar for centering.
    pub canvas_size: egui::Vec2,
    /// When true, pan and zoom are disabled on the player view.
    pub locked: bool,
    /// Map rotation in 90° increments (0, 1, 2, 3 = 0°, 90°, 180°, 270°).
    pub map_rotation: u8,
}

impl Default for PlayerViewState {
    fn default() -> Self {
        Self {
            view: ViewState::default(),
            render_cache: BackgroundRenderCache::default(),
            canvas_size: egui::Vec2::ZERO,
            locked: false,
            map_rotation: 0,
        }
    }
}

fn player_input_hash(
    layout: &SpatialLayout,
    theme: &Theme,
    presentation: &PresentationState,
    dungeon: &Dungeon,
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
        c.connection_id.hash(&mut h);
        c.width.hash(&mut h);
        for wp in &c.waypoints {
            wp.x.hash(&mut h);
            wp.y.hash(&mut h);
        }
    }
    theme.wall_color.hash(&mut h);
    theme.floor_color.hash(&mut h);
    theme.bg_color.hash(&mut h);
    for (room_id, vis) in &presentation.room_visibility {
        room_id.hash(&mut h);
        std::mem::discriminant(vis).hash(&mut h);
    }
    presentation.doors_open.len().hash(&mut h);
    for conn_id in &presentation.doors_open {
        conn_id.hash(&mut h);
    }
    dungeon.light_sources.len().hash(&mut h);
    for light in &dungeon.light_sources {
        light.id.hash(&mut h);
        light.radius.to_bits().hash(&mut h);
        light.intensity.to_bits().hash(&mut h);
    }
    dungeon.ambient_light.to_bits().hash(&mut h);
    presentation.show_labels_player.hash(&mut h);
    h.finish()
}

/// Render the player view in a second egui viewport.
pub fn player_viewport(
    ctx: &egui::Context,
    dungeon: &Dungeon,
    presentation: &PresentationState,
    state: &mut PlayerViewState,
) {
    // F11 toggles fullscreen
    let f11 = ctx.input(|i| i.key_pressed(egui::Key::F11));
    if f11 {
        let is_fullscreen = ctx.input(|i| i.viewport().fullscreen.unwrap_or(false));
        ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(!is_fullscreen));
    }

    egui::CentralPanel::default().show(ctx, |ui| {
        let (response, painter) = ui.allocate_painter(
            ui.available_size(),
            egui::Sense::click_and_drag(),
        );
        let rect = response.rect;

        let bg = dungeon.theme.bg_color;
        painter.rect_filled(rect, 0.0, egui::Color32::from_rgba_unmultiplied(bg[0], bg[1], bg[2], bg[3]));

        if !state.locked {
            handle_pan_zoom(&response, &mut state.view);
        }
        state.canvas_size = rect.size();
        let rotation_rad = state.map_rotation as f32 * std::f32::consts::FRAC_PI_2;
        let base_transform = ViewTransform::new(state.view.offset, state.view.zoom, rect);
        let transform = base_transform.clone().with_rotation(rotation_rad);

        let Some(layout) = &dungeon.layout else {
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "No map available yet.",
                egui::FontId::proportional(16.0),
                egui::Color32::from_rgb(150, 150, 150),
            );
            return;
        };

        // Rebuild if inputs changed
        let hash = player_input_hash(layout, &dungeon.theme, presentation, dungeon);
        let options = RenderOptions {
            show_grid: true,
            show_labels: presentation.show_labels_player,
            show_notes: false,
            show_secrets: false,
            show_decor: true,
        };
        // Clone data for background thread (PresentationState isn't Clone due to CombatTracker,
        // so clone just the fields render_player_view needs)
        let graph = dungeon.graph.clone();
        let layout_c = layout.clone();
        let theme = dungeon.theme.clone();
        let pres_snapshot = crate::presentation::PresentationSnapshot {
            room_visibility: presentation.room_visibility.clone(),
            doors_open: presentation.doors_open.clone(),
        };
        let lights = dungeon.light_sources.clone();
        let ambient = dungeon.ambient_light;
        let cache_ready = state.render_cache.ensure_with(hash, "Player View", move || {
            let mut recorder = crate::render::recording::RecordingRenderer::new();
            crate::render::presentation::render_player_view_snapshot(
                &mut recorder, &graph, &layout_c, &theme, &pres_snapshot, &lights, ambient, &options,
            );
            recorder.commands
        });

        if cache_ready {
            if let Some(commands) = state.render_cache.commands() {
                replay_commands(&painter, &transform, commands);
            }
        } else {
            let msg = format!("Rendering {}...",
                state.render_cache.pending_label().unwrap_or("player view"));
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

        // AoE markers (live overlay on player view, no center crosshairs)
        crate::presentation::aoe::render_aoe_markers(&painter, &transform, &dungeon.aoe_markers, false);

        // Initiative tracker overlay (top-right + bottom-left upside-down)
        if let Some(tracker) = &presentation.combat_tracker {
            if !tracker.initiative_order.is_empty() {
                let lines = build_initiative_display(tracker);
                if !lines.is_empty() {
                    let font = egui::FontId::proportional(14.0);
                    let line_height = 20.0;
                    let padding = 8.0;
                    let box_height = lines.len() as f32 * line_height + padding * 2.0;
                    let bg = egui::Color32::from_rgba_unmultiplied(0, 0, 0, 180);

                    // Pre-layout galleys and measure max width
                    let galleys: Vec<_> = lines.iter().map(|(text, bold)| {
                        let color = if *bold {
                            egui::Color32::WHITE
                        } else {
                            egui::Color32::from_rgb(180, 180, 180)
                        };
                        let galley = painter.layout_no_wrap(text.clone(), font.clone(), color);
                        (galley, *bold)
                    }).collect();
                    let max_width = galleys.iter().map(|(g, _)| g.size().x).fold(0.0f32, f32::max);
                    let box_width = max_width + padding * 2.0;

                    // --- Top-right (normal) ---
                    let tr_rect = egui::Rect::from_min_size(
                        egui::pos2(rect.right() - box_width - 12.0, rect.top() + 12.0),
                        egui::vec2(box_width, box_height),
                    );
                    painter.rect_filled(tr_rect, 6.0, bg);
                    for (i, (galley, _)) in galleys.iter().enumerate() {
                        let pos = egui::pos2(tr_rect.left() + padding, tr_rect.top() + padding + i as f32 * line_height);
                        let shape = egui::epaint::TextShape::new(pos, galley.clone(), egui::Color32::PLACEHOLDER);
                        painter.add(shape);
                    }

                    // --- Bottom-left (upside-down) ---
                    let bl_rect = egui::Rect::from_min_size(
                        egui::pos2(rect.left() + 12.0, rect.bottom() - box_height - 12.0),
                        egui::vec2(box_width, box_height),
                    );
                    painter.rect_filled(bl_rect, 6.0, bg);
                    for (i, (galley, _)) in galleys.iter().enumerate() {
                        // When rotated PI around pos, pos becomes the bottom-right of the text.
                        // Place pos so text fills the box from bottom-right to top-left.
                        let galley_w = galley.size().x;
                        let galley_h = galley.size().y;
                        // Normal position would be (left+padding, top+padding + i*line_height)
                        // Rotated 180° around pos: the text extends left and up from pos.
                        // So pos = normal_pos + (galley_w, galley_h) to compensate.
                        // Lines should be in reverse order (bottom to top when flipped).
                        let rev_i = galleys.len() - 1 - i;
                        let pos = egui::pos2(
                            bl_rect.left() + padding + galley_w,
                            bl_rect.top() + padding + rev_i as f32 * line_height + galley_h,
                        );
                        let mut shape = egui::epaint::TextShape::new(pos, galley.clone(), egui::Color32::PLACEHOLDER);
                        shape.angle = std::f32::consts::PI;
                        painter.add(shape);
                    }
                }
            }
        }

        // Live text overlay for visible room labels
        if !presentation.show_labels_player { return; }
        for rl in &layout.rooms {
            if *presentation.room_visibility(&rl.room_id) != crate::presentation::Visibility::Visible {
                continue;
            }
            if let Some(room) = dungeon.graph.room_by_id(&rl.room_id) {
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
                    egui::Color32::from_rgb(60, 60, 60),
                );
            }
        }
    });
}

/// Build the initiative display lines for the player view.
/// Returns a list of (text, is_bold) pairs.
///
/// Rules:
/// - If current is an NPC, show "Monsters" as current, then find the next player.
/// - If current is a player and next is an NPC, show: Player (current), Monsters, then next player.
/// - If current is a player and next is also a player, just show current then next.
fn build_initiative_display(
    tracker: &crate::presentation::combat_tracker::CombatTracker,
) -> Vec<(String, bool)> {
    let order = &tracker.initiative_order;
    if order.is_empty() { return Vec::new(); }

    let current = tracker.current_turn.min(order.len() - 1);

    let name_of = |id: &CombatantId| -> Option<String> {
        match id {
            CombatantId::Player(pid) => tracker.players.get(pid).map(|p| p.name.clone()),
            CombatantId::Monster(mid) => tracker.instances.get(mid).map(|m| m.label.clone()),
        }
    };
    let is_player = |id: &CombatantId| -> bool {
        matches!(id, CombatantId::Player(_))
    };

    // Find next player after a given index (wrapping)
    let next_player_after = |start: usize| -> Option<String> {
        for offset in 1..order.len() {
            let idx = (start + offset) % order.len();
            if is_player(&order[idx]) {
                return name_of(&order[idx]);
            }
        }
        None
    };

    let mut lines = Vec::new();

    if is_player(&order[current]) {
        // Current is a player
        let current_name = name_of(&order[current]).unwrap_or_else(|| "?".into());
        lines.push((format!("> {}", current_name), true));

        // Check what's next
        let next_idx = (current + 1) % order.len();
        if is_player(&order[next_idx]) {
            // Next is also a player
            let next_name = name_of(&order[next_idx]).unwrap_or_else(|| "?".into());
            lines.push((format!("  {}", next_name), false));
        } else {
            // Next is monster(s) — show "Monsters" then the next player
            lines.push(("  Monsters".into(), false));
            if let Some(next_pc) = next_player_after(next_idx) {
                lines.push((format!("  {}", next_pc), false));
            }
        }
    } else {
        // Current is a monster
        lines.push(("> Monsters".into(), true));
        if let Some(next_pc) = next_player_after(current) {
            lines.push((format!("  {}", next_pc), false));
        }
    }

    lines
}
