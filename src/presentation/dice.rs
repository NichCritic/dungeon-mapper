use rand::Rng;
use regex::Regex;

use crate::model::combat_stats::ParsedAttack;

/// Result of rolling a dice expression like "2d6 + 4".
#[derive(Clone, Debug)]
pub struct DiceResult {
    pub expression: String,
    pub rolls: Vec<u32>,
    pub modifier: i32,
    pub total: i32,
}

/// Result of a full attack roll (d20 + to_hit, then damage if hit).
#[derive(Clone, Debug)]
pub struct AttackResult {
    pub attack_roll: u32,
    pub attack_total: i32,
    pub is_crit: bool,
    pub is_fumble: bool,
    pub hit: bool,
    pub damage: Option<DiceResult>,
    pub extra_damage: Vec<(DiceResult, String)>,
}

/// Parse and roll a dice expression like "2d6 + 4", "1d8 - 1", "3d6", etc.
#[allow(dead_code)]
pub fn roll_dice(expr: &str) -> DiceResult {
    let mut rng = rand::thread_rng();
    roll_dice_with_rng(expr, &mut rng)
}

/// Roll dice with a provided RNG (for testability).
fn roll_dice_with_rng(expr: &str, rng: &mut impl Rng) -> DiceResult {
    let expr = expr.trim();
    let re = Regex::new(r"^(\d+)d(\d+)\s*([+-]\s*\d+)?$").unwrap();

    if let Some(caps) = re.captures(expr) {
        let count: u32 = caps[1].parse().unwrap_or(1);
        let sides: u32 = caps[2].parse().unwrap_or(6);
        let modifier: i32 = caps.get(3)
            .map(|m| m.as_str().replace(' ', "").parse::<i32>().unwrap_or(0))
            .unwrap_or(0);

        let mut rolls = Vec::with_capacity(count as usize);
        for _ in 0..count {
            rolls.push(rng.gen_range(1..=sides));
        }

        let sum: u32 = rolls.iter().sum();
        let total = sum as i32 + modifier;

        DiceResult {
            expression: expr.to_string(),
            rolls,
            modifier,
            total,
        }
    } else {
        // Fallback: treat as a constant
        let val: i32 = expr.parse().unwrap_or(0);
        DiceResult {
            expression: expr.to_string(),
            rolls: Vec::new(),
            modifier: val,
            total: val,
        }
    }
}

/// Roll dice, doubling the number of dice (for critical hits).
fn roll_dice_crit(expr: &str, rng: &mut impl Rng) -> DiceResult {
    let expr = expr.trim();
    let re = Regex::new(r"^(\d+)d(\d+)\s*([+-]\s*\d+)?$").unwrap();

    if let Some(caps) = re.captures(expr) {
        let count: u32 = caps[1].parse().unwrap_or(1);
        let sides: u32 = caps[2].parse().unwrap_or(6);
        let modifier: i32 = caps.get(3)
            .map(|m| m.as_str().replace(' ', "").parse::<i32>().unwrap_or(0))
            .unwrap_or(0);

        let doubled_count = count * 2;
        let mut rolls = Vec::with_capacity(doubled_count as usize);
        for _ in 0..doubled_count {
            rolls.push(rng.gen_range(1..=sides));
        }

        let sum: u32 = rolls.iter().sum();
        let total = sum as i32 + modifier;

        DiceResult {
            expression: format!("{}d{}{}", doubled_count, sides,
                if modifier > 0 { format!(" + {}", modifier) }
                else if modifier < 0 { format!(" - {}", modifier.abs()) }
                else { String::new() }),
            rolls,
            modifier,
            total,
        }
    } else {
        roll_dice_with_rng(expr, rng)
    }
}

