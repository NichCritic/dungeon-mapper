//! Tools for where things sit on the map: moving and turning placed rooms, groups and
//! containment, elevation sections inside rooms, and bounds rectangles.

use serde_json::{json, Value};

use super::tools::{
    label_of, names, opt_bool, opt_f32, opt_str, opt_u32, parse_enum, req_str, resolve_room, tool, ToolCtx,
};
use crate::model::{BoundsRect, ElevationSection, ElevationType, RoomGroup};
use crate::ui::spatial_view;

pub fn definitions() -> Vec<Value> {
    let room = json!({ "type": "string", "description": "Room id, or its label (case-insensitive)" });
    let group = json!({ "type": "string", "description": "Group id, or its label (case-insensitive)" });
    let rooms = json!({ "type": "array", "items": { "type": "string" }, "description": "Rooms, by id or label" });
    let section_fields = json!({
        "elevation": { "type": "string", "enum": names(&ElevationType::ALL), "description": "Heights (Raised, Lowered, Steps, Slope, pits, Hole), Water, or ground cover: Rough (difficult), TallGrass (lightly obscures), Thicket (blocks sight, difficult)" },
        "x": { "type": "number", "description": "Grid units from the room's left edge" },
        "y": { "type": "number", "description": "Grid units from the room's top edge" },
        "width": { "type": "number" },
        "length": { "type": "number" },
        "height": { "type": "number", "description": "Feet up (Raised), down (Lowered) or deep (Water)" },
        "opaque": { "type": "boolean", "description": "Hides the floor beneath it (drawing only)" },
        "organic": { "type": "boolean", "description": "A natural, irregular outline inside the box (a copse, bog or pond) instead of a rectangle" },
    });
    let mut section_update = section_fields.clone();
    section_update["room"] = room.clone();
    section_update["section"] = json!({ "type": "string", "description": "Section id from get_room" });
    let mut section_add = section_fields;
    section_add["room"] = room.clone();
    let group_fields = json!({
        "label": { "type": "string" },
        "container": { "type": ["string", "null"], "description": "A room the group's rooms sit inside (nested rooms); null for none" },
        "padding": { "type": "integer", "minimum": 0, "description": "Squares between the container's walls and the rooms inside" },
        "max_width": { "type": ["integer", "null"], "description": "Keep the group within this many squares across; null for no limit" },
        "max_height": { "type": ["integer", "null"], "description": "Keep the group within this many squares down; null for no limit" },
    });
    let mut group_create = group_fields.clone();
    group_create["rooms"] = rooms.clone();
    let mut group_update = group_fields;
    group_update["group"] = group.clone();
    group_update["add_rooms"] = rooms.clone();
    group_update["remove_rooms"] = rooms;
    vec![
        tool("move_room",
            "Move or turn a placed room, as dragging it on the Spatial view does: rooms inside it move along, a room inside a container stays within it, and its corridors are re-routed.",
            json!({
                "room": room,
                "x": { "type": "integer", "description": "New left edge, in grid squares" },
                "y": { "type": "integer", "description": "New top edge, in grid squares" },
                "rotation": { "type": "number", "description": "Degrees, -180 to 180 (0 = square to the grid)" },
            }), &["room"]),
        tool("create_group",
            "Group rooms so the layout keeps them together. With a container, they are nested inside that room.",
            group_create, &["label"]),
        tool("update_group", "Change a group: label, members, container, padding or size limits. Only the fields given change.",
            group_update, &["group"]),
        tool("delete_group", "Remove a group (its rooms stay).", json!({ "group": group }), &["group"]),
        tool("transform_group",
            "Duplicate a group (copying its rooms and the connections between them), turn it 90°, or mirror it.",
            json!({ "group": group, "action": { "type": "string", "enum": ["duplicate", "rotate", "flip_horizontal", "flip_vertical"] } }),
            &["group", "action"]),
        tool("add_section", "Add an elevation section to a room: a dais, pit, steps, slope, hole or water.",
            section_add, &["room", "elevation", "x", "y", "width", "length"]),
        tool("update_section", "Change an elevation section. Only the fields given change.",
            section_update, &["room", "section"]),
        tool("remove_section", "Remove an elevation section from a room.",
            json!({ "room": room, "section": { "type": "string", "description": "Section id from get_room" } }), &["room", "section"]),
        tool("add_bounds", "Draw a labelled bounds rectangle around the whole map as laid out.",
            json!({ "label": { "type": "string" } }), &[]),
        tool("remove_bounds", "Remove a bounds rectangle.",
            json!({ "bounds": { "description": "Its label, or its index from get_overview", "oneOf": [{ "type": "string" }, { "type": "integer" }] } }),
            &["bounds"]),
    ]
}

