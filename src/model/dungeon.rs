use std::collections::{HashMap, HashSet};
use serde::{Deserialize, Serialize};

use super::token::{MapToken, TokenKind};

use super::{Annotation, CustomMonster, DungeonGraph, Encounter, PlayerCharacter, SpatialLayout, Theme};

/// A light source placed in a room.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LightSource {
    pub id: String,
    pub room_id: String,
    /// Bright light radius in grid cells.
    pub radius: f32,
    pub intensity: f32,
    pub color: [u8; 3],
    /// Absolute position (grid units). `None` = center of `room_id`.
    #[serde(default)]
    pub pos: Option<(f32, f32)>,
    /// Dim light radius in cells; `None` = twice `radius`.
    #[serde(default)]
    pub dim_radius: Option<f32>,
    /// Token carrying this light; overrides `pos`/`room_id` while the token exists.
    #[serde(default)]
    pub carrier: Option<TokenKind>,
}

impl LightSource {
    pub fn dim_radius(&self) -> f32 {
        self.dim_radius.unwrap_or(self.radius * 2.0)
    }

    /// Feed everything that affects how this light renders into a cache fingerprint.
    pub fn hash_into(&self, h: &mut impl std::hash::Hasher) {
        use std::hash::Hash;
        self.id.hash(h);
        self.room_id.hash(h);
        self.radius.to_bits().hash(h);
        self.dim_radius().to_bits().hash(h);
        self.intensity.to_bits().hash(h);
        self.color.hash(h);
        self.pos.map(|(x, y)| (x.to_bits(), y.to_bits())).hash(h);
        self.carrier.hash(h);
    }
}

/// Persisted session state — runtime presentation data that survives between sessions.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionState {
    /// Per-room visibility: "hidden", "explored", or "visible".
    #[serde(default)]
    pub room_visibility: HashMap<String, String>,
    /// Open door connection IDs.
    #[serde(default)]
    pub doors_open: HashSet<String>,
    /// Current positions of encounters: encounter_id -> room_id.
    #[serde(default)]
    pub encounter_positions: HashMap<String, String>,
    /// Encounter IDs that have been fully defeated.
    #[serde(default)]
    pub defeated_encounters: HashSet<String>,
    /// Per-monster current HP: key is "encounter_id/monster_idx/instance", value is current HP.
    #[serde(default)]
    pub encounter_hp: HashMap<String, i32>,
    /// Which room the party token is in (None = not placed).
    #[serde(default)]
    pub party_room: Option<String>,
    /// Whether autobattle is enabled.
    #[serde(default)]
    pub autobattle: bool,
    /// Player-window sharing toggles and the DM light shading preference.
    #[serde(default = "default_true")]
    pub show_light_player: bool,
    #[serde(default)]
    pub show_vision_player: bool,
    #[serde(default)]
    pub show_cover_player: bool,
    #[serde(default)]
    pub dm_show_light: bool,
    /// Line-of-sight lighting (per-cell light map with walls). Off = legacy room-based wash.
    #[serde(default)]
    pub los_lighting: bool,
    /// The floor being viewed and played on (`None` = all floors), shared by every view.
    #[serde(default)]
    pub current_floor: Option<i32>,
    /// Show players faint outlines of the rooms they've seen on the floors below.
    #[serde(default = "default_true")]
    pub ghost_lower_floors: bool,
}

fn default_true() -> bool { true }

