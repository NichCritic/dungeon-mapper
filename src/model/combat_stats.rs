#![allow(dead_code)]

use std::collections::HashMap;

use regex::Regex;

use super::monster::{Monster, HitPoints};

/// A parsed attack from a monster's action entries.
#[derive(Clone, Debug)]
pub struct ParsedAttack {
    pub name: String,
    /// "mw", "rw", "ms", "rs" (melee/ranged weapon/spell)
    pub attack_type: String,
    pub to_hit: i8,
    pub reach: Option<u32>,
    pub range: Option<(u32, u32)>,
    pub damage_dice: String,
    pub damage_avg: f32,
    pub damage_type: String,
    pub extra_damage: Vec<DamageRider>,
}

/// Additional damage on an attack (e.g. "plus 7 (2d6) fire damage").
#[derive(Clone, Debug)]
pub struct DamageRider {
    pub damage_dice: String,
    pub damage_avg: f32,
    pub damage_type: String,
}

/// A saving throw DC extracted from a trait or action.
#[derive(Clone, Debug)]
pub struct ParsedSave {
    pub dc: u8,
    pub ability: String,
    pub source: String,
}

/// Structured combat stats parsed from a Monster's text fields.
#[derive(Clone, Debug)]
pub struct CombatStats {
    pub ac: Option<u8>,
    pub max_hp: i32,
    pub hp_formula: String,
    pub attacks: Vec<ParsedAttack>,
    pub saving_throws: Vec<ParsedSave>,
    pub multiattack_count: u8,
    /// Estimated damage per round (best attack avg * multiattack count).
    pub estimated_dpr: f32,
}

/// Parse structured combat stats from a Monster's fields.
pub fn parse_combat_stats(monster: &Monster) -> CombatStats {
    let ac = monster.ac.first().and_then(|a| a.value());

    let (max_hp, hp_formula) = match &monster.hp {
        HitPoints::Formula { average, formula } => (*average, formula.clone()),
        _ => (0, String::new()),
    };

    let mut attacks = Vec::new();
    let mut saving_throws = Vec::new();
    let mut multiattack_count: u8 = 1;

    for action in &monster.action {
        let text = action.entries_text();

        // Parse multiattack
        if action.name.to_lowercase().contains("multiattack") {
            multiattack_count = parse_multiattack_count(&text);
            continue;
        }

        // Parse attack actions
        if let Some(attack) = parse_attack(&action.name, &text) {
            attacks.push(attack);
        }

        // Parse saving throw DCs
        for save in parse_saving_throws(&action.name, &text) {
            saving_throws.push(save);
        }
    }

    // Also scan traits for saving throws
    for tr in &monster.traits {
        let text = tr.entries_text();
        for save in parse_saving_throws(&tr.name, &text) {
            saving_throws.push(save);
        }
    }

    // Estimate DPR: best single-attack total damage * multiattack count
    let best_attack_dmg = attacks.iter()
        .map(|a| {
            let extra: f32 = a.extra_damage.iter().map(|d| d.damage_avg).sum();
            a.damage_avg + extra
        })
        .fold(0.0_f32, f32::max);
    let estimated_dpr = best_attack_dmg * multiattack_count as f32;

    CombatStats {
        ac,
        max_hp,
        hp_formula,
        attacks,
        saving_throws,
        multiattack_count,
        estimated_dpr,
    }
}

/// Parse an attack action from stripped text.
fn parse_attack(name: &str, text: &str) -> Option<ParsedAttack> {
    // After strip_5e_markup: "+4 to hit, reach 5 ft., one target. 5 (1d6 + 2) slashing damage."
    let hit_re = Regex::new(r"\+(\d+) to hit").unwrap();
    let hit_cap = hit_re.captures(text)?;
    let to_hit: i8 = hit_cap[1].parse().ok()?;

    let reach_re = Regex::new(r"reach (\d+) ft\.").unwrap();
    let reach = reach_re.captures(text).and_then(|c| c[1].parse().ok());

    let range_re = Regex::new(r"range (\d+)/(\d+) ft\.").unwrap();
    let range = range_re.captures(text).and_then(|c| {
        let normal: u32 = c[1].parse().ok()?;
        let long: u32 = c[2].parse().ok()?;
        Some((normal, long))
    });

    // Primary damage: "19 (2d10 + 8) piercing damage"
    let dmg_re = Regex::new(r"(\d+) \((\d+d\d+(?:\s*[+-]\s*\d+)?)\) (\w+) damage").unwrap();
    let dmg_cap = dmg_re.captures(text)?;
    let damage_avg: f32 = dmg_cap[1].parse().ok()?;
    let damage_dice = dmg_cap[2].to_string();
    let damage_type = dmg_cap[3].to_string();

    // Extra damage riders: "plus 7 (2d6) fire damage"
    let extra_re = Regex::new(r"plus (\d+) \((\d+d\d+(?:\s*[+-]\s*\d+)?)\) (\w+) damage").unwrap();
    let extra_damage: Vec<DamageRider> = extra_re.captures_iter(text).map(|c| {
        DamageRider {
            damage_avg: c[1].parse().unwrap_or(0.0),
            damage_dice: c[2].to_string(),
            damage_type: c[3].to_string(),
        }
    }).collect();

    // Determine attack type from presence of reach/range
    let attack_type = if range.is_some() { "rw" } else { "mw" }.to_string();

    Some(ParsedAttack {
        name: name.to_string(),
        attack_type,
        to_hit,
        reach,
        range,
        damage_dice,
        damage_avg,
        damage_type,
        extra_damage,
    })
}

