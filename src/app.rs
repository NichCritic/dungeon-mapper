use crate::model::Dungeon;
use crate::presentation::PresentationState;
use crate::server::PresentationServer;
use crate::ui::graph_editor::{self, GraphEditorState};
use crate::ui::spatial_view::{self, SpatialViewState};
use crate::ui::styled_view::{self, StyledViewState};
use crate::ui::presentation_view::{self, PresentationViewState, ServerAction};
use crate::ui::player_view::{self, PlayerViewState};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Tab {
    Graph,
    Spatial,
    Styled,
}

pub struct DungeonApp {
    pub dungeon: Dungeon,
    pub active_tab: Tab,
    pub graph_state: GraphEditorState,
    pub spatial_state: SpatialViewState,
    pub styled_state: StyledViewState,
    /// Snapshot of graph state to detect when a re-solve is needed
    last_graph_snapshot: u64,

    // Presentation mode
    pub presenting: bool,
    pub presentation: Option<PresentationState>,
    pub presentation_view_state: PresentationViewState,
    pub player_viewport_open: bool,
    pub player_view_state: PlayerViewState,
    pub server: Option<PresentationServer>,
    pub server_port: u16,
    /// Hash of the last PNG pushed to the server, to avoid redundant updates.
    last_server_push_hash: u64,

    /// Pending async file operation (save/load/export).
    pending_file_op: Option<std::sync::mpsc::Receiver<crate::io::save_load::FileOpResult>>,
}

impl Default for DungeonApp {
    fn default() -> Self {
        Self {
            dungeon: Dungeon::default(),
            active_tab: Tab::Graph,
            graph_state: GraphEditorState::default(),
            spatial_state: SpatialViewState::default(),
            styled_state: StyledViewState::default(),
            last_graph_snapshot: 0,

            presenting: false,
            presentation: None,
            presentation_view_state: PresentationViewState::default(),
            player_viewport_open: false,
            player_view_state: PlayerViewState::default(),
            server: None,
            server_port: 8080,
            last_server_push_hash: 0,
            pending_file_op: None,
        }
    }
}

impl DungeonApp {
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
        let layout = self.dungeon.layout.as_ref()?;
        let presentation = self.presentation.as_ref()?;

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
        };
        crate::render::presentation::render_player_view(
            &mut renderer,
            &self.dungeon.graph,
            layout,
            &self.dungeon.theme,
            presentation,
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
        presentation.light_sources.len().hash(&mut h);
        for light in &presentation.light_sources {
            light.id.hash(&mut h);
            light.radius.to_bits().hash(&mut h);
            light.intensity.to_bits().hash(&mut h);
            light.room_id.hash(&mut h);
        }
        presentation.ambient_light.to_bits().hash(&mut h);
        h.finish()
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
        // Poll pending async file operation
        if let Some(rx) = &self.pending_file_op {
            if let Ok(result) = rx.try_recv() {
                use crate::io::save_load::FileOpResult;
                match result {
                    FileOpResult::Loaded(Ok(d)) => {
                        self.dungeon = d;
                        self.graph_state = GraphEditorState::default();
                        self.presenting = false;
                        self.presentation = None;
                    }
                    FileOpResult::Loaded(Err(e)) => eprintln!("Load error: {}", e),
                    FileOpResult::Saved(Ok(_path)) => {}
                    FileOpResult::Saved(Err(e)) => eprintln!("Save error: {}", e),
                    FileOpResult::ExportedPng(Ok(())) => {}
                    FileOpResult::ExportedPng(Err(e)) => eprintln!("Export error: {}", e),
                    FileOpResult::Cancelled => {}
                }
                self.pending_file_op = None;
            }
        }

        // Top menu bar
        egui::TopBottomPanel::top("menu_bar").show(ctx, |ui| {
            egui::menu::bar(ui, |ui| {
                ui.menu_button("File", |ui| {
                    if ui.button("New").clicked() {
                        self.dungeon = Dungeon::default();
                        self.graph_state = GraphEditorState::default();
                        self.spatial_state = SpatialViewState::default();
                        self.styled_state = StyledViewState::default();
                        self.presenting = false;
                        self.presentation = None;
                        ui.close_menu();
                    }
                    if ui.button("Open...").clicked() {
                        if self.pending_file_op.is_none() {
                            self.pending_file_op = Some(crate::io::save_load::load_dungeon_async());
                        }
                        ui.close_menu();
                    }
                    if ui.button("Save As...").clicked() {
                        if self.pending_file_op.is_none() {
                            self.pending_file_op = Some(crate::io::save_load::save_dungeon_async(&self.dungeon));
                        }
                        ui.close_menu();
                    }
                });

                ui.separator();

                if self.presenting {
                    // In presentation mode, show only a "Stop Presenting" button
                    if ui.button("Stop Presenting").clicked() {
                        self.presenting = false;
                        self.player_viewport_open = false;
                        if let Some(server) = &mut self.server {
                            server.stop();
                        }
                        self.server = None;
                    }
                } else {
                    // Normal tab buttons
                    ui.selectable_value(&mut self.active_tab, Tab::Graph, "Graph");
                    ui.selectable_value(&mut self.active_tab, Tab::Spatial, "Spatial");
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
                    ui.label(&self.dungeon.name);
                });
            });
        });

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
            let needs_layout = matches!(self.active_tab, Tab::Spatial | Tab::Styled);
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
        egui::TopBottomPanel::bottom("status_bar").show(ctx, |ui| {
            let zoom = if self.presenting {
                self.presentation_view_state.view.zoom
            } else {
                match self.active_tab {
                    Tab::Graph => self.graph_state.view.zoom,
                    Tab::Spatial => self.spatial_state.view.zoom,
                    Tab::Styled => self.styled_state.view.zoom,
                }
            };
            ui.horizontal(|ui| {
                crate::ui::status_bar::status_bar(ui, &self.dungeon, zoom);
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

        // Right sidebar
        egui::SidePanel::right("properties")
            .default_width(250.0)
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
                                    ui.add(egui::DragValue::new(&mut self.server_port).range(1024..=65535));
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

        // Main canvas
        egui::CentralPanel::default().show(ctx, |ui| {
            if self.presenting {
                if let Some(presentation) = &mut self.presentation {
                    presentation_view::presentation_view(
                        ui,
                        &self.dungeon,
                        presentation,
                        &mut self.presentation_view_state,
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
                    Tab::Styled => {
                        styled_view::styled_view(ui, &self.dungeon, &mut self.styled_state);
                    }
                }
            }
        });

        // Push server update only when presentation state has changed
        if self.presenting && self.server.is_some() {
            self.push_server_update_if_changed();
        }

        // Player viewport (second window)
        if self.presenting && self.player_viewport_open {
            if let Some(presentation) = &self.presentation {
                ctx.show_viewport_immediate(
                    egui::ViewportId::from_hash_of("player_viewport"),
                    egui::ViewportBuilder::default()
                        .with_title("Dungeon Drafter - Player View")
                        .with_inner_size([800.0, 600.0]),
                    |ctx, _class| {
                        player_view::player_viewport(
                            ctx,
                            &self.dungeon,
                            presentation,
                            &mut self.player_view_state,
                        );
                    },
                );
            }
        }
    }
}
