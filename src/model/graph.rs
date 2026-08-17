use std::collections::{BTreeMap, HashMap, HashSet};

use petgraph::graph::UnGraph;
use serde::{Deserialize, Serialize};

use super::{Connection, Room};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DungeonGraph {
    pub rooms: Vec<Room>,
    pub connections: Vec<StoredEdge>,
    /// Visual positions of rooms in the graph editor (room_id -> (x, y)).
    /// Uses BTreeMap for deterministic serialization order (needed for undo hashing).
    #[serde(default)]
    pub graph_positions: BTreeMap<String, (f32, f32)>,
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
    /// When set, this group's `room_ids` are physically contained inside this parent room.
    #[serde(default)]
    pub parent_room_id: Option<String>,
    /// Grid squares of padding between children and parent walls (default 1).
    #[serde(default = "default_containment_padding")]
    pub containment_padding: u32,
}

fn default_group_color() -> [u8; 4] {
    [100, 150, 255, 40]
}

fn default_containment_padding() -> u32 {
    1
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
            parent_room_id: None,
            containment_padding: default_containment_padding(),
        }
    }

    /// Returns true if this is a containment group (has a parent room).
    pub fn is_containment(&self) -> bool {
        self.parent_room_id.is_some()
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
    /// User-pinned exit position on source room wall (corridor center-line at wall edge).
    #[serde(default)]
    pub source_exit: Option<super::spatial::ExitPos>,
    /// User-pinned exit position on target room wall (corridor center-line at wall edge).
    #[serde(default)]
    pub target_exit: Option<super::spatial::ExitPos>,
}

