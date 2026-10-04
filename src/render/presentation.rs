use crate::util::CellSet;
use crate::model::*;
use crate::presentation::{PresentationState, PresentationSnapshot, Visibility};
use crate::presentation::fog::corridor_visibility;
use crate::presentation::lighting::compute_brightness_generic;
use crate::render::themed::*;
use crate::render::traits::MapRenderer;
use crate::util::GRID_PX;

/// Truncate a name to at most `max_len` chars, appending "…" if truncated.
fn truncate_name(name: &str, max_len: usize) -> String {
    if name.chars().count() <= max_len {
        name.to_string()
    } else {
        let truncated: String = name.chars().take(max_len).collect();
        format!("{}…", truncated)
    }
}

/// Render the player-facing view from a snapshot (for background threads).
pub fn render_player_view_snapshot(
    renderer: &mut dyn MapRenderer,
    graph: &DungeonGraph,
    layout: &SpatialLayout,
    theme: &Theme,
    snapshot: &PresentationSnapshot,
    light_sources: &[crate::model::LightSource],
    ambient_light: f32,
    options: &RenderOptions,
) {
    let audience = Audience::Players { visibility: snapshot, lights: light_sources, ambient: ambient_light };
    render_map(renderer, graph, layout, theme, options, &audience);
}

/// Darken the floor by the radial light model (the baked player-view lighting).
pub(crate) fn render_lighting_overlay(
    renderer: &mut dyn MapRenderer,
    layout: &SpatialLayout,
    light_sources: &[crate::model::LightSource],
    ambient_light: f32,
    visible_floor: &CellSet,
) {
    for &(fx, fy) in visible_floor {
        let cell_cx = fx as f32 + 0.5;
        let cell_cy = fy as f32 + 0.5;
        let brightness = compute_brightness_generic(cell_cx, cell_cy, light_sources, ambient_light, layout);
        if brightness < 1.0 {
            let alpha = ((1.0 - brightness) * 180.0) as u8;
            let px = fx as f32 * GRID_PX;
            let py = fy as f32 * GRID_PX;
            renderer.fill_rect(px, py, GRID_PX, GRID_PX, [0, 0, 0, alpha]);
        }
    }
}

