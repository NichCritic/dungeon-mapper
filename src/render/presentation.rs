use std::collections::HashSet;

use crate::model::*;
use crate::presentation::{PresentationState, Visibility};
use crate::presentation::fog::corridor_visibility;
use crate::presentation::lighting::compute_brightness;
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

/// Blend a color toward bg_color at a given ratio (0.0 = original, 1.0 = fully bg).
fn blend_toward(color: [u8; 4], bg: [u8; 4], ratio: f32) -> [u8; 4] {
    [
        (color[0] as f32 + (bg[0] as f32 - color[0] as f32) * ratio) as u8,
        (color[1] as f32 + (bg[1] as f32 - color[1] as f32) * ratio) as u8,
        (color[2] as f32 + (bg[2] as f32 - color[2] as f32) * ratio) as u8,
        color[3],
    ]
}

/// Build a floor set from only visible/explored rooms and their corridors.
fn build_visible_floor_set(
    layout: &SpatialLayout,
    graph: &DungeonGraph,
    presentation: &PresentationState,
) -> HashSet<(i32, i32)> {
    let mut floor: HashSet<(i32, i32)> = HashSet::new();

    for rl in &layout.rooms {
        let vis = presentation.room_visibility(&rl.room_id);
        if *vis == Visibility::Hidden { continue; }
        let room = graph.room_by_id(&rl.room_id);
        let shape = room.map(|r| r.shape).unwrap_or_default();
        match shape {
            RoomShape::Circle => {
                let cx = rl.x as f32 + rl.width as f32 / 2.0;
                let cy = rl.y as f32 + rl.height as f32 / 2.0;
                let r = (rl.width.min(rl.height) as f32) / 2.0;
                for y in rl.y..(rl.y + rl.height as i32) {
                    for x in rl.x..(rl.x + rl.width as i32) {
                        let cell_cx = x as f32 + 0.5;
                        let cell_cy = y as f32 + 0.5;
                        let dx = cell_cx - cx;
                        let dy = cell_cy - cy;
                        if dx * dx + dy * dy <= r * r {
                            floor.insert((x, y));
                        }
                    }
                }
            }
            RoomShape::Cave => {
                if let Some(cave) = room.and_then(|r| r.cave_data.as_ref()) {
                    if !cave.cells.is_empty() {
                        let w = rl.width as usize;
                        for ly in 0..rl.height as usize {
                            for lx in 0..w {
                                if cave.cells.get(ly * w + lx).copied().unwrap_or(false) {
                                    floor.insert((rl.x + lx as i32, rl.y + ly as i32));
                                }
                            }
                        }
                    } else {
                        for y in rl.y..(rl.y + rl.height as i32) {
                            for x in rl.x..(rl.x + rl.width as i32) {
                                floor.insert((x, y));
                            }
                        }
                    }
                }
            }
            RoomShape::Rectangle => {
                for y in rl.y..(rl.y + rl.height as i32) {
                    for x in rl.x..(rl.x + rl.width as i32) {
                        floor.insert((x, y));
                    }
                }
            }
        }
    }

    for corridor in &layout.corridors {
        let vis = corridor_visibility(&corridor.connection_id, presentation, graph);
        if vis == Visibility::Hidden { continue; }
        let cw = corridor.width as i32;
        let half = cw / 2;
        for pair in corridor.waypoints.windows(2) {
            let min_x = pair[0].x.min(pair[1].x) - half;
            let min_y = pair[0].y.min(pair[1].y) - half;
            let max_x = pair[0].x.max(pair[1].x) - half + cw;
            let max_y = pair[0].y.max(pair[1].y) - half + cw;
            for y in min_y..max_y {
                for x in min_x..max_x {
                    floor.insert((x, y));
                }
            }
        }
    }

    floor
}

