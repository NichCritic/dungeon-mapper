use crate::model::ConnectionType;
use crate::render::traits::MapRenderer;

/// Draw a door icon at the given position.
pub fn draw_door_icon(
    renderer: &mut dyn MapRenderer,
    x: f32,
    y: f32,
    connection_type: ConnectionType,
    _horizontal: bool,
    color: [u8; 4],
) {
    let size = 6.0;
    match connection_type {
        ConnectionType::Open => {
            // Gap only, no icon needed
        }
        ConnectionType::Door => {
            // Small arc representation (simplified as a line + small rect)
            renderer.stroke_rect(x - size / 2.0, y - size / 2.0, size, size, 1.0, color);
        }
        ConnectionType::Locked => {
            // Filled rect with X
            renderer.fill_rect(x - size / 2.0, y - size / 2.0, size, size, color);
            renderer.draw_line(
                x - size / 2.0, y - size / 2.0,
                x + size / 2.0, y + size / 2.0,
                1.0,
                [255, 255, 255, 255],
            );
            renderer.draw_line(
                x + size / 2.0, y - size / 2.0,
                x - size / 2.0, y + size / 2.0,
                1.0,
                [255, 255, 255, 255],
            );
        }
        ConnectionType::Secret => {
            // Thin dashed representation (simplified: small S mark)
            renderer.draw_text("S", x, y, 8.0, color);
        }
        ConnectionType::OneWay => {
            // Arrow indicator
            renderer.draw_line(x - size, y, x + size, y, 1.5, color);
            renderer.draw_line(x + size, y, x + size / 2.0, y - size / 2.0, 1.5, color);
            renderer.draw_line(x + size, y, x + size / 2.0, y + size / 2.0, 1.5, color);
        }
    }
}
