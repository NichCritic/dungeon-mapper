//! Creature tokens on the map: one per monster instance (numbered like the combat
//! tracker) and one per player character. Tokens live on `Dungeon::tokens` so they
//! save, sync, and undo with the map. This module resolves their display data,
//! draws them, hit-tests them, and handles DM drag/placement.

use std::path::PathBuf;

use crate::data::MonsterDatabase;
use crate::model::{
    instance_label, size_footprint, size_visual_scale, CustomMonster, Dungeon, Encounter, MapToken,
    PlayerCharacter, RoomLayout, TokenKind,
};
use crate::util::{ViewTransform, GRID_PX};
use super::combat_tracker::{resolve_monster, CombatTracker};

const MONSTER_RING: egui::Color32 = egui::Color32::from_rgb(220, 70, 70);
const PLAYER_RING: egui::Color32 = egui::Color32::from_rgb(80, 140, 255);
const SELECTED_RING: egui::Color32 = egui::Color32::from_rgb(255, 220, 60);

/// Display data for one token, resolved from the encounter/party and (in combat) the tracker.
#[derive(Clone, Debug)]
pub struct TokenInfo {
    /// "Goblin #2" / PC name — matches the combat tracker.
    pub label: String,
    /// Instance number shown on the token when the entry has several.
    pub badge: Option<u32>,
    pub initials: String,
    /// Bestiary token art, if any.
    pub image: Option<PathBuf>,
    /// Footprint in cells per side (0.5 for Tiny, 1, 2, 3, 4).
    pub size: f32,
    /// Drawn size relative to the footprint (Small creatures draw a little smaller).
    pub visual_scale: f32,
    pub ring: egui::Color32,
    pub dead: bool,
    /// Hidden from players (stealth) per the combat tracker.
    pub stealth_hidden: bool,
}

/// Resolve display data for every token in `dungeon.tokens` (same order).
pub fn resolve_tokens(
    dungeon: &Dungeon,
    monster_db: &MonsterDatabase,
    tracker: Option<&CombatTracker>,
) -> Vec<TokenInfo> {
    dungeon.tokens.iter().map(|token| match &token.kind {
        TokenKind::Monster(mid) => {
            let entry = dungeon.encounters.iter()
                .find(|e| e.id == mid.encounter_id)
                .and_then(|enc| enc.monsters.get(mid.monster_index));
            let monster = entry.and_then(|em| resolve_monster(&em.monster_ref, monster_db, &dungeon.custom_monsters));
            let name = monster.map(|m| m.name.as_str()).unwrap_or("?");
            let count = entry.map(|em| em.count).unwrap_or(1);
            let inst = tracker.and_then(|t| t.instances.get(mid));
            TokenInfo {
                label: instance_label(name, count, mid.instance),
                badge: (count > 1).then_some(mid.instance as u32 + 1),
                initials: initials(name),
                image: entry.and_then(|em| monster_db.token_path_for_ref(&em.monster_ref, &dungeon.custom_monsters)),
                size: monster.map(|m| size_footprint(&m.size)).unwrap_or(1.0),
                visual_scale: monster.map(|m| size_visual_scale(&m.size)).unwrap_or(1.0),
                ring: MONSTER_RING,
                dead: inst.map(|i| i.is_dead).unwrap_or(false),
                stealth_hidden: inst.map(|i| i.hidden).unwrap_or(false),
            }
        }
        TokenKind::Player(pid) => {
            let pc = dungeon.party.iter().find(|p| p.id == *pid);
            let name = pc.map(|p| p.name.as_str()).unwrap_or("?");
            let in_combat = tracker.and_then(|t| t.players.get(pid));
            let dead = in_combat.map(|p| p.current_hp <= 0)
                .or_else(|| pc.map(|p| p.current_hp <= 0 && p.max_hp > 0))
                .unwrap_or(false);
            TokenInfo {
                label: name.to_string(),
                badge: None,
                initials: initials(name),
                image: None,
                size: 1.0,
                // Initials disc plus ring reads too heavy at a full cell; inset it a little
                visual_scale: 0.85,
                ring: PLAYER_RING,
                dead,
                stealth_hidden: false,
            }
        }
    }).collect()
}

