//! The note vault: real Markdown files in `<save name>.notes/`, next to the campaign.
//!
//! The folder is plain Obsidian-compatible Markdown — one `.md` per note, organised
//! into `rooms/`, `encounters/`, `doors/`, `maps/` and a free-form `pages/`. Which
//! entity a note belongs to lives in the file's frontmatter (`room: <id>`), never in
//! its path, so a file can be renamed or moved — here or in Obsidian — without
//! losing its binding. Free-floating notes have no binding and are reached with
//! `[[wikilinks]]`, resolved by filename the way Obsidian resolves them.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use super::markdown::{self, Block};
use crate::model::Campaign;

/// The entity a note is bound to. `None` (a free page) is represented by the
/// absence of a bind rather than a variant here.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum NoteBind {
    Campaign,
    Map(String),
    Room(String),
    Encounter(String),
    Door(String),
}

impl NoteBind {
    fn frontmatter_key(&self) -> &'static str {
        match self {
            NoteBind::Campaign => "campaign",
            NoteBind::Map(_) => "map",
            NoteBind::Room(_) => "room",
            NoteBind::Encounter(_) => "encounter",
            NoteBind::Door(_) => "door",
        }
    }

    fn frontmatter_value(&self) -> &str {
        match self {
            NoteBind::Campaign => "true",
            NoteBind::Map(id)
            | NoteBind::Room(id)
            | NoteBind::Encounter(id)
            | NoteBind::Door(id) => id,
        }
    }

    /// Vault subfolder this kind of note lives in by default.
    fn folder(&self) -> &'static str {
        match self {
            NoteBind::Campaign => "",
            NoteBind::Map(_) => "maps",
            NoteBind::Room(_) => "rooms",
            NoteBind::Encounter(_) => "encounters",
            NoteBind::Door(_) => "doors",
        }
    }

    fn from_frontmatter(pairs: &[(String, String)]) -> Option<NoteBind> {
        for (k, v) in pairs {
            let bind = match k.as_str() {
                "campaign" if v == "true" => NoteBind::Campaign,
                "map" => NoteBind::Map(v.clone()),
                "room" => NoteBind::Room(v.clone()),
                "encounter" => NoteBind::Encounter(v.clone()),
                "door" => NoteBind::Door(v.clone()),
                _ => continue,
            };
            return Some(bind);
        }
        None
    }
}

/// One note. `id` is its vault-relative path without the `.md` extension.
pub struct Note {
    pub id: String,
    pub title: String,
    pub body: String,
    pub bind: Option<NoteBind>,
    pub path: PathBuf,
    /// Parsed body, rebuilt lazily whenever `body` changes.
    cached: Option<Vec<Block>>,
}

impl Note {
    pub fn blocks(&mut self) -> &[Block] {
        if self.cached.is_none() {
            self.cached = Some(markdown::parse(&self.body));
        }
        self.cached.as_deref().unwrap_or(&[])
    }

    pub fn is_empty(&self) -> bool {
        self.body.trim().is_empty()
    }
}

#[derive(Default)]
pub struct NoteVault {
    /// Vault root, e.g. `/maps/Sunken Keep.notes`. `None` until the campaign is saved.
    pub root: Option<PathBuf>,
    notes: HashMap<String, Note>,
    dirty: HashSet<String>,
    /// Files to delete from disk on the next flush.
    tombstones: Vec<PathBuf>,
    /// Last I/O failure, surfaced in the notes panel rather than swallowed.
    pub last_error: Option<String>,
}

