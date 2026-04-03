use serde::{Deserialize, Deserializer, Serialize};
use std::collections::HashMap;
use std::cmp;

/// Deserialize a field that may be null as an empty Vec.
fn null_as_empty_vec<'de, D, T>(deserializer: D) -> Result<Vec<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<Vec<T>>::deserialize(deserializer).map(|opt| opt.unwrap_or_default())
}

/// Deserialize a HashMap<String, String> where values may be strings or integers.
fn string_or_int_map<'de, D>(deserializer: D) -> Result<HashMap<String, String>, D::Error>
where
    D: Deserializer<'de>,
{
    let opt: Option<HashMap<String, serde_json::Value>> = Option::deserialize(deserializer)?;
    Ok(opt.map(|m| {
        m.into_iter().map(|(k, v)| {
            let s = match v {
                serde_json::Value::String(s) => s,
                serde_json::Value::Number(n) => {
                    if let Some(i) = n.as_i64() {
                        if i >= 0 { format!("+{}", i) } else { format!("{}", i) }
                    } else {
                        n.to_string()
                    }
                }
                other => other.to_string(),
            };
            (k, s)
        }).collect()
    }).unwrap_or_default())
}

/// A monster stat block, deserialized from 5e-Tools JSON.
/// Fields that are complex or polymorphic are kept as serde_json::Value
/// to avoid modeling every edge case in the data.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Monster {
    pub name: String,
    pub source: String,
    #[serde(default)]
    pub page: Option<u32>,
    #[serde(default, deserialize_with = "null_as_empty_vec")]
    pub size: Vec<String>,
    #[serde(default, rename = "type")]
    pub monster_type: MonsterType,
    #[serde(default, deserialize_with = "null_as_empty_vec")]
    pub alignment: Vec<serde_json::Value>,

    // Ability scores
    #[serde(default, rename = "str")]
    pub str_score: u8,
    #[serde(default, rename = "dex")]
    pub dex_score: u8,
    #[serde(default, rename = "con")]
    pub con_score: u8,
    #[serde(default, rename = "int")]
    pub int_score: u8,
    #[serde(default, rename = "wis")]
    pub wis_score: u8,
    #[serde(default, rename = "cha")]
    pub cha_score: u8,

    // Combat stats
    #[serde(default, deserialize_with = "null_as_empty_vec")]
    pub ac: Vec<ArmorClass>,
    #[serde(default)]
    pub hp: HitPoints,
    #[serde(default)]
    pub speed: Speed,
    #[serde(default)]
    pub cr: ChallengeRating,

    // Saves, skills, senses
    #[serde(default, deserialize_with = "string_or_int_map")]
    pub save: HashMap<String, String>,
    #[serde(default, deserialize_with = "string_or_int_map")]
    pub skill: HashMap<String, String>,
    #[serde(default, deserialize_with = "null_as_empty_vec")]
    pub senses: Vec<String>,
    #[serde(default)]
    pub passive: Option<u8>,
    #[serde(default, deserialize_with = "null_as_empty_vec")]
    pub languages: Vec<String>,

    // Damage immunities, resistances, vulnerabilities
    #[serde(default, deserialize_with = "null_as_empty_vec")]
    pub immune: Vec<serde_json::Value>,
    #[serde(default, deserialize_with = "null_as_empty_vec")]
    pub resist: Vec<serde_json::Value>,
    #[serde(default, deserialize_with = "null_as_empty_vec")]
    pub vulnerable: Vec<serde_json::Value>,
    #[serde(default, rename = "conditionImmune", deserialize_with = "null_as_empty_vec")]
    pub condition_immune: Vec<serde_json::Value>,

    // Features
    #[serde(default, rename = "trait", deserialize_with = "null_as_empty_vec")]
    pub traits: Vec<Feature>,
    #[serde(default, deserialize_with = "null_as_empty_vec")]
    pub action: Vec<Feature>,
    #[serde(default, deserialize_with = "null_as_empty_vec")]
    pub reaction: Vec<Feature>,
    #[serde(default, deserialize_with = "null_as_empty_vec")]
    pub legendary: Vec<Feature>,
    #[serde(default, deserialize_with = "null_as_empty_vec")]
    pub mythic: Vec<Feature>,
    #[serde(default, deserialize_with = "null_as_empty_vec")]
    pub spellcasting: Vec<serde_json::Value>,

    // Environment tags
    #[serde(default, deserialize_with = "null_as_empty_vec")]
    pub environment: Vec<String>,

    // We skip all the metadata/tag fields we don't need for display
}

