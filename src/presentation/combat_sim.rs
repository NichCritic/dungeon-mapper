use rand::Rng;

use crate::data::MonsterDatabase;
use crate::model::combat_stats::{CombatStatsCache, ParsedAttack};
use crate::model::monster::CustomMonster;
use crate::model::party::PlayerCharacter;
use crate::model::Encounter;
use crate::presentation::combat_tracker::resolve_monster;

/// A combatant in the simulation.
#[derive(Clone, Debug)]
pub struct SimCombatant {
    pub name: String,
    pub max_hp: i32,
    pub current_hp: i32,
    pub ac: u8,
    pub initiative_mod: i8,
    pub attacks: Vec<ParsedAttack>,
    pub multiattack_count: u8,
    /// 0 = side A, 1 = side B
    pub side: usize,
}

/// Final state of a combatant after simulation.
#[derive(Clone, Debug)]
pub struct SimCombatantResult {
    pub name: String,
    pub max_hp: i32,
    pub current_hp: i32,
    pub side: usize,
}

/// Result of a single combat simulation.
#[derive(Clone, Debug)]
pub struct SimResult {
    /// Which side won (None = draw / timeout).
    pub winner: Option<usize>,
    /// How many rounds the combat lasted.
    pub rounds: u32,
    /// Final state of all combatants (alive and dead).
    pub combatants: Vec<SimCombatantResult>,
}

/// Aggregated result of a Monte Carlo simulation.
#[derive(Clone, Debug)]
pub struct MonteCarloResult {
    pub num_sims: u32,
    pub side_a_wins: u32,
    pub side_b_wins: u32,
    pub draws: u32,
    pub avg_rounds: f32,
    pub side_a_label: String,
    pub side_b_label: String,
}

impl SimResult {
    #[cfg(test)]
    pub fn survivors(&self) -> Vec<&SimCombatantResult> {
        self.combatants.iter().filter(|c| c.current_hp > 0).collect()
    }
}

/// Run a single combat between two sides.
///
/// Each round, combatants act in initiative order. Each combatant picks a
/// random living enemy and attacks with their best attack, repeated
/// multiattack_count times. Combat ends when one side is eliminated or
/// after 100 rounds (draw).
pub fn run_combat(side_a: &[SimCombatant], side_b: &[SimCombatant]) -> SimResult {
    let mut rng = rand::thread_rng();

    // Clone combatants into a single pool
    let mut combatants: Vec<SimCombatant> = Vec::new();
    combatants.extend(side_a.iter().cloned());
    combatants.extend(side_b.iter().cloned());

    // Roll initiative
    let mut initiatives: Vec<(usize, i32)> = combatants.iter().enumerate().map(|(i, c)| {
        let roll = rng.gen_range(1..=20) + c.initiative_mod as i32;
        (i, roll)
    }).collect();
    initiatives.sort_by(|a, b| b.1.cmp(&a.1));
    let order: Vec<usize> = initiatives.iter().map(|(i, _)| *i).collect();

    let max_rounds = 100u32;
    let mut round = 0u32;

    loop {
        round += 1;
        if round > max_rounds {
            break;
        }

        for &idx in &order {
            if combatants[idx].current_hp <= 0 {
                continue;
            }

            let attacker_side = combatants[idx].side;
            let attacks = combatants[idx].attacks.clone();
            let multi = combatants[idx].multiattack_count;

            if attacks.is_empty() {
                continue;
            }

            // Pick best attack (highest avg damage)
            let best_attack = attacks.iter()
                .max_by(|a, b| a.damage_avg.partial_cmp(&b.damage_avg).unwrap_or(std::cmp::Ordering::Equal))
                .unwrap()
                .clone();

            for _ in 0..multi {
                // Pick random living enemy
                let enemies: Vec<usize> = combatants.iter().enumerate()
                    .filter(|(_, c)| c.side != attacker_side && c.current_hp > 0)
                    .map(|(i, _)| i)
                    .collect();

                if enemies.is_empty() {
                    break;
                }

                let target_idx = enemies[rng.gen_range(0..enemies.len())];
                let target_ac = combatants[target_idx].ac;

                // Roll attack
                let result = crate::presentation::dice::roll_attack(&best_attack, target_ac);
                if result.hit {
                    let mut total_damage = result.damage.as_ref().map(|d| d.total).unwrap_or(0);
                    for (extra, _) in &result.extra_damage {
                        total_damage += extra.total;
                    }
                    combatants[target_idx].current_hp -= total_damage;
                }
            }
        }

        // Check if one side is eliminated
        let side_a_alive = combatants.iter().any(|c| c.side == 0 && c.current_hp > 0);
        let side_b_alive = combatants.iter().any(|c| c.side == 1 && c.current_hp > 0);

        if !side_a_alive || !side_b_alive {
            break;
        }
    }

    let side_a_alive = combatants.iter().any(|c| c.side == 0 && c.current_hp > 0);
    let side_b_alive = combatants.iter().any(|c| c.side == 1 && c.current_hp > 0);

    let winner = if side_a_alive && !side_b_alive {
        Some(0)
    } else if side_b_alive && !side_a_alive {
        Some(1)
    } else {
        None
    };

    let final_states: Vec<SimCombatantResult> = combatants.iter()
        .map(|c| SimCombatantResult {
            name: c.name.clone(),
            max_hp: c.max_hp,
            current_hp: c.current_hp,
            side: c.side,
        })
        .collect();

    SimResult { winner, rounds: round.min(max_rounds), combatants: final_states }
}

