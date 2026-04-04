// --- Shared colors ---

pub const COLOR_SELECTION: egui::Color32 = egui::Color32::from_rgb(100, 200, 255);
pub const COLOR_GRAPH_BG: egui::Color32 = egui::Color32::from_rgb(35, 35, 40);
pub const COLOR_SPATIAL_BG: egui::Color32 = egui::Color32::from_rgb(40, 40, 45);
pub const COLOR_PLACEHOLDER_TEXT: egui::Color32 = egui::Color32::from_rgb(150, 150, 150);

// --- Pan/zoom state ---

/// Shared pan/zoom state for canvas views
#[derive(Clone, Debug)]
pub struct ViewState {
    pub offset: egui::Vec2,
    pub zoom: f32,
}

impl Default for ViewState {
    fn default() -> Self {
        Self {
            offset: egui::Vec2::ZERO,
            zoom: 1.0,
        }
    }
}

impl ViewState {
    /// Set offset so that `world_pos` (in world pixel coords) appears at the
    /// center of a canvas of the given size.
    pub fn center_on(&mut self, world_x: f32, world_y: f32, canvas_size: egui::Vec2) {
        self.offset = egui::vec2(
            canvas_size.x / 2.0 - world_x * self.zoom,
            canvas_size.y / 2.0 - world_y * self.zoom,
        );
    }
}

/// Handle pan (middle-click drag) and zoom (scroll) on a canvas response
pub fn handle_pan_zoom(response: &egui::Response, view: &mut ViewState) {
    // Pan with middle mouse drag
    if response.dragged_by(egui::PointerButton::Middle) {
        view.offset += response.drag_delta();
    }

    // Zoom with scroll wheel
    if response.hovered() {
        let scroll = response.ctx.input(|i| i.smooth_scroll_delta.y);
        if scroll != 0.0 {
            let zoom_factor = 1.0 + scroll * 0.002;
            let new_zoom = (view.zoom * zoom_factor).clamp(0.1, 10.0);

            // Zoom toward the pointer position
            if let Some(pointer) = response.hover_pos() {
                let canvas_pos = pointer - response.rect.min.to_vec2();
                let world_before = (canvas_pos - view.offset) / view.zoom;
                view.zoom = new_zoom;
                view.offset = canvas_pos - world_before * view.zoom;
            } else {
                view.zoom = new_zoom;
            }
        }
    }
}

// --- Shared drawing helpers ---

/// Draw a dashed line between two screen-space points.
pub fn draw_dashed_line(
    painter: &egui::Painter,
    from: egui::Pos2,
    to: egui::Pos2,
    stroke: egui::Stroke,
    dash_len: f32,
    gap_len: f32,
) {
    let dir = to - from;
    let total_len = dir.length();
    if total_len < 1.0 {
        return;
    }
    let dir_norm = dir / total_len;
    let mut d = 0.0;
    while d < total_len {
        let seg_start = from + dir_norm * d;
        let seg_end = from + dir_norm * (d + dash_len).min(total_len);
        painter.line_segment([seg_start, seg_end], stroke);
        d += dash_len + gap_len;
    }
}

/// Truncate text with "..." if it exceeds `max_width` pixels.
pub fn truncate_to_fit(
    painter: &egui::Painter,
    text: &str,
    font: &egui::FontId,
    max_width: f32,
) -> String {
    let galley = painter.layout_no_wrap(text.to_string(), font.clone(), egui::Color32::WHITE);
    if galley.size().x <= max_width {
        return text.to_string();
    }
    let char_indices: Vec<usize> = text.char_indices().map(|(i, _)| i).collect();
    for &end in char_indices.iter().rev() {
        let candidate = format!("{}...", &text[..end]);
        let g = painter.layout_no_wrap(candidate.clone(), font.clone(), egui::Color32::WHITE);
        if g.size().x <= max_width {
            return candidate;
        }
    }
    "...".to_string()
}

/// Draw a filled arrow head pointing from `from` toward `to`.
pub fn draw_arrow_head(painter: &egui::Painter, from: egui::Pos2, to: egui::Pos2, color: egui::Color32) {
    let dir = (to - from).normalized();
    let perp = egui::vec2(-dir.y, dir.x);
    let arrow_size = 10.0;

    let tip = to;
    let left = tip - dir * arrow_size + perp * arrow_size * 0.5;
    let right = tip - dir * arrow_size - perp * arrow_size * 0.5;

    painter.add(egui::Shape::convex_polygon(
        vec![tip, left, right],
        color,
        egui::Stroke::NONE,
    ));
}

// --- Numeric text input helpers ---

/// Select all text in a TextEdit when it gains focus.
fn select_all_on_focus(ui: &egui::Ui, response: &egui::Response, text: &str) {
    if response.gained_focus() {
        if let Some(mut state) = egui::TextEdit::load_state(ui.ctx(), response.id) {
            let range = egui::text::CCursorRange::two(
                egui::text::CCursor::new(0),
                egui::text::CCursor::new(text.len()),
            );
            state.cursor.set_char_range(Some(range));
            state.store(ui.ctx(), response.id);
        }
    }
}

/// A small numeric text input. Returns true if the value changed.
pub fn num_input_u32(ui: &mut egui::Ui, value: &mut u32, width: f32) -> bool {
    let mut text = value.to_string();
    let response = ui.add(egui::TextEdit::singleline(&mut text).desired_width(width));
    select_all_on_focus(ui, &response, &text);
    if response.changed() {
        if let Ok(v) = text.parse::<u32>() {
            *value = v;
            return true;
        }
    }
    false
}

/// A small numeric text input for i32. Returns true if the value changed.
pub fn num_input_i32(ui: &mut egui::Ui, value: &mut i32, width: f32) -> bool {
    let mut text = value.to_string();
    let response = ui.add(egui::TextEdit::singleline(&mut text).desired_width(width));
    select_all_on_focus(ui, &response, &text);
    if response.changed() {
        if let Ok(v) = text.parse::<i32>() {
            *value = v;
            return true;
        }
    }
    false
}

/// A small numeric text input for f32. Returns true if the value changed.
pub fn num_input_f32(ui: &mut egui::Ui, value: &mut f32, width: f32) -> bool {
    let mut text = format!("{:.1}", value);
    let response = ui.add(egui::TextEdit::singleline(&mut text).desired_width(width));
    select_all_on_focus(ui, &response, &text);
    if response.changed() {
        if let Ok(v) = text.parse::<f32>() {
            *value = v;
            return true;
        }
    }
    false
}

/// A small numeric text input for u16. Returns true if the value changed.
pub fn num_input_u16(ui: &mut egui::Ui, value: &mut u16, width: f32) -> bool {
    let mut text = value.to_string();
    let response = ui.add(egui::TextEdit::singleline(&mut text).desired_width(width));
    select_all_on_focus(ui, &response, &text);
    if response.changed() {
        if let Ok(v) = text.parse::<u16>() {
            *value = v;
            return true;
        }
    }
    false
}
