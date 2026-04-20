use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::data::MonsterDatabase;
use crate::model::combat_stats::{CombatStatsCache, ParsedAttack, parse_combat_stats};
use crate::model::monster::{CustomMonster, MonsterRef};
use crate::model::party::PlayerCharacter;
use crate::model::Encounter;

use super::combat_log::CombatLog;

pub const STANDARD_CONDITIONS: &[&str] = &[
    "Blinded", "Charmed", "Deafened", "Frightened", "Grappled",
    "Incapacitated", "Invisible", "Paralyzed", "Petrified",
    "Poisoned", "Prone", "Restrained", "Stunned", "Unconscious",
    "Concentrating",
];

/// Identifies a specific monster instance within an encounter.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MonsterInstanceId {
    pub encounter_id: String,
    pub monster_index: usize,
    pub instance: usize,
}

/// Identifies any combatant (monster or player character).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum CombatantId {
    Monster(MonsterInstanceId),
    Player(String), // PC id
}

/// Runtime state for a single monster instance during combat.
#[derive(Clone, Debug)]
pub struct MonsterInstance {
    pub label: String,
    pub ac: u8,
    pub max_hp: i32,
    pub current_hp: i32,
    pub temp_hp: i32,
    pub initiative: Option<i32>,
    pub conditions: Vec<bool>,
    pub is_dead: bool,
    pub dex_mod: i8,
    /// Parsed attacks from combat stats, populated during init.
    pub attacks: Vec<ParsedAttack>,
    /// Multiattack description text.
    pub multiattack_text: String,
    /// Non-attack abilities (save-based, utility, etc.).
    pub abilities: Vec<crate::model::combat_stats::ParsedAbility>,
    /// If true, rolls initiative with disadvantage (5.5e surprise).
    pub surprised: bool,
    /// If true, rolls initiative with advantage (hidden from all enemies).
    pub hidden: bool,
}

/// Runtime state for a player character during combat.
#[derive(Clone, Debug)]
pub struct PlayerCombatState {
    pub name: String,
    pub ac: u8,
    pub max_hp: i32,
    pub current_hp: i32,
    pub temp_hp: i32,
    pub initiative: Option<i32>,
    pub initiative_modifier: i8,
    pub conditions: Vec<bool>,
    /// If true, rolls initiative with disadvantage (5.5e surprise).
    pub surprised: bool,
    /// If true, rolls initiative with advantage (hidden from all enemies).
    pub hidden: bool,
}

/// Roll 1d20 with advantage, disadvantage, or normal.
/// If both advantage and disadvantage apply, they cancel out → normal roll.
fn roll_with_adv_disadv(rng: &mut impl rand::Rng, advantage: bool, disadvantage: bool) -> i32 {
    if advantage && !disadvantage {
        let r1 = rng.gen_range(1..=20);
        let r2 = rng.gen_range(1..=20);
        r1.max(r2)
    } else if disadvantage && !advantage {
        let r1 = rng.gen_range(1..=20);
        let r2 = rng.gen_range(1..=20);
        r1.min(r2)
    } else {
        rng.gen_range(1..=20)
    }
}

/// Human-readable label for advantage/disadvantage state.
fn adv_label(hidden: bool, surprised: bool) -> &'static str {
    match (hidden, surprised) {
        (true, true) => "",     // cancel out
        (true, false) => "ADV",
        (false, true) => "DIS",
        (false, false) => "",
    }
}

/// Tracks combat state for all active encounters.
pub struct CombatTracker {
    pub instances: HashMap<MonsterInstanceId, MonsterInstance>,
    pub players: HashMap<String, PlayerCombatState>,
    pub round: u32,
    pub initiative_order: Vec<CombatantId>,
    pub current_turn: usize,
    pub log: CombatLog,
}

