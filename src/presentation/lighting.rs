use crate::model::{SpatialLayout, LightSource};

/// Compute brightness for a grid cell from light sources and ambient light.
/// Returns a value in 0.0..=1.0.
pub fn compute_brightness_generic(
    cell_x: f32,
    cell_y: f32,
    light_sources: &[LightSource],
    ambient_light: f32,
    layout: &SpatialLayout,
) -> f32 {
    let mut brightness = ambient_light;

    for light in light_sources {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{RoomLayout, SpatialLayout};

    fn make_layout_with_room(room_id: &str, x: i32, y: i32, w: u32, h: u32) -> SpatialLayout {
        SpatialLayout {
            rooms: vec![RoomLayout {
                room_id: room_id.to_string(),
                x, y, width: w, height: h,
                violations: Vec::new(),
            }],
            corridors: Vec::new(),
            bounds: Vec::new(),
        }
    }

    #[test]
    fn test_no_lights_returns_ambient() {
        let layout = make_layout_with_room("r1", 0, 0, 4, 4);
        let brightness = compute_brightness_generic(2.0, 2.0, &[], 0.3, &layout);
        assert!((brightness - 0.3).abs() < 0.001);
    }

    #[test]
    fn test_single_light_at_center() {
        let layout = make_layout_with_room("r1", 0, 0, 4, 4);
        let light = LightSource {
            id: "l1".to_string(),
            room_id: "r1".to_string(),
            radius: 5.0,
            intensity: 1.0,
            color: [255, 255, 200],
        };
        let brightness = compute_brightness_generic(2.0, 2.0, &[light], 0.0, &layout);
        assert!((brightness - 1.0).abs() < 0.001);
    }

    #[test]
    fn test_cell_outside_light_radius() {
        let layout = make_layout_with_room("r1", 0, 0, 4, 4);
        let light = LightSource {
            id: "l1".to_string(),
            room_id: "r1".to_string(),
            radius: 3.0,
            intensity: 1.0,
            color: [255, 255, 200],
        };
        let brightness = compute_brightness_generic(20.0, 20.0, &[light], 0.0, &layout);
        assert!((brightness - 0.0).abs() < 0.001);
    }

    #[test]
    fn test_brightness_capped_at_1() {
        let layout = make_layout_with_room("r1", 0, 0, 4, 4);
        let light = LightSource {
            id: "l1".to_string(),
            room_id: "r1".to_string(),
            radius: 5.0,
            intensity: 1.0,
            color: [255, 255, 200],
        };
        let brightness = compute_brightness_generic(2.0, 2.0, &[light], 0.8, &layout);
        assert!((brightness - 1.0).abs() < 0.001);
    }
}