/// Roll an attack using a ParsedAttack against a target AC.
pub fn roll_attack(attack: &ParsedAttack, target_ac: u8) -> AttackResult {
    let mut rng = rand::thread_rng();

    let attack_roll = rng.gen_range(1..=20u32);
    let attack_total = attack_roll as i32 + attack.to_hit as i32;
    let is_crit = attack_roll == 20;
    let is_fumble = attack_roll == 1;
    let hit = is_crit || (!is_fumble && attack_total >= target_ac as i32);

    let (damage, extra_damage) = if hit {
        let dmg = if is_crit {
            roll_dice_crit(&attack.damage_dice, &mut rng)
        } else {
            roll_dice_with_rng(&attack.damage_dice, &mut rng)
        };

        let extras: Vec<(DiceResult, String)> = attack.extra_damage.iter().map(|rider| {
            let result = if is_crit {
                roll_dice_crit(&rider.damage_dice, &mut rng)
            } else {
                roll_dice_with_rng(&rider.damage_dice, &mut rng)
            };
            (result, rider.damage_type.clone())
        }).collect();

        (Some(dmg), extras)
    } else {
        (None, Vec::new())
    };

    AttackResult {
        attack_roll,
        attack_total,
        is_crit,
        is_fumble,
        hit,
        damage,
        extra_damage,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_roll_dice_simple() {
        let result = roll_dice("1d6");
        assert_eq!(result.rolls.len(), 1);
        assert!(result.rolls[0] >= 1 && result.rolls[0] <= 6);
        assert_eq!(result.modifier, 0);
        assert_eq!(result.total, result.rolls[0] as i32);
    }

    #[test]
    fn test_roll_dice_with_positive_modifier() {
        let result = roll_dice("2d6 + 4");
        assert_eq!(result.rolls.len(), 2);
        assert_eq!(result.modifier, 4);
        let sum: u32 = result.rolls.iter().sum();
        assert_eq!(result.total, sum as i32 + 4);
    }

    #[test]
    fn test_roll_dice_with_negative_modifier() {
        let result = roll_dice("1d8 - 1");
        assert_eq!(result.rolls.len(), 1);
        assert_eq!(result.modifier, -1);
        let sum: u32 = result.rolls.iter().sum();
        assert_eq!(result.total, sum as i32 - 1);
    }

    #[test]
    fn test_roll_dice_multiple() {
        let result = roll_dice("3d8");
        assert_eq!(result.rolls.len(), 3);
        for &r in &result.rolls {
            assert!(r >= 1 && r <= 8);
        }
    }

    #[test]
    fn test_roll_dice_constant() {
        let result = roll_dice("5");
        assert!(result.rolls.is_empty());
        assert_eq!(result.total, 5);
    }

    #[test]
    fn test_roll_dice_crit_doubles_dice() {
        let mut rng = rand::thread_rng();
        let result = roll_dice_crit("2d6 + 3", &mut rng);
        assert_eq!(result.rolls.len(), 4); // doubled from 2 to 4
        assert_eq!(result.modifier, 3);
    }

    #[test]
    fn test_roll_attack_basic() {
        let attack = ParsedAttack {
            name: "Sword".into(),
            attack_type: "mw".into(),
            to_hit: 5,
            reach: Some(5),
            range: None,
            damage_dice: "1d8 + 3".into(),
            damage_avg: 7.5,
            damage_type: "slashing".into(),
            extra_damage: Vec::new(),
        };

        // Run several times to cover hit/miss cases
        for _ in 0..20 {
            let result = roll_attack(&attack, 10);
            assert!(result.attack_roll >= 1 && result.attack_roll <= 20);

            if result.is_crit {
                assert!(result.hit);
                assert!(result.damage.is_some());
            }
            if result.is_fumble {
                assert!(!result.hit);
                assert!(result.damage.is_none());
            }
            if result.hit {
                assert!(result.damage.is_some());
            } else {
                assert!(result.damage.is_none());
            }
        }
    }

    #[test]
    fn test_roll_attack_with_extra_damage() {
        let attack = ParsedAttack {
            name: "Bite".into(),
            attack_type: "mw".into(),
            to_hit: 14,
            reach: Some(10),
            range: None,
            damage_dice: "2d10 + 8".into(),
            damage_avg: 19.0,
            damage_type: "piercing".into(),
            extra_damage: vec![
                crate::model::combat_stats::DamageRider {
                    damage_dice: "2d6".into(),
                    damage_avg: 7.0,
                    damage_type: "fire".into(),
                },
            ],
        };

        // With +14 to hit vs AC 10, should almost always hit
        let result = roll_attack(&attack, 10);
        if result.hit {
            assert!(result.damage.is_some());
            assert_eq!(result.extra_damage.len(), 1);
            assert_eq!(result.extra_damage[0].1, "fire");
        }
    }
}
