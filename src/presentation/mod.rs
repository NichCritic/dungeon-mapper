pub mod fog;
pub mod lighting;

use std::collections::{HashMap, HashSet};

use crate::model::Dungeon;

#[derive(Clone, Debug, PartialEq)]
pub enum Visibility {
    Hidden,
    Explored,
    Visible,
}

#[derive(Clone, Debug)]
pub struct LightSource {
    pub id: String,
    pub room_id: String,
    pub radius: f32,
    pub intensity: f32,
    pub color: [u8; 3],
}

pub struct PresentationState {
    /// Per-room visibility, directly controlled by the DM.
    pub room_visibility: HashMap<String, Visibility>,
    /// Set of open doors (connection IDs). A door being open means the
    /// corridor is visible to players (if at least one endpoint room is
    /// not Hidden).
    pub doors_open: HashSet<String>,
    pub light_sources: Vec<LightSource>,
    pub ambient_light: f32,
    /// Whether room labels are shown in the player view.
    pub show_labels_player: bool,
}

impl PresentationState {
    pub fn new_from_dungeon(dungeon: &Dungeon) -> Self {
        let mut room_visibility = HashMap::new();
        for room in &dungeon.graph.rooms {
            room_visibility.insert(room.id.clone(), Visibility::Hidden);
        }
        Self {
            room_visibility,
            doors_open: HashSet::new(),
            light_sources: Vec::new(),
            ambient_light: 0.0,
            show_labels_player: false,
        }
    }

    pub fn room_visibility(&self, room_id: &str) -> &Visibility {
        self.room_visibility.get(room_id).unwrap_or(&Visibility::Hidden)
    }

    pub fn is_door_open(&self, connection_id: &str) -> bool {
        self.doors_open.contains(connection_id)
    }
}
