//! The hex map: editing it (terrain, landmarks, rivers and roads, battle maps), and
//! playing on it at the table (the party's token and what the players have seen).

use crate::data::MonsterDatabase;
use crate::model::*;
use crate::render::decor::PainterSink;
use crate::ui::encounters_view::{monster_summary, open_encounter_editor};
use crate::render::recording::{RecordingRenderer, RenderCommand, ReplayCache};
use crate::ui::canvas_common::{handle_pan_zoom, ViewState, COLOR_SELECTION};
use crate::util::ViewTransform;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HexTool {
    Select,
    Terrain,
    Landmark,
    Path,
    Erase,
}

impl HexTool {
    const ALL: [HexTool; 5] = [HexTool::Select, HexTool::Terrain, HexTool::Landmark, HexTool::Path, HexTool::Erase];

    fn label(self) -> &'static str {
        match self {
            HexTool::Select => "Select",
            HexTool::Terrain => "Terrain",
            HexTool::Landmark => "Landmark",
            HexTool::Path => "River / road",
            HexTool::Erase => "Erase",
        }
    }
}

/// Something the hex view needs the app to do: it owns the other maps.
#[derive(Clone, Debug, PartialEq)]
pub enum HexRequest {
    /// Open the map with this id.
    OpenMap(String),
    /// Make a battle map for the selected hexes, link them to it and open it.
    CreateBattleMap,
    /// Copy an encounter onto a hex's battle map (making one if it has none) and
    /// open it there.
    Fight { encounter: String, hex: HexCoord },
}

/// The hex map drawn once into commands, replayed each frame until it changes.
#[derive(Default)]
pub struct HexCanvasCache {
    hash: Option<u64>,
    generation: u64,
    commands: Vec<RenderCommand>,
    replay: ReplayCache,
}

impl HexCanvasCache {
    pub fn paint(&mut self, painter: &egui::Painter, transform: &ViewTransform, map: &HexMap, theme: &Theme, players: bool) {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        crate::util::hash_serde(&mut h, &(map.orientation, &map.hexes, &map.paths));
        crate::util::hash_serde(&mut h, theme);
        players.hash(&mut h);
        if players {
            map.revealed.hash(&mut h);
            map.explored.hash(&mut h);
        }
        let hash = h.finish();
        if self.hash != Some(hash) {
            // Small enough to draw on the spot, so painting terrain never flickers
            let mut rec = RecordingRenderer::new();
            crate::render::hex::render_hex_map(&mut rec, map, theme, players);
            self.commands = rec.commands;
            self.hash = Some(hash);
            self.generation += 1;
        }
        self.replay.paint(painter, transform, &self.commands, self.generation);
    }
}

pub struct HexViewState {
    pub view: ViewState,
    pub tool: HexTool,
    pub brush: HexTerrain,
    /// The landmark the Landmark tool places (`None` clears).
    pub landmark: Option<HexFeature>,
    pub path_kind: HexPathKind,
    /// The hexes clicked so far for a new river or road.
    pub drawing_path: Vec<HexCoord>,
    /// Selected hexes, the first one the main one.
    pub selected: Vec<HexCoord>,
    pub selected_path: Option<String>,
    pub show_labels: bool,
    pub request: Option<HexRequest>,
    /// How far "Observe from here" sees, at the table.
    pub lookout_radius: u32,
    /// The hex the right-click menu is for.
    menu_hex: Option<HexCoord>,
    cache: HexCanvasCache,
    fitted: bool,
    dragging_party: bool,
    /// Canvas size last frame, for fitting the view from the sidebar.
    canvas_size: egui::Vec2,
}

impl Default for HexViewState {
    fn default() -> Self {
        Self {
            view: ViewState::default(),
            tool: HexTool::Select,
            brush: HexTerrain::Grassland,
            landmark: Some(HexFeature::Village),
            path_kind: HexPathKind::River,
            drawing_path: Vec::new(),
            selected: Vec::new(),
            selected_path: None,
            show_labels: true,
            request: None,
            lookout_radius: 2,
            menu_hex: None,
            cache: HexCanvasCache::default(),
            fitted: false,
            dragging_party: false,
            canvas_size: egui::Vec2::ZERO,
        }
    }
}

impl HexViewState {
    /// The main selected hex, if it's on the map.
    pub fn selected_hex<'a>(&self, map: &'a HexMap) -> Option<&'a Hex> {
        self.selected.first().and_then(|&c| map.hex(c))
    }

    /// Forget selections that may point at hexes or paths that are gone.
    pub fn clear_selection(&mut self) {
        self.selected.clear();
        self.selected_path = None;
        self.drawing_path.clear();
        self.dragging_party = false;
    }
}

/// Fit a view to the given hexes on a canvas of `size`.
pub fn fit_view<'a>(view: &mut ViewState, map: &HexMap, coords: impl IntoIterator<Item = &'a HexCoord>, size: egui::Vec2) {
    let Some((x0, y0, x1, y1)) = map.extents(coords) else { return };
    if size.x <= 0.0 || size.y <= 0.0 {
        return;
    }
    let margin = HEX_SIZE;
    view.zoom = (size.x / (x1 - x0 + margin * 2.0)).min(size.y / (y1 - y0 + margin * 2.0)).clamp(0.1, 10.0);
    view.center_on((x0 + x1) / 2.0, (y0 + y1) / 2.0, size);
}

fn hex_screen(map: &HexMap, c: HexCoord, scale: f32, t: &ViewTransform) -> Vec<egui::Pos2> {
    map.orientation.corners(c, scale).iter().map(|&(x, y)| t.world_to_screen(egui::pos2(x, y))).collect()
}

fn outline(painter: &egui::Painter, map: &HexMap, c: HexCoord, t: &ViewTransform, stroke: egui::Stroke) {
    painter.add(egui::Shape::closed_line(hex_screen(map, c, 0.94, t), stroke));
}

fn hex_under(map: &HexMap, t: &ViewTransform, pos: Option<egui::Pos2>) -> Option<HexCoord> {
    let w = t.screen_to_world(pos?);
    Some(map.orientation.hex_at(w.x, w.y))
}

