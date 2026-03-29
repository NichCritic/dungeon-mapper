use crate::model::*;
use crate::ui::graph_editor::Selection;

pub fn sidebar(
    ui: &mut egui::Ui,
    dungeon: &mut Dungeon,
    selection: &Selection,
) {
    ui.heading("Properties");
    ui.separator();

    match selection {
        Selection::None => {
            ui.label("Select a room or connection to edit its properties.");
            ui.separator();
            dungeon_properties(ui, dungeon);
        }
        Selection::Room(id) => {
            if let Some(room) = dungeon.graph.room_by_id_mut(id) {
                room_properties(ui, room);
            } else {
                ui.label("Room not found.");
            }
        }
        Selection::Connection(id) => {
            if let Some(edge) = dungeon.graph.connection_by_id_mut(id) {
                connection_properties(ui, edge);
            } else {
                ui.label("Connection not found.");
            }
        }
    }
}

fn dungeon_properties(ui: &mut egui::Ui, dungeon: &mut Dungeon) {
    ui.label("Dungeon Name:");
    ui.text_edit_singleline(&mut dungeon.name);
    ui.add_space(8.0);
    ui.label(format!(
        "{} rooms, {} connections",
        dungeon.graph.rooms.len(),
        dungeon.graph.connections.len()
    ));
}

fn room_properties(ui: &mut egui::Ui, room: &mut Room) {
    ui.label("Label:");
    ui.text_edit_singleline(&mut room.label);

    ui.add_space(8.0);
    ui.label("Size Preset:");
    egui::ComboBox::from_id_salt("size_hint")
        .selected_text(room.size_hint.label())
        .show_ui(ui, |ui| {
            for hint in SizeHint::ALL {
                if ui.selectable_value(&mut room.size_hint, hint, hint.label()).changed() {
                    // Clear overrides when selecting a preset
                    room.grid_width = None;
                    room.grid_height = None;
                }
            }
        });

    ui.add_space(8.0);
    let (effective_w, effective_h) = room.grid_size();
    let mut w = room.grid_width.unwrap_or(effective_w);
    let mut h = room.grid_height.unwrap_or(effective_h);

    ui.label("Dimensions (grid squares):");
    ui.horizontal(|ui| {
        if ui.add(egui::DragValue::new(&mut w).range(1..=20).prefix("W: ")).changed() {
            room.grid_width = Some(w);
        }
        if ui.add(egui::DragValue::new(&mut h).range(1..=20).prefix("H: ")).changed() {
            room.grid_height = Some(h);
        }
    });
    ui.label(format!("{}x{} ft", w * 5, h * 5));

    ui.add_space(8.0);
    ui.label("Shape:");
    egui::ComboBox::from_id_salt("room_shape")
        .selected_text(room.shape.label())
        .show_ui(ui, |ui| {
            for shape in RoomShape::ALL {
                ui.selectable_value(&mut room.shape, shape, shape.label());
            }
        });

    let (ew, eh) = room.grid_size();
    if room.shape == RoomShape::Rectangle && ew != eh {
        ui.checkbox(&mut room.allow_rotation, "Allow solver to rotate");
    }

    ui.add_space(8.0);
    ui.label("Tags:");
    let all_standard_tags = [
        RoomTag::Entrance,
        RoomTag::Boss,
        RoomTag::Trap,
        RoomTag::Treasure,
        RoomTag::Optional,
        RoomTag::Secret,
        RoomTag::Rest,
    ];
    for tag in &all_standard_tags {
        let mut has_tag = room.tags.contains(tag);
        if ui.checkbox(&mut has_tag, tag.label()).changed() {
            if has_tag {
                room.tags.push(tag.clone());
            } else {
                room.tags.retain(|t| t != tag);
            }
        }
    }

    ui.add_space(8.0);
    ui.label("Notes:");
    ui.text_edit_multiline(&mut room.notes);
}

fn connection_properties(ui: &mut egui::Ui, edge: &mut StoredEdge) {
    ui.label("Connection Type:");
    egui::ComboBox::from_id_salt("conn_type")
        .selected_text(edge.connection.connection_type.label())
        .show_ui(ui, |ui| {
            for ct in ConnectionType::ALL {
                ui.selectable_value(&mut edge.connection.connection_type, ct, ct.label());
            }
        });

    ui.add_space(8.0);
    ui.label("Corridor Width:");
    ui.add(egui::Slider::new(&mut edge.connection.corridor_width, 1..=4).suffix(" sq"));

    ui.add_space(8.0);
    ui.label("Label (optional):");
    let mut label = edge.connection.label.clone().unwrap_or_default();
    if ui.text_edit_singleline(&mut label).changed() {
        edge.connection.label = if label.is_empty() { None } else { Some(label) };
    }
}
