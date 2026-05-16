use std::collections::{HashMap, HashSet};
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

/// Persisted session state — runtime presentation data that survives between sessions.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SessionState {
    /// Per-room visibility: "hidden", "explored", or "visible".
    #[serde(default)]
    pub room_visibility: HashMap<String, String>,
    /// Open door connection IDs.
    #[serde(default)]
    pub doors_open: HashSet<String>,
    /// Current positions of encounters: encounter_id -> room_id.
    #[serde(default)]
    pub encounter_positions: HashMap<String, String>,
    /// Encounter IDs that have been fully defeated.
    #[serde(default)]
    pub defeated_encounters: HashSet<String>,
    /// Per-monster current HP: key is "encounter_id/monster_idx/instance", value is current HP.
    #[serde(default)]
    pub encounter_hp: HashMap<String, i32>,
    /// Which room the party token is in (None = not placed).
    #[serde(default)]
    pub party_room: Option<String>,
    /// Whether autobattle is enabled.
    #[serde(default)]
    pub autobattle: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Dungeon {
    pub name: String,
    pub graph: DungeonGraph,
    pub layout: Option<SpatialLayout>,
    #[serde(default)]
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
    /// Area-of-effect markers placed on the map.
    #[serde(default)]
    pub aoe_markers: Vec<crate::presentation::aoe::AoEMarker>,
    /// Persisted session state (fog of war, encounter positions, HP, etc.).
    #[serde(default)]
    pub session: SessionState,
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
            aoe_markers: Vec::new(),
            session: SessionState::default(),
        }
    }
}

impl Default for Dungeon {
    fn default() -> Self {
        Self::new("Untitled Dungeon".to_string())
    }
}
