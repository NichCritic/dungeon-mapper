use std::path::PathBuf;
use std::sync::mpsc;

use crate::model::{Campaign, Dungeon};

/// Current save file format version. Increment when the data model changes.
const CURRENT_VERSION: u32 = 8;

/// Versioned save file envelope (version 2+: campaign-based).
#[derive(serde::Serialize, serde::Deserialize)]
struct SaveFile {
    version: u32,
    campaign: serde_json::Value,
}

/// Serialize a campaign into a versioned JSON string.
fn serialize_versioned(campaign: &Campaign) -> Result<String, String> {
    let campaign_value = serde_json::to_value(campaign).map_err(|e| e.to_string())?;
    let save_file = SaveFile {
        version: CURRENT_VERSION,
        campaign: campaign_value,
    };
    serde_json::to_string_pretty(&save_file).map_err(|e| e.to_string())
}

/// Public entry point for deserializing campaign JSON (used by cloud sync).
pub fn deserialize_campaign_json(json: &str) -> Result<Campaign, String> {
    deserialize_versioned(json)
}

/// Serialize a campaign to JSON (used by cloud sync).
pub fn serialize_campaign_json(campaign: &Campaign) -> Result<String, String> {
    serialize_versioned(campaign)
}

/// Deserialize a campaign from JSON, handling all format versions:
/// - Version 0: legacy unversioned single dungeon (raw JSON is the dungeon)
/// - Version 1: versioned single dungeon ({ version, dungeon })
/// - Version 2+: campaign format ({ version, campaign })
fn deserialize_versioned(json: &str) -> Result<Campaign, String> {
    let raw: serde_json::Value = serde_json::from_str(json).map_err(|e| e.to_string())?;

    if let Some(version) = raw.get("version").and_then(|v| v.as_u64()) {
        let version = version as u32;
        if version >= 2 {
            // Campaign format
            let campaign_value = raw.get("campaign")
                .ok_or("Save file missing 'campaign' field")?;
            load_campaign(version, campaign_value)
        } else {
            // Legacy dungeon format (version 1)
            let dungeon_value = raw.get("dungeon")
                .ok_or("Save file missing 'dungeon' field")?;
            let dungeon = load_dungeon(version, dungeon_value)?;
            Ok(Campaign::from_dungeon(dungeon))
        }
    } else {
        // Legacy format (pre-versioning, version 0): the JSON is the dungeon directly
        let dungeon = load_dungeon(0, &raw)?;
        Ok(Campaign::from_dungeon(dungeon))
    }
}

/// Load a dungeon from a JSON value using the appropriate adaptor for the given version.
fn load_dungeon(version: u32, value: &serde_json::Value) -> Result<Dungeon, String> {
    match version {
        0 | 1 => serde_json::from_value(value.clone()).map_err(|e| e.to_string()),
        v => Err(format!(
            "Save file version {} is newer than this application supports (max: {})",
            v, CURRENT_VERSION
        )),
    }
}

/// Load a campaign from a JSON value.
fn load_campaign(version: u32, value: &serde_json::Value) -> Result<Campaign, String> {
    match version {
        // v2 → v3: added parent_room_id + containment_padding to RoomGroup (both #[serde(default)])
        // v3 → v4: added Dungeon.tokens (#[serde(default)])
        // v4 → v5: decor cover, light pos/dim/carrier, PC sense ranges, session share flags (all defaults)
        // v5 → v6: added Dungeon.id (generated on load) for binding session notes to maps
        // v6 → v7: added Connection.overlap_walls (#[serde(default)] = Both)
        // v7 → v8: added RoomLayout.rotation (default 0) and Connection.corridor_angle (default Orthogonal)
        2 | 3 | 4 | 5 | 6 | 7 | 8 => serde_json::from_value(value.clone()).map_err(|e| e.to_string()),
        v => Err(format!(
            "Save file version {} is newer than this application supports (max: {})",
            v, CURRENT_VERSION
        )),
    }
}

pub enum FileOpResult {
    Saved(Result<PathBuf, String>),
    Loaded(Result<(Campaign, PathBuf), String>),
    ExportedPng(Result<(), String>),
    ExportedEncounters(Result<(), String>),
    ImportedEncounters(Result<EncounterImportData, String>),
    ExportedCreatures(Result<(), String>),
    ImportedCreatures(Result<Vec<crate::model::monster::CustomMonster>, String>),
    ImportedMap(Result<Campaign, String>),
    Cancelled,
}

