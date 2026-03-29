use std::collections::HashSet;

use crate::model::SpatialLayout;
use super::PresentationState;

/// Compute which rooms are within radius of any light source.
/// Returns room IDs that should be considered lit (Visible).
pub fn compute_lit_rooms(
    presentation: &PresentationState,
    layout: &SpatialLayout,
) -> HashSet<String> {
    let mut lit = HashSet::new();

    for light in &presentation.light_sources {
        let Some(light_rl) = layout.room_by_id(&light.room_id) else { continue };
        let light_cx = light_rl.x as f32 + light_rl.width as f32 / 2.0;
        let light_cy = light_rl.y as f32 + light_rl.height as f32 / 2.0;

        for rl in &layout.rooms {
            let room_cx = rl.x as f32 + rl.width as f32 / 2.0;
            let room_cy = rl.y as f32 + rl.height as f32 / 2.0;
            let dx = room_cx - light_cx;
            let dy = room_cy - light_cy;
            let dist = (dx * dx + dy * dy).sqrt();

            if dist <= light.radius {
                lit.insert(rl.room_id.clone());
            }
        }
    }

    lit
}

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
