use std::path::PathBuf;
use std::sync::mpsc;

use crate::model::Dungeon;

/// Current save file format version. Increment when the data model changes.
const CURRENT_VERSION: u32 = 1;

/// Versioned save file envelope.
#[derive(serde::Serialize, serde::Deserialize)]
struct SaveFile {
    version: u32,
    dungeon: serde_json::Value,
}

/// Serialize a dungeon into a versioned JSON string.
fn serialize_versioned(dungeon: &Dungeon) -> Result<String, String> {
    let dungeon_value = serde_json::to_value(dungeon).map_err(|e| e.to_string())?;
    let save_file = SaveFile {
        version: CURRENT_VERSION,
        dungeon: dungeon_value,
    };
    serde_json::to_string_pretty(&save_file).map_err(|e| e.to_string())
}

/// Deserialize a dungeon from JSON, handling both versioned and legacy (unversioned) formats.
/// Uses version-specific load adaptors rather than a migration chain — each version
/// knows how to load directly into the current `Dungeon` struct.
fn deserialize_versioned(json: &str) -> Result<Dungeon, String> {
    let raw: serde_json::Value = serde_json::from_str(json).map_err(|e| e.to_string())?;

    if let Some(version) = raw.get("version").and_then(|v| v.as_u64()) {
        // Versioned format: { "version": N, "dungeon": { ... } }
        let dungeon_value = raw.get("dungeon")
            .ok_or("Save file missing 'dungeon' field")?;
        load_version(version as u32, dungeon_value)
    } else {
        // Legacy format (pre-versioning): the JSON is the dungeon directly
        load_version(0, &raw)
    }
}

/// Load a dungeon from a JSON value using the appropriate adaptor for the given version.
/// All versions load directly into the current `Dungeon` struct.
/// New fields added with `#[serde(default)]` are handled automatically.
/// Add explicit adaptor logic here only for structural changes (renames, restructures).
fn load_version(version: u32, value: &serde_json::Value) -> Result<Dungeon, String> {
    match version {
        // Version 0: legacy unversioned files
        // Version 1: first versioned format
        // Both use the same struct layout — new fields have #[serde(default)]
        0 | 1 => serde_json::from_value(value.clone()).map_err(|e| e.to_string()),

        // Future versions add arms here:
        // 2 => load_v2(value),

        v => Err(format!(
            "Save file version {} is newer than this application supports (max: {})",
            v, CURRENT_VERSION
        )),
    }
}

pub enum FileOpResult {
    Saved(Result<PathBuf, String>),
    Loaded(Result<(Dungeon, PathBuf), String>),
    ExportedPng(Result<(), String>),
    ExportedEncounters(Result<(), String>),
    ImportedEncounters(Result<EncounterImportData, String>),
    ExportedCreatures(Result<(), String>),
    ImportedCreatures(Result<Vec<crate::model::monster::CustomMonster>, String>),
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

/// Save a dungeon directly to a known file path (no dialog).
/// Returns a receiver that will produce the result.
pub fn save_dungeon_to_path(dungeon: &Dungeon, path: PathBuf) -> mpsc::Receiver<FileOpResult> {
    let (tx, rx) = mpsc::channel();
    let json = match serialize_versioned(dungeon) {
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
pub fn save_dungeon_async(dungeon: &Dungeon) -> mpsc::Receiver<FileOpResult> {
    let (tx, rx) = mpsc::channel();
    let json = match serialize_versioned(dungeon) {
        Ok(j) => j,
        Err(e) => {
            let _ = tx.send(FileOpResult::Saved(Err(e.to_string())));
            return rx;
        }
    };
    let name = dungeon.name.clone();
    std::thread::spawn(move || {
        let handle = pollster::block_on(
            rfd::AsyncFileDialog::new()
                .set_title("Save Dungeon")
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
pub fn load_dungeon_async() -> mpsc::Receiver<FileOpResult> {
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
                            .map(|d| (d, path));
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
