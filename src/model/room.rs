use serde::{Deserialize, Serialize};

/// Where a room or corridor sits relative to the floor being viewed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FloorRel {
    On,
    Below,
    Above,
}

/// Which floor(s) a room belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum FloorAssignment {
    /// Room exists on a single floor.
    Single(i32),
    /// Spans a range of floors, appearing on each: a room between two floors (a "half
    /// floor"), or a corridor or stairwell joining rooms on different floors. Either
    /// order; every floor from the lower to the higher counts.
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
        let (lo, hi) = self.range();
        lo <= floor && floor <= hi
    }

    /// The lowest and highest floor this is on.
    pub fn range(&self) -> (i32, i32) {
        match *self {
            FloorAssignment::Single(f) => (f, f),
            FloorAssignment::Half(a, b) => (a.min(b), a.max(b)),
        }
    }

    /// Returns all floors this room belongs to.
    /// True if the two assignments have a floor in common (like comparing `floors()`,
    /// without allocating; the layout solver asks this per candidate position).
    pub fn shares_floor(&self, other: &FloorAssignment) -> bool {
        let ((a0, a1), (b0, b1)) = (self.range(), other.range());
        a0 <= b1 && b0 <= a1
    }

    /// Where this sits relative to the floor being viewed.
    pub fn relation(&self, floor: i32) -> FloorRel {
        if self.visible_on(floor) {
            FloorRel::On
        } else if self.floors().iter().all(|f| *f < floor) {
            FloorRel::Below
        } else {
            FloorRel::Above
        }
    }

    /// The highest floor the room is on.
    pub fn max_floor(&self) -> i32 {
        self.range().1
    }

    /// Every floor this is on, lowest first.
    pub fn floors(&self) -> Vec<i32> {
        let (lo, hi) = self.range();
        (lo..=hi).collect()
    }

    pub fn label(&self) -> String {
        match *self {
            FloorAssignment::Single(f) => format!("Floor {}", f),
            FloorAssignment::Half(..) => {
                let (lo, hi) = self.range();
                if hi - lo == 1 { format!("Floor {}/{}", lo, hi) } else { format!("Floors {}–{}", lo, hi) }
            }
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Room {
    pub id: String,
    pub label: String,
    pub tags: Vec<RoomTag>,
    /// One-line preview of this room's note, derived from the note vault on load
    /// and save. Not authored here — the note itself is a Markdown file in
    /// `<save name>.notes/`. Kept on the room so map renderers can print it
    /// without reaching into the vault. Legacy saves store full note text here;
    /// the vault migrates that into a file the first time it sees it.
    #[serde(default, rename = "notes")]
    pub note_excerpt: String,
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
    /// Indoor/outdoor/covered environment type
    #[serde(default)]
    pub environment: RoomEnvironment,
    /// What bounds each side: wall, nothing, a fence, a drystone wall or a hedge.
    /// Only applies to Rectangle rooms. (Saved as `open_walls` before v11, when each
    /// side was just open or not.)
    #[serde(default, alias = "open_walls")]
    pub walls: Walls,
    /// Another map of the campaign this area is shown in detail on (by map id), e.g.
    /// a pen on a farm overview linking to the pen's battle map.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub links_to_map: Option<String>,
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

impl CaveData {
    /// Runs of floor cells, row by row over `rows`, for a cave `width` cells wide:
    /// (row, first column, one past the last column). Big caves have tens of thousands
    /// of cells, so they are drawn as runs rather than cell by cell.
    pub fn floor_runs(&self, width: usize, rows: std::ops::Range<usize>) -> impl Iterator<Item = (usize, usize, usize)> + '_ {
        rows.flat_map(move |ly| {
            let row = self.cells.get(ly * width..(ly + 1) * width).unwrap_or(&[]);
            let mut x = 0;
            std::iter::from_fn(move || {
                while x < row.len() && !row[x] {
                    x += 1;
                }
                if x >= row.len() {
                    return None;
                }
                let start = x;
                while x < row.len() && row[x] {
                    x += 1;
                }
                Some((ly, start, x))
            })
        })
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
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
    Clouds,
    Darkness,
    // Outdoors
    Tree,
    Bush,
    Boulder,
    HayBale,
    Trough,
    Cart,
    Tent,
    Campfire,
    Scarecrow,
    /// A free-standing run of fence (scale it to length); room sides use wall kinds.
    Fence,
}

impl DecorType {
    pub const ALL: [DecorType; 43] = [
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
        DecorType::Clouds, DecorType::Darkness,
        DecorType::Tree, DecorType::Bush, DecorType::Boulder, DecorType::HayBale,
        DecorType::Trough, DecorType::Cart, DecorType::Tent, DecorType::Campfire,
        DecorType::Scarecrow, DecorType::Fence,
    ];

    /// Cover an object of this type grants by default (DM can override per item).
    pub fn default_cover(self) -> super::cover::CoverKind {
        use super::cover::CoverKind as C;
        match self {
            DecorType::Pillar | DecorType::Statue | DecorType::Bookshelf | DecorType::Fireplace => C::Full,
            DecorType::Gate | DecorType::Altar | DecorType::Fountain | DecorType::Well
            | DecorType::Brazier | DecorType::Scales
            | DecorType::Tree | DecorType::Cart | DecorType::Tent => C::ThreeQuarters,
            DecorType::Table | DecorType::Chair | DecorType::Bench | DecorType::Chest
            | DecorType::Barrel | DecorType::Crate | DecorType::Throne | DecorType::Bed
            | DecorType::Rubble
            | DecorType::Bush | DecorType::Boulder | DecorType::HayBale | DecorType::Trough
            | DecorType::Fence => C::Half,
            _ => C::None,
        }
    }

    /// Half extents of the glyph in local units (1.0 = `DECOR_HALF_SIZE`), i.e. the
    /// footprint used for cover and light blocking before scale and rotation.
    pub fn local_extent(self) -> (f32, f32) {
        match self {
            DecorType::Table => (1.0, 0.6),
            DecorType::Chair => (0.4, 0.4),
            DecorType::Bench => (0.9, 0.25),
            DecorType::Chest => (0.65, 0.45),
            DecorType::Barrel => (0.55, 0.55),
            DecorType::Crate => (0.55, 0.55),
            DecorType::Pillar => (0.5, 0.5),
            DecorType::Altar => (0.98, 0.58),
            DecorType::Fountain => (0.92, 0.92),
            DecorType::Well => (0.72, 0.72),
            DecorType::Brazier => (0.78, 0.78),
            DecorType::Fireplace => (0.95, 0.65),
            DecorType::Statue => (0.72, 0.72),
            DecorType::Throne => (0.45, 0.6),
            DecorType::Bed => (0.66, 1.0),
            DecorType::Bookshelf => (0.85, 0.4),
            DecorType::Rubble => (0.9, 0.9),
            DecorType::Gate => (0.85, 0.75),
            DecorType::Scales => (0.85, 0.95),
            DecorType::Clouds => (0.9, 0.8),
            DecorType::Darkness => (0.8, 0.8),
            DecorType::Tree => (1.0, 1.0),
            DecorType::Bush => (0.7, 0.6),
            DecorType::Boulder => (0.75, 0.65),
            DecorType::HayBale => (0.75, 0.5),
            DecorType::Trough => (0.95, 0.35),
            DecorType::Cart => (0.95, 0.6),
            DecorType::Tent => (1.0, 0.9),
            DecorType::Campfire => (0.95, 0.95),
            DecorType::Scarecrow => (0.55, 0.55),
            DecorType::Fence => (1.0, 0.2),
            _ => (0.6, 0.6),
        }
    }

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
            DecorType::Clouds => "Clouds",
            DecorType::Darkness => "Darkness",
            DecorType::Tree => "Tree",
            DecorType::Bush => "Bush",
            DecorType::Boulder => "Boulder",
            DecorType::HayBale => "Hay Bale",
            DecorType::Trough => "Trough",
            DecorType::Cart => "Cart",
            DecorType::Tent => "Tent",
            DecorType::Campfire => "Campfire",
            DecorType::Scarecrow => "Scarecrow",
            DecorType::Fence => "Fence",
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
    /// Cover override; `None` means the type's default.
    #[serde(default)]
    pub cover: Option<super::cover::CoverKind>,
    /// Light this item gives: its type's default, off, or its own.
    #[serde(default)]
    pub light: super::decor_light::DecorLightSetting,
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
    #[serde(default)]
    cover: Option<super::cover::CoverKind>,
    #[serde(default)]
    light: super::decor_light::DecorLightSetting,
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
            cover: raw.cover,
            light: raw.light,
        })
    }
}

