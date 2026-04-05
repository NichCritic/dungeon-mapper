use crate::model::*;
use crate::presentation::PresentationState;
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
}

impl Default for PlayerViewState {
    fn default() -> Self {
        Self {
            view: ViewState::default(),
            render_cache: BackgroundRenderCache::default(),
            canvas_size: egui::Vec2::ZERO,
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
    egui::CentralPanel::default().show(ctx, |ui| {
        let (response, painter) = ui.allocate_painter(
            ui.available_size(),
            egui::Sense::click_and_drag(),
        );
        let rect = response.rect;

        let bg = dungeon.theme.bg_color;
        painter.rect_filled(rect, 0.0, egui::Color32::from_rgba_unmultiplied(bg[0], bg[1], bg[2], bg[3]));

        handle_pan_zoom(&response, &mut state.view);
        state.canvas_size = rect.size();
        let transform = ViewTransform::new(state.view.offset, state.view.zoom, rect);

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
