use rand::Rng;

use crate::data::MonsterDatabase;
use crate::model::{Dungeon, DungeonGraph, Encounter, LightSource, SpatialLayout};
use crate::presentation::combat_tracker::resolve_monster;
use crate::presentation::lighting::compute_brightness_generic;

/// Light level for a room, derived from the lighting system.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LightLevel {
    Bright,
    Dim,
    Dark,
}

impl LightLevel {
    /// Passive perception penalty for this light level (sight-based).
    /// Dim/Dark = disadvantage on Perception, which is -5 for passive checks.
    pub fn passive_perception_penalty(self) -> i32 {
        match self {
            LightLevel::Bright => 0,
            LightLevel::Dim => -5,
            LightLevel::Dark => -5,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            LightLevel::Bright => "Bright",
            LightLevel::Dim => "Dim",
            LightLevel::Dark => "Dark",
        }
    }
}

/// Per-creature stealth roll and detection result.
#[derive(Clone, Debug)]
pub struct CreatureAwareness {
    pub name: String,
    /// The stealth roll this creature made (1d20 + modifier).
    pub stealth_roll: i32,
    /// This creature's passive perception (adjusted for light).
    pub passive_perception: i32,
    /// True if this creature is surprised (unaware of ALL enemies).
    /// Surprised → disadvantage on initiative.
    pub surprised: bool,
    /// True if this creature's stealth beat every enemy's PP (nobody detected them).
    /// Hidden → advantage on initiative.
    pub hidden: bool,
}

/// Result of an awareness check between the party and an encounter.
#[derive(Clone, Debug)]
pub struct AwarenessResult {
    pub encounter_id: String,
    pub encounter_name: String,
    /// Distance in room hops.
    pub distance_rooms: u32,
    /// Estimated distance in feet (center-to-center via spatial layout).
    pub distance_feet: Option<f32>,
    /// Light level at the encounter's room.
    pub encounter_light: LightLevel,
    /// Light level at the party's room.
    pub party_light: LightLevel,
    /// Per-monster awareness info (stealth roll, PP, surprised state).
    pub monsters: Vec<CreatureAwareness>,
    /// Per-PC awareness info (stealth roll, PP, surprised state).
    pub party: Vec<CreatureAwareness>,
}

impl AwarenessResult {
    /// True if any party member is surprised.
    #[allow(dead_code)]
    pub fn any_party_surprised(&self) -> bool {
        self.party.iter().any(|c| c.surprised)
    }

    /// True if any monster is surprised.
    #[allow(dead_code)]
    pub fn any_monsters_surprised(&self) -> bool {
        self.monsters.iter().any(|c| c.surprised)
    }
}

/// Get a monster's Stealth bonus from skill map, falling back to DEX modifier.
fn get_monster_stealth_bonus(monster: &crate::model::monster::Monster) -> i32 {
    if let Some(val) = monster.skill.get("stealth") {
        if let Ok(n) = val.trim_start_matches('+').parse::<i32>() {
            return n;
        }
    }
    (monster.dex_score as i32 - 10) / 2
}

/// Get a monster's passive perception, from the `passive` field or WIS modifier.
fn get_monster_passive_perception(monster: &crate::model::monster::Monster) -> i32 {
    if let Some(pp) = monster.passive {
        return pp as i32;
    }
    let wis_mod = (monster.wis_score as i32 - 10) / 2;
    (10 + wis_mod).max(1)
}

/// Check if a monster has a non-sight sense (blindsight, tremorsense) that
/// makes it immune to light-based perception penalties.
fn has_non_sight_sense(monster: &crate::model::monster::Monster) -> bool {
    monster.senses.iter().any(|s| {
        let lower = s.to_lowercase();
        lower.starts_with("blindsight") || lower.starts_with("tremorsense")
    })
}

/// Check if a monster has darkvision (darkness counts as dim light for it).
fn has_darkvision(monster: &crate::model::monster::Monster) -> bool {
    monster.senses.iter().any(|s| s.to_lowercase().starts_with("darkvision"))
}

/// Compute the light penalty for a monster's passive perception.
/// - Blindsight / tremorsense: no penalty (light irrelevant).
/// - Darkvision: darkness treated as dim light, so no penalty in Dim or Dark.
///   (Darkvision makes darkness→dim, and dim doesn't penalize passive perception
///    for the purpose of this check since darkvision already accounts for it.)
/// - No special senses: -5 in Dim or Dark (disadvantage on sight-based Perception).
fn monster_light_penalty(monster: &crate::model::monster::Monster, light: LightLevel) -> i32 {
    if has_non_sight_sense(monster) {
        return 0;
    }
    if has_darkvision(monster) {
        // Darkvision: darkness → dim (no mechanical penalty), dim → bright (no penalty)
        return 0;
    }
    light.passive_perception_penalty()
}

