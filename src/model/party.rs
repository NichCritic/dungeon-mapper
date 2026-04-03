use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PlayerCharacter {
    pub id: String,
    pub name: String,
    pub class: String,
    pub ac: u8,
    pub max_hp: i32,
    pub current_hp: i32,
    pub initiative_modifier: i8,
    pub passive_perception: u8,
    #[serde(default)]
    pub notes: String,
}

impl PlayerCharacter {
    pub fn new(name: String) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            name,
            class: String::new(),
            ac: 10,
            max_hp: 10,
            current_hp: 10,
            initiative_modifier: 0,
            passive_perception: 10,
            notes: String::new(),
        }
    }
}
