use std::collections::HashMap;

use petgraph::graph::UnGraph;
use serde::{Deserialize, Serialize};

use super::{Connection, Room};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DungeonGraph {
    pub rooms: Vec<Room>,
    pub connections: Vec<StoredEdge>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StoredEdge {
    pub source_room_id: String,
    pub target_room_id: String,
    pub connection: Connection,
}

impl DungeonGraph {
    pub fn new() -> Self {
        Self {
            rooms: Vec::new(),
            connections: Vec::new(),
        }
    }

    pub fn add_room(&mut self, room: Room) {
        self.rooms.push(room);
    }

    pub fn remove_room(&mut self, room_id: &str) {
        self.rooms.retain(|r| r.id != room_id);
        self.connections
            .retain(|e| e.source_room_id != room_id && e.target_room_id != room_id);
    }

    pub fn add_connection(&mut self, source_id: String, target_id: String, connection: Connection) {
        self.connections.push(StoredEdge {
            source_room_id: source_id,
            target_room_id: target_id,
            connection,
        });
    }

    pub fn remove_connection(&mut self, connection_id: &str) {
        self.connections.retain(|e| e.connection.id != connection_id);
    }

    pub fn room_by_id(&self, id: &str) -> Option<&Room> {
        self.rooms.iter().find(|r| r.id == id)
    }

    pub fn room_by_id_mut(&mut self, id: &str) -> Option<&mut Room> {
        self.rooms.iter_mut().find(|r| r.id == id)
    }

    pub fn connection_by_id_mut(&mut self, id: &str) -> Option<&mut StoredEdge> {
        self.connections.iter_mut().find(|e| e.connection.id == id)
    }

    /// Build a petgraph for algorithms (BFS, pathfinding, etc.)
    pub fn build_petgraph(&self) -> (UnGraph<String, String>, HashMap<String, petgraph::graph::NodeIndex>) {
        let mut graph = UnGraph::new_undirected();
        let mut node_map = HashMap::new();

        for room in &self.rooms {
            let idx = graph.add_node(room.id.clone());
            node_map.insert(room.id.clone(), idx);
        }

        for edge in &self.connections {
            if let (Some(&src), Some(&tgt)) = (
                node_map.get(&edge.source_room_id),
                node_map.get(&edge.target_room_id),
            ) {
                graph.add_edge(src, tgt, edge.connection.id.clone());
            }
        }

        (graph, node_map)
    }
}

impl Default for DungeonGraph {
    fn default() -> Self {
        Self::new()
    }
}