/// Compute the light penalty for a PC's passive perception based on their senses.
fn pc_light_penalty(senses: crate::model::party::PcSenses, light: LightLevel) -> i32 {
    if senses.has_non_sight_sense() {
        return 0;
    }
    if senses.darkvision {
        return 0;
    }
    light.passive_perception_penalty()
}

/// The minimum stealth roll to successfully hide (5.5e DC 15).
/// Rolls below this mean the creature failed to hide entirely.
const STEALTH_HIDE_DC: i32 = 15;

/// Roll individual stealth: 1d20 + modifier.
fn roll_stealth(bonus: i32) -> i32 {
    let mut rng = rand::thread_rng();
    rng.gen_range(1..=20) + bonus
}

/// Compute distance in room hops between two rooms.
pub fn encounter_distance_rooms(
    party_room: &str,
    encounter_room: &str,
    graph: &DungeonGraph,
) -> Option<u32> {
    let distances = super::bfs_distances(party_room, graph);
    distances.get(encounter_room).copied()
}

/// Compute Euclidean distance in feet between two room centers using the spatial layout.
/// Each grid square = 5 feet.
pub fn encounter_distance_feet(
    party_room: &str,
    encounter_room: &str,
    layout: &SpatialLayout,
) -> Option<f32> {
    let rl_a = layout.room_by_id(party_room)?;
    let rl_b = layout.room_by_id(encounter_room)?;

    let ax = rl_a.x as f32 + rl_a.width as f32 / 2.0;
    let ay = rl_a.y as f32 + rl_a.height as f32 / 2.0;
    let bx = rl_b.x as f32 + rl_b.width as f32 / 2.0;
    let by = rl_b.y as f32 + rl_b.height as f32 / 2.0;

    let dx = bx - ax;
    let dy = by - ay;
    let grid_dist = (dx * dx + dy * dy).sqrt();

    // 1 grid square = 5 feet
    Some(grid_dist * 5.0)
}

/// Compute the light level for a room based on the lighting system.
pub fn room_light_level(
    room_id: &str,
    light_sources: &[LightSource],
    ambient_light: f32,
    layout: &SpatialLayout,
) -> LightLevel {
    let Some(rl) = layout.room_by_id(room_id) else {
        return LightLevel::Dark;
    };

    let cx = rl.x as f32 + rl.width as f32 / 2.0;
    let cy = rl.y as f32 + rl.height as f32 / 2.0;

    let brightness = compute_brightness_generic(cx, cy, light_sources, ambient_light, layout);

    if brightness >= 0.5 {
        LightLevel::Bright
    } else if brightness >= 0.1 {
        LightLevel::Dim
    } else {
        LightLevel::Dark
    }
}

