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
        }
        self.last_graph_snapshot = self.graph_hash();
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
                        match crate::io::save_load::load_dungeon() {
                            Ok(d) => {
                                self.dungeon = d;
                                self.graph_state = GraphEditorState::default();
                                self.presenting = false;
                                self.presentation = None;
                            }
                            Err(e) => eprintln!("Load error: {}", e),
                        }
                        ui.close_menu();
                    }
                    if ui.button("Save As...").clicked() {
                        if let Err(e) = crate::io::save_load::save_dungeon(&self.dungeon) {
                            eprintln!("Save error: {}", e);
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
