//! The hex map: editing it (terrain, landmarks, rivers and roads, battle maps), and
//! playing on it at the table (the party's token and what the players have seen).

use crate::model::*;
use crate::render::decor::PainterSink;
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
                    map.remove_hex(c);
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
                map.remove_hex(c);
            }
        }
    }

    // Drawing
    state.cache.paint(&painter, &t, map, &dungeon.theme, false);
    if state.show_labels {
        draw_labels(&painter, map, &t, &dungeon.theme, |_| true);
    }
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
            response.on_hover_text_at_pointer(format!("{} — {}", hex.name(), hex.terrain.label()));
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
pub fn hex_sidebar(ui: &mut egui::Ui, dungeon: &mut Dungeon, state: &mut HexViewState, other_maps: &[(String, String)]) {
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
}

/// What's in the selected hexes, and their battle map. Editable fields only when `edit`.
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
    if response.secondary_clicked() {
        if let Some(c) = hover {
            if !map.revealed.remove(&c) {
                map.revealed.insert(c);
            }
        }
    }

    state.cache.paint(&painter, &t, map, &dungeon.theme, false);
    // What the players haven't seen
    for hex in &map.hexes {
        if !map.is_revealed(hex.coord) {
            painter.add(egui::Shape::convex_polygon(
                hex_screen(map, hex.coord, 1.0, &t),
                egui::Color32::from_rgba_unmultiplied(0, 0, 0, 110),
                egui::Stroke::NONE,
            ));
        }
    }
    draw_labels(&painter, map, &t, &dungeon.theme, |_| true);
    for &c in &state.selected {
        outline(&painter, map, c, &t, egui::Stroke::new(2.5_f32, COLOR_SELECTION));
    }
    if let Some(c) = hover {
        outline(&painter, map, c, &t, egui::Stroke::new(1.2_f32, COLOR_SELECTION.gamma_multiply(0.6)));
    }
    let party_at = if state.dragging_party { hover.or(map.party) } else { map.party };
    draw_party(&painter, map, &t, party_at);
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
) {
    let Some(map) = dungeon.hex_map.as_mut() else { return };
    ui.heading("Hex Map");
    match map.party.and_then(|c| map.hex(c)) {
        Some(hex) => ui.label(format!("The party is in {} ({}).", hex.name(), hex.terrain.label().to_lowercase())),
        None => ui.label("The party isn't on the map yet: double-click a hex to put it there."),
    };
    ui.weak(format!("Double-click a hex or drag the banner to move the party; each hex is {} miles. Right-click a hex to reveal or hide it.", map.miles_per_hex));
    ui.horizontal(|ui| {
        ui.label("Reveal around the party:");
        ui.add(egui::Slider::new(&mut map.reveal_radius, 0..=3).suffix(" hexes"));
    });
    ui.horizontal(|ui| {
        if ui.button("Reveal all").clicked() {
            map.revealed = map.hexes.iter().map(|h| h.coord).collect();
        }
        if ui.button("Hide all").clicked() {
            map.revealed.clear();
        }
    });

    state.selected.retain(|c| map.hex(*c).is_some());
    if let Some(&c) = state.selected.first() {
        ui.separator();
        state.selected.truncate(1);
        hex_details(ui, map, state, other_maps, false);
        ui.horizontal(|ui| {
            if map.party != Some(c) && ui.button("Move party here").clicked() {
                map.move_party(c);
            }
            let revealed = map.is_revealed(c);
            if ui.button(if revealed { "Hide from players" } else { "Reveal to players" }).clicked() {
                if revealed {
                    map.revealed.remove(&c);
                } else {
                    map.revealed.insert(c);
                }
            }
        });
    }

    ui.separator();
    ui.heading("Player View");
    if ui.button(if *player_viewport_open { "Close Player Window" } else { "Open Player Window" }).clicked() {
        *player_viewport_open = !*player_viewport_open;
    }
    ui.checkbox(&mut presentation.show_labels_player, "Show labels to players");
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
