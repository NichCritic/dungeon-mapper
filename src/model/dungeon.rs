use serde::{Deserialize, Serialize};

use super::{DungeonGraph, SpatialLayout, Theme};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Dungeon {
    pub name: String,
    pub graph: DungeonGraph,
    pub layout: Option<SpatialLayout>,
    pub theme: Theme,
}

impl Dungeon {
    pub fn new(name: String) -> Self {
        Self {
            name,
            graph: DungeonGraph::new(),
            layout: None,
            theme: Theme::default(),
        }
    }
}

impl Default for Dungeon {
    fn default() -> Self {
        Self::new("Untitled Dungeon".to_string())
    }
}