/// Monster type — can be a plain string or a structured object.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MonsterType {
    Simple(String),
    Detailed {
        #[serde(rename = "type")]
        type_name: String,
        #[serde(default)]
        tags: Vec<serde_json::Value>,
        #[serde(default, rename = "swarmSize")]
        swarm_size: Option<String>,
    },
}

impl Default for MonsterType {
    fn default() -> Self {
        MonsterType::Simple("unknown".to_string())
    }
}

impl MonsterType {
    pub fn display(&self) -> String {
        match self {
            MonsterType::Simple(s) => s.clone(),
            MonsterType::Detailed { type_name, swarm_size, .. } => {
                if let Some(swarm) = swarm_size {
                    format!("swarm of {} {}s", size_label(swarm), type_name)
                } else {
                    type_name.clone()
                }
            }
        }
    }
}

/// Armor class — can be a plain int or a structured object.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ArmorClass {
    Simple(u8),
    Detailed {
        ac: u8,
        #[serde(default)]
        from: Vec<String>,
        #[serde(default)]
        condition: Option<String>,
    },
    Special {
        special: String,
    },
}

impl ArmorClass {
    pub fn value(&self) -> Option<u8> {
        match self {
            ArmorClass::Simple(v) => Some(*v),
            ArmorClass::Detailed { ac, .. } => Some(*ac),
            ArmorClass::Special { .. } => None,
        }
    }

    pub fn display(&self) -> String {
        match self {
            ArmorClass::Simple(v) => v.to_string(),
            ArmorClass::Detailed { ac, from, condition } => {
                let mut s = ac.to_string();
                if !from.is_empty() {
                    s.push_str(&format!(" ({})", from.join(", ")));
                }
                if let Some(cond) = condition {
                    s.push_str(&format!(" {}", cond));
                }
                s
            }
            ArmorClass::Special { special } => special.clone(),
        }
    }
}

/// Hit points.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(untagged)]
pub enum HitPoints {
    #[default]
    Unknown,
    Formula {
        average: i32,
        formula: String,
    },
    Special {
        special: String,
    },
}

impl HitPoints {
    pub fn display(&self) -> String {
        match self {
            HitPoints::Unknown => "—".to_string(),
            HitPoints::Formula { average, formula } => format!("{} ({})", average, formula),
            HitPoints::Special { special } => special.clone(),
        }
    }
}

/// Speed — stored as a map, values can be int or structured.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Speed {
    #[serde(default)]
    pub walk: SpeedValue,
    #[serde(default)]
    pub fly: SpeedValue,
    #[serde(default)]
    pub swim: SpeedValue,
    #[serde(default)]
    pub climb: SpeedValue,
    #[serde(default)]
    pub burrow: SpeedValue,
    #[serde(default, rename = "canHover")]
    pub can_hover: bool,
}

impl Speed {
    pub fn display(&self) -> String {
        let mut parts = Vec::new();
        if let Some(v) = self.walk.value() {
            parts.push(format!("{} ft.", v));
        }
        if let Some(v) = self.fly.value() {
            let hover = if self.can_hover { " (hover)" } else { "" };
            parts.push(format!("fly {} ft.{}", v, hover));
        }
        if let Some(v) = self.swim.value() {
            parts.push(format!("swim {} ft.", v));
        }
        if let Some(v) = self.climb.value() {
            parts.push(format!("climb {} ft.", v));
        }
        if let Some(v) = self.burrow.value() {
            parts.push(format!("burrow {} ft.", v));
        }
        if parts.is_empty() {
            "0 ft.".to_string()
        } else {
            parts.join(", ")
        }
    }
}

