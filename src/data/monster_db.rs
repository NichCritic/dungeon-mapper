use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use crate::model::monster::Monster;

/// In-memory database of monsters loaded from 5e-Tools bestiary JSON files.
pub struct MonsterDatabase {
    /// All monsters, sorted by name.
    monsters: Vec<Monster>,
    /// Base image directory (e.g. `5etools-src/img/`).
    /// Token images live at `{img_dir}/bestiary/tokens/{source}/{name}.webp`.
    pub img_dir: Option<PathBuf>,
}

impl MonsterDatabase {
    /// Create an empty database.
    pub fn empty() -> Self {
        Self {
            monsters: Vec::new(),
            img_dir: None,
        }
    }

    /// Load all bestiary-*.json files from the given directory.
    /// Resolves `_copy` inheritance and `_mod` operations at the JSON level
    /// before deserializing into typed `Monster` structs.
    pub fn load_from_directory(dir: &Path) -> Self {
        if !dir.is_dir() {
            eprintln!("Warning: bestiary directory not found: {}", dir.display());
            return Self::empty();
        }

        // Phase 1: Load all files as raw JSON
        let mut raw_monsters: Vec<Value> = Vec::new();
        let mut file_entries: Vec<_> = std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .filter_map(|e| e.ok())
            .filter(|e| {
                let name = e.file_name();
                let name = name.to_string_lossy();
                name.starts_with("bestiary-") && name.ends_with(".json")
            })
            .collect();
        file_entries.sort_by_key(|e| e.file_name());

        for entry in &file_entries {
            match std::fs::read_to_string(entry.path()) {
                Ok(contents) => {
                    match serde_json::from_str::<Value>(&contents) {
                        Ok(val) => {
                            if let Some(arr) = val.get("monster").and_then(|v| v.as_array()) {
                                let count = arr.len();
                                raw_monsters.extend(arr.iter().cloned());
                                eprintln!(
                                    "Loaded {} monsters from {}",
                                    count,
                                    entry.file_name().to_string_lossy()
                                );
                            }
                        }
                        Err(e) => {
                            eprintln!(
                                "Warning: failed to parse {}: {}",
                                entry.file_name().to_string_lossy(),
                                e
                            );
                        }
                    }
                }
                Err(e) => {
                    eprintln!(
                        "Warning: failed to read {}: {}",
                        entry.file_name().to_string_lossy(),
                        e
                    );
                }
            }
        }

        // Phase 2: Build lookup map for _copy resolution
        let mut lookup: HashMap<(String, String), Value> = HashMap::new();
        for m in &raw_monsters {
            if m.get("_copy").is_some() {
                continue; // Don't index copy-monsters as bases
            }
            if let (Some(name), Some(source)) = (
                m.get("name").and_then(|v| v.as_str()),
                m.get("source").and_then(|v| v.as_str()),
            ) {
                lookup.insert((source.to_string(), name.to_string()), m.clone());
            }
        }

        // Phase 3: Resolve _copy references
        let mut resolved: Vec<Value> = Vec::with_capacity(raw_monsters.len());
        let mut copy_resolved = 0;
        let mut copy_failed = 0;

        for m in raw_monsters {
            if let Some(copy_spec) = m.get("_copy").cloned() {
                let base_name = copy_spec.get("name").and_then(|v| v.as_str()).unwrap_or("");
                let base_source = copy_spec.get("source").and_then(|v| v.as_str()).unwrap_or("");

                if let Some(base) = lookup.get(&(base_source.to_string(), base_name.to_string())) {
                    let mut result = base.clone();

                    // Merge top-level fields from the copy monster (overrides base)
                    if let (Some(result_obj), Some(m_obj)) =
                        (result.as_object_mut(), m.as_object())
                    {
                        for (key, val) in m_obj {
                            if key == "_copy" {
                                continue;
                            }
                            result_obj.insert(key.clone(), val.clone());
                        }
                    }

                    // Apply _mod operations
                    if let Some(mods) = copy_spec.get("_mod") {
                        if let Some(mod_obj) = mods.as_object() {
                            apply_mods(result.as_object_mut().unwrap(), mod_obj);
                        }
                    }

                    resolved.push(result);
                    copy_resolved += 1;
                } else {
                    // Base not found — skip this monster
                    copy_failed += 1;
                }
            } else {
                resolved.push(m);
            }
        }

        if copy_resolved > 0 {
            eprintln!("Resolved {} _copy references", copy_resolved);
        }
        if copy_failed > 0 {
            eprintln!("Warning: {} _copy references could not be resolved (base not found)", copy_failed);
        }

        // Phase 4: Deserialize resolved JSON into Monster structs
        let mut monsters = Vec::with_capacity(resolved.len());
        let mut parse_errors = 0;
        for val in resolved {
            let name_str = val.get("name").and_then(|v| v.as_str()).unwrap_or("?").to_string();
            let source_str = val.get("source").and_then(|v| v.as_str()).unwrap_or("?").to_string();
            match serde_json::from_value::<Monster>(val) {
                Ok(m) => monsters.push(m),
                Err(e) => {
                    parse_errors += 1;
                    if parse_errors <= 5 {
                        eprintln!("Warning: failed to deserialize {} ({}): {}", name_str, source_str, e);
                    }
                }
            }
        }
        if parse_errors > 5 {
            eprintln!("... and {} more deserialization errors", parse_errors - 5);
        }

        monsters.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        eprintln!("Monster database: {} total monsters loaded", monsters.len());

        // Compute image directory: bestiary dir is e.g. 5etools-src/data/bestiary
        // img dir is 5etools-src/img/
        let img_dir = dir.parent()   // data/
            .and_then(|p| p.parent()) // 5etools-src/
            .map(|p| p.join("img"))
            .filter(|p| p.is_dir());

        Self {
            monsters,
            img_dir,
        }
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.monsters.len()
    }