/// Hex labels, under each hex's centre.
fn draw_labels(painter: &egui::Painter, map: &HexMap, t: &ViewTransform, theme: &Theme, shown: impl Fn(HexCoord) -> bool) {
    let ink = theme.wall_color;
    let color = egui::Color32::from_rgba_unmultiplied(ink[0], ink[1], ink[2], 230);
    let font = egui::FontId::proportional((7.5 * t.zoom).max(6.0));
    for hex in map.hexes.iter().filter(|h| !h.label.trim().is_empty() && shown(h.coord)) {
        let (cx, cy) = map.orientation.center(hex.coord);
        let pos = t.world_to_screen(egui::pos2(cx, cy + HEX_SIZE * 0.62));
        painter.text(pos, egui::Align2::CENTER_CENTER, &hex.label, font.clone(), color);
    }
}

/// A sheep badge with the count in the upper left of each hex that has sheep.
fn draw_sheep(painter: &egui::Painter, map: &HexMap, t: &ViewTransform, theme: &Theme, shown: impl Fn(HexCoord) -> bool) {
    let [ir, ig, ib, _] = theme.wall_color;
    let [pr, pg, pb, _] = theme.floor_color;
    let ink = egui::Color32::from_rgb(ir, ig, ib);
    let paper = egui::Color32::from_rgba_unmultiplied(pr, pg, pb, 225);
    let font = egui::FontId::proportional((7.5 * t.zoom).max(6.0));
    let u = (HEX_SIZE * 0.075 * t.zoom).max(3.0);
    for hex in map.hexes.iter().filter(|h| h.sheep > 0 && shown(h.coord)) {
        let (cx, cy) = map.orientation.center(hex.coord);
        // Off-centre the other way from the party's banner, clear of the landmark
        let at = t.world_to_screen(egui::pos2(cx - HEX_SIZE * 0.5, cy - HEX_SIZE * 0.36));
        let galley = painter.layout_no_wrap(hex.sheep.to_string(), font.clone(), ink);
        let icon_w = u * 3.2;
        let pad = u * 0.6;
        let w = pad + icon_w + u * 0.4 + galley.size().x + pad;
        let h = (u * 2.6).max(galley.size().y + 2.0);
        let pill = egui::Rect::from_min_size(egui::pos2(at.x - icon_w / 2.0 - pad, at.y - h / 2.0), egui::vec2(w, h));
        painter.rect(pill, h / 2.0, paper, egui::Stroke::new(1.0_f32, ink.gamma_multiply(0.6)), egui::StrokeKind::Inside);
        draw_sheep_icon(painter, at, u, ink);
        painter.galley(egui::pos2(at.x + icon_w / 2.0 + u * 0.4, at.y - galley.size().y / 2.0), galley, ink);
    }
}

/// A little sheep facing right, centred on `at`, about 3 × 2 `u` across.
fn draw_sheep_icon(painter: &egui::Painter, at: egui::Pos2, u: f32, ink: egui::Color32) {
    let fleece = [(-0.55, 0.1), (0.35, 0.1), (-0.1, -0.3), (-0.1, 0.35)];
    let leg = egui::Stroke::new((u * 0.22).max(1.0), ink);
    for dx in [-0.6, 0.35] {
        painter.line_segment([at + egui::vec2(dx * u, 0.4 * u), at + egui::vec2(dx * u, 1.0 * u)], leg);
    }
    // Ink a touch larger first, so the fleece reads as one outlined cloud
    let edge = (u * 0.18).max(0.8);
    for (x, y) in fleece {
        painter.circle_filled(at + egui::vec2(x * u, y * u), u * 0.62 + edge, ink);
    }
    for (x, y) in fleece {
        painter.circle_filled(at + egui::vec2(x * u, y * u), u * 0.62, egui::Color32::WHITE);
    }
    painter.circle_filled(at + egui::vec2(1.05 * u, -0.2 * u), u * 0.42, ink);
}

fn draw_party(painter: &egui::Painter, map: &HexMap, t: &ViewTransform, at: Option<HexCoord>) {
    let Some(c) = at else { return };
    let (cx, cy) = map.orientation.center(c);
    // Off-centre, so the hex's landmark still shows
    let p = t.world_to_screen(egui::pos2(cx + HEX_SIZE * 0.42, cy - HEX_SIZE * 0.36));
    let mut sink = PainterSink { painter };
    crate::render::hex::draw_party_token(&mut sink, p.x, p.y, HEX_SIZE * 0.22 * t.zoom);
}

fn paint_background(painter: &egui::Painter, rect: egui::Rect, theme: &Theme) {
    let bg = theme.bg_color;
    painter.rect_filled(rect, 0.0, egui::Color32::from_rgba_unmultiplied(bg[0], bg[1], bg[2], bg[3]));
}

// ------------------------------------------------------------------ editing