impl Default for SessionState {
    fn default() -> Self {
        Self {
            room_visibility: HashMap::new(),
            doors_open: HashSet::new(),
            encounter_positions: HashMap::new(),
            defeated_encounters: HashSet::new(),
            encounter_hp: HashMap::new(),
            party_room: None,
            autobattle: false,
            show_light_player: true,
            show_vision_player: false,
            show_cover_player: false,
            dm_show_light: false,
            los_lighting: false,
            current_floor: None,
            ghost_lower_floors: true,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Dungeon {
    /// Stable identity for the map, used to bind session notes to it across renames.
    #[serde(default = "new_map_id")]
    pub id: String,
    pub name: String,
    pub graph: DungeonGraph,
    pub layout: Option<SpatialLayout>,
    #[serde(default)]
    pub theme: Theme,
    #[serde(default)]
    pub encounters: Vec<Encounter>,
    /// User-created or cloned custom monsters, saved with the dungeon.
    #[serde(default)]
    pub custom_monsters: Vec<CustomMonster>,
    /// Player characters in the party.
    #[serde(default)]
    pub party: Vec<PlayerCharacter>,
    /// Issue annotations pinned to map locations.
    #[serde(default)]
    pub annotations: Vec<Annotation>,
    /// Placeable light sources for the player view.
    #[serde(default)]
    pub light_sources: Vec<LightSource>,
    /// This map's own decor-lighting settings; `None` follows the campaign's.
    #[serde(default)]
    pub decor_lighting: Option<super::DecorLighting>,
    /// The campaign's decor-lighting settings, copied into the working map by the app
    /// (like the party) so edits to them count for undo and autosave. `None` in the
    /// maps stored on the campaign, so they're saved once, on the campaign.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub campaign_decor_lighting: Option<super::DecorLighting>,
    /// Feet per grid square: 5 for a battle map, more for an overview map.
    #[serde(default = "default_feet_per_square")]
    pub feet_per_square: u32,
    /// Ambient light level (0.0 = dark, 1.0 = fully lit).
    #[serde(default)]
    pub ambient_light: f32,
    /// Area-of-effect markers placed on the map.
    #[serde(default)]
    pub aoe_markers: Vec<crate::presentation::aoe::AoEMarker>,
    /// Creature tokens placed on the map (one per monster instance / player character).
    #[serde(default)]
    pub tokens: Vec<MapToken>,
    /// Persisted session state (fog of war, encounter positions, HP, etc.).
    #[serde(default)]
    pub session: SessionState,
}

fn default_feet_per_square() -> u32 {
    5
}

fn new_map_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

impl Dungeon {
    pub fn new(name: String) -> Self {
        Self {
            id: new_map_id(),
            name,
            graph: DungeonGraph::new(),
            layout: None,
            theme: Theme::default(),
            encounters: Vec::new(),
            custom_monsters: Vec::new(),
            party: Vec::new(),
            annotations: Vec::new(),
            light_sources: Vec::new(),
            decor_lighting: None,
            campaign_decor_lighting: None,
            feet_per_square: default_feet_per_square(),
            ambient_light: 0.0,
            aoe_markers: Vec::new(),
            tokens: Vec::new(),
            session: SessionState::default(),
        }
    }

    /// The decor-lighting settings in force on this map: its own, else the campaign's,
    /// else the built-in default.
    pub fn effective_decor_lighting(&self) -> std::borrow::Cow<'_, super::DecorLighting> {
        match self.decor_lighting.as_ref().or(self.campaign_decor_lighting.as_ref()) {
            Some(l) => std::borrow::Cow::Borrowed(l),
            None => std::borrow::Cow::Owned(super::DecorLighting::default()),
        }
    }

    /// A new room for this map: on an outdoor map, an open outdoor area (its sides can
    /// be fenced afterwards); in a dungeon, a walled room.
    pub fn new_room(&self, label: String) -> super::Room {
        let mut room = super::Room::new(label);
        if self.theme.ground.is_outdoor() {
            room.environment = super::RoomEnvironment::Outdoor;
            room.walls = super::Walls::all(super::WallKind::Open);
        }
        room
    }

    /// Every light on the map: the placed light sources, then lit decor.
    pub fn all_lights(&self) -> Vec<LightSource> {
        let mut lights = self.light_sources.clone();
        if let Some(layout) = &self.layout {
            lights.extend(super::decor_light::decor_light_sources(&self.graph, layout, &self.effective_decor_lighting()));
        }
        lights
    }
}

impl Default for Dungeon {
    fn default() -> Self {
        Self::new("Untitled Dungeon".to_string())
    }
}
