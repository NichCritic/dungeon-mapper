/// Help overlay (F8) — shows contextual usage info as tooltips when hovering UI panels.

const OVERLAY_DIM: egui::Color32 = egui::Color32::from_rgba_premultiplied(0, 0, 0, 120);
const TOOLTIP_BG: egui::Color32 = egui::Color32::from_rgb(30, 30, 40);
const TOOLTIP_BORDER: egui::Color32 = egui::Color32::from_rgb(100, 180, 255);
const HEADING_COLOR: egui::Color32 = egui::Color32::from_rgb(100, 200, 255);
const TEXT_COLOR: egui::Color32 = egui::Color32::WHITE;
const KEY_COLOR: egui::Color32 = egui::Color32::from_rgb(255, 200, 100);

/// Which region of the UI the pointer is over.
enum HoverRegion {
    Canvas,
    Sidebar,
    MenuBar,
    StatusBar,
    None,
}

pub fn help_overlay(
    ctx: &egui::Context,
    panel_rects: &[egui::Rect],
    current_view: &str,
    presenting: bool,
) {
    let screen_rect = ctx.screen_rect();
    let pointer_pos = ctx.pointer_hover_pos();

    // Dim layer
    let overlay_painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Middle,
        egui::Id::new("help_dim_layer"),
    ));
    overlay_painter.rect_filled(screen_rect, 0.0, OVERLAY_DIM);

    // Highlight hovered panel
    if let Some(pos) = pointer_pos {
        if let Some(panel_rect) = panel_rects.iter().find(|r| r.contains(pos)) {
            overlay_painter.rect_stroke(
                *panel_rect, 4.0,
                egui::Stroke::new(2.0, TOOLTIP_BORDER),
                egui::StrokeKind::Outside,
            );
        }
    }

    // Determine hover region
    let region = if let Some(pos) = pointer_pos {
        // panel_rects order: [menu_bar, status_bar, sidebar, canvas]
        if panel_rects.len() >= 4 {
            if panel_rects[0].contains(pos) { HoverRegion::MenuBar }
            else if panel_rects[1].contains(pos) { HoverRegion::StatusBar }
            else if panel_rects[2].contains(pos) { HoverRegion::Sidebar }
            else if panel_rects[3].contains(pos) { HoverRegion::Canvas }
            else { HoverRegion::None }
        } else {
            HoverRegion::None
        }
    } else {
        HoverRegion::None
    };

    // Build help text based on region + current view
    let help = match region {
        HoverRegion::MenuBar => help_menu_bar(),
        HoverRegion::StatusBar => help_status_bar(),
        HoverRegion::Canvas => {
            if presenting {
                help_canvas_presentation()
            } else {
                help_canvas(current_view)
            }
        }
        HoverRegion::Sidebar => {
            if presenting {
                help_sidebar_presentation()
            } else {
                help_sidebar(current_view)
            }
        }
        HoverRegion::None => help_general(),
    };

    // Draw tooltip near pointer
    let tooltip_pos = pointer_pos.unwrap_or(screen_rect.center());
    draw_help_tooltip(ctx, tooltip_pos, &help);

    // Always show the F8 badge
    let badge_painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::new("help_badge"),
    ));
    let badge_pos = egui::pos2(screen_rect.center_top().x, screen_rect.min.y + 30.0);
    let badge_text = "HELP (F8) - Hover over any area for info";
    let galley = badge_painter.layout_no_wrap(
        badge_text.to_string(),
        egui::FontId::proportional(13.0),
        HEADING_COLOR,
    );
    let badge_rect = egui::Rect::from_center_size(
        badge_pos,
        galley.size() + egui::vec2(16.0, 8.0),
    );
    badge_painter.rect_filled(badge_rect, 6.0, TOOLTIP_BG);
    badge_painter.rect_stroke(badge_rect, 6.0, egui::Stroke::new(1.0, TOOLTIP_BORDER), egui::StrokeKind::Outside);
    badge_painter.text(badge_pos, egui::Align2::CENTER_CENTER, badge_text, egui::FontId::proportional(13.0), HEADING_COLOR);
}

