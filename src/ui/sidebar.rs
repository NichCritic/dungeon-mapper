use std::collections::HashSet;

use crate::model::*;
use crate::ui::graph_editor::Selection;

pub fn sidebar(
    ui: &mut egui::Ui,
    dungeon: &mut Dungeon,
    selection: &Selection,
    focus_label: &mut bool,
) {
    ui.heading("Properties");
    ui.separator();

    if !selection.rooms.is_empty() && selection.connections.is_empty() && selection.groups.is_empty() {
        let ids: Vec<String> = selection.rooms.iter().cloned().collect();
        if ids.len() == 1 {
            if let Some(room) = dungeon.graph.room_by_id_mut(&ids[0]) {
                room_properties(ui, room, focus_label);
            }
        } else {
            ui.label(format!("{} rooms selected", ids.len()));
            ui.separator();
            multi_room_properties(ui, dungeon, &ids);
        }
        ui.add_space(8.0);
        ui.separator();
        if ids.len() == 1 {
            room_group_membership(ui, dungeon, &ids[0]);
        } else {
            multi_room_group_membership(ui, dungeon, &selection.rooms);
        }
    } else if !selection.connections.is_empty() && selection.rooms.is_empty() && selection.groups.is_empty() {
        let ids: Vec<String> = selection.connections.iter().cloned().collect();
        if ids.len() == 1 {
            if let Some(edge) = dungeon.graph.connection_by_id_mut(&ids[0]) {
                connection_properties(ui, edge);
            }
        } else {
            ui.label(format!("{} connections selected", ids.len()));
            ui.separator();
            multi_connection_properties(ui, dungeon, &ids);
        }
    } else if let Some(id) = selection.single_group() {
        let id = id.to_string();
        group_properties(ui, dungeon, &id);
    } else if selection.is_empty() {
        ui.label("Select a room or connection to edit its properties.");
        ui.separator();
        dungeon_properties(ui, dungeon);
    }
}

/// Show a mixed-value indicator and return true if user edited the field.
fn mixed_label(ui: &mut egui::Ui, all_same: bool) {
    if !all_same {
        ui.colored_label(egui::Color32::from_rgb(200, 180, 80), "(mixed)");
    }
}

