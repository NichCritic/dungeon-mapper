use serde::{Deserialize, Serialize};

use super::{Dungeon, PlayerCharacter};

/// A campaign is a collection of maps with shared party data.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Campaign {
    pub name: String,
    /// The maps in this campaign.
    pub maps: Vec<Dungeon>,
    /// Index of the currently active map.
    #[serde(default)]
    pub active_map: usize,
    /// Player characters shared across all maps in the campaign.
    #[serde(default)]
    pub party: Vec<PlayerCharacter>,
}

impl Campaign {
    pub fn new(name: String) -> Self {
        Self {
            name,
            maps: vec![Dungeon::default()],
            active_map: 0,
            party: Vec::new(),
        }
    }

    /// Create a campaign from a single dungeon, migrating party data up.
    pub fn from_dungeon(mut dungeon: Dungeon) -> Self {
        let party = std::mem::take(&mut dungeon.party);
        let name = dungeon.name.clone();
        Self {
            name,
            maps: vec![dungeon],
            active_map: 0,
            party,
        }
    }

    /// Get the currently active map.
    pub fn active_dungeon(&self) -> &Dungeon {
        &self.maps[self.active_map]
    }

    /// Add a new empty map to the campaign.
    pub fn add_map(&mut self, name: String) {
        self.maps.push(Dungeon::new(name));
    }

    /// Import a dungeon from another file into this campaign.
    /// Merges the dungeon's party into the campaign party (skipping duplicates by id).
    pub fn import_dungeon(&mut self, mut dungeon: Dungeon) {
        let incoming_party = std::mem::take(&mut dungeon.party);
        self.merge_party(incoming_party);
        self.maps.push(dungeon);
    }

    /// Merge party members, skipping duplicates by id.
    pub fn merge_party(&mut self, incoming: Vec<PlayerCharacter>) {
        let existing_ids: std::collections::HashSet<String> =
            self.party.iter().map(|pc| pc.id.clone()).collect();
        for pc in incoming {
            if !existing_ids.contains(&pc.id) {
                self.party.push(pc);
            }
        }
    }

    /// Switch to a different map by index.
    pub fn switch_map(&mut self, index: usize) {
        if index < self.maps.len() {
            self.active_map = index;
        }
    }

    /// Remove a map by index. Cannot remove the last map.
    pub fn remove_map(&mut self, index: usize) -> Option<Dungeon> {
        if self.maps.len() <= 1 || index >= self.maps.len() {
            return None;
        }
        let removed = self.maps.remove(index);
        if self.active_map >= self.maps.len() {
            self.active_map = self.maps.len() - 1;
        }
        Some(removed)
    }
}

impl Default for Campaign {
    fn default() -> Self {
        Self::new("Untitled Campaign".to_string())
    }
}
