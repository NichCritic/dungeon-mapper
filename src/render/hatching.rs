use crate::render::traits::MapRenderer;

/// Draw Dyson-style hatching along a wall segment, on the exterior side.
pub fn draw_hatching(
    renderer: &mut dyn MapRenderer,
    x1: f32,
    y1: f32,
    x2: f32,
    y2: f32,
    exterior_side: f32, // +1.0 or -1.0 to indicate which side is exterior
    color: [u8; 4],
) {
    let dx = x2 - x1;
    let dy = y2 - y1;
    let len = (dx * dx + dy * dy).sqrt();
    if len < 1.0 {
        return;
    }

    // Normal perpendicular to wall, pointing toward exterior
    let nx = -dy / len * exterior_side;
    let ny = dx / len * exterior_side;

    // Hatch at semi-random intervals
    let mut d = 4.0;
    let mut i = 0u32;
    while d < len {
        let t = d / len;
        let hx = x1 + dx * t;
        let hy = y1 + dy * t;

        // Pseudo-random hatch length variation (6-10px)
        let seed = ((hx * 7.3 + hy * 13.7) * 100.0) as u32;
        let hatch_len = 6.0 + (seed % 5) as f32;

        renderer.draw_line(hx, hy, hx + nx * hatch_len, hy + ny * hatch_len, 0.8, color);

        // Varying interval (4-8px)
        let interval = 4.0 + (i % 3) as f32 * 2.0;
        d += interval;
        i += 1;
    }
}