/// The hex map editor canvas.
pub fn hex_view(ui: &mut egui::Ui, dungeon: &mut Dungeon, state: &mut HexViewState) {
    let (response, painter) = ui.allocate_painter(ui.available_size(), egui::Sense::click_and_drag());
    let rect = response.rect;
    paint_background(&painter, rect, &dungeon.theme);
    let Some(map) = dungeon.hex_map.as_mut() else { return };

    state.canvas_size = rect.size();
    if !state.fitted {
        let all: Vec<HexCoord> = map.hexes.iter().map(|h| h.coord).collect();
        fit_view(&mut state.view, map, &all, rect.size());
        state.fitted = true;
    }
    handle_pan_zoom(&response, &mut state.view);
    // In the select tool a plain drag pans too (unless it's the party being moved)
    if state.tool == HexTool::Select && response.dragged_by(egui::PointerButton::Primary) && !state.dragging_party {
        state.view.offset += response.drag_delta();
    }
    let t = ViewTransform::new(state.view.offset, state.view.zoom, rect);
    let hover = hex_under(map, &t, response.hover_pos());

    // Edits
    let pointer_down = response.is_pointer_button_down_on() && ui.input(|i| i.pointer.primary_down());
    match state.tool {
        HexTool::Select => {
            if response.drag_started_by(egui::PointerButton::Primary) {
                let start = hex_under(map, &t, ui.input(|i| i.pointer.press_origin()));
                state.dragging_party = start.is_some() && start == map.party;
            }
            if state.dragging_party && response.drag_stopped() {
                if let Some(c) = hover.filter(|c| map.hex(*c).is_some()) {
                    map.move_party(c);
                }
                state.dragging_party = false;
            }
            if response.clicked() {
                let path = response.interact_pointer_pos().and_then(|p| path_near(map, &t, p));
                let shift = ui.input(|i| i.modifiers.shift);
                match (path, hover.filter(|c| map.hex(*c).is_some())) {
                    (Some(id), _) if !shift => {
                        state.selected_path = Some(id);
                        state.selected.clear();
                    }
                    (_, Some(c)) => {
                        state.selected_path = None;
                        if shift {
                            match state.selected.iter().position(|s| *s == c) {
                                Some(i) => {
                                    state.selected.remove(i);
                                }
                                None => state.selected.push(c),
                            }
                        } else {
                            state.selected = vec![c];
                        }
                    }
                    _ => {
                        state.selected.clear();
                        state.selected_path = None;
                    }
                }
            }
        }
        HexTool::Terrain => {
            if pointer_down {
                if let Some(c) = hover {
                    map.ensure_hex(c, state.brush).terrain = state.brush;
                }
            }
        }
        HexTool::Landmark => {
            if response.clicked() {
                if let Some(hex) = hover.and_then(|c| map.hex_mut(c)) {
                    hex.feature = state.landmark;
                }
            }
        }
        HexTool::Path => {
            if response.clicked() {
                if let Some(c) = hover.filter(|c| map.hex(*c).is_some()) {
                    if state.drawing_path.last() != Some(&c) {
                        state.drawing_path.push(c);
                    }
                }
            }
            let finish = response.double_clicked()
                || response.secondary_clicked()
                || ui.input(|i| i.key_pressed(egui::Key::Enter));
            if finish {
                finish_path(map, state);
            }
            if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                state.drawing_path.clear();
            }
        }
        HexTool::Erase => {
            if pointer_down {
                if let Some(c) = hover.filter(|c| map.hex(*c).is_some()) {
                    if let Some(gone) = map.remove_hex(c) {
                        dungeon.encounters.retain(|e| e.home_room_id != gone.id);
                    }
                    state.selected.retain(|s| *s != c);
                }
            }
        }
    }
    if state.tool != HexTool::Path {
        state.drawing_path.clear();
    }
    let delete = response.hovered() && ui.input(|i| i.key_pressed(egui::Key::Delete));
    if delete && state.tool == HexTool::Select {
        if let Some(id) = state.selected_path.take() {
            map.paths.retain(|p| p.id != id);
        } else {
            for c in std::mem::take(&mut state.selected) {
                if let Some(gone) = map.remove_hex(c) {
                    dungeon.encounters.retain(|e| e.home_room_id != gone.id);
                }
            }
        }
    }

    // Drawing
    state.cache.paint(&painter, &t, map, &dungeon.theme, false);
    if state.show_labels {
        draw_labels(&painter, map, &t, &dungeon.theme, |_| true);
    }
    draw_sheep(&painter, map, &t, &dungeon.theme, |_| true);
    draw_encounter_marks(&painter, map, &t, &dungeon.theme, &dungeon.encounters);
    if let Some(id) = &state.selected_path {
        if let Some(path) = map.paths.iter().find(|p| &p.id == id) {
            let pts: Vec<egui::Pos2> = path.hexes.iter().map(|&c| {
                let (x, y) = map.orientation.center(c);
                t.world_to_screen(egui::pos2(x, y))
            }).collect();
            painter.add(egui::Shape::line(pts, egui::Stroke::new(3.0_f32, COLOR_SELECTION.gamma_multiply(0.7))));
        }
    }
    for &c in &state.selected {
        outline(&painter, map, c, &t, egui::Stroke::new(2.5_f32, COLOR_SELECTION));
    }
    if let Some(c) = hover {
        let on_map = map.hex(c).is_some();
        // Off the map, only the terrain brush can add a hex there
        if on_map || state.tool == HexTool::Terrain {
            outline(&painter, map, c, &t, egui::Stroke::new(1.2_f32, COLOR_SELECTION.gamma_multiply(0.6)));
        }
    }
    if !state.drawing_path.is_empty() {
        let mut pts: Vec<egui::Pos2> = state.drawing_path.iter().map(|&c| {
            let (x, y) = map.orientation.center(c);
            t.world_to_screen(egui::pos2(x, y))
        }).collect();
        if let Some(c) = hover.filter(|c| map.hex(*c).is_some()) {
            let (x, y) = map.orientation.center(c);
            pts.push(t.world_to_screen(egui::pos2(x, y)));
        }
        for p in &pts {
            painter.circle_filled(*p, 3.0, COLOR_SELECTION);
        }
        painter.add(egui::Shape::line(pts, egui::Stroke::new(2.0_f32, COLOR_SELECTION)));
    }
    let party_at = if state.dragging_party { hover.or(map.party) } else { map.party };
    draw_party(&painter, map, &t, party_at);

    if let Some(c) = hover {
        if let Some(hex) = map.hex(c) {
            response.on_hover_text_at_pointer(hover_text(hex, &dungeon.encounters));
        }
    }
}

fn hover_text(hex: &Hex, encounters: &[Encounter]) -> String {
    let mut text = match hex.sheep {
        0 => format!("{} — {}", hex.name(), hex.terrain.label()),
        1 => format!("{} — {}, 1 sheep", hex.name(), hex.terrain.label()),
        n => format!("{} — {}, {} sheep", hex.name(), hex.terrain.label(), n),
    };
    for e in hex_encounters(hex, encounters) {
        text.push_str(&format!("\nEncounter: {}", e.name));
    }
    text
}

/// A hex's own encounters: always there, never rolled.
fn hex_encounters<'a>(hex: &'a Hex, encounters: &'a [Encounter]) -> impl Iterator<Item = &'a Encounter> + 'a {
    encounters.iter().filter(move |e| e.home_room_id == hex.id)
}

