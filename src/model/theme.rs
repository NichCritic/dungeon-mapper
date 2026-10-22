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
    /// What lies between the rooms: rock for a dungeon, or open ground outdoors.
    #[serde(default)]
    pub ground: Ground,
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
    /// What's drawn between the rooms: plain paper (the floor colour) for an outdoor
    /// map's open ground, else the background around a dungeon's rock.
    pub fn background(&self) -> [u8; 4] {
        if self.ground.is_outdoor() { self.floor_color } else { self.bg_color }
    }

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
            ground: Ground::Rock,
        }
    }
}

impl Default for Theme {
    fn default() -> Self {
        Self::classic_dungeon()
    }
}

/// What lies between a map's rooms.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Ground {
    /// Solid rock: a dungeon. Rooms are carved out of it and its edge is shaded.
    #[default]
    Rock,
    Grass,
    Earth,
    Sand,
    Snow,
}

impl Ground {
    pub const ALL: [Ground; 5] = [Ground::Rock, Ground::Grass, Ground::Earth, Ground::Sand, Ground::Snow];

    pub fn label(self) -> &'static str {
        match self {
            Ground::Rock => "Rock (dungeon)",
            Ground::Grass => "Grass",
            Ground::Earth => "Earth",
            Ground::Sand => "Sand",
            Ground::Snow => "Snow",
        }
    }

    /// Open ground rather than a dungeon's rock.
    pub fn is_outdoor(self) -> bool {
        self != Ground::Rock
    }
}

