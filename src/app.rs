use crate::data::MonsterDatabase;
use crate::history::UndoHistory;
use crate::model::combat_stats::CombatStatsCache;
use crate::model::{Campaign, Dungeon};
use crate::presentation::PresentationState;
use crate::server::PresentationServer;
use crate::updater;
use crate::ui::annotations::{self, AnnotationModeState};
use crate::ui::decor_view::{self, DecorViewState};
use crate::ui::encounters_view::{self, EncountersViewState};
use crate::ui::graph_editor::{self, GraphEditorState};
use crate::ui::spatial_view::{self, SpatialViewState};
use crate::ui::styled_view::{self, StyledViewState};
use crate::ui::presentation_view::{self, PresentationViewState, ServerAction};
use crate::ui::player_view::{self, PlayerViewState};

/// Restart the application by spawning a new process and exiting.
/// How often the status bar and render pre-warming re-check the render caches.
const CACHE_CHECK_INTERVAL: std::time::Duration = std::time::Duration::from_millis(250);
/// How long the map must go unedited before other views' renders are pre-warmed.
const PREWARM_SETTLE: std::time::Duration = std::time::Duration::from_millis(500);

fn restart_app() -> ! {
    let exe = std::env::current_exe().expect("Failed to get current exe path");
    let args: Vec<String> = std::env::args().skip(1).collect();
    std::process::Command::new(exe)
        .args(&args)
        .spawn()
        .expect("Failed to restart application");
    std::process::exit(0);
}

/// How long typing must pause before dirty notes are written to their .md files.
const NOTES_FLUSH_DELAY: std::time::Duration = std::time::Duration::from_millis(1200);

/// Result wrapper for async cloud sync operations.
enum CloudSyncOp {
    Login(crate::io::cloud_sync::LoginResult),
    SyncDone(crate::io::cloud_sync::SyncResult, crate::io::cloud_sync::CloudSyncState),
    FileList(Result<Vec<crate::io::cloud_sync::DriveFile>, String>, crate::io::cloud_sync::CloudSyncState),
    Opened(Result<String, String>, crate::io::cloud_sync::CloudSyncState),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Tab {
    Graph,
    Spatial,
    Decor,
    Encounters,
    Styled,
}

pub struct DungeonApp {
    pub campaign: Campaign,
    pub dungeon: Dungeon,
    pub active_tab: Tab,
    pub graph_state: GraphEditorState,
    pub spatial_state: SpatialViewState,
    pub decor_state: DecorViewState,
    pub encounters_state: EncountersViewState,
    pub styled_state: StyledViewState,
    /// Snapshot of graph state to detect when a re-solve is needed
    last_graph_snapshot: u64,
    /// Monster stats database loaded from 5e-Tools data files.
    pub monster_db: MonsterDatabase,
    /// Lazy cache for parsed combat stats.
    pub combat_stats_cache: CombatStatsCache,

    // Presentation mode
    pub presenting: bool,
    pub presentation: Option<PresentationState>,
    pub presentation_view_state: PresentationViewState,
    pub player_viewport_open: bool,
    pub player_view_state: PlayerViewState,
    /// True after the player viewport has been shown at least once (avoids re-applying initial size every frame).
    player_viewport_initialized: bool,
    pub server: Option<PresentationServer>,
    pub server_port: u16,
    /// Hash of the last PNG pushed to the server, to avoid redundant updates.
    last_server_push_hash: u64,
    /// In-flight background render of the server PNG; signals when it finishes.
    pending_server_png: Option<std::sync::mpsc::Receiver<()>>,
    /// Which map the player view shows. None = same as DM's active map.
    player_map_index: Option<usize>,
    /// Dungeon snapshot for the player view when it differs from DM view.
    player_dungeon: Option<Dungeon>,
    /// Presentation state for the player view when it differs from DM view.
    player_presentation: Option<PresentationState>,

    // Annotation mode
    pub annotation_mode: bool,
    pub annotation_state: AnnotationModeState,
    /// F8 help overlay mode.
    pub help_mode: bool,

    /// Pending async file operation (save/load/export).
    pending_file_op: Option<std::sync::mpsc::Receiver<crate::io::save_load::FileOpResult>>,
    /// Pending background monster database load.
    pending_monster_db: Option<std::sync::mpsc::Receiver<MonsterDatabase>>,
    /// Maps available for import (shown in import dialog).
    import_candidates: Option<Campaign>,

    // Cloud sync
    cloud_sync: crate::io::cloud_sync::CloudSyncState,
    /// Whether cloud sync is enabled for the current file.
    cloud_sync_enabled: bool,
    /// Pending cloud sync operation (login, push, pull).
    pending_cloud_op: Option<std::sync::mpsc::Receiver<CloudSyncOp>>,
    /// Status message from last cloud operation.
    cloud_status: Option<String>,
    /// Drive file list for "Open from Drive" dialog.
    drive_file_list: Option<Vec<crate::io::cloud_sync::DriveFile>>,

    // Undo/Redo
    pub history: UndoHistory,

    // Save state
    /// Current file path (set after Save As or Open).
    pub current_file: Option<std::path::PathBuf>,
    /// Hash of the dungeon at last save (to detect unsaved changes for auto-save).
    last_saved_hash: u64,
    /// Time of last auto-save.
    last_autosave: std::time::Instant,
    /// True when the auto-save timer has elapsed and we're waiting for the next change.
    autosave_due: bool,
    /// Committed hash from previous frame, used to detect new commits for auto-save.
    last_autosave_hash: u64,
    // Auto-update
    pending_update_check: Option<std::sync::mpsc::Receiver<updater::UpdateStatus>>,
    available_update: Option<updater::UpdateInfo>,
    pending_update_apply: Option<std::sync::mpsc::Receiver<updater::ApplyStatus>>,
    update_ready_to_restart: bool,
    update_error: Option<String>,
    show_update_dialog: bool,
    last_update_check: std::time::Instant,

    /// Set when the layout solver refuses to run (e.g. a containment cycle),
    /// shown as a dismissible dialog. Cleared on the next successful solve.
    layout_error: Option<String>,

    /// Hash of dungeon state used for render pre-warming debounce.
    last_prewarm_hash: u64,
    /// When the prewarm hash last changed (for debounce).
    prewarm_hash_changed_at: std::time::Instant,
    /// Skip debounce on next prewarm check (set on map load).
    prewarm_immediate: bool,
    /// Last prewarm pass; passes are throttled since each hashes the whole map per view.
    prewarm_checked_at: std::time::Instant,
    /// Render caches listed as rebuilding in the status bar, refreshed on the same throttle.
    stale_renders: Vec<&'static str>,
    stale_renders_checked_at: std::time::Instant,

    // Session notes
    /// Markdown notes stored as .md files beside the save file.
    pub note_vault: crate::notes::NoteVault,
    pub notes_state: crate::ui::notes_panel::NotesPanelState,
    /// Debounce: write dirty notes to disk once typing pauses.
    notes_dirty_since: Option<std::time::Instant>,
}

impl Default for DungeonApp {
    fn default() -> Self {
        // Start loading the bestiary in the background
        let pending_monster_db = if let Some(dir) = find_bestiary_dir() {
            let (tx, rx) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let db = MonsterDatabase::load_from_directory(&dir);
                let _ = tx.send(db);
            });
            Some(rx)
        } else {
            eprintln!("No bestiary data directory found. Monster database will be empty.");
            eprintln!("Place 5e-Tools bestiary JSON files in data/bestiary/ or 5etools-src/data/bestiary/");
            None
        };

        let campaign = Campaign::default();
        let dungeon = campaign.active_dungeon().clone();
        let history = UndoHistory::new(&dungeon);
        let initial_hash = history.committed_hash();

        Self {
            campaign,
            dungeon,
            active_tab: Tab::Graph,
            graph_state: GraphEditorState::default(),
            spatial_state: SpatialViewState::default(),
            decor_state: DecorViewState::default(),
            encounters_state: EncountersViewState::default(),
            styled_state: StyledViewState::default(),
            last_graph_snapshot: 0,
            monster_db: MonsterDatabase::empty(),
            combat_stats_cache: CombatStatsCache::new(),

            presenting: false,
            presentation: None,
            presentation_view_state: PresentationViewState::default(),
            player_viewport_open: false,
            player_view_state: PlayerViewState::default(),
            player_viewport_initialized: false,
            server: None,
            server_port: 8080,
            last_server_push_hash: 0,
            pending_server_png: None,
            player_map_index: None,
            player_dungeon: None,
            player_presentation: None,
            annotation_mode: false,
            annotation_state: AnnotationModeState::default(),
            help_mode: false,
            pending_file_op: None,
            pending_monster_db,
            import_candidates: None,
            cloud_sync: crate::io::cloud_sync::load_state(),
            cloud_sync_enabled: false,
            pending_cloud_op: None,
            cloud_status: None,
            drive_file_list: None,
            pending_update_check: updater::check_for_update(),
            available_update: None,
            pending_update_apply: None,
            update_ready_to_restart: false,
            update_error: None,
            show_update_dialog: false,
            layout_error: None,
            last_update_check: std::time::Instant::now(),
            history,
            current_file: None,
            last_saved_hash: initial_hash,
            last_autosave: std::time::Instant::now(),
            autosave_due: false,
            last_prewarm_hash: 0,
            prewarm_hash_changed_at: std::time::Instant::now(),
            prewarm_immediate: false,
            prewarm_checked_at: std::time::Instant::now(),
            stale_renders: Vec::new(),
            stale_renders_checked_at: std::time::Instant::now(),
            note_vault: crate::notes::NoteVault::default(),
            notes_state: crate::ui::notes_panel::NotesPanelState::default(),
            notes_dirty_since: None,
            last_autosave_hash: initial_hash,
        }
    }
}

impl DungeonApp {
    /// Sync the campaign party into the working dungeon (before frame processing).
    fn sync_party_to_dungeon(&mut self) {
        self.dungeon.party = self.campaign.party.clone();
    }

    /// Sync the working dungeon's party back to campaign (after frame processing).
    fn sync_party_from_dungeon(&mut self) {
        self.campaign.party = self.dungeon.party.clone();
    }

    /// Sync the current working dungeon back into the campaign's map list.
    fn sync_dungeon_to_campaign(&mut self) {
        self.campaign.maps[self.campaign.active_map] = self.dungeon.clone();
        // Clear per-map party (it lives on campaign)
        self.campaign.maps[self.campaign.active_map].party.clear();
        // Bump version for conflict detection
        self.campaign.version += 1;
    }

    /// Load the active map from campaign into the working dungeon.
    fn load_dungeon_from_campaign(&mut self) {
        self.dungeon = self.campaign.active_dungeon().clone();
        self.dungeon.party = self.campaign.party.clone();
    }

