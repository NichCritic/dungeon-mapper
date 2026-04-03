use crate::model::SpatialLayout;
use super::PresentationState;

/// Compute brightness for a grid cell from all light sources and ambient light.
/// Returns a value in 0.0..=1.0.
pub fn compute_brightness(
    cell_x: f32,
    cell_y: f32,
    presentation: &PresentationState,
    layout: &SpatialLayout,
) -> f32 {
    let mut brightness = presentation.ambient_light;

    for light in &presentation.light_sources {
        let Some(light_rl) = layout.room_by_id(&light.room_id) else { continue };
        let light_cx = light_rl.x as f32 + light_rl.width as f32 / 2.0;
        let light_cy = light_rl.y as f32 + light_rl.height as f32 / 2.0;

        let dx = cell_x - light_cx;
        let dy = cell_y - light_cy;
        let dist = (dx * dx + dy * dy).sqrt();

        if dist < light.radius {
            let falloff = 1.0 - dist / light.radius;
            brightness += light.intensity * falloff * falloff;
        }
    }

    brightness.min(1.0)
}
