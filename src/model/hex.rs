//! Hex maps for overland travel: terrain per hex, a feature, rivers and roads drawn
//! through the hexes, and a battle map for each hex where a fight might break out.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::util::GRID_PX;

/// Centre-to-corner size of a hex in world pixels.
pub const HEX_SIZE: f32 = GRID_PX * 2.0;

const SQRT3: f32 = 1.732_050_8;

/// Axial hex coordinates. `q + r + s = 0` with `s` implied.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct HexCoord {
    pub q: i32,
    pub r: i32,
}

impl HexCoord {
    /// The six neighbour offsets, going round the hex.
    pub const DIRECTIONS: [(i32, i32); 6] = [(1, 0), (1, -1), (0, -1), (-1, 0), (-1, 1), (0, 1)];

    pub fn new(q: i32, r: i32) -> Self {
        Self { q, r }
    }

    pub fn neighbors(self) -> [HexCoord; 6] {
        Self::DIRECTIONS.map(|(dq, dr)| HexCoord::new(self.q + dq, self.r + dr))
    }

    /// Steps between two hexes.
    pub fn distance(self, other: HexCoord) -> i32 {
        let dq = self.q - other.q;
        let dr = self.r - other.r;
        (dq.abs() + dr.abs() + (dq + dr).abs()) / 2
    }

    /// Every hex within `radius` steps of this one, this one first.
    pub fn within(self, radius: i32) -> Vec<HexCoord> {
        let mut out = vec![self];
        for q in -radius..=radius {
            for r in (-radius).max(-q - radius)..=radius.min(-q + radius) {
                if (q, r) != (0, 0) {
                    out.push(HexCoord::new(self.q + q, self.r + r));
                }
            }
        }
        out
    }

    /// The hexes on a straight line from this one to `to`, both ends included.
    pub fn line_to(self, to: HexCoord) -> Vec<HexCoord> {
        let n = self.distance(to);
        if n == 0 {
            return vec![self];
        }
        // Nudge off the exact edges so ties round the same way every time
        let (aq, ar) = (self.q as f32 + 1e-4, self.r as f32 + 1e-4);
        let (bq, br) = (to.q as f32 + 1e-4, to.r as f32 + 1e-4);
        (0..=n)
            .map(|i| {
                let t = i as f32 / n as f32;
                cube_round(aq + (bq - aq) * t, ar + (br - ar) * t)
            })
            .collect()
    }
}

/// The hex containing fractional axial coordinates.
fn cube_round(q: f32, r: f32) -> HexCoord {
    let s = -q - r;
    let (mut rq, mut rr, rs) = (q.round(), r.round(), s.round());
    let (dq, dr, ds) = ((rq - q).abs(), (rr - r).abs(), (rs - s).abs());
    if dq > dr && dq > ds {
        rq = -rr - rs;
    } else if dr > ds {
        rr = -rq - rs;
    }
    HexCoord::new(rq as i32, rr as i32)
}

/// Which way the hexes point.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum HexOrientation {
    /// Flat edges top and bottom; columns of hexes.
    #[default]
    FlatTop,
    /// A corner at the top; rows of hexes.
    PointyTop,
}

impl HexOrientation {
    pub const ALL: [HexOrientation; 2] = [HexOrientation::FlatTop, HexOrientation::PointyTop];

    pub fn label(self) -> &'static str {
        match self {
            HexOrientation::FlatTop => "Flat top",
            HexOrientation::PointyTop => "Pointy top",
        }
    }

    /// World-pixel centre of a hex.
    pub fn center(self, c: HexCoord) -> (f32, f32) {
        let (q, r) = (c.q as f32, c.r as f32);
        match self {
            HexOrientation::FlatTop => (HEX_SIZE * 1.5 * q, HEX_SIZE * SQRT3 * (r + q / 2.0)),
            HexOrientation::PointyTop => (HEX_SIZE * SQRT3 * (q + r / 2.0), HEX_SIZE * 1.5 * r),
        }
    }

    /// The six corners of a hex in world pixels, scaled by `scale` about its centre.
    pub fn corners(self, c: HexCoord, scale: f32) -> [(f32, f32); 6] {
        let (cx, cy) = self.center(c);
        let start = match self {
            HexOrientation::FlatTop => 0.0,
            HexOrientation::PointyTop => 30.0,
        };
        std::array::from_fn(|i| {
            let a = (start + 60.0 * i as f32).to_radians();
            (cx + HEX_SIZE * scale * a.cos(), cy + HEX_SIZE * scale * a.sin())
        })
    }

    /// The hex under a world-pixel point.
    pub fn hex_at(self, x: f32, y: f32) -> HexCoord {
        let (q, r) = match self {
            HexOrientation::FlatTop => {
                let q = x * 2.0 / 3.0 / HEX_SIZE;
                (q, (-x / 3.0 + SQRT3 / 3.0 * y) / HEX_SIZE)
            }
            HexOrientation::PointyTop => ((SQRT3 / 3.0 * x - y / 3.0) / HEX_SIZE, y * 2.0 / 3.0 / HEX_SIZE),
        };
        cube_round(q, r)
    }
}

