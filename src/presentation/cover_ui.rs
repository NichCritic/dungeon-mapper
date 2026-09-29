//! The on-demand cover tool: a heatmap of cover from a chosen attacker, or a badge on
//! each opposing token. Rendering is shared by the DM canvas and the player window.


use crate::model::{CoverLevel, MapToken, TokenKind};
use crate::util::{CellSet, ViewTransform, GRID_PX};
use super::los::{self, Block, CoverResult, Occluders, Square};
use super::tokens::{token_screen_rect, TokenInfo};

/// Heatmap radius around the attacker, in cells.
pub const HEATMAP_RADIUS: i32 = 24;

/// Cached heatmap for one attacker square and one occluder fingerprint.
#[derive(Clone, Debug)]
pub struct CoverCache {
    pub key: u64,
    pub cells: Vec<((i32, i32), CoverLevel)>,
}

/// Fingerprint for the heatmap inputs.
pub fn heatmap_key(attacker: &Square, occ_hash: u64) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    occ_hash.hash(&mut h);
    attacker.x0.to_bits().hash(&mut h);
    attacker.y0.to_bits().hash(&mut h);
    attacker.x1.to_bits().hash(&mut h);
    attacker.y1.to_bits().hash(&mut h);
    h.finish()
}

/// Cached badge results, aligned with the token list they were computed for.
#[derive(Clone, Debug)]
pub struct BadgeCache {
    pub key: u64,
    pub results: Vec<Option<CoverResult>>,
}

/// Fingerprint for the badge inputs: occluders (which include creature squares), the
/// attacker, and each token's square and whether it can be a target.
pub fn badges_key(attacker: &TokenKind, attacker_sq: &Square, tokens: &[MapToken], infos: &[TokenInfo], occ_hash: u64) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    heatmap_key(attacker_sq, occ_hash).hash(&mut h);
    attacker.hash(&mut h);
    for (tok, info) in tokens.iter().zip(infos) {
        tok.kind.hash(&mut h);
        tok.x.to_bits().hash(&mut h);
        tok.y.to_bits().hash(&mut h);
        info.size.to_bits().hash(&mut h);
        info.dead.hash(&mut h);
    }
    h.finish()
}

/// Compute cover for every floor cell within [`HEATMAP_RADIUS`] of the attacker.
pub fn compute_heatmap(
    attacker: &Square,
    attacker_kind: &TokenKind,
    occ: &Occluders,
    floor: &CellSet,
) -> Vec<((i32, i32), CoverLevel)> {
    let (cx, cy) = attacker.center();
    let (acx, acy) = (cx.floor() as i32, cy.floor() as i32);
    let exclude = [attacker_kind.clone()];
    // Every query's hull spans the attacker and one cell of this box, so geometry
    // outside it (with a cell of margin) can never matter.
    let r = HEATMAP_RADIUS as f32 + 1.0;
    let occ = &occ.restricted_to(
        attacker.x0.min(acx as f32 - r), attacker.y0.min(acy as f32 - r),
        attacker.x1.max(acx as f32 + r + 1.0), attacker.y1.max(acy as f32 + r + 1.0),
    );
    let mut out = Vec::new();
    for gy in (acy - HEATMAP_RADIUS)..=(acy + HEATMAP_RADIUS) {
        for gx in (acx - HEATMAP_RADIUS)..=(acx + HEATMAP_RADIUS) {
            if !floor.contains(&(gx, gy)) {
                continue;
            }
            let cell = Square::cell(gx, gy);
            // Skip the attacker's own space
            if cell.x0 >= attacker.x0 - 1e-3 && cell.x1 <= attacker.x1 + 1e-3
                && cell.y0 >= attacker.y0 - 1e-3 && cell.y1 <= attacker.y1 + 1e-3
            {
                continue;
            }
            let r = los::cover_between(attacker, &cell, occ, &exclude);
            out.push(((gx, gy), r.level));
        }
    }
    out
}

/// Fill color for a cover level on the heatmap. Okabe-Ito colour-blind-safe palette
/// with a lightness ramp (light → dark) so the levels read without hue: no cover is
/// left unfilled, half is pale yellow, three-quarters orange, total dark blue.
pub fn heat_color(level: CoverLevel) -> egui::Color32 {
    match level {
        CoverLevel::None => egui::Color32::TRANSPARENT,
        CoverLevel::Half => egui::Color32::from_rgba_unmultiplied(240, 228, 66, 110),
        CoverLevel::ThreeQuarters => egui::Color32::from_rgba_unmultiplied(230, 159, 0, 130),
        CoverLevel::Total => egui::Color32::from_rgba_unmultiplied(0, 68, 136, 160),
    }
}