    /// Switch to a different map in the campaign.
    fn switch_to_map(&mut self, index: usize) {
        if index == self.campaign.active_map || index >= self.campaign.maps.len() {
            return;
        }
        // Save current map back
        self.sync_party_from_dungeon();
        self.sync_dungeon_to_campaign();
        // Switch
        self.campaign.switch_map(index);
        self.load_dungeon_from_campaign();
        // Reset view state
        self.graph_state = GraphEditorState::default();
        self.spatial_state = SpatialViewState::default();
        self.decor_state = DecorViewState::default();
        self.styled_state = StyledViewState::default();
        self.presenting = false;
        self.presentation = None;
        self.last_graph_snapshot = self.graph_hash();
        self.history.reset(&self.dungeon);
    }

    /// Sync presentation state into dungeon.session so it persists on save.
    fn sync_session(&mut self) {
        if let Some(pres) = &self.presentation {
            self.dungeon.session = pres.snapshot_session(&self.dungeon);
        }
    }

    /// Load a campaign from JSON string (used by cloud sync download/open).
    fn load_campaign_from_json(&mut self, json: &str, success_msg: &str) {
        match crate::io::save_load::deserialize_campaign_json(json) {
            Ok(campaign) => {
                self.campaign = campaign;
                self.load_dungeon_from_campaign();
                self.graph_state = GraphEditorState::default();
                self.presenting = false;
                self.presentation = None;
                self.player_map_index = None;
                self.player_dungeon = None;
                self.player_presentation = None;
                self.last_graph_snapshot = self.graph_hash();
                self.history.reset(&self.dungeon);
                self.last_saved_hash = self.history.committed_hash();
                self.cloud_status = Some(success_msg.to_string());
            }
            Err(e) => {
                self.cloud_status = Some(format!("Failed to parse file: {}", e));
            }
        }
    }

