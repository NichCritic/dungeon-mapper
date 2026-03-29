use serde::{Deserialize, Serialize};

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
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
pub enum RoomShape {
    Rectangle,
    Circle,
}

impl Default for RoomShape {
    fn default() -> Self {
        RoomShape::Rectangle
    }
}

impl RoomShape {
    pub const ALL: [RoomShape; 2] = [RoomShape::Rectangle, RoomShape::Circle];

    pub fn label(self) -> &'static str {
        match self {
            RoomShape::Rectangle => "Rectangle",
            RoomShape::Circle => "Circle",
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