impl CombatTracker {
    /// Initialize the tracker from encounters and a party of player characters.
    pub fn init_with_party(
        encounters: &[Encounter],
        monster_db: &MonsterDatabase,
        custom_monsters: &[CustomMonster],
        cache: &mut CombatStatsCache,
        party: &[PlayerCharacter],
    ) -> Self {
        let mut instances = HashMap::new();

        for enc in encounters {
            for (m_idx, em) in enc.monsters.iter().enumerate() {
                let monster = resolve_monster(&em.monster_ref, monster_db, custom_monsters);
                let Some(monster) = monster else { continue };

                let stats = parse_combat_stats(monster);
                let dex_mod = (monster.dex_score as i8 - 10) / 2;

                // Cache it while we're at it
                match &em.monster_ref {
                    MonsterRef::Base { .. } => { cache.get_or_parse(monster); }
                    MonsterRef::Custom { id } | MonsterRef::Merged { id } => {
                        cache.get_or_parse_custom(id, monster);
                    }
                }

                let attacks = stats.attacks.clone();
                let multiattack_text = stats.multiattack_text.clone();
                let abilities = stats.abilities.clone();

                for i in 0..em.count as usize {
                    let id = MonsterInstanceId {
                        encounter_id: enc.id.clone(),
                        monster_index: m_idx,
                        instance: i,
                    };
                    let label = if em.count > 1 {
                        format!("{} #{}", monster.name, i + 1)
                    } else {
                        monster.name.clone()
                    };
                    instances.insert(id, MonsterInstance {
                        label,
                        ac: stats.ac.unwrap_or(10),
                        max_hp: stats.max_hp,
                        current_hp: stats.max_hp,
                        temp_hp: 0,
                        initiative: None,
                        conditions: vec![false; STANDARD_CONDITIONS.len()],
                        is_dead: false,
                        dex_mod,
                        attacks: attacks.clone(),
                        multiattack_text: multiattack_text.clone(),
                        abilities: abilities.clone(),
                        surprised: false,
                        hidden: false,
                    });
                }
            }
        }

        let mut players = HashMap::new();
        for pc in party {
            players.insert(pc.id.clone(), PlayerCombatState {
                name: pc.name.clone(),
                ac: pc.ac,
                max_hp: pc.max_hp,
                current_hp: pc.current_hp,
                temp_hp: 0,
                initiative: None,
                initiative_modifier: pc.initiative_modifier,
                conditions: vec![false; STANDARD_CONDITIONS.len()],
                surprised: false,
                hidden: false,
            });
        }

        Self {
            instances,
            players,
            round: 1,
            initiative_order: Vec::new(),
            current_turn: 0,
            log: CombatLog::new(),
        }
    }

    /// Add an encounter's monsters to an already-running combat.
    pub fn add_encounter(
        &mut self,
        encounter: &Encounter,
        monster_db: &MonsterDatabase,
        custom_monsters: &[CustomMonster],
        cache: &mut CombatStatsCache,
    ) {
        for (m_idx, em) in encounter.monsters.iter().enumerate() {
            let monster = resolve_monster(&em.monster_ref, monster_db, custom_monsters);
            let Some(monster) = monster else { continue };

            let stats = parse_combat_stats(monster);
            let dex_mod = (monster.dex_score as i8 - 10) / 2;

            match &em.monster_ref {
                MonsterRef::Base { .. } => { cache.get_or_parse(monster); }
                MonsterRef::Custom { id } | MonsterRef::Merged { id } => {
                    cache.get_or_parse_custom(id, monster);
                }
            }

            let attacks = stats.attacks.clone();
                let multiattack_text = stats.multiattack_text.clone();
                let abilities = stats.abilities.clone();

            for i in 0..em.count as usize {
                let id = MonsterInstanceId {
                    encounter_id: encounter.id.clone(),
                    monster_index: m_idx,
                    instance: i,
                };
                // Skip if this instance already exists (e.g. encounter already in combat)
                if self.instances.contains_key(&id) { continue; }
                let label = if em.count > 1 {
                    format!("{} #{}", monster.name, i + 1)
                } else {
                    monster.name.clone()
                };
                self.instances.insert(id, MonsterInstance {
                    label,
                    ac: stats.ac.unwrap_or(10),
                    max_hp: stats.max_hp,
                    current_hp: stats.max_hp,
                    temp_hp: 0,
                    initiative: None,
                    conditions: vec![false; STANDARD_CONDITIONS.len()],
                    is_dead: false,
                    dex_mod,
                    attacks: attacks.clone(),
                    multiattack_text: multiattack_text.clone(),
                    abilities: abilities.clone(),
                    surprised: false,
                    hidden: false,
                });
            }
        }
        // Re-sort initiative if order already exists
        if !self.initiative_order.is_empty() {
            self.sort_initiative();
        }
    }