    /// Trigger an async push to Google Drive.
    fn trigger_cloud_push(&mut self) {
        let json = match crate::io::save_load::serialize_campaign_json(&self.campaign) {
            Ok(j) => j,
            Err(e) => {
                self.cloud_status = Some(format!("Sync serialize error: {}", e));
                return;
            }
        };
        let state = self.cloud_sync.clone();
        let name = self.campaign.name.clone();
        let version = self.campaign.version;
        let rx = crate::io::cloud_sync::sync_push_async(state, json, name, version);
        let (tx2, rx2) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            if let Ok((result, new_state)) = rx.recv() {
                let _ = tx2.send(CloudSyncOp::SyncDone(result, new_state));
            }
        });
        self.pending_cloud_op = Some(rx2);
    }

    fn graph_hash(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        use std::collections::hash_map::DefaultHasher;
        let mut h = DefaultHasher::new();
        self.dungeon.graph.rooms.len().hash(&mut h);
        for r in &self.dungeon.graph.rooms {
            r.id.hash(&mut h);
            r.grid_size().hash(&mut h);
            r.tags.len().hash(&mut h);
            r.floor.hash(&mut h);
        }
        self.dungeon.graph.connections.len().hash(&mut h);
        for e in &self.dungeon.graph.connections {
            e.source_room_id.hash(&mut h);
            e.target_room_id.hash(&mut h);
            e.connection.id.hash(&mut h);
            e.connection.corridor_width.hash(&mut h);
            e.connection.min_length.hash(&mut h);
            e.connection.max_length.hash(&mut h);
            // A new angle setting re-routes the corridor
            e.connection.corridor_angle.hash(&mut h);
        }
        // Include group constraints so changes trigger re-solve
        self.dungeon.graph.groups.len().hash(&mut h);
        for g in &self.dungeon.graph.groups {
            g.id.hash(&mut h);
            g.room_ids.len().hash(&mut h);
            g.max_width.hash(&mut h);
            g.max_height.hash(&mut h);
        }
        h.finish()
    }

    /// Refuse to solve while the containment hierarchy has a cycle, which the
    /// solver cannot lay out. Returns true if the solve should be abandoned.
    fn blocked_by_containment_cycle(&mut self) -> bool {
        match self.dungeon.graph.containment_cycle() {
            Some(err) => {
                self.layout_error = Some(format!(
                    "{err}.\n\nFix the container assignment in the Groups section of the \
                     sidebar, then re-run the layout."
                ));
                true
            }
            None => false,
        }
    }

    /// Full re-solve: recomputes all room positions and corridors from scratch.
    pub fn solve_layout_full(&mut self) {
        if self.blocked_by_containment_cycle() {
            return;
        }
        self.layout_error = None;
        let old_bounds = self.dungeon.layout.as_ref()
            .map(|l| l.bounds.clone())
            .unwrap_or_default();
        // Rotation is set by hand, so a re-solve keeps each room's angle
        let old_rotations: std::collections::HashMap<String, f32> = self.dungeon.layout.iter()
            .flat_map(|l| l.rooms.iter())
            .filter(|rl| rl.is_rotated())
            .map(|rl| (rl.room_id.clone(), rl.rotation))
            .collect();
        match crate::solver::layout::solve_layout(
            &self.dungeon.graph,
            self.spatial_state.density_gap,
        ) {
            Ok(mut layout) => {
                layout.bounds = old_bounds;
                if !old_rotations.is_empty() {
                    for rl in &mut layout.rooms {
                        if let Some(&r) = old_rotations.get(&rl.room_id) {
                            rl.rotation = r;
                        }
                    }
                    layout.corridors = crate::solver::corridor::route_corridors(&self.dungeon.graph, &layout);
                    crate::solver::corridor::compute_wall_openings(&self.dungeon.graph, &mut layout);
                }
                self.dungeon.layout = Some(layout);
            }
            Err(e) => eprintln!("Layout solver error: {}", e),
        }
        // Clear cave cells so they regenerate with updated exits
        for room in &mut self.dungeon.graph.rooms {
            if room.shape == crate::model::RoomShape::Cave {
                if let Some(cave) = &mut room.cave_data {
                    cave.cells.clear();
                }
            }
        }
        self.generate_caves();
        self.recompute_cave_contours();
        self.last_graph_snapshot = self.graph_hash();
    }

    /// Incremental solve: keeps existing room positions, only places new rooms
    /// and re-routes corridors.
    fn solve_layout_incremental(&mut self) {
        if self.blocked_by_containment_cycle() {
            // Sync the snapshot so the auto-solve doesn't retry (and re-open the
            // dialog) on every frame while the cycle is still there.
            self.last_graph_snapshot = self.graph_hash();
            return;
        }
        self.layout_error = None;
        if let Some(existing) = &self.dungeon.layout {
            let old_bounds = existing.bounds.clone();
            match crate::solver::layout::solve_incremental(
                &self.dungeon.graph,
                existing,
                self.spatial_state.density_gap,
            ) {
                Ok(mut layout) => {
                    layout.bounds = old_bounds;
                    self.dungeon.layout = Some(layout);
                }
                Err(e) => eprintln!("Incremental layout error: {}", e),
            }
        } else {
            // No existing layout — do a full solve
            self.solve_layout_full();
            return; // full solve already generates caves
        }
        self.generate_caves();
        self.recompute_cave_contours();
        self.last_graph_snapshot = self.graph_hash();
    }

    /// Generate cave cell data for any cave rooms that need it.
    fn generate_caves(&mut self) {
        let Some(layout) = &self.dungeon.layout else { return };

        // First pass: initialize missing cave_data (mutable)
        for room in &mut self.dungeon.graph.rooms {
            if room.shape == crate::model::RoomShape::Cave && room.cave_data.is_none() {
                room.cave_data = Some(crate::model::CaveData {
                    cells: Vec::new(),
                    seed: rand::random(),
                    algorithm: crate::model::CaveAlgorithm::CellularAutomata,
                    density: 0.45,
                    smoothing_iterations: 4,
                    generation: 0,
                    contour_segments: Vec::new(),
                });
            }
        }

        // Second pass: collect tasks (immutable borrow)
        struct CaveTask {
            room_idx: usize,
            w: u32,
            h: u32,
            algorithm: crate::model::CaveAlgorithm,
            seed: u64,
            density: f32,
            smoothing_iterations: u32,
            exits: Vec<(u32, u32)>,
        }
        let mut tasks: Vec<CaveTask> = Vec::new();
        for (i, room) in self.dungeon.graph.rooms.iter().enumerate() {
            if room.shape != crate::model::RoomShape::Cave {
                continue;
            }
            let Some(cave) = &room.cave_data else { continue };
            if !cave.cells.is_empty() {
                continue;
            }
            let (w, h) = room.grid_size();
            let exits = crate::solver::cave_gen::compute_exit_cells(
                &room.id, layout, &self.dungeon.graph,
            );
            tasks.push(CaveTask {
                room_idx: i, w, h,
                algorithm: cave.algorithm,
                seed: cave.seed,
                density: cave.density,
                smoothing_iterations: cave.smoothing_iterations,
                exits,
            });
        }

        // Third pass: generate and store (mutable)
        for task in tasks {
            let cells = crate::solver::cave_gen::generate_cave(
                task.w, task.h, task.algorithm, task.seed,
                task.density, task.smoothing_iterations, &task.exits,
            );
            if let Some(cave) = self.dungeon.graph.rooms[task.room_idx].cave_data.as_mut() {
                cave.cells = cells;
                cave.generation += 1;
            }
        }
    }

    /// Recompute marching squares contour segments for all cave rooms.
    /// Must be called after cave generation or cell edits.
    pub fn recompute_cave_contours(&mut self) {
        let Some(layout) = &self.dungeon.layout else { return };
        let floor = crate::render::themed::build_floor_set(layout, &self.dungeon.graph);

        // Collect room indices and layouts for caves
        let cave_rooms: Vec<(usize, crate::model::RoomLayout)> = self.dungeon.graph.rooms.iter()
            .enumerate()
            .filter(|(_, r)| r.shape == crate::model::RoomShape::Cave
                && r.cave_data.as_ref().is_some_and(|c| !c.cells.is_empty()))
            .filter_map(|(i, r)| {
                layout.room_by_id(&r.id).map(|rl| (i, rl.clone()))
            })
            .collect();

        for (idx, rl) in cave_rooms {
            let room = &self.dungeon.graph.rooms[idx];
            let cave = room.cave_data.as_ref().unwrap();
            let segments = if rl.is_rotated() {
                // Trace in the room's own frame (neighbours don't line up with turned
                // cells), then turn the contour into place
                let local = crate::model::RoomLayout { x: 0, y: 0, rotation: 0.0, ..rl.clone() };
                let g = crate::util::GRID_PX;
                crate::solver::cave_gen::compute_contour_segments(&local, cave, &crate::util::CellSet::default())
                    .into_iter()
                    .map(|(x1, y1, x2, y2)| {
                        let (a, b) = (rl.to_world(x1 / g, y1 / g), rl.to_world(x2 / g, y2 / g));
                        (a.0 * g, a.1 * g, b.0 * g, b.1 * g)
                    })
                    .collect()
            } else {
                crate::solver::cave_gen::compute_contour_segments(&rl, cave, &floor)
            };
            self.dungeon.graph.rooms[idx].cave_data.as_mut().unwrap().contour_segments = segments;
        }
    }

    /// Snapshot what the web server's player PNG needs and return the job that renders
    /// and encodes it. On a large map that takes a second or more, so it runs on a
    /// background thread rather than stalling the session.
    fn player_png_job(&self) -> Option<impl FnOnce() -> Option<Vec<u8>> + Send + 'static> {
        // Use player-specific map if it differs from DM view
        let (dungeon, pres) = if self.player_map_index.is_some()
            && self.player_map_index != Some(self.campaign.active_map)
        {
            (self.player_dungeon.as_ref()?, self.player_presentation.as_ref()?)
        } else {
            (&self.dungeon, self.presentation.as_ref()?)
        };
        let layout = dungeon.layout.clone()?;
        // The radial renderer has no notion of carriers, so pin each light to where it
        // actually is (carried token, explicit position, or room center).
        let resolved_lights: Vec<crate::model::LightSource> = dungeon.light_sources.iter().map(|l| {
            let mut l = l.clone();
            l.pos = crate::presentation::lighting::light_origin(&l, dungeon, &layout);
            l
        }).collect();
        let snapshot = crate::presentation::PresentationSnapshot {
            room_visibility: pres.room_visibility.clone(),
            doors_open: pres.doors_open.clone(),
        };
        let graph = dungeon.graph.clone();
        let theme = dungeon.theme.clone();
        let ambient_light = dungeon.ambient_light;

        Some(move || {
            let (min_x, min_y, max_x, max_y) = layout.extents();
            let margin = 2;
            let grid_w = (max_x - min_x + margin * 2) as u32;
            let grid_h = (max_y - min_y + margin * 2) as u32;

            let scale_multiplier = 2u32;
            let grid_px = crate::util::GRID_PX;
            let scale = grid_px * scale_multiplier as f32;
            let width = (grid_w as f32 * scale) as u32;
            let height = (grid_h as f32 * scale) as u32;

            let mut renderer = crate::render::ImageRenderer::new(width, height, scale / grid_px);
            renderer.offset_x = (min_x - margin) as f32 * grid_px;
            renderer.offset_y = (min_y - margin) as f32 * grid_px;

            let options = crate::render::themed::RenderOptions {
                show_grid: true,
                show_labels: true,
                show_notes: false,
                show_secrets: false,
                show_decor: true,
                show_lighting: true,
            };
            crate::render::presentation::render_player_view_snapshot(
                &mut renderer,
                &graph,
                &layout,
                &theme,
                &snapshot,
                &resolved_lights,
                ambient_light,
                &options,
            );

            // Encode to PNG in memory
            let mut png_bytes = Vec::new();
            let encoder = image::codecs::png::PngEncoder::new(std::io::Cursor::new(&mut png_bytes));
            image::ImageEncoder::write_image(
                encoder,
                renderer.image.as_raw(),
                width,
                height,
                image::ExtendedColorType::Rgba8,
            ).ok()?;

            Some(png_bytes)
        })
    }

    /// Compute a hash of the presentation state to detect changes for server pushes.
    fn presentation_hash(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        use std::collections::hash_map::DefaultHasher;
        let Some(presentation) = &self.presentation else { return 0 };
        let mut h = DefaultHasher::new();
        for (room_id, vis) in &presentation.room_visibility {
            room_id.hash(&mut h);
            std::mem::discriminant(vis).hash(&mut h);
        }
        presentation.doors_open.len().hash(&mut h);
        for conn_id in &presentation.doors_open {
            conn_id.hash(&mut h);
        }
        self.dungeon.light_sources.len().hash(&mut h);
        for light in &self.dungeon.light_sources {
            light.id.hash(&mut h);
            light.radius.to_bits().hash(&mut h);
            light.intensity.to_bits().hash(&mut h);
            light.room_id.hash(&mut h);
        }
        self.dungeon.ambient_light.to_bits().hash(&mut h);
        presentation.encounter_positions.len().hash(&mut h);
        for (eid, rid) in &presentation.encounter_positions {
            eid.hash(&mut h);
            rid.hash(&mut h);
        }
        presentation.party_room.hash(&mut h);
        h.finish()
    }

    /// Get the name of the currently active view for annotation metadata.
    fn current_view_name(&self) -> String {
        if self.presenting {
            "Presentation".to_string()
        } else {
            match self.active_tab {
                Tab::Graph => "Graph".to_string(),
                Tab::Spatial => "Spatial".to_string(),
                Tab::Decor => "Decor".to_string(),
                Tab::Encounters => "Encounters".to_string(),
                Tab::Styled => "Styled".to_string(),
            }
        }
    }

    /// Get the current view's pan/zoom state.
    fn current_view_state(&self) -> &crate::ui::canvas_common::ViewState {
        if self.presenting {
            &self.presentation_view_state.view
        } else {
            match self.active_tab {
                Tab::Graph => &self.graph_state.view,
                Tab::Spatial => &self.spatial_state.view,
                Tab::Decor => &self.decor_state.view,
                Tab::Encounters => &self.encounters_state.view,
                Tab::Styled => &self.styled_state.view,
            }
        }
    }

    /// Called after undo/redo restores a dungeon state. Syncs derived/view state.
    fn after_history_restore(&mut self, ctx: &egui::Context) {
        // Clear graph editor positions so they reload from restored dungeon
        self.graph_state.room_positions.clear();
        // Clear selections (referenced items may no longer exist)
        self.graph_state.selection = Default::default();
        self.graph_state.drag_state = crate::ui::graph_editor::DragState::None;
        self.spatial_state.selected_room = None;
        self.spatial_state.selected_corridor = None;
        self.spatial_state.selected_waypoint = None;
        self.spatial_state.selected_group = None;
        self.spatial_state.selected_section = None;
        self.decor_state.selected_room = None;
        self.decor_state.selected_decor = None;
        // Sync graph hash so auto-solve doesn't trigger inappropriately
        self.last_graph_snapshot = self.graph_hash();
        // Recompute cave contours (they're skipped in serialization)
        self.recompute_cave_contours();
        ctx.request_repaint();
    }

    fn push_server_update_if_changed(&mut self, ctx: &egui::Context) {
        // One render at a time; changes made meanwhile are picked up once it finishes.
        if let Some(rx) = &self.pending_server_png {
            if matches!(rx.try_recv(), Err(std::sync::mpsc::TryRecvError::Empty)) {
                ctx.request_repaint_after(std::time::Duration::from_millis(100));
                return;
            }
            self.pending_server_png = None;
        }
        let hash = self.presentation_hash();
        if hash == self.last_server_push_hash {
            return;
        }
        let Some(server) = &self.server else { return };
        let Some(job) = self.player_png_job() else { return };
        let sender = server.update_sender();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            if let Some(png) = job() {
                let _ = sender.send(png);
            }
            let _ = done_tx.send(());
        });
        self.pending_server_png = Some(done_rx);
        self.last_server_push_hash = hash;
    }

    /// Draw every managed standalone window (edit-mode and presentation) and act on any
    /// file request they raised. Independent of the active tab so windows survive tab switches.
    fn draw_dock_windows(&mut self, ctx: &egui::Context) {
        encounters_view::draw_windows(
            ctx,
            &mut self.dungeon,
            &self.monster_db,
            &mut self.combat_stats_cache,
            &mut self.encounters_state,
        );
        if self.presenting {
            if let Some(presentation) = &mut self.presentation {
                presentation_view::draw_windows(
                    ctx,
                    &mut self.dungeon,
                    presentation,
                    &self.monster_db,
                    &mut self.combat_stats_cache,
                );
            }
        }
        crate::ui::notes_panel::note_browser_window(ctx, &mut self.note_vault, &mut self.notes_state);
        self.dispatch_encounter_file_request();
    }

    /// Reconcile the note vault with the campaign (legacy migration + renames).
    /// The working dungeon is folded back in first so its rooms are seen.
    fn sync_notes(&mut self) {
        let active = self.campaign.active_map;
        if active < self.campaign.maps.len() {
            let party = std::mem::take(&mut self.campaign.maps[active].party);
            self.campaign.maps[active] = self.dungeon.clone();
            self.campaign.maps[active].party = party;
        }
        let mut campaign = std::mem::replace(&mut self.campaign, Campaign::new(String::new()));
        self.note_vault.sync(&mut campaign);
        self.campaign = campaign;
        // Sync rewrites each room's derived note excerpt; pull that into the working copy.
        self.load_dungeon_from_campaign();
        self.note_vault.flush();
    }

    /// Which entity the notes drawer should show notes for right now.
    fn note_context(&self) -> crate::ui::notes_panel::NoteContext {
        use crate::notes::NoteBind;
        use crate::ui::notes_panel::NoteContext;

        let selected = if self.presenting {
            self.presentation_view_state.selected_room.clone()
        } else {
            match self.active_tab {
                Tab::Spatial => self.spatial_state.selected_room.clone(),
                Tab::Decor => self.decor_state.selected_room.clone(),
                Tab::Encounters => self.encounters_state.selected_room.clone(),
                Tab::Graph => {
                    let rooms = &self.graph_state.selection.rooms;
                    (rooms.len() == 1).then(|| rooms.iter().next().cloned()).flatten()
                }
                Tab::Styled => None,
            }
        };

        let map_folder = Some(self.dungeon.name.clone());
        match selected.and_then(|id| self.dungeon.graph.room_by_id(&id).map(|r| (id, r.label.clone()))) {
            Some((id, label)) => NoteContext {
                bind: Some(NoteBind::Room(id)),
                title: label,
                map_folder,
            },
            // With nothing selected the drawer falls back to the map's own note,
            // which is where campaign-wide prep and the session log live.
            None => NoteContext {
                bind: Some(NoteBind::Map(self.dungeon.id.clone())),
                title: self.dungeon.name.clone(),
                map_folder,
            },
        }
    }

    fn dispatch_encounter_file_request(&mut self) {
        // Dispatch encounter/creature file ops
        if let Some(req) = self.encounters_state.file_request.take() {
            if self.pending_file_op.is_none() {
                use encounters_view::EncounterFileRequest;
                self.pending_file_op = Some(match req {
                    EncounterFileRequest::ExportEncounter(idx) => {
                        let slice = if idx < self.dungeon.encounters.len() {
                            &self.dungeon.encounters[idx..idx+1]
                        } else {
                            &[]
                        };
                        crate::io::save_load::export_encounters_async(
                            slice,
                            &self.dungeon.custom_monsters,
                        )
                    }
                    EncounterFileRequest::ImportEncounters { target_room } => {
                        self.encounters_state.import_target_room = target_room;
                        crate::io::save_load::import_encounters_async()
                    }
                    EncounterFileRequest::ExportCreatures => {
                        crate::io::save_load::export_creatures_async(
                            &self.dungeon.custom_monsters,
                        )
                    }
                    EncounterFileRequest::ImportCreatures => {
                        crate::io::save_load::import_creatures_async()
                    }
                });
            }
        }
    }
}