/// Data bundle for encounter export/import.
/// Includes encounters and any custom monsters they reference.
#[derive(serde::Serialize, serde::Deserialize)]
pub struct EncounterExportData {
    pub encounters: Vec<crate::model::Encounter>,
    pub custom_monsters: Vec<crate::model::monster::CustomMonster>,
}

/// Result of importing encounters — needs room remapping by the caller.
pub struct EncounterImportData {
    pub encounters: Vec<crate::model::Encounter>,
    pub custom_monsters: Vec<crate::model::monster::CustomMonster>,
}

/// Export data for custom creatures.
#[derive(serde::Serialize, serde::Deserialize)]
pub struct CreatureExportData {
    pub custom_monsters: Vec<crate::model::monster::CustomMonster>,
}

/// Save a campaign directly to a known file path (no dialog).
/// Returns a receiver that will produce the result. The campaign is cloned and
/// serialized on the background thread: cloning is a fraction of the cost of
/// serializing a large campaign, which would otherwise stall every autosave.
pub fn save_campaign_to_path(campaign: &Campaign, path: PathBuf) -> mpsc::Receiver<FileOpResult> {
    let (tx, rx) = mpsc::channel();
    let campaign = campaign.clone();
    std::thread::spawn(move || {
        let result = serialize_versioned(&campaign)
            .and_then(|json| std::fs::write(&path, &json).map_err(|e| e.to_string()))
            .map(|_| path);
        let _ = tx.send(FileOpResult::Saved(result));
    });
    rx
}

/// Spawn an async save dialog on a background thread.
/// Returns a receiver that will eventually produce the result.
pub fn save_campaign_async(campaign: &Campaign) -> mpsc::Receiver<FileOpResult> {
    let (tx, rx) = mpsc::channel();
    let campaign = campaign.clone();
    let name = campaign.name.clone();
    std::thread::spawn(move || {
        let json = match serialize_versioned(&campaign) {
            Ok(j) => j,
            Err(e) => {
                let _ = tx.send(FileOpResult::Saved(Err(e)));
                return;
            }
        };
        let handle = pollster::block_on(
            rfd::AsyncFileDialog::new()
                .set_title("Save Campaign")
                .add_filter("Dungeon File", &["dungeon"])
                .set_file_name(format!("{}.dungeon", name))
                .save_file(),
        );
        match handle {
            Some(file) => {
                let path = file.path().to_path_buf();
                let result = std::fs::write(&path, &json)
                    .map(|_| path)
                    .map_err(|e| e.to_string());
                let _ = tx.send(FileOpResult::Saved(result));
            }
            None => {
                let _ = tx.send(FileOpResult::Cancelled);
            }
        }
    });
    rx
}

/// Spawn an async open dialog on a background thread.
pub fn load_campaign_async() -> mpsc::Receiver<FileOpResult> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let handle = pollster::block_on(
            rfd::AsyncFileDialog::new()
                .set_title("Open Dungeon")
                .add_filter("Dungeon File", &["dungeon"])
                .pick_file(),
        );
        match handle {
            Some(file) => {
                let path = file.path().to_path_buf();
                match std::fs::read_to_string(&path) {
                    Ok(json) => {
                        let result = deserialize_versioned(&json)
                            .map(|c| (c, path));
                        let _ = tx.send(FileOpResult::Loaded(result));
                    }
                    Err(e) => {
                        let _ = tx.send(FileOpResult::Loaded(Err(e.to_string())));
                    }
                }
            }
            None => {
                let _ = tx.send(FileOpResult::Cancelled);
            }
        }
    });
    rx
}

/// Open a file dialog to import a map from another .dungeon file.
/// Returns the loaded campaign so the caller can pick which map(s) to import.
pub fn import_map_async() -> mpsc::Receiver<FileOpResult> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let handle = pollster::block_on(
            rfd::AsyncFileDialog::new()
                .set_title("Import Map From...")
                .add_filter("Dungeon File", &["dungeon"])
                .pick_file(),
        );
        match handle {
            Some(file) => {
                let path = file.path().to_path_buf();
                match std::fs::read_to_string(&path) {
                    Ok(json) => {
                        let result = deserialize_versioned(&json);
                        let _ = tx.send(FileOpResult::ImportedMap(result));
                    }
                    Err(e) => {
                        let _ = tx.send(FileOpResult::ImportedMap(Err(e.to_string())));
                    }
                }
            }
            None => {
                let _ = tx.send(FileOpResult::Cancelled);
            }
        }
    });
    rx
}

