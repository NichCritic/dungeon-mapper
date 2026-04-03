use std::collections::HashSet;

use crate::model::{ShadingStyle, SpatialLayout};
use crate::render::traits::MapRenderer;
use crate::util::GRID_PX;

/// Parameters controlling exterior shading appearance.
pub struct ShadingParams {
    pub radius: f32,
    pub style: ShadingStyle,
    pub density: f32,
    pub color: [u8; 4],
}

pub fn draw_exterior_shading(
    renderer: &mut dyn MapRenderer,
    layout: &SpatialLayout,
    floor: &HashSet<(i32, i32)>,
    params: &ShadingParams,
    contour_segments: &[(f32, f32, f32, f32)],
) {
    if floor.is_empty() || params.radius <= 0.0 {
        return;
    }

    let radius_px = params.radius * GRID_PX;

    // Find boundary cells (floor cells with at least one non-floor neighbor)
    let mut boundary_cells: HashSet<(i32, i32)> = HashSet::new();
    for &(fx, fy) in floor {
        for dy in -1..=1 {
            for dx in -1..=1 {
                if !floor.contains(&(fx + dx, fy + dy)) {
                    boundary_cells.insert((fx, fy));
                }
            }
        }
    }

    match params.style {
        ShadingStyle::Hatched => {
            draw_dyson_hatching(renderer, floor, &boundary_cells, radius_px, params.density, params.color, contour_segments);
        }
        ShadingStyle::Solid => {
            let extents = layout.extents();
            let search_r = params.radius.ceil() as i32 + 1;
            let search_extents = (
                extents.0 - search_r,
                extents.1 - search_r,
                extents.2 + search_r,
                extents.3 + search_r,
            );
            draw_solid_shading(renderer, floor, &boundary_cells, search_extents, radius_px, params.color, contour_segments);
        }
        ShadingStyle::Stippled => {
            let extents = layout.extents();
            let search_r = params.radius.ceil() as i32 + 1;
            let search_extents = (
                extents.0 - search_r,
                extents.1 - search_r,
                extents.2 + search_r,
                extents.3 + search_r,
            );
            draw_stippled_shading(renderer, floor, &boundary_cells, search_extents, radius_px, params.density, params.color, contour_segments);
        }
    }
}

/// Simple deterministic hash for pseudo-random values from coordinates.
fn hash_pos(x: f32, y: f32, salt: u32) -> u32 {
    let ix = (x * 100.0) as i32;
    let iy = (y * 100.0) as i32;
    let mut h = (ix as u32).wrapping_mul(2654435761);
    h ^= (iy as u32).wrapping_mul(2246822519);
    h ^= salt.wrapping_mul(3266489917);
    h ^= h >> 16;
    h = h.wrapping_mul(2246822519);
    h ^= h >> 13;
    h
}

fn hash_f32(x: f32, y: f32, salt: u32) -> f32 {
    (hash_pos(x, y, salt) & 0xFFFF) as f32 / 65535.0
}