/// The land a hex is mostly made of.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum HexTerrain {
    #[default]
    Plains,
    Grassland,
    Farmland,
    Steppe,
    Forest,
    Hills,
    Mountains,
    Marsh,
    Desert,
    Snow,
    Water,
}

impl HexTerrain {
    pub const ALL: [HexTerrain; 11] = [
        HexTerrain::Plains, HexTerrain::Grassland, HexTerrain::Farmland, HexTerrain::Steppe,
        HexTerrain::Forest, HexTerrain::Hills, HexTerrain::Mountains, HexTerrain::Marsh,
        HexTerrain::Desert, HexTerrain::Snow, HexTerrain::Water,
    ];

    pub fn label(self) -> &'static str {
        match self {
            HexTerrain::Plains => "Plains",
            HexTerrain::Grassland => "Grassland",
            HexTerrain::Farmland => "Farmland",
            HexTerrain::Steppe => "Dry steppe",
            HexTerrain::Forest => "Forest",
            HexTerrain::Hills => "Hills",
            HexTerrain::Mountains => "Mountains",
            HexTerrain::Marsh => "Marsh",
            HexTerrain::Desert => "Desert",
            HexTerrain::Snow => "Snow",
            HexTerrain::Water => "Water",
        }
    }

    /// The open ground a battle map made for a hex of this terrain starts with.
    pub fn battle_ground(self) -> super::Ground {
        use super::Ground;
        match self {
            HexTerrain::Plains | HexTerrain::Grassland | HexTerrain::Forest | HexTerrain::Marsh => Ground::Grass,
            HexTerrain::Farmland | HexTerrain::Steppe | HexTerrain::Hills | HexTerrain::Mountains => Ground::Earth,
            HexTerrain::Desert => Ground::Sand,
            HexTerrain::Snow => Ground::Snow,
            HexTerrain::Water => Ground::Sand,
        }
    }
}

/// A landmark drawn in the middle of a hex.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum HexFeature {
    Village,
    Castle,
    Tower,
    Ruin,
    Cave,
    Shrine,
    Camp,
    /// A sheepfold or pen.
    Fold,
    /// A ring of standing stones.
    Stones,
    /// A gate or door standing in the open: a way in or out.
    Gate,
    Lair,
    /// A pass or defile through high ground.
    Pass,
    Well,
    Tree,
}

impl HexFeature {
    pub const ALL: [HexFeature; 14] = [
        HexFeature::Village, HexFeature::Castle, HexFeature::Tower, HexFeature::Ruin,
        HexFeature::Cave, HexFeature::Shrine, HexFeature::Camp, HexFeature::Fold,
        HexFeature::Stones, HexFeature::Gate, HexFeature::Lair, HexFeature::Pass,
        HexFeature::Well, HexFeature::Tree,
    ];

    pub fn label(self) -> &'static str {
        match self {
            HexFeature::Village => "Village",
            HexFeature::Castle => "Castle",
            HexFeature::Tower => "Tower",
            HexFeature::Ruin => "Ruin",
            HexFeature::Cave => "Cave",
            HexFeature::Shrine => "Shrine",
            HexFeature::Camp => "Camp",
            HexFeature::Fold => "Fold / pen",
            HexFeature::Stones => "Standing stones",
            HexFeature::Gate => "Gate",
            HexFeature::Lair => "Lair",
            HexFeature::Pass => "Pass",
            HexFeature::Well => "Well",
            HexFeature::Tree => "Lone tree",
        }
    }
}