/// Run a free-for-all combat where each group is a separate side.
/// Groups are provided as slices of combatants, each already assigned their side.
pub fn run_combat_ffa(groups: &[Vec<SimCombatant>]) -> SimResult {
    let mut rng = rand::thread_rng();

    let mut combatants: Vec<SimCombatant> = Vec::new();
    for group in groups {
        combatants.extend(group.iter().cloned());
    }

    // Roll initiative
    let mut initiatives: Vec<(usize, i32)> = combatants.iter().enumerate().map(|(i, c)| {
        let roll = rng.gen_range(1..=20) + c.initiative_mod as i32;
        (i, roll)
    }).collect();
    initiatives.sort_by(|a, b| b.1.cmp(&a.1));
    let order: Vec<usize> = initiatives.iter().map(|(i, _)| *i).collect();

    let max_rounds = 100u32;
    let mut round = 0u32;

    loop {
        round += 1;
        if round > max_rounds { break; }

        for &idx in &order {
            if combatants[idx].current_hp <= 0 { continue; }

            let attacker_side = combatants[idx].side;
            let attacks = combatants[idx].attacks.clone();
            let multi = combatants[idx].multiattack_count;
            if attacks.is_empty() { continue; }

            let best_attack = attacks.iter()
                .max_by(|a, b| a.damage_avg.partial_cmp(&b.damage_avg).unwrap_or(std::cmp::Ordering::Equal))
                .unwrap()
                .clone();

            for _ in 0..multi {
                let enemies: Vec<usize> = combatants.iter().enumerate()
                    .filter(|(_, c)| c.side != attacker_side && c.current_hp > 0)
                    .map(|(i, _)| i)
                    .collect();
                if enemies.is_empty() { break; }

                let target_idx = enemies[rng.gen_range(0..enemies.len())];
                let target_ac = combatants[target_idx].ac;
                let result = crate::presentation::dice::roll_attack(&best_attack, target_ac);
                if result.hit {
                    let mut total_damage = result.damage.as_ref().map(|d| d.total).unwrap_or(0);
                    for (extra, _) in &result.extra_damage {
                        total_damage += extra.total;
                    }
                    combatants[target_idx].current_hp -= total_damage;
                }
            }
        }

        // Check how many sides still have living combatants
        let alive_sides: std::collections::HashSet<usize> = combatants.iter()
            .filter(|c| c.current_hp > 0)
            .map(|c| c.side)
            .collect();
        if alive_sides.len() <= 1 { break; }
    }

    let alive_sides: std::collections::HashSet<usize> = combatants.iter()
        .filter(|c| c.current_hp > 0)
        .map(|c| c.side)
        .collect();
    let winner = if alive_sides.len() == 1 {
        Some(*alive_sides.iter().next().unwrap())
    } else {
        None
    };

    let final_states: Vec<SimCombatantResult> = combatants.iter()
        .map(|c| SimCombatantResult {
            name: c.name.clone(),
            max_hp: c.max_hp,
            current_hp: c.current_hp,
            side: c.side,
        })
        .collect();

    SimResult { winner, rounds: round.min(max_rounds), combatants: final_states }
}