/// Dyson-style hatching: randomly scattered seeds in the exterior zone,
/// denser near walls, each with a random angle. Parallel lines fill each
/// Voronoi cell, extending in both directions from the seed.
fn draw_dyson_hatching(
    renderer: &mut dyn MapRenderer,
    floor: &HashSet<(i32, i32)>,
    boundary_cells: &HashSet<(i32, i32)>,
    radius_px: f32,
    density: f32,
    color: [u8; 4],
    contour_segments: &[(f32, f32, f32, f32)],
) {
    let base_spacing = (6.0 / density).max(2.0);

    let mut seeds: Vec<(f32, f32, f32)> = Vec::new(); // (x, y, angle)

    let search_r = (radius_px / GRID_PX).ceil() as i32 + 1;
    let mut exterior_cells: Vec<(i32, i32)> = Vec::new();
    for &(bx, by) in boundary_cells {
        for dy in -search_r..=search_r {
            for dx in -search_r..=search_r {
                let gx = bx + dx;
                let gy = by + dy;
                if !floor.contains(&(gx, gy)) {
                    exterior_cells.push((gx, gy));
                }
            }
        }
    }
    exterior_cells.sort();
    exterior_cells.dedup();

    for &(gx, gy) in &exterior_cells {
        let wx = gx as f32 * GRID_PX;
        let wy = gy as f32 * GRID_PX;

        let d = dist_to_floor(wx + GRID_PX / 2.0, wy + GRID_PX / 2.0, boundary_cells, contour_segments);
        if d > radius_px {
            continue;
        }

        let dist_factor = (d / radius_px).max(0.1);
        let local_spacing = base_spacing * (0.5 + dist_factor * 1.5);

        let mut sy = wy + local_spacing * hash_f32(wx, wy, 1) * 0.5;
        while sy < wy + GRID_PX {
            let mut sx = wx + local_spacing * hash_f32(wx, sy, 2) * 0.5;
            while sx < wx + GRID_PX {
                let jx = sx + (hash_f32(sx, sy, 3) - 0.5) * local_spacing * 0.6;
                let jy = sy + (hash_f32(sx, sy, 4) - 0.5) * local_spacing * 0.6;

                let jgx = (jx / GRID_PX).floor() as i32;
                let jgy = (jy / GRID_PX).floor() as i32;
                if !floor.contains(&(jgx, jgy)) {
                    let jd = dist_to_floor(jx, jy, boundary_cells, contour_segments);
                    if jd <= radius_px {
                        let angle = hash_f32(jx, jy, 7) * std::f32::consts::PI;
                        seeds.push((jx, jy, angle));
                    }
                }
                sx += local_spacing;
            }
            sy += local_spacing;
        }
    }

    if seeds.is_empty() {
        return;
    }

    let nearest_seed = |px: f32, py: f32| -> usize {
        let mut best = 0;
        let mut best_d = f32::MAX;
        for (i, &(sx, sy, _)) in seeds.iter().enumerate() {
            let d = (px - sx).powi(2) + (py - sy).powi(2);
            if d < best_d {
                best_d = d;
                best = i;
            }
        }
        best
    };

    let line_spacing = (2.5 / density).max(1.0);
    let line_weight = 0.8;
    let step = 1.5;

    for (seed_idx, &(sx, sy, angle)) in seeds.iter().enumerate() {
        let line_dx = angle.cos();
        let line_dy = angle.sin();
        let perp_dx = -line_dy;
        let perp_dy = line_dx;

        let mut min_neighbor_dist = radius_px * 2.0;
        for (j, &(ox, oy, _)) in seeds.iter().enumerate() {
            if j == seed_idx { continue; }
            let d = ((ox - sx).powi(2) + (oy - sy).powi(2)).sqrt();
            if d < min_neighbor_dist {
                min_neighbor_dist = d;
            }
        }
        let cell_half_width = (min_neighbor_dist / 2.0).min(base_spacing);
        let num_lines = (cell_half_width * 2.0 / line_spacing).ceil() as i32;

        for i in -num_lines / 2..=num_lines / 2 {
            let offset = i as f32 * line_spacing;
            let lx = sx + perp_dx * offset;
            let ly = sy + perp_dy * offset;

            if nearest_seed(lx, ly) != seed_idx {
                continue;
            }

            let mut neg_t = 0.0_f32;
            let mut pos_t = 0.0_f32;

            let mut t = step;
            loop {
                let px = lx + line_dx * t;
                let py = ly + line_dy * t;
                let pgx = (px / GRID_PX).floor() as i32;
                let pgy = (py / GRID_PX).floor() as i32;
                if floor.contains(&(pgx, pgy)) { break; }
                if dist_to_floor(px, py, boundary_cells, contour_segments) > radius_px { break; }
                if nearest_seed(px, py) != seed_idx { break; }
                pos_t = t;
                t += step;
                if t > radius_px * 2.0 { break; }
            }

            t = -step;
            loop {
                let px = lx + line_dx * t;
                let py = ly + line_dy * t;
                let pgx = (px / GRID_PX).floor() as i32;
                let pgy = (py / GRID_PX).floor() as i32;
                if floor.contains(&(pgx, pgy)) { break; }
                if dist_to_floor(px, py, boundary_cells, contour_segments) > radius_px { break; }
                if nearest_seed(px, py) != seed_idx { break; }
                neg_t = t;
                t -= step;
                if t < -radius_px * 2.0 { break; }
            }

            if pos_t - neg_t < step {
                continue;
            }

            let x1 = lx + line_dx * neg_t;
            let y1 = ly + line_dy * neg_t;
            let x2 = lx + line_dx * pos_t;
            let y2 = ly + line_dy * pos_t;

            renderer.draw_line(x1, y1, x2, y2, line_weight, color);
        }
    }
}