impl NoteVault {
    /// Vault folder for a campaign save path: `Foo.dmap` -> `Foo.notes`.
    pub fn root_for(save_path: &Path) -> PathBuf {
        let stem = save_path.file_stem().map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "campaign".to_string());
        save_path.with_file_name(format!("{}.notes", stem))
    }

    /// Point the vault at a save file's folder and read every `.md` in it.
    /// In-memory notes written before a save path existed are carried over.
    pub fn attach(&mut self, save_path: &Path) {
        let root = Self::root_for(save_path);
        if self.root.as_deref() == Some(root.as_path()) {
            return;
        }
        let carried: Vec<Note> = if self.root.is_none() {
            self.notes.drain().map(|(_, n)| n).filter(|n| !n.is_empty()).collect()
        } else {
            Vec::new()
        };
        self.notes.clear();
        self.dirty.clear();
        self.tombstones.clear();
        self.root = Some(root.clone());
        self.last_error = None;
        self.scan(&root);
        // Re-home anything typed before the campaign had a file.
        for mut note in carried {
            if note.bind.as_ref().is_some_and(|b| self.note_id_for(b).is_some()) {
                continue;
            }
            note.path = root.join(format!("{}.md", note.id));
            self.dirty.insert(note.id.clone());
            self.notes.insert(note.id.clone(), note);
        }
    }

    /// Forget everything (new campaign, or a load failure).
    pub fn reset(&mut self) {
        self.root = None;
        self.notes.clear();
        self.dirty.clear();
        self.tombstones.clear();
        self.last_error = None;
    }

    fn scan(&mut self, root: &Path) {
        if !root.is_dir() {
            return;
        }
        let mut stack = vec![root.to_path_buf()];
        while let Some(dir) = stack.pop() {
            let entries = match std::fs::read_dir(&dir) {
                Ok(e) => e,
                Err(e) => {
                    self.last_error = Some(format!("{}: {}", dir.display(), e));
                    continue;
                }
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    // Obsidian's own metadata folder is not ours to read.
                    if path.file_name().is_some_and(|n| n == ".obsidian") {
                        continue;
                    }
                    stack.push(path);
                } else if path.extension().is_some_and(|e| e == "md") {
                    match std::fs::read_to_string(&path) {
                        Ok(raw) => self.insert_from_disk(root, path, &raw),
                        Err(e) => self.last_error = Some(format!("{}: {}", path.display(), e)),
                    }
                }
            }
        }
    }

    fn insert_from_disk(&mut self, root: &Path, path: PathBuf, raw: &str) {
        let (front, body) = markdown::split_frontmatter(raw);
        let bind = NoteBind::from_frontmatter(&front);
        let id = relative_id(root, &path);
        let title = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
        self.notes.insert(
            id.clone(),
            Note { id, title, body: body.to_string(), bind, path, cached: None },
        );
    }

    // ------------------------------------------------------------ lookup

    pub fn note_id_for(&self, bind: &NoteBind) -> Option<&str> {
        self.notes
            .values()
            .find(|n| n.bind.as_ref() == Some(bind))
            .map(|n| n.id.as_str())
    }

    pub fn get(&self, id: &str) -> Option<&Note> {
        self.notes.get(id)
    }

    pub fn get_mut(&mut self, id: &str) -> Option<&mut Note> {
        self.notes.get_mut(id)
    }

    /// Every note, sorted by id — the browser's listing order.
    pub fn iter(&self) -> impl Iterator<Item = &Note> {
        let mut ids: Vec<&String> = self.notes.keys().collect();
        ids.sort();
        ids.into_iter().filter_map(|id| self.notes.get(id))
    }

    /// Resolve a `[[wikilink]]` the way Obsidian does: by filename anywhere in the
    /// vault, falling back to a full relative path, then a case-insensitive match.
    pub fn resolve_wiki(&self, target: &str) -> Option<&str> {
        let target = target.trim();
        if let Some(n) = self.notes.get(target) {
            return Some(n.id.as_str());
        }
        let lower = target.to_lowercase();
        self.notes
            .values()
            .find(|n| n.title == target)
            .or_else(|| self.notes.values().find(|n| n.title.to_lowercase() == lower))
            .or_else(|| self.notes.values().find(|n| n.id.to_lowercase() == lower))
            .map(|n| n.id.as_str())
    }

    /// Notes whose title or body contains `query`, with a matching snippet.
    pub fn search(&self, query: &str) -> Vec<(&str, String)> {
        let q = query.trim().to_lowercase();
        if q.is_empty() {
            return self.iter().map(|n| (n.id.as_str(), markdown::summary_line(&n.body))).collect();
        }
        let mut hits: Vec<(&str, String)> = Vec::new();
        for note in self.iter() {
            if note.title.to_lowercase().contains(&q) {
                hits.push((note.id.as_str(), markdown::summary_line(&note.body)));
                continue;
            }
            if let Some(line) = note
                .body
                .lines()
                .find(|l| l.to_lowercase().contains(&q))
            {
                hits.push((note.id.as_str(), line.trim().to_string()));
            }
        }
        hits
    }

    /// Titles matching a partial `[[` link, for editor autocomplete.
    pub fn complete(&self, partial: &str) -> Vec<&str> {
        let p = partial.trim().to_lowercase();
        let mut out: Vec<&str> = self
            .iter()
            .filter(|n| p.is_empty() || n.title.to_lowercase().contains(&p))
            .map(|n| n.title.as_str())
            .collect();
        out.dedup();
        out.truncate(12);
        out
    }

    // ------------------------------------------------------------ mutation

    /// Create (or fetch) the note bound to an entity, returning its id.
    pub fn ensure(&mut self, bind: NoteBind, title: &str, map_folder: Option<&str>) -> String {
        if let Some(id) = self.note_id_for(&bind) {
            return id.to_string();
        }
        let rel = self.unique_rel_path(bind.folder(), map_folder, title);
        self.insert_new(rel, Some(bind))
    }

    /// Create a free-floating page under `pages/`, returning its id.
    pub fn create_page(&mut self, title: &str) -> String {
        let rel = self.unique_rel_path("pages", None, title);
        self.insert_new(rel, None)
    }

    fn insert_new(&mut self, rel: String, bind: Option<NoteBind>) -> String {
        let title = rel.rsplit('/').next().unwrap_or(&rel).to_string();
        let path = self
            .root
            .as_ref()
            .map(|r| r.join(format!("{}.md", rel)))
            .unwrap_or_else(|| PathBuf::from(format!("{}.md", rel)));
        self.dirty.insert(rel.clone());
        self.notes.insert(
            rel.clone(),
            Note { id: rel.clone(), title, body: String::new(), bind, path, cached: None },
        );
        rel
    }

    fn unique_rel_path(&self, folder: &str, map_folder: Option<&str>, title: &str) -> String {
        let mut parts: Vec<String> = Vec::new();
        if !folder.is_empty() {
            parts.push(folder.to_string());
        }
        if let Some(m) = map_folder.filter(|_| !folder.is_empty() && folder != "maps") {
            parts.push(sanitize(m));
        }
        let base_name = sanitize(title);
        let prefix = if parts.is_empty() { String::new() } else { format!("{}/", parts.join("/")) };
        let mut candidate = format!("{}{}", prefix, base_name);
        let mut n = 2;
        while self.notes.contains_key(&candidate) {
            candidate = format!("{}{} {}", prefix, base_name, n);
            n += 1;
        }
        candidate
    }

    pub fn set_body(&mut self, id: &str, body: String) {
        if let Some(note) = self.notes.get_mut(id) {
            if note.body == body {
                return;
            }
            note.body = body;
            note.cached = None;
            self.dirty.insert(id.to_string());
        }
    }

    pub fn delete(&mut self, id: &str) {
        if let Some(note) = self.notes.remove(id) {
            self.tombstones.push(note.path);
            self.dirty.remove(id);
        }
    }

    pub fn has_unsaved(&self) -> bool {
        !self.dirty.is_empty() || !self.tombstones.is_empty()
    }

    // ------------------------------------------------------------ sync & flush

    /// Reconcile the vault against the campaign: migrate legacy inline room notes
    /// into files, and rename bound notes whose entity was renamed.
    pub fn sync(&mut self, campaign: &mut Campaign) {
        // One-way migration: a legacy save keeps full note text on the room itself.
        // Only rooms with no vault note yet are migrated, so an excerpt written back
        // by a previous sync is never appended to the note it came from.
        for map_idx in 0..campaign.maps.len() {
            let map_name = campaign.maps[map_idx].name.clone();
            for room_idx in 0..campaign.maps[map_idx].graph.rooms.len() {
                let room = &campaign.maps[map_idx].graph.rooms[room_idx];
                if room.note_excerpt.trim().is_empty() {
                    continue;
                }
                let bind = NoteBind::Room(room.id.clone());
                if self.note_id_for(&bind).is_some() {
                    continue;
                }
                let (label, legacy) = (room.label.clone(), room.note_excerpt.clone());
                let note_id = self.ensure(bind, &label, Some(&map_name));
                self.set_body(&note_id, legacy);
            }
        }

        // Follow entity renames so filenames stay human-readable.
        let mut renames: Vec<(String, String)> = Vec::new();
        for map in &campaign.maps {
            let map_name = map.name.clone();
            renames.extend(self.rename_for(&NoteBind::Map(map.id.clone()), &map_name, None));
            for room in &map.graph.rooms {
                renames.extend(self.rename_for(
                    &NoteBind::Room(room.id.clone()),
                    &room.label,
                    Some(&map_name),
                ));
            }
            for enc in &map.encounters {
                renames.extend(self.rename_for(
                    &NoteBind::Encounter(enc.id.clone()),
                    &enc.name,
                    Some(&map_name),
                ));
            }
            for edge in &map.graph.connections {
                let src = map.graph.room_by_id(&edge.source_room_id).map(|r| r.label.as_str()).unwrap_or("?");
                let tgt = map.graph.room_by_id(&edge.target_room_id).map(|r| r.label.as_str()).unwrap_or("?");
                renames.extend(self.rename_for(
                    &NoteBind::Door(edge.connection.id.clone()),
                    &format!("{} to {}", src, tgt),
                    Some(&map_name),
                ));
            }
        }
        for (old_id, new_id) in renames {
            self.apply_rename(&old_id, &new_id);
        }

        self.refresh_excerpts(campaign);
    }

    /// Push a one-line preview of each room's note back onto the room, so map
    /// renderers can print it without reaching into the vault.
    fn refresh_excerpts(&self, campaign: &mut Campaign) {
        for map in &mut campaign.maps {
            for room in &mut map.graph.rooms {
                let excerpt = self
                    .note_id_for(&NoteBind::Room(room.id.clone()))
                    .and_then(|id| self.notes.get(id))
                    .map(|n| markdown::summary_line(&n.body))
                    .unwrap_or_default();
                room.note_excerpt = excerpt;
            }
        }
    }

    /// If the note bound to `bind` no longer sits at the path its entity's name
    /// implies, produce the (old id, new id) pair to move it to.
    fn rename_for(&self, bind: &NoteBind, title: &str, map_folder: Option<&str>) -> Option<(String, String)> {
        let note = self.notes.values().find(|n| n.bind.as_ref() == Some(bind))?;
        let wanted_title = sanitize(title);
        if note.title == wanted_title {
            return None;
        }
        let old_id = note.id.clone();
        let new_id = self.unique_rel_path(bind.folder(), map_folder, title);
        (old_id != new_id).then_some((old_id, new_id))
    }

    fn apply_rename(&mut self, old_id: &str, new_id: &str) {
        let Some(mut note) = self.notes.remove(old_id) else { return };
        let old_path = note.path.clone();
        note.id = new_id.to_string();
        note.title = new_id.rsplit('/').next().unwrap_or(new_id).to_string();
        note.path = self
            .root
            .as_ref()
            .map(|r| r.join(format!("{}.md", new_id)))
            .unwrap_or_else(|| PathBuf::from(format!("{}.md", new_id)));
        if old_path != note.path && old_path.exists() {
            if let Some(parent) = note.path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if let Err(e) = std::fs::rename(&old_path, &note.path) {
                self.last_error = Some(format!("rename {}: {}", old_path.display(), e));
            }
        }
        self.dirty.remove(old_id);
        self.dirty.insert(new_id.to_string());
        self.notes.insert(new_id.to_string(), note);
    }

    /// Write every changed note to disk and delete tombstoned files.
    /// A no-op while the campaign has no save path — notes stay in memory until then.
    pub fn flush(&mut self) {
        let Some(root) = self.root.clone() else { return };

        for path in std::mem::take(&mut self.tombstones) {
            if path.exists() {
                if let Err(e) = std::fs::remove_file(&path) {
                    self.last_error = Some(format!("delete {}: {}", path.display(), e));
                }
            }
        }

        if self.dirty.is_empty() {
            return;
        }
        if let Err(e) = std::fs::create_dir_all(&root) {
            self.last_error = Some(format!("{}: {}", root.display(), e));
            return;
        }

        for id in std::mem::take(&mut self.dirty) {
            let Some(note) = self.notes.get(&id) else { continue };
            // Never leave an empty file behind for a note nobody wrote in.
            if note.is_empty() && !note.path.exists() {
                continue;
            }
            if let Some(parent) = note.path.parent() {
                if let Err(e) = std::fs::create_dir_all(parent) {
                    self.last_error = Some(format!("{}: {}", parent.display(), e));
                    continue;
                }
            }
            let contents = match &note.bind {
                Some(bind) => format!(
                    "---\n{}: {}\n---\n\n{}",
                    bind.frontmatter_key(),
                    bind.frontmatter_value(),
                    note.body
                ),
                None => note.body.clone(),
            };
            if let Err(e) = std::fs::write(&note.path, contents) {
                self.last_error = Some(format!("{}: {}", note.path.display(), e));
            }
        }
    }
}

