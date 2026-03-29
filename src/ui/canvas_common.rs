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