fn multi_room_properties(ui: &mut egui::Ui, dungeon: &mut Dungeon, ids: &[String]) {
    // Snapshot values to avoid borrow conflicts
    struct RoomSnap { hint: SizeHint, w: u32, h: u32, shape: RoomShape, rot: bool, tags: Vec<RoomTag>, floor: FloorAssignment }
    let snaps: Vec<RoomSnap> = ids.iter().filter_map(|id| {
        dungeon.graph.room_by_id(id).map(|r| {
            let (w, h) = r.grid_size();
            RoomSnap { hint: r.size_hint, w, h, shape: r.shape, rot: r.allow_rotation, tags: r.tags.clone(), floor: r.floor }
        })
    }).collect();
    if snaps.is_empty() { return; }

    // Size preset
    let all_same_hint = snaps.iter().all(|s| s.hint == snaps[0].hint);
    ui.label("Size Preset:");
    let label = if all_same_hint { snaps[0].hint.label() } else { "(mixed)" };
    let mut hint = snaps[0].hint;
    egui::ComboBox::from_id_salt("multi_size_hint")
        .selected_text(label)
        .show_ui(ui, |ui| {
            for h in SizeHint::ALL {
                ui.selectable_value(&mut hint, h, h.label());
            }
        });
    if hint != snaps[0].hint {
        for id in ids {
            if let Some(room) = dungeon.graph.room_by_id_mut(id) {
                room.size_hint = hint;
                room.grid_width = None;
                room.grid_height = None;
            }
        }
    }

    // Dimensions
    ui.add_space(8.0);
    let all_same_w = snaps.iter().all(|s| s.w == snaps[0].w);
    let all_same_h = snaps.iter().all(|s| s.h == snaps[0].h);
    ui.label("Dimensions:");
    let mut w = snaps[0].w;
    ui.horizontal(|ui| {
        if ui.add(egui::DragValue::new(&mut w).range(1..=20).prefix("W: ")).changed() {
            for id in ids {
                if let Some(room) = dungeon.graph.room_by_id_mut(id) {
                    room.grid_width = Some(w);
                }
            }
        }
        mixed_label(ui, all_same_w);
    });
    let mut h = snaps[0].h;
    ui.horizontal(|ui| {
        if ui.add(egui::DragValue::new(&mut h).range(1..=20).prefix("H: ")).changed() {
            for id in ids {
                if let Some(room) = dungeon.graph.room_by_id_mut(id) {
                    room.grid_height = Some(h);
                }
            }
        }
        mixed_label(ui, all_same_h);
    });

    // Shape
    ui.add_space(8.0);
    let all_same_shape = snaps.iter().all(|s| s.shape == snaps[0].shape);
    let shape_label = if all_same_shape { snaps[0].shape.label() } else { "(mixed)" };
    let mut shape = snaps[0].shape;
    ui.horizontal(|ui| {
        ui.label("Shape:");
        egui::ComboBox::from_id_salt("multi_shape")
            .selected_text(shape_label)
            .show_ui(ui, |ui| {
                for s in RoomShape::ALL {
                    ui.selectable_value(&mut shape, s, s.label());
                }
            });
    });
    if shape != snaps[0].shape {
        for id in ids {
            if let Some(room) = dungeon.graph.room_by_id_mut(id) {
                room.shape = shape;
            }
        }
    }

    // Tags
    ui.add_space(8.0);
    ui.label("Tags:");
    let all_tags = [
        RoomTag::Entrance, RoomTag::Boss, RoomTag::Trap, RoomTag::Treasure,
        RoomTag::Optional, RoomTag::Secret, RoomTag::Rest,
    ];
    for tag in &all_tags {
        let count_with = snaps.iter().filter(|s| s.tags.contains(tag)).count();
        let all_have = count_with == snaps.len();
        let none_have = count_with == 0;
        let mut checked = all_have;
        ui.horizontal(|ui| {
            if ui.checkbox(&mut checked, tag.label()).changed() {
                for id in ids {
                    if let Some(room) = dungeon.graph.room_by_id_mut(id) {
                        if checked && !room.tags.contains(tag) {
                            room.tags.push(tag.clone());
                        } else if !checked {
                            room.tags.retain(|t| t != tag);
                        }
                    }
                }
            }
            if !all_have && !none_have {
                ui.colored_label(egui::Color32::from_rgb(200, 180, 80), "(mixed)");
            }
        });
    }

    // Floor
    ui.add_space(8.0);
    let all_same_floor = snaps.iter().all(|s| s.floor == snaps[0].floor);
    let floor_label = if all_same_floor { snaps[0].floor.label() } else { "(mixed)".to_string() };
    ui.horizontal(|ui| {
        ui.label(format!("Floor: {}", floor_label));
    });
    let mut floor_val = match snaps[0].floor {
        FloorAssignment::Single(f) | FloorAssignment::Half(f, _) => f,
    };
    ui.horizontal(|ui| {
        if ui.add(egui::DragValue::new(&mut floor_val).speed(0.1).prefix("Set floor: ")).changed() {
            for id in ids {
                if let Some(room) = dungeon.graph.room_by_id_mut(id) {
                    room.floor = FloorAssignment::Single(floor_val);
                }
            }
        }
        mixed_label(ui, all_same_floor);
    });

    // Allow rotation
    ui.add_space(8.0);
    let all_same_rot = snaps.iter().all(|s| s.rot == snaps[0].rot);
    let mut rot = snaps[0].rot;
    ui.horizontal(|ui| {
        if ui.checkbox(&mut rot, "Allow solver to rotate").changed() {
            for id in ids {
                if let Some(room) = dungeon.graph.room_by_id_mut(id) {
                    room.allow_rotation = rot;
                }
            }
        }
        mixed_label(ui, all_same_rot);
    });
}

