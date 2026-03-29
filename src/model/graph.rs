use std::collections::HashMap;

use petgraph::graph::UnGraph;
use serde::{Deserialize, Serialize};

use super::{Connection, Room};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DungeonGraph {
    pub rooms: Vec<Room>,
    pub connections: Vec<StoredEdge>,
    /// Visual positions of rooms in the graph editor (room_id -> (x, y))
    #[serde(default)]
    pub graph_positions: HashMap<String, (f32, f32)>,
    /// Room groups with optional solver constraints
    #[serde(default)]
    pub groups: Vec<RoomGroup>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RoomGroup {
    pub id: String,
    pub label: String,
    pub room_ids: Vec<String>,
    /// Max bounding width in grid squares (None = unconstrained)
    pub max_width: Option<u32>,
    /// Max bounding height in grid squares (None = unconstrained)
    pub max_height: Option<u32>,
    /// Display color (RGBA)
    #[serde(default = "default_group_color")]
    pub color: [u8; 4],
    /// Spatial position (top-left grid coordinate). Computed from rooms if None.
    #[serde(default)]
    pub spatial_x: Option<i32>,
    #[serde(default)]
    pub spatial_y: Option<i32>,
}

fn default_group_color() -> [u8; 4] {
    [100, 150, 255, 40]
}

impl RoomGroup {
    pub fn new(label: String) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            label,
            room_ids: Vec::new(),
            max_width: None,
            max_height: None,
            color: default_group_color(),
            spatial_x: None,
            spatial_y: None,
        }
    }

    /// Compute the bounding rect of this group's rooms in the spatial layout.
    /// Returns (x, y, w, h) in grid coordinates, or None if no rooms are placed.
    pub fn spatial_bounds(&self, layout: &crate::model::SpatialLayout) -> Option<(i32, i32, u32, u32)> {
        let mut min_x = i32::MAX;
        let mut min_y = i32::MAX;
        let mut max_x = i32::MIN;
        let mut max_y = i32::MIN;
        let mut found = false;

        for rl in &layout.rooms {
            if self.room_ids.contains(&rl.room_id) {
                min_x = min_x.min(rl.x);
                min_y = min_y.min(rl.y);
                max_x = max_x.max(rl.x + rl.width as i32);
                max_y = max_y.max(rl.y + rl.height as i32);
                found = true;
            }
        }

        if !found {
            return None;
        }

        // Use spatial position if set, otherwise use computed bounds
        let x = self.spatial_x.unwrap_or(min_x);
        let y = self.spatial_y.unwrap_or(min_y);
        let w = self.max_width.unwrap_or((max_x - min_x) as u32);
        let h = self.max_height.unwrap_or((max_y - min_y) as u32);

        Some((x, y, w, h))
    }
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
            graph_positions: HashMap::new(),
            groups: Vec::new(),
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