/// Render the player-facing view that respects visibility state.
pub fn render_player_view(
    renderer: &mut dyn MapRenderer,
    graph: &DungeonGraph,
    layout: &SpatialLayout,
    theme: &Theme,
    presentation: &PresentationState,
    options: &RenderOptions,
) {
    let dim_ratio = 0.4;
    let dimmed_floor = blend_toward(theme.floor_color, theme.bg_color, dim_ratio);
    let dimmed_wall = blend_toward(theme.wall_color, theme.bg_color, dim_ratio);

    // Background (always)
    render_background(renderer, layout, theme);

    // Exterior shading (build floor set from visible rooms only)
    let visible_floor = build_visible_floor_set(layout, graph, presentation);
    if theme.exterior_shading {
        use crate::render::hatching::{draw_exterior_shading, ShadingParams};
        let params = ShadingParams {
            radius: theme.shading_radius,
            style: theme.shading_style,
            density: theme.hatching_density,
            color: theme.wall_color,
        };
        let contour: Vec<(f32, f32, f32, f32)> = graph.rooms.iter()
            .filter_map(|r| r.cave_data.as_ref())
            .flat_map(|c| c.contour_segments.iter().copied())
            .collect();
        draw_exterior_shading(renderer, layout, &visible_floor, &params, &contour);
    }

    // Room floors
    for rl in &layout.rooms {
        let vis = presentation.room_visibility(&rl.room_id);
        match vis {
            Visibility::Hidden => continue,
            Visibility::Explored => {
                render_room_floor_with_color(renderer, rl, graph, dimmed_floor);
            }
            Visibility::Visible => {
                render_room_floor(renderer, rl, graph, theme);
            }
        }
    }

    // Corridor floors
    for corridor in &layout.corridors {
        let vis = corridor_visibility(&corridor.connection_id, presentation, graph);
        match vis {
            Visibility::Hidden => continue,
            Visibility::Explored => {
                render_corridor_floor_with_color(renderer, corridor, dimmed_floor);
            }
            Visibility::Visible => {
                render_corridor_floor(renderer, corridor, theme);
            }
        }
    }

    // Corridor chamfers
    if theme.corridor_chamfer != ChamferStyle::Sharp {
        for corridor in &layout.corridors {
            let vis = corridor_visibility(&corridor.connection_id, presentation, graph);
            if vis == Visibility::Hidden { continue; }
            render_corridor_chamfers(renderer, corridor, theme);
        }
    }

    // Grid lines (only over visible/explored floor)
    if options.show_grid {
        render_grid(renderer, &visible_floor);
    }

    // Room decor (visible rooms only)
    for rl in &layout.rooms {
        let vis = presentation.room_visibility(&rl.room_id);
        if *vis != Visibility::Visible { continue; }
        render_decor(renderer, rl, graph, theme);
    }

    // Room walls
    for rl in &layout.rooms {
        let vis = presentation.room_visibility(&rl.room_id);
        if *vis == Visibility::Hidden { continue; }
        let wall_color = if *vis == Visibility::Explored { dimmed_wall } else { theme.wall_color };
        // Cave rooms use baked contour segments
        let room = graph.room_by_id(&rl.room_id);
        if let Some(cave) = room.and_then(|r| {
            if r.shape == RoomShape::Cave { r.cave_data.as_ref() } else { None }
        }) {
            if !cave.contour_segments.is_empty() {
                for &(x1, y1, x2, y2) in &cave.contour_segments {
                    renderer.draw_line(x1, y1, x2, y2, 2.0, wall_color);
                }
                continue;
            }
        }
        if *vis == Visibility::Explored {
            let dimmed_theme = Theme { wall_color: dimmed_wall, ..theme.clone() };
            render_room_walls(renderer, rl, graph, &dimmed_theme);
        } else {
            render_room_walls(renderer, rl, graph, theme);
        }
    }
    repair_circle_junctions(renderer, graph, layout, theme);

    // Corridor walls
    let cave_cells = build_cave_cell_set(layout, graph);
    for corridor in &layout.corridors {
        let vis = corridor_visibility(&corridor.connection_id, presentation, graph);
        if vis == Visibility::Hidden { continue; }
        if vis == Visibility::Explored {
            let dimmed_theme = Theme {
                wall_color: dimmed_wall,
                ..theme.clone()
            };
            render_corridor_walls(renderer, corridor, &visible_floor, &dimmed_theme, &cave_cells);
        } else {
            render_corridor_walls(renderer, corridor, &visible_floor, theme, &cave_cells);
        }
    }

    // Doors (only on visible/explored corridors)
    render_doors_filtered(renderer, graph, layout, theme, presentation);

    // Labels only on visible rooms
    if options.show_labels {
        for rl in &layout.rooms {
            if *presentation.room_visibility(&rl.room_id) != Visibility::Visible {
                continue;
            }
            if let Some(room) = graph.room_by_id(&rl.room_id) {
                let cx = (rl.x as f32 + rl.width as f32 / 2.0) * GRID_PX;
                let cy = (rl.y as f32 + rl.height as f32 / 2.0) * GRID_PX;
                renderer.draw_text(&room.label, cx, cy, 10.0, [60, 60, 60, 255]);
            }
        }
    }

    // Lighting brightness overlay
    if !presentation.light_sources.is_empty() {
        render_lighting_overlay(renderer, layout, presentation, &visible_floor);
    }
}

