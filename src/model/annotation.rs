use serde::{Deserialize, Serialize};

/// A user-created annotation / issue pinned to a location on the map.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Annotation {
    pub id: String,
    /// User-entered description of the issue.
    pub text: String,
    /// World-space X coordinate (pixels at default zoom).
    pub world_x: f32,
    /// World-space Y coordinate (pixels at default zoom).
    pub world_y: f32,
    /// Which view the annotation was created on ("Graph", "Spatial", "Encounters", "Styled", "Presentation").
    pub view: String,
    /// Nearest room ID at time of creation, if any.
    pub room_id: Option<String>,
    /// Whether this issue has been resolved.
    pub resolved: bool,
    /// ISO 8601 timestamp of creation.
    pub created_at: String,
}

impl Annotation {
    pub fn new(text: String, world_x: f32, world_y: f32, view: String, room_id: Option<String>) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            text,
            world_x,
            world_y,
            view,
            room_id,
            resolved: false,
            created_at: chrono_now(),
        }
    }
}

/// Simple timestamp without pulling in chrono — uses std SystemTime.
fn chrono_now() -> String {
    use std::time::SystemTime;
    let dur = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = dur.as_secs();
    // Format as a readable timestamp
    let days = secs / 86400;
    let time_of_day = secs % 86400;
    let hours = time_of_day / 3600;
    let minutes = (time_of_day % 3600) / 60;
    let seconds = time_of_day % 60;
    // Days since epoch → approximate date (good enough for sorting)
    // Use a simple epoch-relative format
    format!(
        "day-{} {:02}:{:02}:{:02} UTC",
        days, hours, minutes, seconds
    )
}