/// Crossed swords in the lower left of each hex with encounters of its own: the DM's
/// reminder, never shown to the players.
fn draw_encounter_marks(painter: &egui::Painter, map: &HexMap, t: &ViewTransform, theme: &Theme, encounters: &[Encounter]) {
    let [ir, ig, ib, _] = theme.wall_color;
    let ink = egui::Color32::from_rgb(ir, ig, ib);
    let [pr, pg, pb, _] = theme.floor_color;
    let paper = egui::Color32::from_rgba_unmultiplied(pr, pg, pb, 225);
    let u = (HEX_SIZE * 0.1 * t.zoom).max(4.0);
    let blade = egui::Stroke::new((u * 0.2).max(1.2), ink);
    for hex in map.hexes.iter().filter(|h| hex_encounters(h, encounters).next().is_some()) {
        let (cx, cy) = map.orientation.center(hex.coord);
        let at = t.world_to_screen(egui::pos2(cx - HEX_SIZE * 0.42, cy + HEX_SIZE * 0.3));
        painter.circle(at, u * 1.25, paper, egui::Stroke::new(1.0_f32, ink.gamma_multiply(0.6)));
        for dir in [1.0_f32, -1.0] {
            // Each sword runs corner to corner; a short guard crosses it near the hilt
            let tip = at + egui::vec2(dir * 0.75 * u, -0.75 * u);
            let pommel = at + egui::vec2(-dir * 0.75 * u, 0.75 * u);
            painter.line_segment([tip, pommel], blade);
            let guard = at + egui::vec2(-dir * 0.4 * u, 0.4 * u);
            let across = egui::vec2(0.3 * u, dir * 0.3 * u);
            painter.line_segment([guard - across, guard + across], blade);
        }
    }
}

/// The path whose line passes near a screen point.
fn path_near(map: &HexMap, t: &ViewTransform, p: egui::Pos2) -> Option<String> {
    map.paths.iter().find_map(|path| {
        let pts: Vec<egui::Pos2> = path.hexes.iter().map(|&c| {
            let (x, y) = map.orientation.center(c);
            t.world_to_screen(egui::pos2(x, y))
        }).collect();
        pts.windows(2)
            .any(|w| crate::util::point_to_segment_dist(p, w[0], w[1]) < 5.0)
            .then(|| path.id.clone())
    })
}

fn finish_path(map: &mut HexMap, state: &mut HexViewState) {
    let points = std::mem::take(&mut state.drawing_path);
    if points.len() >= 2 {
        let path = HexPath::through(state.path_kind, &points);
        state.selected_path = Some(path.id.clone());
        map.paths.push(path);
    }
}

/// The hex map editor's sidebar. `other_maps` are (id, name) of the campaign's other maps.
pub fn hex_sidebar(ui: &mut egui::Ui, dungeon: &mut Dungeon, state: &mut HexViewState, other_maps: &[(String, String)], monster_db: &MonsterDatabase) {
    let Some(map) = dungeon.hex_map.as_mut() else { return };
    ui.heading("Hex Map");
    egui::ComboBox::from_label("Hexes")
        .selected_text(map.orientation.label())
        .show_ui(ui, |ui| {
            for o in HexOrientation::ALL {
                ui.selectable_value(&mut map.orientation, o, o.label());
            }
        });
    ui.horizontal(|ui| {
        ui.label("Miles per hex:");
        ui.add(egui::DragValue::new(&mut map.miles_per_hex).range(0.1..=100.0).speed(0.1));
    });
    sheep_total(ui, map);
    ui.horizontal(|ui| {
        ui.label(format!("{} hexes", map.hexes.len()));
        if ui.button("Grow").on_hover_text(format!("Add a ring of {} round the edge", state.brush.label().to_lowercase())).clicked() {
            for c in map.rim() {
                map.ensure_hex(c, state.brush);
            }
        }
        if ui.button("Fit view").clicked() {
            let all: Vec<HexCoord> = map.hexes.iter().map(|h| h.coord).collect();
            fit_view(&mut state.view, map, &all, state.canvas_size);
        }
    });
    ui.checkbox(&mut state.show_labels, "Show labels");

    ui.separator();
    ui.horizontal_wrapped(|ui| {
        for tool in HexTool::ALL {
            ui.selectable_value(&mut state.tool, tool, tool.label());
        }
    });
    match state.tool {
        HexTool::Select => {
            ui.weak("Click a hex (Shift adds more). Drag the party's banner to move it.");
        }
        HexTool::Terrain => {
            ui.weak("Click or drag to paint. Painting next to the map adds hexes.");
            ui.horizontal_wrapped(|ui| {
                for t in HexTerrain::ALL {
                    ui.selectable_value(&mut state.brush, t, t.label());
                }
            });
        }
        HexTool::Landmark => {
            ui.weak("Click a hex to put this landmark in its middle.");
            ui.horizontal_wrapped(|ui| {
                ui.selectable_value(&mut state.landmark, None, "None");
                for f in HexFeature::ALL {
                    ui.selectable_value(&mut state.landmark, Some(f), f.label());
                }
            });
        }
        HexTool::Path => {
            ui.horizontal(|ui| {
                for k in HexPathKind::ALL {
                    ui.selectable_value(&mut state.path_kind, k, k.label());
                }
            });
            ui.weak("Click hexes in order (gaps fill in straight). Double-click, right-click or Enter finishes; Esc cancels.");
            if !state.drawing_path.is_empty() {
                ui.horizontal(|ui| {
                    if ui.button("Finish").clicked() {
                        finish_path(map, state);
                    }
                    if ui.button("Cancel").clicked() {
                        state.drawing_path.clear();
                    }
                });
            }
        }
        HexTool::Erase => {
            ui.weak("Click or drag to remove hexes.");
        }
    }

    // The selected hexes
    state.selected.retain(|c| map.hex(*c).is_some());
    if !state.selected.is_empty() {
        ui.separator();
        hex_details(ui, map, state, other_maps, true);
        if let [c] = state.selected[..] {
            if let Some(hex) = map.hex(c).cloned() {
                ui.add_space(4.0);
                own_encounters_ui(ui, &mut dungeon.encounters, &dungeon.custom_monsters, monster_db, &hex, state, false);
            }
        }
    }

    // The selected path
    if let Some(id) = state.selected_path.clone() {
        if let Some(i) = map.paths.iter().position(|p| p.id == id) {
            ui.separator();
            let path = &mut map.paths[i];
            ui.horizontal(|ui| {
                ui.label(format!("{} hexes long:", path.hexes.len()));
                for k in HexPathKind::ALL {
                    ui.selectable_value(&mut path.kind, k, k.label());
                }
            });
            if ui.button("Delete").clicked() {
                map.paths.remove(i);
                state.selected_path = None;
            }
        } else {
            state.selected_path = None;
        }
    }
    if !map.paths.is_empty() {
        ui.separator();
        ui.label("Rivers and roads:");
        for path in &map.paths {
            let name = format!("{} ({} hexes)", path.kind.label(), path.hexes.len());
            if ui.selectable_label(state.selected_path.as_ref() == Some(&path.id), name).clicked() {
                state.selected_path = Some(path.id.clone());
                state.selected.clear();
            }
        }
    }

    ui.separator();
    random_table_ui(ui, map, &mut dungeon.encounters, &dungeon.custom_monsters, monster_db);
}

