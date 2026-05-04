use serde::{Deserialize, Serialize};

/// Which floor(s) a room belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum FloorAssignment {
    /// Room exists on a single floor.
    Single(i32),
    /// Room spans two adjacent floors (a "half floor"), appearing on both.
    Half(i32, i32),
}

impl Default for FloorAssignment {
    fn default() -> Self {
        FloorAssignment::Single(0)
    }
}

impl FloorAssignment {
    /// Returns true if this room should be visible on the given floor.
    pub fn visible_on(&self, floor: i32) -> bool {
        match self {
            FloorAssignment::Single(f) => *f == floor,
            FloorAssignment::Half(a, b) => *a == floor || *b == floor,
        }
    }

    /// Returns all floors this room belongs to.
    pub fn floors(&self) -> Vec<i32> {
        match self {
            FloorAssignment::Single(f) => vec![*f],
            FloorAssignment::Half(a, b) => vec![*a, *b],
        }
    }

    pub fn label(&self) -> String {
        match self {
            FloorAssignment::Single(f) => format!("Floor {}", f),
            FloorAssignment::Half(a, b) => format!("Floor {}/{}", a, b),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Room {
    pub id: String,
    pub label: String,
    pub tags: Vec<RoomTag>,
    pub notes: String,
    pub size_hint: SizeHint,
    /// Explicit grid width (overrides size_hint if set)
    #[serde(default)]
    pub grid_width: Option<u32>,
    /// Explicit grid height (overrides size_hint if set)
    #[serde(default)]
    pub grid_height: Option<u32>,
    /// Room shape
    #[serde(default)]
    pub shape: RoomShape,
    /// Whether the layout solver may swap width and height
    #[serde(default)]
    pub allow_rotation: bool,
    /// Decorative elements placed inside the room
    #[serde(default)]
    pub decor: Vec<RoomDecor>,
    /// Cave generation data (only used when shape == Cave)
    #[serde(default)]
    pub cave_data: Option<CaveData>,
    /// Which floor(s) this room belongs to
    #[serde(default)]
    pub floor: FloorAssignment,
    /// Raised/lowered sub-regions within this room.
    #[serde(default)]
    pub sections: Vec<ElevationSection>,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq)]
pub enum CaveAlgorithm {
    #[default]
    CellularAutomata,
    DrunkardsWalk,
}

impl CaveAlgorithm {
    pub const ALL: [CaveAlgorithm; 2] = [
        CaveAlgorithm::CellularAutomata,
        CaveAlgorithm::DrunkardsWalk,
    ];

    pub fn label(self) -> &'static str {
        match self {
            CaveAlgorithm::CellularAutomata => "Cellular Automata",
            CaveAlgorithm::DrunkardsWalk => "Drunkard's Walk",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CaveData {
    /// Per-cell floor mask. Index = y * width + x. true = floor, false = wall.
    /// Empty vec means "needs generation".
    pub cells: Vec<bool>,
    pub seed: u64,
    pub algorithm: CaveAlgorithm,
    /// Initial fill density (0.0–1.0)
    pub density: f32,
    /// Smoothing iterations (cellular automata)
    pub smoothing_iterations: u32,
    /// Incremented on each edit/regeneration for cache invalidation
    #[serde(default)]
    pub generation: u32,
    /// Precomputed marching squares contour segments in world pixel coords (x1, y1, x2, y2).
    /// Computed from the global floor set so adjacent caves/corridors merge seamlessly.
    #[serde(skip)]
    pub contour_segments: Vec<(f32, f32, f32, f32)>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
pub enum DecorType {
    Table,
    Chair,
    Bench,
    Chest,
    Barrel,
    Crate,
    Pillar,
    StairsUp,
    StairsDown,
    Ladder,
    Altar,
    Fountain,
    Well,
    Brazier,
    Fireplace,
    Statue,
    Throne,
    Bed,
    Bookshelf,
    Trap,
    Rubble,
    Bones,
    Web,
    Door,
    Gate,
    OfferingMouth,
    Vines,
    Scales,
    Crack,
    Stream,
    Pool,
}

impl DecorType {
    pub const ALL: [DecorType; 31] = [
        DecorType::Table, DecorType::Chair, DecorType::Bench,
        DecorType::Chest, DecorType::Barrel, DecorType::Crate,
        DecorType::Pillar, DecorType::StairsUp, DecorType::StairsDown,
        DecorType::Ladder, DecorType::Altar, DecorType::Fountain,
        DecorType::Well, DecorType::Brazier, DecorType::Fireplace,
        DecorType::Statue, DecorType::Throne, DecorType::Bed,
        DecorType::Bookshelf, DecorType::Trap, DecorType::Rubble,
        DecorType::Bones, DecorType::Web, DecorType::Door, DecorType::Gate,
        DecorType::OfferingMouth, DecorType::Vines, DecorType::Scales,
        DecorType::Crack, DecorType::Stream, DecorType::Pool,
    ];

    pub fn label(self) -> &'static str {
        match self {
            DecorType::Table => "Table",
            DecorType::Chair => "Chair",
            DecorType::Bench => "Bench",
            DecorType::Chest => "Chest",
            DecorType::Barrel => "Barrel",
            DecorType::Crate => "Crate",
            DecorType::Pillar => "Pillar",
            DecorType::StairsUp => "Stairs Up",
            DecorType::StairsDown => "Stairs Down",
            DecorType::Ladder => "Ladder",
            DecorType::Altar => "Altar",
            DecorType::Fountain => "Fountain",
            DecorType::Well => "Well",
            DecorType::Brazier => "Brazier",
            DecorType::Fireplace => "Fireplace",
            DecorType::Statue => "Statue",
            DecorType::Throne => "Throne",
            DecorType::Bed => "Bed",
            DecorType::Bookshelf => "Bookshelf",
            DecorType::Trap => "Trap",
            DecorType::Rubble => "Rubble",
            DecorType::Bones => "Bones",
            DecorType::Web => "Web",
            DecorType::Door => "Door",
            DecorType::Gate => "Gate",
            DecorType::OfferingMouth => "Offering Mouth",
            DecorType::Vines => "Vines",
            DecorType::Scales => "Scales",
            DecorType::Crack => "Crack",
            DecorType::Stream => "Stream",
            DecorType::Pool => "Pool",
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct RoomDecor {
    pub id: String,
    pub decor_type: DecorType,
    /// Position relative to room top-left, in grid units
    pub x: f32,
    pub y: f32,
    /// Rotation in degrees
    #[serde(default)]
    pub rotation: f32,
    /// Horizontal scale multiplier (1.0 = default size)
    pub scale_x: f32,
    /// Vertical scale multiplier (1.0 = default size)
    pub scale_y: f32,
}

/// Raw shape for backward-compatible deserialization (handles legacy `scale` field).
#[derive(Deserialize)]
struct RoomDecorRaw {
    id: String,
    decor_type: DecorType,
    x: f32,
    y: f32,
    #[serde(default)]
    rotation: f32,
    #[serde(default = "default_one")]
    scale: f32,
    scale_x: Option<f32>,
    scale_y: Option<f32>,
}

fn default_one() -> f32 { 1.0 }

impl<'de> Deserialize<'de> for RoomDecor {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = RoomDecorRaw::deserialize(deserializer)?;
        Ok(Self {
            id: raw.id,
            decor_type: raw.decor_type,
            x: raw.x,
            y: raw.y,
            rotation: raw.rotation,
            scale_x: raw.scale_x.unwrap_or(raw.scale),
            scale_y: raw.scale_y.unwrap_or(raw.scale),
        })
    }
}

impl RoomDecor {
    pub fn new(decor_type: DecorType, x: f32, y: f32) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            decor_type,
            x,
            y,
            rotation: 0.0,
            scale_x: 1.0,
            scale_y: 1.0,
        }
    }
}

/// A raised or lowered sub-region within a room.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ElevationSection {
    pub id: String,
    /// Position relative to room top-left, in grid units.
    pub x: f32,
    pub y: f32,
    /// Width in grid units (east-west).
    pub width: f32,
    /// Length in grid units (north-south).
    pub length: f32,
    /// Elevation type.
    pub elevation: ElevationType,
    /// Height of the elevation change in feet (e.g. 5ft raised platform, 10ft pit).
    #[serde(default = "default_section_height")]
    pub height: f32,
    /// When true, the section fill is fully opaque (hides floor beneath).
    #[serde(default)]
    pub opaque: bool,
}

fn default_section_height() -> f32 { 5.0 }

impl ElevationSection {
    pub fn new(elevation: ElevationType, x: f32, y: f32, width: f32, length: f32) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            x, y, width, length,
            elevation,
            height: 5.0,
            opaque: false,
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
pub enum ElevationType {
    /// Raised platform (e.g. dais, stage)
    Raised,
    /// Lowered pit (e.g. sunken area, pool)
    Lowered,
    /// Steps connecting levels
    Steps,
    /// Gradual slope (like steps but smooth/flat)
    Slope,
    /// Bottomless pit (no floor)
    BottomlessPit,
    /// Hole down to the floor below
    Hole,
    /// Water (pool, stream, flooded area)
    Water,
}

impl ElevationType {
    pub const ALL: [ElevationType; 7] = [
        ElevationType::Raised,
        ElevationType::Lowered,
        ElevationType::Steps,
        ElevationType::Slope,
        ElevationType::BottomlessPit,
        ElevationType::Hole,
        ElevationType::Water,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ElevationType::Raised => "Raised",
            ElevationType::Lowered => "Lowered",
            ElevationType::Steps => "Steps",
            ElevationType::Slope => "Slope",
            ElevationType::BottomlessPit => "Bottomless Pit",
            ElevationType::Hole => "Hole",
            ElevationType::Water => "Water",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq)]
pub enum RoomShape {
    #[default]
    Rectangle,
    Circle,
    Cave,
}

impl RoomShape {
    pub const ALL: [RoomShape; 3] = [RoomShape::Rectangle, RoomShape::Circle, RoomShape::Cave];

    pub fn label(self) -> &'static str {
        match self {
            RoomShape::Rectangle => "Rectangle",
            RoomShape::Circle => "Circle",
            RoomShape::Cave => "Cave",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum RoomTag {
    Entrance,
    Boss,
    Trap,
    Treasure,
    Optional,
    Secret,
    Rest,
    Custom(String),
}

impl RoomTag {
    pub fn color(&self) -> egui::Color32 {
        match self {
            RoomTag::Entrance => egui::Color32::from_rgb(80, 200, 80),
            RoomTag::Boss => egui::Color32::from_rgb(220, 60, 60),
            RoomTag::Trap => egui::Color32::from_rgb(230, 150, 30),
            RoomTag::Treasure => egui::Color32::from_rgb(230, 210, 50),
            RoomTag::Optional => egui::Color32::from_rgb(150, 150, 150),
            RoomTag::Secret => egui::Color32::from_rgb(160, 80, 200),
            RoomTag::Rest => egui::Color32::from_rgb(80, 130, 220),
            RoomTag::Custom(_) => egui::Color32::from_rgb(200, 200, 200),
        }
    }

    pub fn label(&self) -> &str {
        match self {
            RoomTag::Entrance => "Entrance",
            RoomTag::Boss => "Boss",
            RoomTag::Trap => "Trap",
            RoomTag::Treasure => "Treasure",
            RoomTag::Optional => "Optional",
            RoomTag::Secret => "Secret",
            RoomTag::Rest => "Rest",
            RoomTag::Custom(s) => s.as_str(),
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
pub enum SizeHint {
    Small,
    Medium,
    Large,
    Huge,
}

impl SizeHint {
    pub fn grid_size(self) -> (u32, u32) {
        match self {
            SizeHint::Small => (3, 3),
            SizeHint::Medium => (4, 4),
            SizeHint::Large => (6, 6),
            SizeHint::Huge => (8, 8),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            SizeHint::Small => "Small (15x15 ft)",
            SizeHint::Medium => "Medium (20x20 ft)",
            SizeHint::Large => "Large (30x30 ft)",
            SizeHint::Huge => "Huge (40x40 ft)",
        }
    }

    pub const ALL: [SizeHint; 4] = [
        SizeHint::Small,
        SizeHint::Medium,
        SizeHint::Large,
        SizeHint::Huge,
    ];
}

impl Room {
    pub fn new(label: String) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            label,
            tags: Vec::new(),
            notes: String::new(),
            size_hint: SizeHint::Medium,
            grid_width: None,
            grid_height: None,
            shape: RoomShape::default(),
            allow_rotation: false,
            decor: Vec::new(),
            cave_data: None,
            floor: FloorAssignment::default(),
            sections: Vec::new(),
        }
    }

    /// Effective grid dimensions (explicit overrides size_hint).
    pub fn grid_size(&self) -> (u32, u32) {
        let (hint_w, hint_h) = self.size_hint.grid_size();
        (
            self.grid_width.unwrap_or(hint_w),
            self.grid_height.unwrap_or(hint_h),
        )
    }

    pub fn primary_color(&self) -> egui::Color32 {
        self.tags
            .first()
            .map(|t| t.color())
            .unwrap_or(egui::Color32::from_rgb(200, 200, 200))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_room_new() {
        let room = Room::new("Test Room".to_string());
        // Should have a UUID id (36 chars with hyphens)
        assert_eq!(room.id.len(), 36);
        assert_eq!(room.label, "Test Room");
        assert_eq!(room.size_hint, SizeHint::Medium);
        assert!(room.cave_data.is_none());
        assert!(room.tags.is_empty());
        assert!(room.notes.is_empty());
        assert_eq!(room.grid_width, None);
        assert_eq!(room.grid_height, None);
        assert_eq!(room.shape, RoomShape::default());
        assert!(!room.allow_rotation);
        assert!(room.decor.is_empty());
    }

    #[test]
    fn test_room_grid_size_default() {
        let room = Room::new("Test".to_string());
        // Medium default is (4, 4)
        assert_eq!(room.grid_size(), (4, 4));
    }

    #[test]
    fn test_room_grid_size_overrides() {
        let mut room = Room::new("Test".to_string());
        room.grid_width = Some(10);
        room.grid_height = Some(12);
        assert_eq!(room.grid_size(), (10, 12));
    }

    #[test]
    fn test_room_grid_size_partial_override() {
        let mut room = Room::new("Test".to_string());
        room.grid_width = Some(10);
        // height still uses size_hint
        assert_eq!(room.grid_size(), (10, 4));
    }

    #[test]
    fn test_size_hint_grid_size() {
        assert_eq!(SizeHint::Small.grid_size(), (3, 3));
        assert_eq!(SizeHint::Medium.grid_size(), (4, 4));
        assert_eq!(SizeHint::Large.grid_size(), (6, 6));
        assert_eq!(SizeHint::Huge.grid_size(), (8, 8));
    }

    #[test]
    fn test_floor_assignment_visible_on() {
        let single = FloorAssignment::Single(0);
        assert!(single.visible_on(0));
        assert!(!single.visible_on(1));

        let half = FloorAssignment::Half(0, 1);
        assert!(half.visible_on(0));
        assert!(half.visible_on(1));
        assert!(!half.visible_on(2));
    }

    #[test]
    fn test_floor_assignment_floors() {
        assert_eq!(FloorAssignment::Single(0).floors(), vec![0]);
        assert_eq!(FloorAssignment::Half(0, 1).floors(), vec![0, 1]);
    }

    #[test]
    fn test_floor_assignment_label() {
        assert_eq!(FloorAssignment::Single(0).label(), "Floor 0");
        assert_eq!(FloorAssignment::Half(0, 1).label(), "Floor 0/1");
    }
}
