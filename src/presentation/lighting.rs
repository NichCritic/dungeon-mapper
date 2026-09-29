//! Lighting and party vision.
//!
//! `compute_brightness_generic` is the legacy radial model (no walls), still used by
//! the awareness check and the PNG player render. `LightMap` is the per-cell model
//! with line of sight: lights and PC senses cast through door/wall geometry, and
//! `party_visible` is what at least one party member can currently see.


use crate::model::{Dungeon, LightSource, SpatialLayout, TokenKind};
use crate::render::themed::build_floor_set;
use crate::util::{CellMap, CellSet, ViewTransform, GRID_PX};
use super::los::{self, Occluders, Pt};
use super::{PresentationSnapshot, PresentationState, Visibility, VisibilityProvider};

/// Compute brightness for a grid cell from light sources and ambient light.
/// Returns a value in 0.0..=1.0.
pub fn compute_brightness_generic(
    cell_x: f32,
    cell_y: f32,
    light_sources: &[LightSource],
    ambient_light: f32,
    layout: &SpatialLayout,
) -> f32 {
    let mut brightness = ambient_light;

    for light in light_sources {
        let (light_cx, light_cy) = match light.pos {
            Some(p) => p,
            None => {
                let Some(light_rl) = layout.room_by_id(&light.room_id) else { continue };
                (light_rl.x as f32 + light_rl.width as f32 / 2.0, light_rl.y as f32 + light_rl.height as f32 / 2.0)
            }
        };

        let dx = cell_x - light_cx;
        let dy = cell_y - light_cy;
        let dist = (dx * dx + dy * dy).sqrt();

        if dist < light.radius {
            let falloff = 1.0 - dist / light.radius;
            brightness += light.intensity * falloff * falloff;
        }
    }

    brightness.min(1.0)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum CellLight {
    Dark,
    Dim,
    Bright,
}

/// A light's position and radii, for drawing rings.
#[derive(Clone, Debug)]
pub struct LightRing {
    pub center: Pt,
    pub bright: f32,
    pub dim: f32,
    pub color: [u8; 3],
}

/// A PC token's darkvision / blind sense, for drawing rings.
#[derive(Clone, Debug)]
pub struct VisionRing {
    pub center: Pt,
    pub darkvision: f32,
    pub blind_sense: f32,
}

/// Per-cell light with line of sight, plus what the party can see.
#[derive(Clone, Debug)]
pub struct LightMap {
    pub key: u64,
    pub floor: CellSet,
    /// Non-floor cells touching the floor (the drawn walls/hatching). Shaded like the
    /// brightest floor cell they touch, so lit rooms show their walls.
    pub band: CellSet,
    /// Lit cells; absent = Dark.
    pub cells: CellMap<CellLight>,
    /// Cells at least one party member can see (lit, or within a sense range, with LOS).
    /// With no PC tokens on the map this falls back to fog visibility.
    pub party_visible: CellSet,
    pub lights: Vec<LightRing>,
    pub vision: Vec<VisionRing>,
    /// Shading to draw, merged into horizontal runs of equal darkness so the overlay is
    /// a few hundred rects rather than one per cell.
    pub shade_runs: Vec<ShadeRun>,
}

/// Cells `x0..x1` of row `y` share one shade alpha (before the view's scale).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShadeRun {
    pub y: i32,
    pub x0: i32,
    pub x1: i32,
    pub alpha: f32,
}

impl LightMap {
    pub fn level(&self, cell: (i32, i32)) -> CellLight {
        self.cells.get(&cell).copied().unwrap_or(CellLight::Dark)
    }

    /// Shade alpha for a cell: near-black where the party can't see, grey when dim.
    fn shade_alpha(&self, c: (i32, i32)) -> Option<f32> {
        if !self.party_visible.contains(&c) {
            return Some(235.0);
        }
        match self.level(c) {
            CellLight::Bright => None,
            CellLight::Dim => Some(110.0),
            CellLight::Dark => Some(170.0),
        }
    }