impl eframe::App for DungeonApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Keep the event loop alive so the Wayland compositor doesn't mark us unresponsive
        ctx.request_repaint_after(std::time::Duration::from_secs(2));

        // Sync campaign party into working dungeon at frame start
        self.sync_party_to_dungeon();

        self.poll_background_tasks();
        self.show_import_dialogs(ctx);
        // Pre-warm render caches for all views (debounced, runs before UI so status bar sees pending state)
        self.prewarm_render_caches(ctx);

        self.handle_global_keys(ctx);
        // Collect panel rects for annotation spotlight
        self.annotation_state.panel_rects.clear();

        self.show_menu_bar(ctx);
        self.show_map_tabs(ctx);
        self.process_layout_requests(ctx);
        self.show_status_bar(ctx);
        self.show_dialogs(ctx);
        self.show_side_panels(ctx);
        let canvas_rect = self.show_canvas(ctx);
        self.show_overlays(ctx, canvas_rect);
        self.end_frame(ctx);
    }
}

impl DungeonApp {
    /// Poll the background jobs: bestiary load, file operations, cloud sync and updates.
    fn poll_background_tasks(&mut self) {
        // Poll background monster database load
        if let Some(rx) = &self.pending_monster_db {
            if let Ok(db) = rx.try_recv() {
                self.monster_db = db;
                self.pending_monster_db = None;
            }
        }

        // Poll pending async file operation
        if let Some(rx) = &self.pending_file_op {
            if let Ok(result) = rx.try_recv() {
                use crate::io::save_load::FileOpResult;
                match result {
                    FileOpResult::Loaded(Ok((campaign, path))) => {
                        self.campaign = campaign;
                        self.load_dungeon_from_campaign();
                        self.graph_state = GraphEditorState::default();
                        self.presenting = false;
                        self.presentation = None;
                        self.player_map_index = None;
                        self.player_dungeon = None;
                        self.player_presentation = None;
                        // Sync snapshot so auto-solve doesn't re-route saved corridors
                        self.last_graph_snapshot = self.graph_hash();
                        self.history.reset(&self.dungeon);
                        self.note_vault.attach(&path);
                        self.current_file = Some(path);
                        self.last_saved_hash = self.history.committed_hash();
                        self.sync_notes();
                        // Trigger immediate render cache pre-warming (skip debounce)
                        self.prewarm_immediate = true;
                    }
                    FileOpResult::Loaded(Err(e)) => eprintln!("Load error: {}", e),
                    FileOpResult::Saved(Ok(path)) => {
                        self.note_vault.attach(&path);
                        self.current_file = Some(path);
                        self.last_saved_hash = self.history.committed_hash();
                        self.sync_notes();
                        self.note_vault.flush();
                        if self.update_ready_to_restart {
                            restart_app();
                        }
                        // Auto-push to Drive if sync is enabled
                        if self.cloud_sync_enabled && self.cloud_sync.is_logged_in() && self.pending_cloud_op.is_none() {
                            self.trigger_cloud_push();
                        }
                    }
                    FileOpResult::Saved(Err(e)) => eprintln!("Save error: {}", e),
                    FileOpResult::ExportedPng(Ok(())) => {}
                    FileOpResult::ExportedPng(Err(e)) => eprintln!("Export error: {}", e),
                    FileOpResult::ExportedEncounters(Ok(())) => {}
                    FileOpResult::ExportedEncounters(Err(e)) => eprintln!("Encounter export error: {}", e),
                    FileOpResult::ImportedEncounters(Ok(data)) => {
                        let target_room = self.encounters_state.import_target_room.take();
                        let fallback_room = self.dungeon.graph.rooms.first()
                            .map(|r| r.id.clone())
                            .unwrap_or_default();
                        for mut enc in data.encounters {
                            if let Some(ref room) = target_room {
                                // User chose a specific room to import into
                                enc.home_room_id = room.clone();
                            } else if self.dungeon.graph.room_by_id(&enc.home_room_id).is_none() {
                                enc.home_room_id = fallback_room.clone();
                            }
                            enc.id = uuid::Uuid::new_v4().to_string();
                            self.dungeon.encounters.push(enc);
                        }
                        // Merge custom monsters, skipping duplicates by id
                        let existing_ids: std::collections::HashSet<String> = self.dungeon.custom_monsters.iter()
                            .map(|cm| cm.id.clone()).collect();
                        for cm in data.custom_monsters {
                            if !existing_ids.contains(&cm.id) {
                                self.dungeon.custom_monsters.push(cm);
                            }
                        }
                    }
                    FileOpResult::ImportedEncounters(Err(e)) => eprintln!("Encounter import error: {}", e),
                    FileOpResult::ExportedCreatures(Ok(())) => {}
                    FileOpResult::ExportedCreatures(Err(e)) => eprintln!("Creature export error: {}", e),
                    FileOpResult::ImportedCreatures(Ok(creatures)) => {
                        let existing_ids: std::collections::HashSet<String> = self.dungeon.custom_monsters.iter()
                            .map(|cm| cm.id.clone()).collect();
                        for cm in creatures {
                            if !existing_ids.contains(&cm.id) {
                                self.dungeon.custom_monsters.push(cm);
                            }
                        }
                    }
                    FileOpResult::ImportedCreatures(Err(e)) => eprintln!("Creature import error: {}", e),
                    FileOpResult::ImportedMap(Ok(source_campaign)) => {
                        self.import_candidates = Some(source_campaign);
                    }
                    FileOpResult::ImportedMap(Err(e)) => eprintln!("Map import error: {}", e),
                    FileOpResult::Cancelled => {}
                }
                self.pending_file_op = None;
            }
        }

        // Poll pending cloud sync operation
        if let Some(rx) = &self.pending_cloud_op {
            if let Ok(op) = rx.try_recv() {
                match op {
                    CloudSyncOp::Login(crate::io::cloud_sync::LoginResult::Success(tokens)) => {
                        self.cloud_sync.tokens = Some(tokens);
                        crate::io::cloud_sync::save_state(&self.cloud_sync);
                        self.cloud_status = Some("Logged in to Google Drive".into());
                    }
                    CloudSyncOp::Login(crate::io::cloud_sync::LoginResult::Error(e)) => {
                        self.cloud_status = Some(format!("Login failed: {}", e));
                    }
                    CloudSyncOp::SyncDone(result, new_state) => {
                        self.cloud_sync = new_state;
                        match result {
                            crate::io::cloud_sync::SyncResult::Uploaded => {
                                self.cloud_status = Some("Synced to Drive".into());
                            }
                            crate::io::cloud_sync::SyncResult::Downloaded(json) => {
                                self.load_campaign_from_json(&json, "Downloaded newer version from Drive");
                            }
                            crate::io::cloud_sync::SyncResult::Conflict { local_version, remote_version } => {
                                self.cloud_status = Some(format!(
                                    "Conflict! Local v{} vs remote v{}. Pull to get remote version, or force push to overwrite.",
                                    local_version, remote_version
                                ));
                            }
                            crate::io::cloud_sync::SyncResult::NotLoggedIn => {
                                self.cloud_sync_enabled = false;
                                self.cloud_status = Some("Not logged in".into());
                            }
                            crate::io::cloud_sync::SyncResult::Error(e) => {
                                self.cloud_status = Some(format!("Sync error: {}", e));
                            }
                        }
                    }
                    CloudSyncOp::FileList(Ok(files), new_state) => {
                        self.cloud_sync = new_state;
                        self.drive_file_list = Some(files);
                    }
                    CloudSyncOp::FileList(Err(e), new_state) => {
                        self.cloud_sync = new_state;
                        self.cloud_status = Some(format!("Failed to list Drive files: {}", e));
                    }
                    CloudSyncOp::Opened(Ok(json), new_state) => {
                        self.cloud_sync = new_state;
                        self.cloud_sync_enabled = true;
                        self.load_campaign_from_json(&json, "Opened from Drive");
                    }
                    CloudSyncOp::Opened(Err(e), new_state) => {
                        self.cloud_sync = new_state;
                        self.cloud_status = Some(format!("Failed to open from Drive: {}", e));
                    }
                }
                self.pending_cloud_op = None;
            }
        }

        // Poll update check
        if let Some(rx) = &self.pending_update_check {
            if let Ok(status) = rx.try_recv() {
                self.last_update_check = std::time::Instant::now();
                match status {
                    updater::UpdateStatus::Available(info) => {
                        self.available_update = Some(info);
                    }
                    updater::UpdateStatus::Error(e) => {
                        eprintln!("Update check error: {}", e);
                    }
                    updater::UpdateStatus::NoUpdate => {}
                }
                self.pending_update_check = None;
            }
        } else if self.available_update.is_none()
            && !self.update_ready_to_restart
            && self.last_update_check.elapsed() >= std::time::Duration::from_secs(60)
        {
            self.pending_update_check = updater::check_for_update();
        }

        // Poll update apply
        if let Some(rx) = &self.pending_update_apply {
            if let Ok(status) = rx.try_recv() {
                match status {
                    updater::ApplyStatus::Success => {
                        let has_unsaved = self.history.committed_hash() != self.last_saved_hash;
                        if has_unsaved {
                            self.update_ready_to_restart = true;
                        } else {
                            restart_app();
                        }
                    }
                    updater::ApplyStatus::Error(e) => {
                        self.update_error = Some(e);
                    }
                }
                self.pending_update_apply = None;
            }
        }
    }

