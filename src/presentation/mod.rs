pub mod combat_log;
pub mod combat_sim;
pub mod combat_tracker;
pub mod dice;
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
    /// Combat tracker, activated by the DM during a session.
    pub combat_tracker: Option<combat_tracker::CombatTracker>,
    /// Which room the party token is in (None = not shown).
    pub party_room: Option<String>,
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
            combat_tracker: None,
            party_room: None,
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
            let reachable = if let Some(r) = range {
                bfs_within_range(&encounter.home_room_id, r, &dungeon.graph)
            } else {
                // Unlimited range: all rooms are reachable
                dungeon.graph.rooms.iter().map(|r| r.id.clone()).collect()
            };
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::*;

    fn make_dungeon_with_encounters() -> Dungeon {
        let mut graph = DungeonGraph::new();
        let r1 = Room::new("Room A".to_string());
        let r2 = Room::new("Room B".to_string());
        let id1 = r1.id.clone();
        let id2 = r2.id.clone();
        graph.add_room(r1);
        graph.add_room(r2);
        graph.add_connection(id1.clone(), id2.clone(), Connection::new(ConnectionType::Door));

        let enc = Encounter::new("Goblins".to_string(), id1.clone());

        Dungeon {
            name: "test".to_string(),
            graph,
            layout: None,
            theme: Theme::default(),
            encounters: vec![enc],
            custom_monsters: Vec::new(),
            party: Vec::new(),
            annotations: Vec::new(),
        }
    }

    #[test]
    fn test_new_from_dungeon_all_hidden() {
        let dungeon = make_dungeon_with_encounters();
        let state = PresentationState::new_from_dungeon(&dungeon);

        for room in &dungeon.graph.rooms {
            assert_eq!(*state.room_visibility(&room.id), Visibility::Hidden);
        }
    }

    #[test]
    fn test_new_from_dungeon_encounter_positions() {
        let dungeon = make_dungeon_with_encounters();
        let state = PresentationState::new_from_dungeon(&dungeon);

        let enc = &dungeon.encounters[0];
        assert_eq!(
            state.encounter_positions.get(&enc.id).unwrap(),
            &enc.home_room_id
        );
    }

    #[test]
    fn test_encounter_room_from_positions() {
        let dungeon = make_dungeon_with_encounters();
        let mut state = PresentationState::new_from_dungeon(&dungeon);
        let enc = &dungeon.encounters[0];
        let room_b_id = dungeon.graph.rooms[1].id.clone();

        // Move encounter to room B
        state.encounter_positions.insert(enc.id.clone(), room_b_id.clone());
        assert_eq!(state.encounter_room(enc), room_b_id);
    }

    #[test]
    fn test_encounter_room_fallback() {
        let dungeon = make_dungeon_with_encounters();
        let mut state = PresentationState::new_from_dungeon(&dungeon);
        let enc = &dungeon.encounters[0];

        // Remove from positions to trigger fallback
        state.encounter_positions.remove(&enc.id);
        assert_eq!(state.encounter_room(enc), enc.home_room_id);
    }

    #[test]
    fn test_encounter_ids_in_room_empty() {
        let dungeon = make_dungeon_with_encounters();
        let state = PresentationState::new_from_dungeon(&dungeon);
        let room_b_id = &dungeon.graph.rooms[1].id;

        // No encounters in room B
        assert!(state.encounter_ids_in_room(room_b_id).is_empty());
    }

    #[test]
    fn test_encounter_ids_in_room_found() {
        let dungeon = make_dungeon_with_encounters();
        let state = PresentationState::new_from_dungeon(&dungeon);
        let room_a_id = &dungeon.graph.rooms[0].id;

        let ids = state.encounter_ids_in_room(room_a_id);
        assert_eq!(ids.len(), 1);
        assert_eq!(ids[0], dungeon.encounters[0].id);
    }

    #[test]
    fn test_bfs_within_range_single_room() {
        let mut graph = DungeonGraph::new();
        let r1 = Room::new("A".to_string());
        let id1 = r1.id.clone();
        graph.add_room(r1);

        let reachable = bfs_within_range(&id1, 0, &graph);
        assert_eq!(reachable.len(), 1);
        assert!(reachable.contains(&id1));
    }

    #[test]
    fn test_bfs_within_range_linear_chain() {
        // Build a chain: A -- B -- C -- D -- E
        let mut graph = DungeonGraph::new();
        let rooms: Vec<Room> = (0..5).map(|i| Room::new(format!("Room {}", i))).collect();
        let ids: Vec<String> = rooms.iter().map(|r| r.id.clone()).collect();
        for room in rooms {
            graph.add_room(room);
        }
        for i in 0..4 {
            graph.add_connection(ids[i].clone(), ids[i + 1].clone(), Connection::new(ConnectionType::Open));
        }

        // From room A (index 0) with range 2, should reach A, B, C
        let reachable = bfs_within_range(&ids[0], 2, &graph);
        assert_eq!(reachable.len(), 3);
        assert!(reachable.contains(&ids[0]));
        assert!(reachable.contains(&ids[1]));
        assert!(reachable.contains(&ids[2]));
        assert!(!reachable.contains(&ids[3]));
    }

    #[test]
    fn test_bfs_within_range_disconnected() {
        let mut graph = DungeonGraph::new();
        let r1 = Room::new("A".to_string());
        let r2 = Room::new("B".to_string());
        let id1 = r1.id.clone();
        let id2 = r2.id.clone();
        graph.add_room(r1);
        graph.add_room(r2);
        // No connections

        let reachable = bfs_within_range(&id1, 5, &graph);
        assert_eq!(reachable.len(), 1);
        assert!(reachable.contains(&id1));
        assert!(!reachable.contains(&id2));
    }
}