const FIGHT_TIP: &str = "Copy it onto this hex's battle map (making one if it has none) and open it there; \
    the players stay on the hex map until you show them";

/// A hex's own encounters: to edit and delete while building, to fight at the table.
fn own_encounters_ui(
    ui: &mut egui::Ui,
    encounters: &mut Vec<Encounter>,
    custom: &[CustomMonster],
    db: &MonsterDatabase,
    hex: &Hex,
    state: &mut HexViewState,
    at_table: bool,
) {
    let mut remove = None;
    let mut any = false;
    for e in encounters.iter().filter(|e| e.home_room_id == hex.id) {
        if !any {
            ui.strong(format!("Encounters in {}", hex.name()));
            any = true;
        }
        ui.horizontal(|ui| {
            ui.label(&e.name);
            if at_table && ui.small_button("Fight").on_hover_text(FIGHT_TIP).clicked() {
                state.request = Some(HexRequest::Fight { encounter: e.id.clone(), hex: hex.coord });
            }
            if ui.small_button("Edit").clicked() {
                open_encounter_editor(ui.ctx(), &e.id);
            }
            if !at_table && ui.small_button("×").on_hover_text("Delete this encounter").clicked() {
                remove = Some(e.id.clone());
            }
        });
        ui.weak(monster_summary(e, db, custom));
    }
    if let Some(id) = remove {
        encounters.retain(|e| e.id != id);
    }
    if !at_table
        && ui.button("+ Encounter in this hex")
            .on_hover_text("Always here, never rolled: a den, a camp, a guard post")
            .clicked()
    {
        let e = Encounter::new(format!("{} encounter", hex.name()), hex.id.clone());
        open_encounter_editor(ui.ctx(), &e.id);
        encounters.push(e);
    }
}

/// The map's random encounter table, to fill in while building.
fn random_table_ui(ui: &mut egui::Ui, map: &mut HexMap, encounters: &mut Vec<Encounter>, custom: &[CustomMonster], db: &MonsterDatabase) {
    map.sync_random(encounters);
    let total = map.random_weight().max(1);
    egui::CollapsingHeader::new(format!("Random encounters ({})", map.random_encounters.len()))
        .id_salt("hex_random_encounters")
        .default_open(true)
        .show(ui, |ui| {
            ui.weak("Could turn up anywhere on the map; roll for one at the table. A higher weight comes up more often.");
            let mut remove = None;
            for entry in &mut map.random_encounters {
                let Some(e) = encounters.iter().find(|e| e.id == entry.encounter) else { continue };
                ui.horizontal(|ui| {
                    ui.add(egui::DragValue::new(&mut entry.weight).range(1..=100).speed(0.1)).on_hover_text("Weight");
                    ui.label(&e.name);
                    ui.weak(format!("{:.0}%", 100.0 * entry.weight as f32 / total as f32));
                    if ui.small_button("Edit").clicked() {
                        open_encounter_editor(ui.ctx(), &e.id);
                    }
                    if ui.small_button("×").on_hover_text("Delete this encounter").clicked() {
                        remove = Some(e.id.clone());
                    }
                });
                ui.weak(monster_summary(e, db, custom));
            }
            if let Some(id) = remove {
                encounters.retain(|e| e.id != id);
            }
            if ui.button("+ Random encounter").clicked() {
                let e = Encounter::new("Random encounter".into(), String::new());
                open_encounter_editor(ui.ctx(), &e.id);
                encounters.push(e);
            }
        });
    map.sync_random(encounters);
}

/// At the table: roll on the random table, and what came up.
fn random_roll_ui(
    ui: &mut egui::Ui,
    map: &mut HexMap,
    encounters: &[Encounter],
    custom: &[CustomMonster],
    db: &MonsterDatabase,
    state: &mut HexViewState,
) {
    map.sync_random(encounters);
    ui.strong("Random encounters");
    if map.random_encounters.is_empty() {
        ui.weak("None on this map yet: add them while editing it.");
        return;
    }
    // Where the party is; failing that, the selected hex
    let target = map.party.or(state.selected.first().copied()).filter(|c| map.hex(*c).is_some());
    ui.horizontal(|ui| {
        let roll = ui.add_enabled(target.is_some(), egui::Button::new(if map.rolled.is_some() { "Roll again" } else { "Roll" }))
            .on_hover_text("Roll on the map's table for the party's hex")
            .on_disabled_hover_text("Put the party on the map, or select a hex, first");
        if roll.clicked() {
            if let Some(c) = target {
                map.roll_random(c);
            }
        }
        ui.weak(format!("{} on the table", map.random_encounters.len()));
    });
    let Some(rolled) = map.rolled.clone() else { return };
    let Some(e) = encounters.iter().find(|e| e.id == rolled.encounter) else { return };
    let place = map.hex(rolled.hex).map_or_else(|| "a hex that's gone".to_string(), |h| h.name());
    ui.group(|ui| {
        ui.strong(format!("{} — in {}", e.name, place));
        ui.weak(monster_summary(e, db, custom));
        if !e.notes.trim().is_empty() {
            ui.label(e.notes.trim());
        }
        ui.horizontal(|ui| {
            if ui.button("Fight").on_hover_text(FIGHT_TIP).clicked() {
                state.request = Some(HexRequest::Fight { encounter: e.id.clone(), hex: rolled.hex });
            }
            if ui.button("Edit").clicked() {
                open_encounter_editor(ui.ctx(), &e.id);
            }
            if ui.button("Clear").on_hover_text("Nothing comes of it").clicked() {
                map.rolled = None;
            }
        });
    });
}

