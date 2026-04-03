use serde::{Deserialize, Serialize};

use super::monster::EncounterMonster;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum EncounterType {
    Static,
    /// Wandering encounter. `None` means unlimited range.
    Wandering(Option<u32>),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encounter_new() {
        let enc = Encounter::new("Goblin Ambush".to_string(), "room-123".to_string());
        assert_eq!(enc.id.len(), 36); // UUID
        assert_eq!(enc.name, "Goblin Ambush");
        assert_eq!(enc.home_room_id, "room-123");
        assert_eq!(enc.encounter_type, EncounterType::Static);
        assert!(enc.monsters.is_empty());
        assert!(enc.notes.is_empty());
    }

    #[test]
    fn test_encounter_type_equality() {
        assert_eq!(EncounterType::Static, EncounterType::Static);
        assert_eq!(EncounterType::Wandering(Some(3)), EncounterType::Wandering(Some(3)));
        assert_ne!(EncounterType::Static, EncounterType::Wandering(Some(1)));
        assert_eq!(EncounterType::Wandering(None), EncounterType::Wandering(None));
    }
}