/// Spawn an async export dialog on a background thread.
pub fn export_png_async(dungeon: &Dungeon, dm_mode: bool) -> mpsc::Receiver<FileOpResult> {
    let (tx, rx) = mpsc::channel();
    let dungeon = dungeon.clone();
    std::thread::spawn(move || {
        let handle = pollster::block_on(
            rfd::AsyncFileDialog::new()
                .set_title(if dm_mode { "Export DM Map" } else { "Export Player Map" })
                .add_filter("PNG Image", &["png"])
                .save_file(),
        );
        match handle {
            Some(file) => {
                let path = file.path().to_path_buf();
                let result = crate::io::export::export_png(&dungeon, &path, dm_mode, 2)
                    .map_err(|e| e.to_string());
                let _ = tx.send(FileOpResult::ExportedPng(result));
            }
            None => {
                let _ = tx.send(FileOpResult::Cancelled);
            }
        }
    });
    rx
}

/// Export selected encounters (and their referenced custom monsters) to a JSON file.
pub fn export_encounters_async(
    encounters: &[crate::model::Encounter],
    custom_monsters: &[crate::model::monster::CustomMonster],
) -> mpsc::Receiver<FileOpResult> {
    let (tx, rx) = mpsc::channel();
    let referenced_ids: std::collections::HashSet<String> = encounters.iter()
        .flat_map(|e| e.monsters.iter())
        .filter_map(|em| match &em.monster_ref {
            crate::model::monster::MonsterRef::Custom { id }
            | crate::model::monster::MonsterRef::Merged { id } => Some(id.clone()),
            _ => None,
        })
        .collect();
    let data = EncounterExportData {
        encounters: encounters.to_vec(),
        custom_monsters: custom_monsters.iter()
            .filter(|cm| referenced_ids.contains(&cm.id))
            .cloned()
            .collect(),
    };
    let json = match serde_json::to_string_pretty(&data) {
        Ok(j) => j,
        Err(e) => {
            let _ = tx.send(FileOpResult::ExportedEncounters(Err(e.to_string())));
            return rx;
        }
    };
    std::thread::spawn(move || {
        let handle = pollster::block_on(
            rfd::AsyncFileDialog::new()
                .set_title("Export Encounters")
                .add_filter("Encounter JSON", &["json"])
                .set_file_name("encounters.json")
                .save_file(),
        );
        match handle {
            Some(file) => {
                let path = file.path().to_path_buf();
                let result = std::fs::write(&path, &json).map(|_| ()).map_err(|e| e.to_string());
                let _ = tx.send(FileOpResult::ExportedEncounters(result));
            }
            None => { let _ = tx.send(FileOpResult::Cancelled); }
        }
    });
    rx
}

/// Import encounters from a JSON file.
pub fn import_encounters_async() -> mpsc::Receiver<FileOpResult> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let handle = pollster::block_on(
            rfd::AsyncFileDialog::new()
                .set_title("Import Encounters")
                .add_filter("Encounter JSON", &["json"])
                .pick_file(),
        );
        match handle {
            Some(file) => {
                let path = file.path().to_path_buf();
                match std::fs::read_to_string(&path) {
                    Ok(json) => {
                        match serde_json::from_str::<EncounterExportData>(&json) {
                            Ok(data) => {
                                let _ = tx.send(FileOpResult::ImportedEncounters(Ok(EncounterImportData {
                                    encounters: data.encounters,
                                    custom_monsters: data.custom_monsters,
                                })));
                            }
                            Err(e) => {
                                let _ = tx.send(FileOpResult::ImportedEncounters(Err(e.to_string())));
                            }
                        }
                    }
                    Err(e) => {
                        let _ = tx.send(FileOpResult::ImportedEncounters(Err(e.to_string())));
                    }
                }
            }
            None => { let _ = tx.send(FileOpResult::Cancelled); }
        }
    });
    rx
}

/// Export custom creatures to a JSON file.
pub fn export_creatures_async(
    custom_monsters: &[crate::model::monster::CustomMonster],
) -> mpsc::Receiver<FileOpResult> {
    let (tx, rx) = mpsc::channel();
    let data = CreatureExportData {
        custom_monsters: custom_monsters.to_vec(),
    };
    let json = match serde_json::to_string_pretty(&data) {
        Ok(j) => j,
        Err(e) => {
            let _ = tx.send(FileOpResult::ExportedCreatures(Err(e.to_string())));
            return rx;
        }
    };
    std::thread::spawn(move || {
        let handle = pollster::block_on(
            rfd::AsyncFileDialog::new()
                .set_title("Export Creatures")
                .add_filter("Creature JSON", &["json"])
                .set_file_name("creatures.json")
                .save_file(),
        );
        match handle {
            Some(file) => {
                let path = file.path().to_path_buf();
                let result = std::fs::write(&path, &json).map(|_| ()).map_err(|e| e.to_string());
                let _ = tx.send(FileOpResult::ExportedCreatures(result));
            }
            None => { let _ = tx.send(FileOpResult::Cancelled); }
        }
    });
    rx
}