/// Two-letter fallback for tokens without art: first letters of the first two words,
/// else the first two letters of the name.
pub fn initials(name: &str) -> String {
    let words: Vec<&str> = name.split_whitespace()
        .filter(|w| w.chars().next().map(|c| c.is_alphanumeric()).unwrap_or(false))
        .collect();
    let picked: String = if words.len() >= 2 {
        words.iter().take(2).filter_map(|w| w.chars().next()).collect()
    } else {
        name.chars().filter(|c| c.is_alphanumeric()).take(2).collect()
    };
    picked.to_uppercase()
}

/// Screen rectangle of a token as drawn. Axis-aligned around the transformed
/// center so it stays on its cell under map rotation.
pub fn token_screen_rect(token: &MapToken, info: &TokenInfo, t: &ViewTransform) -> egui::Rect {
    let center = t.world_to_screen(egui::pos2(token.x * GRID_PX, token.y * GRID_PX));
    let side = info.size.max(0.25) * info.visual_scale * GRID_PX * t.zoom;
    egui::Rect::from_center_size(center, egui::vec2(side, side))
}

/// Footprint sizes (cells per side) for each entry of an encounter.
pub fn encounter_sizes(enc: &Encounter, custom: &[CustomMonster], monster_db: &MonsterDatabase) -> Vec<f32> {
    enc.monsters.iter().map(|em| {
        resolve_monster(&em.monster_ref, monster_db, custom)
            .map(|m| size_footprint(&m.size))
            .unwrap_or(1.0)
    }).collect()
}

/// Topmost token under a screen position, if any.
pub fn token_at_screen_pos(
    pos: egui::Pos2,
    t: &ViewTransform,
    tokens: &[MapToken],
    infos: &[TokenInfo],
) -> Option<usize> {
    tokens.iter().enumerate().rev().find(|(i, token)| {
        let Some(info) = infos.get(*i) else { return false };
        let rect = token_screen_rect(token, info, t);
        rect.center().distance(pos) <= (rect.width() / 2.0).max(6.0)
    }).map(|(i, _)| i)
}

/// How to draw the token layer.
pub struct TokenStyle<'a> {
    /// Draw the full label under each token (DM views).
    pub show_names: bool,
    pub selected: Option<&'a TokenKind>,
    /// Player-facing: hides stealth-hidden monsters.
    pub player_view: bool,
}

