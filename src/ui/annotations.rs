use crate::model::Annotation;
use crate::util::ViewTransform;

const PIN_RADIUS: f32 = 8.0;
const PIN_COLOR_OPEN: egui::Color32 = egui::Color32::from_rgb(255, 80, 80);
const PIN_COLOR_RESOLVED: egui::Color32 = egui::Color32::from_rgb(80, 200, 80);
const PIN_STEM_LEN: f32 = 16.0;

/// Transient state for the annotation mode overlay.
pub struct AnnotationModeState {
    /// Annotation currently being composed (after clicking to place).
    pub composing: Option<ComposingAnnotation>,
    /// ID of annotation whose detail popup is open.
    pub viewing: Option<String>,
    /// Whether to show resolved annotations.
    pub show_resolved: bool,
}

pub struct ComposingAnnotation {
    pub world_x: f32,
    pub world_y: f32,
    pub text: String,
}

impl Default for AnnotationModeState {
    fn default() -> Self {
        Self {
            composing: None,
            viewing: None,
            show_resolved: true,
        }
    }
}

/// Draw annotation pin markers and handle click interactions.
/// Returns true if a click was consumed by the annotation system.
pub fn draw_annotations(
    ui: &mut egui::Ui,
    painter: &egui::Painter,
    transform: &ViewTransform,
    annotations: &[Annotation],
    state: &mut AnnotationModeState,
    current_view: &str,
) -> bool {
    let mut consumed = false;

    // Draw pins for annotations on this view
    for ann in annotations {
        if ann.view != current_view {
            continue;
        }
        if ann.resolved && !state.show_resolved {
            continue;
        }

        let color = if ann.resolved { PIN_COLOR_RESOLVED } else { PIN_COLOR_OPEN };
        let screen_pos = transform.world_to_screen(egui::pos2(ann.world_x, ann.world_y));

        draw_pin(painter, screen_pos, color, transform.zoom);

        // Number badge
        let badge_center = egui::pos2(
            screen_pos.x,
            screen_pos.y - PIN_STEM_LEN * transform.zoom.clamp(0.3, 2.0) - PIN_RADIUS * transform.zoom.clamp(0.3, 2.0),
        );

        // If this annotation's detail popup is open, draw it
        if state.viewing.as_deref() == Some(&ann.id) {
            let popup_id = ui.id().with("ann_popup").with(&ann.id);
            let popup_pos = egui::pos2(
                screen_pos.x + 12.0,
                screen_pos.y - PIN_STEM_LEN * transform.zoom.clamp(0.3, 2.0) - 40.0,
            );
            egui::Area::new(popup_id)
                .fixed_pos(popup_pos)
                .order(egui::Order::Foreground)
                .show(ui.ctx(), |ui| {
                    egui::Frame::popup(ui.style()).show(ui, |ui| {
                        ui.set_max_width(280.0);
                        ui.horizontal(|ui| {
                            let status = if ann.resolved { "RESOLVED" } else { "OPEN" };
                            let status_color = if ann.resolved { PIN_COLOR_RESOLVED } else { PIN_COLOR_OPEN };
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
                    });
                });
        }

        // Hit test for clicking on existing pins
        let hit_radius = (PIN_RADIUS * transform.zoom.clamp(0.3, 2.0) + 4.0).max(12.0);
        if let Some(pos) = ui.ctx().pointer_interact_pos() {
            if ui.ctx().input(|i| i.pointer.primary_clicked()) {
                if badge_center.distance(pos) < hit_radius || screen_pos.distance(pos) < hit_radius {
                    if state.viewing.as_deref() == Some(&ann.id) {
                        state.viewing = None;
                    } else {
                        state.viewing = Some(ann.id.clone());
                        state.composing = None;
                    }
                    consumed = true;
                }
            }
        }
    }

    consumed
}

/// Draw a map pin shape at the given screen position.
fn draw_pin(painter: &egui::Painter, base: egui::Pos2, color: egui::Color32, zoom: f32) {
    let scale = zoom.clamp(0.3, 2.0);
    let stem_len = PIN_STEM_LEN * scale;
    let radius = PIN_RADIUS * scale;
    let top = egui::pos2(base.x, base.y - stem_len);

    // Stem
    painter.line_segment(
        [base, egui::pos2(base.x, base.y - stem_len + radius * 0.3)],
        egui::Stroke::new(2.0 * scale, color),
    );

    // Circle head
    let head_center = egui::pos2(top.x, top.y - radius);
    painter.circle_filled(head_center, radius, color);
    painter.circle_stroke(head_center, radius, egui::Stroke::new(1.0, egui::Color32::BLACK));

    // Exclamation mark inside
    let mark_color = egui::Color32::WHITE;
    let font = egui::FontId::monospace(radius * 1.2);
    painter.text(head_center, egui::Align2::CENTER_CENTER, "!", font, mark_color);
}

/// Draw the composing popup (text input for a new annotation).
/// Returns Some(Annotation) when the user confirms.
pub fn draw_compose_popup(
    ui: &mut egui::Ui,
    painter: &egui::Painter,
    transform: &ViewTransform,
    state: &mut AnnotationModeState,
    current_view: &str,
    nearest_room_id: Option<String>,
) -> Option<Annotation> {
    if state.composing.is_none() {
        return None;
    }

    let (wx, wy) = {
        let c = state.composing.as_ref().unwrap();
        (c.world_x, c.world_y)
    };

    let screen_pos = transform.world_to_screen(egui::pos2(wx, wy));

    // Draw pin at compose location
    draw_pin(painter, screen_pos, egui::Color32::from_rgb(255, 200, 80), transform.zoom);

    let popup_id = ui.id().with("ann_compose");
    let popup_pos = egui::pos2(screen_pos.x + 12.0, screen_pos.y - 40.0);

    let mut result = None;
    let mut cancel = false;

    egui::Area::new(popup_id)
        .fixed_pos(popup_pos)
        .order(egui::Order::Foreground)
        .show(ui.ctx(), |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.set_max_width(280.0);
                ui.label(egui::RichText::new("New Issue").strong());
                let composing = state.composing.as_mut().unwrap();
                let te = ui.text_edit_multiline(&mut composing.text);
                // Auto-focus the text field
                if composing.text.is_empty() {
                    te.request_focus();
                }
                ui.horizontal(|ui| {
                    if ui.button("Add").clicked() && !composing.text.trim().is_empty() {
                        result = Some(Annotation::new(
                            composing.text.trim().to_string(),
                            wx,
                            wy,
                            current_view.to_string(),
                            nearest_room_id.clone(),
                        ));
                    }
                    if ui.button("Cancel").clicked() {
                        cancel = true;
                    }
                });
            });
        });

    if result.is_some() || cancel {
        state.composing = None;
    }
    result
}

