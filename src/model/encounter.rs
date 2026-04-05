use serde::{Deserialize, Serialize};

use super::monster::EncounterMonster;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum EncounterType {
    Static,
    /// Wandering encounter. `None` means unlimited range.
    Wandering(Option<u32>),
}

/// Which ability modifier to use for the hazard save.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub enum SaveAbility {
    #[default]
    Dex,
    Str,
    Con,
    Int,
    Wis,
    Cha,
}

impl SaveAbility {
    pub const ALL: &[SaveAbility] = &[
        SaveAbility::Str, SaveAbility::Dex, SaveAbility::Con,
        SaveAbility::Int, SaveAbility::Wis, SaveAbility::Cha,
    ];

    pub fn label(&self) -> &str {
        match self {
            SaveAbility::Str => "STR",
            SaveAbility::Dex => "DEX",
            SaveAbility::Con => "CON",
            SaveAbility::Int => "INT",
            SaveAbility::Wis => "WIS",
            SaveAbility::Cha => "CHA",
        }
    }
}

/// A hazard effect applied to encounters sharing the room.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Hazard {
    /// Dice expression for damage (e.g. "2d6", "3d8 + 4"). Empty = no damage.
    #[serde(default)]
    pub damage: String,
    /// Optional save DC. If set, a d20 + ability mod roll >= DC avoids the effect.
    #[serde(default)]
    pub save_dc: Option<u8>,
    /// Which ability to use for the save.
    #[serde(default)]
    pub save_ability: SaveAbility,
    /// Status effect to apply on failure (index into STANDARD_CONDITIONS). None = damage only.
    #[serde(default)]
    pub condition: Option<String>,
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
    /// If set, this encounter acts as a hazard that applies an effect to
    /// other encounters sharing the room. Orthogonal to Static/Wandering.
    #[serde(default)]
    pub hazard: Option<Hazard>,
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
            hazard: None,
        }
    }

    pub fn is_hazard(&self) -> bool {
        self.hazard.is_some()
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