    fn compute_shade_runs(&self) -> Vec<ShadeRun> {
        let mut shaded: Vec<((i32, i32), f32)> = self.floor.iter().chain(self.band.iter())
            .filter_map(|&c| self.shade_alpha(c).map(|a| (c, a)))
            .collect();
        shaded.sort_by_key(|&((x, y), _)| (y, x));
        let mut runs: Vec<ShadeRun> = Vec::new();
        for ((x, y), alpha) in shaded {
            match runs.last_mut() {
                Some(r) if r.y == y && r.x1 == x && r.alpha == alpha => r.x1 += 1,
                _ => runs.push(ShadeRun { y, x0: x, x1: x + 1, alpha }),
            }
        }
        runs
    }
}

/// Origin of a light: carried token, explicit position, or the room center.
pub fn light_origin(light: &LightSource, dungeon: &Dungeon, layout: &SpatialLayout) -> Option<Pt> {
    if let Some(kind) = &light.carrier {
        if let Some(tok) = dungeon.tokens.iter().find(|t| t.kind == *kind) {
            return Some((tok.x, tok.y));
        }
    }
    if let Some(p) = light.pos {
        return Some(p);
    }
    let rl = layout.room_by_id(&light.room_id)?;
    Some((rl.x as f32 + rl.width as f32 / 2.0, rl.y as f32 + rl.height as f32 / 2.0))
}

/// Fingerprint of everything the light map depends on.
pub fn light_map_key(dungeon: &Dungeon, presentation: &PresentationState, occ: &Occluders) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    los::occluder_hash_static(occ).hash(&mut h);
    dungeon.ambient_light.to_bits().hash(&mut h);
    for l in &dungeon.light_sources {
        l.id.hash(&mut h);
        l.room_id.hash(&mut h);
        l.radius.to_bits().hash(&mut h);
        l.dim_radius().to_bits().hash(&mut h);
        if let Some((x, y)) = l.pos { x.to_bits().hash(&mut h); y.to_bits().hash(&mut h); }
        l.carrier.hash(&mut h);
    }
    for t in &dungeon.tokens {
        if let TokenKind::Player(pid) = &t.kind {
            pid.hash(&mut h);
            t.x.to_bits().hash(&mut h);
            t.y.to_bits().hash(&mut h);
            if let Some(pc) = dungeon.party.iter().find(|p| p.id == *pid) {
                pc.senses.darkvision_ft.hash(&mut h);
                pc.senses.blindsight_ft.hash(&mut h);
                pc.senses.tremorsense_ft.hash(&mut h);
            }
        }
    }
    let mut vis: Vec<(&String, u8)> = presentation.room_visibility.iter()
        .map(|(k, v)| (k, match v { Visibility::Hidden => 0u8, Visibility::Explored => 1, Visibility::Visible => 2 }))
        .collect();
    vis.sort();
    vis.hash(&mut h);
    h.finish()
}

/// Rebuild the light map if its inputs changed, and return it.
/// `allow_recompute = false` keeps a stale map (used while a token is being dragged,
/// so the sweep runs once on release instead of every frame).
pub fn ensure_light_map<'a>(
    presentation: &'a mut PresentationState,
    dungeon: &Dungeon,
    layout: &SpatialLayout,
    occ: &Occluders,
    allow_recompute: bool,
) -> Option<&'a LightMap> {
    let key = light_map_key(dungeon, presentation, occ);
    let stale = presentation.light_cache.as_ref().map(|m| m.key != key).unwrap_or(true);
    if stale && (allow_recompute || presentation.light_cache.is_none()) {
        let snapshot = PresentationSnapshot {
            room_visibility: presentation.room_visibility.clone(),
            doors_open: presentation.doors_open.clone(),
        };
        presentation.light_cache = Some(compute_light_map(dungeon, layout, &snapshot, occ, key));
    }
    presentation.light_cache.as_ref()
}

