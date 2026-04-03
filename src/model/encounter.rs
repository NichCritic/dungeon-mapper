use serde::{Deserialize, Serialize};

use super::monster::EncounterMonster;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum EncounterType {
    Static,
    Wandering(u32),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Encounter {
    pub id: String,
    pub name: String,
    pub encounter_type: EncounterType,
    /// The room this encounter is assigned to (home room).
    pub home_room_id: String,
    /// Monsters in this encounter.
    #[serde(default)]
    pub monsters: Vec<EncounterMonster>,
    /// DM notes for this encounter.
    #[serde(default)]
    pub notes: String,
}

impl Encounter {
    pub fn new(name: String, home_room_id: String) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            name,
            encounter_type: EncounterType::Static,
            home_room_id,
            monsters: Vec::new(),
            notes: String::new(),
        }
    }

}