    /// Toggle hidden status on any combatant.
    pub fn toggle_hidden(&mut self, id: &CombatantId) {
        match id {
            CombatantId::Monster(mid) => {
                if let Some(inst) = self.instances.get_mut(mid) {
                    inst.hidden = !inst.hidden;
                }
            }
            CombatantId::Player(pid) => {
                if let Some(pc) = self.players.get_mut(pid) {
                    pc.hidden = !pc.hidden;
                }
            }
        }
    }

    /// Apply damage to a monster instance. Temp HP absorbs first.
    #[cfg(test)]
    pub fn init(
        encounters: &[Encounter],
        monster_db: &MonsterDatabase,
        custom_monsters: &[CustomMonster],
        cache: &mut CombatStatsCache,
    ) -> Self {
        Self::init_with_party(encounters, monster_db, custom_monsters, cache, &[])
    }

    #[cfg(test)]
    pub fn apply_damage(&mut self, id: &MonsterInstanceId, damage: i32) {
        self.apply_damage_to(&CombatantId::Monster(id.clone()), damage);
    }

    #[cfg(test)]
    pub fn heal(&mut self, id: &MonsterInstanceId, amount: i32) {
        self.heal_combatant(&CombatantId::Monster(id.clone()), amount);
    }

    #[cfg(test)]
    pub fn toggle_condition(&mut self, id: &MonsterInstanceId, condition_index: usize) {
        self.toggle_combatant_condition(&CombatantId::Monster(id.clone()), condition_index);
    }

    /// Apply damage to any combatant. Temp HP absorbs first.
    pub fn apply_damage_to(&mut self, id: &CombatantId, damage: i32) {
        match id {
            CombatantId::Monster(mid) => {
                if let Some(inst) = self.instances.get_mut(mid) {
                    let remaining = if inst.temp_hp > 0 {
                        let absorbed = damage.min(inst.temp_hp);
                        inst.temp_hp -= absorbed;
                        damage - absorbed
                    } else {
                        damage
                    };
                    inst.current_hp = (inst.current_hp - remaining).max(0);
                    if inst.current_hp == 0 {
                        inst.is_dead = true;
                    }
                    let name = inst.label.clone();
                    let hp = inst.current_hp;
                    self.log.log_damage(&name, damage, hp);
                }
            }
            CombatantId::Player(pid) => {
                if let Some(pc) = self.players.get_mut(pid) {
                    let remaining = if pc.temp_hp > 0 {
                        let absorbed = damage.min(pc.temp_hp);
                        pc.temp_hp -= absorbed;
                        damage - absorbed
                    } else {
                        damage
                    };
                    pc.current_hp = (pc.current_hp - remaining).max(0);
                    let name = pc.name.clone();
                    let hp = pc.current_hp;
                    self.log.log_damage(&name, damage, hp);
                }
            }
        }
    }

    /// Heal any combatant.
    pub fn heal_combatant(&mut self, id: &CombatantId, amount: i32) {
        match id {
            CombatantId::Monster(mid) => {
                if let Some(inst) = self.instances.get_mut(mid) {
                    inst.current_hp = (inst.current_hp + amount).min(inst.max_hp);
                    if inst.current_hp > 0 {
                        inst.is_dead = false;
                    }
                    let name = inst.label.clone();
                    let hp = inst.current_hp;
                    self.log.log_healing(&name, amount, hp);
                }
            }
            CombatantId::Player(pid) => {
                if let Some(pc) = self.players.get_mut(pid) {
                    pc.current_hp = (pc.current_hp + amount).min(pc.max_hp);
                    let name = pc.name.clone();
                    let hp = pc.current_hp;
                    self.log.log_healing(&name, amount, hp);
                }
            }
        }
    }