/// Vault-relative id for a file: path from the root, `/`-separated, no extension.
fn relative_id(root: &Path, path: &Path) -> String {
    let rel = path.strip_prefix(root).unwrap_or(path);
    let s = rel.with_extension("").to_string_lossy().to_string();
    s.replace('\\', "/")
}

/// Make a title safe as a single filename component, keeping it readable.
fn sanitize(title: &str) -> String {
    let cleaned: String = title
        .chars()
        .map(|c| if matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') { '-' } else { c })
        .collect();
    let trimmed = cleaned.trim().trim_matches('.').trim();
    if trimmed.is_empty() {
        "Untitled".to_string()
    } else {
        trimmed.chars().take(80).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Dungeon, Room};

    /// A scratch vault rooted in a fresh temp directory.
    struct Scratch {
        dir: PathBuf,
    }

    impl Scratch {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "dm-vault-{}-{}-{:?}",
                tag,
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            std::fs::create_dir_all(&dir).unwrap();
            Scratch { dir }
        }

        fn save_path(&self) -> PathBuf {
            self.dir.join("Sunken Keep.dmap")
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn campaign_with_room(label: &str, legacy_notes: &str) -> (Campaign, String) {
        let mut map = Dungeon::new("Sunken Keep".to_string());
        let mut room = Room::new(label.to_string());
        room.note_excerpt = legacy_notes.to_string();
        let room_id = room.id.clone();
        map.graph.rooms.push(room);
        let mut campaign = Campaign::new("Sunken Keep".to_string());
        campaign.maps = vec![map];
        (campaign, room_id)
    }

    #[test]
    fn root_sits_next_to_the_save_file() {
        let root = NoteVault::root_for(Path::new("/maps/Sunken Keep.dmap"));
        assert_eq!(root, PathBuf::from("/maps/Sunken Keep.notes"));
    }

    #[test]
    fn legacy_room_notes_migrate_into_a_file_once() {
        let scratch = Scratch::new("migrate");
        let (mut campaign, room_id) = campaign_with_room("Throne Room", "The dais is cracked.");

        let mut vault = NoteVault::default();
        vault.attach(&scratch.save_path());
        vault.sync(&mut campaign);
        vault.flush();

        let file = scratch.dir.join("Sunken Keep.notes/rooms/Sunken Keep/Throne Room.md");
        let written = std::fs::read_to_string(&file).expect("note file written");
        assert!(written.contains(&format!("room: {}", room_id)));
        assert!(written.contains("The dais is cracked."));
        // The room now carries only a derived one-line excerpt.
        assert_eq!(campaign.maps[0].graph.rooms[0].note_excerpt, "The dais is cracked.");

        // Re-opening must not append the excerpt back onto the note it came from.
        let mut reopened = NoteVault::default();
        reopened.attach(&scratch.save_path());
        reopened.sync(&mut campaign);
        reopened.flush();
        let again = std::fs::read_to_string(&file).unwrap();
        assert_eq!(again.matches("The dais is cracked.").count(), 1);
    }

    #[test]
    fn a_note_survives_a_write_and_reload() {
        let scratch = Scratch::new("roundtrip");
        let (mut campaign, room_id) = campaign_with_room("Entry Hall", "");

        let mut vault = NoteVault::default();
        vault.attach(&scratch.save_path());
        let id = vault.ensure(NoteBind::Room(room_id.clone()), "Entry Hall", Some("Sunken Keep"));
        vault.set_body(&id, "# Entry Hall\n\nSee [[NPC - Vex]].".to_string());
        vault.sync(&mut campaign);
        vault.flush();

        let mut reloaded = NoteVault::default();
        reloaded.attach(&scratch.save_path());
        let found = reloaded.note_id_for(&NoteBind::Room(room_id)).expect("binding survived");
        assert_eq!(reloaded.get(found).unwrap().body.trim(), "# Entry Hall\n\nSee [[NPC - Vex]].");
    }

    #[test]
    fn renaming_a_room_renames_its_note_file() {
        let scratch = Scratch::new("rename");
        let (mut campaign, room_id) = campaign_with_room("Throne Room", "Cracked dais.");

        let mut vault = NoteVault::default();
        vault.attach(&scratch.save_path());
        vault.sync(&mut campaign);
        vault.flush();

        campaign.maps[0].graph.rooms[0].label = "Hall of Kings".to_string();
        vault.sync(&mut campaign);
        vault.flush();

        let rooms_dir = scratch.dir.join("Sunken Keep.notes/rooms/Sunken Keep");
        assert!(rooms_dir.join("Hall of Kings.md").exists());
        assert!(!rooms_dir.join("Throne Room.md").exists());
        // The binding follows the file, not the other way round.
        assert!(vault.note_id_for(&NoteBind::Room(room_id)).is_some());
    }

    #[test]
    fn wikilinks_resolve_by_filename_anywhere_in_the_vault() {
        let scratch = Scratch::new("wiki");
        let mut vault = NoteVault::default();
        vault.attach(&scratch.save_path());
        let page = vault.create_page("NPC - Vex");
        assert_eq!(vault.resolve_wiki("NPC - Vex"), Some(page.as_str()));
        assert_eq!(vault.resolve_wiki("npc - vex"), Some(page.as_str()));
        assert_eq!(vault.resolve_wiki("Nobody"), None);
    }

    #[test]
    fn titles_that_are_illegal_filenames_are_made_safe() {
        let scratch = Scratch::new("sanitize");
        let mut vault = NoteVault::default();
        vault.attach(&scratch.save_path());
        let id = vault.ensure(NoteBind::Room("r1".into()), "Vault / Crypt: 2", Some("Sunken Keep"));
        vault.set_body(&id, "body".into());
        vault.flush();
        assert!(scratch
            .dir
            .join("Sunken Keep.notes/rooms/Sunken Keep/Vault - Crypt- 2.md")
            .exists());
    }

    #[test]
    fn two_rooms_with_the_same_label_get_distinct_files() {
        let scratch = Scratch::new("dedupe");
        let mut vault = NoteVault::default();
        vault.attach(&scratch.save_path());
        let a = vault.ensure(NoteBind::Room("r1".into()), "Cell", Some("Sunken Keep"));
        let b = vault.ensure(NoteBind::Room("r2".into()), "Cell", Some("Sunken Keep"));
        assert_ne!(a, b);
        assert_eq!(b, "rooms/Sunken Keep/Cell 2");
    }

    #[test]
    fn notes_typed_before_a_save_path_exists_are_carried_over() {
        let scratch = Scratch::new("carry");
        let mut vault = NoteVault::default();
        // No root yet: the campaign has never been saved.
        let id = vault.ensure(NoteBind::Room("r1".into()), "Throne Room", Some("Sunken Keep"));
        vault.set_body(&id, "Written before the first save.".to_string());
        assert!(vault.root.is_none());

        vault.attach(&scratch.save_path());
        vault.flush();

        let file = scratch.dir.join("Sunken Keep.notes/rooms/Sunken Keep/Throne Room.md");
        assert!(std::fs::read_to_string(file).unwrap().contains("Written before the first save."));
    }

    #[test]
    fn empty_notes_never_reach_disk() {
        let scratch = Scratch::new("empty");
        let mut vault = NoteVault::default();
        vault.attach(&scratch.save_path());
        vault.ensure(NoteBind::Room("r1".into()), "Untouched", Some("Sunken Keep"));
        vault.flush();
        assert!(!scratch
            .dir
            .join("Sunken Keep.notes/rooms/Sunken Keep/Untouched.md")
            .exists());
    }

    #[test]
    fn deleting_a_note_removes_its_file() {
        let scratch = Scratch::new("delete");
        let mut vault = NoteVault::default();
        vault.attach(&scratch.save_path());
        let id = vault.create_page("Scratch");
        vault.set_body(&id, "temporary".into());
        vault.flush();
        let file = scratch.dir.join("Sunken Keep.notes/pages/Scratch.md");
        assert!(file.exists());

        vault.delete(&id);
        vault.flush();
        assert!(!file.exists());
        assert!(vault.get(&id).is_none());
    }

    #[test]
    fn search_matches_titles_and_body_text() {
        let scratch = Scratch::new("search");
        let mut vault = NoteVault::default();
        vault.attach(&scratch.save_path());
        let a = vault.create_page("NPC - Vex");
        vault.set_body(&a, "A tiefling broker.".into());
        let b = vault.create_page("Plot - Sigil");
        vault.set_body(&b, "Vex knows where it is.".into());

        let by_title: Vec<&str> = vault.search("vex").into_iter().map(|(id, _)| id).collect();
        assert!(by_title.contains(&a.as_str()));
        assert!(by_title.contains(&b.as_str()));

        let by_body: Vec<&str> = vault.search("tiefling").into_iter().map(|(id, _)| id).collect();
        assert_eq!(by_body, vec![a.as_str()]);
    }

    #[test]
    fn free_pages_have_no_frontmatter() {
        let scratch = Scratch::new("frontmatter");
        let mut vault = NoteVault::default();
        vault.attach(&scratch.save_path());
        let id = vault.create_page("NPC - Vex");
        vault.set_body(&id, "A tiefling broker.".into());
        vault.flush();
        let raw = std::fs::read_to_string(scratch.dir.join("Sunken Keep.notes/pages/NPC - Vex.md")).unwrap();
        assert_eq!(raw, "A tiefling broker.");
    }
}
