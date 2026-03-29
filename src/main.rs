mod app;
mod io;
mod model;
mod presentation;
mod render;
mod server;
mod solver;
mod ui;
mod util;

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1200.0, 800.0])
            .with_title("Dungeon Drafter"),
        ..Default::default()
    };

    eframe::run_native(
        "Dungeon Drafter",
        options,
        Box::new(|_cc| Ok(Box::new(app::DungeonApp::default()))),
    )
}
