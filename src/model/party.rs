use serde::{Deserialize, Deserializer, Serialize};

fn default_attack_bonus() -> i8 { 5 }
fn default_damage_dice() -> String { "1d8 + 3".to_string() }

/// Special senses a PC might have (non-exclusive flags).
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize)]
pub struct PcSenses {
    #[serde(default)]
    pub darkvision: bool,
    #[serde(default)]
    pub blindsight: bool,
    #[serde(default)]
    pub tremorsense: bool,
}

/// Custom deserializer that handles both the old enum format ("Normal",
/// "Darkvision", etc.) and the new struct format.
impl<'de> Deserialize<'de> for PcSenses {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        use serde::de;

        struct PcSensesVisitor;

        impl<'de> de::Visitor<'de> for PcSensesVisitor {
            type Value = PcSenses;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("a PcSenses struct or a legacy enum string")
            }

            // New format: { "darkvision": true, ... }
            fn visit_map<A>(self, mut map: A) -> Result<PcSenses, A::Error>
            where
                A: de::MapAccess<'de>,
            {
                let mut senses = PcSenses::default();
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "darkvision" => senses.darkvision = map.next_value()?,
                        "blindsight" => senses.blindsight = map.next_value()?,
                        "tremorsense" => senses.tremorsense = map.next_value()?,
                        _ => { let _ = map.next_value::<serde::de::IgnoredAny>()?; }
                    }
                }
                Ok(senses)
            }

            // Old format: "Normal", "Darkvision", "Blindsight", "Tremorsense"
            fn visit_str<E>(self, value: &str) -> Result<PcSenses, E>
            where
                E: de::Error,
            {
                Ok(match value {
                    "Darkvision" => PcSenses { darkvision: true, ..Default::default() },
                    "Blindsight" => PcSenses { blindsight: true, ..Default::default() },
                    "Tremorsense" => PcSenses { tremorsense: true, ..Default::default() },
                    _ => PcSenses::default(), // "Normal" or unknown
                })
            }
        }

        deserializer.deserialize_any(PcSensesVisitor)
    }
}

impl PcSenses {
    /// True if the creature has a non-sight sense that ignores light entirely.
    pub fn has_non_sight_sense(self) -> bool {
        self.blindsight || self.tremorsense
    }
}

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
    /// Dexterity (Stealth) modifier for group stealth checks.
    #[serde(default)]
    pub stealth_modifier: i8,
    /// Special senses (darkvision, blindsight, tremorsense).
    #[serde(default)]
    pub senses: PcSenses,
    /// Manual stealth roll override. When set, awareness checks use this
    /// value instead of rolling (for players who prefer to roll themselves).
    #[serde(default)]
    pub stealth_override: Option<i32>,
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
            stealth_modifier: 0,
            senses: PcSenses::default(),
            stealth_override: None,
        }
    }
}