/// What a line drawn through the hexes is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum HexPathKind {
    #[default]
    River,
    Road,
    Track,
}

impl HexPathKind {
    pub const ALL: [HexPathKind; 3] = [HexPathKind::River, HexPathKind::Road, HexPathKind::Track];

    pub fn label(self) -> &'static str {
        match self {
            HexPathKind::River => "River",
            HexPathKind::Road => "Road",
            HexPathKind::Track => "Track",
        }
    }
}

/// A river, road or track running from hex to hex.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HexPath {
    pub id: String,
    pub kind: HexPathKind,
    /// The hexes it passes through, in order, each next to the one before.
    pub hexes: Vec<HexCoord>,
}

impl HexPath {
    /// A path through `points`, filling any gaps between them with straight runs.
    pub fn through(kind: HexPathKind, points: &[HexCoord]) -> Self {
        let mut hexes: Vec<HexCoord> = Vec::new();
        for &p in points {
            match hexes.last() {
                Some(&last) => hexes.extend(last.line_to(p).into_iter().skip(1)),
                None => hexes.push(p),
            }
        }
        Self { id: uuid::Uuid::new_v4().to_string(), kind, hexes }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Hex {
    /// Stable identity, for binding the hex's note.
    pub id: String,
    pub coord: HexCoord,
    pub terrain: HexTerrain,
    #[serde(default)]
    pub feature: Option<HexFeature>,
    #[serde(default)]
    pub label: String,
    /// The map (by id) to fight on when an encounter happens here.
    #[serde(default)]
    pub battle_map: Option<String>,
    /// How many sheep are in the hex.
    #[serde(default)]
    pub sheep: u32,
}

impl Hex {
    pub fn new(coord: HexCoord, terrain: HexTerrain) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            coord,
            terrain,
            feature: None,
            label: String::new(),
            battle_map: None,
            sheep: 0,
        }
    }

    /// Its label, or its coordinates when it has none.
    pub fn name(&self) -> String {
        if self.label.trim().is_empty() {
            format!("Hex {},{}", self.coord.q, self.coord.r)
        } else {
            self.label.clone()
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HexMap {
    #[serde(default)]
    pub orientation: HexOrientation,
    /// Distance across a hex, for travel.
    #[serde(default = "default_miles_per_hex")]
    pub miles_per_hex: f32,
    pub hexes: Vec<Hex>,
    #[serde(default)]
    pub paths: Vec<HexPath>,
    /// The hex the party is in, at the table.
    #[serde(default)]
    pub party: Option<HexCoord>,
    /// Hexes the players have seen, from inside or from afar.
    #[serde(default)]
    pub revealed: BTreeSet<HexCoord>,
    /// Hexes the party has been in: always also in `revealed`. The rest of
    /// `revealed` has only been observed (from a watch tower, a hilltop).
    #[serde(default)]
    pub explored: BTreeSet<HexCoord>,
    /// How far from the party hexes are revealed as it moves.
    #[serde(default = "default_reveal_radius")]
    pub reveal_radius: u32,
    /// The encounters that may turn up anywhere on the map, rolled for at the
    /// table. The encounters themselves are the map's (`Dungeon::encounters`, with
    /// no home); a hex's own encounters have the hex's id as their home and are
    /// always there, never rolled.
    #[serde(default)]
    pub random_encounters: Vec<RandomEncounter>,
    /// The last random encounter rolled, until it's fought or cleared.
    #[serde(default)]
    pub rolled: Option<RolledEncounter>,
}

/// An entry on a hex map's random encounter table.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RandomEncounter {
    /// The encounter's id.
    pub encounter: String,
    /// How likely it is against the others: 2 comes up twice as often as 1.
    pub weight: u32,
}

/// A random encounter that came up, and the hex it came up in.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RolledEncounter {
    pub encounter: String,
    pub hex: HexCoord,
}

/// How much the players know of a hex.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum HexSight {
    Hidden,
    /// Seen from somewhere else.
    Observed,
    /// The party has been in it.
    Explored,
}

impl HexSight {
    pub const ALL: [HexSight; 3] = [HexSight::Hidden, HexSight::Observed, HexSight::Explored];

