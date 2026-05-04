// Hide the console window on Windows release builds
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod data;
mod history;
mod io;
mod model;
mod presentation;
mod render;
mod server;
mod solver;
mod ui;
mod updater;
mod util;

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1200.0, 800.0])
            .with_title("Dungeon Mapper"),
        ..Default::default()
    };

    eframe::run_native(
        "Dungeon Mapper",
        options,
        Box::new(|cc| {
            cc.egui_ctx.set_visuals(egui::Visuals::dark());
            egui_extras::install_image_loaders(&cc.egui_ctx);
            Ok(Box::new(app::DungeonApp::default()))
        }),
    )
}
