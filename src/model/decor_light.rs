//! Light given off by decor: braziers and fireplaces light the room around them
//! without the DM placing a separate light.
//!
//! Three levels, each overriding the one before: the campaign's setting (which decor
//! types give light, and how much), an optional replacement for one map, and each
//! decor item's own setting (follow the default, off, or its own light).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::{DecorType, LightSource, RoomDecor, SpatialLayout};

/// Light from one decor item: radii in grid cells, and colour.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
pub struct DecorLight {
    /// Bright light radius in grid cells.
    pub bright: f32,
    /// Dim light radius in grid cells (measured from the item, so at least `bright`).
    pub dim: f32,
    pub color: [u8; 3],
}

impl DecorLight {
    /// A torch: bright light for 20 feet and dim light for 20 feet beyond.
    pub const TORCH: DecorLight = DecorLight { bright: 4.0, dim: 8.0, color: [255, 170, 90] };
}

/// Which decor types give light, and how much. Set for the campaign; a map can
/// replace it with its own.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct DecorLighting {
    /// Off: no decor gives light by default (items with their own light still do).
    pub enabled: bool,
    /// The types that give light. A type not listed gives none.
    pub by_type: BTreeMap<DecorType, DecorLight>,
}

impl Default for DecorLighting {
    fn default() -> Self {
        Self {
            enabled: true,
            by_type: [(DecorType::Brazier, DecorLight::TORCH), (DecorType::Fireplace, DecorLight::TORCH)]
                .into_iter()
                .collect(),
        }
    }
}

impl DecorLighting {
    /// The light a decor type gives under these settings.
    pub fn for_type(&self, decor_type: DecorType) -> Option<DecorLight> {
        if !self.enabled {
            return None;
        }
        self.by_type.get(&decor_type).copied()
    }
}

/// One decor item's light.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq)]
pub enum DecorLightSetting {
    /// Whatever its type gives under the map's (or campaign's) settings.
    #[default]
    Default,
    /// No light, whatever the settings say.
    Off,
    /// Its own light, whatever the settings say.
    Custom(DecorLight),
}

impl RoomDecor {
    /// The light this item gives under `lighting` (the map's settings, or the campaign's).
    pub fn light(&self, lighting: &DecorLighting) -> Option<DecorLight> {
        match self.light {
            DecorLightSetting::Default => lighting.for_type(self.decor_type),
            DecorLightSetting::Off => None,
            DecorLightSetting::Custom(l) => Some(l),
        }
    }
}

/// Light sources for every lit decor item on the map, placed at the item (turned with
/// its room). Their ids are the decor ids prefixed `decor:`.
pub fn decor_light_sources(
    graph: &super::DungeonGraph,
    layout: &SpatialLayout,
    lighting: &DecorLighting,
) -> Vec<LightSource> {
    let mut out = Vec::new();
    for room in &graph.rooms {
        let Some(rl) = layout.room_by_id(&room.id) else { continue };
        for d in &room.decor {
            let Some(light) = d.light(lighting) else { continue };
            out.push(LightSource {
                id: format!("decor:{}", d.id),
                room_id: room.id.clone(),
                radius: light.bright,
                intensity: 1.0,
                color: light.color,
                pos: Some(rl.to_world(d.x, d.y)),
                dim_radius: Some(light.dim.max(light.bright)),
                carrier: None,
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{DungeonGraph, Room, RoomLayout};

    fn map_with(decor: Vec<RoomDecor>) -> (DungeonGraph, SpatialLayout) {
        let mut graph = DungeonGraph::new();
        let mut room = Room::new("Hall".into());
        room.decor = decor;
        let layout = SpatialLayout {
            rooms: vec![RoomLayout {
                room_id: room.id.clone(), x: 10, y: 20, width: 6, height: 6,
                violations: Vec::new(), wall_openings: Vec::new(), rotation: 0.0,
            }],
            ..SpatialLayout::new()
        };
        graph.add_room(room);
        (graph, layout)
    }

    #[test]
    fn braziers_and_fireplaces_give_torchlight_by_default() {
        let (graph, layout) = map_with(vec![
            RoomDecor::new(DecorType::Brazier, 1.5, 2.5),
            RoomDecor::new(DecorType::Fireplace, 3.0, 0.5),
            RoomDecor::new(DecorType::Table, 2.0, 2.0),
        ]);
        let lights = decor_light_sources(&graph, &layout, &DecorLighting::default());
        assert_eq!(lights.len(), 2, "the table gives no light");
        assert_eq!(lights[0].pos, Some((11.5, 22.5)), "placed at the brazier, in map coordinates");
        assert_eq!((lights[0].radius, lights[0].dim_radius()), (4.0, 8.0));
    }

    #[test]
    fn items_override_the_settings_and_settings_override_the_defaults() {
        let mut off = RoomDecor::new(DecorType::Brazier, 1.0, 1.0);
        off.light = DecorLightSetting::Off;
        let mut candle = RoomDecor::new(DecorType::Altar, 2.0, 2.0);
        candle.light = DecorLightSetting::Custom(DecorLight { bright: 1.0, dim: 2.0, color: [255, 220, 160] });
        let plain = RoomDecor::new(DecorType::Brazier, 3.0, 3.0);
        let (graph, layout) = map_with(vec![off, candle, plain]);

        let lit = decor_light_sources(&graph, &layout, &DecorLighting::default());
        assert_eq!(lit.len(), 2, "the switched-off brazier is dark; the altar has its own light");
        assert!(lit.iter().any(|l| l.radius == 1.0));

        let dark_map = DecorLighting { enabled: false, ..DecorLighting::default() };
        let lit = decor_light_sources(&graph, &layout, &dark_map);
        assert_eq!(lit.len(), 1, "with decor lighting off, only the custom light remains");
    }
}
