pub mod fog;
pub mod lighting;

use std::collections::{HashMap, HashSet, VecDeque};

use crate::model::{Dungeon, DungeonGraph, EncounterType};

#[derive(Clone, Debug, PartialEq)]
pub enum Visibility {
    Hidden,
    Explored,
    Visible,
}

#[derive(Clone, Debug)]
pub struct LightSource {
    pub id: String,
    pub room_id: String,
    pub radius: f32,
    pub intensity: f32,
    pub color: [u8; 3],
}

pub struct PresentationState {
    /// Per-room visibility, directly controlled by the DM.
    pub room_visibility: HashMap<String, Visibility>,
    /// Set of open doors (connection IDs). A door being open means the
    /// corridor is visible to players (if at least one endpoint room is
    /// not Hidden).
    pub doors_open: HashSet<String>,
    pub light_sources: Vec<LightSource>,
    pub ambient_light: f32,
    /// Whether room labels are shown in the player view.
    pub show_labels_player: bool,
    /// Runtime positions of encounters: encounter_id -> current_room_id.
    /// Initialized from encounter home rooms, mutated by tick.
    pub encounter_positions: HashMap<String, String>,
}

impl PresentationState {
    pub fn new_from_dungeon(dungeon: &Dungeon) -> Self {
        let mut room_visibility = HashMap::new();
        for room in &dungeon.graph.rooms {
            room_visibility.insert(room.id.clone(), Visibility::Hidden);
        }
        let mut encounter_positions = HashMap::new();
        for enc in &dungeon.encounters {
            encounter_positions.insert(enc.id.clone(), enc.home_room_id.clone());
        }
        Self {
            room_visibility,
            doors_open: HashSet::new(),
            light_sources: Vec::new(),
            ambient_light: 0.0,
            show_labels_player: false,
            encounter_positions,
        }
    }

    pub fn room_visibility(&self, room_id: &str) -> &Visibility {
        self.room_visibility.get(room_id).unwrap_or(&Visibility::Hidden)
    }

    pub fn is_door_open(&self, connection_id: &str) -> bool {
        self.doors_open.contains(connection_id)
    }

    /// Get current room for an encounter (falls back to home room).
    pub fn encounter_room<'a>(&'a self, encounter: &'a crate::model::Encounter) -> &'a str {
        self.encounter_positions
            .get(&encounter.id)
            .map(|s| s.as_str())
            .unwrap_or(&encounter.home_room_id)
    }

    /// Tick all wandering encounters: each moves randomly up to its range
    /// from its home room.
    pub fn tick_encounters(&mut self, dungeon: &Dungeon) {
        use rand::Rng;
        let mut rng = rand::thread_rng();

        for encounter in &dungeon.encounters {
            let EncounterType::Wandering(range) = encounter.encounter_type else {
                continue;
            };

            let current_room = self.encounter_positions
                .get(&encounter.id)
                .cloned()
                .unwrap_or_else(|| encounter.home_room_id.clone());

            // BFS from home room to find all reachable rooms within range
            let reachable = bfs_within_range(&encounter.home_room_id, range, &dungeon.graph);
            if reachable.is_empty() {
                continue;
            }

            // Pick from neighbors of current room that are within range
            let neighbors: Vec<&String> = dungeon.graph.connections.iter()
                .filter_map(|e| {
                    if e.source_room_id == current_room {
                        Some(&e.target_room_id)
                    } else if e.target_room_id == current_room {
                        Some(&e.source_room_id)
                    } else {
                        None
                    }
                })
                .filter(|rid| reachable.contains(rid.as_str()))
                .collect();

            if neighbors.is_empty() {
                continue;
            }

            // 50% chance to stay, 50% chance to move to a neighbor
            if rng.gen_bool(0.5) {
                let idx = rng.gen_range(0..neighbors.len());
                self.encounter_positions.insert(encounter.id.clone(), neighbors[idx].clone());
            }
        }
    }

    /// Reset all encounter positions to their home rooms.
    pub fn reset_encounter_positions(&mut self, dungeon: &Dungeon) {
        self.encounter_positions.clear();
        for enc in &dungeon.encounters {
            self.encounter_positions.insert(enc.id.clone(), enc.home_room_id.clone());
        }
    }

    /// Get encounter IDs currently in a given room.
    pub fn encounter_ids_in_room(&self, room_id: &str) -> Vec<String> {
        self.encounter_positions.iter()
            .filter(|(_, rid)| rid.as_str() == room_id)
            .map(|(eid, _)| eid.clone())
            .collect()
    }
}

/// BFS from a starting room, returning all room IDs reachable within `max_dist` hops.
fn bfs_within_range(start_room_id: &str, max_dist: u32, graph: &DungeonGraph) -> HashSet<String> {
    let mut visited: HashMap<String, u32> = HashMap::new();
    let mut queue: VecDeque<(String, u32)> = VecDeque::new();

    visited.insert(start_room_id.to_string(), 0);
    queue.push_back((start_room_id.to_string(), 0));

    while let Some((room_id, dist)) = queue.pop_front() {
        if dist >= max_dist {
            continue;
        }
        for edge in &graph.connections {
            let neighbor = if edge.source_room_id == room_id {
                &edge.target_room_id
            } else if edge.target_room_id == room_id {
                &edge.source_room_id
            } else {
                continue;
            };
            if !visited.contains_key(neighbor.as_str()) {
                visited.insert(neighbor.clone(), dist + 1);
                queue.push_back((neighbor.clone(), dist + 1));
            }
        }
    }

    visited.into_keys().collect()
}