impl RoomDecor {
    /// Effective cover this object grants.
    pub fn cover_kind(&self) -> super::cover::CoverKind {
        self.cover.unwrap_or_else(|| self.decor_type.default_cover())
    }

    pub fn new(decor_type: DecorType, x: f32, y: f32) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            decor_type,
            x,
            y,
            rotation: 0.0,
            scale_x: 1.0,
            scale_y: 1.0,
            cover: None,
            light: Default::default(),
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
    /// Drawn with an irregular natural outline inside its box (a copse, a bog),
    /// instead of a rectangle. The outline is derived from the id, so it's stable.
    #[serde(default)]
    pub organic: bool,
}

fn default_section_height() -> f32 { 5.0 }

impl ElevationSection {
    /// The section's outline in room-local grid units: its box, or for an organic
    /// section an irregular blob filling most of the box. The blob's wobble comes
    /// from the section's id, so it's the same every time it's drawn.
    pub fn outline(&self) -> Vec<(f32, f32)> {
        let (x0, y0, x1, y1) = (self.x, self.y, self.x + self.width, self.y + self.length);
        if !self.organic {
            return vec![(x0, y0), (x1, y0), (x1, y1), (x0, y1)];
        }
        // FNV-1a: stable across runs and Rust versions, unlike the std hasher
        let seed = self.id.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ b as u64).wrapping_mul(0x100_0000_01b3));
        let phase = |shift: u32| ((seed >> shift) & 0xffff) as f32 / 65535.0 * std::f32::consts::TAU;
        let (p1, p2, p3) = (phase(0), phase(16), phase(32));
        let (cx, cy, rx, ry) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0, self.width / 2.0, self.length / 2.0);
        const POINTS: usize = 32;
        (0..POINTS)
            .map(|i| {
                let t = i as f32 / POINTS as f32 * std::f32::consts::TAU;
                let r = 0.84 + 0.08 * (2.0 * t + p1).sin() + 0.05 * (3.0 * t + p2).sin() + 0.03 * (5.0 * t + p3).sin();
                (cx + rx * r * t.cos(), cy + ry * r * t.sin())
            })
            .collect()
    }

    pub fn new(elevation: ElevationType, x: f32, y: f32, width: f32, length: f32) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            x, y, width, length,
            elevation,
            height: 5.0,
            opaque: false,
            organic: false,
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
    /// Rough ground: mud, scree, ploughed field. Difficult terrain.
    Rough,
    /// Tall grass or crops: lightly obscures.
    TallGrass,
    /// Thicket, bracken or dense woods: heavily obscures, difficult terrain.
    Thicket,
}