/// Run an awareness check between the party and a specific encounter.
///
/// Each creature rolls Stealth individually. A creature is **surprised** if
/// its passive perception fails to beat every opposing creature's stealth roll
/// (i.e. it begins combat unaware of any enemies). Per 5.5e, surprised
/// creatures roll initiative with disadvantage.
pub fn run_awareness_check(
    dungeon: &Dungeon,
    encounter: &Encounter,
    encounter_room: &str,
    party_room: &str,
    monster_db: &MonsterDatabase,
) -> AwarenessResult {
    let graph = &dungeon.graph;

    // Distance
    let distance_rooms = encounter_distance_rooms(party_room, encounter_room, graph)
        .unwrap_or(u32::MAX);
    let distance_feet = dungeon.layout.as_ref()
        .and_then(|layout| encounter_distance_feet(party_room, encounter_room, layout));

    // Light levels
    let encounter_light = dungeon.layout.as_ref()
        .map(|layout| room_light_level(encounter_room, &dungeon.light_sources, dungeon.ambient_light, layout))
        .unwrap_or(LightLevel::Dark);
    let party_light = dungeon.layout.as_ref()
        .map(|layout| room_light_level(party_room, &dungeon.light_sources, dungeon.ambient_light, layout))
        .unwrap_or(LightLevel::Dark);

    // Roll stealth and compute PP for each monster
    let mut monsters: Vec<CreatureAwareness> = Vec::new();
    for em in &encounter.monsters {
        if let Some(monster) = resolve_monster(&em.monster_ref, monster_db, &dungeon.custom_monsters) {
            let stealth_bonus = get_monster_stealth_bonus(monster);
            let raw_pp = get_monster_passive_perception(monster);
            // Monster PP adjusted for light, accounting for darkvision/blindsight/tremorsense
            let pp = (raw_pp + monster_light_penalty(monster, party_light)).max(1);

            for i in 0..em.count {
                let name = if em.count > 1 {
                    format!("{} #{}", monster.name, i + 1)
                } else {
                    monster.name.clone()
                };
                let stealth_roll = roll_stealth(stealth_bonus);
                monsters.push(CreatureAwareness {
                    name,
                    stealth_roll,
                    passive_perception: pp,
                    surprised: false, // computed below
                    hidden: false, // computed below
                });
            }
        }
    }

    // Roll stealth and compute PP for each PC
    let mut party: Vec<CreatureAwareness> = Vec::new();
    for pc in &dungeon.party {
        // Use manual override if set, otherwise roll
        let stealth_roll = pc.stealth_override
            .unwrap_or_else(|| roll_stealth(pc.stealth_modifier as i32));
        // PC PP adjusted for light, accounting for senses
        let light_penalty = pc_light_penalty(pc.senses, encounter_light);
        let pp = (pc.passive_perception as i32 + light_penalty).max(1);
        party.push(CreatureAwareness {
            name: pc.name.clone(),
            stealth_roll,
            passive_perception: pp,
            surprised: false, // computed below
            hidden: false, // computed below
        });
    }

    // Determine surprise and hidden per creature.
    //
    // Stealth DC 15: a roll below 15 means the creature failed to hide
    // entirely — it is automatically detected by everyone.
    //
    // Surprised: creature does NOT detect ANY enemy (every enemy that
    //   successfully hid has stealth >= creature's PP).
    //   → disadvantage on initiative.
    //
    // Hidden: creature successfully hid (stealth >= 15) AND no enemy's PP
    //   meets or exceeds its stealth roll.
    //   → advantage on initiative.

    // Helper: effective stealth for detection purposes.
    // Below DC 15 = failed to hide, treat as automatically visible.
    let effective_stealth = |roll: i32| -> Option<i32> {
        if roll >= STEALTH_HIDE_DC { Some(roll) } else { None }
    };

    // For each PC
    for pc in &mut party {
        // Can this PC detect any monster? (only monsters that successfully hid matter)
        let detects_any = monsters.iter().any(|m| {
            match effective_stealth(m.stealth_roll) {
                Some(stealth) => pc.passive_perception >= stealth,
                None => true, // monster failed to hide, auto-detected
            }
        });
        pc.surprised = !detects_any;

        // Is this PC hidden from all monsters?
        pc.hidden = effective_stealth(pc.stealth_roll).is_some()
            && !monsters.iter().any(|m| m.passive_perception >= pc.stealth_roll);
    }

    // For each monster
    for m in &mut monsters {
        let detects_any = party.iter().any(|pc| {
            match effective_stealth(pc.stealth_roll) {
                Some(stealth) => m.passive_perception >= stealth,
                None => true, // PC failed to hide, auto-detected
            }
        });
        m.surprised = !detects_any;

        m.hidden = effective_stealth(m.stealth_roll).is_some()
            && !party.iter().any(|pc| pc.passive_perception >= m.stealth_roll);
    }

    AwarenessResult {
        encounter_id: encounter.id.clone(),
        encounter_name: encounter.name.clone(),
        distance_rooms,
        distance_feet,
        encounter_light,
        party_light,
        monsters,
        party,
    }
}

impl AwarenessResult {
    /// Look up a PC's awareness state by name.
    pub fn pc_awareness(&self, name: &str) -> Option<&CreatureAwareness> {
        self.party.iter().find(|c| c.name == name)
    }