/// Render doors when at least one endpoint room is not hidden (unless secret).
fn render_doors_filtered(
    renderer: &mut dyn MapRenderer,
    graph: &DungeonGraph,
    layout: &SpatialLayout,
    theme: &Theme,
    presentation: &PresentationState,
) {
    for edge in &graph.connections {
        // Never show secrets to players
        if edge.connection.connection_type == ConnectionType::Secret {
            continue;
        }
        if edge.connection.connection_type == ConnectionType::Open {
            continue;
        }
        // Show door if either endpoint room is visible or explored
        let src_vis = presentation.room_visibility(&edge.source_room_id);
        let tgt_vis = presentation.room_visibility(&edge.target_room_id);
        let any_room_shown = *src_vis != Visibility::Hidden || *tgt_vis != Visibility::Hidden;
        if !any_room_shown { continue; }

        let corridor = layout.corridors.iter().find(|c| c.connection_id == edge.connection.id);
        let Some(corridor) = corridor else { continue };
        if corridor.waypoints.len() < 2 { continue; }

        let dw = edge.connection.door_width() as f32;
        let door_depth = 0.3;

        let room_ids = [&edge.source_room_id, &edge.target_room_id];
        let wp_ends = [&corridor.waypoints[0], corridor.waypoints.last().unwrap()];

        let corr_vis = corridor_visibility(&edge.connection.id, presentation, graph);

        for (room_id, wp) in room_ids.iter().zip(wp_ends.iter()) {
            // Skip drawing door on cave room walls
            let is_cave = graph.room_by_id(room_id)
                .is_some_and(|r| r.shape == RoomShape::Cave);
            if is_cave { continue; }
            // Draw door on this room's wall if:
            // - the room itself is shown, OR
            // - the corridor is visible (door open + other room shown),
            //   so the player can see down the hall to this door
            let room_shown = *presentation.room_visibility(room_id) != Visibility::Hidden;
            let corridor_shown = corr_vis != Visibility::Hidden;
            if !room_shown && !corridor_shown { continue; }
            let Some(rl) = layout.room_by_id(room_id) else { continue };

            let wp_cx = wp.x as f32;
            let wp_cy = wp.y as f32;
            let dist_right = (wp_cx - (rl.x + rl.width as i32) as f32).abs();
            let dist_left = (wp_cx - rl.x as f32).abs();
            let dist_bottom = (wp_cy - (rl.y + rl.height as i32) as f32).abs();
            let dist_top = (wp_cy - rl.y as f32).abs();
            let min_dist = dist_right.min(dist_left).min(dist_bottom).min(dist_top);
            let dw_half = dw / 2.0;

            let (dx1, dy1, dx2, dy2) = if min_dist == dist_right {
                let wall_x = (rl.x + rl.width as i32) as f32;
                (wall_x - door_depth / 2.0, wp_cy - dw_half, wall_x + door_depth / 2.0, wp_cy + dw_half)
            } else if min_dist == dist_left {
                let wall_x = rl.x as f32;
                (wall_x - door_depth / 2.0, wp_cy - dw_half, wall_x + door_depth / 2.0, wp_cy + dw_half)
            } else if min_dist == dist_bottom {
                let wall_y = (rl.y + rl.height as i32) as f32;
                (wp_cx - dw_half, wall_y - door_depth / 2.0, wp_cx + dw_half, wall_y + door_depth / 2.0)
            } else {
                let wall_y = rl.y as f32;
                (wp_cx - dw_half, wall_y - door_depth / 2.0, wp_cx + dw_half, wall_y + door_depth / 2.0)
            };

            let px = dx1 * GRID_PX;
            let py = dy1 * GRID_PX;
            let pw = (dx2 - dx1) * GRID_PX;
            let ph = (dy2 - dy1) * GRID_PX;

            match edge.connection.connection_type {
                ConnectionType::Open | ConnectionType::Secret => {}
                ConnectionType::Door | ConnectionType::OneWay => {
                    renderer.fill_rect(px, py, pw, ph, [255, 255, 255, 255]);
                    renderer.stroke_rect(px, py, pw, ph, 1.0, theme.wall_color);
                }
                ConnectionType::Locked => {
                    renderer.fill_rect(px, py, pw, ph, [255, 255, 255, 255]);
                    renderer.stroke_rect(px, py, pw, ph, 1.0, theme.wall_color);
                    let cx = px + pw / 2.0;
                    let cy = py + ph / 2.0;
                    let r = pw.min(ph) * 0.15;
                    renderer.fill_rect(cx - r, cy - r, r * 2.0, r * 2.0, theme.wall_color);
                }
            }
        }
    }
}

