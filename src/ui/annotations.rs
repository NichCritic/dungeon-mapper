use crate::model::Annotation;

const PIN_RADIUS: f32 = 8.0;
const PIN_COLOR_OPEN: egui::Color32 = egui::Color32::from_rgb(255, 80, 80);
const PIN_COLOR_RESOLVED: egui::Color32 = egui::Color32::from_rgb(80, 200, 80);
const PIN_COLOR_COMPOSE: egui::Color32 = egui::Color32::from_rgb(255, 200, 80);
const PIN_STEM_LEN: f32 = 16.0;
const OVERLAY_DIM: egui::Color32 = egui::Color32::from_rgba_premultiplied(0, 0, 0, 140);

/// Transient state for the annotation mode overlay.
pub struct AnnotationModeState {
    /// Annotation currently being composed (after clicking to place).
    pub composing: Option<ComposingAnnotation>,
    /// ID of annotation whose detail popup is open.
    pub viewing: Option<String>,
    /// Whether to show resolved annotations.
    pub show_resolved: bool,
    /// Panel rects collected during the current frame, used for spotlight.
    pub panel_rects: Vec<egui::Rect>,
}

pub struct ComposingAnnotation {
    /// Screen-fraction X (0.0 – 1.0).
    pub frac_x: f32,
    /// Screen-fraction Y (0.0 – 1.0).
    pub frac_y: f32,
    pub text: String,
}

impl Default for AnnotationModeState {
    fn default() -> Self {
        Self {
            composing: None,
            viewing: None,
            show_resolved: true,
            panel_rects: Vec::new(),
        }
    }
}

/// Result of running the annotation overlay for one frame.
pub struct OverlayResult {
    pub new_annotation: Option<Annotation>,
    pub annotations_changed: bool,
}