impl ElevationType {
    pub const ALL: [ElevationType; 10] = [
        ElevationType::Raised,
        ElevationType::Lowered,
        ElevationType::Steps,
        ElevationType::Slope,
        ElevationType::BottomlessPit,
        ElevationType::Hole,
        ElevationType::Water,
        ElevationType::Rough,
        ElevationType::TallGrass,
        ElevationType::Thicket,
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
            ElevationType::Rough => "Rough ground",
            ElevationType::TallGrass => "Tall grass",
            ElevationType::Thicket => "Thicket",
        }
    }

    /// Ground cover rather than a change of height: rough ground, tall grass, thicket.
    pub fn is_terrain(self) -> bool {
        matches!(self, ElevationType::Rough | ElevationType::TallGrass | ElevationType::Thicket)
    }

    /// Too dense to see through (treated as blocking sight at its edge).
    pub fn blocks_sight(self) -> bool {
        self == ElevationType::Thicket
    }
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq)]
pub enum RoomEnvironment {
    #[default]
    Indoor,
    Outdoor,
    Covered,
}

impl RoomEnvironment {
    pub const ALL: [RoomEnvironment; 3] = [
        RoomEnvironment::Indoor,
        RoomEnvironment::Outdoor,
        RoomEnvironment::Covered,
    ];

    pub fn label(self) -> &'static str {
        match self {
            RoomEnvironment::Indoor => "Indoor",
            RoomEnvironment::Outdoor => "Outdoor",
            RoomEnvironment::Covered => "Covered",
        }
    }
}

/// What bounds one side of a rectangular room.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize)]
pub enum WallKind {
    /// A full wall: blocks movement and sight.
    #[default]
    Wall,
    /// Nothing: the side is open.
    Open,
    /// A wooden fence: blocks movement, but you can see (and shoot) over it.
    Fence,
    /// A low drystone wall: like a fence, but stone.
    StoneWall,
    /// A hedge: blocks movement and sight.
    Hedge,
}

impl WallKind {
    pub const ALL: [WallKind; 5] = [WallKind::Wall, WallKind::Open, WallKind::Fence, WallKind::StoneWall, WallKind::Hedge];

    pub fn label(self) -> &'static str {
        match self {
            WallKind::Wall => "Wall",
            WallKind::Open => "Open",
            WallKind::Fence => "Fence",
            WallKind::StoneWall => "Drystone wall",
            WallKind::Hedge => "Hedge",
        }
    }

    /// Sight stops at it: full walls and hedges.
    pub fn blocks_sight(self) -> bool {
        matches!(self, WallKind::Wall | WallKind::Hedge)
    }

    /// A low boundary you can see over, giving half cover to whoever is behind it.
    pub fn is_low(self) -> bool {
        matches!(self, WallKind::Fence | WallKind::StoneWall)
    }
}