/// Apply lighting brightness overlay on visible floor cells.
fn render_lighting_overlay(
    renderer: &mut dyn MapRenderer,
    layout: &SpatialLayout,
    presentation: &PresentationState,
    visible_floor: &HashSet<(i32, i32)>,
) {
    for &(fx, fy) in visible_floor {
        let cell_cx = fx as f32 + 0.5;
        let cell_cy = fy as f32 + 0.5;
        let brightness = compute_brightness(cell_cx, cell_cy, presentation, layout);
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
    // Room overlays (derived visibility)
    for rl in &layout.rooms {
        let vis = presentation.room_visibility(&rl.room_id);
        let alpha = match vis {
            Visibility::Hidden => 178,   // 70% opacity
            Visibility::Explored => 76,  // 30% opacity
            Visibility::Visible => 0,
        };

        if alpha > 0 {
            let min = transform.world_to_screen(egui::pos2(
                rl.x as f32 * GRID_PX,
                rl.y as f32 * GRID_PX,
            ));
            let max = transform.world_to_screen(egui::pos2(
                (rl.x + rl.width as i32) as f32 * GRID_PX,
                (rl.y + rl.height as i32) as f32 * GRID_PX,
            ));
            painter.rect_filled(
                egui::Rect::from_min_max(min, max),
                0.0,
                egui::Color32::from_rgba_unmultiplied(0, 0, 0, alpha),
            );
        }

        // Visibility badge at room center
        let cx = (rl.x as f32 + rl.width as f32 / 2.0) * GRID_PX;
        let cy = (rl.y as f32 + rl.height as f32 / 2.0) * GRID_PX;
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

    // Corridor overlays (direct doorway visibility)
    for corridor in &layout.corridors {
        let vis = corridor_visibility(&corridor.connection_id, presentation, graph);
        let alpha = match vis {
            Visibility::Hidden => 178,
            Visibility::Explored => 76,
            Visibility::Visible => 0,
        };

        if alpha > 0 {
            let cw = corridor.width as i32;
            let half = cw / 2;
            for pair in corridor.waypoints.windows(2) {
                let min_gx = pair[0].x.min(pair[1].x) - half;
                let min_gy = pair[0].y.min(pair[1].y) - half;
                let max_gx = pair[0].x.max(pair[1].x) - half + cw;
                let max_gy = pair[0].y.max(pair[1].y) - half + cw;

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

        let cx = (rl.x as f32 + rl.width as f32 / 2.0) * GRID_PX;
        let cy = (rl.y as f32 + rl.height as f32 / 2.0) * GRID_PX;
        let screen = transform.world_to_screen(egui::pos2(cx, cy));

        for (j, enc) in encounters.iter().enumerate() {
            let offset_y = (j as f32 - (encounters.len() as f32 - 1.0) / 2.0) * 12.0 * transform.zoom;
            let pos = screen + egui::vec2(0.0, 16.0 * transform.zoom + offset_y);

            let (marker, color) = match enc.encounter_type {
                crate::model::EncounterType::Static => {
                    ("S", egui::Color32::from_rgb(255, 80, 80))
                }
                crate::model::EncounterType::Wandering(_) => {
                    ("W", egui::Color32::from_rgb(255, 160, 40))
                }
            };

            let text_size = 8.0 * transform.zoom;
            let display = format!("{} {}", marker, truncate_name(&enc.name, 6));

            // Background pill sized to text
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

    // Light source indicators
    for light in &presentation.light_sources {
        let Some(rl) = layout.room_by_id(&light.room_id) else { continue };
        let cx = (rl.x as f32 + rl.width as f32 / 2.0) * GRID_PX;
        let cy = (rl.y as f32 + rl.height as f32 / 2.0) * GRID_PX;
        let screen = transform.world_to_screen(egui::pos2(cx, cy));

        // Draw light radius circle
        let radius_px = light.radius * GRID_PX * transform.zoom;
        painter.circle_stroke(
            screen,
            radius_px,
            egui::Stroke::new(1.0, egui::Color32::from_rgba_unmultiplied(
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