    /// Look up a monster's awareness state by label (e.g. "Goblin #1").
    pub fn monster_awareness(&self, label: &str) -> Option<&CreatureAwareness> {
        self.monsters.iter().find(|c| c.name == label)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::*;

    fn make_monster_with_stealth(name: &str, dex: u8, wis: u8, stealth: Option<&str>, passive: Option<u8>) -> Monster {
        let mut skill = std::collections::HashMap::new();
        if let Some(s) = stealth {
            skill.insert("stealth".to_string(), s.to_string());
        }
        Monster {
            name: name.into(), source: "TEST".into(), page: None,
            size: vec!["M".into()],
            monster_type: MonsterType::Simple("beast".into()),
            alignment: Vec::new(),
            str_score: 10, dex_score: dex, con_score: 10,
            int_score: 10, wis_score: wis, cha_score: 10,
            ac: vec![ArmorClass::Simple(12)],
            hp: HitPoints::Formula { average: 10, formula: "2d8".into() },
            speed: Speed::default(),
            cr: ChallengeRating::Simple("1".into()),
            save: Default::default(), skill,
            senses: Vec::new(), passive, languages: Vec::new(),
            immune: Vec::new(), resist: Vec::new(), vulnerable: Vec::new(),
            condition_immune: Vec::new(),
            traits: Vec::new(), action: Vec::new(), reaction: Vec::new(),
            legendary: Vec::new(), mythic: Vec::new(),
            spellcasting: Vec::new(), environment: Vec::new(),
        }
    }

    #[test]
    fn test_monster_stealth_bonus_from_skill() {
        let m = make_monster_with_stealth("Goblin", 14, 8, Some("+6"), None);
        assert_eq!(get_monster_stealth_bonus(&m), 6);
    }

    #[test]
    fn test_monster_stealth_bonus_fallback_dex() {
        let m = make_monster_with_stealth("Zombie", 6, 8, None, None);
        assert_eq!(get_monster_stealth_bonus(&m), -2);
    }

    #[test]
    fn test_monster_passive_perception_from_field() {
        let m = make_monster_with_stealth("Goblin", 14, 8, None, Some(9));
        assert_eq!(get_monster_passive_perception(&m), 9);
    }

    #[test]
    fn test_monster_passive_perception_fallback_wis() {
        let m = make_monster_with_stealth("Zombie", 6, 8, None, None);
        // WIS 8 -> mod -1 -> 10 + (-1) = 9
        assert_eq!(get_monster_passive_perception(&m), 9);
    }

    #[test]
    fn test_encounter_distance_feet() {
        let layout = SpatialLayout {
            rooms: vec![
                RoomLayout { room_id: "r1".into(), x: 0, y: 0, width: 4, height: 4, violations: vec![] },
                RoomLayout { room_id: "r2".into(), x: 10, y: 0, width: 4, height: 4, violations: vec![] },
            ],
            corridors: Vec::new(),
            bounds: Vec::new(),
        };
        // Centers: r1 = (2, 2), r2 = (12, 2). Distance = 10 grid squares = 50 feet.
        let feet = encounter_distance_feet("r1", "r2", &layout).unwrap();
        assert!((feet - 50.0).abs() < 0.1);
    }

    #[test]
    fn test_light_level_thresholds() {
        let layout = SpatialLayout {
            rooms: vec![
                RoomLayout { room_id: "r1".into(), x: 0, y: 0, width: 4, height: 4, violations: vec![] },
            ],
            corridors: Vec::new(),
            bounds: Vec::new(),
        };

        assert_eq!(room_light_level("r1", &[], 0.0, &layout), LightLevel::Dark);
        assert_eq!(room_light_level("r1", &[], 0.3, &layout), LightLevel::Dim);
        assert_eq!(room_light_level("r1", &[], 0.8, &layout), LightLevel::Bright);
    }

    /// Helper to apply the surprise/hidden logic matching the production code.
    fn apply_awareness(party: &mut [CreatureAwareness], monsters: &mut [CreatureAwareness]) {
        let effective = |roll: i32| -> Option<i32> {
            if roll >= STEALTH_HIDE_DC { Some(roll) } else { None }
        };
        for pc in party.iter_mut() {
            let detects_any = monsters.iter().any(|m| {
                match effective(m.stealth_roll) {
                    Some(stealth) => pc.passive_perception >= stealth,
                    None => true,
                }
            });
            pc.surprised = !detects_any;
            pc.hidden = effective(pc.stealth_roll).is_some()
                && !monsters.iter().any(|m| m.passive_perception >= pc.stealth_roll);
        }
        for m in monsters.iter_mut() {
            let detects_any = party.iter().any(|pc| {
                match effective(pc.stealth_roll) {
                    Some(stealth) => m.passive_perception >= stealth,
                    None => true,
                }
            });
            m.surprised = !detects_any;
            m.hidden = effective(m.stealth_roll).is_some()
                && !party.iter().any(|pc| pc.passive_perception >= m.stealth_roll);
        }
    }

    #[test]
    fn test_surprise_and_hidden_per_individual() {
        // Rogue: stealth 20 (>= 15, hid), PP 12
        // Fighter: stealth 5 (< 15, failed to hide), PP 18
        // Goblin: stealth 15 (>= 15, hid), PP 10
        //
        // PC surprise:
        //   Rogue PP 12 vs Goblin stealth 15 -> 12 < 15, doesn't detect
        //     -> Rogue surprised (no enemies detected)
        //   Fighter PP 18 vs Goblin stealth 15 -> 18 >= 15, detects
        //     -> Fighter NOT surprised
        //
        // Monster surprise:
        //   Goblin PP 10 vs Rogue stealth 20 -> 10 < 20, doesn't detect
        //   Goblin PP 10 vs Fighter stealth 5 -> failed to hide, auto-detected
        //     -> Goblin NOT surprised (detected the Fighter)
        //
        // Hidden:
        //   Rogue stealth 20 >= 15, Goblin PP 10 < 20 -> HIDDEN
        //   Fighter stealth 5 < 15 -> failed to hide, NOT HIDDEN
        //   Goblin stealth 15 >= 15, but Fighter PP 18 >= 15 -> NOT HIDDEN

        let mut party = vec![
            CreatureAwareness { name: "Rogue".into(), stealth_roll: 20, passive_perception: 12, surprised: false, hidden: false },
            CreatureAwareness { name: "Fighter".into(), stealth_roll: 5, passive_perception: 18, surprised: false, hidden: false },
        ];
        let mut monsters = vec![
            CreatureAwareness { name: "Goblin".into(), stealth_roll: 15, passive_perception: 10, surprised: false, hidden: false },
        ];

        apply_awareness(&mut party, &mut monsters);

        assert!(party[0].surprised, "Rogue surprised (PP 12 < Goblin stealth 15)");
        assert!(!party[1].surprised, "Fighter NOT surprised (PP 18 >= Goblin stealth 15)");
        assert!(!monsters[0].surprised, "Goblin NOT surprised (Fighter failed to hide, auto-detected)");

        assert!(party[0].hidden, "Rogue hidden (stealth 20 >= 15, beats Goblin PP 10)");
        assert!(!party[1].hidden, "Fighter NOT hidden (stealth 5 < DC 15)");
        assert!(!monsters[0].hidden, "Goblin NOT hidden (Fighter PP 18 >= stealth 15)");
    }

    #[test]
    fn test_stealth_dc_15_floor() {
        // Both sides roll under 15 — nobody hid, everyone auto-detects everyone.
        let mut party = vec![
            CreatureAwareness { name: "Wizard".into(), stealth_roll: 8, passive_perception: 9, surprised: false, hidden: false },
        ];
        let mut monsters = vec![
            CreatureAwareness { name: "Zombie".into(), stealth_roll: 3, passive_perception: 8, surprised: false, hidden: false },
        ];

        apply_awareness(&mut party, &mut monsters);

        assert!(!party[0].surprised, "Wizard NOT surprised (Zombie failed to hide)");
        assert!(!monsters[0].surprised, "Zombie NOT surprised (Wizard failed to hide)");
        assert!(!party[0].hidden, "Wizard NOT hidden (stealth 8 < DC 15)");
        assert!(!monsters[0].hidden, "Zombie NOT hidden (stealth 3 < DC 15)");
    }

    #[test]
    fn test_high_stealth_enemies_cause_surprise() {
        // Both enemies successfully hid (>= 15) and beat Wizard's PP.
        let mut party = vec![
            CreatureAwareness { name: "Wizard".into(), stealth_roll: 8, passive_perception: 9, surprised: false, hidden: false },
        ];
        let mut monsters = vec![
            CreatureAwareness { name: "Shadow".into(), stealth_roll: 22, passive_perception: 14, surprised: false, hidden: false },
            CreatureAwareness { name: "Wraith".into(), stealth_roll: 18, passive_perception: 12, surprised: false, hidden: false },
        ];

        apply_awareness(&mut party, &mut monsters);

        assert!(party[0].surprised, "Wizard surprised (PP 9 < both stealth rolls >= 15)");
        assert!(!party[0].hidden, "Wizard NOT hidden (stealth 8 < DC 15)");
        assert!(!monsters[0].surprised, "Shadow NOT surprised (Wizard failed to hide)");
        assert!(monsters[0].hidden, "Shadow hidden (stealth 22, Wizard PP 9 < 22)");
        assert!(!monsters[1].surprised, "Wraith NOT surprised (Wizard failed to hide)");
        assert!(monsters[1].hidden, "Wraith hidden (stealth 18, Wizard PP 9 < 18)");
    }
}