/// Draw the DM overlay showing visibility state over the full map.
pub fn render_dm_overlay(
    painter: &egui::Painter,
    transform: &crate::util::ViewTransform,
    layout: &SpatialLayout,
    dungeon: &crate::model::Dungeon,
    presentation: &PresentationState,
) {
    let graph = &dungeon.graph;
    // Room visibility overlays
    for rl in &layout.rooms {
        let vis = presentation.room_visibility(&rl.room_id);
        let alpha = match vis {
            Visibility::Hidden => 178,   // 70% opacity
            Visibility::Explored => 76,  // 30% opacity
            Visibility::Visible => 0,
        };

        if alpha > 0 && rl.is_rotated() {
            let pts: Vec<egui::Pos2> = rl.corners().iter()
                .map(|&(x, y)| transform.world_to_screen(egui::pos2(x * GRID_PX, y * GRID_PX)))
                .collect();
            painter.add(egui::Shape::convex_polygon(pts, egui::Color32::from_rgba_unmultiplied(0, 0, 0, alpha), egui::Stroke::NONE));
        }
        let min = transform.world_to_screen(egui::pos2(
            rl.x as f32 * GRID_PX,
            rl.y as f32 * GRID_PX,
        ));
        let max = transform.world_to_screen(egui::pos2(
            (rl.x + rl.width as i32) as f32 * GRID_PX,
            (rl.y + rl.height as i32) as f32 * GRID_PX,
        ));
        if alpha > 0 && !rl.is_rotated() {
            painter.rect_filled(
                egui::Rect::from_min_max(min, max),
                0.0,
                egui::Color32::from_rgba_unmultiplied(0, 0, 0, alpha),
            );
        }

        // Visibility badge at room center
        let (cx, cy) = crate::util::room_center_px(rl);
        let screen = transform.world_to_screen(egui::pos2(cx, cy));
        let badge = match vis {
            Visibility::Hidden => "H",
            Visibility::Explored => "E",
            Visibility::Visible => "V",
        };
        let badge_color = match vis {
            Visibility::Hidden => egui::Color32::from_rgb(255, 100, 100),
            Visibility::Explored => egui::Color32::from_rgb(255, 200, 100),
            Visibility::Visible => egui::Color32::from_rgb(100, 255, 100),
        };
        painter.text(
            screen + egui::vec2(0.0, -8.0 * transform.zoom),
            egui::Align2::CENTER_CENTER,
            badge,
            egui::FontId::monospace(8.0 * transform.zoom),
            badge_color,
        );
    }

    // Corridor visibility overlays
    let shapes = crate::model::geometry::corridor_shapes(layout, graph);
    for (ci, corridor) in layout.corridors.iter().enumerate() {
        let vis = corridor_visibility(&corridor.connection_id, presentation, graph);
        let alpha = match vis {
            Visibility::Hidden => 178,
            Visibility::Explored => 76,
            Visibility::Visible => 0,
        };

        if alpha > 0 {
            if let Some(shape) = &shapes[ci] {
                for piece in shape.floor_pieces() {
                    let pts: Vec<egui::Pos2> = piece.iter()
                        .map(|&(x, y)| transform.world_to_screen(egui::pos2(x * GRID_PX, y * GRID_PX)))
                        .collect();
                    painter.add(egui::Shape::convex_polygon(pts, egui::Color32::from_rgba_unmultiplied(0, 0, 0, alpha), egui::Stroke::NONE));
                }
                continue;
            }
            for (min_gx, min_gy, max_gx, max_gy) in corridor.run_boxes() {

                let min = transform.world_to_screen(egui::pos2(
                    min_gx as f32 * GRID_PX,
                    min_gy as f32 * GRID_PX,
                ));
                let max = transform.world_to_screen(egui::pos2(
                    max_gx as f32 * GRID_PX,
                    max_gy as f32 * GRID_PX,
                ));
                painter.rect_filled(
                    egui::Rect::from_min_max(min, max),
                    0.0,
                    egui::Color32::from_rgba_unmultiplied(0, 0, 0, alpha),
                );
            }
        }
    }

    // Encounter markers (shown at their current runtime positions)
    for rl in &layout.rooms {
        // Find encounters currently in this room
        let enc_ids = presentation.encounter_ids_in_room(&rl.room_id);
        let encounters: Vec<_> = dungeon.encounters.iter()
            .filter(|e| enc_ids.contains(&e.id))
            .collect();
        if encounters.is_empty() { continue; }

        let (cx, cy) = crate::util::room_center_px(rl);
        let screen = transform.world_to_screen(egui::pos2(cx, cy));

        for (j, enc) in encounters.iter().enumerate() {
            let offset_y = (j as f32 - (encounters.len() as f32 - 1.0) / 2.0) * 12.0 * transform.zoom;
            let pos = screen + egui::vec2(0.0, 16.0 * transform.zoom + offset_y);

            let is_defeated = presentation.defeated_encounters.contains(&enc.id);
            let color = if is_defeated {
                egui::Color32::from_rgb(120, 120, 120)
            } else if enc.is_hazard() {
                egui::Color32::from_rgb(160, 80, 255)
            } else {
                match enc.encounter_type {
                    crate::model::EncounterType::Static => egui::Color32::from_rgb(255, 80, 80),
                    crate::model::EncounterType::Wandering(_) => egui::Color32::from_rgb(255, 160, 40),
                }
            };

            let text_size = 8.0 * transform.zoom;
            let display = if is_defeated {
                format!("{} (dead)", truncate_name(&enc.name, 6))
            } else {
                truncate_name(&enc.name, 8)
            };

            let galley = painter.layout_no_wrap(
                display.clone(),
                egui::FontId::monospace(text_size),
                color,
            );
            let pill_size = galley.size() + egui::vec2(6.0, 2.0);
            let pill_rect = egui::Rect::from_center_size(pos, pill_size);
            painter.rect_filled(pill_rect, 3.0, egui::Color32::from_rgba_unmultiplied(0, 0, 0, 180));

            painter.text(
                pos,
                egui::Align2::CENTER_CENTER,
                &display,
                egui::FontId::monospace(text_size),
                color,
            );
        }
    }

    // Party token
    if let Some(party_room_id) = &presentation.party_room {
        if let Some(rl) = layout.room_by_id(party_room_id) {
            let (cx, cy) = crate::util::room_center_px(rl);
            let screen = transform.world_to_screen(egui::pos2(cx, cy));
            // Offset above encounter markers
            let pos = screen + egui::vec2(0.0, -16.0 * transform.zoom);

            let text_size = 8.0 * transform.zoom;
            let display = "Party";
            let color = egui::Color32::from_rgb(80, 160, 255);

            let galley = painter.layout_no_wrap(
                display.to_string(),
                egui::FontId::monospace(text_size),
                color,
            );
            let pill_size = galley.size() + egui::vec2(6.0, 2.0);
            let pill_rect = egui::Rect::from_center_size(pos, pill_size);
            painter.rect_filled(pill_rect, 3.0, egui::Color32::from_rgba_unmultiplied(0, 0, 60, 200));

            painter.text(
                pos,
                egui::Align2::CENTER_CENTER,
                display,
                egui::FontId::monospace(text_size),
                color,
            );
        }
    }

    // Light source indicators
    for light in &dungeon.light_sources {
        let Some(rl) = layout.room_by_id(&light.room_id) else { continue };
        let (cx, cy) = crate::util::room_center_px(rl);
        let screen = transform.world_to_screen(egui::pos2(cx, cy));

        // Draw light radius circle
        let radius_px = light.radius * GRID_PX * transform.zoom;
        painter.circle_stroke(
            screen,
            radius_px,
            egui::Stroke::new(1.0_f32, egui::Color32::from_rgba_unmultiplied(
                light.color[0], light.color[1], light.color[2], 128,
            )),
        );

        // Light source marker
        painter.text(
            screen + egui::vec2(0.0, 8.0 * transform.zoom),
            egui::Align2::CENTER_CENTER,
            "L",
            egui::FontId::monospace(8.0 * transform.zoom),
            egui::Color32::from_rgb(light.color[0], light.color[1], light.color[2]),
        );
    }
}
