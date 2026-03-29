use crate::model::DungeonGraph;
use super::{PresentationState, Visibility};

/// Derive corridor visibility from endpoint rooms and door state.
/// A corridor is visible if its door is open and at least one endpoint room
/// is not Hidden. It takes the best visibility of the two endpoint rooms.
pub fn corridor_visibility(
    connection_id: &str,
    presentation: &PresentationState,
    graph: &DungeonGraph,
) -> Visibility {
    if !presentation.is_door_open(connection_id) {
        return Visibility::Hidden;
    }

    let edge = graph.connections.iter().find(|e| e.connection.id == connection_id);
    let Some(edge) = edge else { return Visibility::Hidden };

    let src_vis = presentation.room_visibility(&edge.source_room_id);
    let tgt_vis = presentation.room_visibility(&edge.target_room_id);

    if *src_vis == Visibility::Visible || *tgt_vis == Visibility::Visible {
        Visibility::Visible
    } else if *src_vis == Visibility::Explored || *tgt_vis == Visibility::Explored {
        Visibility::Explored
    } else {
        Visibility::Hidden
    }
}

// --- Room visibility helpers ---

pub fn reveal_room(room_id: &str, presentation: &mut PresentationState) {
    presentation.room_visibility.insert(room_id.to_string(), Visibility::Visible);
}

pub fn explore_room(room_id: &str, presentation: &mut PresentationState) {
    presentation.room_visibility.insert(room_id.to_string(), Visibility::Explored);
}

pub fn hide_room(room_id: &str, presentation: &mut PresentationState) {
    presentation.room_visibility.insert(room_id.to_string(), Visibility::Hidden);
}

pub fn cycle_room_visibility(room_id: &str, presentation: &mut PresentationState) {
    let current = presentation.room_visibility(room_id).clone();
    let next = match current {
        Visibility::Hidden => Visibility::Visible,
        Visibility::Visible => Visibility::Explored,
        Visibility::Explored => Visibility::Hidden,
    };
    presentation.room_visibility.insert(room_id.to_string(), next);
}

/// Reveal a room and all of its neighbors, opening all connecting doors.
pub fn reveal_room_and_adjacent(
    room_id: &str,
    presentation: &mut PresentationState,
    graph: &DungeonGraph,
) {
    reveal_room(room_id, presentation);

    for edge in &graph.connections {
        let neighbor_id = if edge.source_room_id == room_id {
            Some(&edge.target_room_id)
        } else if edge.target_room_id == room_id {
            Some(&edge.source_room_id)
        } else {
            None
        };
        if let Some(neighbor_id) = neighbor_id {
            reveal_room(neighbor_id, presentation);
            presentation.doors_open.insert(edge.connection.id.clone());
        }
    }
}

// --- Door helpers ---

pub fn open_door(connection_id: &str, presentation: &mut PresentationState) {
    presentation.doors_open.insert(connection_id.to_string());
}

pub fn close_door(connection_id: &str, presentation: &mut PresentationState) {
    presentation.doors_open.remove(connection_id);
}

pub fn toggle_door(connection_id: &str, presentation: &mut PresentationState) {
    if presentation.is_door_open(connection_id) {
        close_door(connection_id, presentation);
    } else {
        open_door(connection_id, presentation);
    }
}

/// Open all doors connected to a room.
pub fn open_room_doors(room_id: &str, presentation: &mut PresentationState, graph: &DungeonGraph) {
    for edge in &graph.connections {
        if edge.source_room_id == room_id || edge.target_room_id == room_id {
            open_door(&edge.connection.id, presentation);
        }
    }
}

