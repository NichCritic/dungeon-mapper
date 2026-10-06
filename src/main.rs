// Hide the console window on Windows release builds
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod data;
mod history;
mod io;
mod model;
mod notes;
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
        // On Wayland a vsynced buffer swap waits for the compositor's frame callback,
        // and a window on a hidden workspace never gets one: the swap blocks the main
        // thread, the app stops answering the compositor's pings, and it is reported
        // as not responding. Seen with NVIDIA's EGL on Hyprland. egui only repaints
        // when asked, so without vsync it still idles.
        vsync: !cfg!(target_os = "linux"),
        ..Default::default()
    };

    eframe::run_native(
        "Dungeon Mapper",
        options,
        Box::new(|cc| {
            cc.egui_ctx.set_theme(egui::Theme::Dark);
            egui_extras::install_image_loaders(&cc.egui_ctx);
            Ok(Box::new(app::DungeonApp::default()))
        }),
    )
}