fn multi_connection_properties(ui: &mut egui::Ui, dungeon: &mut Dungeon, ids: &[String]) {
    // Snapshot values
    struct ConnSnap { conn_type: ConnectionType, width: u32, double: bool, min_len: Option<u32>, max_len: Option<u32> }
    let snaps: Vec<ConnSnap> = ids.iter().filter_map(|id| {
        dungeon.graph.connection_by_id_mut(id).map(|e| {
            ConnSnap {
                conn_type: e.connection.connection_type,
                width: e.connection.corridor_width,
                double: e.connection.double_door,
                min_len: e.connection.min_length,
                max_len: e.connection.max_length,
            }
        })
    }).collect();
    if snaps.is_empty() { return; }

    // Connection type
    let all_same_type = snaps.iter().all(|s| s.conn_type == snaps[0].conn_type);
    let type_label = if all_same_type { snaps[0].conn_type.label() } else { "(mixed)" };
    let mut ct = snaps[0].conn_type;
    ui.label("Connection Type:");
    egui::ComboBox::from_id_salt("multi_conn_type")
        .selected_text(type_label)
        .show_ui(ui, |ui| {
            for t in ConnectionType::ALL {
                ui.selectable_value(&mut ct, t, t.label());
            }
        });
    if ct != snaps[0].conn_type {
        for id in ids {
            if let Some(edge) = dungeon.graph.connection_by_id_mut(id) {
                edge.connection.connection_type = ct;
            }
        }
    }

    // Corridor width
    ui.add_space(8.0);
    let all_same_width = snaps.iter().all(|s| s.width == snaps[0].width);
    let mut w = snaps[0].width;
    ui.horizontal(|ui| {
        ui.label("Corridor Width:");
        if ui.add(egui::Slider::new(&mut w, 1..=4).suffix(" sq")).changed() {
            for id in ids {
                if let Some(edge) = dungeon.graph.connection_by_id_mut(id) {
                    edge.connection.corridor_width = w;
                }
            }
        }
        mixed_label(ui, all_same_width);
    });

    // Double door
    ui.add_space(8.0);
    let all_same_double = snaps.iter().all(|s| s.double == snaps[0].double);
    let mut dbl = snaps[0].double;
    ui.horizontal(|ui| {
        if ui.checkbox(&mut dbl, "Double door").changed() {
            for id in ids {
                if let Some(edge) = dungeon.graph.connection_by_id_mut(id) {
                    edge.connection.double_door = dbl;
                }
            }
        }
        mixed_label(ui, all_same_double);
    });

    // Length constraints
    ui.add_space(8.0);
    ui.label("Length Constraints:");

    let all_same_min = snaps.iter().all(|s| s.min_len == snaps[0].min_len);
    let mut has_min = snaps[0].min_len.is_some();
    let mut min_val = snaps[0].min_len.unwrap_or(1);
    ui.horizontal(|ui| {
        if ui.checkbox(&mut has_min, "Min:").changed() || ui.add_enabled(has_min, egui::DragValue::new(&mut min_val).range(0..=200).suffix(" sq")).changed() {
            for id in ids {
                if let Some(edge) = dungeon.graph.connection_by_id_mut(id) {
                    edge.connection.min_length = if has_min { Some(min_val) } else { None };
                }
            }
        }
        mixed_label(ui, all_same_min);
    });

    let all_same_max = snaps.iter().all(|s| s.max_len == snaps[0].max_len);
    let mut has_max = snaps[0].max_len.is_some();
    let mut max_val = snaps[0].max_len.unwrap_or(50);
    ui.horizontal(|ui| {
        if ui.checkbox(&mut has_max, "Max:").changed() || ui.add_enabled(has_max, egui::DragValue::new(&mut max_val).range(0..=200).suffix(" sq")).changed() {
            for id in ids {
                if let Some(edge) = dungeon.graph.connection_by_id_mut(id) {
                    edge.connection.max_length = if has_max { Some(max_val) } else { None };
                }
            }
        }
        mixed_label(ui, all_same_max);
    });
}

fn dungeon_properties(ui: &mut egui::Ui, dungeon: &mut Dungeon) {
    ui.label("Dungeon Name:");
    ui.text_edit_singleline(&mut dungeon.name);
    ui.add_space(8.0);
    ui.label(format!(
        "{} rooms, {} connections, {} groups",
        dungeon.graph.rooms.len(),
        dungeon.graph.connections.len(),
        dungeon.graph.groups.len(),
    ));

    ui.add_space(8.0);
    if ui.button("New Group").clicked() {
        let group = RoomGroup::new(format!("Group {}", dungeon.graph.groups.len() + 1));
        dungeon.graph.groups.push(group);
    }
}

fn room_group_membership(ui: &mut egui::Ui, dungeon: &mut Dungeon, room_id: &str) {
    ui.label("Groups:");
    for group in &mut dungeon.graph.groups {
        let mut in_group = group.room_ids.contains(&room_id.to_string());
        if ui.checkbox(&mut in_group, &group.label).changed() {
            if in_group {
                group.room_ids.push(room_id.to_string());
            } else {
                group.room_ids.retain(|id| id != room_id);
            }
        }
    }
}