fn dist_to_floor(wx: f32, wy: f32, boundary_cells: &HashSet<(i32, i32)>, contour_segments: &[(f32, f32, f32, f32)]) -> f32 {
    let gx = (wx / GRID_PX).floor() as i32;
    let gy = (wy / GRID_PX).floor() as i32;
    let mut min_dist_sq = f32::MAX;
    for dy in -2..=2 {
        for dx in -2..=2 {
            let cx = gx + dx;
            let cy = gy + dy;
            if !boundary_cells.contains(&(cx, cy)) { continue; }
            let cell_x1 = cx as f32 * GRID_PX;
            let cell_y1 = cy as f32 * GRID_PX;
            let nearest_x = wx.clamp(cell_x1, cell_x1 + GRID_PX);
            let nearest_y = wy.clamp(cell_y1, cell_y1 + GRID_PX);
            let d = (wx - nearest_x).powi(2) + (wy - nearest_y).powi(2);
            min_dist_sq = min_dist_sq.min(d);
        }
    }

    // Also check distance to marching squares contour segments (for smooth cave boundaries)
    for &(x1, y1, x2, y2) in contour_segments {
        let d = point_to_segment_dist_sq(wx, wy, x1, y1, x2, y2);
        min_dist_sq = min_dist_sq.min(d);
    }

    min_dist_sq.sqrt()
}

/// Squared distance from point (px, py) to line segment (x1,y1)-(x2,y2).
fn point_to_segment_dist_sq(px: f32, py: f32, x1: f32, y1: f32, x2: f32, y2: f32) -> f32 {
    let dx = x2 - x1;
    let dy = y2 - y1;
    let len_sq = dx * dx + dy * dy;
    if len_sq < 0.001 {
        return (px - x1).powi(2) + (py - y1).powi(2);
    }
    let t = ((px - x1) * dx + (py - y1) * dy) / len_sq;
    let t = t.clamp(0.0, 1.0);
    let proj_x = x1 + t * dx;
    let proj_y = y1 + t * dy;
    (px - proj_x).powi(2) + (py - proj_y).powi(2)
}

fn draw_solid_shading(
    renderer: &mut dyn MapRenderer,
    floor: &HashSet<(i32, i32)>,
    boundary_cells: &HashSet<(i32, i32)>,
    extents: (i32, i32, i32, i32),
    radius_px: f32,
    color: [u8; 4],
    contour_segments: &[(f32, f32, f32, f32)],
) {
    let mut shade_color = color;
    shade_color[3] = (color[3] as f32 * 0.3) as u8;

    let (min_x, min_y, max_x, max_y) = extents;
    for gy in min_y..max_y {
        for gx in min_x..max_x {
            if floor.contains(&(gx, gy)) { continue; }
            let wx = gx as f32 * GRID_PX + GRID_PX / 2.0;
            let wy = gy as f32 * GRID_PX + GRID_PX / 2.0;
            let d = dist_to_floor(wx, wy, boundary_cells, contour_segments);
            if d < radius_px {
                let alpha = 1.0 - (d / radius_px);
                let mut c = shade_color;
                c[3] = (c[3] as f32 * alpha) as u8;
                renderer.fill_rect(gx as f32 * GRID_PX, gy as f32 * GRID_PX, GRID_PX, GRID_PX, c);
            }
        }
    }
}

fn draw_stippled_shading(
    renderer: &mut dyn MapRenderer,
    floor: &HashSet<(i32, i32)>,
    boundary_cells: &HashSet<(i32, i32)>,
    extents: (i32, i32, i32, i32),
    radius_px: f32,
    density: f32,
    color: [u8; 4],
    contour_segments: &[(f32, f32, f32, f32)],
) {
    let dot_interval = (4.0 / density).max(1.5);

    let (min_x, min_y, max_x, max_y) = extents;
    for gy in min_y..max_y {
        for gx in min_x..max_x {
            if floor.contains(&(gx, gy)) { continue; }
            let wx = gx as f32 * GRID_PX;
            let wy = gy as f32 * GRID_PX;
            let d = dist_to_floor(wx + GRID_PX / 2.0, wy + GRID_PX / 2.0, boundary_cells, contour_segments);
            if d >= radius_px { continue; }

            let mut dy = 1.0;
            while dy < GRID_PX {
                let row_offset = if ((dy / dot_interval) as i32) % 2 == 0 { 0.0 } else { dot_interval / 2.0 };
                let mut dx = 1.0 + row_offset;
                while dx < GRID_PX {
                    let px = wx + dx;
                    let py = wy + dy;
                    let pd = dist_to_floor(px, py, boundary_cells, contour_segments);
                    if pd < radius_px {
                        let pgx = (px / GRID_PX).floor() as i32;
                        let pgy = (py / GRID_PX).floor() as i32;
                        if !floor.contains(&(pgx, pgy)) {
                            let alpha = 1.0 - (pd / radius_px);
                            let dot_size = 0.5 + alpha;
                            renderer.fill_rect(px - dot_size / 2.0, py - dot_size / 2.0, dot_size, dot_size, color);
                        }
                    }
                    dx += dot_interval;
                }
                dy += dot_interval;
            }
        }
    }
}
