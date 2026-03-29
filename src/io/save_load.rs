use std::path::PathBuf;

use crate::model::Dungeon;

pub fn save_dungeon(dungeon: &Dungeon) -> Result<PathBuf, String> {
    let path = rfd::FileDialog::new()
        .set_title("Save Dungeon")
        .add_filter("Dungeon File", &["dungeon"])
        .set_file_name(format!("{}.dungeon", dungeon.name))
        .save_file()
        .ok_or("Save cancelled")?;

    let json = serde_json::to_string_pretty(dungeon).map_err(|e| e.to_string())?;
    std::fs::write(&path, json).map_err(|e| e.to_string())?;

    Ok(path)
}

pub fn load_dungeon() -> Result<Dungeon, String> {
    let path = rfd::FileDialog::new()
        .set_title("Open Dungeon")
        .add_filter("Dungeon File", &["dungeon"])
        .pick_file()
        .ok_or("Open cancelled")?;

    let json = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
    let dungeon: Dungeon = serde_json::from_str(&json).map_err(|e| e.to_string())?;

    Ok(dungeon)
}