/// Build SimCombatants from an encounter definition.
pub fn build_combatants_from_encounter(
    encounter: &Encounter,
    db: &MonsterDatabase,
    custom: &[CustomMonster],
    cache: &mut CombatStatsCache,
    side: usize,
) -> Vec<SimCombatant> {
    let mut combatants = Vec::new();

    for em in &encounter.monsters {
        let Some(monster) = resolve_monster(&em.monster_ref, db, custom) else {
            continue;
        };

        let stats = cache.get_or_parse(monster).clone();

        for i in 0..em.count {
            let label = if em.count > 1 {
                format!("{} #{}", monster.name, i + 1)
            } else {
                monster.name.clone()
            };

            combatants.push(SimCombatant {
                name: label,
                max_hp: stats.max_hp,
                current_hp: stats.max_hp,
                ac: stats.ac.unwrap_or(10),
                initiative_mod: 0, // Could extract DEX mod but keep simple
                attacks: stats.attacks.clone(),
                multiattack_count: stats.multiattack_count,
                side,
            });
        }
    }

    combatants
}

/// Build SimCombatants from the party.
pub fn build_combatants_from_party(party: &[PlayerCharacter], side: usize) -> Vec<SimCombatant> {
    party.iter().map(|pc| {
        // Synthesize a ParsedAttack from the PC's attack_bonus and damage_dice
        let attack = ParsedAttack {
            name: format!("{}'s Attack", pc.name),
            attack_type: "mw".to_string(),
            to_hit: pc.attack_bonus,
            reach: Some(5),
            range: None,
            damage_dice: pc.damage_dice.clone(),
            damage_avg: estimate_dice_avg(&pc.damage_dice),
            damage_type: "weapon".to_string(),
            extra_damage: Vec::new(),
            effect: String::new(),
        };

        SimCombatant {
            name: pc.name.clone(),
            max_hp: pc.max_hp,
            current_hp: pc.max_hp,
            ac: pc.ac,
            initiative_mod: pc.initiative_modifier,
            attacks: vec![attack],
            multiattack_count: 1,
            side,
        }
    }).collect()
}

/// Rough estimate of average damage for a dice expression like "1d8 + 3".
pub fn estimate_dice_avg_pub(expr: &str) -> f32 {
    estimate_dice_avg(expr)
}

fn estimate_dice_avg(expr: &str) -> f32 {
    let expr = expr.trim();
    let re = regex::Regex::new(r"^(\d+)d(\d+)\s*([+-]\s*\d+)?$").unwrap();
    if let Some(caps) = re.captures(expr) {
        let count: f32 = caps[1].parse().unwrap_or(1.0);
        let sides: f32 = caps[2].parse().unwrap_or(6.0);
        let modifier: f32 = caps.get(3)
            .map(|m| m.as_str().replace(' ', "").parse::<f32>().unwrap_or(0.0))
            .unwrap_or(0.0);
        count * (sides + 1.0) / 2.0 + modifier
    } else {
        expr.parse().unwrap_or(0.0)
    }
}