struct HelpContent {
    title: &'static str,
    lines: Vec<HelpLine>,
}

enum HelpLine {
    Text(&'static str),
    Key(&'static str, &'static str), // key, description
    Blank,
}

fn draw_help_tooltip(ctx: &egui::Context, pos: egui::Pos2, help: &HelpContent) {
    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::new("help_tooltip"),
    ));

    let font = egui::FontId::proportional(12.0);
    let heading_font = egui::FontId::proportional(14.0);
    let line_height = 16.0;

    // Measure content
    let mut max_width: f32 = painter.layout_no_wrap(help.title.to_string(), heading_font.clone(), HEADING_COLOR).size().x;
    for line in &help.lines {
        let w = match line {
            HelpLine::Text(t) => painter.layout_no_wrap(t.to_string(), font.clone(), TEXT_COLOR).size().x,
            HelpLine::Key(k, d) => {
                painter.layout_no_wrap(k.to_string(), font.clone(), KEY_COLOR).size().x
                + 8.0
                + painter.layout_no_wrap(d.to_string(), font.clone(), TEXT_COLOR).size().x
            }
            HelpLine::Blank => 0.0,
        };
        max_width = max_width.max(w);
    }

    let padding = 12.0;
    let content_height = line_height + (help.lines.len() as f32 * line_height) + padding;
    let tooltip_size = egui::vec2(max_width + padding * 2.0, content_height + padding);

    // Position tooltip so it stays on screen
    let screen = ctx.screen_rect();
    let mut tl = pos + egui::vec2(16.0, 16.0);
    if tl.x + tooltip_size.x > screen.max.x { tl.x = pos.x - tooltip_size.x - 8.0; }
    if tl.y + tooltip_size.y > screen.max.y { tl.y = pos.y - tooltip_size.y - 8.0; }
    tl.x = tl.x.max(screen.min.x);
    tl.y = tl.y.max(screen.min.y);

    let tooltip_rect = egui::Rect::from_min_size(tl, tooltip_size);
    painter.rect_filled(tooltip_rect, 8.0, TOOLTIP_BG);
    painter.rect_stroke(tooltip_rect, 8.0, egui::Stroke::new(1.0, TOOLTIP_BORDER), egui::StrokeKind::Outside);

    let mut y = tl.y + padding;
    let x = tl.x + padding;

    // Title
    painter.text(egui::pos2(x, y), egui::Align2::LEFT_TOP, help.title, heading_font, HEADING_COLOR);
    y += line_height + 4.0;

    // Lines
    for line in &help.lines {
        match line {
            HelpLine::Text(t) => {
                painter.text(egui::pos2(x, y), egui::Align2::LEFT_TOP, *t, font.clone(), TEXT_COLOR);
            }
            HelpLine::Key(k, d) => {
                let key_galley = painter.layout_no_wrap(k.to_string(), font.clone(), KEY_COLOR);
                let kw = key_galley.size().x;
                painter.text(egui::pos2(x, y), egui::Align2::LEFT_TOP, *k, font.clone(), KEY_COLOR);
                painter.text(egui::pos2(x + kw + 8.0, y), egui::Align2::LEFT_TOP, *d, font.clone(), TEXT_COLOR);
            }
            HelpLine::Blank => {}
        }
        y += line_height;
    }
}

// --- Help content for each region/view ---

fn help_general() -> HelpContent {
    HelpContent {
        title: "Dungeon Mapper",
        lines: vec![
            HelpLine::Text("Hover over any panel for contextual help."),
            HelpLine::Blank,
            HelpLine::Key("Ctrl+S", "Save"),
            HelpLine::Key("Ctrl+Z", "Undo"),
            HelpLine::Key("Ctrl+Y", "Redo"),
            HelpLine::Key("F7", "Annotation mode"),
            HelpLine::Key("F8", "Help overlay (this)"),
        ],
    }
}