/// Import custom creatures from a JSON file.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rotation_and_corridor_angle_round_trip() {
        use crate::model::*;
        let mut campaign = Campaign::new("Turned".into());
        let map = &mut campaign.maps[0];
        let (a, b) = (Room::new("A".into()), Room::new("B".into()));
        let (aid, bid) = (a.id.clone(), b.id.clone());
        map.graph.add_room(a);
        map.graph.add_room(b);
        let mut conn = Connection::new(ConnectionType::Door);
        conn.corridor_angle = CorridorAngle::Any;
        map.graph.add_connection(aid.clone(), bid, conn);
        let mut layout = SpatialLayout::new();
        layout.rooms.push(RoomLayout { room_id: aid, x: 1, y: 2, width: 3, height: 4, violations: Vec::new(), wall_openings: Vec::new(), rotation: 37.5 });
        map.layout = Some(layout);
        let loaded = deserialize_versioned(&serialize_versioned(&campaign).unwrap()).unwrap();
        assert_eq!(loaded.maps[0].layout.as_ref().unwrap().rooms[0].rotation, 37.5);
        assert_eq!(loaded.maps[0].graph.connections[0].connection.corridor_angle, CorridorAngle::Any);
        // Files from before v8 load unrotated and orthogonal
        let v7 = r#"{"version":7,"campaign":{"name":"Old","maps":[{"name":"M","graph":{"rooms":[],"connections":[],"graph_positions":{}},"layout":{"rooms":[{"room_id":"r","x":0,"y":0,"width":2,"height":2}],"corridors":[],"bounds":[]}}]}}"#;
        let old = deserialize_versioned(v7).unwrap();
        assert_eq!(old.maps[0].layout.as_ref().unwrap().rooms[0].rotation, 0.0);
    }

    #[test]
    fn test_save_to_path_writes_loadable_file() {
        let mut campaign = Campaign::new("Saved".into());
        campaign.maps[0].name = "Only Map".into();
        let path = std::env::temp_dir().join(format!("dm-save-test-{}.dungeon", uuid::Uuid::new_v4()));
        let rx = save_campaign_to_path(&campaign, path.clone());
        match rx.recv().unwrap() {
            FileOpResult::Saved(Ok(p)) => assert_eq!(p, path),
            _ => panic!("save failed"),
        }
        let loaded = deserialize_versioned(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let _ = std::fs::remove_file(&path);
        assert_eq!(loaded.name, "Saved");
        assert_eq!(loaded.maps[0].name, "Only Map");
    }

    #[test]
    fn test_load_legacy_unversioned() {
        let json = r#"{"name":"Test Dungeon","graph":{"rooms":[],"connections":[],"graph_positions":{}}}"#;
        let campaign = deserialize_versioned(json).unwrap();
        assert_eq!(campaign.maps.len(), 1);
        assert_eq!(campaign.maps[0].name, "Test Dungeon");
        assert_eq!(campaign.name, "Test Dungeon");
    }

    #[test]
    fn test_load_version_1() {
        let json = r#"{"version":1,"dungeon":{"name":"V1 Map","graph":{"rooms":[],"connections":[],"graph_positions":{}},"party":[{"id":"pc1","name":"Gandalf","class":"Wizard","ac":12,"max_hp":40,"current_hp":40,"initiative_modifier":2,"passive_perception":14}]}}"#;
        let campaign = deserialize_versioned(json).unwrap();
        assert_eq!(campaign.maps.len(), 1);
        assert_eq!(campaign.maps[0].name, "V1 Map");
        assert_eq!(campaign.party.len(), 1);
        assert_eq!(campaign.party[0].name, "Gandalf");
        assert!(campaign.maps[0].party.is_empty());
    }

    #[test]
    fn test_roundtrip_campaign() {
        let mut campaign = Campaign::new("Test Campaign".to_string());
        campaign.maps[0].name = "First Map".to_string();
        campaign.add_map("Second Map".to_string());
        campaign.party.push(crate::model::PlayerCharacter::new("Fighter".to_string()));

        let json = serialize_versioned(&campaign).unwrap();
        let loaded = deserialize_versioned(&json).unwrap();

        assert_eq!(loaded.name, "Test Campaign");
        assert_eq!(loaded.maps.len(), 2);
        assert_eq!(loaded.maps[0].name, "First Map");
        assert_eq!(loaded.maps[1].name, "Second Map");
        assert_eq!(loaded.party.len(), 1);
        assert_eq!(loaded.party[0].name, "Fighter");
    }

    #[test]
    fn test_version_4_tokens_roundtrip_and_v3_loads() {
        use crate::model::{MapToken, MonsterInstanceId, TokenKind};
        let mut campaign = Campaign::new("Tokens".to_string());
        campaign.maps[0].tokens.push(MapToken {
            kind: TokenKind::Monster(MonsterInstanceId { encounter_id: "e".into(), monster_index: 0, instance: 1 }),
            x: 3.5,
            y: 4.5,
        });
        campaign.maps[0].tokens.push(MapToken { kind: TokenKind::Player("pc1".into()), x: 1.5, y: 1.5 });

        let json = serialize_versioned(&campaign).unwrap();
        let raw: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(raw["version"], CURRENT_VERSION);
        let loaded = deserialize_versioned(&json).unwrap();
        assert_eq!(loaded.maps[0].tokens.len(), 2);
        assert_eq!(loaded.maps[0].tokens[0].x, 3.5);
        assert!(matches!(loaded.maps[0].tokens[1].kind, TokenKind::Player(ref id) if id == "pc1"));

        // A v3 file has no tokens field and still loads
        let v3 = r#"{"version":3,"campaign":{"name":"Old","maps":[{"name":"M","graph":{"rooms":[],"connections":[],"graph_positions":{}}}]}}"#;
        let old = deserialize_versioned(v3).unwrap();
        assert!(old.maps[0].tokens.is_empty());
    }

    #[test]
    fn test_version_5_cover_lights_senses_roundtrip_and_v4_loads() {
        use crate::model::{CoverKind, DecorType, LightSource, Room, RoomDecor, TokenKind};
        let mut campaign = Campaign::new("Five".to_string());
        let mut room = Room::new("Hall".to_string());
        let mut decor = RoomDecor::new(DecorType::Table, 1.0, 1.0);
        decor.cover = Some(CoverKind::Full);
        room.decor.push(decor);
        let room_id = room.id.clone();
        campaign.maps[0].graph.add_room(room);
        campaign.maps[0].light_sources.push(LightSource {
            id: "l".into(), room_id, radius: 4.0, intensity: 1.0, color: [255, 200, 100],
            pos: Some((3.5, 4.5)), dim_radius: Some(8.0), carrier: Some(TokenKind::Player("pc1".into())),
        });
        let mut pc = crate::model::PlayerCharacter::new("Ann".to_string());
        pc.senses.darkvision = true;
        pc.senses.darkvision_ft = 120;
        campaign.party.push(pc);
        campaign.maps[0].session.show_cover_player = true;

        let json = serialize_versioned(&campaign).unwrap();
        let raw: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(raw["version"], CURRENT_VERSION);
        let loaded = deserialize_versioned(&json).unwrap();
        assert_eq!(loaded.maps[0].graph.rooms[0].decor[0].cover, Some(CoverKind::Full));
        assert_eq!(loaded.maps[0].light_sources[0].pos, Some((3.5, 4.5)));
        assert_eq!(loaded.maps[0].light_sources[0].dim_radius, Some(8.0));
        assert!(matches!(loaded.maps[0].light_sources[0].carrier, Some(TokenKind::Player(ref p)) if p == "pc1"));
        assert_eq!(loaded.party[0].senses.darkvision_ft, 120);
        assert!(loaded.maps[0].session.show_cover_player);
        assert!(loaded.maps[0].session.show_light_player, "defaults to sharing light");

        // A v4 file (no cover / pos / ranges) still loads with defaults: take the v5
        // JSON and strip every field this version added.
        let mut v4: serde_json::Value = serde_json::from_str(&json).unwrap();
        v4["version"] = serde_json::json!(4);
        let map = &mut v4["campaign"]["maps"][0];
        map["graph"]["rooms"][0]["decor"][0]["decor_type"] = serde_json::json!("Pillar");
        map["graph"]["rooms"][0]["decor"][0].as_object_mut().unwrap().remove("cover");
        let light = map["light_sources"][0].as_object_mut().unwrap();
        light.remove("pos"); light.remove("dim_radius"); light.remove("carrier");
        light.insert("radius".into(), serde_json::json!(5.0));
        let session = map["session"].as_object_mut().unwrap();
        for k in ["show_light_player", "show_vision_player", "show_cover_player", "dm_show_light"] { session.remove(k); }
        let senses = v4["campaign"]["party"][0]["senses"].as_object_mut().unwrap();
        senses.remove("darkvision_ft"); senses.remove("blindsight_ft"); senses.remove("tremorsense_ft");
        let old = deserialize_versioned(&v4.to_string()).unwrap();
        let d = &old.maps[0].graph.rooms[0].decor[0];
        assert_eq!(d.cover, None);
        assert_eq!(d.cover_kind(), CoverKind::Full);
        assert_eq!(old.maps[0].light_sources[0].pos, None);
        assert_eq!(old.maps[0].light_sources[0].dim_radius(), 10.0);
        assert_eq!(old.party[0].senses.darkvision_ft, 60);
    }

    #[test]
    fn test_unsupported_version() {
        let json = r#"{"version":999,"campaign":{}}"#;
        let result = deserialize_versioned(json);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("newer than this application supports"));
    }

    #[test]
    fn test_version_6_map_ids_roundtrip_and_v5_loads() {
        let mut campaign = Campaign::new("Six".to_string());
        campaign.add_map("Second".to_string());
        let ids: Vec<String> = campaign.maps.iter().map(|m| m.id.clone()).collect();
        assert!(ids.iter().all(|id| !id.is_empty()), "every map gets an id");
        assert_ne!(ids[0], ids[1], "map ids are distinct");

        let json = serialize_versioned(&campaign).unwrap();
        let raw: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(raw["version"], CURRENT_VERSION);
        let loaded = deserialize_versioned(&json).unwrap();
        assert_eq!(loaded.maps[0].id, ids[0]);
        assert_eq!(loaded.maps[1].id, ids[1]);

        // A v5 file has no map ids; loading generates fresh, distinct ones.
        let mut v5: serde_json::Value = serde_json::from_str(&json).unwrap();
        v5["version"] = serde_json::json!(5);
        for map in v5["campaign"]["maps"].as_array_mut().unwrap() {
            map.as_object_mut().unwrap().remove("id");
        }
        let old = deserialize_versioned(&v5.to_string()).unwrap();
        assert!(!old.maps[0].id.is_empty());
        assert_ne!(old.maps[0].id, old.maps[1].id);
    }

    #[test]
    fn test_legacy_room_notes_field_still_round_trips() {
        // Room.notes was renamed to note_excerpt in v6 but keeps its wire name so
        // older saves keep loading; the note vault migrates the text out on load.
        let v5 = r#"{"version":5,"campaign":{"name":"Old","maps":[{"name":"M","graph":{"rooms":[{"id":"r1","label":"Hall","tags":[],"notes":"The dais is cracked.","size_hint":"Medium"}],"connections":[],"graph_positions":{}}}]}}"#;
        let loaded = deserialize_versioned(v5).unwrap();
        assert_eq!(loaded.maps[0].graph.rooms[0].note_excerpt, "The dais is cracked.");

        let json = serialize_versioned(&loaded).unwrap();
        let raw: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(raw["campaign"]["maps"][0]["graph"]["rooms"][0]["notes"], "The dais is cracked.");
    }
}

pub fn import_creatures_async() -> mpsc::Receiver<FileOpResult> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let handle = pollster::block_on(
            rfd::AsyncFileDialog::new()
                .set_title("Import Creatures")
                .add_filter("Creature JSON", &["json"])
                .pick_file(),
        );
        match handle {
            Some(file) => {
                let path = file.path().to_path_buf();
                match std::fs::read_to_string(&path) {
                    Ok(json) => {
                        match serde_json::from_str::<CreatureExportData>(&json) {
                            Ok(data) => {
                                let _ = tx.send(FileOpResult::ImportedCreatures(Ok(data.custom_monsters)));
                            }
                            Err(e) => {
                                let _ = tx.send(FileOpResult::ImportedCreatures(Err(e.to_string())));
                            }
                        }
                    }
                    Err(e) => {
                        let _ = tx.send(FileOpResult::ImportedCreatures(Err(e.to_string())));
                    }
                }
            }
            None => { let _ = tx.send(FileOpResult::Cancelled); }
        }
    });
    rx
}