/// Draw the full-screen annotation overlay.
/// This should be called at the very end of the frame, after all panels are drawn.
pub fn annotation_overlay(
    ctx: &egui::Context,
    annotations: &mut Vec<Annotation>,
    state: &mut AnnotationModeState,
    current_view: &str,
    nearest_room_fn: &dyn Fn(f32, f32) -> Option<String>,
) -> OverlayResult {
    let mut result = OverlayResult {
        new_annotation: None,
        annotations_changed: false,
    };

    let screen_rect = ctx.screen_rect();

    // --- 1. Dimming overlay with spotlight cutout ---
    let pointer_pos = ctx.pointer_hover_pos();
    let highlight_rect = pointer_pos.and_then(|pos| {
        find_containing_panel(pos, &state.panel_rects)
    });

    let overlay_painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Middle,
        egui::Id::new("ann_dim_layer"),
    ));

    if let Some(highlight) = highlight_rect {
        // Paint 4 dim rects around the highlighted panel
        // Top
        if highlight.min.y > screen_rect.min.y {
            overlay_painter.rect_filled(
                egui::Rect::from_min_max(screen_rect.min, egui::pos2(screen_rect.max.x, highlight.min.y)),
                0.0, OVERLAY_DIM,
            );
        }
        // Bottom
        if highlight.max.y < screen_rect.max.y {
            overlay_painter.rect_filled(
                egui::Rect::from_min_max(egui::pos2(screen_rect.min.x, highlight.max.y), screen_rect.max),
                0.0, OVERLAY_DIM,
            );
        }
        // Left (between top and bottom strips)
        if highlight.min.x > screen_rect.min.x {
            overlay_painter.rect_filled(
                egui::Rect::from_min_max(
                    egui::pos2(screen_rect.min.x, highlight.min.y),
                    egui::pos2(highlight.min.x, highlight.max.y),
                ),
                0.0, OVERLAY_DIM,
            );
        }
        // Right
        if highlight.max.x < screen_rect.max.x {
            overlay_painter.rect_filled(
                egui::Rect::from_min_max(
                    egui::pos2(highlight.max.x, highlight.min.y),
                    egui::pos2(screen_rect.max.x, highlight.max.y),
                ),
                0.0, OVERLAY_DIM,
            );
        }
    } else {
        // No panel under cursor — dim everything
        overlay_painter.rect_filled(screen_rect, 0.0, OVERLAY_DIM);
    }

    // --- 2. Draw annotation pins (on a Foreground layer so they're above the dim) ---
    let pin_painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::new("ann_pin_layer"),
    ));

    for ann in annotations.iter() {
        if ann.view != current_view {
            continue;
        }
        if ann.resolved && !state.show_resolved {
            continue;
        }
        let screen_pos = frac_to_screen(ann.world_x, ann.world_y, screen_rect);
        let color = if ann.resolved { PIN_COLOR_RESOLVED } else { PIN_COLOR_OPEN };
        draw_pin(&pin_painter, screen_pos, color);
    }

    // Draw compose pin if composing
    if let Some(composing) = &state.composing {
        let screen_pos = frac_to_screen(composing.frac_x, composing.frac_y, screen_rect);
        draw_pin(&pin_painter, screen_pos, PIN_COLOR_COMPOSE);
    }

    // --- 3. Interaction area (invisible, over everything) ---
    egui::Area::new(egui::Id::new("ann_interact"))
        .fixed_pos(screen_rect.min)
        .order(egui::Order::Foreground)
        .interactable(true)
        .show(ctx, |ui| {
            let (response, _painter) = ui.allocate_painter(
                screen_rect.size(),
                egui::Sense::click(),
            );

            // Click handling
            if response.clicked() {
                if let Some(pos) = response.interact_pointer_pos() {
                    let (fx, fy) = screen_to_frac(pos, screen_rect);

                    // Check if clicking on an existing pin
                    let mut hit_ann_id: Option<String> = None;
                    for ann in annotations.iter() {
                        if ann.view != current_view {
                            continue;
                        }
                        if ann.resolved && !state.show_resolved {
                            continue;
                        }
                        let pin_pos = frac_to_screen(ann.world_x, ann.world_y, screen_rect);
                        let head_center = pin_head_center(pin_pos);
                        if head_center.distance(pos) < PIN_RADIUS + 6.0
                            || pin_pos.distance(pos) < PIN_RADIUS + 6.0
                        {
                            hit_ann_id = Some(ann.id.clone());
                            break;
                        }
                    }

                    if let Some(id) = hit_ann_id {
                        // Toggle viewing
                        if state.viewing.as_deref() == Some(&id) {
                            state.viewing = None;
                        } else {
                            state.viewing = Some(id);
                            state.composing = None;
                        }
                    } else if state.composing.is_some() {
                        // Cancel composing on click elsewhere
                        state.composing = None;
                    } else {
                        // Start composing at this position
                        state.composing = Some(ComposingAnnotation {
                            frac_x: fx,
                            frac_y: fy,
                            text: String::new(),
                        });
                        state.viewing = None;
                    }
                }
            }
        });

    // --- 4. Detail popup for viewed annotation ---
    if let Some(viewing_id) = &state.viewing.clone() {
        if let Some(ann) = annotations.iter().find(|a| a.id == *viewing_id) {
            let screen_pos = frac_to_screen(ann.world_x, ann.world_y, screen_rect);
            let popup_pos = egui::pos2(screen_pos.x + 14.0, screen_pos.y - 60.0);
            let ann_id = ann.id.clone();
            let ann_resolved = ann.resolved;

            let mut do_resolve = false;
            let mut do_reopen = false;
            let mut do_delete = false;

            egui::Area::new(egui::Id::new("ann_detail_popup"))
                .fixed_pos(popup_pos)
                .order(egui::Order::Tooltip)
                .show(ctx, |ui| {
                    egui::Frame::popup(ui.style()).show(ui, |ui| {
                        ui.set_max_width(300.0);
                        ui.horizontal(|ui| {
                            let (status, status_color) = if ann_resolved {
                                ("RESOLVED", PIN_COLOR_RESOLVED)
                            } else {
                                ("OPEN", PIN_COLOR_OPEN)
                            };
                            ui.colored_label(status_color, status);
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if ui.small_button("x").clicked() {
                                    state.viewing = None;
                                }
                            });
                        });
                        ui.label(&ann.text);
                        ui.add_space(4.0);
                        if let Some(room_id) = &ann.room_id {
                            ui.label(egui::RichText::new(format!("Room: {}", room_id)).small().weak());
                        }
                        ui.label(egui::RichText::new(format!("View: {} | {}", ann.view, ann.created_at)).small().weak());
                        ui.add_space(4.0);
                        ui.horizontal(|ui| {
                            if ann_resolved {
                                if ui.button("Reopen").clicked() { do_reopen = true; }
                            } else if ui.button("Resolve").clicked() { do_resolve = true; }
                            if ui.button("Delete").clicked() { do_delete = true; }
                        });
                    });
                });

            if do_resolve {
                if let Some(a) = annotations.iter_mut().find(|a| a.id == ann_id) {
                    a.resolved = true;
                    result.annotations_changed = true;
                }
            }
            if do_reopen {
                if let Some(a) = annotations.iter_mut().find(|a| a.id == ann_id) {
                    a.resolved = false;
                    result.annotations_changed = true;
                }
            }
            if do_delete {
                annotations.retain(|a| a.id != ann_id);
                state.viewing = None;
                result.annotations_changed = true;
            }
        } else {
            state.viewing = None;
        }
    }

    // --- 5. Compose popup ---
    if state.composing.is_some() {
        let (fx, fy) = {
            let c = state.composing.as_ref().unwrap();
            (c.frac_x, c.frac_y)
        };
        let screen_pos = frac_to_screen(fx, fy, screen_rect);
        let popup_pos = egui::pos2(screen_pos.x + 14.0, screen_pos.y - 40.0);

        let mut do_add = false;
        let mut do_cancel = false;

        egui::Area::new(egui::Id::new("ann_compose_popup"))
            .fixed_pos(popup_pos)
            .order(egui::Order::Tooltip)
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.set_max_width(300.0);
                    ui.label(egui::RichText::new("New Issue").strong().color(PIN_COLOR_COMPOSE));
                    let composing = state.composing.as_mut().unwrap();
                    let te = ui.text_edit_multiline(&mut composing.text);
                    if composing.text.is_empty() {
                        te.request_focus();
                    }
                    ui.horizontal(|ui| {
                        if ui.button("Add").clicked() && !composing.text.trim().is_empty() {
                            do_add = true;
                        }
                        if ui.button("Cancel").clicked() {
                            do_cancel = true;
                        }
                    });
                });
            });

        if do_add {
            let composing = state.composing.take().unwrap();
            let nearest_room = nearest_room_fn(fx, fy);
            result.new_annotation = Some(Annotation::new(
                composing.text.trim().to_string(),
                fx,
                fy,
                current_view.to_string(),
                nearest_room,
            ));
            result.annotations_changed = true;
        } else if do_cancel {
            state.composing = None;
        }
    }

    // --- 6. Small floating issue count badge (top-right, always visible) ---
    let view_annotations: Vec<_> = annotations.iter().filter(|a| a.view == current_view).collect();
    let open_count = view_annotations.iter().filter(|a| !a.resolved).count();
    let total_count = view_annotations.len();
    egui::Area::new(egui::Id::new("ann_badge"))
        .fixed_pos(egui::pos2(screen_rect.max.x - 220.0, screen_rect.min.y + 4.0))
        .order(egui::Order::Tooltip)
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.colored_label(PIN_COLOR_COMPOSE, format!(
                    "ANNOTATE (F7) | {} open / {} total",
                    open_count, total_count,
                ));
                ui.checkbox(&mut state.show_resolved, "resolved");
            });
        });

    result
}