/// Draw tokens: art (or an initials disc), a colored ring, the instance badge,
/// a dead marker, and optionally the label.
pub fn render_tokens(
    painter: &egui::Painter,
    ctx: &egui::Context,
    t: &ViewTransform,
    tokens: &[MapToken],
    infos: &[TokenInfo],
    style: &TokenStyle<'_>,
) {
    for (token, info) in tokens.iter().zip(infos) {
        if style.player_view && info.stealth_hidden && matches!(token.kind, TokenKind::Monster(_)) {
            continue;
        }
        let rect = token_screen_rect(token, info, t);
        if !painter.clip_rect().intersects(rect.expand(20.0)) {
            continue;
        }
        let center = rect.center();
        let radius = rect.width() / 2.0;
        let alpha: u8 = if info.dead { 110 } else { 255 };

        let mut drew_image = false;
        if let Some(path) = &info.image {
            let uri = format!("file://{}", path.display());
            match egui::Image::from_uri(uri).load_for_size(ctx, rect.size()) {
                Ok(egui::load::TexturePoll::Ready { texture }) => {
                    let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
                    painter.image(texture.id, rect, uv, egui::Color32::from_white_alpha(alpha));
                    drew_image = true;
                }
                Ok(egui::load::TexturePoll::Pending { .. }) => ctx.request_repaint(),
                Err(_) => {}
            }
        }
        if !drew_image {
            painter.circle_filled(center, radius, egui::Color32::from_rgba_unmultiplied(35, 35, 40, alpha));
            let font = egui::FontId::proportional((radius * 0.9).max(7.0));
            painter.text(center, egui::Align2::CENTER_CENTER, &info.initials, font,
                egui::Color32::from_rgba_unmultiplied(235, 235, 235, alpha));
        }

        let selected = style.selected == Some(&token.kind);
        let ring = if selected { SELECTED_RING } else { info.ring };
        let ring_w = (2.0 * t.zoom).clamp(1.5, 4.0);
        painter.circle_stroke(center, radius, egui::Stroke::new(ring_w, ring));

        if let Some(n) = info.badge {
            let br = (radius * 0.42).clamp(5.0, 14.0).min(radius.max(5.0));
            let bpos = rect.right_top() + egui::vec2(-br * 0.7, br * 0.7);
            painter.circle_filled(bpos, br, egui::Color32::from_rgb(20, 20, 20));
            painter.circle_stroke(bpos, br, egui::Stroke::new(1.0_f32, info.ring));
            painter.text(bpos, egui::Align2::CENTER_CENTER, n.to_string(),
                egui::FontId::proportional(br * 1.3), egui::Color32::WHITE);
        }

        if info.dead {
            let s = egui::Stroke::new(ring_w, egui::Color32::from_rgb(230, 50, 50));
            let d = radius * 0.7;
            painter.line_segment([center + egui::vec2(-d, -d), center + egui::vec2(d, d)], s);
            painter.line_segment([center + egui::vec2(-d, d), center + egui::vec2(d, -d)], s);
        }

        if style.show_names {
            let font = egui::FontId::proportional((9.0 * t.zoom).clamp(8.0, 14.0));
            let galley = painter.layout_no_wrap(info.label.clone(), font.clone(), egui::Color32::WHITE);
            let pos = rect.center_bottom() + egui::vec2(0.0, 2.0);
            let pill = egui::Rect::from_min_size(
                pos - egui::vec2(galley.size().x / 2.0 + 3.0, 0.0),
                galley.size() + egui::vec2(6.0, 2.0),
            );
            painter.rect_filled(pill, 3.0, egui::Color32::from_rgba_unmultiplied(0, 0, 0, 190));
            painter.galley(pill.min + egui::vec2(3.0, 1.0), galley, egui::Color32::WHITE);
        }
    }
}

/// Snap a token center to the grid: sub-cell footprints to quarter-cell centers,
/// odd whole sizes to cell centers, even sizes to grid lines.
pub fn snap(token: &mut MapToken, size: f32) {
    let snap_axis = |v: f32| -> f32 {
        if size < 1.0 {
            (v * 2.0).floor() / 2.0 + 0.25
        } else if (size.round() as i32) % 2 == 1 {
            v.floor() + 0.5
        } else {
            v.round()
        }
    };
    token.x = snap_axis(token.x);
    token.y = snap_axis(token.y);
}

/// A point given in the room's unrotated grid position, turned into place when the
/// room is rotated (packing runs in the room's own frame).
fn in_room(room: &RoomLayout, x: f32, y: f32) -> (f32, f32) {
    room.to_world(x - room.x as f32, y - room.y as f32)
}

/// Row-pack `items` (id, size) into a room, top-left first, one-cell margin when
/// the room is big enough. Returns tokens in the same order.
fn pack_into_room(items: Vec<(TokenKind, f32)>, room: &RoomLayout) -> Vec<MapToken> {
    let margin = if room.width > 2 && room.height > 2 { 1 } else { 0 };
    let x0 = room.x + margin;
    let y0 = room.y + margin;
    let inner_w = (room.width as i32 - margin * 2).max(1);
    let (mut cx, mut cy, mut row_h) = (x0, y0, 0i32);
    let mut out = Vec::with_capacity(items.len());
    for (kind, size) in items {
        // Sub-cell creatures still take a whole cell in the packing
        let s = (size.ceil() as i32).max(1);
        if cx > x0 && cx + s > x0 + inner_w {
            cx = x0;
            cy += row_h;
            row_h = 0;
        }
        let (x, y) = in_room(room, cx as f32 + s as f32 / 2.0, cy as f32 + s as f32 / 2.0);
        out.push(MapToken { kind, x, y });
        cx += s;
        row_h = row_h.max(s);
    }
    out
}

