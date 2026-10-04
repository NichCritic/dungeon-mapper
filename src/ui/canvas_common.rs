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

/// The "Rendering ..." placard shown while a view's map render builds; keeps
/// repainting so the finished render shows up.
pub fn paint_render_pending(ui: &egui::Ui, painter: &egui::Painter, rect: egui::Rect, what: &str) {
    let spinner_rect = egui::Rect::from_center_size(rect.center(), egui::vec2(200.0, 40.0));
    painter.rect_filled(spinner_rect, 8.0, egui::Color32::from_rgba_unmultiplied(0, 0, 0, 180));
    painter.text(
        spinner_rect.center(),
        egui::Align2::CENTER_CENTER,
        format!("Rendering {}...", what),
        egui::FontId::proportional(14.0),
        egui::Color32::WHITE,
    );
    ui.ctx().request_repaint();
}

/// A room's outline on screen (turned with the room when rotated).
pub fn room_screen_outline(rl: &crate::model::RoomLayout, transform: &crate::util::ViewTransform) -> Vec<egui::Pos2> {
    rl.corners().iter()
        .map(|&(x, y)| transform.world_to_screen(egui::pos2(x * crate::util::GRID_PX, y * crate::util::GRID_PX)))
        .collect()
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
    // Width grows with the prefix, so binary search for the longest prefix that fits
    // (one layout per step instead of one per character).
    let char_indices: Vec<usize> = text.char_indices().map(|(i, _)| i).collect();
    let fits = |end: usize| {
        let candidate = format!("{}...", &text[..end]);
        painter.layout_no_wrap(candidate, font.clone(), egui::Color32::WHITE).size().x <= max_width
    };
    // Invariant: prefixes ending before char_indices[lo] fit; from char_indices[hi] on they don't.
    let (mut lo, mut hi) = (0, char_indices.len());
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        if fits(char_indices[mid - 1]) { lo = mid; } else { hi = mid - 1; }
    }
    if lo == 0 {
        return "...".to_string();
    }
    format!("{}...", &text[..char_indices[lo - 1]])
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
        let trimmed = text.trim();
        if trimmed.is_empty() || trimmed == "-" {
            *value = 0;
            return true;
        }
        if let Ok(v) = trimmed.parse::<i32>() {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_to_fit_keeps_longest_prefix_that_fits() {
        let ctx = egui::Context::default();
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            let painter = ctx.layer_painter(egui::LayerId::background());
            let font = egui::FontId::proportional(12.0);
            let width = |s: &str| painter.layout_no_wrap(s.to_string(), font.clone(), egui::Color32::WHITE).size().x;
            let text = "Hall of the Mountain Kïng";
            // Naive reference: drop characters from the end until it fits
            let naive = |max: f32| {
                if width(text) <= max { return text.to_string(); }
                let ends: Vec<usize> = text.char_indices().map(|(i, _)| i).collect();
                ends.iter().rev().map(|&e| format!("{}...", &text[..e])).find(|c| width(c) <= max)
                    .unwrap_or_else(|| "...".to_string())
            };
            for max in [0.0, 5.0, 20.0, 40.0, 60.0, 90.0, 120.0, 1000.0] {
                assert_eq!(truncate_to_fit(&painter, text, &font, max), naive(max), "max_width {max}");
            }
        });
    }
}