/// A speed value can be a plain int or a structured object.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SpeedValue {
    #[default]
    None,
    Simple(u32),
    Detailed {
        number: u32,
        #[serde(default)]
        condition: Option<String>,
    },
}

impl SpeedValue {
    pub fn value(&self) -> Option<u32> {
        match self {
            SpeedValue::None => None,
            SpeedValue::Simple(v) => Some(*v),
            SpeedValue::Detailed { number, .. } => Some(*number),
        }
    }
}

/// Challenge rating — can be a string like "1/4" or a structured object.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ChallengeRating {
    Simple(String),
    Detailed {
        cr: String,
        #[serde(default)]
        lair: Option<String>,
        #[serde(default)]
        coven: Option<String>,
    },
}

impl Default for ChallengeRating {
    fn default() -> Self {
        ChallengeRating::Simple("0".to_string())
    }
}

impl ChallengeRating {
    pub fn cr_string(&self) -> &str {
        match self {
            ChallengeRating::Simple(s) => s,
            ChallengeRating::Detailed { cr, .. } => cr,
        }
    }

    /// Parse CR to a numeric value for sorting/filtering.
    pub fn cr_numeric(&self) -> f32 {
        cr_to_numeric(self.cr_string())
    }

    /// Look up XP value for this CR.
    pub fn xp(&self) -> u32 {
        cr_to_xp(self.cr_string())
    }
}

/// A named feature (trait, action, reaction, legendary action).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Feature {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub entries: Vec<serde_json::Value>,
}

impl Feature {
    /// Render entries to plain text, stripping 5e-Tools markup.
    pub fn entries_text(&self) -> String {
        self.entries.iter()
            .map(|e| entry_to_text(e))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// A reference to a monster — either from the base database or a custom one.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum MonsterRef {
    /// Reference to a monster in the bundled database.
    Base { source: String, name: String },
    /// Reference to a user-created custom monster.
    Custom { id: String },
    /// Reference to a custom monster created by merging two monsters.
    Merged { id: String },
}

/// A user-customized monster stored in the dungeon file.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CustomMonster {
    pub id: String,
    /// If cloned from a base monster, the original source/name.
    pub based_on: Option<(String, String)>,
    pub monster: Monster,
}

/// A monster slot in an encounter.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EncounterMonster {
    pub monster_ref: MonsterRef,
    pub count: u32,
    #[serde(default)]
    pub notes: String,
}

// --- Utility functions ---

/// Strip 5e-Tools markup tags from text.
/// Converts {@atk mw} -> "", {@hit 4} -> "+4", {@damage 1d6 + 2} -> "1d6 + 2",
/// {@dc 15} -> "DC 15", {@condition prone} -> "prone", etc.
pub fn strip_5e_markup(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();

    while let Some(c) = chars.next() {
        if c == '{' && chars.peek() == Some(&'@') {
            // Consume {@tag content}
            chars.next(); // skip @
            // Read tag name
            let mut tag = String::new();
            while let Some(&ch) = chars.peek() {
                if ch == ' ' || ch == '}' {
                    break;
                }
                tag.push(ch);
                chars.next();
            }
            // Read content until }
            let mut content = String::new();
            if chars.peek() == Some(&' ') {
                chars.next(); // skip space after tag
            }
            let mut depth = 1;
            while let Some(ch) = chars.next() {
                if ch == '{' {
                    depth += 1;
                    content.push(ch);
                } else if ch == '}' {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                    content.push(ch);
                } else {
                    content.push(ch);
                }
            }

            // Transform based on tag
            match tag.as_str() {
                "atk" => {} // skip attack type markers
                "hit" => {
                    result.push('+');
                    result.push_str(&content);
                }
                "dc" => {
                    result.push_str("DC ");
                    result.push_str(&content);
                }
                "h" => {} // hit marker, skip
                "recharge" => {
                    result.push_str(&format!("(Recharge {})", content));
                }
                _ => {
                    // For damage, condition, creature, spell, skill, etc.
                    // just take the content (first part before |)
                    let display = content.split('|').next().unwrap_or(&content);
                    result.push_str(display);
                }
            }
        } else {
            result.push(c);
        }
    }

    result
}

