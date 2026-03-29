use crate::model::Dungeon;
use crate::ui::graph_editor::{self, GraphEditorState};
use crate::ui::spatial_view::{self, SpatialViewState};
use crate::ui::styled_view::{self, StyledViewState};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Tab {
    Graph,
    Spatial,
    Styled,
}

pub struct DungeonApp {
    pub dungeon: Dungeon,
    pub active_tab: Tab,
    prev_tab: Tab,
    pub graph_state: GraphEditorState,
    pub spatial_state: SpatialViewState,
    pub styled_state: StyledViewState,
    /// Snapshot of graph state to detect when a re-solve is needed
    last_graph_snapshot: u64,
}

impl Default for DungeonApp {
    fn default() -> Self {
        Self {
            dungeon: Dungeon::default(),
            active_tab: Tab::Graph,
            prev_tab: Tab::Graph,
            graph_state: GraphEditorState::default(),
            spatial_state: SpatialViewState::default(),
            styled_state: StyledViewState::default(),
            last_graph_snapshot: 0,
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
        }
        h.finish()
    }

    fn solve_layout(&mut self) {
        // Preserve user-placed bounds across re-solves
        let old_bounds = self.dungeon.layout.as_ref()
            .map(|l| l.bounds.clone())
            .unwrap_or_default();
        match crate::solver::layout::solve_layout(
            &self.dungeon.graph,
            self.spatial_state.density_gap,
            self.spatial_state.corridor_width,
        ) {
            Ok(mut layout) => {
                layout.bounds = old_bounds;
                self.dungeon.layout = Some(layout);
            }
            Err(e) => eprintln!("Layout solver error: {}", e),
        }
        self.last_graph_snapshot = self.graph_hash();
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
                        ui.close_menu();
                    }
                    if ui.button("Open...").clicked() {
                        match crate::io::save_load::load_dungeon() {
                            Ok(d) => {
                                self.dungeon = d;
                                self.graph_state = GraphEditorState::default();
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

                // Tab buttons
                ui.selectable_value(&mut self.active_tab, Tab::Graph, "Graph");
                ui.selectable_value(&mut self.active_tab, Tab::Spatial, "Spatial");
                ui.selectable_value(&mut self.active_tab, Tab::Styled, "Styled");

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(&self.dungeon.name);
                });
            });
        });

        // Auto-solve layout when graph topology changes
        let current_hash = self.graph_hash();
        let needs_layout = matches!(self.active_tab, Tab::Spatial | Tab::Styled);
        if needs_layout && current_hash != self.last_graph_snapshot {
            self.solve_layout();
        }
        // Also solve on first visit if no layout exists yet
        if needs_layout && self.dungeon.layout.is_none() && !self.dungeon.graph.rooms.is_empty() {
            self.solve_layout();
        }
        self.prev_tab = self.active_tab;

        // Status bar
        egui::TopBottomPanel::bottom("status_bar").show(ctx, |ui| {
            let zoom = match self.active_tab {
                Tab::Graph => self.graph_state.view.zoom,
                Tab::Spatial => self.spatial_state.view.zoom,
                Tab::Styled => self.styled_state.view.zoom,
            };
            crate::ui::status_bar::status_bar(ui, &self.dungeon, zoom);
        });

        // Right sidebar
        egui::SidePanel::right("properties")
            .default_width(250.0)
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
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
                });
            });

        // Main canvas
        egui::CentralPanel::default().show(ctx, |ui| {
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
        });
    }
}
