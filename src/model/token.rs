use serde::{Deserialize, Serialize};

use super::encounter::MonsterInstanceId;

/// What a map token stands for.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TokenKind {
    /// One monster instance of an encounter; numbering matches the combat tracker.
    Monster(MonsterInstanceId),
    /// A player character, by `PlayerCharacter::id`.
    Player(String),
}

/// A creature token placed on the map.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MapToken {
    pub kind: TokenKind,
    /// Token center in grid units (float for sub-grid placement).
    pub x: f32,
    pub y: f32,
}