/// Convert a JSON entry value to plain text.
fn entry_to_text(entry: &serde_json::Value) -> String {
    match entry {
        serde_json::Value::String(s) => strip_5e_markup(s),
        serde_json::Value::Object(obj) => {
            // Handle list entries, etc.
            if let Some(items) = obj.get("items") {
                if let Some(arr) = items.as_array() {
                    return arr.iter()
                        .map(|item| format!("  - {}", entry_to_text(item)))
                        .collect::<Vec<_>>()
                        .join("\n");
                }
            }
            if let Some(entries) = obj.get("entries") {
                if let Some(arr) = entries.as_array() {
                    return arr.iter()
                        .map(|e| entry_to_text(e))
                        .collect::<Vec<_>>()
                        .join("\n");
                }
            }
            // Fallback: stringify
            strip_5e_markup(&entry.to_string())
        }
        _ => entry.to_string(),
    }
}

/// Convert CR string to numeric for sorting.
pub fn cr_to_numeric(cr: &str) -> f32 {
    match cr {
        "0" => 0.0,
        "1/8" => 0.125,
        "1/4" => 0.25,
        "1/2" => 0.5,
        _ => cr.parse().unwrap_or(0.0),
    }
}

/// CR to XP lookup table (5e standard).
pub fn cr_to_xp(cr: &str) -> u32 {
    match cr {
        "0" => 10,
        "1/8" => 25,
        "1/4" => 50,
        "1/2" => 100,
        "1" => 200,
        "2" => 450,
        "3" => 700,
        "4" => 1100,
        "5" => 1800,
        "6" => 2300,
        "7" => 2900,
        "8" => 3900,
        "9" => 5000,
        "10" => 5900,
        "11" => 7200,
        "12" => 8400,
        "13" => 10000,
        "14" => 11500,
        "15" => 13000,
        "16" => 15000,
        "17" => 18000,
        "18" => 20000,
        "19" => 22000,
        "20" => 25000,
        "21" => 33000,
        "22" => 41000,
        "23" => 50000,
        "24" => 62000,
        "25" => 75000,
        "26" => 90000,
        "27" => 105000,
        "28" => 120000,
        "29" => 135000,
        "30" => 155000,
        _ => 0,
    }
}

/// Convert size code to label.
pub fn size_label(code: &str) -> &str {
    match code {
        "T" => "Tiny",
        "S" => "Small",
        "M" => "Medium",
        "L" => "Large",
        "H" => "Huge",
        "G" => "Gargantuan",
        _ => code,
    }
}

/// Display-friendly alignment string.
pub fn alignment_display(alignment: &[serde_json::Value]) -> String {
    let codes: Vec<String> = alignment.iter()
        .filter_map(|v| v.as_str().map(String::from))
        .collect();
    if codes.is_empty() {
        return "unaligned".to_string();
    }
    let expanded: Vec<&str> = codes.iter().map(|c| match c.as_str() {
        "L" => "lawful",
        "N" => "neutral",
        "C" => "chaotic",
        "G" => "good",
        "E" => "evil",
        "U" => "unaligned",
        "A" => "any alignment",
        other => other,
    }).collect();
    expanded.join(" ")
}