/// Run a layout tool; `None` if the tool isn't one of these.
pub fn run(ctx: &mut ToolCtx, tool: &str, args: &Value) -> Option<Result<Value, String>> {
    Some(match tool {
        "move_room" => move_room(ctx, args),
        "create_group" => create_group(ctx, args),
        "update_group" => update_group(ctx, args),
        "delete_group" => delete_group(ctx, args),
        "transform_group" => transform_group(ctx, args),
        "add_section" => add_section(ctx, args),
        "update_section" => update_section(ctx, args),
        "remove_section" => remove_section(ctx, args),
        "add_bounds" => add_bounds(ctx, args),
        "remove_bounds" => remove_bounds(ctx, args),
        _ => return None,
    })
}

// ------------------------------------------------------------------ rooms

fn move_room(ctx: &mut ToolCtx, args: &Value) -> Result<Value, String> {
    let id = resolve_room(ctx.dungeon, req_str(args, "room")?)?;
    let rl = ctx.dungeon.layout.as_ref().and_then(|l| l.room_by_id(&id))
        .ok_or("That room isn't placed on the map yet; it gets placed when the user opens a map view.")?;
    let whole = |key: &str| match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v.as_i64().map(|n| Some(n as i32)).ok_or(format!("'{}' must be a whole number", key)),
    };
    let (dx, dy) = (whole("x")?.map_or(0, |x| x - rl.x), whole("y")?.map_or(0, |y| y - rl.y));
    let rotation = opt_f32(args, "rotation")?.map(|r| (r + 180.0).rem_euclid(360.0) - 180.0);
    if dx != 0 || dy != 0 {
        spatial_view::move_room_by(ctx.dungeon, &id, dx, dy);
    }
    if let Some(r) = rotation {
        if let Some(rl) = ctx.dungeon.layout.as_mut().and_then(|l| l.room_by_id_mut(&id)) {
            rl.rotation = r;
        }
    }
    spatial_view::reroute_room(ctx.dungeon, &id);
    ctx.spatial.cave_contours_dirty = true;
    let rl = ctx.dungeon.layout.as_ref().and_then(|l| l.room_by_id(&id)).ok_or("Room vanished")?;
    Ok(json!({ "room": label_of(ctx.dungeon, &id), "x": rl.x, "y": rl.y, "rotation": rl.rotation }))
}

// ------------------------------------------------------------------ groups

fn resolve_group(ctx: &ToolCtx, g: &str) -> Result<usize, String> {
    let groups = &ctx.dungeon.graph.groups;
    if let Some(i) = groups.iter().position(|x| x.id == g) {
        return Ok(i);
    }
    let matches: Vec<usize> = (0..groups.len()).filter(|i| groups[*i].label.eq_ignore_ascii_case(g.trim())).collect();
    match matches.as_slice() {
        [i] => Ok(*i),
        [] => Err(format!("No group '{}'. get_overview lists them.", g)),
        _ => Err(format!("Several groups are called '{}'; use its id.", g)),
    }
}

fn room_list(ctx: &ToolCtx, args: &Value, key: &str) -> Result<Vec<String>, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Array(list)) => list.iter()
            .map(|r| r.as_str().ok_or(format!("'{}' must be a list of rooms", key)).and_then(|r| resolve_room(ctx.dungeon, r)))
            .collect(),
        Some(_) => Err(format!("'{}' must be a list of rooms", key)),
    }
}

fn group_summary(ctx: &ToolCtx, g: &RoomGroup) -> Value {
    json!({
        "id": g.id,
        "label": g.label,
        "rooms": g.room_ids.iter().map(|id| label_of(ctx.dungeon, id)).collect::<Vec<_>>(),
        "container": g.parent_room_id.as_deref().map(|id| label_of(ctx.dungeon, id)),
        "padding": g.containment_padding,
        "max_width": g.max_width,
        "max_height": g.max_height,
    })
}