/// Replace an encounter's tokens with a fresh set packed into `room`.
/// `sizes[i]` is the cell size of `enc.monsters[i]`.
pub fn place_encounter_tokens(tokens: &mut Vec<MapToken>, enc: &Encounter, room: &RoomLayout, sizes: &[f32]) {
    remove_encounter_tokens(tokens, &enc.id);
    let mut items = Vec::new();
    for (m_idx, em) in enc.monsters.iter().enumerate() {
        let size = sizes.get(m_idx).copied().unwrap_or(1.0);
        for i in 0..em.count as usize {
            let mid = crate::model::MonsterInstanceId {
                encounter_id: enc.id.clone(),
                monster_index: m_idx,
                instance: i,
            };
            items.push((TokenKind::Monster(mid), size));
        }
    }
    tokens.extend(pack_into_room(items, room));
}

/// Replace the party's tokens with one per PC packed into `room`.
pub fn place_party_tokens(tokens: &mut Vec<MapToken>, party: &[PlayerCharacter], room: &RoomLayout) {
    remove_party_tokens(tokens);
    let items = party.iter().map(|pc| (TokenKind::Player(pc.id.clone()), 1.0)).collect();
    tokens.extend(pack_into_room(items, room));
}

/// Make sure `pc_id` has a token, placing it on the first free cell of `room`
/// (row-major from the top-left, one-cell margin) without moving anyone else.
/// Returns true if a token was added.
pub fn ensure_player_token(tokens: &mut Vec<MapToken>, pc_id: &str, room: &RoomLayout) -> bool {
    let kind = TokenKind::Player(pc_id.to_string());
    if tokens.iter().any(|t| t.kind == kind) {
        return false;
    }
    let margin = if room.width > 2 && room.height > 2 { 1 } else { 0 };
    let occupied = |gx: i32, gy: i32| {
        let (x, y) = in_room(room, gx as f32 + 0.5, gy as f32 + 0.5);
        tokens.iter().any(|t| t.x.floor() == x.floor() && t.y.floor() == y.floor())
    };
    let mut spot = None;
    'scan: for gy in (room.y + margin)..(room.y + room.height as i32 - margin).max(room.y + margin + 1) {
        for gx in (room.x + margin)..(room.x + room.width as i32 - margin).max(room.x + margin + 1) {
            if !occupied(gx, gy) {
                spot = Some((gx, gy));
                break 'scan;
            }
        }
    }
    let (gx, gy) = spot.unwrap_or((room.x + margin, room.y + margin));
    let (x, y) = in_room(room, gx as f32 + 0.5, gy as f32 + 0.5);
    tokens.push(MapToken { kind, x, y });
    true
}

pub fn remove_encounter_tokens(tokens: &mut Vec<MapToken>, enc_id: &str) {
    tokens.retain(|t| !matches!(&t.kind, TokenKind::Monster(mid) if mid.encounter_id == enc_id));
}

pub fn remove_party_tokens(tokens: &mut Vec<MapToken>) {
    tokens.retain(|t| !matches!(t.kind, TokenKind::Player(_)));
}

pub fn encounter_has_tokens(tokens: &[MapToken], enc_id: &str) -> bool {
    tokens.iter().any(|t| matches!(&t.kind, TokenKind::Monster(mid) if mid.encounter_id == enc_id))
}