fn multi_room_group_membership(ui: &mut egui::Ui, dungeon: &mut Dungeon, room_ids: &HashSet<String>) {
    ui.label("Groups:");
    for group in &mut dungeon.graph.groups {
        let all_in = room_ids.iter().all(|rid| group.room_ids.contains(rid));
        let none_in = room_ids.iter().all(|rid| !group.room_ids.contains(rid));

        let mut checked = all_in;
        let response = ui.checkbox(&mut checked, &group.label);

        // Show indeterminate state visually via label
        if !all_in && !none_in {
            ui.label("  (partial)");
        }

        if response.changed() {
            if checked {
                // Add all selected rooms to group
                for rid in room_ids {
                    if !group.room_ids.contains(rid) {
                        group.room_ids.push(rid.clone());
                    }
                }
            } else {
                // Remove all selected rooms from group
                group.room_ids.retain(|id| !room_ids.contains(id));
            }
        }
    }

    ui.add_space(8.0);
    if ui.button("New Group from Selection").clicked() {
        let mut group = RoomGroup::new(format!("Group {}", dungeon.graph.groups.len() + 1));
        group.room_ids = room_ids.iter().cloned().collect();
        dungeon.graph.groups.push(group);
    }
}

fn group_properties(ui: &mut egui::Ui, dungeon: &mut Dungeon, group_id: &str) {
    let group_idx = dungeon.graph.groups.iter().position(|g| g.id == group_id);
    let Some(idx) = group_idx else {
        ui.label("Group not found.");
        return;
    };

    ui.label("Group Label:");
    ui.text_edit_singleline(&mut dungeon.graph.groups[idx].label);

    ui.add_space(8.0);
    ui.label(format!("{} rooms", dungeon.graph.groups[idx].room_ids.len()));

    ui.add_space(8.0);
    ui.label("Solver Constraints:");

    let group = &mut dungeon.graph.groups[idx];

    let mut has_max_w = group.max_width.is_some();
    let mut max_w = group.max_width.unwrap_or(20);
    ui.horizontal(|ui| {
        ui.checkbox(&mut has_max_w, "Max width:");
        ui.add_enabled(has_max_w, egui::DragValue::new(&mut max_w).range(1..=100).suffix(" sq"));
    });
    group.max_width = if has_max_w { Some(max_w) } else { None };

    let mut has_max_h = group.max_height.is_some();
    let mut max_h = group.max_height.unwrap_or(20);
    ui.horizontal(|ui| {
        ui.checkbox(&mut has_max_h, "Max height:");
        ui.add_enabled(has_max_h, egui::DragValue::new(&mut max_h).range(1..=100).suffix(" sq"));
    });
    group.max_height = if has_max_h { Some(max_h) } else { None };

    ui.add_space(8.0);
    ui.label("Members:");
    let room_ids = dungeon.graph.groups[idx].room_ids.clone();
    for rid in &room_ids {
        if let Some(room) = dungeon.graph.room_by_id(rid) {
            ui.label(format!("  {}", room.label));
        }
    }

    ui.add_space(8.0);
    if ui.button("Delete Group").clicked() {
        dungeon.graph.groups.remove(idx);
    }
}