/// Compute the per-cell light and party vision.
pub fn compute_light_map(
    dungeon: &Dungeon,
    layout: &SpatialLayout,
    visibility: &dyn VisibilityProvider,
    occ: &Occluders,
    key: u64,
) -> LightMap {
    let floor = build_floor_set(layout, &dungeon.graph);

    // Ambient
    let ambient = if dungeon.ambient_light >= 0.5 {
        CellLight::Bright
    } else if dungeon.ambient_light >= 0.1 {
        CellLight::Dim
    } else {
        CellLight::Dark
    };
    let mut cells: CellMap<CellLight> = CellMap::default();
    if ambient != CellLight::Dark {
        for &c in &floor {
            cells.insert(c, ambient);
        }
    }

    // Lights
    let mut lights = Vec::new();
    for light in &dungeon.light_sources {
        let Some(origin) = light_origin(light, dungeon, layout) else { continue };
        let bright = light.radius.max(0.0);
        let dim = light.dim_radius().max(bright);
        lights.push(LightRing { center: origin, bright, dim, color: light.color });
        if dim <= 0.0 {
            continue;
        }
        let seen = los::visible_cells(origin, dim, occ, &floor);
        for c in seen {
            let dx = c.0 as f32 + 0.5 - origin.0;
            let dy = c.1 as f32 + 0.5 - origin.1;
            let d = (dx * dx + dy * dy).sqrt();
            let lvl = if d <= bright { CellLight::Bright } else { CellLight::Dim };
            let e = cells.entry(c).or_insert(CellLight::Dark);
            if lvl > *e {
                *e = lvl;
            }
        }
    }

    // Party vision
    let mut vision = Vec::new();
    let mut party_visible: CellSet = CellSet::default();
    let pc_tokens: Vec<(Pt, f32, f32)> = dungeon.tokens.iter().filter_map(|t| {
        let TokenKind::Player(pid) = &t.kind else { return None };
        let senses = dungeon.party.iter().find(|p| p.id == *pid).map(|p| p.senses).unwrap_or_default();
        Some(((t.x, t.y), senses.darkvision_cells(), senses.blind_sense_cells()))
    }).collect();

    if pc_tokens.is_empty() {
        // No party tokens: what players see is governed by fog alone
        let order = layout.render_order(&dungeon.graph);
        for &c in &floor {
            let visible = if let Some(rl) = layout.room_at_grid_ordered(&order, c.0, c.1) {
                *visibility.room_visibility(&rl.room_id) == Visibility::Visible
            } else if let Some(cid) = crate::ui::presentation_view::corridor_at_grid(layout, c.0, c.1) {
                super::fog::corridor_visibility_generic(&cid, visibility, &dungeon.graph) == Visibility::Visible
            } else {
                false
            };
            if visible {
                party_visible.insert(c);
            }
        }
    } else {
        // Farthest lit cell from any PC bounds the LOS sweep
        const MAX_SIGHT: f32 = 40.0;
        for (origin, dv, bs) in pc_tokens {
            let mut range = dv.max(bs);
            for (&c, &lvl) in &cells {
                if lvl == CellLight::Dark { continue; }
                let dx = c.0 as f32 + 0.5 - origin.0;
                let dy = c.1 as f32 + 0.5 - origin.1;
                range = range.max((dx * dx + dy * dy).sqrt());
            }
            let range = range.min(MAX_SIGHT).max(1.5);
            vision.push(VisionRing { center: origin, darkvision: dv, blind_sense: bs });
            let seen = los::visible_cells(origin, range, occ, &floor);
            for c in seen {
                let dx = c.0 as f32 + 0.5 - origin.0;
                let dy = c.1 as f32 + 0.5 - origin.1;
                let d = (dx * dx + dy * dy).sqrt();
                let lit = cells.get(&c).map(|l| *l != CellLight::Dark).unwrap_or(false);
                if lit || d <= dv || d <= bs {
                    party_visible.insert(c);
                }
            }
        }
    }

    // Wall band: one cell out from the floor, inheriting the best neighbouring floor cell
    let mut band: CellSet = CellSet::default();
    for &(x, y) in &floor {
        for dy in -1..=1 {
            for dx in -1..=1 {
                let c = (x + dx, y + dy);
                if !floor.contains(&c) {
                    band.insert(c);
                }
            }
        }
    }
    let mut band_levels: Vec<((i32, i32), CellLight, bool)> = Vec::new();
    for &(bx, by) in &band {
        let mut best = CellLight::Dark;
        let mut seen = false;
        for dy in -1..=1 {
            for dx in -1..=1 {
                let n = (bx + dx, by + dy);
                if !floor.contains(&n) { continue; }
                best = best.max(cells.get(&n).copied().unwrap_or(CellLight::Dark));
                seen |= party_visible.contains(&n);
            }
        }
        band_levels.push(((bx, by), best, seen));
    }
    for (c, lvl, seen) in band_levels {
        if lvl != CellLight::Dark { cells.insert(c, lvl); }
        if seen { party_visible.insert(c); }
    }

    let mut map = LightMap { key, floor, band, cells, party_visible, lights, vision, shade_runs: Vec::new() };
    map.shade_runs = map.compute_shade_runs();
    map
}