/// Close all doors connected to a room.
pub fn close_room_doors(room_id: &str, presentation: &mut PresentationState, graph: &DungeonGraph) {
    for edge in &graph.connections {
        if edge.source_room_id == room_id || edge.target_room_id == room_id {
            close_door(&edge.connection.id, presentation);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::*;

    fn test_graph() -> DungeonGraph {
        let mut graph = DungeonGraph::new();
        let r1 = Room::new("Room A".to_string());
        let r2 = Room::new("Room B".to_string());
        let r3 = Room::new("Room C".to_string());
        let id1 = r1.id.clone();
        let id2 = r2.id.clone();
        let id3 = r3.id.clone();
        graph.add_room(r1);
        graph.add_room(r2);
        graph.add_room(r3);
        graph.add_connection(id1.clone(), id2.clone(), Connection::new(ConnectionType::Door));
        graph.add_connection(id2.clone(), id3.clone(), Connection::new(ConnectionType::Open));
        graph
    }

    #[test]
    fn test_cycle_room_visibility() {
        let graph = test_graph();
        let dungeon = Dungeon { name: "test".into(), graph, layout: None, theme: Theme::default() };
        let mut state = PresentationState::new_from_dungeon(&dungeon);
        let room_id = &dungeon.graph.rooms[0].id;

        assert_eq!(*state.room_visibility(room_id), Visibility::Hidden);
        cycle_room_visibility(room_id, &mut state);
        assert_eq!(*state.room_visibility(room_id), Visibility::Visible);
        cycle_room_visibility(room_id, &mut state);
        assert_eq!(*state.room_visibility(room_id), Visibility::Explored);
        cycle_room_visibility(room_id, &mut state);
        assert_eq!(*state.room_visibility(room_id), Visibility::Hidden);
    }

    #[test]
    fn test_corridor_hidden_when_door_closed() {
        let graph = test_graph();
        let dungeon = Dungeon { name: "test".into(), graph, layout: None, theme: Theme::default() };
        let mut state = PresentationState::new_from_dungeon(&dungeon);
        let conn_id = &dungeon.graph.connections[0].connection.id;
        let src_id = &dungeon.graph.connections[0].source_room_id;

        // Room visible but door closed → corridor hidden
        reveal_room(src_id, &mut state);
        assert_eq!(corridor_visibility(conn_id, &state, &dungeon.graph), Visibility::Hidden);

        // Open door → corridor visible
        open_door(conn_id, &mut state);
        assert_eq!(corridor_visibility(conn_id, &state, &dungeon.graph), Visibility::Visible);
    }

    #[test]
    fn test_corridor_hidden_when_both_rooms_hidden() {
        let graph = test_graph();
        let dungeon = Dungeon { name: "test".into(), graph, layout: None, theme: Theme::default() };
        let mut state = PresentationState::new_from_dungeon(&dungeon);
        let conn_id = &dungeon.graph.connections[0].connection.id;

        // Door open but both rooms hidden → corridor hidden
        open_door(conn_id, &mut state);
        assert_eq!(corridor_visibility(conn_id, &state, &dungeon.graph), Visibility::Hidden);
    }

    #[test]
    fn test_toggle_door() {
        let graph = test_graph();
        let dungeon = Dungeon { name: "test".into(), graph, layout: None, theme: Theme::default() };
        let mut state = PresentationState::new_from_dungeon(&dungeon);
        let conn_id = &dungeon.graph.connections[0].connection.id;

        assert!(!state.is_door_open(conn_id));
        toggle_door(conn_id, &mut state);
        assert!(state.is_door_open(conn_id));
        toggle_door(conn_id, &mut state);
        assert!(!state.is_door_open(conn_id));
    }

    #[test]
    fn test_reveal_room_and_adjacent() {
        let graph = test_graph();
        let dungeon = Dungeon { name: "test".into(), graph, layout: None, theme: Theme::default() };
        let mut state = PresentationState::new_from_dungeon(&dungeon);
        let room_b_id = &dungeon.graph.rooms[1].id;

        reveal_room_and_adjacent(room_b_id, &mut state, &dungeon.graph);

        // All rooms visible
        for room in &dungeon.graph.rooms {
            assert_eq!(*state.room_visibility(&room.id), Visibility::Visible);
        }
        // All doors open
        for edge in &dungeon.graph.connections {
            assert!(state.is_door_open(&edge.connection.id));
        }
    }
}