/// Apply the fields shared by create and update; leaves the graph unchanged on error.
fn apply_group_fields(ctx: &mut ToolCtx, gi: usize, args: &Value) -> Result<(), String> {
    let before = ctx.dungeon.graph.groups[gi].clone();
    let add = room_list(ctx, args, "add_rooms")?;
    let remove = room_list(ctx, args, "remove_rooms")?;
    let container = match args.get("container") {
        None => None,
        Some(Value::Null) => Some(None),
        Some(v) => Some(Some(resolve_room(ctx.dungeon, v.as_str().ok_or("'container' must be a room")?)?)),
    };
    let size = |key: &str| match args.get(key) {
        None => Ok(None),
        Some(Value::Null) => Ok(Some(None)),
        Some(_) => opt_u32(args, key).map(Some),
    };
    let (max_width, max_height) = (size("max_width")?, size("max_height")?);
    let padding = match args.get("padding") {
        None | Some(Value::Null) => None,
        Some(v) => Some(v.as_u64().filter(|n| *n <= 20).ok_or("'padding' must be 0 to 20")? as u32),
    };

    let g = &mut ctx.dungeon.graph.groups[gi];
    if let Some(label) = opt_str(args, "label").map(str::trim).filter(|l| !l.is_empty()) {
        g.label = label.to_string();
    }
    for id in add {
        if !g.room_ids.contains(&id) {
            g.room_ids.push(id);
        }
    }
    g.room_ids.retain(|id| !remove.contains(id));
    if let Some(c) = container {
        g.parent_room_id = c;
    }
    if let Some(p) = padding {
        g.containment_padding = p;
    }
    if let Some(w) = max_width {
        g.max_width = w;
    }
    if let Some(h) = max_height {
        g.max_height = h;
    }
    if g.parent_room_id.as_ref().is_some_and(|p| g.room_ids.contains(p)) {
        ctx.dungeon.graph.groups[gi] = before;
        return Err("A room can't be inside a group it contains.".into());
    }
    if let Some(cycle) = ctx.dungeon.graph.containment_cycle() {
        ctx.dungeon.graph.groups[gi] = before;
        return Err(format!("That would nest rooms inside themselves: {}", cycle));
    }
    Ok(())
}

/// Move the group's placed rooms that sit outside its container to free spots inside
/// it (the layout keeps placed rooms where they are, so setting a container alone
/// doesn't). Returns the rooms moved and those too big to fit.
fn pull_inside_container(ctx: &mut ToolCtx, gi: usize) -> (Vec<String>, Vec<String>) {
    let g = &ctx.dungeon.graph.groups[gi];
    let Some(parent) = g.parent_room_id.clone() else { return Default::default() };
    let pad = g.containment_padding as i32;
    let members = g.room_ids.clone();
    let Some(layout) = ctx.dungeon.layout.as_ref() else { return Default::default() };
    let Some(p) = layout.room_by_id(&parent) else { return Default::default() };
    let (x0, y0, x1, y1) = (p.x + pad, p.y + pad, p.x + p.width as i32 - pad, p.y + p.height as i32 - pad);
    let inside = |x: i32, y: i32, w: u32, h: u32| x >= x0 && y >= y0 && x + w as i32 <= x1 && y + h as i32 <= y1;

    let (mut moved, mut too_big) = (Vec::new(), Vec::new());
    for id in members {
        let layout = ctx.dungeon.layout.as_ref().unwrap();
        let Some(rl) = layout.room_by_id(&id) else { continue };
        let (w, h) = (rl.width, rl.height);
        if inside(rl.x, rl.y, w, h) {
            continue;
        }
        // Rooms already inside, which a newcomer mustn't overlap.
        let occupied: Vec<(i32, i32, u32, u32)> = layout.rooms.iter()
            .filter(|o| o.room_id != id && o.room_id != parent && inside(o.x, o.y, o.width, o.height))
            .map(|o| (o.x, o.y, o.width, o.height))
            .collect();
        let free = |x: i32, y: i32| occupied.iter().all(|&(ox, oy, ow, oh)| {
            x + w as i32 <= ox || ox + ow as i32 <= x || y + h as i32 <= oy || oy + oh as i32 <= y
        });
        let spot = (y0..=y1 - h as i32).flat_map(|y| (x0..=x1 - w as i32).map(move |x| (x, y))).find(|&(x, y)| free(x, y));
        let label = label_of(ctx.dungeon, &id);
        match spot {
            Some((x, y)) => {
                let (dx, dy) = (x - rl.x, y - rl.y);
                spatial_view::move_room_by(ctx.dungeon, &id, dx, dy);
                spatial_view::reroute_room(ctx.dungeon, &id);
                moved.push(label);
            }
            None => too_big.push(label),
        }
    }
    if !moved.is_empty() {
        ctx.spatial.cave_contours_dirty = true;
    }
    (moved, too_big)
}