/// Shade cells the party cannot see (near-black) and dim cells (grey).
/// `alpha_scale` lets the DM canvas draw the same map faintly.
pub fn render_light_overlay(painter: &egui::Painter, t: &ViewTransform, map: &LightMap, alpha_scale: f32) {
    let clip = painter.clip_rect();
    for run in &map.shade_runs {
        let min = t.world_to_screen(egui::pos2(run.x0 as f32 * GRID_PX, run.y as f32 * GRID_PX));
        let max = t.world_to_screen(egui::pos2(run.x1 as f32 * GRID_PX, (run.y + 1) as f32 * GRID_PX));
        let rect = egui::Rect::from_two_pos(min, max).expand(0.5);
        if !clip.intersects(rect) {
            continue;
        }
        let a = (run.alpha * alpha_scale).clamp(0.0, 255.0) as u8;
        painter.rect_filled(rect, 0.0, egui::Color32::from_rgba_unmultiplied(0, 0, 0, a));
    }
}

/// Light rings (bright solid, dim dashed) and PC darkvision rings (dotted).
pub fn render_vision_rings(painter: &egui::Painter, t: &ViewTransform, map: &LightMap) {
    let px = |p: Pt| t.world_to_screen(egui::pos2(p.0 * GRID_PX, p.1 * GRID_PX));
    for l in &map.lights {
        let c = px(l.center);
        let col = egui::Color32::from_rgba_unmultiplied(l.color[0], l.color[1], l.color[2], 150);
        if l.bright > 0.0 {
            painter.circle_stroke(c, l.bright * GRID_PX * t.zoom, egui::Stroke::new(1.5_f32, col));
        }
        if l.dim > l.bright {
            dashed_circle(painter, c, l.dim * GRID_PX * t.zoom, egui::Stroke::new(1.0_f32, col), 8.0);
        }
    }
    for v in &map.vision {
        let c = px(v.center);
        let col = egui::Color32::from_rgba_unmultiplied(120, 170, 255, 150);
        if v.darkvision > 0.0 {
            dashed_circle(painter, c, v.darkvision * GRID_PX * t.zoom, egui::Stroke::new(1.0_f32, col), 3.0);
        }
        if v.blind_sense > 0.0 {
            let col2 = egui::Color32::from_rgba_unmultiplied(200, 140, 255, 150);
            dashed_circle(painter, c, v.blind_sense * GRID_PX * t.zoom, egui::Stroke::new(1.0_f32, col2), 3.0);
        }
    }
}

