use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Theme {
    pub name: String,
    pub wall_color: [u8; 4],
    pub floor_color: [u8; 4],
    pub bg_color: [u8; 4],
    pub wall_style: WallStyle,
    pub grid_visible: bool,
    pub hatching: bool,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
pub enum WallStyle {
    Sharp,
    Rough,
    Rounded,
}

impl Theme {
    pub fn classic_dungeon() -> Self {
        Self {
            name: "Classic Dungeon".to_string(),
            wall_color: [0, 0, 0, 255],
            floor_color: [255, 255, 255, 255],
            bg_color: [245, 240, 232, 255],
            wall_style: WallStyle::Sharp,
            grid_visible: true,
            hatching: true,
        }
    }
}

impl Default for Theme {
    fn default() -> Self {
        Self::classic_dungeon()
    }
}