    /// Toggle a condition on any combatant.
    pub fn toggle_combatant_condition(&mut self, id: &CombatantId, condition_index: usize) {
        match id {
            CombatantId::Monster(mid) => {
                if let Some(inst) = self.instances.get_mut(mid) {
                    if condition_index < inst.conditions.len() {
                        inst.conditions[condition_index] = !inst.conditions[condition_index];
                    }
                }
            }
            CombatantId::Player(pid) => {
                if let Some(pc) = self.players.get_mut(pid) {
                    if condition_index < pc.conditions.len() {
                        pc.conditions[condition_index] = !pc.conditions[condition_index];
                    }
                }
            }
        }
    }

    /// Roll initiative for all instances and players (1d20 + modifier).
    /// Surprised → disadvantage (roll twice, take lower).
    /// Hidden → advantage (roll twice, take higher).
    /// Both → they cancel out, roll normally.
    pub fn roll_all_initiative(&mut self) {
        let mut rng = rand::thread_rng();
        self.log.log_info("--- Initiative ---".to_string());
        for inst in self.instances.values_mut() {
            if let Some(total) = inst.initiative {
                self.log.log_info(format!("{} initiative: {} (preset)", inst.label, total));
            } else {
                let die = roll_with_adv_disadv(&mut rng, inst.hidden, inst.surprised);
                let total = die + inst.dex_mod as i32;
                inst.initiative = Some(total);
                let adv = adv_label(inst.hidden, inst.surprised);
                self.log.log_initiative(&inst.label, die, inst.dex_mod as i32, total, adv);
            }
        }
        for pc in self.players.values_mut() {
            if let Some(total) = pc.initiative {
                self.log.log_info(format!("{} initiative: {} (preset)", pc.name, total));
            } else {
                let die = roll_with_adv_disadv(&mut rng, pc.hidden, pc.surprised);
                let total = die + pc.initiative_modifier as i32;
                pc.initiative = Some(total);
                let adv = adv_label(pc.hidden, pc.surprised);
                self.log.log_initiative(&pc.name, die, pc.initiative_modifier as i32, total, adv);
            }
        }
        self.sort_initiative();
    }


    /// Sort initiative order by initiative value (descending).
    pub fn sort_initiative(&mut self) {
        let mut order: Vec<CombatantId> = Vec::new();

        // Add all monster instances
        for key in self.instances.keys() {
            order.push(CombatantId::Monster(key.clone()));
        }
        // Add all players
        for key in self.players.keys() {
            order.push(CombatantId::Player(key.clone()));
        }

        order.sort_by(|a, b| {
            let init_a = self.get_initiative(a).unwrap_or(0);
            let init_b = self.get_initiative(b).unwrap_or(0);
            init_b.cmp(&init_a) // descending
        });
        self.initiative_order = order;
        self.current_turn = 0;
    }

    /// Get the initiative value for a combatant.
    fn get_initiative(&self, id: &CombatantId) -> Option<i32> {
        match id {
            CombatantId::Monster(mid) => self.instances.get(mid).and_then(|i| i.initiative),
            CombatantId::Player(pid) => self.players.get(pid).and_then(|p| p.initiative),
        }
    }

    /// Advance to next turn.
    pub fn next_turn(&mut self) {
        if self.initiative_order.is_empty() { return; }
        self.current_turn += 1;
        if self.current_turn >= self.initiative_order.len() {
            self.current_turn = 0;
            self.round += 1;
            self.log.log_round(self.round);
        }
        if let Some(name) = self.current_combatant_id().map(|id| self.get_combatant_name(id).to_string()) {
            self.log.log_turn(&name);
        }
    }

