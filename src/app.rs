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
fn restart_app() -> ! {
    let exe = std::env::current_exe().expect("Failed to get current exe path");
    let args: Vec<String> = std::env::args().skip(1).collect();
    std::process::Command::new(exe)
        .args(&args)
        .spawn()
        .expect("Failed to restart application");
    std::process::exit(0);
}

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
    pub combat_window_open: bool,
    /// True after the player viewport has been shown at least once (avoids re-applying initial size every frame).
    player_viewport_initialized: bool,
    pub server: Option<PresentationServer>,
    pub server_port: u16,
    /// Hash of the last PNG pushed to the server, to avoid redundant updates.
    last_server_push_hash: u64,
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

    /// Hash of dungeon state used for render pre-warming debounce.
    last_prewarm_hash: u64,
    /// When the prewarm hash last changed (for debounce).
    prewarm_hash_changed_at: std::time::Instant,
    /// Skip debounce on next prewarm check (set on map load).
    prewarm_immediate: bool,
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
            combat_window_open: false,
            player_viewport_initialized: false,
            server: None,
            server_port: 8080,
            last_server_push_hash: 0,
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
            last_update_check: std::time::Instant::now(),
            history,
            current_file: None,
            last_saved_hash: initial_hash,
            last_autosave: std::time::Instant::now(),
            autosave_due: false,
            last_prewarm_hash: 0,
            prewarm_hash_changed_at: std::time::Instant::now(),
            prewarm_immediate: false,
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

    /// Full re-solve: recomputes all room positions and corridors from scratch.
    pub fn solve_layout_full(&mut self) {
        let old_bounds = self.dungeon.layout.as_ref()
            .map(|l| l.bounds.clone())
            .unwrap_or_default();
        match crate::solver::layout::solve_layout(
            &self.dungeon.graph,
            self.spatial_state.density_gap,
        ) {
            Ok(mut layout) => {
                layout.bounds = old_bounds;
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
            let segments = crate::solver::cave_gen::compute_contour_segments(&rl, cave, &floor);
            self.dungeon.graph.rooms[idx].cave_data.as_mut().unwrap().contour_segments = segments;
        }
    }

    /// Render a player-view PNG for the web server.
    fn render_player_png(&self) -> Option<Vec<u8>> {
        // Use player-specific map if it differs from DM view
        let (dungeon, pres) = if self.player_map_index.is_some()
            && self.player_map_index != Some(self.campaign.active_map)
        {
            (self.player_dungeon.as_ref()?, self.player_presentation.as_ref()?)
        } else {
            (&self.dungeon, self.presentation.as_ref()?)
        };
        let layout = dungeon.layout.as_ref()?;
        let presentation = pres;

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
        };
        crate::render::presentation::render_player_view(
            &mut renderer,
            &dungeon.graph,
            layout,
            &dungeon.theme,
            presentation,
            &dungeon.light_sources,
            dungeon.ambient_light,
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

    fn push_server_update_if_changed(&mut self) {
        let hash = self.presentation_hash();
        if hash == self.last_server_push_hash {
            return;
        }
        if let Some(server) = &self.server {
            if let Some(png) = self.render_player_png() {
                server.push_update(png);
                self.last_server_push_hash = hash;
            }
        }
    }
}

impl eframe::App for DungeonApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Sync campaign party into working dungeon at frame start
        self.sync_party_to_dungeon();

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
                        self.current_file = Some(path);
                        self.last_saved_hash = self.history.committed_hash();
                        // Trigger immediate render cache pre-warming (skip debounce)
                        self.prewarm_immediate = true;
                    }
                    FileOpResult::Loaded(Err(e)) => eprintln!("Load error: {}", e),
                    FileOpResult::Saved(Ok(path)) => {
                        self.current_file = Some(path);
                        self.last_saved_hash = self.history.committed_hash();
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

        // Pre-warm render caches for all views (debounced, runs before UI so status bar sees pending state)
        self.prewarm_render_caches(ctx);

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

        // Global key: F8 toggles help overlay
        let f8_pressed = ctx.input(|i| i.key_pressed(egui::Key::F8));
        if f8_pressed {
            self.help_mode = !self.help_mode;
            self.annotation_mode = false;
        }

        // Collect panel rects for annotation spotlight
        self.annotation_state.panel_rects.clear();

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
                        self.combat_window_open = false;
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
            // Compute loading/rendering status for status bar
            let mut stale_renders: Vec<&str> = Vec::new();
            if self.pending_monster_db.is_some() {
                stale_renders.push("Bestiary");
            }
            if let Some(layout) = &self.dungeon.layout {
                let enc_hash = crate::ui::encounters_view::render_cache_hash(layout, &self.dungeon.graph, &self.dungeon.theme);
                if !self.encounters_state.render_cache.is_current(enc_hash) { stale_renders.push("Encounters"); }
                let pres_hash = crate::ui::presentation_view::render_cache_hash(layout, &self.dungeon.theme);
                if !self.presentation_view_state.render_cache.is_current(pres_hash) { stale_renders.push("Presentation"); }
                let styled_hash = crate::ui::styled_view::render_cache_hash(layout, &self.dungeon.graph, &self.dungeon.theme, self.styled_state.show_grid, self.styled_state.current_floor);
                if !self.styled_state.render_cache.is_current(styled_hash) { stale_renders.push("Styled"); }
                let decor_hash = crate::ui::decor_view::render_cache_hash(layout, &self.dungeon.graph, &self.dungeon.theme, self.decor_state.current_floor);
                if !self.decor_state.render_cache.is_current(decor_hash) { stale_renders.push("Decor"); }
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
                let update_clicked = crate::ui::status_bar::status_bar(ui, &self.dungeon, zoom, saved, cloud_state, &stale_renders, update_state);
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

        // Combat log panel (bottom, only during presentation with active combat)
        if self.presenting {
            if let Some(presentation) = &self.presentation {
                if let Some(tracker) = &presentation.combat_tracker {
                    if !tracker.log.entries.is_empty() {
                        egui::TopBottomPanel::bottom("combat_log_panel")
                            .resizable(true)
                            .default_height(150.0)
                            .min_height(60.0)
                            .show(ctx, |ui| {
                                ui.heading("Combat Log");
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

        // Combat tracker floating window (when popped out of sidebar)
        if self.presenting && self.combat_window_open {
            if let Some(presentation) = &mut self.presentation {
                presentation_view::combat_tracker_window(
                    ctx,
                    presentation,
                    &self.dungeon,
                    &mut self.combat_window_open,
                );
            }
        }

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
                                &mut self.combat_window_open,
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
                                    &mut self.combat_stats_cache,
                                    &mut self.encounters_state,
                                );
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
                        encounters_view::encounters_view(ui, &self.dungeon, &mut self.encounters_state);
                    }
                    Tab::Styled => {
                        styled_view::styled_view(ui, &self.dungeon, &mut self.styled_state);
                    }
                }
            }
        });

        // Also record central panel rect
        self.annotation_state.panel_rects.push(central_response.response.rect);

        // Full-screen annotation overlay (drawn on top of everything)
        if self.annotation_mode {
            let current_view = self.current_view_name();
            let canvas_rect = central_response.response.rect;
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
                    if gx >= rl.x && gx < rl.x + rl.width as i32
                        && gy >= rl.y && gy < rl.y + rl.height as i32
                    {
                        return Some(rl.room_id.clone());
                    }
                }
                let mut best: Option<(f32, &str)> = None;
                for rl in &layout_rooms {
                    let cx = (rl.x as f32 + rl.width as f32 / 2.0) * crate::util::GRID_PX;
                    let cy = (rl.y as f32 + rl.height as f32 / 2.0) * crate::util::GRID_PX;
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
            self.push_server_update_if_changed();
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
                        );
                    },
                );
            }
        }
    }
}