    pub fn is_empty(&self) -> bool {
        self.monsters.is_empty()
    }

    /// Get all monsters (for iteration).
    pub fn all(&self) -> &[Monster] {
        &self.monsters
    }

    /// Find a monster by source and name (exact match).
    pub fn find(&self, source: &str, name: &str) -> Option<&Monster> {
        self.monsters.iter().find(|m| m.source == source && m.name == name)
    }

    /// Return the path to a monster's token image if it exists on disk.
    /// Token images are at `{img_dir}/bestiary/tokens/{source}/{name}.webp`.
    pub fn token_path(&self, source: &str, name: &str) -> Option<PathBuf> {
        let img_dir = self.img_dir.as_ref()?;
        let path = img_dir
            .join("bestiary")
            .join("tokens")
            .join(source)
            .join(format!("{}.webp", name));
        if path.is_file() {
            Some(path)
        } else {
            None
        }
    }

    /// Filter monsters by criteria.
    pub fn filter(&self, filter: &MonsterFilter) -> Vec<&Monster> {
        self.monsters
            .iter()
            .filter(|m| filter.matches(m))
            .collect()
    }
}

// --- _mod operation implementation ---

/// Apply all _mod operations to a monster JSON object.
fn apply_mods(monster: &mut Map<String, Value>, mods: &Map<String, Value>) {
    for (field, ops) in mods {
        if field == "*" {
            // Wildcard: apply to all string-containing fields
            apply_wildcard_ops(monster, ops);
        } else {
            apply_field_ops(monster, field, ops);
        }
    }
}