/// The map's sheep, and how many hexes they're spread over.
fn sheep_total(ui: &mut egui::Ui, map: &HexMap) {
    let total = map.total_sheep();
    if total > 0 {
        let hexes = map.hexes.iter().filter(|h| h.sheep > 0).count();
        ui.label(format!("{} sheep in {} {}", total, hexes, if hexes == 1 { "hex" } else { "hexes" }));
    }
}

/// What's in the selected hexes, and their battle map. Editable fields only when
/// `edit`, except the sheep, which are counted at the table too.
fn hex_details(ui: &mut egui::Ui, map: &mut HexMap, state: &mut HexViewState, other_maps: &[(String, String)], edit: bool) {
    let many = state.selected.len() > 1;
    let first = state.selected[0];
    if many {
        ui.label(format!("{} hexes selected", state.selected.len()));
    } else if let Some(hex) = map.hex_mut(first) {
        if edit {
            ui.horizontal(|ui| {
                ui.label("Label:");
                ui.text_edit_singleline(&mut hex.label);
            });
            ui.weak(format!("Hex {},{}", first.q, first.r));
        } else {
            ui.strong(hex.name());
            ui.label(match hex.feature {
                Some(f) => format!("{}, {}", hex.terrain.label(), f.label().to_lowercase()),
                None => hex.terrain.label().to_string(),
            });
        }
    }
    let selected = state.selected.clone();
    let each = |map: &mut HexMap, f: &dyn Fn(&mut Hex)| {
        for c in &selected {
            if let Some(h) = map.hex_mut(*c) {
                f(h);
            }
        }
    };
    let Some(hex) = map.hex(first).cloned() else { return };
    if edit {
        let mut terrain = hex.terrain;
        egui::ComboBox::from_label("Terrain").selected_text(terrain.label()).show_ui(ui, |ui| {
            for t in HexTerrain::ALL {
                ui.selectable_value(&mut terrain, t, t.label());
            }
        });
        if terrain != hex.terrain {
            each(map, &|h| h.terrain = terrain);
        }
        let mut feature = hex.feature;
        egui::ComboBox::from_label("Landmark").selected_text(feature.map_or("None", |f| f.label())).show_ui(ui, |ui| {
            ui.selectable_value(&mut feature, None, "None");
            for f in HexFeature::ALL {
                ui.selectable_value(&mut feature, Some(f), f.label());
            }
        });
        if feature != hex.feature {
            each(map, &|h| h.feature = feature);
        }
    }

    // Sheep: counted per hex; a selection of several shows their total
    if many {
        let total: u32 = selected.iter().filter_map(|c| map.hex(*c)).map(|h| h.sheep).sum();
        ui.label(format!("Sheep: {} in these hexes", total));
    } else if let Some(hex) = map.hex_mut(first) {
        ui.horizontal(|ui| {
            ui.label("Sheep:");
            if ui.small_button("−").clicked() {
                hex.sheep = hex.sheep.saturating_sub(1);
            }
            ui.add(egui::DragValue::new(&mut hex.sheep).range(0..=100_000).speed(0.2));
            if ui.small_button("+").clicked() {
                hex.sheep += 1;
            }
        });
    }

    // Battle map: shared by the selection if they all have the same one
    let all_same = selected.iter().all(|c| map.hex(*c).is_some_and(|h| h.battle_map == hex.battle_map));
    let linked = hex.battle_map.as_ref().filter(|_| all_same)
        .and_then(|id| other_maps.iter().find(|(mid, _)| mid == id));
    let mut battle = if all_same { hex.battle_map.clone() } else { Some(String::new()) };
    ui.horizontal(|ui| {
        ui.label("Battle map:");
        let text = match (&battle, linked) {
            (_, Some((_, name))) => name.as_str(),
            (Some(s), None) if s.is_empty() => "(mixed)",
            _ => "None",
        };
        egui::ComboBox::from_id_salt("hex_battle_map").selected_text(text).show_ui(ui, |ui| {
            ui.selectable_value(&mut battle, None, "None");
            for (id, name) in other_maps {
                ui.selectable_value(&mut battle, Some(id.clone()), name);
            }
        });
    });
    let changed = if all_same { battle != hex.battle_map } else { battle.as_deref() != Some("") };
    if changed {
        each(map, &|h| h.battle_map = battle.clone());
    }
    ui.horizontal(|ui| {
        if let Some((id, name)) = linked {
            if ui.button(format!("Open {}", name)).on_hover_text("Fight here: the players stay on the hex map until you show them this one").clicked() {
                state.request = Some(HexRequest::OpenMap(id.clone()));
            }
        }
        let tip = if many { "A new outdoor map for these hexes, linked to them" } else { "A new outdoor map for this hex, linked to it" };
        if ui.button("New battle map").on_hover_text(tip).clicked() {
            state.request = Some(HexRequest::CreateBattleMap);
        }
    });
}

// ------------------------------------------------------------------ at the table

