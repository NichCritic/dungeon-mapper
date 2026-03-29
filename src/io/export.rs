use std::path::Path;

use crate::model::Dungeon;
use crate::render::ImageRenderer;
use crate::util::GRID_PX;

pub fn export_png(
    dungeon: &Dungeon,
    path: &Path,
    dm_mode: bool,
    scale_multiplier: u32,
) -> Result<(), String> {
    let layout = dungeon.layout.as_ref().ok_or("No layout to export")?;

    let (min_x, min_y, max_x, max_y) = layout.extents();
    let margin = 2;
    let grid_w = (max_x - min_x + margin * 2) as u32;
    let grid_h = (max_y - min_y + margin * 2) as u32;

    let scale = GRID_PX * scale_multiplier as f32;
    let width = (grid_w as f32 * scale) as u32;
    let height = (grid_h as f32 * scale) as u32;

    let mut renderer = ImageRenderer::new(width, height, scale / GRID_PX);

    crate::render::themed::render_themed(
        &mut renderer,
        &dungeon.graph,
        layout,
        &dungeon.theme,
        dungeon.theme.grid_visible,
        true,
        dm_mode,
        dm_mode,
    );

    renderer
        .image
        .save(path)
        .map_err(|e| e.to_string())
}