/// Opaque swatch colour for legends and badges.
pub fn swatch_color(level: CoverLevel) -> egui::Color32 {
    match level {
        CoverLevel::None => egui::Color32::from_rgb(200, 200, 200),
        CoverLevel::Half => egui::Color32::from_rgb(240, 228, 66),
        CoverLevel::ThreeQuarters => egui::Color32::from_rgb(230, 159, 0),
        CoverLevel::Total => egui::Color32::from_rgb(0, 68, 136),
    }
}

/// Badge text for a token's cover.
pub fn badge_text(level: CoverLevel) -> Option<&'static str> {
    match level {
        CoverLevel::None => None,
        CoverLevel::Half => Some("\u{00bd}"),
        CoverLevel::ThreeQuarters => Some("\u{00be}"),
        CoverLevel::Total => Some("\u{00d7}"),
    }
}

/// Draw the heatmap cells. `visible` restricts which cells are drawn (player window).
pub fn render_heatmap(
    painter: &egui::Painter,
    t: &ViewTransform,
    cells: &[((i32, i32), CoverLevel)],
    visible: Option<&CellSet>,
) {
    for ((gx, gy), level) in cells {
        if let Some(v) = visible {
            if !v.contains(&(*gx, *gy)) {
                continue;
            }
        }
        let min = t.world_to_screen(egui::pos2(*gx as f32 * GRID_PX, *gy as f32 * GRID_PX));
        let max = t.world_to_screen(egui::pos2((*gx + 1) as f32 * GRID_PX, (*gy + 1) as f32 * GRID_PX));
        let rect = egui::Rect::from_two_pos(min, max);
        if !painter.clip_rect().intersects(rect) {
            continue;
        }
        if *level == CoverLevel::None {
            continue;
        }
        painter.rect_filled(rect, 0.0, heat_color(*level));
    }
}

/// Cover badges on tokens: `results[i]` is the cover of `tokens[i]`, `None` for tokens
/// that are not targets (same side as the attacker, or the attacker itself).
pub fn render_badges(
    painter: &egui::Painter,
    t: &ViewTransform,
    tokens: &[MapToken],
    infos: &[TokenInfo],
    results: &[Option<CoverResult>],
) {
    for ((token, info), res) in tokens.iter().zip(infos).zip(results) {
        let Some(res) = res else { continue };
        let Some(text) = badge_text(res.level) else { continue };
        let rect = token_screen_rect(token, info, t);
        if !painter.clip_rect().intersects(rect.expand(20.0)) {
            continue;
        }
        let br = (rect.width() * 0.22).clamp(6.0, 14.0);
        let pos = rect.left_bottom() + egui::vec2(br * 0.7, -br * 0.7);
        let fill = swatch_color(res.level);
        let ink = if res.level == CoverLevel::Total { egui::Color32::WHITE } else { egui::Color32::BLACK };
        painter.circle_filled(pos, br, fill);
        painter.circle_stroke(pos, br, egui::Stroke::new(1.0_f32, egui::Color32::BLACK));
        painter.text(pos, egui::Align2::CENTER_CENTER, text,
            egui::FontId::proportional(br * 1.4), ink);
    }
}

/// The four corner lines of one cover result, colored like the simulator.
pub fn render_lines(painter: &egui::Painter, t: &ViewTransform, res: &CoverResult) {
    for (a, b, block) in &res.lines {
        let color = match block {
            Block::Clear => egui::Color32::from_rgba_unmultiplied(50, 170, 50, 220),
            Block::Wall => egui::Color32::from_rgba_unmultiplied(205, 50, 50, 220),
            Block::Object(_) => egui::Color32::from_rgba_unmultiplied(180, 80, 180, 220),
            Block::Creature => egui::Color32::from_rgba_unmultiplied(205, 150, 50, 220),
        };
        let pa = t.world_to_screen(egui::pos2(a.0 * GRID_PX, a.1 * GRID_PX));
        let pb = t.world_to_screen(egui::pos2(b.0 * GRID_PX, b.1 * GRID_PX));
        painter.line_segment([pa, pb], egui::Stroke::new(1.5_f32, color));
    }
}

/// Which tokens are "enemies" of the attacker for badge purposes.
pub fn is_opposing(attacker: &TokenKind, other: &TokenKind) -> bool {
    matches!(
        (attacker, other),
        (TokenKind::Player(_), TokenKind::Monster(_)) | (TokenKind::Monster(_), TokenKind::Player(_))
    )
}

/// Cover of every opposing token from the attacker (None for non-targets).
pub fn compute_badges(
    attacker: &TokenKind,
    attacker_sq: &Square,
    tokens: &[MapToken],
    infos: &[TokenInfo],
    occ: &Occluders,
) -> Vec<Option<CoverResult>> {
    tokens.iter().zip(infos).map(|(tok, info)| {
        if !is_opposing(attacker, &tok.kind) || info.dead {
            return None;
        }
        let target = Square::centered(tok.x, tok.y, info.size.max(0.5));
        let exclude = [attacker.clone(), tok.kind.clone()];
        Some(los::cover_between(attacker_sq, &target, occ, &exclude))
    }).collect()
}