    pub fn label(self) -> &'static str {
        match self {
            HexSight::Hidden => "Hidden",
            HexSight::Observed => "Observed",
            HexSight::Explored => "Explored",
        }
    }
}

fn default_miles_per_hex() -> f32 {
    6.0
}

fn default_reveal_radius() -> u32 {
    1
}

impl HexMap {
    /// A round map of hexes out to `radius` from the centre, all of one terrain.
    pub fn with_radius(radius: u32, terrain: HexTerrain) -> Self {
        Self {
            orientation: HexOrientation::default(),
            miles_per_hex: default_miles_per_hex(),
            hexes: HexCoord::new(0, 0).within(radius as i32).into_iter().map(|c| Hex::new(c, terrain)).collect(),
            paths: Vec::new(),
            party: None,
            revealed: BTreeSet::new(),
            explored: BTreeSet::new(),
            reveal_radius: default_reveal_radius(),
            random_encounters: Vec::new(),
            rolled: None,
        }
    }

    pub fn hex(&self, c: HexCoord) -> Option<&Hex> {
        self.hexes.iter().find(|h| h.coord == c)
    }

    pub fn hex_mut(&mut self, c: HexCoord) -> Option<&mut Hex> {
        self.hexes.iter_mut().find(|h| h.coord == c)
    }

    /// The hex at `c`, made with `terrain` if there isn't one.
    pub fn ensure_hex(&mut self, c: HexCoord, terrain: HexTerrain) -> &mut Hex {
        match self.hexes.iter().position(|h| h.coord == c) {
            Some(i) => &mut self.hexes[i],
            None => {
                self.hexes.push(Hex::new(c, terrain));
                self.hexes.last_mut().unwrap()
            }
        }
    }

    /// Remove a hex, and take it out of the paths through it (a path cut in two
    /// becomes two paths). Returns the hex, so its encounters can go too.
    pub fn remove_hex(&mut self, c: HexCoord) -> Option<Hex> {
        let removed = self.hexes.iter().position(|h| h.coord == c).map(|i| self.hexes.remove(i));
        if self.rolled.as_ref().is_some_and(|r| r.hex == c) {
            self.rolled = None;
        }
        self.revealed.remove(&c);
        self.explored.remove(&c);
        if self.party == Some(c) {
            self.party = None;
        }
        let mut paths = Vec::new();
        for path in std::mem::take(&mut self.paths) {
            if !path.hexes.contains(&c) {
                paths.push(path);
                continue;
            }
            for (i, run) in path.hexes.split(|&h| h == c).filter(|run| run.len() >= 2).enumerate() {
                let id = if i == 0 { path.id.clone() } else { uuid::Uuid::new_v4().to_string() };
                paths.push(HexPath { id, kind: path.kind, hexes: run.to_vec() });
            }
        }
        self.paths = paths;
        removed
    }

    /// Keep the random table in step with the map's encounters: an encounter with
    /// no home is on it (at weight 1 if new), any other is not. A roll of an
    /// encounter that's gone is forgotten.
    pub fn sync_random(&mut self, encounters: &[super::Encounter]) {
        self.random_encounters.retain(|r| encounters.iter().any(|e| e.id == r.encounter && e.home_room_id.is_empty()));
        for e in encounters.iter().filter(|e| e.home_room_id.is_empty()) {
            if !self.random_encounters.iter().any(|r| r.encounter == e.id) {
                self.random_encounters.push(RandomEncounter { encounter: e.id.clone(), weight: 1 });
            }
        }
        if self.rolled.as_ref().is_some_and(|r| !encounters.iter().any(|e| e.id == r.encounter)) {
            self.rolled = None;
        }
    }

    /// The table's total weight.
    pub fn random_weight(&self) -> u32 {
        self.random_encounters.iter().map(|r| r.weight).sum()
    }

    /// The random encounter at `n` along the table, for `n` below its total weight.
    pub fn random_at(&self, mut n: u32) -> Option<&RandomEncounter> {
        self.random_encounters.iter().find(|r| {
            if n < r.weight {
                return true;
            }
            n -= r.weight;
            false
        })
    }

