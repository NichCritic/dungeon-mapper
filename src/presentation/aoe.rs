use serde::{Deserialize, Serialize};

/// The shape geometry of an AoE.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum AoEShape {
    /// Circle with radius in grid squares.
    Circle { radius: f32 },
    /// Square with side length in grid squares.
    Square { size: f32 },
    /// Line with length and width in grid squares.
    Line { length: f32, width: f32 },
}

impl AoEShape {
    pub const PRESETS: &[(&str, AoEShape)] = &[
        ("Circle 10ft", AoEShape::Circle { radius: 2.0 }),
        ("Circle 15ft", AoEShape::Circle { radius: 3.0 }),
        ("Circle 20ft", AoEShape::Circle { radius: 4.0 }),
        ("Circle 30ft", AoEShape::Circle { radius: 6.0 }),
        ("Square 10ft", AoEShape::Square { size: 2.0 }),
        ("Square 15ft", AoEShape::Square { size: 3.0 }),
        ("Square 20ft", AoEShape::Square { size: 4.0 }),
        ("Line 30ft", AoEShape::Line { length: 6.0, width: 1.0 }),
        ("Line 60ft", AoEShape::Line { length: 12.0, width: 1.0 }),
    ];

    pub fn label(&self) -> String {
        match self {
            AoEShape::Circle { radius } => format!("Circle {}ft", radius * 5.0),
            AoEShape::Square { size } => format!("Square {}ft", size * 5.0),
            AoEShape::Line { length, width } => format!("Line {}x{}ft", length * 5.0, width * 5.0),
        }
    }
}

/// A placed AoE on the map.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AoEMarker {
    pub id: String,
    /// Center position in grid coordinates (float for sub-grid placement).
    pub x: f32,
    pub y: f32,
    /// The shape type and dimensions.
    pub shape: AoEShape,
    /// RGBA color with alpha for transparency.
    pub color: [u8; 4],
    /// Rotation in degrees (for lines and squares).
    #[serde(default)]
    pub rotation: f32,
}

impl AoEMarker {
    pub fn new(shape: AoEShape, x: f32, y: f32, color: [u8; 4]) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            x,
            y,
            shape,
            color,
            rotation: 0.0,
        }
    }
}

/// Render AoE markers using the egui painter (live overlay).
pub fn render_aoe_markers(
    painter: &egui::Painter,
    transform: &crate::util::ViewTransform,
    markers: &[AoEMarker],
    show_centers: bool,
) {
    let grid_px = crate::util::GRID_PX;

    for marker in markers {
        let color = egui::Color32::from_rgba_unmultiplied(
            marker.color[0], marker.color[1], marker.color[2], marker.color[3],
        );
        let stroke_color = egui::Color32::from_rgba_unmultiplied(
            marker.color[0], marker.color[1], marker.color[2],
            (marker.color[3] as u16 * 2).min(255) as u8,
        );

        let center_world = egui::pos2(marker.x * grid_px, marker.y * grid_px);
        let center_screen = transform.world_to_screen(center_world);

        match &marker.shape {
            AoEShape::Circle { radius } => {
                let radius_screen = *radius * grid_px * transform.zoom;
                painter.circle_filled(center_screen, radius_screen, color);
                painter.circle_stroke(
                    center_screen, radius_screen,
                    egui::Stroke::new(1.5 * transform.zoom, stroke_color),
                );
            }
            AoEShape::Square { size } => {
                let half = *size * grid_px * transform.zoom / 2.0;
                let rect = egui::Rect::from_center_size(
                    center_screen,
                    egui::vec2(half * 2.0, half * 2.0),
                );
                painter.rect_filled(rect, 0.0, color);
                painter.rect_stroke(rect, 0.0, egui::Stroke::new(1.5 * transform.zoom, stroke_color), egui::StrokeKind::Outside);
            }
            AoEShape::Line { length, width } => {
                let rot = marker.rotation.to_radians();
                let half_len = *length * grid_px * transform.zoom / 2.0;
                let half_wid = *width * grid_px * transform.zoom / 2.0;
                let dir = egui::vec2(rot.cos(), rot.sin());
                let perp = egui::vec2(-rot.sin(), rot.cos());
                let corners = [
                    center_screen + dir * half_len + perp * half_wid,
                    center_screen + dir * half_len - perp * half_wid,
                    center_screen - dir * half_len - perp * half_wid,
                    center_screen - dir * half_len + perp * half_wid,
                ];
                let mesh = egui::Mesh {
                    indices: vec![0, 1, 2, 0, 2, 3],
                    vertices: corners.iter().map(|&p| egui::epaint::Vertex {
                        pos: p,
                        uv: egui::epaint::WHITE_UV,
                        color,
                    }).collect(),
                    texture_id: egui::TextureId::default(),
                };
                painter.add(egui::Shape::mesh(mesh));
                let outline_points: Vec<egui::Pos2> = corners.iter().copied().chain(std::iter::once(corners[0])).collect();
                painter.add(egui::Shape::line(
                    outline_points,
                    egui::Stroke::new(1.5 * transform.zoom, stroke_color),
                ));
            }
        }

        // Center crosshair (DM only)
        if show_centers {
            let arm = 4.0 * transform.zoom;
            let cross_color = egui::Color32::from_rgba_unmultiplied(
                marker.color[0], marker.color[1], marker.color[2], 200,
            );
            let cross_stroke = egui::Stroke::new(1.5 * transform.zoom, cross_color);
            painter.line_segment(
                [center_screen - egui::vec2(arm, 0.0), center_screen + egui::vec2(arm, 0.0)],
                cross_stroke,
            );
            painter.line_segment(
                [center_screen - egui::vec2(0.0, arm), center_screen + egui::vec2(0.0, arm)],
                cross_stroke,
            );
        }
    }
}

/// Check if a screen point hits an AoE marker (for selection/dragging).
pub fn marker_at_screen_pos(
    pos: egui::Pos2,
    transform: &crate::util::ViewTransform,
    markers: &[AoEMarker],
) -> Option<usize> {
    let grid_px = crate::util::GRID_PX;
    for (i, marker) in markers.iter().enumerate().rev() {
        let center = transform.world_to_screen(egui::pos2(marker.x * grid_px, marker.y * grid_px));
        let hit_radius = match &marker.shape {
            AoEShape::Circle { radius } => *radius * grid_px * transform.zoom,
            AoEShape::Square { size } => *size * grid_px * transform.zoom / 2.0,
            AoEShape::Line { length, .. } => *length * grid_px * transform.zoom / 2.0,
        };
        if center.distance(pos) <= hit_radius.max(8.0) {
            return Some(i);
        }
    }
    None
}
