use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Theme {
    pub name: String,
    pub wall_color: [u8; 4],
    pub floor_color: [u8; 4],
    pub bg_color: [u8; 4],
    pub wall_style: WallStyle,
    pub grid_visible: bool,
    /// Exterior shading enabled
    #[serde(default = "default_true")]
    pub exterior_shading: bool,
    /// Radius of exterior shading in grid squares
    #[serde(default = "default_shading_radius")]
    pub shading_radius: f32,
    /// Style of exterior shading
    #[serde(default)]
    pub shading_style: ShadingStyle,
    /// Density of hatching lines (when style is Hatched)
    #[serde(default = "default_hatching_density")]
    pub hatching_density: f32,
    /// Corridor corner style
    #[serde(default)]
    pub corridor_chamfer: ChamferStyle,
}

fn default_true() -> bool { true }
fn default_shading_radius() -> f32 { 1.0 }
fn default_hatching_density() -> f32 { 1.0 }

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
pub enum WallStyle {
    Sharp,
    Rough,
    Rounded,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq)]
pub enum ShadingStyle {
    #[default]
    Hatched,
    Solid,
    Stippled,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq)]
pub enum ChamferStyle {
    #[default]
    Sharp,
    Rounded,
    Angled,
}

impl ChamferStyle {
    pub fn label(self) -> &'static str {
        match self {
            ChamferStyle::Sharp => "Sharp",
            ChamferStyle::Rounded => "Rounded",
            ChamferStyle::Angled => "45° Angled",
        }
    }

    pub const ALL: [ChamferStyle; 3] = [
        ChamferStyle::Sharp,
        ChamferStyle::Rounded,
        ChamferStyle::Angled,
    ];
}

impl ShadingStyle {
    pub fn label(self) -> &'static str {
        match self {
            ShadingStyle::Hatched => "Hatched",
            ShadingStyle::Solid => "Solid",
            ShadingStyle::Stippled => "Stippled",
        }
    }

    pub const ALL: [ShadingStyle; 3] = [
        ShadingStyle::Hatched,
        ShadingStyle::Solid,
        ShadingStyle::Stippled,
    ];
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
            exterior_shading: true,
            shading_radius: 1.0,
            shading_style: ShadingStyle::Hatched,
            hatching_density: 1.0,
            corridor_chamfer: ChamferStyle::Sharp,
        }
    }
}

impl Default for Theme {
    fn default() -> Self {
        Self::classic_dungeon()
    }
}