/// Apply operations to a specific field.
fn apply_field_ops(monster: &mut Map<String, Value>, field: &str, ops: &Value) {
    let ops_list = match ops {
        Value::Array(arr) => arr.clone(),
        Value::Object(_) => vec![ops.clone()],
        _ => return,
    };

    for op in &ops_list {
        let Some(mode) = op.get("mode").and_then(|v| v.as_str()) else {
            continue;
        };
        match mode {
            "replaceArr" => {
                let replace_name = op.get("replace").and_then(|v| v.as_str()).unwrap_or("");
                let items = op.get("items");
                if let Some(arr) = monster.get_mut(field).and_then(|v| v.as_array_mut()) {
                    if let Some(idx) = arr.iter().position(|item| {
                        item.get("name").and_then(|n| n.as_str()) == Some(replace_name)
                    }) {
                        if let Some(new_items) = items {
                            // If items is an array, splice all elements in; otherwise replace single
                            arr.remove(idx);
                            if let Some(new_arr) = new_items.as_array() {
                                for (i, item) in new_arr.iter().enumerate() {
                                    arr.insert(idx + i, item.clone());
                                }
                            } else {
                                arr.insert(idx, new_items.clone());
                            }
                        } else {
                            arr.remove(idx);
                        }
                    }
                }
            }
            "appendArr" => {
                if let Some(items) = op.get("items") {
                    let arr = monster
                        .entry(field.to_string())
                        .or_insert_with(|| Value::Array(Vec::new()));
                    if let Some(arr) = arr.as_array_mut() {
                        match items {
                            Value::Array(new_items) => arr.extend(new_items.iter().cloned()),
                            _ => arr.push(items.clone()),
                        }
                    }
                }
            }
            "prependArr" => {
                if let Some(items) = op.get("items") {
                    let arr = monster
                        .entry(field.to_string())
                        .or_insert_with(|| Value::Array(Vec::new()));
                    if let Some(arr) = arr.as_array_mut() {
                        match items {
                            Value::Array(new_items) => {
                                for (i, item) in new_items.iter().enumerate() {
                                    arr.insert(i, item.clone());
                                }
                            }
                            _ => arr.insert(0, items.clone()),
                        }
                    }
                }
            }
            "appendIfNotExistsArr" => {
                if let Some(items) = op.get("items") {
                    let arr = monster
                        .entry(field.to_string())
                        .or_insert_with(|| Value::Array(Vec::new()));
                    if let Some(arr) = arr.as_array_mut() {
                        let new_items = match items {
                            Value::Array(v) => v.clone(),
                            _ => vec![items.clone()],
                        };
                        for item in new_items {
                            let item_name = item.get("name").and_then(|n| n.as_str());
                            let exists = arr.iter().any(|existing| {
                                existing.get("name").and_then(|n| n.as_str()) == item_name
                            });
                            if !exists {
                                arr.push(item);
                            }
                        }
                    }
                }
            }
            "insertArr" => {
                if let Some(items) = op.get("items") {
                    let index = op.get("index").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
                    let arr = monster
                        .entry(field.to_string())
                        .or_insert_with(|| Value::Array(Vec::new()));
                    if let Some(arr) = arr.as_array_mut() {
                        let idx = index.min(arr.len());
                        match items {
                            Value::Array(new_items) => {
                                for (i, item) in new_items.iter().enumerate() {
                                    arr.insert(idx + i, item.clone());
                                }
                            }
                            _ => arr.insert(idx, items.clone()),
                        }
                    }
                }
            }
            "removeArr" => {
                // Can remove by "names" (string or array) or by "items"
                if let Some(arr) = monster.get_mut(field).and_then(|v| v.as_array_mut()) {
                    if let Some(names) = op.get("names") {
                        let names_list: Vec<&str> = match names {
                            Value::String(s) => vec![s.as_str()],
                            Value::Array(a) => a.iter().filter_map(|v| v.as_str()).collect(),
                            _ => vec![],
                        };
                        arr.retain(|item| {
                            let item_name = item.get("name").and_then(|n| n.as_str()).unwrap_or("");
                            !names_list.contains(&item_name)
                        });
                    }
                    if let Some(items) = op.get("items").and_then(|v| v.as_array()) {
                        for remove_item in items {
                            arr.retain(|item| item != remove_item);
                        }
                    }
                }
            }
            "replaceTxt" => {
                let replace = op.get("replace").and_then(|v| v.as_str()).unwrap_or("");
                let with = op.get("with").and_then(|v| v.as_str()).unwrap_or("");
                let case_insensitive = op
                    .get("flags")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .contains('i');
                if let Some(val) = monster.get_mut(field) {
                    replace_text_in_value(val, replace, with, case_insensitive);
                }
            }
            "setProp" => {
                // Set arbitrary properties
                if let Some(props) = op.get("prop").and_then(|v| v.as_object()) {
                    for (k, v) in props {
                        monster.insert(k.clone(), v.clone());
                    }
                }
            }
            "addSkills" => {
                if let Some(skills) = op.get("skills").and_then(|v| v.as_object()) {
                    let skill_obj = monster
                        .entry("skill".to_string())
                        .or_insert_with(|| Value::Object(Map::new()));
                    if let Some(existing) = skill_obj.as_object_mut() {
                        for (k, v) in skills {
                            existing.insert(k.clone(), v.clone());
                        }
                    }
                }
            }
            _ => {
                // addSpells, removeSpells, replaceSpells — less common, skip for now
            }
        }
    }
}