/// Saves before v11 stored each side as open (`true`) or walled (`false`).
impl<'de> Deserialize<'de> for WallKind {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        enum Kind { Wall, Open, Fence, StoneWall, Hedge }
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw { Open(bool), Kind(Kind) }
        Ok(match Raw::deserialize(deserializer)? {
            Raw::Open(true) => WallKind::Open,
            Raw::Open(false) => WallKind::Wall,
            Raw::Kind(Kind::Wall) => WallKind::Wall,
            Raw::Kind(Kind::Open) => WallKind::Open,
            Raw::Kind(Kind::Fence) => WallKind::Fence,
            Raw::Kind(Kind::StoneWall) => WallKind::StoneWall,
            Raw::Kind(Kind::Hedge) => WallKind::Hedge,
        })
    }
}

/// What bounds each side of a rectangular room.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct Walls {
    #[serde(default)]
    pub north: WallKind,
    #[serde(default)]
    pub south: WallKind,
    #[serde(default)]
    pub east: WallKind,
    #[serde(default)]
    pub west: WallKind,
}

impl Walls {
    /// Every side the same.
    pub fn all(kind: WallKind) -> Self {
        Self { north: kind, south: kind, east: kind, west: kind }
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
            SizeHint::Small => "Small (3x3)",
            SizeHint::Medium => "Medium (4x4)",
            SizeHint::Large => "Large (6x6)",
            SizeHint::Huge => "Huge (8x8)",
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
            note_excerpt: String::new(),
            size_hint: SizeHint::Medium,
            grid_width: None,
            grid_height: None,
            shape: RoomShape::default(),
            allow_rotation: false,
            decor: Vec::new(),
            cave_data: None,
            floor: FloorAssignment::default(),
            sections: Vec::new(),
            environment: RoomEnvironment::default(),
            walls: Walls::default(),
            links_to_map: None,
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
    #[test]
    fn organic_outlines_are_stable_and_stay_in_their_box() {
        let mut sec = ElevationSection::new(ElevationType::Thicket, 2.0, 1.0, 4.0, 3.0);
        assert_eq!(sec.outline(), vec![(2.0, 1.0), (6.0, 1.0), (6.0, 4.0), (2.0, 4.0)]);
        sec.organic = true;
        let a = sec.outline();
        assert_eq!(a, sec.outline(), "the same every time it's drawn");
        assert!(a.iter().all(|&(x, y)| (2.0..=6.0).contains(&x) && (1.0..=4.0).contains(&y)));
        let mut other = sec.clone();
        other.id = "another".into();
        assert_ne!(a, other.outline(), "each section has its own shape");
    }

    use super::*;

    #[test]
    fn a_span_between_floors_is_on_every_floor_between() {
        let stair = FloorAssignment::Half(2, 0);
        assert!(stair.visible_on(0) && stair.visible_on(1) && stair.visible_on(2) && !stair.visible_on(3));
        assert_eq!(stair.floors(), vec![0, 1, 2]);
        assert!(stair.shares_floor(&FloorAssignment::Single(1)), "a floor-1 room is in the stairwell's way");
        assert!(!stair.shares_floor(&FloorAssignment::Single(3)));
        assert_eq!(stair.relation(3), FloorRel::Below);
        assert_eq!(stair.label(), "Floors 0–2");
        assert_eq!(FloorAssignment::Half(1, 2).label(), "Floor 1/2");
    }

    #[test]
    fn floor_runs_merge_neighbouring_floor_cells() {
        // 4 wide, rows: floor floor rock floor / all rock / all floor
        let cave = CaveData {
            cells: vec![true, true, false, true, false, false, false, false, true, true, true, true],
            seed: 0,
            algorithm: CaveAlgorithm::CellularAutomata,
            density: 0.45,
            smoothing_iterations: 4,
            generation: 0,
            contour_segments: Vec::new(),
        };
        let runs: Vec<_> = cave.floor_runs(4, 0..3).collect();
        assert_eq!(runs, vec![(0, 0, 2), (0, 3, 4), (2, 0, 4)]);
        assert_eq!(cave.floor_runs(4, 1..2).count(), 0);
    }

    #[test]
    fn test_room_new() {
        let room = Room::new("Test Room".to_string());
        // Should have a UUID id (36 chars with hyphens)
        assert_eq!(room.id.len(), 36);
        assert_eq!(room.label, "Test Room");
        assert_eq!(room.size_hint, SizeHint::Medium);
        assert!(room.cave_data.is_none());
        assert!(room.tags.is_empty());
        assert!(room.note_excerpt.is_empty());
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