    /// Roll on the random table for a hex, remembering what came up.
    pub fn roll_random(&mut self, hex: HexCoord) -> Option<&RolledEncounter> {
        use rand::Rng;
        let total = self.random_weight();
        if total == 0 {
            return None;
        }
        let pick = self.random_at(rand::thread_rng().gen_range(0..total))?.encounter.clone();
        self.rolled = Some(RolledEncounter { encounter: pick, hex });
        self.rolled.as_ref()
    }

    /// Hexes next to the map that aren't on it yet: where it can grow.
    pub fn rim(&self) -> Vec<HexCoord> {
        let on: BTreeSet<HexCoord> = self.hexes.iter().map(|h| h.coord).collect();
        let rim: BTreeSet<HexCoord> = on.iter().flat_map(|c| c.neighbors()).filter(|c| !on.contains(c)).collect();
        rim.into_iter().collect()
    }

    /// Put the party in a hex, exploring it, and observe what it can see from there.
    pub fn move_party(&mut self, c: HexCoord) {
        self.party = Some(c);
        self.observe_from(c, self.reveal_radius);
        if self.hex(c).is_some() {
            self.set_sight(c, HexSight::Explored);
        }
    }

    /// Observe the hexes within `radius` of `c` (say from a watch tower). Hexes
    /// already explored stay explored.
    pub fn observe_from(&mut self, c: HexCoord, radius: u32) {
        for seen in c.within(radius as i32) {
            if self.hex(seen).is_some() {
                self.revealed.insert(seen);
            }
        }
    }

    /// Whether the players can see a hex.
    pub fn is_revealed(&self, c: HexCoord) -> bool {
        self.revealed.contains(&c)
    }

    pub fn sight(&self, c: HexCoord) -> HexSight {
        if self.explored.contains(&c) {
            HexSight::Explored
        } else if self.revealed.contains(&c) {
            HexSight::Observed
        } else {
            HexSight::Hidden
        }
    }

    pub fn set_sight(&mut self, c: HexCoord, sight: HexSight) {
        match sight {
            HexSight::Hidden => {
                self.revealed.remove(&c);
                self.explored.remove(&c);
            }
            HexSight::Observed => {
                self.revealed.insert(c);
                self.explored.remove(&c);
            }
            HexSight::Explored => {
                self.revealed.insert(c);
                self.explored.insert(c);
            }
        }
    }

    /// Hide every hex from the players.
    pub fn hide_all(&mut self) {
        self.revealed.clear();
        self.explored.clear();
    }

    /// Sheep on the whole map.
    pub fn total_sheep(&self) -> u32 {
        self.hexes.iter().map(|h| h.sheep).sum()
    }

