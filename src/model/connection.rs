use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Connection {
    pub id: String,
    pub connection_type: ConnectionType,
    pub label: Option<String>,
    /// Corridor width in grid squares (default 2)
    #[serde(default = "default_corridor_width")]
    pub corridor_width: u32,
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
        }
    }
}