    /// The import-map and Drive file-picker dialogs, while open.
    fn show_import_dialogs(&mut self, ctx: &egui::Context) {
        // Import map dialog
        if self.import_candidates.is_some() {
            let mut close_dialog = false;
            let mut import_indices: Vec<usize> = Vec::new();
            egui::Window::new("Import Maps")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    let source = self.import_candidates.as_ref().unwrap();
                    ui.label(format!("Source: {} ({} map{})", source.name, source.maps.len(), if source.maps.len() == 1 { "" } else { "s" }));
                    ui.separator();
                    for (i, map) in source.maps.iter().enumerate() {
                        if ui.button(format!("Import \"{}\"", map.name)).clicked() {
                            import_indices.push(i);
                        }
                    }
                    ui.separator();
                    if ui.button("Import All").clicked() {
                        import_indices = (0..source.maps.len()).collect();
                    }
                    if ui.button("Cancel").clicked() {
                        close_dialog = true;
                    }
                });
            if !import_indices.is_empty() {
                self.sync_party_from_dungeon();
                self.sync_dungeon_to_campaign();
                let source = self.import_candidates.take().unwrap();
                for i in import_indices {
                    if let Some(map) = source.maps.get(i) {
                        self.campaign.import_dungeon(map.clone());
                    }
                }
                // Also merge source campaign party
                self.campaign.merge_party(source.party.clone());
                self.sync_party_to_dungeon();
            } else if close_dialog {
                self.import_candidates = None;
            }
        }

        // Drive file picker dialog
        if self.drive_file_list.is_some() {
            let mut close_dialog = false;
            let mut open_file_id = None;
            egui::Window::new("Open from Drive")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    let files = self.drive_file_list.as_ref().unwrap();
                    if files.is_empty() {
                        ui.label("No campaigns found in Drive.");
                    } else {
                        for file in files {
                            if ui.button(&file.name).clicked() {
                                open_file_id = Some(file.id.clone());
                            }
                        }
                    }
                    ui.separator();
                    if ui.button("Cancel").clicked() {
                        close_dialog = true;
                    }
                });
            if let Some(file_id) = open_file_id {
                self.drive_file_list = None;
                let state = self.cloud_sync.clone();
                let rx = crate::io::cloud_sync::open_from_drive_async(state, file_id);
                let (tx2, rx2) = std::sync::mpsc::channel();
                std::thread::spawn(move || {
                    if let Ok((result, new_state)) = rx.recv() {
                        let _ = tx2.send(CloudSyncOp::Opened(result, new_state));
                    }
                });
                self.pending_cloud_op = Some(rx2);
            } else if close_dialog {
                self.drive_file_list = None;
            }
        }
    }

    /// Global shortcuts: undo/redo, save, and the annotation (F7), notes (F9) and help (F8) toggles.
    fn handle_global_keys(&mut self, ctx: &egui::Context) {
        // Global keys: Ctrl+Z undo, Ctrl+Y / Ctrl+Shift+Z redo, Ctrl+S save
        let (undo_pressed, redo_pressed, save_pressed) = ctx.input(|i| {
            let ctrl = i.modifiers.command; // Cmd on Mac, Ctrl on others
            let shift = i.modifiers.shift;
            let undo = ctrl && !shift && i.key_pressed(egui::Key::Z);
            let redo = (ctrl && i.key_pressed(egui::Key::Y))
                || (ctrl && shift && i.key_pressed(egui::Key::Z));
            let save = ctrl && !shift && i.key_pressed(egui::Key::S);
            (undo, redo, save)
        });
        if undo_pressed {
            if self.history.undo(&mut self.dungeon) {
                self.after_history_restore(ctx);
            }
        } else if redo_pressed {
            if self.history.redo(&mut self.dungeon) {
                self.after_history_restore(ctx);
            }
        }
        // Ctrl+S: save to current file or open Save As dialog
        if save_pressed && self.pending_file_op.is_none() {
            self.sync_session();
            self.sync_party_from_dungeon();
            self.sync_dungeon_to_campaign();
            if let Some(path) = &self.current_file {
                self.pending_file_op = Some(
                    crate::io::save_load::save_campaign_to_path(&self.campaign, path.clone()),
                );
            } else {
                self.pending_file_op = Some(
                    crate::io::save_load::save_campaign_async(&self.campaign),
                );
            }
        }

        // Global key: F7 toggles annotation mode
        let f7_pressed = ctx.input(|i| i.key_pressed(egui::Key::F7));
        if f7_pressed {
            self.annotation_mode = !self.annotation_mode;
            self.help_mode = false;
            self.annotation_state.composing = None;
            self.annotation_state.viewing = None;
        }

        // Global key: F9 cycles the notes drawer (expanded -> collapsed -> hidden)
        if ctx.input(|i| i.key_pressed(egui::Key::F9)) {
            self.notes_state.toggle();
        }

        // Global key: F8 toggles help overlay
        let f8_pressed = ctx.input(|i| i.key_pressed(egui::Key::F8));
        if f8_pressed {
            self.help_mode = !self.help_mode;
            self.annotation_mode = false;
        }
    }

    /// The top menu bar.
    fn show_menu_bar(&mut self, ctx: &egui::Context) {
        // Top menu bar
        let menu_response = egui::TopBottomPanel::top("menu_bar").show(ctx, |ui| {
            egui::menu::bar(ui, |ui| {
                ui.menu_button("File", |ui| {
                    if ui.button("New").clicked() {
                        self.campaign = Campaign::default();
                        self.dungeon = self.campaign.active_dungeon().clone();
                        self.graph_state = GraphEditorState::default();
                        self.spatial_state = SpatialViewState::default();
                        self.decor_state = DecorViewState::default();
                        self.styled_state = StyledViewState::default();
                        self.presenting = false;
                        self.presentation = None;
                        self.player_map_index = None;
                        self.player_dungeon = None;
                        self.player_presentation = None;
                        self.history.reset(&self.dungeon);
                        self.current_file = None;
                        self.note_vault.reset();
                        self.notes_state = crate::ui::notes_panel::NotesPanelState::default();
                        self.last_saved_hash = 0;
                        ui.close_menu();
                    }
                    if ui.button("Open...").clicked() {
                        if self.pending_file_op.is_none() {
                            self.pending_file_op = Some(crate::io::save_load::load_campaign_async());
                        }
                        ui.close_menu();
                    }
                    if ui.button("Save  Ctrl+S").clicked() {
                        if self.pending_file_op.is_none() {
                            self.sync_session();
                            self.sync_party_from_dungeon();
                            self.sync_dungeon_to_campaign();
                            if let Some(path) = &self.current_file {
                                self.pending_file_op = Some(
                                    crate::io::save_load::save_campaign_to_path(&self.campaign, path.clone()),
                                );
                            } else {
                                self.pending_file_op = Some(
                                    crate::io::save_load::save_campaign_async(&self.campaign),
                                );
                            }
                        }
                        ui.close_menu();
                    }
                    if ui.button("Save As...").clicked() {
                        if self.pending_file_op.is_none() {
                            self.sync_session();
                            self.sync_party_from_dungeon();
                            self.sync_dungeon_to_campaign();
                            self.pending_file_op = Some(crate::io::save_load::save_campaign_async(&self.campaign));
                        }
                        ui.close_menu();
                    }
                    // Cloud sync section
                    if crate::io::cloud_sync::is_available() {
                        ui.separator();
                        if self.cloud_sync.is_logged_in() {
                            ui.checkbox(&mut self.cloud_sync_enabled, "Cloud Sync");
                            if self.pending_cloud_op.is_none() {
                                if ui.button("Open from Drive...").clicked() {
                                    let state = self.cloud_sync.clone();
                                    let rx = crate::io::cloud_sync::list_drive_files_async(state);
                                    let (tx2, rx2) = std::sync::mpsc::channel();
                                    std::thread::spawn(move || {
                                        if let Ok((result, new_state)) = rx.recv() {
                                            let _ = tx2.send(CloudSyncOp::FileList(result, new_state));
                                        }
                                    });
                                    self.pending_cloud_op = Some(rx2);
                                    ui.close_menu();
                                }
                                if self.cloud_sync_enabled && self.cloud_sync.drive_file_id.is_some() {
                                    if ui.button("Pull from Drive").clicked() {
                                        let state = self.cloud_sync.clone();
                                        let version = self.campaign.version;
                                        let rx = crate::io::cloud_sync::sync_pull_async(state, version);
                                        let (tx2, rx2) = std::sync::mpsc::channel();
                                        std::thread::spawn(move || {
                                            if let Ok((result, new_state)) = rx.recv() {
                                                let _ = tx2.send(CloudSyncOp::SyncDone(result, new_state));
                                            }
                                        });
                                        self.pending_cloud_op = Some(rx2);
                                        ui.close_menu();
                                    }
                                }
                                if ui.button("Logout").clicked() {
                                    self.cloud_sync.logout();
                                    self.cloud_sync_enabled = false;
                                    self.cloud_status = Some("Logged out".into());
                                    ui.close_menu();
                                }
                            }
                        } else if self.pending_cloud_op.is_none() {
                            if ui.button("Login to Google Drive").clicked() {
                                let rx = crate::io::cloud_sync::login_async();
                                let (tx2, rx2) = std::sync::mpsc::channel();
                                std::thread::spawn(move || {
                                    if let Ok(result) = rx.recv() {
                                        let _ = tx2.send(CloudSyncOp::Login(result));
                                    }
                                });
                                self.pending_cloud_op = Some(rx2);
                                ui.close_menu();
                            }
                        } else {
                            ui.label("Working...");
                        }
                    }
                });
                ui.menu_button("Edit", |ui| {
                    if ui.add_enabled(self.history.can_undo(), egui::Button::new("Undo  Ctrl+Z")).clicked() {
                        self.history.undo(&mut self.dungeon);
                        self.graph_state.room_positions.clear();
                        self.graph_state.selection = Default::default();
                        self.graph_state.drag_state = crate::ui::graph_editor::DragState::None;
                        self.spatial_state.selected_room = None;
                        self.spatial_state.selected_corridor = None;
                        self.spatial_state.selected_waypoint = None;
                        self.spatial_state.selected_group = None;
                        self.spatial_state.selected_section = None;
                        self.decor_state.selected_room = None;
                        self.decor_state.selected_decor = None;
                        self.last_graph_snapshot = self.graph_hash();
                        ui.close_menu();
                    }
                    if ui.add_enabled(self.history.can_redo(), egui::Button::new("Redo  Ctrl+Y")).clicked() {
                        self.history.redo(&mut self.dungeon);
                        self.graph_state.room_positions.clear();
                        self.graph_state.selection = Default::default();
                        self.graph_state.drag_state = crate::ui::graph_editor::DragState::None;
                        self.spatial_state.selected_room = None;
                        self.spatial_state.selected_corridor = None;
                        self.spatial_state.selected_waypoint = None;
                        self.spatial_state.selected_group = None;
                        self.spatial_state.selected_section = None;
                        self.decor_state.selected_room = None;
                        self.decor_state.selected_decor = None;
                        self.last_graph_snapshot = self.graph_hash();
                        ui.close_menu();
                    }
                });
                ui.menu_button("Campaign", |ui| {
                    ui.horizontal(|ui| {
                        ui.label("Name:");
                        ui.text_edit_singleline(&mut self.campaign.name);
                    });
                    ui.separator();
                    if ui.button("New Map").clicked() {
                        self.sync_party_from_dungeon();
                        self.sync_dungeon_to_campaign();
                        let map_name = format!("Map {}", self.campaign.maps.len() + 1);
                        self.campaign.add_map(map_name);
                        let new_idx = self.campaign.maps.len() - 1;
                        self.campaign.switch_map(new_idx);
                        self.load_dungeon_from_campaign();
                        self.graph_state = GraphEditorState::default();
                        self.spatial_state = SpatialViewState::default();
                        self.decor_state = DecorViewState::default();
                        self.styled_state = StyledViewState::default();
                        self.presenting = false;
                        self.presentation = None;
                        self.last_graph_snapshot = self.graph_hash();
                        self.history.reset(&self.dungeon);
                        ui.close_menu();
                    }
                    if ui.button("Import Map...").clicked() {
                        if self.pending_file_op.is_none() {
                            self.pending_file_op = Some(crate::io::save_load::import_map_async());
                        }
                        ui.close_menu();
                    }
                    if self.campaign.maps.len() > 1 {
                        if ui.button("Remove Current Map").clicked() {
                            self.sync_party_from_dungeon();
                            self.sync_dungeon_to_campaign();
                            let idx = self.campaign.active_map;
                            self.campaign.remove_map(idx);
                            self.load_dungeon_from_campaign();
                            self.graph_state = GraphEditorState::default();
                            self.spatial_state = SpatialViewState::default();
                            self.decor_state = DecorViewState::default();
                            self.styled_state = StyledViewState::default();
                            self.presenting = false;
                            self.presentation = None;
                            self.last_graph_snapshot = self.graph_hash();
                            self.history.reset(&self.dungeon);
                            ui.close_menu();
                        }
                    }
                    ui.separator();
                    ui.label("Maps:");
                    let active = self.campaign.active_map;
                    let mut switch_to = None;
                    for (i, map) in self.campaign.maps.iter().enumerate() {
                        let label = if i == active {
                            format!("> {}", map.name)
                        } else {
                            map.name.clone()
                        };
                        if ui.selectable_label(i == active, &label).clicked() && i != active {
                            switch_to = Some(i);
                            ui.close_menu();
                        }
                    }
                    if let Some(idx) = switch_to {
                        self.switch_to_map(idx);
                    }
                });

                ui.separator();

                if self.presenting {
                    // In presentation mode, show only a "Stop Presenting" button
                    if ui.button("Stop Presenting").clicked() {
                        self.presenting = false;
                        self.player_viewport_open = false;
                        self.player_viewport_initialized = false;
                        crate::ui::window_dock::WindowDock::close_presentation_windows(ui.ctx());
                        self.player_map_index = None;
                        self.player_dungeon = None;
                        self.player_presentation = None;
                        if let Some(server) = &mut self.server {
                            server.stop();
                        }
                        self.server = None;
                    }
                } else {
                    // Normal tab buttons
                    ui.selectable_value(&mut self.active_tab, Tab::Graph, "Graph");
                    ui.selectable_value(&mut self.active_tab, Tab::Spatial, "Spatial");
                    ui.selectable_value(&mut self.active_tab, Tab::Decor, "Decor");
                    ui.selectable_value(&mut self.active_tab, Tab::Encounters, "Encounters");
                    ui.selectable_value(&mut self.active_tab, Tab::Styled, "Styled");

                    ui.separator();

                    if ui.button("Present").clicked() {
                        self.presenting = true;
                        if self.presentation.is_none() {
                            self.presentation = Some(PresentationState::new_from_dungeon(&self.dungeon));
                        }
                        // Ensure layout exists
                        if self.dungeon.layout.is_none() && !self.dungeon.graph.rooms.is_empty() {
                            self.solve_layout_full();
                        }
                    }
                }

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if self.presenting {
                        ui.colored_label(egui::Color32::from_rgb(255, 100, 100), "PRESENTING");
                        ui.separator();
                    }
                    let unsaved = self.history.committed_hash() != self.last_saved_hash;
                    let title = if self.campaign.maps.len() > 1 {
                        let base = format!("{} - {}", self.campaign.name, self.dungeon.name);
                        if unsaved { format!("{} *", base) } else { base }
                    } else {
                        if unsaved { format!("{} *", self.dungeon.name) } else { self.dungeon.name.clone() }
                    };
                    ui.label(&title);
                });
            });
        });
        self.annotation_state.panel_rects.push(menu_response.response.rect);
    }

    /// The map tab strip, when the campaign has several maps.
    fn show_map_tabs(&mut self, ctx: &egui::Context) {
        // Map tab strip (shown when campaign has multiple maps)
        if self.campaign.maps.len() > 1 {
            let mut switch_to = None;
            let mut push_to_players = false;
            egui::TopBottomPanel::top("map_tabs").show(ctx, |ui| {
                ui.horizontal(|ui| {
                    let player_idx = self.player_map_index.unwrap_or(self.campaign.active_map);
                    for (i, map) in self.campaign.maps.iter().enumerate() {
                        let is_active = i == self.campaign.active_map;
                        let is_player = self.presenting && i == player_idx;
                        let label = if is_player && !is_active {
                            format!("{} [player]", map.name)
                        } else if is_player {
                            format!("{} [player]", map.name)
                        } else {
                            map.name.clone()
                        };
                        if ui.selectable_label(is_active, &label).clicked() && !is_active {
                            switch_to = Some(i);
                        }
                    }
                    if self.presenting {
                        ui.separator();
                        let dm_is_player = self.player_map_index.is_none()
                            || self.player_map_index == Some(self.campaign.active_map);
                        if !dm_is_player {
                            if ui.button("Show to Players").clicked() {
                                push_to_players = true;
                            }
                        }
                    }
                });
            });
            if push_to_players {
                // Push current DM map to player view
                self.player_map_index = Some(self.campaign.active_map);
                self.player_dungeon = Some(self.dungeon.clone());
                self.player_presentation = self.presentation.clone();
            }
            if let Some(idx) = switch_to {
                if self.presenting {
                    // During presentation, switching DM map doesn't affect player view
                    // If player was following DM, freeze it on the old map first
                    if self.player_map_index.is_none() {
                        self.player_map_index = Some(self.campaign.active_map);
                        self.player_dungeon = Some(self.dungeon.clone());
                        self.player_presentation = self.presentation.clone();
                    }
                    self.switch_to_map(idx);
                    // Re-enter presentation on new map
                    self.presenting = true;
                    self.presentation = Some(PresentationState::new_from_dungeon(&self.dungeon));
                    if self.dungeon.layout.is_none() && !self.dungeon.graph.rooms.is_empty() {
                        self.solve_layout_full();
                    }
                } else {
                    self.switch_to_map(idx);
                }
            }
        }
    }

    /// Act on layout work requested this frame: full re-solves, cave regeneration and contours, and the auto-solve after graph edits.
    fn process_layout_requests(&mut self, ctx: &egui::Context) {
        // Handle "Recompute All" request from sidebar
        if self.spatial_state.recompute_requested {
            self.spatial_state.recompute_requested = false;
            self.solve_layout_full();
            ctx.request_repaint();
        }
        // Recompute cave contours after cell edits
        if self.spatial_state.cave_contours_dirty {
            self.spatial_state.cave_contours_dirty = false;
            self.recompute_cave_contours();
        }
        // Regenerate caves with empty cells (e.g. after sidebar Regenerate button)
        if self.dungeon.layout.is_some() {
            let needs_gen = self.dungeon.graph.rooms.iter().any(|r| {
                r.shape == crate::model::RoomShape::Cave
                    && r.cave_data.as_ref().is_some_and(|c| c.cells.is_empty())
            });
            if needs_gen {
                self.generate_caves();
                self.recompute_cave_contours();
            }
        }

        // Auto-solve layout when graph topology changes or first entering spatial/styled
        if !self.presenting {
            let current_hash = self.graph_hash();
            let needs_layout = matches!(self.active_tab, Tab::Spatial | Tab::Decor | Tab::Encounters | Tab::Styled);
            if needs_layout
                && (current_hash != self.last_graph_snapshot
                    || self.dungeon.layout.is_none()
                        && !self.dungeon.graph.rooms.is_empty())
            {
                self.solve_layout_incremental();
                ctx.request_repaint();
            }
        }
    }

    /// The bottom status bar.
    fn show_status_bar(&mut self, ctx: &egui::Context) {
        // Status bar
        let status_response = egui::TopBottomPanel::bottom("status_bar").show(ctx, |ui| {
            let zoom = if self.presenting {
                self.presentation_view_state.view.zoom
            } else {
                match self.active_tab {
                    Tab::Graph => self.graph_state.view.zoom,
                    Tab::Spatial => self.spatial_state.view.zoom,
                    Tab::Decor => self.decor_state.view.zoom,
                    Tab::Encounters => self.encounters_state.view.zoom,
                    Tab::Styled => self.styled_state.view.zoom,
                }
            };
            // Compute loading/rendering status for status bar (throttled: it hashes the map per view)
            if self.stale_renders_checked_at.elapsed() >= CACHE_CHECK_INTERVAL {
                self.stale_renders_checked_at = std::time::Instant::now();
                let stale_renders = &mut self.stale_renders;
                stale_renders.clear();
                if self.pending_monster_db.is_some() {
                    stale_renders.push("Bestiary");
                }
                if let Some(layout) = &self.dungeon.layout {
                    let enc_hash = crate::ui::encounters_view::cache_spec(layout, &self.dungeon.graph, &self.dungeon.theme).hash;
                    if !self.encounters_state.render_cache.is_current(enc_hash) { stale_renders.push("Encounters"); }
                    let pres_hash = crate::ui::presentation_view::cache_spec(layout, &self.dungeon.graph, &self.dungeon.theme).hash;
                    if !self.presentation_view_state.render_cache.is_current(pres_hash) { stale_renders.push("Presentation"); }
                    let styled_hash = crate::ui::styled_view::cache_spec(layout, &self.dungeon.graph, &self.dungeon.theme, self.styled_state.show_grid, self.styled_state.current_floor).hash;
                    if !self.styled_state.render_cache.is_current(styled_hash) { stale_renders.push("Styled"); }
                    let decor_hash = crate::ui::decor_view::cache_spec(layout, &self.dungeon.graph, &self.dungeon.theme, self.decor_state.current_floor).hash;
                    if !self.decor_state.render_cache.is_current(decor_hash) { stale_renders.push("Decor"); }
                }
            }
            ui.horizontal(|ui| {
                let saved = self.history.committed_hash() == self.last_saved_hash;
                let update_state = if self.update_ready_to_restart {
                    crate::ui::status_bar::UpdateState::Applied
                } else if self.pending_update_apply.is_some() {
                    crate::ui::status_bar::UpdateState::Applying
                } else if let Some(ref e) = self.update_error {
                    crate::ui::status_bar::UpdateState::Error(e)
                } else if let Some(ref info) = self.available_update {
                    crate::ui::status_bar::UpdateState::Available(&info.version)
                } else {
                    crate::ui::status_bar::UpdateState::None
                };
                let cloud_state = if !self.cloud_sync_enabled || !self.cloud_sync.is_logged_in() {
                    crate::ui::status_bar::CloudState::Disabled
                } else if self.pending_cloud_op.is_some() {
                    crate::ui::status_bar::CloudState::Syncing
                } else if let Some(status) = &self.cloud_status {
                    if status.contains("error") || status.contains("Error") || status.contains("failed") || status.contains("Failed") || status.contains("Conflict") {
                        crate::ui::status_bar::CloudState::Error(status.as_str())
                    } else {
                        crate::ui::status_bar::CloudState::Synced
                    }
                } else {
                    crate::ui::status_bar::CloudState::Synced
                };
                let update_clicked = crate::ui::status_bar::status_bar(ui, &self.dungeon, zoom, saved, cloud_state, &self.stale_renders, update_state);
                if update_clicked {
                    self.show_update_dialog = true;
                }
                if self.presenting {
                    ui.separator();
                    if let Some(server) = &self.server {
                        ui.label(format!(
                            "Server: port {} ({} clients)",
                            server.port,
                            server.client_count(),
                        ));
                    }
                }
            });
        });
        self.annotation_state.panel_rects.push(status_response.response.rect);
    }

    /// Modal dialogs: update confirmation, layout-solver refusals and the post-update restart prompt.
    fn show_dialogs(&mut self, ctx: &egui::Context) {
        // Update confirmation dialog
        if self.show_update_dialog {
            if let Some(info) = &self.available_update {
                let mut open = true;
                let version = info.version.clone();
                let notes = info.release_notes.clone();
                let download_url = info.download_url.clone();
                let sig_url = info.sig_url.clone();
                egui::Window::new(format!("Update to v{}", version))
                    .id(egui::Id::new("update_dialog"))
                    .open(&mut open)
                    .collapsible(false)
                    .default_size([400.0, 300.0])
                    .show(ctx, |ui| {
                        ui.label(format!("A new version is available: v{}", version));
                        ui.label(format!("Current version: v{}", env!("CARGO_PKG_VERSION")));
                        if !notes.is_empty() {
                            ui.add_space(8.0);
                            ui.label("Release notes:");
                            egui::ScrollArea::vertical().max_height(200.0).show(ui, |ui| {
                                ui.label(&notes);
                            });
                        }
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            if ui.button("Update Now").clicked() {
                                self.pending_update_apply = Some(
                                    updater::download_and_apply(download_url.clone(), sig_url.clone())
                                );
                                self.show_update_dialog = false;
                            }
                            if ui.button("Later").clicked() {
                                self.show_update_dialog = false;
                            }
                        });
                    });
                if !open {
                    self.show_update_dialog = false;
                }
            } else {
                self.show_update_dialog = false;
            }
        }

        // Layout solver refused to run (e.g. a containment cycle)
        if let Some(err) = self.layout_error.clone() {
            let mut open = true;
            egui::Window::new("Cannot arrange rooms")
                .id(egui::Id::new("layout_error_dialog"))
                .open(&mut open)
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.set_max_width(420.0);
                    ui.label(&err);
                    ui.add_space(8.0);
                    if ui.button("OK").clicked() {
                        self.layout_error = None;
                    }
                });
            if !open {
                self.layout_error = None;
            }
        }

        // Update restart dialog (shown when update applied but unsaved changes exist)
        if self.update_ready_to_restart {
            egui::Window::new("Update Ready")
                .id(egui::Id::new("update_restart_dialog"))
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.label("An update has been installed. You have unsaved changes.");
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button("Save and Restart").clicked() {
                            self.sync_session();
                            self.sync_party_from_dungeon();
                            self.sync_dungeon_to_campaign();
                            if let Some(path) = &self.current_file {
                                self.pending_file_op = Some(
                                    crate::io::save_load::save_campaign_to_path(&self.campaign, path.clone()),
                                );
                            } else {
                                self.pending_file_op = Some(
                                    crate::io::save_load::save_campaign_async(&self.campaign),
                                );
                            }
                            // restart_app() will be called when the save completes
                        }
                        if ui.button("Restart Later").clicked() {
                            self.update_ready_to_restart = false;
                        }
                    });
                });
        }
    }

    /// The combat log, minimized-window tabs and windows, the right sidebar and the notes drawer.
    fn show_side_panels(&mut self, ctx: &egui::Context) {
        // Combat log panel (bottom, only during presentation with active combat)
        if self.presenting {
            if let Some(presentation) = &mut self.presentation {
                if let Some(tracker) = &mut presentation.combat_tracker {
                    if !tracker.log.entries.is_empty() {
                        egui::TopBottomPanel::bottom("combat_log_panel")
                            .resizable(true)
                            .default_height(150.0)
                            .min_height(60.0)
                            .show(ctx, |ui| {
                                ui.horizontal(|ui| {
                                    ui.heading("Combat Log");
                                    if ui.small_button("Save Log").clicked() {
                                        let text = tracker.log.export_text();
                                        std::thread::spawn(move || {
                                            if let Some(path) = rfd::FileDialog::new()
                                                .set_file_name("combat_log.txt")
                                                .add_filter("Text", &["txt"])
                                                .save_file()
                                            {
                                                let _ = std::fs::write(path, text);
                                            }
                                        });
                                    }
                                    if ui.small_button("Clear Log").clicked() {
                                        tracker.log.entries.clear();
                                    }
                                });
                                egui::ScrollArea::vertical()
                                    .stick_to_bottom(true)
                                    .auto_shrink([false, false])
                                    .show(ui, |ui| {
                                        for entry in &tracker.log.entries {
                                            let color = egui::Color32::from_rgb(entry.color[0], entry.color[1], entry.color[2]);
                                            ui.label(egui::RichText::new(&entry.text).color(color).monospace().size(11.0));
                                        }
                                    });
                            });
                    }
                }
            }
        }

        // Tab bar of minimized windows (above the combat log), then the windows themselves
        crate::ui::window_dock::window_bar(ctx);
        self.draw_dock_windows(ctx);

        // Right sidebar
        let sidebar_response = egui::SidePanel::right("properties")
            .default_width(320.0)
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    if self.presenting {
                        if let Some(presentation) = &mut self.presentation {
                            let mut server_action = ServerAction::None;
                            presentation_view::presentation_sidebar(
                                ui,
                                &mut self.dungeon,
                                presentation,
                                &mut self.presentation_view_state,
                                &mut self.player_view_state,
                                &mut self.player_viewport_open,
                                &mut server_action,
                                &self.monster_db,
                                &mut self.combat_stats_cache,
                            );

                            // Web server controls (drawn here since sidebar fn can't own server)
                            ui.add_space(8.0);
                            if self.server.is_some() {
                                let server = self.server.as_ref().unwrap();
                                ui.label(format!("Listening on port {}", server.port));
                                ui.label(format!("Connected clients: {}", server.client_count()));

                                // Show local IP hint
                                ui.label(format!("http://localhost:{}", server.port));

                                if ui.button("Stop Server").clicked() {
                                    if let Some(mut s) = self.server.take() {
                                        s.stop();
                                    }
                                }
                            } else {
                                ui.horizontal(|ui| {
                                    ui.label("Port:");
                                    crate::ui::canvas_common::num_input_u16(ui, &mut self.server_port, 60.0);
                                });
                                if ui.button("Start Server").clicked() {
                                    match PresentationServer::start(self.server_port) {
                                        Ok(server) => {
                                            self.server = Some(server);
                                            // Push initial frame
                                        }
                                        Err(e) => eprintln!("Server error: {}", e),
                                    }
                                }
                            }
                        }
                    } else {
                        match self.active_tab {
                            Tab::Graph => {
                                crate::ui::sidebar::sidebar(
                                    ui,
                                    &mut self.dungeon,
                                    &self.graph_state.selection,
                                    &mut self.graph_state.focus_label,
                                );
                            }
                            Tab::Spatial => {
                                spatial_view::spatial_sidebar(
                                    ui,
                                    &mut self.dungeon,
                                    &mut self.spatial_state,
                                );
                            }
                            Tab::Decor => {
                                decor_view::decor_sidebar(
                                    ui,
                                    &mut self.dungeon,
                                    &mut self.decor_state,
                                );
                            }
                            Tab::Encounters => {
                                encounters_view::encounters_sidebar(
                                    ui,
                                    &mut self.dungeon,
                                    &self.monster_db,
                                    &mut self.encounters_state,
                                );
                            }
                            Tab::Styled => {
                                styled_view::styled_sidebar(
                                    ui,
                                    &mut self.dungeon,
                                    &mut self.styled_state,
                                );
                                // Dispatch async export if requested
                                if let Some(dm_mode) = self.styled_state.export_requested.take() {
                                    if self.dungeon.layout.is_some() && self.pending_file_op.is_none() {
                                        self.pending_file_op = Some(
                                            crate::io::save_load::export_png_async(&self.dungeon, dm_mode),
                                        );
                                    }
                                }
                            }
                        }
                    }
                });
            });
        self.annotation_state.panel_rects.push(sidebar_response.response.rect);

        // Session notes drawer — under the map, inside the sidebar's remaining width.
        // Declared after the sidebar so the sidebar keeps full height.
        {
            let context = self.note_context();
            crate::ui::notes_panel::notes_drawer(
                ctx,
                &mut self.note_vault,
                &mut self.notes_state,
                &context,
            );
            if self.note_vault.has_unsaved() {
                let now = std::time::Instant::now();
                let since = *self.notes_dirty_since.get_or_insert(now);
                // Typing pauses for a beat, then the .md files hit disk.
                if now.duration_since(since) >= NOTES_FLUSH_DELAY {
                    self.note_vault.flush();
                    self.notes_dirty_since = None;
                } else {
                    ctx.request_repaint_after(NOTES_FLUSH_DELAY);
                }
            } else {
                self.notes_dirty_since = None;
            }
        }
    }

    /// The central canvas for the active tab (or the presentation); returns its rect.
    fn show_canvas(&mut self, ctx: &egui::Context) -> egui::Rect {
        // Main canvas
        let central_response = egui::CentralPanel::default().show(ctx, |ui| {
            if self.presenting {
                if let Some(presentation) = &mut self.presentation {
                    presentation_view::presentation_view(
                        ui,
                        &mut self.dungeon,
                        presentation,
                        &mut self.presentation_view_state,
                        &mut self.player_view_state,
                        &self.monster_db,
                    );
                }
            } else {
                match self.active_tab {
                    Tab::Graph => {
                        graph_editor::graph_editor(ui, &mut self.dungeon, &mut self.graph_state);
                    }
                    Tab::Spatial => {
                        spatial_view::spatial_view(ui, &mut self.dungeon, &mut self.spatial_state);
                    }
                    Tab::Decor => {
                        decor_view::decor_view(ui, &mut self.dungeon, &mut self.decor_state);
                    }
                    Tab::Encounters => {
                        encounters_view::encounters_view(ui, &mut self.dungeon, &mut self.encounters_state, &self.monster_db);
                    }
                    Tab::Styled => {
                        styled_view::styled_view(ui, &self.dungeon, &mut self.styled_state);
                    }
                }
            }
        });

        // Also record central panel rect
        self.annotation_state.panel_rects.push(central_response.response.rect);
        central_response.response.rect
    }

    /// The annotation and help overlays, drawn over everything.
    fn show_overlays(&mut self, ctx: &egui::Context, canvas_rect: egui::Rect) {
        // Full-screen annotation overlay (drawn on top of everything)
        if self.annotation_mode {
            let current_view = self.current_view_name();
            let screen_rect = ctx.screen_rect();

            // Pre-extract data for nearest-room lookup to avoid borrowing self in the closure
            let view_state = self.current_view_state().clone();
            let is_graph = !self.presenting && self.active_tab == Tab::Graph;
            let graph_positions = self.graph_state.room_positions.clone();
            let rooms: Vec<(String, String)> = self.dungeon.graph.rooms.iter()
                .map(|r| (r.id.clone(), r.label.clone()))
                .collect();
            let layout_rooms: Vec<crate::model::RoomLayout> = self.dungeon.layout.as_ref()
                .map(|l| l.rooms.clone())
                .unwrap_or_default();

            let nearest_room_fn = move |fx: f32, fy: f32| -> Option<String> {
                let screen_pos = egui::pos2(
                    screen_rect.min.x + fx * screen_rect.width(),
                    screen_rect.min.y + fy * screen_rect.height(),
                );
                if !canvas_rect.contains(screen_pos) {
                    return None;
                }
                let transform = crate::util::ViewTransform::new(
                    view_state.offset, view_state.zoom, canvas_rect,
                );
                let world = transform.screen_to_world(screen_pos);

                if is_graph {
                    let mut best: Option<(f32, &str)> = None;
                    for (id, _) in &rooms {
                        if let Some(pos) = graph_positions.get(id) {
                            let dist = ((pos.x - world.x).powi(2) + (pos.y - world.y).powi(2)).sqrt();
                            if best.is_none() || dist < best.unwrap().0 {
                                best = Some((dist, id));
                            }
                        }
                    }
                    return best.filter(|(d, _)| *d < 200.0).map(|(_, id)| id.to_string());
                }

                let gx = (world.x / crate::util::GRID_PX).floor() as i32;
                let gy = (world.y / crate::util::GRID_PX).floor() as i32;
                for rl in &layout_rooms {
                    if rl.contains_point(gx as f32 + 0.5, gy as f32 + 0.5)
                    {
                        return Some(rl.room_id.clone());
                    }
                }
                let mut best: Option<(f32, &str)> = None;
                for rl in &layout_rooms {
                    let (cx, cy) = crate::util::room_center_px(rl);
                    let dist = ((cx - world.x).powi(2) + (cy - world.y).powi(2)).sqrt();
                    if best.is_none() || dist < best.unwrap().0 {
                        best = Some((dist, &rl.room_id));
                    }
                }
                best.filter(|(d, _)| *d < 200.0).map(|(_, id)| id.to_string())
            };

            let overlay_result = annotations::annotation_overlay(
                ctx,
                &mut self.dungeon.annotations,
                &mut self.annotation_state,
                &current_view,
                &nearest_room_fn,
            );

            if let Some(ann) = overlay_result.new_annotation {
                self.dungeon.annotations.push(ann);
                dump_annotations_file(&self.dungeon.annotations, &self.dungeon);
            } else if overlay_result.annotations_changed {
                dump_annotations_file(&self.dungeon.annotations, &self.dungeon);
            }
        }

        // Help overlay (F8)
        if self.help_mode {
            let current_view = self.current_view_name();
            crate::ui::help_overlay::help_overlay(
                ctx,
                &self.annotation_state.panel_rects,
                &current_view,
                self.presenting,
            );
        }
    }

    /// End of frame: sync the party back, track undo history, autosave, push the web view and draw the player window.
    fn end_frame(&mut self, ctx: &egui::Context) {
        // Sync party changes back to campaign
        self.sync_party_from_dungeon();

        // Track state changes for undo/redo
        let pointer_down = ctx.input(|i| i.pointer.any_down());
        self.history.track(&self.dungeon, pointer_down);

        // Auto-save: after 10s since last save, arm the trigger, then save on
        // the next committed state change (i.e. when the undo history records a
        // new commit, meaning the user finished an action).
        const AUTOSAVE_INTERVAL: std::time::Duration = std::time::Duration::from_secs(10);
        let committed_hash = self.history.committed_hash();
        let has_unsaved = committed_hash != self.last_saved_hash;
        if self.current_file.is_some() && has_unsaved && self.last_autosave.elapsed() >= AUTOSAVE_INTERVAL {
            self.autosave_due = true;
        }
        if self.autosave_due
            && self.current_file.is_some()
            && self.pending_file_op.is_none()
            && committed_hash != self.last_autosave_hash
        {
            self.sync_session();
            self.sync_party_from_dungeon();
            self.sync_dungeon_to_campaign();
            let path = self.current_file.clone().unwrap();
            self.pending_file_op = Some(
                crate::io::save_load::save_campaign_to_path(&self.campaign, path),
            );
            self.last_autosave = std::time::Instant::now();
            self.autosave_due = false;
        }
        self.last_autosave_hash = committed_hash;

        // Push server update only when presentation state has changed
        if self.presenting && self.server.is_some() {
            self.push_server_update_if_changed(ctx);
        }

        // Player viewport (second window)
        if self.presenting && self.player_viewport_open {
            // Determine which dungeon/presentation to show players
            let (player_dg, player_pres) = if self.player_map_index.is_some()
                && self.player_map_index != Some(self.campaign.active_map)
            {
                // Player sees a different map than DM
                (self.player_dungeon.as_ref(), self.player_presentation.as_ref())
            } else {
                // Player sees same map as DM
                (Some(&self.dungeon), self.presentation.as_ref())
            };
            if let (Some(dungeon), Some(presentation)) = (player_dg, player_pres) {
                let mut builder = egui::ViewportBuilder::default()
                    .with_title("Dungeon Mapper - Player View");
                if !self.player_viewport_initialized {
                    builder = builder.with_inner_size([800.0, 600.0]);
                    self.player_viewport_initialized = true;
                }
                ctx.show_viewport_immediate(
                    egui::ViewportId::from_hash_of("player_viewport"),
                    builder,
                    |ctx, _class| {
                        player_view::player_viewport(
                            ctx,
                            dungeon,
                            presentation,
                            &mut self.player_view_state,
                            &self.monster_db,
                        );
                    },
                );
            }
        }
    }
    /// Pre-warm render caches for all views in the background.
    /// Uses debouncing: only triggers builds after the dungeon hash has been stable for 500ms.
    fn prewarm_render_caches(&mut self, ctx: &egui::Context) {

        let Some(layout) = &self.dungeon.layout else { return };
        if !self.prewarm_immediate && self.prewarm_checked_at.elapsed() < CACHE_CHECK_INTERVAL {
            return;
        }
        self.prewarm_checked_at = std::time::Instant::now();

        // Debounce on the real render inputs (cave cells, decor, layout, theme), so a
        // stream of edits such as painting a cave doesn't kick off a background
        // re-render of every other view on each check.
        let prewarm_hash = {
            use std::hash::Hasher;
            let mut h = std::collections::hash_map::DefaultHasher::new();
            crate::render::bg_cache::map_render_hash(&mut h, layout, &self.dungeon.graph, &self.dungeon.theme, true);
            h.finish()
        };
        let immediate = self.prewarm_immediate;
        // Mid-stroke (painting, dragging) the map is still changing: hold off entirely.
        let editing = ctx.input(|i| i.pointer.any_down());
        if prewarm_hash != self.last_prewarm_hash || editing {
            self.last_prewarm_hash = prewarm_hash;
            self.prewarm_hash_changed_at = std::time::Instant::now();
            if !immediate {
                ctx.request_repaint_after(PREWARM_SETTLE);
                return;
            }
        }

        // Wait until the map has settled (unless immediate)
        if !immediate && self.prewarm_hash_changed_at.elapsed() < PREWARM_SETTLE {
            ctx.request_repaint_after(PREWARM_SETTLE);
            return;
        }
        self.prewarm_immediate = false;

        // Poll all caches for completed builds
        self.encounters_state.render_cache.poll();
        self.styled_state.render_cache.poll();
        self.decor_state.render_cache.poll();
        self.presentation_view_state.render_cache.poll();
        self.player_view_state.render_cache.poll();

        // Trigger builds for stale caches (one at a time to avoid thread spam)
        let graph = &self.dungeon.graph;
        let theme = &self.dungeon.theme;

        // One stale cache at a time, to avoid thread spam. A floor-filtered view renders a
        // filtered layout under its key, so leave that to the view itself.
        let styled_filtered = self.styled_state.current_floor.is_some();
        let decor_filtered = self.decor_state.current_floor.is_some();
        let caches = [
            (crate::ui::encounters_view::cache_spec(layout, graph, theme), &mut self.encounters_state.render_cache, false),
            (crate::ui::presentation_view::cache_spec(layout, graph, theme), &mut self.presentation_view_state.render_cache, false),
            (crate::ui::styled_view::cache_spec(layout, graph, theme, self.styled_state.show_grid, self.styled_state.current_floor), &mut self.styled_state.render_cache, styled_filtered),
            (crate::ui::decor_view::cache_spec(layout, graph, theme, self.decor_state.current_floor), &mut self.decor_state.render_cache, decor_filtered),
        ];
        for (spec, cache, filtered) in caches {
            if !filtered && !cache.is_current(spec.hash) && cache.pending_label().is_none() {
                cache.ensure_spec(&spec, graph, layout, theme);
                ctx.request_repaint_after(CACHE_CHECK_INTERVAL);
                return;
            }
        }
    }
}