    /// World-pixel bounds of the given hexes: (min x, min y, max x, max y).
    pub fn extents<'a>(&self, coords: impl IntoIterator<Item = &'a HexCoord>) -> Option<(f32, f32, f32, f32)> {
        let mut ext: Option<(f32, f32, f32, f32)> = None;
        for &c in coords {
            for (x, y) in self.orientation.corners(c, 1.0) {
                ext = Some(match ext {
                    None => (x, y, x, y),
                    Some((x0, y0, x1, y1)) => (x0.min(x), y0.min(y), x1.max(x), y1.max(y)),
                });
            }
        }
        ext
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn radius_three_has_thirty_seven_hexes() {
        let map = HexMap::with_radius(3, HexTerrain::Grassland);
        assert_eq!(map.hexes.len(), 37);
        assert!(map.hexes.iter().all(|h| h.coord.distance(HexCoord::new(0, 0)) <= 3));
        assert_eq!(map.rim().len(), 24);
    }

    #[test]
    fn hex_at_finds_the_hex_under_its_centre_and_corners() {
        for orientation in HexOrientation::ALL {
            for c in HexCoord::new(0, 0).within(4) {
                let (x, y) = orientation.center(c);
                assert_eq!(orientation.hex_at(x, y), c);
                // Just inside each corner is still this hex
                for (px, py) in orientation.corners(c, 0.9) {
                    assert_eq!(orientation.hex_at(px, py), c, "{orientation:?} {c:?}");
                }
            }
        }
    }

    #[test]
    fn neighbours_sit_one_hex_width_apart() {
        for orientation in HexOrientation::ALL {
            let (x0, y0) = orientation.center(HexCoord::new(0, 0));
            for n in HexCoord::new(0, 0).neighbors() {
                let (x, y) = orientation.center(n);
                let d = ((x - x0).powi(2) + (y - y0).powi(2)).sqrt();
                assert!((d - HEX_SIZE * SQRT3).abs() < 0.01);
            }
        }
    }

    #[test]
    fn paths_fill_gaps_with_adjacent_hexes() {
        let path = HexPath::through(HexPathKind::River, &[HexCoord::new(-3, 0), HexCoord::new(3, 0), HexCoord::new(3, -3)]);
        assert_eq!(path.hexes.len(), 10);
        assert!(path.hexes.windows(2).all(|w| w[0].distance(w[1]) == 1));
    }

    #[test]
    fn removing_a_hex_splits_the_paths_through_it() {
        let mut map = HexMap::with_radius(3, HexTerrain::Plains);
        map.paths.push(HexPath::through(HexPathKind::Road, &[HexCoord::new(-3, 0), HexCoord::new(3, 0)]));
        map.move_party(HexCoord::new(0, 0));
        map.remove_hex(HexCoord::new(0, 0));
        assert_eq!(map.paths.len(), 2);
        assert!(map.paths.iter().all(|p| p.hexes.len() == 3));
        assert_eq!(map.party, None);
    }

    #[test]
    fn moving_the_party_reveals_around_it() {
        let mut map = HexMap::with_radius(3, HexTerrain::Plains);
        map.move_party(HexCoord::new(3, 0));
        // The hex and its neighbours that are on the map (it's on the edge)
        assert_eq!(map.revealed.len(), 4);
        map.reveal_radius = 0;
        map.move_party(HexCoord::new(-3, 0));
        assert_eq!(map.revealed.len(), 5);
    }

    #[test]
    fn the_random_table_follows_the_encounters_and_picks_by_weight() {
        use crate::model::Encounter;
        let mut map = HexMap::with_radius(1, HexTerrain::Plains);
        let wolves = Encounter::new("Wolves".into(), String::new());
        let bandits = Encounter::new("Bandits".into(), String::new());
        let den = Encounter::new("Wolf den".into(), map.hexes[0].id.clone());
        let mut encounters = vec![wolves.clone(), bandits.clone(), den];
        map.sync_random(&encounters);
        // The den belongs to its hex, so only the homeless two are on the table
        assert_eq!(map.random_encounters.len(), 2);
        map.random_encounters[0].weight = 3;
        assert_eq!(map.random_weight(), 4);
        let picks: Vec<&str> = (0..4).map(|n| map.random_at(n).unwrap().encounter.as_str()).collect();
        assert_eq!(picks, [wolves.id.as_str(), &wolves.id, &wolves.id, &bandits.id]);
        assert!(map.random_at(4).is_none());

        map.roll_random(HexCoord::new(0, 0)).unwrap();
        // Once the rolled one and the bandits are gone, the roll is forgotten too
        encounters.retain(|e| e.name == "Wolf den");
        map.sync_random(&encounters);
        assert!(map.random_encounters.is_empty());
        assert!(map.rolled.is_none());
        assert!(map.roll_random(HexCoord::new(0, 0)).is_none());
    }

    #[test]
    fn the_party_explores_where_it_goes_and_observes_round_it() {
        let mut map = HexMap::with_radius(3, HexTerrain::Plains);
        map.move_party(HexCoord::new(0, 0));
        map.move_party(HexCoord::new(1, 0));
        assert_eq!(map.sight(HexCoord::new(0, 0)), HexSight::Explored);
        assert_eq!(map.sight(HexCoord::new(1, 0)), HexSight::Explored);
        assert_eq!(map.sight(HexCoord::new(2, 0)), HexSight::Observed);
        assert_eq!(map.sight(HexCoord::new(-3, 0)), HexSight::Hidden);
        // A watch tower sees further, without un-exploring anything
        map.observe_from(HexCoord::new(0, 0), 3);
        assert_eq!(map.sight(HexCoord::new(-3, 0)), HexSight::Observed);
        assert_eq!(map.sight(HexCoord::new(0, 0)), HexSight::Explored);
        assert!(map.explored.is_subset(&map.revealed));
        map.set_sight(HexCoord::new(0, 0), HexSight::Hidden);
        assert!(!map.is_revealed(HexCoord::new(0, 0)));
        assert!(map.explored.is_subset(&map.revealed));
    }
}