impl DungeonApp {
    /// Pre-warm render caches for all views in the background.
    /// Uses debouncing: only triggers builds after the dungeon hash has been stable for 500ms.
    fn prewarm_render_caches(&mut self, ctx: &egui::Context) {
        use crate::render::themed::RenderOptions;

        let Some(layout) = &self.dungeon.layout else { return };

        // Compute a simple hash of things that affect renders
        let prewarm_hash = {
            use std::hash::{Hash, Hasher};
            use std::collections::hash_map::DefaultHasher;
            let mut h = DefaultHasher::new();
            layout.rooms.len().hash(&mut h);
            self.dungeon.theme.wall_color.hash(&mut h);
            self.dungeon.theme.floor_color.hash(&mut h);
            self.dungeon.theme.bg_color.hash(&mut h);
            self.dungeon.graph.rooms.len().hash(&mut h);
            for r in &self.dungeon.graph.rooms {
                r.decor.len().hash(&mut h);
            }
            h.finish()
        };

        // Debounce: track when the hash last changed
        let immediate = self.prewarm_immediate;
        if prewarm_hash != self.last_prewarm_hash {
            self.last_prewarm_hash = prewarm_hash;
            self.prewarm_hash_changed_at = std::time::Instant::now();
            if !immediate {
                return; // Don't trigger builds yet (unless immediate flag is set)
            }
        }

        // Wait 500ms after last change (unless immediate)
        if !immediate && self.prewarm_hash_changed_at.elapsed() < std::time::Duration::from_millis(500) {
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
        let layout = layout.clone();
        let graph = &self.dungeon.graph;
        let theme = &self.dungeon.theme;

        // Encounters view cache
        let enc_hash = crate::ui::encounters_view::render_cache_hash(&layout, graph, theme);
        if !self.encounters_state.render_cache.is_current(enc_hash)
            && self.encounters_state.render_cache.pending_label().is_none()
        {
            self.encounters_state.render_cache.ensure(
                enc_hash, graph, &layout, theme,
                RenderOptions { show_grid: true, show_labels: true, show_notes: false, show_secrets: false, show_decor: true },
                "Encounters",
            );
            ctx.request_repaint();
            return;
        }

        // Presentation view cache
        let pres_hash = crate::ui::presentation_view::render_cache_hash(&layout, theme);
        if !self.presentation_view_state.render_cache.is_current(pres_hash)
            && self.presentation_view_state.render_cache.pending_label().is_none()
        {
            self.presentation_view_state.render_cache.ensure(
                pres_hash, graph, &layout, theme,
                RenderOptions { show_grid: true, show_labels: true, show_notes: true, show_secrets: true, show_decor: true },
                "Presentation",
            );
            ctx.request_repaint();
            return;
        }

        // Styled view cache
        let styled_hash = crate::ui::styled_view::render_cache_hash(&layout, graph, theme, self.styled_state.show_grid, self.styled_state.current_floor);
        if !self.styled_state.render_cache.is_current(styled_hash)
            && self.styled_state.render_cache.pending_label().is_none()
        {
            self.styled_state.render_cache.ensure(
                styled_hash, graph, &layout, theme,
                RenderOptions { show_grid: self.styled_state.show_grid, show_labels: true, show_notes: true, show_secrets: true, show_decor: true },
                "Styled",
            );
            ctx.request_repaint();
            return;
        }

        // Decor view cache
        let decor_hash = crate::ui::decor_view::render_cache_hash(&layout, graph, theme, self.decor_state.current_floor);
        if !self.decor_state.render_cache.is_current(decor_hash)
            && self.decor_state.render_cache.pending_label().is_none()
        {
            self.decor_state.render_cache.ensure(
                decor_hash, graph, &layout, theme,
                RenderOptions { show_grid: true, show_labels: true, show_notes: false, show_secrets: false, show_decor: false },
                "Decor",
            );
            ctx.request_repaint();
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
