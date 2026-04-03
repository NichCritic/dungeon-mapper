use serde::{Deserialize, Serialize};

use super::{CustomMonster, DungeonGraph, Encounter, PlayerCharacter, SpatialLayout, Theme};

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
        }
    }
}

impl Default for Dungeon {
    fn default() -> Self {
        Self::new("Untitled Dungeon".to_string())
    }
}