/// Format a damage/resistance list for display.
pub fn damage_list_display(entries: &[serde_json::Value]) -> String {
    entries.iter().map(|e| match e {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Object(obj) => {
            let mut parts = Vec::new();
            if let Some(pre) = obj.get("preNote").and_then(|v| v.as_str()) {
                parts.push(pre.to_string());
            }
            for key in &["resist", "immune", "vulnerable"] {
                if let Some(arr) = obj.get(*key).and_then(|v| v.as_array()) {
                    let items: Vec<String> = arr.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect();
                    parts.push(items.join(", "));
                }
            }
            if let Some(note) = obj.get("note").and_then(|v| v.as_str()) {
                parts.push(note.to_string());
            }
            parts.join(" ")
        }
        _ => e.to_string(),
    }).collect::<Vec<_>>().join("; ")
}

impl Monster {
    /// Ability score modifier.
    pub fn modifier(score: u8) -> i8 {
        (score as i8 - 10) / 2
    }

    /// Format modifier with sign.
    pub fn modifier_str(score: u8) -> String {
        let m = Self::modifier(score);
        if m >= 0 { format!("+{}", m) } else { format!("{}", m) }
    }
}

// --- Merge system ---

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum MergeStrategy {
    Min,
    Max,
    ConcatA,
    ConcatB,
    TakeA,
    TakeB,
    Exclude,
}

