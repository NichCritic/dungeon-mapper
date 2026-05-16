use std::path::PathBuf;
use std::sync::mpsc;

use crate::model::{Campaign, Dungeon};

/// Current save file format version. Increment when the data model changes.
const CURRENT_VERSION: u32 = 2;

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
        2 => serde_json::from_value(value.clone()).map_err(|e| e.to_string()),
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
/// Returns a receiver that will produce the result.
pub fn save_campaign_to_path(campaign: &Campaign, path: PathBuf) -> mpsc::Receiver<FileOpResult> {
    let (tx, rx) = mpsc::channel();
    let json = match serialize_versioned(campaign) {
        Ok(j) => j,
        Err(e) => {
            let _ = tx.send(FileOpResult::Saved(Err(e.to_string())));
            return rx;
        }
    };
    std::thread::spawn(move || {
        let result = std::fs::write(&path, &json)
            .map(|_| path)
            .map_err(|e| e.to_string());
        let _ = tx.send(FileOpResult::Saved(result));
    });
    rx
}

/// Spawn an async save dialog on a background thread.
/// Returns a receiver that will eventually produce the result.
pub fn save_campaign_async(campaign: &Campaign) -> mpsc::Receiver<FileOpResult> {
    let (tx, rx) = mpsc::channel();
    let json = match serialize_versioned(campaign) {
        Ok(j) => j,
        Err(e) => {
            let _ = tx.send(FileOpResult::Saved(Err(e.to_string())));
            return rx;
        }
    };
    let name = campaign.name.clone();
    std::thread::spawn(move || {
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
    fn test_unsupported_version() {
        let json = r#"{"version":999,"campaign":{}}"#;
        let result = deserialize_versioned(json);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("newer than this application supports"));
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