/// Tokens inside a room's footprint (by center cell).
pub fn tokens_in_room(tokens: &[MapToken], room: &RoomLayout) -> Vec<usize> {
    tokens.iter().enumerate()
        .filter(|(_, t)| room.contains_point(t.x.floor() + 0.5, t.y.floor() + 0.5))
        .map(|(i, _)| i)
        .collect()
}

/// Drop tokens whose creature no longer exists: removed encounters/monster entries,
/// instances beyond the entry's count, and (when a party is loaded) unknown PCs.
pub fn prune_stale(tokens: &mut Vec<MapToken>, dungeon: &Dungeon) {
    let party_known = !dungeon.party.is_empty();
    tokens.retain(|t| match &t.kind {
        TokenKind::Monster(mid) => dungeon.encounters.iter()
            .find(|e| e.id == mid.encounter_id)
            .and_then(|enc| enc.monsters.get(mid.monster_index))
            .map(|em| (mid.instance as u32) < em.count)
            .unwrap_or(false),
        TokenKind::Player(pid) => !party_known || dungeon.party.iter().any(|p| p.id == *pid),
    });
}

/// In-progress token drag.
#[derive(Clone, Debug)]
pub struct TokenDrag {
    pub idx: usize,
    /// Pointer offset from the token center at grab time, in grid units.
    pub grab: egui::Vec2,
}

/// Press / drag / release / click handling for DM canvases. Uses the absolute
/// pointer position so it works under rotation. Returns true when the pointer
/// interacted with a token this frame (callers should then skip room selection).
pub fn handle_token_drag(
    response: &egui::Response,
    t: &ViewTransform,
    tokens: &mut Vec<MapToken>,
    infos: &[TokenInfo],
    drag: &mut Option<TokenDrag>,
    selected: &mut Option<TokenKind>,
) -> bool {
    let grid_pos = |pos: egui::Pos2| {
        let w = t.screen_to_world(pos);
        egui::vec2(w.x / GRID_PX, w.y / GRID_PX)
    };
    let mut busy = false;

    if drag.is_none() && response.drag_started_by(egui::PointerButton::Primary) {
        if let Some(pos) = response.interact_pointer_pos() {
            if let Some(idx) = token_at_screen_pos(pos, t, tokens, infos) {
                let g = grid_pos(pos);
                let token = &tokens[idx];
                *drag = Some(TokenDrag { idx, grab: g - egui::vec2(token.x, token.y) });
                *selected = Some(token.kind.clone());
            }
        }
    }

    if let Some(d) = drag.clone() {
        busy = true;
        if d.idx >= tokens.len() {
            *drag = None;
        } else {
            if response.dragged_by(egui::PointerButton::Primary) {
                if let Some(pos) = response.interact_pointer_pos() {
                    let g = grid_pos(pos) - d.grab;
                    tokens[d.idx].x = g.x;
                    tokens[d.idx].y = g.y;
                }
            }
            if response.drag_stopped() {
                let size = infos.get(d.idx).map(|i| i.size).unwrap_or(1.0);
                snap(&mut tokens[d.idx], size);
                *drag = None;
            }
        }
    } else if response.clicked() {
        if let Some(pos) = response.interact_pointer_pos() {
            if let Some(idx) = token_at_screen_pos(pos, t, tokens, infos) {
                *selected = Some(tokens[idx].kind.clone());
                busy = true;
            } else if selected.is_some() {
                *selected = None;
            }
        }
    }
    busy
}