/// Run a Monte Carlo simulation of N combats.
pub fn run_monte_carlo(
    side_a: &[SimCombatant],
    side_b: &[SimCombatant],
    n: u32,
    side_a_label: String,
    side_b_label: String,
) -> MonteCarloResult {
    let mut side_a_wins = 0u32;
    let mut side_b_wins = 0u32;
    let mut draws = 0u32;
    let mut total_rounds = 0u32;

    for _ in 0..n {
        let result = run_combat(side_a, side_b);
        total_rounds += result.rounds;
        match result.winner {
            Some(0) => side_a_wins += 1,
            Some(1) => side_b_wins += 1,
            _ => draws += 1,
        }
    }

    let avg_rounds = if n > 0 { total_rounds as f32 / n as f32 } else { 0.0 };

    MonteCarloResult {
        num_sims: n,
        side_a_wins,
        side_b_wins,
        draws,
        avg_rounds,
        side_a_label,
        side_b_label,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::combat_stats::ParsedAttack;

    fn make_attack(to_hit: i8, damage_dice: &str, damage_avg: f32) -> ParsedAttack {
        ParsedAttack {
            name: "Test Attack".to_string(),
            attack_type: "mw".to_string(),
            to_hit,
            reach: Some(5),
            range: None,
            damage_dice: damage_dice.to_string(),
            damage_avg,
            damage_type: "slashing".to_string(),
            extra_damage: Vec::new(),
            effect: String::new(),
        }
    }

    fn make_combatant(name: &str, hp: i32, ac: u8, attack: ParsedAttack, side: usize) -> SimCombatant {
        SimCombatant {
            name: name.to_string(),
            max_hp: hp,
            current_hp: hp,
            ac,
            initiative_mod: 0,
            attacks: vec![attack],
            multiattack_count: 1,
            side,
        }
    }

    #[test]
    fn test_run_combat_one_side_wins() {
        // Strong side A vs weak side B: side A should almost always win
        let attack_strong = make_attack(10, "2d10 + 5", 16.0);
        let attack_weak = make_attack(2, "1d4", 2.5);

        let side_a = vec![
            make_combatant("Fighter", 100, 18, attack_strong.clone(), 0),
            make_combatant("Paladin", 100, 18, attack_strong.clone(), 0),
        ];
        let side_b = vec![
            make_combatant("Goblin", 7, 8, attack_weak.clone(), 1),
        ];

        let result = run_combat(&side_a, &side_b);
        assert_eq!(result.winner, Some(0));
        assert!(!result.survivors().is_empty());
        assert!(result.rounds <= 100);
    }

    #[test]
    fn test_run_combat_produces_result() {
        let attack = make_attack(5, "1d8 + 3", 7.5);
        let side_a = vec![make_combatant("A1", 20, 12, attack.clone(), 0)];
        let side_b = vec![make_combatant("B1", 20, 12, attack.clone(), 1)];

        let result = run_combat(&side_a, &side_b);
        // One side wins or it's a draw
        assert!(result.winner.is_some() || result.rounds == 100);
    }

    #[test]
    fn test_run_combat_no_attacks_is_draw() {
        // Combatants with no attacks should result in a draw after 100 rounds
        let side_a = vec![SimCombatant {
            name: "Harmless A".to_string(),
            max_hp: 10, current_hp: 10, ac: 10,
            initiative_mod: 0, attacks: Vec::new(),
            multiattack_count: 1, side: 0,
        }];
        let side_b = vec![SimCombatant {
            name: "Harmless B".to_string(),
            max_hp: 10, current_hp: 10, ac: 10,
            initiative_mod: 0, attacks: Vec::new(),
            multiattack_count: 1, side: 1,
        }];

        let result = run_combat(&side_a, &side_b);
        assert!(result.winner.is_none());
        assert_eq!(result.rounds, 100);
    }

    #[test]
    fn test_monte_carlo_aggregation() {
        let attack_strong = make_attack(10, "2d10 + 5", 16.0);
        let attack_weak = make_attack(2, "1d4", 2.5);

        let side_a = vec![
            make_combatant("Fighter", 100, 18, attack_strong.clone(), 0),
        ];
        let side_b = vec![
            make_combatant("Goblin", 7, 8, attack_weak.clone(), 1),
        ];

        let result = run_monte_carlo(
            &side_a, &side_b, 20,
            "Party".to_string(), "Goblins".to_string(),
        );

        assert_eq!(result.num_sims, 20);
        assert_eq!(result.side_a_wins + result.side_b_wins + result.draws, 20);
        // Strong side A should win the vast majority
        assert!(result.side_a_wins > 10);
        assert!(result.avg_rounds > 0.0);
        assert_eq!(result.side_a_label, "Party");
        assert_eq!(result.side_b_label, "Goblins");
    }

    #[test]
    fn test_build_combatants_from_party() {
        let party = vec![
            PlayerCharacter {
                id: "pc1".to_string(),
                name: "Aragorn".to_string(),
                class: "Fighter".to_string(),
                ac: 16,
                max_hp: 45,
                current_hp: 45,
                initiative_modifier: 2,
                passive_perception: 14,
                notes: String::new(),
                attack_bonus: 7,
                damage_dice: "1d8 + 4".to_string(),
            },
        ];

        let combatants = build_combatants_from_party(&party, 0);
        assert_eq!(combatants.len(), 1);
        assert_eq!(combatants[0].name, "Aragorn");
        assert_eq!(combatants[0].ac, 16);
        assert_eq!(combatants[0].max_hp, 45);
        assert_eq!(combatants[0].attacks.len(), 1);
        assert_eq!(combatants[0].attacks[0].to_hit, 7);
        assert_eq!(combatants[0].side, 0);
    }

    #[test]
    fn test_estimate_dice_avg() {
        assert!((estimate_dice_avg("1d8 + 3") - 7.5).abs() < 0.01);
        assert!((estimate_dice_avg("2d6") - 7.0).abs() < 0.01);
        assert!((estimate_dice_avg("1d4 - 1") - 1.5).abs() < 0.01);
        assert!((estimate_dice_avg("5") - 5.0).abs() < 0.01);
    }
}