/// A group's summary, after pulling its rooms inside its container.
fn settle_group(ctx: &mut ToolCtx, gi: usize) -> Value {
    let (moved, too_big) = pull_inside_container(ctx, gi);
    let mut v = group_summary(ctx, &ctx.dungeon.graph.groups[gi]);
    if !moved.is_empty() {
        v["moved_inside_container"] = json!(moved);
    }
    if !too_big.is_empty() {
        v["too_big_for_container"] = json!(too_big);
    }
    v
}

fn create_group(ctx: &mut ToolCtx, args: &Value) -> Result<Value, String> {
    let label = req_str(args, "label")?.trim();
    if label.is_empty() {
        return Err("A group needs a label".into());
    }
    let mut g = RoomGroup::new(label.to_string());
    g.room_ids = room_list(ctx, args, "rooms")?;
    ctx.dungeon.graph.groups.push(g);
    let gi = ctx.dungeon.graph.groups.len() - 1;
    if let Err(e) = apply_group_fields(ctx, gi, args) {
        ctx.dungeon.graph.groups.pop();
        return Err(e);
    }
    Ok(settle_group(ctx, gi))
}

fn update_group(ctx: &mut ToolCtx, args: &Value) -> Result<Value, String> {
    let gi = resolve_group(ctx, req_str(args, "group")?)?;
    apply_group_fields(ctx, gi, args)?;
    Ok(settle_group(ctx, gi))
}

fn delete_group(ctx: &mut ToolCtx, args: &Value) -> Result<Value, String> {
    let gi = resolve_group(ctx, req_str(args, "group")?)?;
    let g = ctx.dungeon.graph.groups.remove(gi);
    ctx.graph.selection.groups.remove(&g.id);
    Ok(json!({ "deleted": g.label }))
}

fn transform_group(ctx: &mut ToolCtx, args: &Value) -> Result<Value, String> {
    let gi = resolve_group(ctx, req_str(args, "group")?)?;
    if ctx.dungeon.layout.is_none() {
        return Err("The map hasn't been laid out yet: open the Spatial view in the app first.".into());
    }
    let room_ids = ctx.dungeon.graph.groups[gi].room_ids.clone();
    match req_str(args, "action")? {
        "duplicate" => {
            let before = ctx.dungeon.graph.groups.len();
            spatial_view::duplicate_group(ctx.dungeon, &room_ids, gi);
            let Some(copy) = ctx.dungeon.graph.groups.get(before) else {
                return Err("Nothing to duplicate: the group's rooms aren't placed yet.".into());
            };
            return Ok(json!({ "duplicated": group_summary(ctx, copy) }));
        }
        "rotate" => spatial_view::rotate_group(ctx.dungeon, &room_ids),
        "flip_horizontal" => spatial_view::flip_group(ctx.dungeon, &room_ids, true),
        "flip_vertical" => spatial_view::flip_group(ctx.dungeon, &room_ids, false),
        other => return Err(format!("Unknown action '{}'", other)),
    }
    ctx.spatial.cave_contours_dirty = true;
    Ok(group_summary(ctx, &ctx.dungeon.graph.groups[gi]))
}

// ------------------------------------------------------------------ sections