    /// Go back to previous turn.
    pub fn prev_turn(&mut self) {
        if self.initiative_order.is_empty() { return; }
        if self.current_turn == 0 {
            self.current_turn = self.initiative_order.len() - 1;
            self.round = self.round.saturating_sub(1).max(1);
        } else {
            self.current_turn -= 1;
        }
    }

    /// Get the current turn's combatant ID, if any.
    pub fn current_combatant_id(&self) -> Option<&CombatantId> {
        self.initiative_order.get(self.current_turn)
    }

    /// Count alive/dead for a specific encounter (monsters only).
    pub fn counts_for_encounter(&self, encounter_id: &str) -> (usize, usize) {
        let mut alive = 0;
        let mut dead = 0;
        for (id, inst) in &self.instances {
            if id.encounter_id == encounter_id {
                if inst.is_dead { dead += 1; } else { alive += 1; }
            }
        }
        (alive, dead)
    }

    /// Collect all living combatants as potential attack targets.
    /// Returns (CombatantId, name, AC, hidden).
    pub fn attack_targets(&self) -> Vec<(CombatantId, String, u8, bool)> {
        let mut targets = Vec::new();
        for (mid, inst) in &self.instances {
            if !inst.is_dead {
                targets.push((
                    CombatantId::Monster(mid.clone()),
                    inst.label.clone(),
                    inst.ac,
                    inst.hidden,
                ));
            }
        }
        for (pid, pc) in &self.players {
            if pc.current_hp > 0 {
                targets.push((
                    CombatantId::Player(pid.clone()),
                    pc.name.clone(),
                    pc.ac,
                    pc.hidden,
                ));
            }
        }
        targets
    }

    /// Get the display name for a combatant.
    pub fn get_combatant_name(&self, id: &CombatantId) -> &str {
        match id {
            CombatantId::Monster(mid) => {
                self.instances.get(mid).map(|i| i.label.as_str()).unwrap_or("Unknown")
            }
            CombatantId::Player(pid) => {
                self.players.get(pid).map(|p| p.name.as_str()).unwrap_or("Unknown")
            }
        }
    }
}