/// Apply wildcard operations (field = "*") to all relevant fields on the monster.
fn apply_wildcard_ops(monster: &mut Map<String, Value>, ops: &Value) {
    let ops_list = match ops {
        Value::Array(arr) => arr.clone(),
        Value::Object(_) => vec![ops.clone()],
        _ => return,
    };

    for op in &ops_list {
        let Some(mode) = op.get("mode").and_then(|v| v.as_str()) else {
            continue;
        };
        if mode == "replaceTxt" {
            let replace = op.get("replace").and_then(|v| v.as_str()).unwrap_or("");
            let with = op.get("with").and_then(|v| v.as_str()).unwrap_or("");
            let case_insensitive = op
                .get("flags")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .contains('i');

            // Apply to all values in the monster
            let keys: Vec<String> = monster.keys().cloned().collect();
            for key in keys {
                if let Some(val) = monster.get_mut(&key) {
                    replace_text_in_value(val, replace, with, case_insensitive);
                }
            }
        }
    }
}

/// Recursively replace text within a JSON value.
fn replace_text_in_value(val: &mut Value, find: &str, replace: &str, case_insensitive: bool) {
    match val {
        Value::String(s) => {
            if case_insensitive {
                // Case-insensitive replace
                let lower = s.to_lowercase();
                let find_lower = find.to_lowercase();
                let mut result = String::with_capacity(s.len());
                let mut search_start = 0;
                while let Some(pos) = lower[search_start..].find(&find_lower) {
                    let abs_pos = search_start + pos;
                    result.push_str(&s[search_start..abs_pos]);
                    result.push_str(replace);
                    search_start = abs_pos + find.len();
                }
                result.push_str(&s[search_start..]);
                *s = result;
            } else {
                *s = s.replace(find, replace);
            }
        }
        Value::Array(arr) => {
            for item in arr {
                replace_text_in_value(item, find, replace, case_insensitive);
            }
        }
        Value::Object(obj) => {
            for (_, v) in obj.iter_mut() {
                replace_text_in_value(v, find, replace, case_insensitive);
            }
        }
        _ => {}
    }
}

/// Source codes for 2024-revised core rulebooks.
pub const SOURCES_2024: &[&str] = &["XMM", "XPHB", "XDMG"];

/// Filter criteria for searching the monster database.
#[derive(Default)]
pub struct MonsterFilter {
    pub name_query: String,
    pub cr_min: Option<f32>,
    pub cr_max: Option<f32>,
    pub size: Option<String>,
    pub monster_type: Option<String>,
    pub source: Option<String>,
    /// When true, only include monsters from 2024-revised sources.
    pub only_2024: bool,
}

impl MonsterFilter {
    pub fn matches(&self, monster: &Monster) -> bool {
        if !self.name_query.is_empty() {
            let query = self.name_query.to_lowercase();
            if !monster.name.to_lowercase().contains(&query) {
                return false;
            }
        }

        let cr = monster.cr.cr_numeric();
        if let Some(min) = self.cr_min {
            if cr < min {
                return false;
            }
        }
        if let Some(max) = self.cr_max {
            if cr > max {
                return false;
            }
        }

        if let Some(ref size) = self.size {
            if !monster.size.contains(size) {
                return false;
            }
        }

        if let Some(ref type_filter) = self.monster_type {
            let type_lower = type_filter.to_lowercase();
            let monster_type = monster.monster_type.display().to_lowercase();
            if !monster_type.contains(&type_lower) {
                return false;
            }
        }

        if let Some(ref source) = self.source {
            let source_lower = source.to_lowercase();
            if !monster.source.to_lowercase().contains(&source_lower) {
                return false;
            }
        }

        if self.only_2024 && !SOURCES_2024.contains(&monster.source.as_str()) {
            return false;
        }

        true
    }