/// The DM's hex map while presenting: everything, with unseen hexes shaded.
pub fn hex_presentation_view(ui: &mut egui::Ui, dungeon: &mut Dungeon, state: &mut HexViewState) {
    let (response, painter) = ui.allocate_painter(ui.available_size(), egui::Sense::click_and_drag());
    let rect = response.rect;
    paint_background(&painter, rect, &dungeon.theme);
    let Some(map) = dungeon.hex_map.as_mut() else { return };

    state.canvas_size = rect.size();
    if !state.fitted {
        let all: Vec<HexCoord> = map.hexes.iter().map(|h| h.coord).collect();
        fit_view(&mut state.view, map, &all, rect.size());
        state.fitted = true;
    }
    handle_pan_zoom(&response, &mut state.view);
    let t = ViewTransform::new(state.view.offset, state.view.zoom, rect);
    let hover = hex_under(map, &t, response.hover_pos()).filter(|c| map.hex(*c).is_some());

    if response.drag_started_by(egui::PointerButton::Primary) {
        let start = hex_under(map, &t, ui.input(|i| i.pointer.press_origin()));
        state.dragging_party = start.is_some() && start == map.party;
    }
    if response.dragged_by(egui::PointerButton::Primary) && !state.dragging_party {
        state.view.offset += response.drag_delta();
    }
    if state.dragging_party && response.drag_stopped() {
        if let Some(c) = hover {
            map.move_party(c);
            state.selected = vec![c];
        }
        state.dragging_party = false;
    }
    if response.clicked() {
        state.selected = hover.into_iter().collect();
    }
    if response.double_clicked() {
        if let Some(c) = hover {
            map.move_party(c);
        }
    }
    // Remember which hex was right-clicked, so the menu stays on it as the pointer moves
    if response.secondary_clicked() {
        state.menu_hex = hover;
        if let Some(c) = hover {
            state.selected = vec![c];
        }
    }

    state.cache.paint(&painter, &t, map, &dungeon.theme, false);
    // Darkest where the players haven't seen, lighter where they've only looked
    for hex in &map.hexes {
        let alpha = match map.sight(hex.coord) {
            HexSight::Hidden => 110,
            HexSight::Observed => 45,
            HexSight::Explored => continue,
        };
        painter.add(egui::Shape::convex_polygon(
            hex_screen(map, hex.coord, 1.0, &t),
            egui::Color32::from_rgba_unmultiplied(0, 0, 0, alpha),
            egui::Stroke::NONE,
        ));
    }
    draw_labels(&painter, map, &t, &dungeon.theme, |_| true);
    draw_sheep(&painter, map, &t, &dungeon.theme, |_| true);
    draw_encounter_marks(&painter, map, &t, &dungeon.theme, &dungeon.encounters);
    if let Some(hex) = hover.and_then(|c| map.hex(c)) {
        let sight = map.sight(hex.coord).label().to_lowercase();
        response.clone().on_hover_text_at_pointer(format!("{} ({})", hover_text(hex, &dungeon.encounters), sight));
    }
    for &c in &state.selected {
        outline(&painter, map, c, &t, egui::Stroke::new(2.5_f32, COLOR_SELECTION));
    }
    if let Some(c) = hover {
        outline(&painter, map, c, &t, egui::Stroke::new(1.2_f32, COLOR_SELECTION.gamma_multiply(0.6)));
    }
    let party_at = if state.dragging_party { hover.or(map.party) } else { map.party };
    draw_party(&painter, map, &t, party_at);

    response.context_menu(|ui| hex_menu(ui, map, &dungeon.encounters, state));
}

/// The right-click menu on a hex at the table.
fn hex_menu(ui: &mut egui::Ui, map: &mut HexMap, encounters: &[Encounter], state: &mut HexViewState) {
    let Some(c) = state.menu_hex else {
        ui.close_menu();
        return;
    };
    let Some(hex) = map.hex(c) else {
        ui.close_menu();
        return;
    };
    let sight = map.sight(c);
    ui.label(format!("{} — {} ({})", hex.name(), hex.terrain.label(), sight.label().to_lowercase()));
    let battle_map = hex.battle_map.clone();
    ui.separator();

    if map.party != Some(c) && ui.button("Move party here").clicked() {
        map.move_party(c);
        ui.close_menu();
    }
    if ui.button(format!("Observe from here ({} hexes)", state.lookout_radius))
        .on_hover_text("What can be seen from this hex, say from a watch tower; the sidebar sets how far")
        .clicked()
    {
        map.observe_from(c, state.lookout_radius);
        ui.close_menu();
    }
    ui.separator();

    for (to, text) in [(HexSight::Observed, "Observe"), (HexSight::Explored, "Explore"), (HexSight::Hidden, "Hide")] {
        if sight != to && ui.button(text).clicked() {
            map.set_sight(c, to);
            ui.close_menu();
        }
    }
    ui.separator();

    // Encounters: the hex's own, and a roll on the map's table
    let own: Vec<(String, String)> = map.hex(c)
        .map(|hex| hex_encounters(hex, encounters).map(|e| (e.id.clone(), e.name.clone())).collect())
        .unwrap_or_default();
    for (id, name) in own {
        if ui.button(format!("Fight: {}", name)).on_hover_text(FIGHT_TIP).clicked() {
            state.request = Some(HexRequest::Fight { encounter: id, hex: c });
            ui.close_menu();
        }
    }
    if !map.random_encounters.is_empty() && ui.button("Roll a random encounter here").clicked() {
        map.roll_random(c);
        ui.close_menu();
    }
    ui.separator();

    // Stays open, for counting several in or out
    if let Some(hex) = map.hex_mut(c) {
        ui.horizontal(|ui| {
            ui.label("Sheep:");
            if ui.small_button("−").clicked() {
                hex.sheep = hex.sheep.saturating_sub(1);
            }
            ui.add(egui::DragValue::new(&mut hex.sheep).range(0..=100_000).speed(0.2));
            if ui.small_button("+").clicked() {
                hex.sheep += 1;
            }
        });
    }
    if let Some(id) = battle_map {
        ui.separator();
        if ui.button("Open battle map").on_hover_text("The players stay on the hex map until you show them this one").clicked() {
            state.request = Some(HexRequest::OpenMap(id));
            ui.close_menu();
        }
    }
}