impl DungeonGraph {
    pub fn new() -> Self {
        Self {
            rooms: Vec::new(),
            connections: Vec::new(),
            graph_positions: BTreeMap::new(),
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
            source_exit: None,
            target_exit: None,
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

    /// Returns the IDs of all rooms that are children of the given parent room
    /// (i.e. contained in a containment group whose parent_room_id matches).
    pub fn children_of(&self, parent_room_id: &str) -> Vec<&str> {
        self.groups.iter()
            .filter(|g| g.parent_room_id.as_deref() == Some(parent_room_id))
            .flat_map(|g| g.room_ids.iter().map(|s| s.as_str()))
            .collect()
    }

    /// Returns the parent room ID if this room is a child in a containment group.
    pub fn parent_of(&self, room_id: &str) -> Option<&str> {
        self.groups.iter()
            .find(|g| g.parent_room_id.is_some() && g.room_ids.contains(&room_id.to_string()))
            .and_then(|g| g.parent_room_id.as_deref())
    }

    /// Returns the nesting depth of a room (0 = top-level, 1 = inside a container, etc.).
    pub fn nesting_depth(&self, room_id: &str) -> u32 {
        let mut depth = 0;
        let mut current = room_id.to_string();
        while let Some(parent) = self.parent_of(&current) {
            depth += 1;
            current = parent.to_string();
            if depth > 20 { break; } // cycle guard
        }
        depth
    }

    /// Returns true if this room is a container (has children in a containment group).
    pub fn is_container(&self, room_id: &str) -> bool {
        self.groups.iter().any(|g| g.parent_room_id.as_deref() == Some(room_id))
    }

    /// Returns the containment group for a given parent room, if any.
    pub fn containment_group(&self, parent_room_id: &str) -> Option<&RoomGroup> {
        self.groups.iter().find(|g| g.parent_room_id.as_deref() == Some(parent_room_id))
    }

    /// Validate containment hierarchy: no cycles, room in at most one containment group,
    /// container and children on same floor.
    pub fn validate_containment(&self) -> Vec<String> {
        let mut errors = Vec::new();

        // Check for rooms in multiple containment groups
        let mut child_to_group: HashMap<&str, &str> = HashMap::new();
        for group in &self.groups {
            if group.parent_room_id.is_none() {
                continue;
            }
            for rid in &group.room_ids {
                if let Some(existing_group) = child_to_group.get(rid.as_str()) {
                    errors.push(format!(
                        "Room '{}' is in multiple containment groups ('{}' and '{}')",
                        self.room_by_id(rid).map(|r| r.label.as_str()).unwrap_or("?"),
                        existing_group, group.label
                    ));
                } else {
                    child_to_group.insert(rid.as_str(), &group.label);
                }
            }
        }

        // Check for cycles in containment hierarchy
        for room in &self.rooms {
            let mut visited = HashSet::new();
            let mut current = room.id.as_str();
            while let Some(parent) = self.parent_of(current) {
                if !visited.insert(parent) {
                    errors.push(format!("Cycle in containment hierarchy involving room '{}'", room.label));
                    break;
                }
                current = parent;
                if visited.len() > 20 { break; }
            }
        }

        // Check floor consistency: children should be on same floor as parent
        for group in &self.groups {
            let Some(parent_id) = &group.parent_room_id else { continue };
            let parent_floor = self.room_by_id(parent_id).map(|r| r.floor);
            for rid in &group.room_ids {
                let child_floor = self.room_by_id(rid).map(|r| r.floor);
                if let (Some(pf), Some(cf)) = (parent_floor, child_floor) {
                    let parent_floors = pf.floors();
                    let child_floors = cf.floors();
                    if !child_floors.iter().any(|f| parent_floors.contains(f)) {
                        let parent_label = self.room_by_id(parent_id).map(|r| r.label.as_str()).unwrap_or("?");
                        let child_label = self.room_by_id(rid).map(|r| r.label.as_str()).unwrap_or("?");
                        errors.push(format!(
                            "Child '{}' is on a different floor than container '{}'",
                            child_label, parent_label
                        ));
                    }
                }
            }
        }

        errors
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Connection, ConnectionType, Room};

    #[test]
    fn test_dungeon_graph_new() {
        let graph = DungeonGraph::new();
        assert!(graph.rooms.is_empty());
        assert!(graph.connections.is_empty());
        assert!(graph.groups.is_empty());
        assert!(graph.graph_positions.is_empty());
    }

    #[test]
    fn test_add_room_and_find() {
        let mut graph = DungeonGraph::new();
        let room = Room::new("Room A".to_string());
        let id = room.id.clone();
        graph.add_room(room);

        assert_eq!(graph.rooms.len(), 1);
        let found = graph.room_by_id(&id);
        assert!(found.is_some());
        assert_eq!(found.unwrap().label, "Room A");
    }

    #[test]
    fn test_remove_room() {
        let mut graph = DungeonGraph::new();
        let r1 = Room::new("Room A".to_string());
        let r2 = Room::new("Room B".to_string());
        let id1 = r1.id.clone();
        let id2 = r2.id.clone();
        graph.add_room(r1);
        graph.add_room(r2);
        graph.add_connection(id1.clone(), id2.clone(), Connection::new(ConnectionType::Door));

        assert_eq!(graph.rooms.len(), 2);
        assert_eq!(graph.connections.len(), 1);

        graph.remove_room(&id1);

        assert_eq!(graph.rooms.len(), 1);
        assert!(graph.room_by_id(&id1).is_none());
        // Connection should also be removed
        assert!(graph.connections.is_empty());
    }

    #[test]
    fn test_add_and_remove_connection() {
        let mut graph = DungeonGraph::new();
        let r1 = Room::new("A".to_string());
        let r2 = Room::new("B".to_string());
        let id1 = r1.id.clone();
        let id2 = r2.id.clone();
        graph.add_room(r1);
        graph.add_room(r2);
        graph.add_connection(id1, id2, Connection::new(ConnectionType::Open));

        assert_eq!(graph.connections.len(), 1);
        let conn_id = graph.connections[0].connection.id.clone();

        graph.remove_connection(&conn_id);
        assert!(graph.connections.is_empty());
    }

    #[test]
    fn test_build_petgraph_empty() {
        let graph = DungeonGraph::new();
        let (pg, node_map) = graph.build_petgraph();
        assert_eq!(pg.node_count(), 0);
        assert_eq!(pg.edge_count(), 0);
        assert!(node_map.is_empty());
    }

    #[test]
    fn test_build_petgraph_three_rooms() {
        let mut graph = DungeonGraph::new();
        let r1 = Room::new("A".to_string());
        let r2 = Room::new("B".to_string());
        let r3 = Room::new("C".to_string());
        let id1 = r1.id.clone();
        let id2 = r2.id.clone();
        let id3 = r3.id.clone();
        graph.add_room(r1);
        graph.add_room(r2);
        graph.add_room(r3);
        graph.add_connection(id1.clone(), id2.clone(), Connection::new(ConnectionType::Door));
        graph.add_connection(id2.clone(), id3.clone(), Connection::new(ConnectionType::Open));

        let (pg, node_map) = graph.build_petgraph();
        assert_eq!(pg.node_count(), 3);
        assert_eq!(pg.edge_count(), 2);
        assert!(node_map.contains_key(&id1));
        assert!(node_map.contains_key(&id2));
        assert!(node_map.contains_key(&id3));
    }

    #[test]
    fn test_containment_helpers() {
        let mut graph = DungeonGraph::new();
        let parent = Room::new("Hall".to_string());
        let child1 = Room::new("Alcove A".to_string());
        let child2 = Room::new("Alcove B".to_string());
        let outside = Room::new("Corridor".to_string());
        let parent_id = parent.id.clone();
        let child1_id = child1.id.clone();
        let child2_id = child2.id.clone();
        let outside_id = outside.id.clone();

        graph.add_room(parent);
        graph.add_room(child1);
        graph.add_room(child2);
        graph.add_room(outside);

        let mut group = RoomGroup::new("Hall Contents".to_string());
        group.parent_room_id = Some(parent_id.clone());
        group.room_ids = vec![child1_id.clone(), child2_id.clone()];
        graph.groups.push(group);

        // children_of
        let children = graph.children_of(&parent_id);
        assert_eq!(children.len(), 2);
        assert!(children.contains(&child1_id.as_str()));
        assert!(children.contains(&child2_id.as_str()));

        // parent_of
        assert_eq!(graph.parent_of(&child1_id), Some(parent_id.as_str()));
        assert_eq!(graph.parent_of(&child2_id), Some(parent_id.as_str()));
        assert_eq!(graph.parent_of(&parent_id), None);
        assert_eq!(graph.parent_of(&outside_id), None);

        // nesting_depth
        assert_eq!(graph.nesting_depth(&parent_id), 0);
        assert_eq!(graph.nesting_depth(&child1_id), 1);
        assert_eq!(graph.nesting_depth(&outside_id), 0);

        // is_container
        assert!(graph.is_container(&parent_id));
        assert!(!graph.is_container(&child1_id));
        assert!(!graph.is_container(&outside_id));

        // containment_group
        assert!(graph.containment_group(&parent_id).is_some());
        assert!(graph.containment_group(&outside_id).is_none());
    }

    #[test]
    fn test_containment_validation() {
        let mut graph = DungeonGraph::new();
        let parent = Room::new("Hall".to_string());
        let child = Room::new("Alcove".to_string());
        let parent_id = parent.id.clone();
        let child_id = child.id.clone();
        graph.add_room(parent);
        graph.add_room(child);

        // Valid containment
        let mut group = RoomGroup::new("Contents".to_string());
        group.parent_room_id = Some(parent_id.clone());
        group.room_ids = vec![child_id.clone()];
        graph.groups.push(group);

        assert!(graph.validate_containment().is_empty());

        // Add child to second containment group -> duplicate error
        let mut group2 = RoomGroup::new("Dup".to_string());
        group2.parent_room_id = Some(parent_id.clone());
        group2.room_ids = vec![child_id.clone()];
        graph.groups.push(group2);

        let errors = graph.validate_containment();
        assert!(!errors.is_empty());
        assert!(errors[0].contains("multiple containment groups"));
    }

    #[test]
    fn test_nested_containment_depth() {
        let mut graph = DungeonGraph::new();
        let outer = Room::new("Outer".to_string());
        let middle = Room::new("Middle".to_string());
        let inner = Room::new("Inner".to_string());
        let outer_id = outer.id.clone();
        let middle_id = middle.id.clone();
        let inner_id = inner.id.clone();
        graph.add_room(outer);
        graph.add_room(middle);
        graph.add_room(inner);

        let mut g1 = RoomGroup::new("Outer->Middle".to_string());
        g1.parent_room_id = Some(outer_id.clone());
        g1.room_ids = vec![middle_id.clone()];
        graph.groups.push(g1);

        let mut g2 = RoomGroup::new("Middle->Inner".to_string());
        g2.parent_room_id = Some(middle_id.clone());
        g2.room_ids = vec![inner_id.clone()];
        graph.groups.push(g2);

        assert_eq!(graph.nesting_depth(&outer_id), 0);
        assert_eq!(graph.nesting_depth(&middle_id), 1);
        assert_eq!(graph.nesting_depth(&inner_id), 2);
    }
}
