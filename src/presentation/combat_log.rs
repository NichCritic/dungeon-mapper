use super::dice::AttackResult;
use crate::model::combat_stats::ParsedAttack;

/// A single line in the combat log.
pub struct LogEntry {
    pub text: String,
    pub color: [u8; 3],
}

/// Scrollable combat log with colored text lines.
pub struct CombatLog {
    pub entries: Vec<LogEntry>,
}

// Color scheme constants
const COLOR_WHITE: [u8; 3] = [255, 255, 255];
const COLOR_GOLD: [u8; 3] = [255, 215, 0];
const COLOR_HIT: [u8; 3] = [100, 255, 100];
const COLOR_MISS: [u8; 3] = [255, 100, 100];
const COLOR_DAMAGE: [u8; 3] = [255, 80, 80];
const COLOR_HEALING: [u8; 3] = [80, 255, 80];
const COLOR_GRAY: [u8; 3] = [150, 150, 150];

impl CombatLog {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    /// Add a log entry with a specific color.
    pub fn log(&mut self, text: String, color: [u8; 3]) {
        self.entries.push(LogEntry { text, color });
    }

    /// Log an attack result, including any additional effects from the attack.
    pub fn log_attack(&mut self, attacker: &str, target: &str, attack_name: &str, result: &AttackResult, attack: Option<&ParsedAttack>) {
        // Attack roll line
        let roll_text = format!(
            "{} attacks {} with {} - d20({}) + bonus = {} vs AC",
            attacker, target, attack_name, result.attack_roll, result.attack_total,
        );
        self.log(roll_text, COLOR_WHITE);

        if result.is_crit {
            self.log(format!("  CRITICAL HIT!"), COLOR_GOLD);
        } else if result.is_fumble {
            self.log(format!("  CRITICAL MISS!"), COLOR_MISS);
        }

        if result.hit {
            if let Some(ref dmg) = result.damage {
                let rolls_str: Vec<String> = dmg.rolls.iter().map(|r| r.to_string()).collect();
                let total_damage = dmg.total + result.extra_damage.iter()
                    .map(|(d, _)| d.total)
                    .sum::<i32>();
                self.log(
                    format!("  HIT! {} damage [{}] + {} = {}",
                        dmg.expression, rolls_str.join(", "), dmg.modifier, total_damage),
                    COLOR_HIT,
                );
            }
            for (extra, dtype) in &result.extra_damage {
                let rolls_str: Vec<String> = extra.rolls.iter().map(|r| r.to_string()).collect();
                self.log(
                    format!("    + {} {} damage [{}]", extra.total, dtype, rolls_str.join(", ")),
                    COLOR_HIT,
                );
            }
            // Log additional effects on hit
            if let Some(atk) = attack {
                if !atk.effect.is_empty() {
                    self.log(format!("  Effect: {}", atk.effect), COLOR_GOLD);
                }
            }
        } else {
            self.log(format!("  MISS!"), COLOR_MISS);
        }
    }

    /// Log damage applied to a target.
    pub fn log_damage(&mut self, target: &str, amount: i32, remaining_hp: i32) {
        self.log(
            format!("{} takes {} damage ({} HP remaining)", target, amount, remaining_hp),
            COLOR_DAMAGE,
        );
    }

    /// Log healing applied to a target.
    pub fn log_healing(&mut self, target: &str, amount: i32, new_hp: i32) {
        self.log(
            format!("{} healed for {} ({} HP)", target, amount, new_hp),
            COLOR_HEALING,
        );
    }

    /// Log a new round.
    pub fn log_round(&mut self, round: u32) {
        self.log(format!("--- Round {} ---", round), COLOR_GRAY);
    }

    /// Log a combatant's turn.
    pub fn log_turn(&mut self, name: &str) {
        self.log(format!("> {}'s turn", name), COLOR_GRAY);
    }
}
