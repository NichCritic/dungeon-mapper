use crate::model::Dungeon;

pub fn status_bar(ui: &mut egui::Ui, dungeon: &Dungeon, zoom: f32, saved: bool, rendering: &[&str]) {
    ui.horizontal(|ui| {
        if saved {
            ui.colored_label(egui::Color32::from_rgb(100, 200, 100), "\u{2713} Saved");
        } else {
            ui.colored_label(egui::Color32::from_rgb(180, 180, 180), "\u{25cb} Unsaved");
        }
        ui.separator();
        ui.label(format!(
            "{} rooms, {} connections",
            dungeon.graph.rooms.len(),
            dungeon.graph.connections.len()
        ));
        if !rendering.is_empty() {
            ui.separator();
            ui.colored_label(
                egui::Color32::from_rgb(200, 200, 100),
                format!("Loading: {}", rendering.join(", ")),
            );
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(format!("Zoom: {:.0}%", zoom * 100.0));
        });
    });
}
