use serde::{Deserialize, Serialize};

fn default_attack_bonus() -> i8 { 5 }
fn default_damage_dice() -> String { "1d8 + 3".to_string() }

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
    #[serde(default = "default_attack_bonus")]
    pub attack_bonus: i8,
    #[serde(default = "default_damage_dice")]
    pub damage_dice: String,
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
            attack_bonus: default_attack_bonus(),
            damage_dice: default_damage_dice(),
        }
    }
}