/// Parse saving throw DCs from text.
fn parse_saving_throws(source_name: &str, text: &str) -> Vec<ParsedSave> {
    // After strip_5e_markup: "DC 19 Wisdom saving throw"
    let dc_re = Regex::new(r"DC (\d+) (\w+)").unwrap();
    dc_re.captures_iter(text).filter_map(|c| {
        let dc: u8 = c[1].parse().ok()?;
        let ability = c[2].to_string();
        Some(ParsedSave {
            dc,
            ability,
            source: source_name.to_string(),
        })
    }).collect()
}

/// Parse the number of attacks from a Multiattack description.
fn parse_multiattack_count(text: &str) -> u8 {
    let lower = text.to_lowercase();

    // "makes three attacks" / "makes two melee attacks"
    let re = Regex::new(r"makes (\w+)").unwrap();
    if let Some(cap) = re.captures(&lower) {
        if let Some(n) = word_to_number(&cap[1]) {
            return n;
        }
    }

    // Fallback: count "and" conjunctions for "one bite and two claws" pattern
    // "one with its bite and two with its claws" = 3 total
    let count_re = Regex::new(r"(\w+) (?:with its |melee |ranged )?(?:attack|bite|claw|tail|fist|slam)").unwrap();
    let total: u8 = count_re.captures_iter(&lower)
        .filter_map(|c| word_to_number(&c[1]))
        .sum();
    if total > 0 {
        return total;
    }

    2 // sensible default for multiattack
}

fn word_to_number(word: &str) -> Option<u8> {
    match word {
        "one" | "1" => Some(1),
        "two" | "2" => Some(2),
        "three" | "3" => Some(3),
        "four" | "4" => Some(4),
        "five" | "5" => Some(5),
        "six" | "6" => Some(6),
        _ => word.parse().ok(),
    }
}

/// Cache for parsed combat stats, keyed by (source, name) or custom ID.
pub struct CombatStatsCache {
    cache: HashMap<String, CombatStats>,
}

impl CombatStatsCache {
    pub fn new() -> Self {
        Self {
            cache: HashMap::new(),
        }
    }

    /// Get or compute combat stats for a base monster.
    pub fn get_or_parse(&mut self, monster: &Monster) -> &CombatStats {
        let key = format!("{}:{}", monster.source, monster.name);
        self.cache.entry(key).or_insert_with(|| parse_combat_stats(monster))
    }

    /// Get or compute combat stats for a custom monster (keyed by ID).
    pub fn get_or_parse_custom(&mut self, id: &str, monster: &Monster) -> &CombatStats {
        let key = format!("custom:{}", id);
        self.cache.entry(key).or_insert_with(|| parse_combat_stats(monster))
    }