// --- Helpers ---

fn frac_to_screen(fx: f32, fy: f32, screen_rect: egui::Rect) -> egui::Pos2 {
    egui::pos2(
        screen_rect.min.x + fx * screen_rect.width(),
        screen_rect.min.y + fy * screen_rect.height(),
    )
}

fn screen_to_frac(pos: egui::Pos2, screen_rect: egui::Rect) -> (f32, f32) {
    (
        (pos.x - screen_rect.min.x) / screen_rect.width(),
        (pos.y - screen_rect.min.y) / screen_rect.height(),
    )
}

fn pin_head_center(base: egui::Pos2) -> egui::Pos2 {
    egui::pos2(base.x, base.y - PIN_STEM_LEN - PIN_RADIUS)
}

fn draw_pin(painter: &egui::Painter, base: egui::Pos2, color: egui::Color32) {
    let stem_len = PIN_STEM_LEN;
    let radius = PIN_RADIUS;

    // Stem
    painter.line_segment(
        [base, egui::pos2(base.x, base.y - stem_len + radius * 0.3)],
        egui::Stroke::new(2.0, color),
    );

    // Circle head
    let head_center = pin_head_center(base);
    painter.circle_filled(head_center, radius, color);
    painter.circle_stroke(head_center, radius, egui::Stroke::new(1.0, egui::Color32::BLACK));

    // Exclamation mark
    let font = egui::FontId::monospace(radius * 1.2);
    painter.text(head_center, egui::Align2::CENTER_CENTER, "!", font, egui::Color32::WHITE);
}

fn find_containing_panel(pos: egui::Pos2, panels: &[egui::Rect]) -> Option<egui::Rect> {
    panels.iter().find(|r| r.contains(pos)).copied()
}