/// Draw the annotation mode status bar indicator and sidebar controls.
pub fn annotation_mode_indicator(ui: &mut egui::Ui) {
    ui.colored_label(
        egui::Color32::from_rgb(255, 200, 80),
        "ANNOTATE MODE (F7)",
    );
}

/// Sidebar panel for managing annotations when in annotation mode.
pub fn annotation_sidebar(
    ui: &mut egui::Ui,
    annotations: &mut Vec<Annotation>,
    state: &mut AnnotationModeState,
) {
    ui.heading("Issues");
    ui.checkbox(&mut state.show_resolved, "Show resolved");
    ui.separator();

    let mut resolve_id: Option<String> = None;
    let mut reopen_id: Option<String> = None;
    let mut delete_id: Option<String> = None;
    let mut focus_id: Option<String> = None;

    let open_count = annotations.iter().filter(|a| !a.resolved).count();
    let resolved_count = annotations.iter().filter(|a| a.resolved).count();
    ui.label(format!("{} open, {} resolved", open_count, resolved_count));
    ui.separator();

    egui::ScrollArea::vertical().show(ui, |ui| {
        for ann in annotations.iter() {
            if ann.resolved && !state.show_resolved {
                continue;
            }

            let color = if ann.resolved { PIN_COLOR_RESOLVED } else { PIN_COLOR_OPEN };
            ui.horizontal(|ui| {
                ui.colored_label(color, if ann.resolved { "+" } else { "!" });
                let label_text = {
                    let rt = egui::RichText::new(truncate(&ann.text, 30));
                    if ann.resolved { rt.strikethrough() } else { rt }
                };
                let btn = ui.button(label_text);
                if btn.clicked() {
                    focus_id = Some(ann.id.clone());
                }
            });
            ui.indent(&ann.id, |ui| {
                ui.label(egui::RichText::new(format!("{} | {}", ann.view, ann.created_at)).small().weak());
                if let Some(room_id) = &ann.room_id {
                    ui.label(egui::RichText::new(format!("Room: {}", room_id)).small().weak());
                }
                ui.horizontal(|ui| {
                    if ann.resolved {
                        if ui.small_button("Reopen").clicked() {
                            reopen_id = Some(ann.id.clone());
                        }
                    } else if ui.small_button("Resolve").clicked() {
                        resolve_id = Some(ann.id.clone());
                    }
                    if ui.small_button("Delete").clicked() {
                        delete_id = Some(ann.id.clone());
                    }
                });
            });
            ui.separator();
        }
    });

    if let Some(id) = resolve_id {
        if let Some(ann) = annotations.iter_mut().find(|a| a.id == id) {
            ann.resolved = true;
        }
    }
    if let Some(id) = reopen_id {
        if let Some(ann) = annotations.iter_mut().find(|a| a.id == id) {
            ann.resolved = false;
        }
    }
    if let Some(id) = delete_id {
        annotations.retain(|a| a.id != id);
    }
    if let Some(id) = focus_id {
        state.viewing = Some(id);
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}...", &s[..max])
    }
}