impl MergeStrategy {
    pub const ALL: &'static [MergeStrategy] = &[
        MergeStrategy::Min,
        MergeStrategy::Max,
        MergeStrategy::ConcatA,
        MergeStrategy::ConcatB,
        MergeStrategy::TakeA,
        MergeStrategy::TakeB,
        MergeStrategy::Exclude,
    ];

    pub fn label(&self) -> &'static str {
        match self {
            MergeStrategy::Min => "Min",
            MergeStrategy::Max => "Max",
            MergeStrategy::ConcatA => "Concat (A first)",
            MergeStrategy::ConcatB => "Concat (B first)",
            MergeStrategy::TakeA => "Take A",
            MergeStrategy::TakeB => "Take B",
            MergeStrategy::Exclude => "Exclude",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MergeConfig {
    /// Default strategy for numeric fields
    pub default_numeric: MergeStrategy,
    /// Default strategy for list fields
    pub default_list: MergeStrategy,
    /// Default strategy for string fields
    pub default_string: MergeStrategy,
    /// Per-field overrides
    pub overrides: HashMap<String, MergeStrategy>,
}

impl Default for MergeConfig {
    fn default() -> Self {
        Self {
            default_numeric: MergeStrategy::Max,
            default_list: MergeStrategy::ConcatA,
            default_string: MergeStrategy::TakeA,
            overrides: HashMap::new(),
        }
    }
}

/// Human-readable field names for the merge config UI.
pub const MERGE_NUMERIC_FIELDS: &[(&str, &str)] = &[
    ("str_score", "Strength"),
    ("dex_score", "Dexterity"),
    ("con_score", "Constitution"),
    ("int_score", "Intelligence"),
    ("wis_score", "Wisdom"),
    ("cha_score", "Charisma"),
    ("ac", "AC"),
    ("hp_average", "HP"),
    ("passive", "Passive Perception"),
    ("walk", "Speed (Walk)"),
    ("fly", "Speed (Fly)"),
    ("swim", "Speed (Swim)"),
    ("climb", "Speed (Climb)"),
    ("burrow", "Speed (Burrow)"),
];

pub const MERGE_LIST_FIELDS: &[(&str, &str)] = &[
    ("traits", "Traits"),
    ("action", "Actions"),
    ("reaction", "Reactions"),
    ("legendary", "Legendary Actions"),
    ("immune", "Damage Immunities"),
    ("resist", "Damage Resistances"),
    ("vulnerable", "Vulnerabilities"),
    ("condition_immune", "Condition Immunities"),
    ("senses", "Senses"),
    ("languages", "Languages"),
];

pub const MERGE_STRING_FIELDS: &[(&str, &str)] = &[
    ("name", "Name"),
    ("monster_type", "Type"),
    ("cr", "Challenge Rating"),
];

fn merge_u8(a: u8, b: u8, strategy: &MergeStrategy) -> u8 {
    match strategy {
        MergeStrategy::Min => cmp::min(a, b),
        MergeStrategy::Max => cmp::max(a, b),
        MergeStrategy::TakeA => a,
        MergeStrategy::TakeB => b,
        MergeStrategy::Exclude => 0,
        _ => cmp::max(a, b),
    }
}

fn merge_opt_u8(a: Option<u8>, b: Option<u8>, strategy: &MergeStrategy) -> Option<u8> {
    match (a, b) {
        (Some(va), Some(vb)) => Some(merge_u8(va, vb, strategy)),
        (Some(va), None) => match strategy {
            MergeStrategy::TakeB | MergeStrategy::Exclude => None,
            _ => Some(va),
        },
        (None, Some(vb)) => match strategy {
            MergeStrategy::TakeA | MergeStrategy::Exclude => None,
            _ => Some(vb),
        },
        (None, None) => None,
    }
}

fn merge_speed_value(a: &SpeedValue, b: &SpeedValue, strategy: &MergeStrategy) -> SpeedValue {
    match (a.value(), b.value()) {
        (Some(va), Some(vb)) => {
            let val = match strategy {
                MergeStrategy::Min => cmp::min(va, vb),
                MergeStrategy::Max => cmp::max(va, vb),
                MergeStrategy::TakeA => va,
                MergeStrategy::TakeB => vb,
                MergeStrategy::Exclude => return SpeedValue::None,
                _ => cmp::max(va, vb),
            };
            SpeedValue::Simple(val)
        }
        (Some(va), None) => match strategy {
            MergeStrategy::TakeB | MergeStrategy::Exclude => SpeedValue::None,
            _ => SpeedValue::Simple(va),
        },
        (None, Some(vb)) => match strategy {
            MergeStrategy::TakeA | MergeStrategy::Exclude => SpeedValue::None,
            _ => SpeedValue::Simple(vb),
        },
        (None, None) => SpeedValue::None,
    }
}

fn merge_features(a: &[Feature], b: &[Feature], strategy: &MergeStrategy) -> Vec<Feature> {
    match strategy {
        MergeStrategy::ConcatA => {
            let mut result = a.to_vec();
            result.extend(b.iter().cloned());
            result
        }
        MergeStrategy::ConcatB => {
            let mut result = b.to_vec();
            result.extend(a.iter().cloned());
            result
        }
        MergeStrategy::TakeA => a.to_vec(),
        MergeStrategy::TakeB => b.to_vec(),
        MergeStrategy::Exclude => Vec::new(),
        _ => {
            let mut result = a.to_vec();
            result.extend(b.iter().cloned());
            result
        }
    }
}

fn merge_json_list(a: &[serde_json::Value], b: &[serde_json::Value], strategy: &MergeStrategy) -> Vec<serde_json::Value> {
    match strategy {
        MergeStrategy::ConcatA => {
            let mut result = a.to_vec();
            result.extend(b.iter().cloned());
            result
        }
        MergeStrategy::ConcatB => {
            let mut result = b.to_vec();
            result.extend(a.iter().cloned());
            result
        }
        MergeStrategy::TakeA => a.to_vec(),
        MergeStrategy::TakeB => b.to_vec(),
        MergeStrategy::Exclude => Vec::new(),
        _ => {
            let mut result = a.to_vec();
            result.extend(b.iter().cloned());
            result
        }
    }
}

fn merge_string_list(a: &[String], b: &[String], strategy: &MergeStrategy) -> Vec<String> {
    match strategy {
        MergeStrategy::ConcatA => {
            let mut result = a.to_vec();
            result.extend(b.iter().cloned());
            result
        }
        MergeStrategy::ConcatB => {
            let mut result = b.to_vec();
            result.extend(a.iter().cloned());
            result
        }
        MergeStrategy::TakeA => a.to_vec(),
        MergeStrategy::TakeB => b.to_vec(),
        MergeStrategy::Exclude => Vec::new(),
        _ => {
            let mut result = a.to_vec();
            result.extend(b.iter().cloned());
            result
        }
    }
}

impl MergeConfig {
    fn strategy_for(&self, field: &str, field_type: &str) -> &MergeStrategy {
        if let Some(s) = self.overrides.get(field) {
            return s;
        }
        match field_type {
            "numeric" => &self.default_numeric,
            "list" => &self.default_list,
            "string" => &self.default_string,
            _ => &self.default_numeric,
        }
    }
}

/// Merge two monsters according to the given config.
pub fn merge_monsters(a: &Monster, b: &Monster, config: &MergeConfig) -> Monster {
    let name_strategy = config.strategy_for("name", "string");
    let name = match name_strategy {
        MergeStrategy::TakeA => a.name.clone(),
        MergeStrategy::TakeB => b.name.clone(),
        MergeStrategy::Exclude => String::new(),
        _ => format!("{} + {}", a.name, b.name),
    };

    let type_strategy = config.strategy_for("monster_type", "string");
    let monster_type = match type_strategy {
        MergeStrategy::TakeB => b.monster_type.clone(),
        MergeStrategy::Exclude => MonsterType::default(),
        _ => a.monster_type.clone(),
    };

    let cr_strategy = config.strategy_for("cr", "string");
    let cr = match cr_strategy {
        MergeStrategy::TakeB => b.cr.clone(),
        MergeStrategy::Max => {
            if a.cr.cr_numeric() >= b.cr.cr_numeric() { a.cr.clone() } else { b.cr.clone() }
        }
        MergeStrategy::Min => {
            if a.cr.cr_numeric() <= b.cr.cr_numeric() { a.cr.clone() } else { b.cr.clone() }
        }
        MergeStrategy::Exclude => ChallengeRating::default(),
        _ => a.cr.clone(),
    };

    // Size: take A by default
    let size_strategy = config.strategy_for("size", "string");
    let size = match size_strategy {
        MergeStrategy::TakeB => b.size.clone(),
        _ => a.size.clone(),
    };

    // Ability scores
    let str_score = merge_u8(a.str_score, b.str_score, config.strategy_for("str_score", "numeric"));
    let dex_score = merge_u8(a.dex_score, b.dex_score, config.strategy_for("dex_score", "numeric"));
    let con_score = merge_u8(a.con_score, b.con_score, config.strategy_for("con_score", "numeric"));
    let int_score = merge_u8(a.int_score, b.int_score, config.strategy_for("int_score", "numeric"));
    let wis_score = merge_u8(a.wis_score, b.wis_score, config.strategy_for("wis_score", "numeric"));
    let cha_score = merge_u8(a.cha_score, b.cha_score, config.strategy_for("cha_score", "numeric"));

    // AC: take the first AC entry with the merge strategy
    let ac_strategy = config.strategy_for("ac", "numeric");
    let ac = match ac_strategy {
        MergeStrategy::TakeB => b.ac.clone(),
        MergeStrategy::TakeA => a.ac.clone(),
        MergeStrategy::Exclude => Vec::new(),
        MergeStrategy::Max => {
            let a_val = a.ac.first().and_then(|ac| ac.value()).unwrap_or(0);
            let b_val = b.ac.first().and_then(|ac| ac.value()).unwrap_or(0);
            if a_val >= b_val { a.ac.clone() } else { b.ac.clone() }
        }
        MergeStrategy::Min => {
            let a_val = a.ac.first().and_then(|ac| ac.value()).unwrap_or(0);
            let b_val = b.ac.first().and_then(|ac| ac.value()).unwrap_or(0);
            if a_val <= b_val { a.ac.clone() } else { b.ac.clone() }
        }
        _ => a.ac.clone(),
    };

    // HP: merge average, take formula from the higher
    let hp_strategy = config.strategy_for("hp_average", "numeric");
    let hp = match (&a.hp, &b.hp) {
        (HitPoints::Formula { average: avg_a, formula: f_a }, HitPoints::Formula { average: avg_b, formula: f_b }) => {
            match hp_strategy {
                MergeStrategy::Max => {
                    if avg_a >= avg_b {
                        HitPoints::Formula { average: *avg_a, formula: f_a.clone() }
                    } else {
                        HitPoints::Formula { average: *avg_b, formula: f_b.clone() }
                    }
                }
                MergeStrategy::Min => {
                    if avg_a <= avg_b {
                        HitPoints::Formula { average: *avg_a, formula: f_a.clone() }
                    } else {
                        HitPoints::Formula { average: *avg_b, formula: f_b.clone() }
                    }
                }
                MergeStrategy::TakeA => HitPoints::Formula { average: *avg_a, formula: f_a.clone() },
                MergeStrategy::TakeB => HitPoints::Formula { average: *avg_b, formula: f_b.clone() },
                MergeStrategy::Exclude => HitPoints::Unknown,
                _ => HitPoints::Formula { average: cmp::max(*avg_a, *avg_b), formula: f_a.clone() },
            }
        }
        _ => match hp_strategy {
            MergeStrategy::TakeB => b.hp.clone(),
            _ => a.hp.clone(),
        },
    };

    // Speed
    let speed = Speed {
        walk: merge_speed_value(&a.speed.walk, &b.speed.walk, config.strategy_for("walk", "numeric")),
        fly: merge_speed_value(&a.speed.fly, &b.speed.fly, config.strategy_for("fly", "numeric")),
        swim: merge_speed_value(&a.speed.swim, &b.speed.swim, config.strategy_for("swim", "numeric")),
        climb: merge_speed_value(&a.speed.climb, &b.speed.climb, config.strategy_for("climb", "numeric")),
        burrow: merge_speed_value(&a.speed.burrow, &b.speed.burrow, config.strategy_for("burrow", "numeric")),
        can_hover: a.speed.can_hover || b.speed.can_hover,
    };

    // Passive perception
    let passive = merge_opt_u8(a.passive, b.passive, config.strategy_for("passive", "numeric"));

    // Saves and skills: merge hashmaps
    let mut save = a.save.clone();
    for (k, v) in &b.save {
        save.entry(k.clone()).or_insert_with(|| v.clone());
    }
    let mut skill = a.skill.clone();
    for (k, v) in &b.skill {
        skill.entry(k.clone()).or_insert_with(|| v.clone());
    }

    // List fields
    let traits = merge_features(&a.traits, &b.traits, config.strategy_for("traits", "list"));
    let action = merge_features(&a.action, &b.action, config.strategy_for("action", "list"));
    let reaction = merge_features(&a.reaction, &b.reaction, config.strategy_for("reaction", "list"));
    let legendary = merge_features(&a.legendary, &b.legendary, config.strategy_for("legendary", "list"));
    let immune = merge_json_list(&a.immune, &b.immune, config.strategy_for("immune", "list"));
    let resist = merge_json_list(&a.resist, &b.resist, config.strategy_for("resist", "list"));
    let vulnerable = merge_json_list(&a.vulnerable, &b.vulnerable, config.strategy_for("vulnerable", "list"));
    let condition_immune = merge_json_list(&a.condition_immune, &b.condition_immune, config.strategy_for("condition_immune", "list"));
    let senses = merge_string_list(&a.senses, &b.senses, config.strategy_for("senses", "list"));
    let languages = merge_string_list(&a.languages, &b.languages, config.strategy_for("languages", "list"));

    Monster {
        name,
        source: "Custom".to_string(),
        page: None,
        size,
        monster_type,
        alignment: a.alignment.clone(),
        str_score,
        dex_score,
        con_score,
        int_score,
        wis_score,
        cha_score,
        ac,
        hp,
        speed,
        cr,
        save,
        skill,
        senses,
        passive,
        languages,
        immune,
        resist,
        vulnerable,
        condition_immune,
        traits,
        action,
        reaction,
        legendary,
        mythic: Vec::new(),
        spellcasting: Vec::new(),
        environment: Vec::new(),
    }
}