/// The DM's sidebar while presenting a hex map.
#[allow(clippy::too_many_arguments)]
pub fn hex_presentation_sidebar(
    ui: &mut egui::Ui,
    dungeon: &mut Dungeon,
    presentation: &mut crate::presentation::PresentationState,
    state: &mut HexViewState,
    player_view_state: &mut crate::ui::player_view::PlayerViewState,
    player_viewport_open: &mut bool,
    other_maps: &[(String, String)],
    monster_db: &MonsterDatabase,
) {
    let Some(map) = dungeon.hex_map.as_mut() else { return };
    ui.heading("Hex Map");
    let party_hex = map.party.and_then(|c| map.hex(c)).cloned();
    match &party_hex {
        Some(hex) => ui.label(format!("The party is in {} ({}).", hex.name(), hex.terrain.label().to_lowercase())),
        None => ui.label("The party isn't on the map yet: double-click a hex to put it there."),
    };
    if let Some(hex) = &party_hex {
        own_encounters_ui(ui, &mut dungeon.encounters, &dungeon.custom_monsters, monster_db, hex, state, true);
    }
    ui.weak(format!(
        "Double-click a hex or drag the banner to move the party; each hex is {} miles. \
         Hexes the party enters are explored, those round it observed (shaded lighter). \
         Right-click a hex for a menu: move the party, observe from it, hide, observe or explore it, count its sheep.",
        map.miles_per_hex
    ));
    ui.horizontal(|ui| {
        ui.label("Observe around the party:");
        ui.add(egui::Slider::new(&mut map.reveal_radius, 0..=3).suffix(" hexes"));
    });
    ui.horizontal(|ui| {
        if ui.button("Reveal all").on_hover_text("Observe every hex; explored ones stay explored").clicked() {
            map.revealed = map.hexes.iter().map(|h| h.coord).collect();
        }
        if ui.button("Hide all").clicked() {
            map.hide_all();
        }
    });
    sheep_total(ui, map);

    ui.separator();
    random_roll_ui(ui, map, &dungeon.encounters, &dungeon.custom_monsters, monster_db, state);

    state.selected.retain(|c| map.hex(*c).is_some());
    if let Some(&c) = state.selected.first() {
        ui.separator();
        state.selected.truncate(1);
        hex_details(ui, map, state, other_maps, false);
        if map.party != Some(c) && ui.button("Move party here").clicked() {
            map.move_party(c);
        }
        ui.horizontal(|ui| {
            ui.label("Players:");
            let mut sight = map.sight(c);
            for s in HexSight::ALL {
                ui.selectable_value(&mut sight, s, s.label());
            }
            if sight != map.sight(c) {
                map.set_sight(c, sight);
            }
        });
        ui.horizontal(|ui| {
            if ui.button("Observe from here").on_hover_text("What can be seen from this hex, say from a watch tower or hilltop").clicked() {
                map.observe_from(c, state.lookout_radius);
            }
            ui.add(egui::Slider::new(&mut state.lookout_radius, 1..=6).suffix(" hexes"));
        });
        // The party's hex shows its encounters above already
        if map.party != Some(c) {
            if let Some(hex) = map.hex(c).cloned() {
                own_encounters_ui(ui, &mut dungeon.encounters, &dungeon.custom_monsters, monster_db, &hex, state, true);
            }
        }
    }

    ui.separator();
    ui.heading("Player View");
    if ui.button(if *player_viewport_open { "Close Player Window" } else { "Open Player Window" }).clicked() {
        *player_viewport_open = !*player_viewport_open;
    }
    ui.checkbox(&mut presentation.show_labels_player, "Show labels to players");
    ui.checkbox(&mut presentation.show_sheep_player, "Show sheep to players");
    ui.checkbox(&mut player_view_state.locked, "Lock player view (no scroll/pan)");
    ui.horizontal(|ui| {
        if ui.button("Fit to what they've seen").clicked() {
            player_view_state.hex_fitted = false;
        }
        if ui.button("Sync to DM view").clicked() {
            let cx = (state.canvas_size.x / 2.0 - state.view.offset.x) / state.view.zoom;
            let cy = (state.canvas_size.y / 2.0 - state.view.offset.y) / state.view.zoom;
            player_view_state.view.zoom = state.view.zoom;
            player_view_state.view.center_on(cx, cy, player_view_state.canvas_size);
            player_view_state.hex_fitted = true;
        }
    });
}

/// The players' window on a hex map: the hexes they've seen and the party.
pub fn player_hex_view(
    response: &egui::Response,
    painter: &egui::Painter,
    dungeon: &Dungeon,
    map: &HexMap,
    show_labels: bool,
    show_sheep: bool,
    state: &mut crate::ui::player_view::PlayerViewState,
) {
    let rect = response.rect;
    let revealed: Vec<HexCoord> = map.revealed.iter().copied().filter(|c| map.hex(*c).is_some()).collect();
    if !state.hex_fitted && !revealed.is_empty() {
        fit_view(&mut state.view, map, &revealed, rect.size());
        state.hex_fitted = true;
    }
    if !state.locked {
        handle_pan_zoom(response, &mut state.view);
    }
    let rotation = state.map_rotation as f32 * std::f32::consts::FRAC_PI_2;
    let t = ViewTransform::new(state.view.offset, state.view.zoom, rect).with_rotation(rotation);
    state.hex_cache.paint(painter, &t, map, &dungeon.theme, true);
    if show_labels {
        draw_labels(painter, map, &t, &dungeon.theme, |c| map.is_revealed(c));
    }
    if show_sheep {
        draw_sheep(painter, map, &t, &dungeon.theme, |c| map.is_revealed(c));
    }
    draw_party(painter, map, &t, map.party);
}

/// The players' view of a hex map as a PNG, for the web view.
pub fn player_hex_png(dungeon: &Dungeon) -> Option<impl FnOnce() -> Option<Vec<u8>> + Send + 'static> {
    let map = dungeon.hex_map.clone()?;
    let theme = dungeon.theme.clone();
    Some(move || {
        let revealed: Vec<HexCoord> = map.revealed.iter().copied().filter(|c| map.hex(*c).is_some()).collect();
        let (x0, y0, x1, y1) = map.extents(&revealed)?;
        render_png(&map, &theme, true, (x0, y0, x1, y1), 2.0).map(|(png, _)| png)
    })
}

/// Draw a hex map to a PNG covering `extents` (world pixels) at `scale` image pixels
/// per world pixel. Returns the bytes and the world position of the top-left pixel.
pub fn render_png(map: &HexMap, theme: &Theme, players: bool, (x0, y0, x1, y1): (f32, f32, f32, f32), scale: f32) -> Option<(Vec<u8>, (f32, f32))> {
    let margin = HEX_SIZE * 0.5;
    let (ox, oy) = (x0 - margin, y0 - margin);
    let (w, h) = (x1 - x0 + margin * 2.0, y1 - y0 + margin * 2.0);
    let mut r = crate::render::ImageRenderer::new((w * scale) as u32, (h * scale) as u32, scale);
    r.offset_x = ox;
    r.offset_y = oy;
    crate::render::traits::MapRenderer::fill_rect(&mut r, ox, oy, w, h, theme.bg_color);
    crate::render::hex::render_hex_map(&mut r, map, theme, players);
    if let Some(c) = map.party {
        let (cx, cy) = map.orientation.center(c);
        let mut sink = crate::render::decor::MapRendererSink { renderer: &mut r };
        crate::render::hex::draw_party_token(&mut sink, cx + HEX_SIZE * 0.42, cy - HEX_SIZE * 0.36, HEX_SIZE * 0.22);
    }
    let mut png = std::io::Cursor::new(Vec::new());
    r.image.write_to(&mut png, image::ImageFormat::Png).ok()?;
    Some((png.into_inner(), (ox, oy)))
}