    pub fn is_active(&self) -> bool {
        !self.name_query.is_empty()
            || self.cr_min.is_some()
            || self.cr_max.is_some()
            || self.size.is_some()
            || self.monster_type.is_some()
            || self.source.is_some()
            || self.only_2024
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_load_bestiary() {
        let dir = std::path::Path::new("5etools-src/data/bestiary");
        if !dir.is_dir() {
            eprintln!("Skipping test: bestiary directory not found");
            return;
        }
        let db = MonsterDatabase::load_from_directory(dir);
        assert!(db.len() > 1000, "Expected >1000 monsters, got {}", db.len());

        // Spot check a well-known monster
        let dragon = db.find("MM", "Ancient Red Dragon");
        assert!(dragon.is_some(), "Ancient Red Dragon not found");
        let dragon = dragon.unwrap();
        assert_eq!(dragon.str_score, 30);
        assert_eq!(dragon.cr.cr_string(), "24");
    }

    #[test]
    fn test_copy_resolution() {
        let dir = std::path::Path::new("5etools-src/data/bestiary");
        if !dir.is_dir() {
            eprintln!("Skipping test: bestiary directory not found");
            return;
        }
        let db = MonsterDatabase::load_from_directory(dir);

        // Adult Amonkhet Dragon copies Adult Red Dragon but overrides INT
        let amonkhet = db.find("PSA", "Adult Amonkhet Dragon");
        assert!(amonkhet.is_some(), "Adult Amonkhet Dragon not found");
        let amonkhet = amonkhet.unwrap();

        // Should have inherited stats from Adult Red Dragon
        let base = db.find("MM", "Adult Red Dragon");
        assert!(base.is_some());
        let base = base.unwrap();

        // STR should be inherited
        assert_eq!(amonkhet.str_score, base.str_score);
        // INT should be overridden to 8
        assert_eq!(amonkhet.int_score, 8);
        // Should have actions inherited
        assert!(!amonkhet.action.is_empty(), "Should inherit actions from base");
        // CR should be inherited
        assert_eq!(amonkhet.cr.cr_string(), base.cr.cr_string());
    }

    fn make_test_monster(name: &str, source: &str, cr: &str, size: &str, mtype: &str) -> Monster {
        use crate::model::monster::*;
        use std::collections::HashMap;
        Monster {
            name: name.to_string(),
            source: source.to_string(),
            page: None,
            size: vec![size.to_string()],
            monster_type: MonsterType::Simple(mtype.to_string()),
            alignment: Vec::new(),
            str_score: 10,
            dex_score: 10,
            con_score: 10,
            int_score: 10,
            wis_score: 10,
            cha_score: 10,
            ac: vec![ArmorClass::Simple(10)],
            hp: HitPoints::default(),
            speed: Speed::default(),
            cr: ChallengeRating::Simple(cr.to_string()),
            save: HashMap::new(),
            skill: HashMap::new(),
            senses: Vec::new(),
            passive: None,
            languages: Vec::new(),
            immune: Vec::new(),
            resist: Vec::new(),
            vulnerable: Vec::new(),
            condition_immune: Vec::new(),
            traits: Vec::new(),
            action: Vec::new(),
            reaction: Vec::new(),
            legendary: Vec::new(),
            mythic: Vec::new(),
            spellcasting: Vec::new(),
            environment: Vec::new(),
        }
    }

    #[test]
    fn test_empty_database() {
        let db = MonsterDatabase::empty();
        assert_eq!(db.len(), 0);
        assert!(db.is_empty());
        assert!(db.find("MM", "Anything").is_none());
    }

    #[test]
    fn test_monster_filter_name_case_insensitive() {
        let m = make_test_monster("Ancient Red Dragon", "MM", "24", "H", "dragon");
        let filter = MonsterFilter {
            name_query: "ancient red".to_string(),
            ..MonsterFilter::default()
        };
        assert!(filter.matches(&m));

        let filter_upper = MonsterFilter {
            name_query: "ANCIENT RED".to_string(),
            ..MonsterFilter::default()
        };
        assert!(filter_upper.matches(&m));
    }

    #[test]
    fn test_monster_filter_cr_range() {
        let m = make_test_monster("Goblin", "MM", "1/4", "S", "humanoid");
        let filter = MonsterFilter {
            cr_min: Some(0.0),
            cr_max: Some(0.5),
            ..MonsterFilter::default()
        };
        assert!(filter.matches(&m)); // 1/4 = 0.25, within range

        let filter_high = MonsterFilter {
            cr_min: Some(1.0),
            ..MonsterFilter::default()
        };
        assert!(!filter_high.matches(&m));
    }

    #[test]
    fn test_monster_filter_size() {
        let m = make_test_monster("Goblin", "MM", "1/4", "S", "humanoid");
        let filter = MonsterFilter {
            size: Some("S".to_string()),
            ..MonsterFilter::default()
        };
        assert!(filter.matches(&m));

        let filter_wrong = MonsterFilter {
            size: Some("L".to_string()),
            ..MonsterFilter::default()
        };
        assert!(!filter_wrong.matches(&m));
    }

    #[test]
    fn test_monster_filter_type() {
        let m = make_test_monster("Goblin", "MM", "1/4", "S", "humanoid");
        let filter = MonsterFilter {
            monster_type: Some("humanoid".to_string()),
            ..MonsterFilter::default()
        };
        assert!(filter.matches(&m));

        let filter_wrong = MonsterFilter {
            monster_type: Some("dragon".to_string()),
            ..MonsterFilter::default()
        };
        assert!(!filter_wrong.matches(&m));
    }

    #[test]
    fn test_monster_filter_source() {
        let m = make_test_monster("Goblin", "MM", "1/4", "S", "humanoid");
        let filter = MonsterFilter {
            source: Some("MM".to_string()),
            ..MonsterFilter::default()
        };
        assert!(filter.matches(&m));

        let filter_wrong = MonsterFilter {
            source: Some("VGM".to_string()),
            ..MonsterFilter::default()
        };
        assert!(!filter_wrong.matches(&m));
    }

    #[test]
    fn test_monster_filter_combined() {
        let m = make_test_monster("Goblin", "MM", "1/4", "S", "humanoid");
        let filter = MonsterFilter {
            name_query: "goblin".to_string(),
            cr_max: Some(1.0),
            size: Some("S".to_string()),
            source: Some("MM".to_string()),
            ..MonsterFilter::default()
        };
        assert!(filter.matches(&m));
    }

    #[test]
    fn test_monster_filter_is_active() {
        let default_filter = MonsterFilter::default();
        assert!(!default_filter.is_active());

        let name_filter = MonsterFilter {
            name_query: "dragon".to_string(),
            ..MonsterFilter::default()
        };
        assert!(name_filter.is_active());

        let cr_filter = MonsterFilter {
            cr_min: Some(1.0),
            ..MonsterFilter::default()
        };
        assert!(cr_filter.is_active());
    }

    #[test]
    fn test_copy_with_replace_txt() {
        let dir = std::path::Path::new("5etools-src/data/bestiary");
        if !dir.is_dir() {
            eprintln!("Skipping test: bestiary directory not found");
            return;
        }
        let db = MonsterDatabase::load_from_directory(dir);

        // Bitter Breath copies Horned Devil with replaceTxt "the devil" -> "Bitter Breath"
        let bitter = db.find("BGDIA", "Bitter Breath");
        assert!(bitter.is_some(), "Bitter Breath not found");
        let bitter = bitter.unwrap();

        // Should have inherited Horned Devil stats
        assert!(!bitter.action.is_empty(), "Should have actions");

        // Text replacement should have happened
        let all_text: String = bitter.action.iter()
            .flat_map(|a| a.entries.iter())
            .filter_map(|e| e.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        // "the devil" should have been replaced
        assert!(
            !all_text.to_lowercase().contains("the devil"),
            "replaceTxt should have replaced 'the devil': {}",
            &all_text[..200.min(all_text.len())]
        );
    }
}