    /// Invalidate a custom monster's cache entry (after edits).
    pub fn invalidate_custom(&mut self, id: &str) {
        self.cache.remove(&format!("custom:{}", id));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::monster::*;

    fn test_monster_with_actions(actions: Vec<Feature>) -> Monster {
        Monster {
            name: "Test".into(),
            source: "TEST".into(),
            page: None,
            size: vec!["M".into()],
            monster_type: MonsterType::Simple("beast".into()),
            alignment: Vec::new(),
            str_score: 10, dex_score: 14, con_score: 10,
            int_score: 10, wis_score: 10, cha_score: 10,
            ac: vec![ArmorClass::Simple(12)],
            hp: HitPoints::Formula { average: 13, formula: "3d8".into() },
            speed: Speed::default(),
            cr: ChallengeRating::Simple("1/4".into()),
            save: Default::default(), skill: Default::default(),
            senses: Vec::new(), passive: None, languages: Vec::new(),
            immune: Vec::new(), resist: Vec::new(), vulnerable: Vec::new(),
            condition_immune: Vec::new(),
            traits: Vec::new(),
            action: actions,
            reaction: Vec::new(), legendary: Vec::new(),
            mythic: Vec::new(), spellcasting: Vec::new(),
            environment: Vec::new(),
        }
    }

    #[test]
    fn test_parse_simple_melee_attack() {
        let action = Feature {
            name: "Scimitar".into(),
            entries: vec![serde_json::Value::String(
                "{@atk mw} {@hit 4} to hit, reach 5 ft., one target. {@h}5 ({@damage 1d6 + 2}) slashing damage.".into()
            )],
        };
        let monster = test_monster_with_actions(vec![action]);
        let stats = parse_combat_stats(&monster);

        assert_eq!(stats.ac, Some(12));
        assert_eq!(stats.max_hp, 13);
        assert_eq!(stats.attacks.len(), 1);

        let atk = &stats.attacks[0];
        assert_eq!(atk.name, "Scimitar");
        assert_eq!(atk.to_hit, 4);
        assert_eq!(atk.reach, Some(5));
        assert_eq!(atk.range, None);
        assert_eq!(atk.damage_dice, "1d6 + 2");
        assert!((atk.damage_avg - 5.0).abs() < 0.01);
        assert_eq!(atk.damage_type, "slashing");
        assert!(atk.extra_damage.is_empty());
    }

    #[test]
    fn test_parse_ranged_attack() {
        let action = Feature {
            name: "Shortbow".into(),
            entries: vec![serde_json::Value::String(
                "{@atk rw} {@hit 4} to hit, range 80/320 ft., one target. {@h}5 ({@damage 1d6 + 2}) piercing damage.".into()
            )],
        };
        let monster = test_monster_with_actions(vec![action]);
        let stats = parse_combat_stats(&monster);

        assert_eq!(stats.attacks.len(), 1);
        let atk = &stats.attacks[0];
        assert_eq!(atk.attack_type, "rw");
        assert_eq!(atk.range, Some((80, 320)));
        assert_eq!(atk.reach, None);
    }

    #[test]
    fn test_parse_attack_with_extra_damage() {
        let action = Feature {
            name: "Bite".into(),
            entries: vec![serde_json::Value::String(
                "{@atk mw} {@hit 14} to hit, reach 10 ft., one target. {@h}19 ({@damage 2d10 + 8}) piercing damage plus 7 ({@damage 2d6}) fire damage.".into()
            )],
        };
        let monster = test_monster_with_actions(vec![action]);
        let stats = parse_combat_stats(&monster);

        let atk = &stats.attacks[0];
        assert_eq!(atk.to_hit, 14);
        assert!((atk.damage_avg - 19.0).abs() < 0.01);
        assert_eq!(atk.damage_type, "piercing");
        assert_eq!(atk.extra_damage.len(), 1);
        assert!((atk.extra_damage[0].damage_avg - 7.0).abs() < 0.01);
        assert_eq!(atk.extra_damage[0].damage_type, "fire");
    }

    #[test]
    fn test_parse_multiattack() {
        let multi = Feature {
            name: "Multiattack".into(),
            entries: vec![serde_json::Value::String(
                "The dragon makes three attacks: one with its bite and two with its claws.".into()
            )],
        };
        let bite = Feature {
            name: "Bite".into(),
            entries: vec![serde_json::Value::String(
                "{@atk mw} {@hit 14} to hit, reach 10 ft., one target. {@h}19 ({@damage 2d10 + 8}) piercing damage.".into()
            )],
        };
        let monster = test_monster_with_actions(vec![multi, bite]);
        let stats = parse_combat_stats(&monster);

        assert_eq!(stats.multiattack_count, 3);
        assert_eq!(stats.attacks.len(), 1); // Multiattack itself isn't an attack
        assert!((stats.estimated_dpr - 57.0).abs() < 0.01); // 19 * 3
    }

    #[test]
    fn test_parse_saving_throw_dc() {
        let action = Feature {
            name: "Fire Breath".into(),
            entries: vec![serde_json::Value::String(
                "Each creature must make a {@dc 21} Dexterity saving throw, taking 63 ({@damage 18d6}) fire damage on a failed save.".into()
            )],
        };
        let monster = test_monster_with_actions(vec![action]);
        let stats = parse_combat_stats(&monster);

        assert_eq!(stats.saving_throws.len(), 1);
        assert_eq!(stats.saving_throws[0].dc, 21);
        assert_eq!(stats.saving_throws[0].ability, "Dexterity");
        assert_eq!(stats.saving_throws[0].source, "Fire Breath");
    }

    #[test]
    fn test_parse_no_attacks() {
        let monster = test_monster_with_actions(Vec::new());
        let stats = parse_combat_stats(&monster);
        assert!(stats.attacks.is_empty());
        assert_eq!(stats.multiattack_count, 1);
        assert!((stats.estimated_dpr - 0.0).abs() < 0.01);
    }

    #[test]
    fn test_cache_reuses_result() {
        let monster = test_monster_with_actions(Vec::new());
        let mut cache = CombatStatsCache::new();
        let _ = cache.get_or_parse(&monster);
        let _ = cache.get_or_parse(&monster);
        // Just verify it doesn't panic and returns consistently
        assert_eq!(cache.get_or_parse(&monster).max_hp, 13);
    }

    #[test]
    fn test_cache_invalidate_custom() {
        let monster = test_monster_with_actions(Vec::new());
        let mut cache = CombatStatsCache::new();
        let _ = cache.get_or_parse_custom("c1", &monster);
        cache.invalidate_custom("c1");
        // After invalidation, next call re-parses
        assert_eq!(cache.get_or_parse_custom("c1", &monster).max_hp, 13);
    }

    #[test]
    fn test_multiattack_word_variants() {
        assert_eq!(parse_multiattack_count("The creature makes two melee attacks."), 2);
        assert_eq!(parse_multiattack_count("It makes three attacks."), 3);
        assert_eq!(parse_multiattack_count("The dragon makes four attacks."), 4);
    }
}
