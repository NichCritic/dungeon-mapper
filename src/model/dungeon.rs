use serde::{Deserialize, Serialize};

use super::{Annotation, CustomMonster, DungeonGraph, Encounter, PlayerCharacter, SpatialLayout, Theme};

/// A light source placed in a room.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LightSource {
    pub id: String,
    pub room_id: String,
    pub radius: f32,
    pub intensity: f32,
    pub color: [u8; 3],
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Dungeon {
    pub name: String,
    pub graph: DungeonGraph,
    pub layout: Option<SpatialLayout>,
    pub theme: Theme,
    #[serde(default)]
    pub encounters: Vec<Encounter>,
    /// User-created or cloned custom monsters, saved with the dungeon.
    #[serde(default)]
    pub custom_monsters: Vec<CustomMonster>,
    /// Player characters in the party.
    #[serde(default)]
    pub party: Vec<PlayerCharacter>,
    /// Issue annotations pinned to map locations.
    #[serde(default)]
    pub annotations: Vec<Annotation>,
    /// Placeable light sources for the player view.
    #[serde(default)]
    pub light_sources: Vec<LightSource>,
    /// Ambient light level (0.0 = dark, 1.0 = fully lit).
    #[serde(default)]
    pub ambient_light: f32,
}

impl Dungeon {
    pub fn new(name: String) -> Self {
        Self {
            name,
            graph: DungeonGraph::new(),
            layout: None,
            theme: Theme::default(),
            encounters: Vec::new(),
            custom_monsters: Vec::new(),
            party: Vec::new(),
            annotations: Vec::new(),
            light_sources: Vec::new(),
            ambient_light: 0.0,
        }
    }
}

impl Default for Dungeon {
    fn default() -> Self {
        Self::new("Untitled Dungeon".to_string())
    }
}
