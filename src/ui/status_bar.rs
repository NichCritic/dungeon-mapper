use crate::model::Dungeon;

pub fn status_bar(ui: &mut egui::Ui, dungeon: &Dungeon, zoom: f32) {
    ui.horizontal(|ui| {
        ui.label(format!(
            "{} rooms, {} connections",
            dungeon.graph.rooms.len(),
            dungeon.graph.connections.len()
        ));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(format!("Zoom: {:.0}%", zoom * 100.0));
        });
    });
}