fn room_properties(ui: &mut egui::Ui, room: &mut Room, focus_label: &mut bool) {
    ui.label("Label:");
    let label_response = ui.text_edit_singleline(&mut room.label);
    if *focus_label {
        label_response.request_focus();
        // Select all text so typing replaces it
        if let Some(mut state) = egui::TextEdit::load_state(ui.ctx(), label_response.id) {
            let range = egui::text::CCursorRange::two(
                egui::text::CCursor::new(0),
                egui::text::CCursor::new(room.label.len()),
            );
            state.cursor.set_char_range(Some(range));
            state.store(ui.ctx(), label_response.id);
        }
        *focus_label = false;
    }

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

    // Shape-specific controls
    let (ew, eh) = room.grid_size();
    if room.shape == RoomShape::Rectangle && ew != eh {
        ui.checkbox(&mut room.allow_rotation, "Allow solver to rotate");
    }

    if room.shape == RoomShape::Cave {
        // Initialize cave_data if missing
        if room.cave_data.is_none() {
            room.cave_data = Some(CaveData {
                cells: Vec::new(),
                seed: rand::random(),
                algorithm: CaveAlgorithm::CellularAutomata,
                density: 0.45,
                smoothing_iterations: 4,
                generation: 0,
                contour_segments: Vec::new(),
            });
        }
        if let Some(cave) = &mut room.cave_data {
            ui.add_space(8.0);
            ui.label("Cave Settings:");
            egui::ComboBox::from_id_salt("cave_algorithm")
                .selected_text(cave.algorithm.label())
                .show_ui(ui, |ui| {
                    for alg in CaveAlgorithm::ALL {
                        ui.selectable_value(&mut cave.algorithm, alg, alg.label());
                    }
                });
            ui.horizontal(|ui| {
                ui.label("Seed:");
                ui.add(egui::DragValue::new(&mut cave.seed).speed(1.0));
                if ui.small_button("Rand").clicked() {
                    cave.seed = rand::random();
                    cave.cells.clear();
                    cave.generation += 1;
                }
            });
            ui.add(egui::Slider::new(&mut cave.density, 0.3..=0.7).text("Density"));
            if cave.algorithm == CaveAlgorithm::CellularAutomata {
                ui.add(egui::Slider::new(&mut cave.smoothing_iterations, 1..=10).text("Smoothing"));
            }
            if ui.button("Regenerate").clicked() {
                cave.cells.clear();
                cave.generation += 1;
            }
        }
    } else if room.cave_data.is_some() {
        // Switched away from Cave — clear cave data
        room.cave_data = None;
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

    // Floor assignment
    ui.add_space(8.0);
    ui.label("Floor:");
    let mut is_half = matches!(room.floor, FloorAssignment::Half(_, _));
    let (mut floor_a, mut floor_b) = match room.floor {
        FloorAssignment::Single(f) => (f, f + 1),
        FloorAssignment::Half(a, b) => (a, b),
    };
    ui.horizontal(|ui| {
        ui.add(egui::DragValue::new(&mut floor_a).speed(0.1).prefix("Floor: "));
        if is_half {
            ui.add(egui::DragValue::new(&mut floor_b).speed(0.1).prefix("& "));
        }
    });
    if ui.checkbox(&mut is_half, "Half floor (spans two)").changed() || floor_a != match room.floor { FloorAssignment::Single(f) | FloorAssignment::Half(f, _) => f } || (is_half && floor_b != match room.floor { FloorAssignment::Half(_, b) => b, _ => floor_a + 1 }) {
        if is_half {
            // Ensure the two floors are adjacent and ordered
            if floor_b <= floor_a {
                floor_b = floor_a + 1;
            }
            room.floor = FloorAssignment::Half(floor_a, floor_b);
        } else {
            room.floor = FloorAssignment::Single(floor_a);
        }
    }

    ui.add_space(8.0);
    ui.label("Notes:");
    ui.text_edit_multiline(&mut room.notes);

    // Decor section
    ui.add_space(8.0);
    ui.label("Decor:");
    let mut remove_idx = None;
    for (i, decor) in room.decor.iter_mut().enumerate() {
        ui.horizontal(|ui| {
            ui.label(decor.decor_type.label());
            ui.add(egui::DragValue::new(&mut decor.x).range(0.0..=20.0).speed(0.1).prefix("x:"));
            ui.add(egui::DragValue::new(&mut decor.y).range(0.0..=20.0).speed(0.1).prefix("y:"));
            if ui.small_button("X").clicked() {
                remove_idx = Some(i);
            }
        });
    }
    if let Some(idx) = remove_idx {
        room.decor.remove(idx);
    }
    ui.horizontal(|ui| {
        // Use a static-ish default for the combo box
        let mut new_type = DecorType::Table;
        egui::ComboBox::from_id_salt("add_decor_type")
            .selected_text(new_type.label())
            .width(90.0)
            .show_ui(ui, |ui| {
                for dt in DecorType::ALL {
                    ui.selectable_value(&mut new_type, dt, dt.label());
                }
            });
        if ui.button("Add").clicked() {
            let (w, h) = room.grid_size();
            let decor = RoomDecor::new(new_type, w as f32 / 2.0, h as f32 / 2.0);
            room.decor.push(decor);
        }
    });
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
    ui.checkbox(&mut edge.connection.double_door, "Double door");

    ui.add_space(8.0);
    ui.label("Length Constraints:");
    let mut has_min = edge.connection.min_length.is_some();
    let mut min_val = edge.connection.min_length.unwrap_or(1);
    ui.horizontal(|ui| {
        ui.checkbox(&mut has_min, "Min:");
        ui.add_enabled(has_min, egui::DragValue::new(&mut min_val).range(0..=200).suffix(" sq"));
    });
    edge.connection.min_length = if has_min { Some(min_val) } else { None };

    let mut has_max = edge.connection.max_length.is_some();
    let mut max_val = edge.connection.max_length.unwrap_or(50);
    ui.horizontal(|ui| {
        ui.checkbox(&mut has_max, "Max:");
        ui.add_enabled(has_max, egui::DragValue::new(&mut max_val).range(0..=200).suffix(" sq"));
    });
    edge.connection.max_length = if has_max { Some(max_val) } else { None };

    ui.add_space(8.0);
    ui.label("Label (optional):");
    let mut label = edge.connection.label.clone().unwrap_or_default();
    if ui.text_edit_singleline(&mut label).changed() {
        edge.connection.label = if label.is_empty() { None } else { Some(label) };
    }
}
