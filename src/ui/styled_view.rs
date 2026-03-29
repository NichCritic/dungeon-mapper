use std::hash::{Hash, Hasher};

use crate::model::*;
use crate::render::recording::{RecordingRenderer, RenderCommand, replay_commands};
use crate::render::themed::RenderOptions;
use crate::ui::canvas_common::{handle_pan_zoom, ViewState, COLOR_PLACEHOLDER_TEXT};
use crate::util::{ViewTransform, GRID_PX};

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
    render_cache: Option<RenderCache>,
}

impl Default for StyledViewState {
    fn default() -> Self {
        Self {
            view: ViewState::default(),
            show_grid: true,
            show_labels: true,
            show_notes: true,
            show_secrets: true,
            render_cache: None,
        }
    }
}

/// Compute a hash over all inputs that affect the cached render commands.
/// Text (labels, notes, secret "S" markers) is drawn as a live overlay
/// so show_labels/show_notes don't trigger a cache rebuild.
fn render_input_hash(layout: &SpatialLayout, theme: &Theme, show_grid: bool) -> u64 {
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
    (theme.exterior_shading as u8).hash(&mut h);
    theme.shading_radius.to_bits().hash(&mut h);
    (theme.shading_style as u8).hash(&mut h);
    theme.hatching_density.to_bits().hash(&mut h);
    show_grid.hash(&mut h);
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
        // Rebuild cached render commands if inputs changed
        let hash = render_input_hash(layout, &dungeon.theme, state.show_grid);
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
                layout,
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

        // Live text overlay (labels, notes, secret door markers)
        draw_text_overlay(&painter, &transform, &dungeon.graph, layout, &dungeon.theme, state);
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

                painter.text(
                    screen,
                    egui::Align2::CENTER_CENTER,
                    &room.label,
                    egui::FontId::monospace(10.0 * transform.zoom),
                    egui::Color32::from_rgb(60, 60, 60),
                );

                if state.show_notes && !room.notes.is_empty() {
                    let notes_screen = transform.world_to_screen(egui::pos2(cx, cy + 14.0));
                    painter.text(
                        notes_screen,
                        egui::Align2::CENTER_CENTER,
                        &room.notes,
                        egui::FontId::monospace(7.0 * transform.zoom),
                        egui::Color32::from_rgb(120, 120, 120),
                    );
                }
            }
        }
    }
}

pub fn styled_sidebar(ui: &mut egui::Ui, dungeon: &mut Dungeon, state: &mut StyledViewState) {
    ui.heading("Styled View");
    ui.separator();

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

    ui.add_space(16.0);
    ui.heading("Export");
    ui.separator();

    if ui.button("Export DM Map (PNG)").clicked() {
        export_png(dungeon, true);
    }
    if ui.button("Export Player Map (PNG)").clicked() {
        export_png(dungeon, false);
    }
}

fn export_png(dungeon: &Dungeon, dm_mode: bool) {
    if dungeon.layout.is_none() {
        return;
    }

    let path = rfd::FileDialog::new()
        .set_title(if dm_mode { "Export DM Map" } else { "Export Player Map" })
        .add_filter("PNG Image", &["png"])
        .save_file();

    if let Some(path) = path {
        if let Err(e) = crate::io::export::export_png(dungeon, &path, dm_mode, 2) {
            eprintln!("Export error: {}", e);
        }
    }
}