/// Remove the selected token if Delete/Backspace was pressed. Returns true if removed.
pub fn delete_selected(ui: &egui::Ui, tokens: &mut Vec<MapToken>, selected: &mut Option<TokenKind>) -> bool {
    let Some(kind) = selected.as_ref() else { return false };
    if !ui.input(|i| i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace)) {
        return false;
    }
    let before = tokens.len();
    tokens.retain(|t| t.kind != *kind);
    *selected = None;
    tokens.len() != before
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{EncounterMonster, EncounterType, MonsterRef};

    fn room(x: i32, y: i32, w: u32, h: u32) -> RoomLayout {
        RoomLayout { room_id: "r".into(), x, y, width: w, height: h, violations: Vec::new(), wall_openings: Vec::new(), rotation: 0.0 }
    }

    fn encounter(counts: &[u32]) -> Encounter {
        let mut enc = Encounter::new("Camp".into(), "r".into());
        enc.encounter_type = EncounterType::Static;
        for (i, &count) in counts.iter().enumerate() {
            enc.monsters.push(EncounterMonster {
                monster_ref: MonsterRef::Base { source: "MM".into(), name: format!("M{}", i) },
                count,
                notes: String::new(),
            });
        }
        enc
    }

    #[test]
    fn initials_pick_two_letters() {
        assert_eq!(initials("Hobgoblin Captain"), "HC");
        assert_eq!(initials("Goblin"), "GO");
        assert_eq!(initials("Ancient Red Dragon"), "AR");
        assert_eq!(initials("X"), "X");
    }

    #[test]
    fn instance_labels_match_tracker_rule() {
        assert_eq!(instance_label("Goblin", 3, 1), "Goblin #2");
        assert_eq!(instance_label("Ogre", 1, 0), "Ogre");
    }

    #[test]
    fn placement_fills_room_and_stays_inside() {
        let enc = encounter(&[3, 1]);
        let rl = room(4, 6, 6, 6);
        let mut tokens = Vec::new();
        place_encounter_tokens(&mut tokens, &enc, &rl, &[1.0, 2.0]);
        assert_eq!(tokens.len(), 4);
        for t in &tokens {
            assert!(t.x > rl.x as f32 && t.x < (rl.x + rl.width as i32) as f32, "x {}", t.x);
            assert!(t.y > rl.y as f32 && t.y < (rl.y + rl.height as i32) as f32, "y {}", t.y);
        }
        // Re-placing replaces rather than duplicates
        place_encounter_tokens(&mut tokens, &enc, &rl, &[1.0, 2.0]);
        assert_eq!(tokens.len(), 4);
        assert_eq!(tokens_in_room(&tokens, &rl).len(), 4);
    }

    #[test]
    fn snap_centers_by_parity() {
        let mut t = MapToken { kind: TokenKind::Player("p".into()), x: 3.2, y: 7.9 };
        snap(&mut t, 1.0);
        assert_eq!((t.x, t.y), (3.5, 7.5));
        snap(&mut t, 2.0);
        assert_eq!((t.x, t.y), (4.0, 8.0));
        t.x = 3.2; t.y = 7.9;
        snap(&mut t, 0.5);
        assert_eq!((t.x, t.y), (3.25, 7.75));
    }

    #[test]
    fn prune_drops_missing_instances_but_keeps_players_without_party() {
        let mut dungeon = Dungeon::new("d".into());
        let enc = encounter(&[2]);
        let enc_id = enc.id.clone();
        dungeon.encounters.push(enc);
        let mid = |i| crate::model::MonsterInstanceId { encounter_id: enc_id.clone(), monster_index: 0, instance: i };
        dungeon.tokens = vec![
            MapToken { kind: TokenKind::Monster(mid(0)), x: 0.5, y: 0.5 },
            MapToken { kind: TokenKind::Monster(mid(1)), x: 1.5, y: 0.5 },
            MapToken { kind: TokenKind::Monster(mid(2)), x: 2.5, y: 0.5 },
            MapToken { kind: TokenKind::Player("ghost".into()), x: 3.5, y: 0.5 },
        ];
        let mut tokens = dungeon.tokens.clone();
        prune_stale(&mut tokens, &dungeon);
        assert_eq!(tokens.len(), 3);
        assert!(tokens.iter().any(|t| matches!(t.kind, TokenKind::Player(_))));

        dungeon.party.push(PlayerCharacter::new("Ann".into()));
        prune_stale(&mut tokens, &dungeon);
        assert_eq!(tokens.len(), 2);
    }
}