fn dashed_circle(painter: &egui::Painter, center: egui::Pos2, radius: f32, stroke: egui::Stroke, dash: f32) {
    if radius <= 0.0 {
        return;
    }
    let circumference = std::f32::consts::TAU * radius;
    let n = ((circumference / (dash * 2.0)).ceil() as usize).max(8);
    for i in 0..n {
        let a0 = i as f32 / n as f32 * std::f32::consts::TAU;
        let a1 = (i as f32 + 0.5) / n as f32 * std::f32::consts::TAU;
        let p0 = center + egui::vec2(a0.cos(), a0.sin()) * radius;
        let p1 = center + egui::vec2(a1.cos(), a1.sin()) * radius;
        painter.line_segment([p0, p1], stroke);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{RoomLayout, SpatialLayout};

    #[test]
    fn shade_runs_cover_exactly_the_shaded_cells() {
        // Row 0: dim, dim, bright, unseen. Row 1: dark, dark. Band cell above: unseen.
        let floor: CellSet = [(0, 0), (1, 0), (2, 0), (3, 0), (0, 1), (1, 1)].into_iter().collect();
        let band: CellSet = [(0, -1)].into_iter().collect();
        let cells: CellMap<CellLight> =
            [((0, 0), CellLight::Dim), ((1, 0), CellLight::Dim), ((2, 0), CellLight::Bright)].into_iter().collect();
        let party_visible: CellSet = [(0, 0), (1, 0), (2, 0), (0, 1), (1, 1)].into_iter().collect();
        let mut map = LightMap { key: 0, floor, band, cells, party_visible, lights: Vec::new(), vision: Vec::new(), shade_runs: Vec::new() };
        map.shade_runs = map.compute_shade_runs();
        assert_eq!(map.shade_runs, vec![
            ShadeRun { y: -1, x0: 0, x1: 1, alpha: 235.0 },
            ShadeRun { y: 0, x0: 0, x1: 2, alpha: 110.0 },
            ShadeRun { y: 0, x0: 3, x1: 4, alpha: 235.0 },
            ShadeRun { y: 1, x0: 0, x1: 2, alpha: 170.0 },
        ]);
    }

    fn make_layout_with_room(room_id: &str, x: i32, y: i32, w: u32, h: u32) -> SpatialLayout {
        SpatialLayout {
            rooms: vec![RoomLayout {
                room_id: room_id.to_string(),
                x, y, width: w, height: h,
                violations: Vec::new(),
                wall_openings: Vec::new(),
            }],
            corridors: Vec::new(),
            bounds: Vec::new(),
        }
    }

    fn light(room: &str, radius: f32) -> LightSource {
        LightSource {
            id: "l1".to_string(),
            room_id: room.to_string(),
            radius,
            intensity: 1.0,
            color: [255, 255, 200],
            pos: None,
            dim_radius: None,
            carrier: None,
        }
    }

    #[test]
    fn test_no_lights_returns_ambient() {
        let layout = make_layout_with_room("r1", 0, 0, 4, 4);
        let brightness = compute_brightness_generic(2.0, 2.0, &[], 0.3, &layout);
        assert!((brightness - 0.3).abs() < 0.001);
    }

    #[test]
    fn test_single_light_at_center() {
        let layout = make_layout_with_room("r1", 0, 0, 4, 4);
        let brightness = compute_brightness_generic(2.0, 2.0, &[light("r1", 5.0)], 0.0, &layout);
        assert!((brightness - 1.0).abs() < 0.001);
    }

    #[test]
    fn test_cell_outside_light_radius() {
        let layout = make_layout_with_room("r1", 0, 0, 4, 4);
        let brightness = compute_brightness_generic(20.0, 20.0, &[light("r1", 3.0)], 0.0, &layout);
        assert!((brightness - 0.0).abs() < 0.001);
    }

    #[test]
    fn test_brightness_capped_at_1() {
        let layout = make_layout_with_room("r1", 0, 0, 4, 4);
        let brightness = compute_brightness_generic(2.0, 2.0, &[light("r1", 5.0)], 0.8, &layout);
        assert!((brightness - 1.0).abs() < 0.001);
    }
}
