use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Connection {
    pub id: String,
    pub connection_type: ConnectionType,
    pub label: Option<String>,
    /// Corridor width in grid squares (default 2)
    #[serde(default = "default_corridor_width")]
    pub corridor_width: u32,
    /// Double door (2 squares wide instead of 1)
    #[serde(default)]
    pub double_door: bool,
    /// Minimum corridor length in Manhattan distance (grid squares). None = unconstrained.
    #[serde(default)]
    pub min_length: Option<u32>,
    /// Maximum corridor length in Manhattan distance (grid squares). None = unconstrained.
    #[serde(default)]
    pub max_length: Option<u32>,
}

fn default_corridor_width() -> u32 {
    2
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
pub enum ConnectionType {
    Open,
    Door,
    Locked,
    Secret,
    OneWay,
}

impl ConnectionType {
    pub fn label(self) -> &'static str {
        match self {
            ConnectionType::Open => "Open",
            ConnectionType::Door => "Door",
            ConnectionType::Locked => "Locked",
            ConnectionType::Secret => "Secret",
            ConnectionType::OneWay => "One-Way",
        }
    }

    pub const ALL: [ConnectionType; 5] = [
        ConnectionType::Open,
        ConnectionType::Door,
        ConnectionType::Locked,
        ConnectionType::Secret,
        ConnectionType::OneWay,
    ];
}

impl Connection {
    pub fn new(connection_type: ConnectionType) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            connection_type,
            label: None,
            corridor_width: 2,
            double_door: false,
            min_length: None,
            max_length: None,
        }
    }

    /// Door width in grid squares (1 for single, 2 for double).
    pub fn door_width(&self) -> u32 {
        if self.double_door { 2 } else { 1 }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_connection_new() {
        let conn = Connection::new(ConnectionType::Door);
        assert_eq!(conn.id.len(), 36); // UUID
        assert_eq!(conn.connection_type, ConnectionType::Door);
        assert_eq!(conn.label, None);
        assert_eq!(conn.corridor_width, 2);
        assert!(!conn.double_door);
        assert_eq!(conn.min_length, None);
        assert_eq!(conn.max_length, None);
    }

    #[test]
    fn test_connection_door_width_single() {
        let conn = Connection::new(ConnectionType::Door);
        assert_eq!(conn.door_width(), 1);
    }

    #[test]
    fn test_connection_door_width_double() {
        let mut conn = Connection::new(ConnectionType::Door);
        conn.double_door = true;
        assert_eq!(conn.door_width(), 2);
    }

    #[test]
    fn test_connection_type_label() {
        assert_eq!(ConnectionType::Open.label(), "Open");
        assert_eq!(ConnectionType::Door.label(), "Door");
        assert_eq!(ConnectionType::Locked.label(), "Locked");
        assert_eq!(ConnectionType::Secret.label(), "Secret");
        assert_eq!(ConnectionType::OneWay.label(), "One-Way");
    }

    #[test]
    fn test_connection_type_all() {
        assert_eq!(ConnectionType::ALL.len(), 5);
    }
}