/// Dump unresolved annotations to a text file for external tools (e.g. Claude) to read.
/// Written to `annotations.md` in the current working directory.
fn dump_annotations_file(annotations: &[crate::model::Annotation], dungeon: &Dungeon) {
    use std::io::Write;
    let path = std::path::Path::new("annotations.md");
    let unresolved: Vec<_> = annotations.iter().filter(|a| !a.resolved).collect();

    let mut contents = String::new();
    contents.push_str("# Dungeon Mapper - Open Issues\n\n");
    contents.push_str(&format!("Dungeon: {}\n\n", dungeon.name));
    if unresolved.is_empty() {
        contents.push_str("No open issues.\n");
    } else {
        contents.push_str(&format!("{} open issue(s):\n\n", unresolved.len()));
        for (i, ann) in unresolved.iter().enumerate() {
            contents.push_str(&format!("## Issue {}\n\n", i + 1));
            contents.push_str(&format!("- **ID:** {}\n", ann.id));
            contents.push_str(&format!("- **Description:** {}\n", ann.text));
            contents.push_str(&format!("- **View:** {}\n", ann.view));
            contents.push_str(&format!("- **Screen position:** ({:.3}, {:.3}) [fraction of window]\n", ann.world_x, ann.world_y));
            if let Some(room_id) = &ann.room_id {
                let room_label = dungeon.graph.room_by_id(room_id)
                    .map(|r| r.label.as_str())
                    .unwrap_or("(unknown)");
                contents.push_str(&format!("- **Near room:** {} ({})\n", room_label, room_id));
            }
            contents.push_str(&format!("- **Created:** {}\n", ann.created_at));
            contents.push('\n');
        }
    }

    match std::fs::File::create(path) {
        Ok(mut f) => {
            let _ = f.write_all(contents.as_bytes());
        }
        Err(e) => eprintln!("Failed to write annotations.md: {}", e),
    }
}

/// Search for the bestiary data directory in several candidate locations.
fn find_bestiary_dir() -> Option<std::path::PathBuf> {
    let candidates = [
        // Relative to CWD
        std::path::PathBuf::from("data/bestiary"),
        std::path::PathBuf::from("5etools-src/data/bestiary"),
        // Relative to executable
        std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|p| p.join("data/bestiary")))
            .unwrap_or_default(),
    ];
    candidates.into_iter().find(|p| p.is_dir())
}