fn help_menu_bar() -> HelpContent {
    HelpContent {
        title: "Menu Bar",
        lines: vec![
            HelpLine::Text("File: New, Open, Save, Save As, Export"),
            HelpLine::Text("Click a tab name to switch views."),
            HelpLine::Text("Present button enters presentation mode."),
            HelpLine::Blank,
            HelpLine::Key("Ctrl+S", "Save / Save As"),
        ],
    }
}

fn help_status_bar() -> HelpContent {
    HelpContent {
        title: "Status Bar",
        lines: vec![
            HelpLine::Text("Shows save state, room/connection count, and zoom level."),
            HelpLine::Text("'Loading: ...' appears when render caches are building."),
        ],
    }
}

fn help_canvas(view: &str) -> HelpContent {
    match view {
        "Graph" => HelpContent {
            title: "Graph Canvas",
            lines: vec![
                HelpLine::Key("Double-click", "Create new room"),
                HelpLine::Key("Click", "Select room"),
                HelpLine::Key("Shift+click", "Toggle selection (multi-select)"),
                HelpLine::Key("Ctrl+click", "Connect selected rooms to clicked room"),
                HelpLine::Key("Right-drag", "Draw connection between rooms"),
                HelpLine::Key("Drag room", "Move selected rooms"),
                HelpLine::Key("Drag empty", "Marquee selection"),
                HelpLine::Key("Delete", "Delete selected items"),
                HelpLine::Key("Ctrl+C/V", "Copy/paste rooms"),
                HelpLine::Blank,
                HelpLine::Key("Scroll", "Zoom"),
                HelpLine::Key("Middle-drag", "Pan"),
            ],
        },
        "Spatial" => HelpContent {
            title: "Spatial Canvas",
            lines: vec![
                HelpLine::Key("Drag room", "Move room"),
                HelpLine::Key("Drag waypoint", "Adjust corridor routing"),
                HelpLine::Key("Drag exit handle", "Route connection entry point"),
                HelpLine::Key("Double-click corridor", "Insert waypoint"),
                HelpLine::Key("Delete", "Delete selected waypoint/section"),
                HelpLine::Blank,
                HelpLine::Key("Scroll", "Zoom"),
                HelpLine::Key("Middle-drag", "Pan"),
            ],
        },
        "Decor" => HelpContent {
            title: "Decor Canvas",
            lines: vec![
                HelpLine::Key("Click room", "Select room for editing"),
                HelpLine::Key("Click decor", "Select decor item"),
                HelpLine::Key("Drag decor", "Move decor item"),
                HelpLine::Key("Right-drag", "Box-select multiple decor items"),
                HelpLine::Key("Delete", "Delete selected decor"),
                HelpLine::Text("Enable 'place mode' in sidebar to click-place new decor."),
                HelpLine::Blank,
                HelpLine::Key("Scroll", "Zoom"),
                HelpLine::Key("Middle-drag", "Pan"),
            ],
        },
        "Encounters" => HelpContent {
            title: "Encounters Canvas",
            lines: vec![
                HelpLine::Key("Click room", "Select room to see its encounters"),
                HelpLine::Text("Red markers = static encounters"),
                HelpLine::Text("Orange markers = wandering encounters"),
                HelpLine::Text("Purple markers = hazards"),
                HelpLine::Blank,
                HelpLine::Key("Scroll", "Zoom"),
                HelpLine::Key("Middle-drag", "Pan"),
            ],
        },
        "Styled" => HelpContent {
            title: "Styled Canvas",
            lines: vec![
                HelpLine::Key("Click room", "Select for sidebar details"),
                HelpLine::Text("This is the final themed map view."),
                HelpLine::Text("Use sidebar to toggle grid, labels, secrets."),
                HelpLine::Text("Export DM or player maps as PNG."),
                HelpLine::Blank,
                HelpLine::Key("Scroll", "Zoom"),
                HelpLine::Key("Middle-drag", "Pan"),
            ],
        },
        _ => help_general(),
    }
}