fn apply_section_fields(s: &mut ElevationSection, args: &Value, room_size: (u32, u32)) -> Result<(), String> {
    if let Some(e) = opt_str(args, "elevation") {
        s.elevation = parse_enum(&ElevationType::ALL, e, "elevation")?;
    }
    for (key, field) in [("x", &mut s.x), ("y", &mut s.y), ("width", &mut s.width), ("length", &mut s.length), ("height", &mut s.height)] {
        if let Some(v) = opt_f32(args, key)? {
            *field = v;
        }
    }
    if let Some(o) = opt_bool(args, "opaque")? {
        s.opaque = o;
    }
    if let Some(o) = opt_bool(args, "organic")? {
        s.organic = o;
    }
    let (w, h) = (room_size.0 as f32, room_size.1 as f32);
    if s.width <= 0.0 || s.length <= 0.0 {
        return Err("A section needs a width and length above 0".into());
    }
    if s.x < 0.0 || s.y < 0.0 || s.x + s.width > w || s.y + s.length > h {
        return Err(format!("That section doesn't fit in the room, which is {}x{} squares", w, h));
    }
    Ok(())
}

fn add_section(ctx: &mut ToolCtx, args: &Value) -> Result<Value, String> {
    let id = resolve_room(ctx.dungeon, req_str(args, "room")?)?;
    let elevation = parse_enum(&ElevationType::ALL, req_str(args, "elevation")?, "elevation")?;
    let room = ctx.dungeon.graph.room_by_id_mut(&id).ok_or("Room vanished")?;
    let mut s = ElevationSection::new(elevation, 0.0, 0.0, 1.0, 1.0);
    apply_section_fields(&mut s, args, room.grid_size())?;
    let v = serde_json::to_value(&s).unwrap_or_default();
    room.sections.push(s);
    Ok(v)
}

fn update_section(ctx: &mut ToolCtx, args: &Value) -> Result<Value, String> {
    let id = resolve_room(ctx.dungeon, req_str(args, "room")?)?;
    let section_id = req_str(args, "section")?;
    let room = ctx.dungeon.graph.room_by_id_mut(&id).ok_or("Room vanished")?;
    let size = room.grid_size();
    let s = room.sections.iter_mut().find(|s| s.id == section_id)
        .ok_or_else(|| format!("No section '{}' in that room. get_room lists them.", section_id))?;
    let mut changed = s.clone();
    apply_section_fields(&mut changed, args, size)?;
    *s = changed;
    Ok(serde_json::to_value(&*s).unwrap_or_default())
}

fn remove_section(ctx: &mut ToolCtx, args: &Value) -> Result<Value, String> {
    let id = resolve_room(ctx.dungeon, req_str(args, "room")?)?;
    let section_id = req_str(args, "section")?;
    let room = ctx.dungeon.graph.room_by_id_mut(&id).ok_or("Room vanished")?;
    let i = room.sections.iter().position(|s| s.id == section_id)
        .ok_or_else(|| format!("No section '{}' in that room. get_room lists them.", section_id))?;
    let s = room.sections.remove(i);
    Ok(json!({ "removed": s.elevation.label() }))
}

// ------------------------------------------------------------------ bounds

fn add_bounds(ctx: &mut ToolCtx, args: &Value) -> Result<Value, String> {
    let layout = ctx.dungeon.layout.as_mut()
        .ok_or("The map hasn't been laid out yet: open the Spatial view in the app first.")?;
    let (min_x, min_y, max_x, max_y) = layout.extents();
    let label = opt_str(args, "label").unwrap_or("Bounds").to_string();
    // Sized like the Spatial sidebar's button: the map plus a 2-square margin.
    let b = BoundsRect {
        label,
        x: min_x - 2,
        y: min_y - 2,
        width: (max_x - min_x + 4).max(1) as u32,
        height: (max_y - min_y + 4).max(1) as u32,
    };
    let v = json!({ "label": b.label, "x": b.x, "y": b.y, "width": b.width, "height": b.height });
    layout.bounds.push(b);
    Ok(v)
}

fn remove_bounds(ctx: &mut ToolCtx, args: &Value) -> Result<Value, String> {
    let layout = ctx.dungeon.layout.as_mut().ok_or("The map has no bounds yet.")?;
    let i = match args.get("bounds") {
        Some(Value::Number(n)) => n.as_u64().map(|i| i as usize).filter(|i| *i < layout.bounds.len()),
        Some(Value::String(s)) => layout.bounds.iter().position(|b| b.label.eq_ignore_ascii_case(s.trim())),
        _ => return Err("Missing 'bounds': a label or index".into()),
    }
    .ok_or("No such bounds rectangle. get_overview lists them.")?;
    let b = layout.bounds.remove(i);
    Ok(json!({ "removed": b.label }))
}