/// Resolve a MonsterRef to a concrete Monster reference.
pub fn resolve_monster<'a>(
    mref: &MonsterRef,
    db: &'a MonsterDatabase,
    custom: &'a [CustomMonster],
) -> Option<&'a crate::model::monster::Monster> {
    match mref {
        MonsterRef::Base { source, name } => db.find(source, name),
        MonsterRef::Custom { id } | MonsterRef::Merged { id } => {
            custom.iter().find(|c| c.id == *id).map(|c| &c.monster)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::monster::*;
    use crate::model::{Encounter, EncounterType};

    fn make_monster(name: &str, hp: i32, dex: u8) -> Monster {
        Monster {
            name: name.into(), source: "TEST".into(), page: None,
            size: vec!["M".into()],
            monster_type: MonsterType::Simple("beast".into()),
            alignment: Vec::new(),
            str_score: 10, dex_score: dex, con_score: 10,
            int_score: 10, wis_score: 10, cha_score: 10,
            ac: vec![ArmorClass::Simple(12)],
            hp: HitPoints::Formula { average: hp, formula: format!("{}d8", hp / 4) },
            speed: Speed::default(),
            cr: ChallengeRating::Simple("1".into()),
            save: Default::default(), skill: Default::default(),
            senses: Vec::new(), passive: None, languages: Vec::new(),
            immune: Vec::new(), resist: Vec::new(), vulnerable: Vec::new(),
            condition_immune: Vec::new(),
            traits: Vec::new(), action: Vec::new(), reaction: Vec::new(),
            legendary: Vec::new(), mythic: Vec::new(),
            spellcasting: Vec::new(), environment: Vec::new(),
        }
    }

    fn make_custom(id: &str, name: &str, hp: i32) -> CustomMonster {
        CustomMonster {
            id: id.into(),
            based_on: None,
            monster: make_monster(name, hp, 14),
        }
    }

    fn make_tracker_with_goblin(id: &MonsterInstanceId) -> CombatTracker {
        let mut tracker = CombatTracker {
            instances: HashMap::new(),
            players: HashMap::new(),
            round: 1,
            initiative_order: Vec::new(),
            current_turn: 0,
            log: CombatLog::new(),
        };
        tracker.instances.insert(id.clone(), MonsterInstance {
            label: "Goblin".into(), ac: 12, max_hp: 10, current_hp: 10, temp_hp: 0,
            initiative: None, conditions: vec![false; STANDARD_CONDITIONS.len()],
            is_dead: false, dex_mod: 2,
            attacks: Vec::new(),
            multiattack_text: String::new(),
            abilities: Vec::new(),
            surprised: false,
            hidden: false,
        });
        tracker
    }

    #[test]
    fn test_apply_damage() {
        let id = MonsterInstanceId {
            encounter_id: "e1".into(), monster_index: 0, instance: 0,
        };
        let mut tracker = make_tracker_with_goblin(&id);

        tracker.apply_damage(&id, 3);
        assert_eq!(tracker.instances[&id].current_hp, 7);
        assert!(!tracker.instances[&id].is_dead);

        tracker.apply_damage(&id, 20); // overkill
        assert_eq!(tracker.instances[&id].current_hp, 0);
        assert!(tracker.instances[&id].is_dead);
    }

    #[test]
    fn test_temp_hp_absorbs_damage() {
        let id = MonsterInstanceId {
            encounter_id: "e1".into(), monster_index: 0, instance: 0,
        };
        let mut tracker = CombatTracker {
            instances: HashMap::new(),
            players: HashMap::new(),
            round: 1, initiative_order: Vec::new(), current_turn: 0,
            log: CombatLog::new(),
        };
        tracker.instances.insert(id.clone(), MonsterInstance {
            label: "Goblin".into(), ac: 12, max_hp: 10, current_hp: 10, temp_hp: 5,
            initiative: None, conditions: vec![false; STANDARD_CONDITIONS.len()],
            is_dead: false, dex_mod: 2,
            attacks: Vec::new(),
            multiattack_text: String::new(),
            abilities: Vec::new(),
            surprised: false,
            hidden: false,
        });

        tracker.apply_damage(&id, 7);
        assert_eq!(tracker.instances[&id].temp_hp, 0);
        assert_eq!(tracker.instances[&id].current_hp, 8); // 5 absorbed, 2 to HP
    }

    #[test]
    fn test_heal() {
        let id = MonsterInstanceId {
            encounter_id: "e1".into(), monster_index: 0, instance: 0,
        };
        let mut tracker = CombatTracker {
            instances: HashMap::new(),
            players: HashMap::new(),
            round: 1, initiative_order: Vec::new(), current_turn: 0,
            log: CombatLog::new(),
        };
        tracker.instances.insert(id.clone(), MonsterInstance {
            label: "Goblin".into(), ac: 12, max_hp: 10, current_hp: 3, temp_hp: 0,
            initiative: None, conditions: vec![false; STANDARD_CONDITIONS.len()],
            is_dead: false, dex_mod: 2,
            attacks: Vec::new(),
            multiattack_text: String::new(),
            abilities: Vec::new(),
            surprised: false,
            hidden: false,
        });

        tracker.heal(&id, 5);
        assert_eq!(tracker.instances[&id].current_hp, 8);

        tracker.heal(&id, 100); // can't exceed max
        assert_eq!(tracker.instances[&id].current_hp, 10);
    }

    #[test]
    fn test_heal_revives() {
        let id = MonsterInstanceId {
            encounter_id: "e1".into(), monster_index: 0, instance: 0,
        };
        let mut tracker = CombatTracker {
            instances: HashMap::new(),
            players: HashMap::new(),
            round: 1, initiative_order: Vec::new(), current_turn: 0,
            log: CombatLog::new(),
        };
        tracker.instances.insert(id.clone(), MonsterInstance {
            label: "Goblin".into(), ac: 12, max_hp: 10, current_hp: 0, temp_hp: 0,
            initiative: None, conditions: vec![false; STANDARD_CONDITIONS.len()],
            is_dead: true, dex_mod: 2,
            attacks: Vec::new(),
            multiattack_text: String::new(),
            abilities: Vec::new(),
            surprised: false,
            hidden: false,
        });

        tracker.heal(&id, 5);
        assert_eq!(tracker.instances[&id].current_hp, 5);
        assert!(!tracker.instances[&id].is_dead);
    }

    #[test]
    fn test_toggle_condition() {
        let id = MonsterInstanceId {
            encounter_id: "e1".into(), monster_index: 0, instance: 0,
        };
        let mut tracker = make_tracker_with_goblin(&id);

        tracker.toggle_condition(&id, 0); // Blinded on
        assert!(tracker.instances[&id].conditions[0]);
        tracker.toggle_condition(&id, 0); // Blinded off
        assert!(!tracker.instances[&id].conditions[0]);
    }

    #[test]
    fn test_next_turn_wraps_round() {
        let mut tracker = CombatTracker {
            instances: HashMap::new(),
            players: HashMap::new(),
            round: 1,
            initiative_order: vec![
                CombatantId::Monster(MonsterInstanceId { encounter_id: "e1".into(), monster_index: 0, instance: 0 }),
                CombatantId::Monster(MonsterInstanceId { encounter_id: "e1".into(), monster_index: 0, instance: 1 }),
            ],
            current_turn: 0,
            log: CombatLog::new(),
        };

        tracker.next_turn();
        assert_eq!(tracker.current_turn, 1);
        assert_eq!(tracker.round, 1);

        tracker.next_turn(); // wraps
        assert_eq!(tracker.current_turn, 0);
        assert_eq!(tracker.round, 2);
    }

    #[test]
    fn test_prev_turn_wraps() {
        let mut tracker = CombatTracker {
            instances: HashMap::new(),
            players: HashMap::new(),
            round: 2,
            initiative_order: vec![
                CombatantId::Monster(MonsterInstanceId { encounter_id: "e1".into(), monster_index: 0, instance: 0 }),
                CombatantId::Monster(MonsterInstanceId { encounter_id: "e1".into(), monster_index: 0, instance: 1 }),
            ],
            current_turn: 0,
            log: CombatLog::new(),
        };

        tracker.prev_turn();
        assert_eq!(tracker.current_turn, 1);
        assert_eq!(tracker.round, 1);
    }

    #[test]
    fn test_counts_for_encounter() {
        let mut tracker = CombatTracker {
            instances: HashMap::new(),
            players: HashMap::new(),
            round: 1, initiative_order: Vec::new(), current_turn: 0,
            log: CombatLog::new(),
        };
        for i in 0..3 {
            let id = MonsterInstanceId {
                encounter_id: "e1".into(), monster_index: 0, instance: i,
            };
            tracker.instances.insert(id, MonsterInstance {
                label: format!("Goblin #{}", i + 1), ac: 12, max_hp: 10, current_hp: if i == 2 { 0 } else { 10 }, temp_hp: 0,
                initiative: None, conditions: vec![false; STANDARD_CONDITIONS.len()],
                is_dead: i == 2, dex_mod: 2,
                attacks: Vec::new(),
                multiattack_text: String::new(),
                abilities: Vec::new(),
                surprised: false,
                hidden: false,
            });
        }

        let (alive, dead) = tracker.counts_for_encounter("e1");
        assert_eq!(alive, 2);
        assert_eq!(dead, 1);
    }

    #[test]
    fn test_init_from_encounters() {
        let custom = vec![make_custom("c1", "Custom Beast", 20)];
        let enc = Encounter {
            id: "e1".into(),
            name: "Test Fight".into(),
            encounter_type: EncounterType::Static,
            home_room_id: "r1".into(),
            monsters: vec![
                crate::model::monster::EncounterMonster {
                    monster_ref: MonsterRef::Custom { id: "c1".into() },
                    count: 2,
                    notes: String::new(),
                },
            ],
            notes: String::new(),
            hazard: None,
        };

        let db = MonsterDatabase::empty();
        let mut cache = CombatStatsCache::new();
        let tracker = CombatTracker::init(&[enc], &db, &custom, &mut cache);

        assert_eq!(tracker.instances.len(), 2);
        for inst in tracker.instances.values() {
            assert_eq!(inst.max_hp, 20);
            assert_eq!(inst.current_hp, 20);
            assert!(!inst.is_dead);
        }
    }

    #[test]
    fn test_init_with_party() {
        let pc = PlayerCharacter {
            id: "pc1".into(),
            name: "Gandalf".into(),
            class: "Wizard".into(),
            ac: 15,
            max_hp: 40,
            current_hp: 40,
            initiative_modifier: 2,
            passive_perception: 14,
            attack_bonus: 5,
            damage_dice: "1d8 + 3".into(),
            notes: String::new(),
            stealth_modifier: 0,
            senses: Default::default(),
            stealth_override: None,
        };

        let db = MonsterDatabase::empty();
        let mut cache = CombatStatsCache::new();
        let tracker = CombatTracker::init_with_party(&[], &db, &[], &mut cache, &[pc]);

        assert_eq!(tracker.players.len(), 1);
        let state = &tracker.players["pc1"];
        assert_eq!(state.name, "Gandalf");
        assert_eq!(state.ac, 15);
        assert_eq!(state.max_hp, 40);
    }

    #[test]
    fn test_apply_damage_to_player() {
        let mut tracker = CombatTracker {
            instances: HashMap::new(),
            players: HashMap::new(),
            round: 1, initiative_order: Vec::new(), current_turn: 0,
            log: CombatLog::new(),
        };
        tracker.players.insert("pc1".into(), PlayerCombatState {
            name: "Fighter".into(),
            ac: 18,
            max_hp: 50,
            current_hp: 50,
            temp_hp: 0,
            initiative: None,
            initiative_modifier: 1,
            conditions: vec![false; STANDARD_CONDITIONS.len()],
            surprised: false,
            hidden: false,
        });

        tracker.apply_damage_to(&CombatantId::Player("pc1".into()), 15);
        assert_eq!(tracker.players["pc1"].current_hp, 35);
    }

    #[test]
    fn test_heal_combatant_player() {
        let mut tracker = CombatTracker {
            instances: HashMap::new(),
            players: HashMap::new(),
            round: 1, initiative_order: Vec::new(), current_turn: 0,
            log: CombatLog::new(),
        };
        tracker.players.insert("pc1".into(), PlayerCombatState {
            name: "Fighter".into(),
            ac: 18,
            max_hp: 50,
            current_hp: 20,
            temp_hp: 0,
            initiative: None,
            initiative_modifier: 1,
            conditions: vec![false; STANDARD_CONDITIONS.len()],
            surprised: false,
            hidden: false,
        });

        tracker.heal_combatant(&CombatantId::Player("pc1".into()), 10);
        assert_eq!(tracker.players["pc1"].current_hp, 30);

        tracker.heal_combatant(&CombatantId::Player("pc1".into()), 100);
        assert_eq!(tracker.players["pc1"].current_hp, 50); // capped at max
    }

    #[test]
    fn test_get_combatant_name() {
        let id = MonsterInstanceId {
            encounter_id: "e1".into(), monster_index: 0, instance: 0,
        };
        let mut tracker = make_tracker_with_goblin(&id);
        tracker.players.insert("pc1".into(), PlayerCombatState {
            name: "Aragorn".into(),
            ac: 16,
            max_hp: 60,
            current_hp: 60,
            temp_hp: 0,
            initiative: None,
            initiative_modifier: 3,
            conditions: vec![false; STANDARD_CONDITIONS.len()],
            surprised: false,
            hidden: false,
        });

        assert_eq!(tracker.get_combatant_name(&CombatantId::Monster(id)), "Goblin");
        assert_eq!(tracker.get_combatant_name(&CombatantId::Player("pc1".into())), "Aragorn");
    }
}