fn help_sidebar(view: &str) -> HelpContent {
    match view {
        "Graph" => HelpContent {
            title: "Graph Sidebar",
            lines: vec![
                HelpLine::Text("Edit properties of selected rooms and connections."),
                HelpLine::Text("Set room label, shape, size, floor, notes."),
                HelpLine::Text("Set connection type (door, locked, secret, open)."),
                HelpLine::Text("Create and manage room groups."),
            ],
        },
        "Spatial" => HelpContent {
            title: "Spatial Sidebar",
            lines: vec![
                HelpLine::Text("'Recompute All' re-solves the layout from scratch."),
                HelpLine::Text("Adjust density gap for room spacing."),
                HelpLine::Text("Add bounds rectangles to constrain layout."),
                HelpLine::Text("Rotate rooms, add elevation sections."),
                HelpLine::Text("Filter by floor."),
            ],
        },
        "Decor" => HelpContent {
            title: "Decor Sidebar",
            lines: vec![
                HelpLine::Text("Select a decor type and use 'Start Placing' to click-place."),
                HelpLine::Text("Edit selected decor: type, position, rotation, scale."),
                HelpLine::Text("Lighting section: ambient light and room light sources."),
                HelpLine::Text("Filter by floor."),
            ],
        },
        "Encounters" => HelpContent {
            title: "Encounters Sidebar",
            lines: vec![
                HelpLine::Text("Add encounters and assign monsters from the bestiary."),
                HelpLine::Text("Monster Browser: search, filter by CR/type, add to encounters."),
                HelpLine::Text("Monster Workshop: merge monsters, edit custom creatures."),
                HelpLine::Text("Mark encounters as hazards with damage/save/condition."),
                HelpLine::Text("Import/export encounters and creatures as JSON."),
            ],
        },
        "Styled" => HelpContent {
            title: "Styled Sidebar",
            lines: vec![
                HelpLine::Text("Toggle rendering options: grid, labels, notes, secrets."),
                HelpLine::Text("Exterior shading: solid, hatched, or stippled."),
                HelpLine::Text("Corridor chamfer style."),
                HelpLine::Text("Export DM Map (with secrets) or Player Map as PNG."),
            ],
        },
        _ => help_general(),
    }
}

fn help_canvas_presentation() -> HelpContent {
    HelpContent {
        title: "Presentation Canvas",
        lines: vec![
            HelpLine::Key("Click room", "Select room for sidebar controls"),
            HelpLine::Key("Right-click room", "Quick visibility/door menu"),
            HelpLine::Key("Right-click corridor", "Toggle door open/closed"),
            HelpLine::Key("Drag green rect", "Pan the player view"),
            HelpLine::Blank,
            HelpLine::Text("Room overlays show visibility state:"),
            HelpLine::Text("  Dark = Hidden, Dim = Explored, Clear = Visible"),
            HelpLine::Blank,
            HelpLine::Key("Scroll", "Zoom"),
            HelpLine::Key("Middle-drag", "Pan"),
        ],
    }
}

fn help_sidebar_presentation() -> HelpContent {
    HelpContent {
        title: "Presentation Sidebar",
        lines: vec![
            HelpLine::Text("Click a room on the map for room-specific controls."),
            HelpLine::Blank,
            HelpLine::Text("Room selected: visibility, doors, encounters, combat."),
            HelpLine::Text("No selection: full room/door lists, party, encounters."),
            HelpLine::Blank,
            HelpLine::Text("Encounters: Tick moves wandering encounters."),
            HelpLine::Text("Autobattle: encounters fight when sharing a room."),
            HelpLine::Text("Start Combat: initiative tracker with attack rolls."),
            HelpLine::Text("Combat Simulator: test encounters against each other."),
        ],
    }
}
