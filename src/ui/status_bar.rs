use crate::model::Dungeon;

pub enum UpdateState<'a> {
    None,
    Available(&'a str),
    Applying,
    Applied,
    Error(&'a str),
}

pub enum CloudState<'a> {
    /// Cloud sync not configured or not logged in.
    Disabled,
    /// Synced and up to date.
    Synced,
    /// Currently syncing.
    Syncing,
    /// Sync error.
    Error(&'a str),
}

/// Returns true if the update label was clicked.
pub fn status_bar(ui: &mut egui::Ui, dungeon: &Dungeon, zoom: f32, saved: bool, cloud: CloudState, rendering: &[&str], update: UpdateState) -> bool {
    let mut update_clicked = false;
    ui.horizontal(|ui| {
        if saved {
            ui.colored_label(egui::Color32::from_rgb(100, 200, 100), "\u{2713} Saved");
        } else {
            ui.colored_label(egui::Color32::from_rgb(180, 180, 180), "\u{25cb} Unsaved");
        }
        match cloud {
            CloudState::Synced => {
                ui.colored_label(egui::Color32::from_rgb(100, 180, 255), "\u{2601} Synced");
            }
            CloudState::Syncing => {
                ui.colored_label(egui::Color32::from_rgb(200, 200, 100), "\u{2601} Syncing...");
            }
            CloudState::Error(e) => {
                ui.colored_label(egui::Color32::from_rgb(255, 100, 100), "\u{2601} Sync error")
                    .on_hover_text(e);
            }
            CloudState::Disabled => {}
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
            ui.colored_label(egui::Color32::from_rgb(130, 130, 130), "F8: Help");
            ui.separator();
            match update {
                UpdateState::Available(version) => {
                    if ui.colored_label(
                        egui::Color32::from_rgb(100, 220, 255),
                        format!("Update: v{}", version),
                    ).on_hover_text("Click to update").clicked() {
                        update_clicked = true;
                    }
                    ui.separator();
                }
                UpdateState::Applying => {
                    ui.colored_label(egui::Color32::from_rgb(220, 200, 100), "Updating...");
                    ui.separator();
                }
                UpdateState::Applied => {
                    ui.colored_label(egui::Color32::from_rgb(100, 255, 100), "Updated! Restart to apply.");
                    ui.separator();
                }
                UpdateState::Error(e) => {
                    ui.colored_label(egui::Color32::from_rgb(255, 100, 100), "Update failed")
                        .on_hover_text(e);
                    ui.separator();
                }
                UpdateState::None => {}
            }
            ui.label(format!("Zoom: {:.0}%", zoom * 100.0));
        });
    });
    update_clicked
}
