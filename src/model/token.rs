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
    /// The floor it stands on; `None` (older maps) takes the floor of the room under it.
    #[serde(default)]
    pub floor: Option<i32>,
}

impl MapToken {
    /// The floor this token is on: its own, else the lowest floor of the topmost room
    /// under it, else `None` (outside every room, with no floor of its own).
    pub fn floor_in(&self, graph: &super::DungeonGraph, layout: &super::SpatialLayout) -> Option<i32> {
        self.floor.or_else(|| {
            let room = layout.room_at_grid(graph, self.x.floor() as i32, self.y.floor() as i32)?;
            graph.room_by_id(&room.room_id).map(|r| r.floor.range().0)
        })
    }

    /// Whether the token shows when viewing `floor` (`None` = all floors). A token with
    /// no floor of any kind shows on every floor.
    pub fn on_floor(&self, floor: Option<i32>, graph: &super::DungeonGraph, layout: &super::SpatialLayout) -> bool {
        match (floor, self.floor_in(graph, layout)) {
            (Some(view), Some(mine)) => view == mine,
            _ => true,
        }
    }
}
