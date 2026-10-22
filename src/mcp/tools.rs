//! The tools Claude gets: what each one is (for `tools/list`) and what it does to the
//! open map. Everything here runs on the UI thread, against the app's live state.

use std::collections::BTreeSet;
use std::fmt::Debug;
use std::path::Path;

use serde_json::{json, Value};

use super::{encounters, McpCall, ToolOutput};
use crate::app::Tab;
use crate::model::{
    Campaign, Connection, ConnectionType, CoverKind, DecorLight, DecorLightSetting, DecorType, Dungeon,
    FloorAssignment, Room, RoomDecor, RoomEnvironment, RoomShape, RoomTag, SizeHint,
};
use crate::notes::{NoteBind, NoteVault};
use crate::ui::decor_view::DecorViewState;
use crate::ui::encounters_view::EncountersViewState;
use crate::ui::graph_editor::GraphEditorState;
use crate::ui::presentation_view::PresentationViewState;
use crate::ui::spatial_view::SpatialViewState;
use crate::ui::styled_view::StyledViewState;

/// The parts of the app the tools work on, borrowed for one call.
pub struct ToolCtx<'a> {
    pub campaign: &'a Campaign,
    pub monsters: &'a crate::data::MonsterDatabase,
    /// The bestiary loads in the background at startup.
    pub bestiary_loading: bool,
    /// What Claude has changed, newest last, for the status bar and the Claude menu.
    pub activity: &'a mut Vec<(std::time::Instant, String)>,
    pub dungeon: &'a mut Dungeon,
    pub notes: &'a mut NoteVault,
    pub current_floor: &'a mut Option<i32>,
    pub active_tab: &'a mut Tab,
    pub presenting: bool,
    pub current_file: Option<&'a Path>,
    pub graph: &'a mut GraphEditorState,
    pub spatial: &'a mut SpatialViewState,
    pub decor: &'a mut DecorViewState,
    pub encounters: &'a mut EncountersViewState,
    pub styled: &'a mut StyledViewState,
    pub presentation: &'a mut PresentationViewState,
    /// Set by tools only the app can carry out; it does so, then answers the call.
    pub app_action: Option<AppAction>,
}

#[derive(Debug, PartialEq)]
pub enum AppAction {
    OpenMap(usize),
    CreateMap(String),
    Save,
}

/// Something for the app to do before replying to the call that asked for it.
pub struct PendingAction {
    pub action: AppAction,
    pub reply: std::sync::mpsc::Sender<ToolOutput>,
}

const TABS: [Tab; 5] = [Tab::Graph, Tab::Spatial, Tab::Decor, Tab::Encounters, Tab::Styled];
const COVER_KINDS: [CoverKind; 4] = [CoverKind::None, CoverKind::Half, CoverKind::ThreeQuarters, CoverKind::Full];
const TAGS: [RoomTag; 7] = [
    RoomTag::Entrance, RoomTag::Boss, RoomTag::Trap, RoomTag::Treasure,
    RoomTag::Optional, RoomTag::Secret, RoomTag::Rest,
];
/// Longest side of a rendered map, in pixels.
const RENDER_MAX_PX: f32 = 1568.0;

// ------------------------------------------------------------------ definitions

fn names<T: Debug>(all: &[T]) -> Vec<String> {
    all.iter().map(|v| format!("{:?}", v)).collect()
}

pub(super) fn tool(name: &str, description: &str, properties: Value, required: &[&str]) -> Value {
    json!({
        "name": name,
        "description": description,
        "inputSchema": { "type": "object", "properties": properties, "required": required },
    })
}

/// The tool list sent to Claude.
pub fn definitions() -> Vec<Value> {
    let room = json!({ "type": "string", "description": "Room id, or its label (case-insensitive)" });
    let floor = json!({
        "description": "Floor number, or [lowest, highest] for a room spanning floors",
        "oneOf": [{ "type": "integer" }, { "type": "array", "items": { "type": "integer" }, "minItems": 2, "maxItems": 2 }],
    });
    let room_fields = json!({
        "size_hint": { "type": "string", "enum": names(&SizeHint::ALL), "description": "Small 3x3, Medium 4x4, Large 6x6, Huge 8x8 squares. Clears width/height unless they're given too." },
        "width": { "type": "integer", "minimum": 1, "description": "Width in grid squares (5 ft each)" },
        "height": { "type": "integer", "minimum": 1, "description": "Height in grid squares (5 ft each)" },
        "shape": { "type": "string", "enum": names(&RoomShape::ALL) },
        "floor": floor,
        "tags": { "type": "array", "items": { "type": "string" }, "description": format!("Replaces the room's tags. Known: {}; anything else is a custom tag.", names(&TAGS).join(", ")) },
        "environment": { "type": "string", "enum": names(&RoomEnvironment::ALL) },
    });
    let with = |base: &Value, extra: Value| {
        let mut m = base.as_object().cloned().unwrap_or_default();
        m.extend(extra.as_object().cloned().unwrap_or_default());
        Value::Object(m)
    };
    let connection_type = json!({ "type": "string", "enum": names(&ConnectionType::ALL), "description": "Defaults to Door" });
    let cover = json!({ "type": "string", "enum": ["default", "None", "Half", "ThreeQuarters", "Full"], "description": "Cover it grants; default = its type's" });
    let light = json!({
        "description": "\"default\" (its type's light), \"off\", or its own light {bright, dim, color}: radii in grid squares, color [r,g,b]",
        "oneOf": [
            { "type": "string", "enum": ["default", "off"] },
            { "type": "object", "properties": {
                "bright": { "type": "number" }, "dim": { "type": "number" },
                "color": { "type": "array", "items": { "type": "integer" } } },
              "required": ["bright", "dim"] },
        ],
    });
    let decor_fields = json!({
        "x": { "type": "number", "description": "Grid units from the room's left edge" },
        "y": { "type": "number", "description": "Grid units from the room's top edge" },
        "rotation": { "type": "number", "description": "Degrees" },
        "scale_x": { "type": "number", "description": "1 = normal size" },
        "scale_y": { "type": "number", "description": "1 = normal size" },
        "cover": cover,
        "light": light,
    });
    let decor_type = json!({ "type": "string", "enum": names(&DecorType::ALL) });
    let note_about = json!({ "type": "string", "enum": ["room", "map", "campaign", "page"], "description": "room: a room's note; map: the open map's note (prep, session log); campaign: the campaign note; page: a free page by title" });
    let note_name = json!({ "type": "string", "description": "The room (id or label) for about=room, the page title for about=page" });

    vec![
        tool("get_overview",
            "The open map: its rooms (with tags, floor, size, a one-line note excerpt), connections, groups and encounters, plus the campaign's maps, the floors in use, the current floor and view.",
            json!({}), &[]),
        tool("get_selection",
            "What the user has selected in the app right now (rooms, a corridor or connection, a decor item), with the current view and floor.",
            json!({}), &[]),
        tool("get_room",
            "Everything about one room: size, shape, floor, tags, position on the map, decor (with ids), elevation sections, connections, encounters, containment, and its full note.",
            json!({ "room": room }), &["room"]),
        tool("render_map",
            "A picture of the map as laid out (the DM's view, secrets shown unless players=true). Defaults to the floor the user is looking at.",
            json!({
                "floor": { "type": "integer", "description": "Floor to draw; omit for the current one" },
                "all_floors": { "type": "boolean", "description": "Draw every floor at once" },
                "players": { "type": "boolean", "description": "Hide secret doors and rooms as players would" },
            }), &[]),
        tool("add_room",
            "Add a room. Give connect_to to join it to an existing room (it's placed next to that room); it takes that room's floor unless floor is given.",
            with(&room_fields, json!({
                "label": { "type": "string" },
                "connect_to": room,
                "connection_type": connection_type,
            })), &["label"]),
        tool("update_room", "Change a room's label, size, shape, floor, tags or environment. Only the fields given change.",
            with(&room_fields, json!({ "room": room, "label": { "type": "string" } })), &["room"]),
        tool("delete_room", "Delete a room and its connections. Its note stays in the vault.",
            json!({ "room": room }), &["room"]),
        tool("connect_rooms", "Connect two rooms with a corridor or door.",
            json!({
                "from": room, "to": room, "type": connection_type,
                "label": { "type": "string" },
                "corridor_width": { "type": "integer", "minimum": 1, "description": "Grid squares; default 2" },
                "double_door": { "type": "boolean" },
            }), &["from", "to"]),
        tool("update_connection", "Change a connection's type, label, corridor width or door. An empty label clears it.",
            json!({
                "connection": { "type": "string", "description": "Connection id" },
                "type": connection_type, "label": { "type": "string" },
                "corridor_width": { "type": "integer", "minimum": 1 },
                "double_door": { "type": "boolean" },
            }), &["connection"]),
        tool("delete_connection", "Remove a connection between two rooms.",
            json!({ "connection": { "type": "string", "description": "Connection id" } }), &["connection"]),
        tool("add_decor", "Place a decor item (furniture, feature, hazard) in a room.",
            with(&decor_fields, json!({ "room": room, "type": decor_type })), &["room", "type", "x", "y"]),
        tool("update_decor", "Move, turn, resize or retype a decor item, or change its cover or light. Only the fields given change.",
            with(&decor_fields, json!({ "room": room, "decor": { "type": "string", "description": "Decor id from get_room" }, "type": decor_type })),
            &["room", "decor"]),
        tool("remove_decor", "Remove a decor item from a room.",
            json!({ "room": room, "decor": { "type": "string", "description": "Decor id from get_room" } }), &["room", "decor"]),
        tool("read_note", "Read a note: a room's, the map's, the campaign's, or a free page.",
            json!({ "about": note_about, "name": note_name }), &["about"]),
        tool("write_note", "Write a note (Markdown). Creates it if needed. mode=append adds to the end instead of replacing.",
            json!({
                "about": note_about, "name": note_name,
                "body": { "type": "string" },
                "mode": { "type": "string", "enum": ["replace", "append"] },
            }), &["about", "body"]),
        tool("search_notes", "Find notes whose title or text contains the query.",
            json!({ "query": { "type": "string" } }), &["query"]),
        tool("focus_room", "Select a room in the app so the user sees it, switching to its floor (and to a given view).",
            json!({ "room": room, "view": { "type": "string", "enum": names(&TABS) } }), &["room"]),
        tool("set_floor", "Show one floor in every view, or all floors with null.",
            json!({ "floor": { "type": ["integer", "null"] } }), &["floor"]),
        tool("create_map",
            "Add a new, empty map to the campaign and open it. Not while the user is presenting.",
            json!({ "name": { "type": "string" } }), &["name"]),
        tool("save", "Save the campaign to its file, as Ctrl+S does. Only works once the user has saved it somewhere.",
            json!({}), &[]),
        tool("switch_map",
            "Open another map of the campaign in the app (every other tool works on the open map). Returns the new map's overview.",
            json!({ "map": {
                "description": "The map's name (case-insensitive) or its index from get_overview",
                "oneOf": [{ "type": "string" }, { "type": "integer" }],
            } }), &["map"]),
    ]
    .into_iter()
    .chain(encounters::definitions())
    .collect()
}

// ------------------------------------------------------------------ dispatch

/// Answer one call. Map renders go to a background thread so the UI doesn't stall; a
/// map change or save is handed back for the app to make and answer.
pub fn handle(ctx: &mut ToolCtx, call: McpCall) -> Option<PendingAction> {
    if call.tool == "render_map" {
        match render_job(ctx, &call.args) {
            Ok(job) => {
                std::thread::spawn(move || {
                    let _ = call.reply.send(job().unwrap_or_else(ToolOutput::error));
                });
            }
            Err(e) => {
                let _ = call.reply.send(ToolOutput::error(e));
            }
        }
        return None;
    }
    let output = match execute(ctx, &call.tool, &call.args) {
        Ok(v) => ToolOutput::json(&v),
        Err(e) => ToolOutput::error(e),
    };
    if let Some(action) = ctx.app_action.take() {
        return Some(PendingAction { action, reply: call.reply });
    }
    let _ = call.reply.send(output);
    None
}

/// The answer to switch_map or create_map, once the app has opened the map.
pub fn map_opened(ctx: &ToolCtx) -> ToolOutput {
    ToolOutput::json(&json!({ "opened": ctx.dungeon.name, "overview": overview(ctx) }))
}

/// Keep a short log of what Claude did.
pub fn log_activity(log: &mut Vec<(std::time::Instant, String)>, what: String) {
    log.push((std::time::Instant::now(), what));
    if log.len() > 50 {
        log.remove(0);
    }
}

/// A few words on what a call changed, for the activity log; `None` for reads.
fn describe(ctx: &ToolCtx, tool: &str, args: &Value, result: &Value) -> Option<String> {
    let s = |v: &Value| v.as_str().unwrap_or("?").to_string();
    let room = || {
        opt_str(args, "room")
            .and_then(|r| resolve_room(ctx.dungeon, r).ok())
            .map_or("?".to_string(), |id| label_of(ctx.dungeon, &id))
    };
    Some(match tool {
        "add_room" => format!("added room {}", s(&result["room"]["label"])),
        "update_room" => format!("changed room {}", s(&result["label"])),
        "delete_room" => format!("deleted room {}", s(&result["deleted"])),
        "connect_rooms" => format!("connected {} to {}", s(&result["from"]), s(&result["to"])),
        "update_connection" => format!("changed the {}–{} connection", s(&result["from"]), s(&result["to"])),
        "delete_connection" => format!("removed the {}–{} connection", s(&result["deleted"]["from"]), s(&result["deleted"]["to"])),
        "add_decor" => format!("placed a {} in {}", s(&result["decor_type"]), room()),
        "update_decor" => format!("changed a {} in {}", s(&result["decor_type"]), room()),
        "remove_decor" => format!("removed a {} from {}", s(&result["removed"]), room()),
        "write_note" => match opt_str(args, "about") {
            Some("room") => format!("wrote notes for {}", room_or_name(ctx, args)),
            Some("page") => format!("wrote page {}", opt_str(args, "name").unwrap_or("?")),
            Some(about) => format!("wrote the {} note", about),
            None => return None,
        },
        "create_encounter" => format!("added encounter {} to {}", s(&result["name"]), s(&result["room"])),
        "update_encounter" => format!("changed encounter {}", s(&result["name"])),
        "delete_encounter" => format!("removed encounter {}", s(&result["deleted"])),
        "focus_room" => format!("selected {}", s(&result["selected"])),
        "set_floor" => match result["current_floor"].as_i64() {
            Some(f) => format!("showed floor {}", f),
            None => "showed all floors".into(),
        },
        _ => return None,
    })
}

fn room_or_name(ctx: &ToolCtx, args: &Value) -> String {
    let name = opt_str(args, "name").unwrap_or("?");
    resolve_room(ctx.dungeon, name).map_or(name.to_string(), |id| label_of(ctx.dungeon, &id))
}

/// Run a tool and log what it changed.
fn execute(ctx: &mut ToolCtx, tool: &str, args: &Value) -> Result<Value, String> {
    let result = run(ctx, tool, args)?;
    if let Some(what) = describe(ctx, tool, args, &result) {
        log_activity(ctx.activity, what);
    }
    Ok(result)
}

fn run(ctx: &mut ToolCtx, tool: &str, args: &Value) -> Result<Value, String> {
    if let Some(result) = encounters::run(ctx, tool, args) {
        return result;
    }
    match tool {
        "get_overview" => Ok(overview(ctx)),
        "get_selection" => Ok(selection(ctx)),
        "get_room" => {
            let id = resolve_room(ctx.dungeon, req_str(args, "room")?)?;
            Ok(room_detail(ctx, &id))
        }
        "add_room" => add_room(ctx, args),
        "update_room" => update_room(ctx, args),
        "delete_room" => delete_room(ctx, args),
        "connect_rooms" => connect_rooms(ctx, args),
        "update_connection" => update_connection(ctx, args),
        "delete_connection" => delete_connection(ctx, args),
        "add_decor" => add_decor(ctx, args),
        "update_decor" => update_decor(ctx, args),
        "remove_decor" => remove_decor(ctx, args),
        "read_note" => read_note(ctx, args),
        "write_note" => write_note(ctx, args),
        "search_notes" => search_notes(ctx, args),
        "focus_room" => focus_room(ctx, args),
        "set_floor" => set_floor(ctx, args),
        "switch_map" => switch_map(ctx, args),
        "create_map" => create_map(ctx, args),
        "save" => save(ctx),
        _ => Err(format!("Unknown tool: {}", tool)),
    }
}

// ------------------------------------------------------------------ arguments

pub(super) fn req_str<'v>(args: &'v Value, key: &str) -> Result<&'v str, String> {
    args.get(key).and_then(Value::as_str).ok_or_else(|| format!("Missing '{}'", key))
}

pub(super) fn opt_str<'v>(args: &'v Value, key: &str) -> Option<&'v str> {
    args.get(key).and_then(Value::as_str)
}

pub(super) fn opt_f32(args: &Value, key: &str) -> Result<Option<f32>, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v.as_f64().map(|n| Some(n as f32)).ok_or_else(|| format!("'{}' must be a number", key)),
    }
}

fn opt_u32(args: &Value, key: &str) -> Result<Option<u32>, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v
            .as_u64()
            .filter(|n| *n >= 1 && *n <= 200)
            .map(|n| Some(n as u32))
            .ok_or_else(|| format!("'{}' must be a whole number from 1 to 200", key)),
    }
}

pub(super) fn opt_bool(args: &Value, key: &str) -> Result<Option<bool>, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v.as_bool().map(Some).ok_or_else(|| format!("'{}' must be true or false", key)),
    }
}

/// Match an enum by name, ignoring case, spaces, dashes and underscores.
fn parse_enum<T: Copy + Debug>(all: &[T], s: &str, what: &str) -> Result<T, String> {
    let squash = |s: &str| s.chars().filter(|c| c.is_alphanumeric()).collect::<String>().to_lowercase();
    let want = squash(s);
    all.iter()
        .copied()
        .find(|v| squash(&format!("{:?}", v)) == want)
        .ok_or_else(|| format!("Unknown {} '{}'. One of: {}", what, s, names(all).join(", ")))
}

fn parse_floor(v: &Value) -> Result<FloorAssignment, String> {
    if let Some(f) = v.as_i64() {
        return Ok(FloorAssignment::Single(f as i32));
    }
    match v.as_array().map(|a| a.iter().map(Value::as_i64).collect::<Vec<_>>()).as_deref() {
        Some([Some(a), Some(b)]) if a == b => Ok(FloorAssignment::Single(*a as i32)),
        Some([Some(a), Some(b)]) => Ok(FloorAssignment::Half(*a as i32, *b as i32)),
        _ => Err("'floor' must be a floor number or [lowest, highest]".into()),
    }
}

fn parse_tag(s: &str) -> RoomTag {
    TAGS.iter()
        .find(|t| t.label().eq_ignore_ascii_case(s.trim()))
        .cloned()
        .unwrap_or_else(|| RoomTag::Custom(s.trim().to_string()))
}

fn parse_cover(s: &str) -> Result<Option<CoverKind>, String> {
    if s.eq_ignore_ascii_case("default") {
        return Ok(None);
    }
    parse_enum(&COVER_KINDS, s, "cover").map(Some)
}

fn parse_light(v: &Value) -> Result<DecorLightSetting, String> {
    match v {
        Value::String(s) if s.eq_ignore_ascii_case("default") => Ok(DecorLightSetting::Default),
        Value::String(s) if s.eq_ignore_ascii_case("off") => Ok(DecorLightSetting::Off),
        Value::Object(_) => {
            let bright = opt_f32(v, "bright")?.ok_or("light needs 'bright'")?.max(0.0);
            let dim = opt_f32(v, "dim")?.ok_or("light needs 'dim'")?.max(bright);
            let color = match v.get("color").and_then(Value::as_array) {
                Some(c) if c.len() == 3 => {
                    let ch = |i: usize| c[i].as_u64().map(|n| n.min(255) as u8).ok_or("color must be [r,g,b]");
                    [ch(0)?, ch(1)?, ch(2)?]
                }
                Some(_) => return Err("color must be [r,g,b]".into()),
                None => DecorLight::TORCH.color,
            };
            Ok(DecorLightSetting::Custom(DecorLight { bright, dim, color }))
        }
        _ => Err("light must be \"default\", \"off\" or {bright, dim, color}".into()),
    }
}

/// A room by id, or by its label when exactly one room has it.
pub(super) fn resolve_room(d: &Dungeon, r: &str) -> Result<String, String> {
    if let Some(room) = d.graph.room_by_id(r) {
        return Ok(room.id.clone());
    }
    let matches: Vec<&Room> = d.graph.rooms.iter().filter(|x| x.label.eq_ignore_ascii_case(r.trim())).collect();
    match matches.as_slice() {
        [one] => Ok(one.id.clone()),
        [] => Err(format!("No room '{}' on this map. get_overview lists the rooms.", r)),
        many => Err(format!(
            "{} rooms are labelled '{}'; use an id: {}",
            many.len(),
            r,
            many.iter().map(|x| x.id.as_str()).collect::<Vec<_>>().join(", ")
        )),
    }
}

// ------------------------------------------------------------------ reading

fn floor_json(f: &FloorAssignment) -> Value {
    match *f {
        FloorAssignment::Single(n) => json!(n),
        FloorAssignment::Half(..) => {
            let (lo, hi) = f.range();
            json!([lo, hi])
        }
    }
}

pub(super) fn label_of(d: &Dungeon, id: &str) -> String {
    d.graph.room_by_id(id).map(|r| r.label.clone()).unwrap_or_else(|| id.to_string())
}

fn room_summary(d: &Dungeon, room: &Room) -> Value {
    let (w, h) = room.grid_size();
    let mut v = json!({
        "id": room.id,
        "label": room.label,
        "floor": floor_json(&room.floor),
        "size": [w, h],
        "shape": format!("{:?}", room.shape),
        "placed": d.layout.as_ref().is_some_and(|l| l.room_by_id(&room.id).is_some()),
    });
    if !room.tags.is_empty() {
        v["tags"] = json!(room.tags.iter().map(|t| t.label()).collect::<Vec<_>>());
    }
    if !room.note_excerpt.is_empty() {
        v["note_excerpt"] = json!(room.note_excerpt);
    }
    v
}

fn connection_summary(d: &Dungeon, edge: &crate::model::StoredEdge) -> Value {
    let c = &edge.connection;
    let mut v = json!({
        "id": c.id,
        "from": label_of(d, &edge.source_room_id),
        "to": label_of(d, &edge.target_room_id),
        "type": format!("{:?}", c.connection_type),
        "corridor_width": c.corridor_width,
    });
    if let Some(label) = &c.label {
        v["label"] = json!(label);
    }
    if c.double_door {
        v["double_door"] = json!(true);
    }
    v
}

fn floors_in_use(d: &Dungeon) -> Vec<i32> {
    let set: BTreeSet<i32> = d.graph.rooms.iter().flat_map(|r| r.floor.floors()).collect();
    set.into_iter().collect()
}

fn view_name(ctx: &ToolCtx) -> String {
    if ctx.presenting { "Presenting".into() } else { format!("{:?}", ctx.active_tab) }
}

fn overview(ctx: &ToolCtx) -> Value {
    let d = &*ctx.dungeon;
    let active = ctx.campaign.active_map;
    json!({
        "campaign": ctx.campaign.name,
        "file": ctx.current_file.map(|p| p.display().to_string()),
        "maps": ctx.campaign.maps.iter().enumerate().map(|(i, m)| json!({
            "index": i,
            "name": if i == active { &d.name } else { &m.name },
            "open": i == active,
        })).collect::<Vec<_>>(),
        "map": d.name,
        "view": view_name(ctx),
        "floors": floors_in_use(d),
        "current_floor": *ctx.current_floor,
        "laid_out": d.layout.is_some(),
        "rooms": d.graph.rooms.iter().map(|r| room_summary(d, r)).collect::<Vec<_>>(),
        "connections": d.graph.connections.iter().map(|e| connection_summary(d, e)).collect::<Vec<_>>(),
        "groups": d.graph.groups.iter().map(|g| json!({
            "label": g.label,
            "rooms": g.room_ids.iter().map(|id| label_of(d, id)).collect::<Vec<_>>(),
            "container": g.parent_room_id.as_deref().map(|id| label_of(d, id)),
        })).collect::<Vec<_>>(),
        "encounters": d.encounters.iter().map(|e| json!({
            "id": e.id,
            "name": e.name,
            "room": label_of(d, &e.home_room_id),
            "monsters": encounters::monster_line(ctx, e),
            "total_xp": encounters::total_xp(ctx, e),
        })).collect::<Vec<_>>(),
        "party": d.party.iter().map(|p| json!({
            "name": p.name, "class": p.class, "ac": p.ac, "max_hp": p.max_hp,
            "passive_perception": p.passive_perception,
        })).collect::<Vec<_>>(),
    })
}

fn selection(ctx: &ToolCtx) -> Value {
    let d = &*ctx.dungeon;
    let mut rooms: Vec<String> = Vec::new();
    let mut connections: Vec<String> = Vec::new();
    let mut decor = Value::Null;
    if ctx.presenting {
        rooms.extend(ctx.presentation.selected_room.clone());
    } else {
        match ctx.active_tab {
            Tab::Graph => {
                rooms.extend(ctx.graph.selection.rooms.iter().cloned());
                connections.extend(ctx.graph.selection.connections.iter().cloned());
            }
            Tab::Spatial => {
                rooms.extend(ctx.spatial.selected_room.clone());
                let corridor = ctx.spatial.selected_corridor
                    .and_then(|i| d.layout.as_ref()?.corridors.get(i))
                    .map(|c| c.connection_id.clone());
                connections.extend(corridor);
            }
            Tab::Decor => {
                rooms.extend(ctx.decor.selected_room.clone());
                if let (Some(room), Some(i)) = (&ctx.decor.selected_room, ctx.decor.selected_decor) {
                    if let Some(item) = d.graph.room_by_id(room).and_then(|r| r.decor.get(i)) {
                        decor = serde_json::to_value(item).unwrap_or_default();
                    }
                }
            }
            Tab::Encounters => rooms.extend(ctx.encounters.selected_room.clone()),
            Tab::Styled => rooms.extend(ctx.styled.selected_room.clone()),
        }
    }
    json!({
        "view": view_name(ctx),
        "current_floor": *ctx.current_floor,
        "rooms": rooms.iter().filter_map(|id| d.graph.room_by_id(id)).map(|r| room_summary(d, r)).collect::<Vec<_>>(),
        "connections": connections.iter().filter_map(|id| d.graph.connection_by_id(id)).map(|e| connection_summary(d, e)).collect::<Vec<_>>(),
        "decor": decor,
    })
}

fn note_for(ctx: &ToolCtx, bind: &NoteBind) -> Option<Value> {
    let note = ctx.notes.get(ctx.notes.note_id_for(bind)?)?;
    Some(json!({ "id": note.id, "title": note.title, "body": note.body }))
}

fn room_detail(ctx: &ToolCtx, id: &str) -> Value {
    let d = &*ctx.dungeon;
    let Some(room) = d.graph.room_by_id(id) else { return Value::Null };
    let mut v = room_summary(d, room);
    v["size_hint"] = json!(format!("{:?}", room.size_hint));
    v["environment"] = json!(format!("{:?}", room.environment));
    v["open_walls"] = serde_json::to_value(room.open_walls).unwrap_or_default();
    v["decor"] = serde_json::to_value(&room.decor).unwrap_or_default();
    if !room.sections.is_empty() {
        v["elevation_sections"] = serde_json::to_value(&room.sections).unwrap_or_default();
    }
    if let Some(l) = d.layout.as_ref().and_then(|l| l.room_by_id(id)) {
        v["position"] = json!({ "x": l.x, "y": l.y, "width": l.width, "height": l.height, "rotation": l.rotation });
    }
    v["connections"] = json!(d.graph.connections.iter()
        .filter(|e| e.source_room_id == id || e.target_room_id == id)
        .map(|e| connection_summary(d, e))
        .collect::<Vec<_>>());
    v["encounters"] = json!(d.encounters.iter()
        .filter(|e| e.home_room_id == id)
        .map(|e| encounters::encounter_summary(ctx, e))
        .collect::<Vec<_>>());
    if let Some(parent) = d.graph.parent_of(id) {
        v["inside"] = json!(label_of(d, parent));
    }
    let children = d.graph.children_of(id);
    if !children.is_empty() {
        v["contains"] = json!(children.iter().map(|c| label_of(d, c)).collect::<Vec<_>>());
    }
    v["note"] = note_for(ctx, &NoteBind::Room(id.to_string())).map(|n| n["body"].clone()).unwrap_or(json!(""));
    v
}

// ------------------------------------------------------------------ rooms

/// Set the room fields shared by add_room and update_room.
fn apply_room_fields(room: &mut Room, args: &Value) -> Result<(), String> {
    if let Some(s) = opt_str(args, "size_hint") {
        room.size_hint = parse_enum(&SizeHint::ALL, s, "size")?;
        room.grid_width = None;
        room.grid_height = None;
    }
    if let Some(w) = opt_u32(args, "width")? {
        room.grid_width = Some(w);
    }
    if let Some(h) = opt_u32(args, "height")? {
        room.grid_height = Some(h);
    }
    if let Some(s) = opt_str(args, "shape") {
        room.shape = parse_enum(&RoomShape::ALL, s, "shape")?;
    }
    if let Some(f) = args.get("floor").filter(|v| !v.is_null()) {
        room.floor = parse_floor(f)?;
    }
    if let Some(tags) = args.get("tags").and_then(Value::as_array) {
        room.tags = tags.iter().filter_map(Value::as_str).filter(|s| !s.trim().is_empty()).map(parse_tag).collect();
    }
    if let Some(s) = opt_str(args, "environment") {
        room.environment = parse_enum(&RoomEnvironment::ALL, s, "environment")?;
    }
    Ok(())
}

/// A free spot on the Graph view near `anchor`'s node (or near the rest of the graph),
/// which also steers where the layout puts the new room.
fn graph_spot(ctx: &ToolCtx, anchor: Option<&str>) -> (f32, f32) {
    let mut taken: Vec<(f32, f32)> = ctx.dungeon.graph.graph_positions.values().copied().collect();
    taken.extend(ctx.graph.room_positions.values().map(|p| (p.x, p.y)));
    let pos_of = |id: &str| {
        ctx.graph.room_positions.get(id).map(|p| (p.x, p.y))
            .or_else(|| ctx.dungeon.graph.graph_positions.get(id).copied())
    };
    let centre = anchor.and_then(pos_of).unwrap_or_else(|| {
        if taken.is_empty() {
            (200.0, 200.0)
        } else {
            let n = taken.len() as f32;
            (taken.iter().map(|p| p.0).sum::<f32>() / n, taken.iter().map(|p| p.1).sum::<f32>() / n)
        }
    });
    for ring in 1..=6 {
        let radius = 180.0 * ring as f32;
        for step in 0..8 {
            let angle = step as f32 * std::f32::consts::FRAC_PI_4;
            let p = (centre.0 + radius * angle.cos(), centre.1 + radius * angle.sin());
            if taken.iter().all(|t| (t.0 - p.0).hypot(t.1 - p.1) > 140.0) {
                return p;
            }
        }
    }
    (centre.0 + 180.0, centre.1)
}

fn add_room(ctx: &mut ToolCtx, args: &Value) -> Result<Value, String> {
    let label = req_str(args, "label")?.trim();
    if label.is_empty() {
        return Err("A room needs a label".into());
    }
    let neighbour = opt_str(args, "connect_to").map(|r| resolve_room(ctx.dungeon, r)).transpose()?;
    let connection_type = opt_str(args, "connection_type")
        .map(|s| parse_enum(&ConnectionType::ALL, s, "connection type"))
        .transpose()?
        .unwrap_or(ConnectionType::Door);

    let mut room = Room::new(label.to_string());
    room.floor = match (&neighbour, *ctx.current_floor) {
        (Some(n), _) => FloorAssignment::Single(ctx.dungeon.graph.room_by_id(n).map_or(0, |r| r.floor.range().0)),
        (None, Some(f)) => FloorAssignment::Single(f),
        (None, None) => FloorAssignment::default(),
    };
    apply_room_fields(&mut room, args)?;

    let spot = graph_spot(ctx, neighbour.as_deref());
    let id = room.id.clone();
    ctx.dungeon.graph.add_room(room);
    ctx.dungeon.graph.graph_positions.insert(id.clone(), spot);
    let mut connection = None;
    if let Some(n) = neighbour {
        let c = Connection::new(connection_type);
        connection = Some(c.id.clone());
        ctx.dungeon.graph.add_connection(n, id.clone(), c);
    }
    Ok(json!({ "room": room_summary(ctx.dungeon, ctx.dungeon.graph.room_by_id(&id).unwrap()), "connection": connection }))
}

fn update_room(ctx: &mut ToolCtx, args: &Value) -> Result<Value, String> {
    let id = resolve_room(ctx.dungeon, req_str(args, "room")?)?;
    // Check everything parses before touching the room.
    let mut room = ctx.dungeon.graph.room_by_id(&id).cloned().ok_or("Room vanished")?;
    apply_room_fields(&mut room, args)?;
    if let Some(label) = opt_str(args, "label").map(str::trim).filter(|l| !l.is_empty()) {
        room.label = label.to_string();
    }
    if let Some(r) = ctx.dungeon.graph.room_by_id_mut(&id) {
        *r = room;
    }
    Ok(room_summary(ctx.dungeon, ctx.dungeon.graph.room_by_id(&id).unwrap()))
}

fn delete_room(ctx: &mut ToolCtx, args: &Value) -> Result<Value, String> {
    let id = resolve_room(ctx.dungeon, req_str(args, "room")?)?;
    let label = label_of(ctx.dungeon, &id);
    ctx.dungeon.graph.remove_room(&id);
    ctx.graph.room_positions.remove(&id);
    forget_missing_selections(ctx);
    let orphans: Vec<&str> = ctx.dungeon.encounters.iter()
        .filter(|e| e.home_room_id == id)
        .map(|e| e.name.as_str())
        .collect();
    Ok(json!({ "deleted": label, "encounters_left_without_a_room": orphans }))
}

/// Drop selections pointing at rooms or connections that no longer exist.
fn forget_missing_selections(ctx: &mut ToolCtx) {
    let graph = &ctx.dungeon.graph;
    let gone = |id: &Option<String>| id.as_deref().is_some_and(|id| graph.room_by_id(id).is_none());
    if gone(&ctx.spatial.selected_room) {
        ctx.spatial.selected_room = None;
    }
    if gone(&ctx.decor.selected_room) {
        ctx.decor.selected_room = None;
        ctx.decor.selected_decor = None;
        ctx.decor.selected_decor_set.clear();
    }
    if gone(&ctx.encounters.selected_room) {
        ctx.encounters.selected_room = None;
    }
    if gone(&ctx.styled.selected_room) {
        ctx.styled.selected_room = None;
    }
    if gone(&ctx.presentation.selected_room) {
        ctx.presentation.selected_room = None;
    }
    ctx.graph.selection.rooms.retain(|id| graph.room_by_id(id).is_some());
    ctx.graph.selection.connections.retain(|id| graph.connection_by_id(id).is_some());
    // Corridor indices shift as connections come and go.
    ctx.spatial.selected_corridor = None;
}

// ------------------------------------------------------------------ connections

fn connect_rooms(ctx: &mut ToolCtx, args: &Value) -> Result<Value, String> {
    let from = resolve_room(ctx.dungeon, req_str(args, "from")?)?;
    let to = resolve_room(ctx.dungeon, req_str(args, "to")?)?;
    if from == to {
        return Err("A room can't connect to itself".into());
    }
    let kind = opt_str(args, "type")
        .map(|s| parse_enum(&ConnectionType::ALL, s, "connection type"))
        .transpose()?
        .unwrap_or(ConnectionType::Door);
    let mut c = Connection::new(kind);
    c.label = opt_str(args, "label").map(str::trim).filter(|l| !l.is_empty()).map(String::from);
    if let Some(w) = opt_u32(args, "corridor_width")? {
        c.corridor_width = w;
    }
    if let Some(b) = opt_bool(args, "double_door")? {
        c.double_door = b;
    }
    let id = c.id.clone();
    ctx.dungeon.graph.add_connection(from, to, c);
    Ok(connection_summary(ctx.dungeon, ctx.dungeon.graph.connection_by_id(&id).unwrap()))
}

fn update_connection(ctx: &mut ToolCtx, args: &Value) -> Result<Value, String> {
    let id = req_str(args, "connection")?;
    let kind = opt_str(args, "type").map(|s| parse_enum(&ConnectionType::ALL, s, "connection type")).transpose()?;
    let width = opt_u32(args, "corridor_width")?;
    let double = opt_bool(args, "double_door")?;
    let edge = ctx.dungeon.graph.connection_by_id_mut(id)
        .ok_or_else(|| format!("No connection '{}'. get_overview lists them.", id))?;
    let c = &mut edge.connection;
    if let Some(k) = kind {
        c.connection_type = k;
    }
    if let Some(label) = opt_str(args, "label") {
        c.label = Some(label.trim().to_string()).filter(|l| !l.is_empty());
    }
    if let Some(w) = width {
        c.corridor_width = w;
    }
    if let Some(b) = double {
        c.double_door = b;
    }
    Ok(connection_summary(ctx.dungeon, ctx.dungeon.graph.connection_by_id(id).unwrap()))
}

fn delete_connection(ctx: &mut ToolCtx, args: &Value) -> Result<Value, String> {
    let id = req_str(args, "connection")?;
    let summary = ctx.dungeon.graph.connection_by_id(id)
        .map(|e| connection_summary(ctx.dungeon, e))
        .ok_or_else(|| format!("No connection '{}'. get_overview lists them.", id))?;
    ctx.dungeon.graph.remove_connection(id);
    forget_missing_selections(ctx);
    Ok(json!({ "deleted": summary }))
}

// ------------------------------------------------------------------ decor

/// Apply decor fields from the arguments onto `item`, checking the position against
/// the room's size.
fn apply_decor_fields(item: &mut RoomDecor, args: &Value, room_size: (u32, u32)) -> Result<(), String> {
    if let Some(s) = opt_str(args, "type") {
        item.decor_type = parse_enum(&DecorType::ALL, s, "decor type")?;
    }
    if let Some(x) = opt_f32(args, "x")? {
        item.x = x;
    }
    if let Some(y) = opt_f32(args, "y")? {
        item.y = y;
    }
    let (w, h) = (room_size.0 as f32, room_size.1 as f32);
    if !(0.0..=w).contains(&item.x) || !(0.0..=h).contains(&item.y) {
        return Err(format!("({}, {}) is outside the room, which is {}x{} squares", item.x, item.y, w, h));
    }
    if let Some(r) = opt_f32(args, "rotation")? {
        item.rotation = r.rem_euclid(360.0);
    }
    if let Some(s) = opt_f32(args, "scale_x")? {
        item.scale_x = s.clamp(0.1, 20.0);
    }
    if let Some(s) = opt_f32(args, "scale_y")? {
        item.scale_y = s.clamp(0.1, 20.0);
    }
    if let Some(c) = opt_str(args, "cover") {
        item.cover = parse_cover(c)?;
    }
    if let Some(l) = args.get("light").filter(|v| !v.is_null()) {
        item.light = parse_light(l)?;
    }
    Ok(())
}

fn add_decor(ctx: &mut ToolCtx, args: &Value) -> Result<Value, String> {
    let id = resolve_room(ctx.dungeon, req_str(args, "room")?)?;
    req_str(args, "type")?;
    let room = ctx.dungeon.graph.room_by_id_mut(&id).ok_or("Room vanished")?;
    let mut item = RoomDecor::new(DecorType::Table, 0.0, 0.0);
    apply_decor_fields(&mut item, args, room.grid_size())?;
    let v = serde_json::to_value(&item).unwrap_or_default();
    room.decor.push(item);
    Ok(v)
}

fn update_decor(ctx: &mut ToolCtx, args: &Value) -> Result<Value, String> {
    let id = resolve_room(ctx.dungeon, req_str(args, "room")?)?;
    let decor_id = req_str(args, "decor")?;
    let room = ctx.dungeon.graph.room_by_id_mut(&id).ok_or("Room vanished")?;
    let size = room.grid_size();
    let item = room.decor.iter_mut().find(|d| d.id == decor_id)
        .ok_or_else(|| format!("No decor '{}' in that room. get_room lists its decor.", decor_id))?;
    let mut changed = item.clone();
    apply_decor_fields(&mut changed, args, size)?;
    *item = changed;
    Ok(serde_json::to_value(&*item).unwrap_or_default())
}

fn remove_decor(ctx: &mut ToolCtx, args: &Value) -> Result<Value, String> {
    let id = resolve_room(ctx.dungeon, req_str(args, "room")?)?;
    let decor_id = req_str(args, "decor")?;
    let room = ctx.dungeon.graph.room_by_id_mut(&id).ok_or("Room vanished")?;
    let i = room.decor.iter().position(|d| d.id == decor_id)
        .ok_or_else(|| format!("No decor '{}' in that room. get_room lists its decor.", decor_id))?;
    let item = room.decor.remove(i);
    // Selection is by index, which has just shifted.
    if ctx.decor.selected_room.as_deref() == Some(id.as_str()) {
        ctx.decor.selected_decor = None;
        ctx.decor.selected_decor_set.clear();
    }
    Ok(json!({ "removed": item.decor_type.label() }))
}

// ------------------------------------------------------------------ notes

/// Which note the arguments mean: its binding (None for a free page), title and folder.
fn note_target(ctx: &ToolCtx, args: &Value) -> Result<(Option<NoteBind>, String, Option<String>), String> {
    let d = &*ctx.dungeon;
    match req_str(args, "about")? {
        "room" => {
            let id = resolve_room(d, req_str(args, "name")?)?;
            Ok((Some(NoteBind::Room(id.clone())), label_of(d, &id), Some(d.name.clone())))
        }
        "map" => Ok((Some(NoteBind::Map(d.id.clone())), d.name.clone(), Some(d.name.clone()))),
        "campaign" => Ok((Some(NoteBind::Campaign), ctx.campaign.name.clone(), None)),
        "page" => Ok((None, req_str(args, "name")?.trim().to_string(), None)),
        other => Err(format!("Unknown note kind '{}': room, map, campaign or page", other)),
    }
}

fn read_note(ctx: &mut ToolCtx, args: &Value) -> Result<Value, String> {
    let (bind, title, _) = note_target(ctx, args)?;
    let id = match &bind {
        Some(b) => ctx.notes.note_id_for(b),
        None => ctx.notes.resolve_wiki(&title),
    };
    Ok(match id.and_then(|id| ctx.notes.get(id)) {
        Some(n) => json!({ "id": n.id, "title": n.title, "body": n.body }),
        None => json!({ "title": title, "body": "", "exists": false }),
    })
}

fn write_note(ctx: &mut ToolCtx, args: &Value) -> Result<Value, String> {
    let (bind, title, folder) = note_target(ctx, args)?;
    let body = req_str(args, "body")?;
    let append = match opt_str(args, "mode").unwrap_or("replace") {
        "replace" => false,
        "append" => true,
        other => return Err(format!("Unknown mode '{}': replace or append", other)),
    };
    let id = match &bind {
        Some(b) => ctx.notes.ensure(b.clone(), &title, folder.as_deref()),
        None => match ctx.notes.resolve_wiki(&title) {
            Some(id) => id.to_string(),
            None => ctx.notes.create_page(&title),
        },
    };
    let new_body = match ctx.notes.get(&id) {
        Some(n) if append && !n.body.trim().is_empty() => format!("{}\n\n{}", n.body.trim_end(), body),
        _ => body.to_string(),
    };
    ctx.notes.set_body(&id, new_body);
    // Rooms carry a one-line preview of their note for the map renderers.
    if let Some(NoteBind::Room(room_id)) = &bind {
        let excerpt = ctx.notes.get(&id).map(|n| crate::notes::markdown::summary_line(&n.body)).unwrap_or_default();
        if let Some(room) = ctx.dungeon.graph.room_by_id_mut(room_id) {
            room.note_excerpt = excerpt;
        }
    }
    let saved = ctx.notes.root.is_some();
    Ok(json!({
        "id": id,
        "written": true,
        "saved_to_disk": if saved { "shortly" } else { "once the campaign is saved (it has no file yet)" },
    }))
}

fn search_notes(ctx: &mut ToolCtx, args: &Value) -> Result<Value, String> {
    let query = req_str(args, "query")?;
    let hits: Vec<Value> = ctx.notes.search(query).into_iter().take(30).map(|(id, snippet)| {
        let title = ctx.notes.get(id).map(|n| n.title.clone()).unwrap_or_default();
        json!({ "id": id, "title": title, "snippet": snippet })
    }).collect();
    Ok(json!({ "results": hits }))
}

// ------------------------------------------------------------------ the view

fn focus_room(ctx: &mut ToolCtx, args: &Value) -> Result<Value, String> {
    let id = resolve_room(ctx.dungeon, req_str(args, "room")?)?;
    if let Some(v) = opt_str(args, "view") {
        let tab = parse_enum(&TABS, v, "view")?;
        if !ctx.presenting {
            *ctx.active_tab = tab;
        }
    }
    let room_floor = ctx.dungeon.graph.room_by_id(&id).map(|r| r.floor).unwrap_or_default();
    if let Some(f) = *ctx.current_floor {
        if !room_floor.visible_on(f) {
            *ctx.current_floor = Some(room_floor.range().0);
        }
    }
    let some = Some(id.clone());
    if ctx.presenting {
        ctx.presentation.selected_room = some;
    } else {
        match ctx.active_tab {
            Tab::Graph => {
                ctx.graph.selection = Default::default();
                ctx.graph.selection.rooms.insert(id.clone());
            }
            Tab::Spatial => {
                ctx.spatial.selected_room = some;
                ctx.spatial.selected_corridor = None;
            }
            Tab::Decor => {
                ctx.decor.selected_room = some;
                ctx.decor.selected_decor = None;
                ctx.decor.selected_decor_set.clear();
            }
            Tab::Encounters => ctx.encounters.selected_room = some,
            Tab::Styled => ctx.styled.selected_room = some,
        }
    }
    Ok(json!({ "selected": label_of(ctx.dungeon, &id), "view": view_name(ctx), "current_floor": *ctx.current_floor }))
}

fn set_floor(ctx: &mut ToolCtx, args: &Value) -> Result<Value, String> {
    *ctx.current_floor = match args.get("floor") {
        None | Some(Value::Null) => None,
        Some(v) => Some(v.as_i64().ok_or("'floor' must be a whole number or null")? as i32),
    };
    Ok(json!({ "current_floor": *ctx.current_floor }))
}

fn switch_map(ctx: &mut ToolCtx, args: &Value) -> Result<Value, String> {
    let active = ctx.campaign.active_map;
    // The open map's name lives on the working copy, which may have been renamed.
    let name_of = |i: usize| if i == active { ctx.dungeon.name.as_str() } else { ctx.campaign.maps[i].name.as_str() };
    let count = ctx.campaign.maps.len();
    let index = match args.get("map") {
        Some(Value::Number(n)) => n.as_u64().map(|i| i as usize).filter(|i| *i < count)
            .ok_or_else(|| format!("No map {}; there are {}.", n, count))?,
        Some(Value::String(s)) => {
            let matches: Vec<usize> = (0..count).filter(|i| name_of(*i).eq_ignore_ascii_case(s.trim())).collect();
            match matches.as_slice() {
                [i] => *i,
                [] => return Err(format!(
                    "No map '{}'. Maps: {}",
                    s,
                    (0..count).map(|i| format!("{} ({})", name_of(i), i)).collect::<Vec<_>>().join(", ")
                )),
                _ => return Err(format!("Several maps are called '{}'; use its index from get_overview.", s)),
            }
        }
        _ => return Err("Missing 'map': a map name or index".into()),
    };
    if index == active {
        return Ok(json!({ "already_open": name_of(index) }));
    }
    ctx.app_action = Some(AppAction::OpenMap(index));
    Ok(Value::Null)
}

fn create_map(ctx: &mut ToolCtx, args: &Value) -> Result<Value, String> {
    let name = req_str(args, "name")?.trim();
    if name.is_empty() {
        return Err("A map needs a name".into());
    }
    if ctx.presenting {
        return Err("The user is presenting; a new map would end the presentation. Ask them to stop presenting first.".into());
    }
    ctx.app_action = Some(AppAction::CreateMap(name.to_string()));
    Ok(Value::Null)
}

fn save(ctx: &mut ToolCtx) -> Result<Value, String> {
    if ctx.current_file.is_none() {
        return Err("The campaign has never been saved, so it has no file yet. Ask the user to save it (File ▸ Save As); after that this tool works.".into());
    }
    ctx.app_action = Some(AppAction::Save);
    Ok(Value::Null)
}

// ------------------------------------------------------------------ rendering

type RenderJob = Box<dyn FnOnce() -> Result<ToolOutput, String> + Send>;

/// Snapshot what the render needs; the returned job draws it off the UI thread.
fn render_job(ctx: &ToolCtx, args: &Value) -> Result<RenderJob, String> {
    let d = &*ctx.dungeon;
    let layout = d.layout.as_ref()
        .ok_or("The map hasn't been laid out yet: open the Spatial view in the app first.")?;
    let floor = if opt_bool(args, "all_floors")?.unwrap_or(false) {
        None
    } else {
        match args.get("floor") {
            Some(v) if !v.is_null() => Some(v.as_i64().ok_or("'floor' must be a whole number")? as i32),
            _ => *ctx.current_floor,
        }
    };
    let layout = match floor {
        Some(f) => layout.filtered_to_floor(&d.graph, f),
        None => layout.clone(),
    };
    if layout.rooms.is_empty() {
        return Err(match floor {
            Some(f) => format!("Nothing is laid out on floor {}.", f),
            None => "Nothing is laid out yet.".into(),
        });
    }
    let players = opt_bool(args, "players")?.unwrap_or(false);
    let (graph, theme) = (d.graph.clone(), d.theme.clone());
    Ok(Box::new(move || {
        use crate::util::GRID_PX;
        let (min_x, min_y, max_x, max_y) = layout.extents();
        let margin = 1;
        let (gw, gh) = ((max_x - min_x + 2 * margin) as f32, (max_y - min_y + 2 * margin) as f32);
        let px = (RENDER_MAX_PX / gw.max(gh)).clamp(4.0, 40.0).floor();
        let mut renderer = crate::render::ImageRenderer::new((gw * px) as u32, (gh * px) as u32, px / GRID_PX);
        renderer.offset_x = (min_x - margin) as f32 * GRID_PX;
        renderer.offset_y = (min_y - margin) as f32 * GRID_PX;
        let options = crate::render::themed::RenderOptions {
            show_grid: true,
            show_labels: true,
            show_notes: false,
            show_secrets: !players,
            show_decor: true,
            show_lighting: true,
        };
        crate::render::themed::render_themed(&mut renderer, &graph, &layout, &theme, &options);
        let mut png = std::io::Cursor::new(Vec::new());
        renderer.image.write_to(&mut png, image::ImageFormat::Png).map_err(|e| e.to_string())?;
        let (ox, oy) = ((min_x - margin) as f32, (min_y - margin) as f32);
        let mut caption = format!(
            "{}. Each grid square (5 ft) is {} px; the top-left pixel is grid ({}, {}).\n\
             The image has no text, so here is where each room is (bounding box):",
            floor.map_or("All floors".to_string(), |f| format!("Floor {}", f)),
            px,
            ox,
            oy,
        );
        for rl in &layout.rooms {
            let (x0, y0, x1, y1) = rl.aabb();
            let label = graph.room_by_id(&rl.room_id).map_or("?", |r| r.label.as_str());
            caption.push_str(&format!(
                "\n- {}: grid ({}, {}) to ({}, {}); pixels ({}, {}) to ({}, {})",
                label, x0, y0, x1, y1,
                ((x0 - ox) * px) as i32, ((y0 - oy) * px) as i32,
                ((x1 - ox) * px) as i32, ((y1 - oy) * px) as i32,
            ));
        }
        if !players && graph.connections.iter().any(|e| e.connection.connection_type == ConnectionType::Secret) {
            caption.push_str("\nSecret doors aren't marked in the image; get_overview lists them.");
        }
        Ok(ToolOutput::png(png.get_ref(), caption))
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture {
        campaign: Campaign,
        dungeon: Dungeon,
        notes: NoteVault,
        floor: Option<i32>,
        tab: Tab,
        graph: GraphEditorState,
        spatial: SpatialViewState,
        decor: DecorViewState,
        encounters: EncountersViewState,
        styled: StyledViewState,
        presentation: PresentationViewState,
        /// What the last call asked the app to do.
        app_action: Option<AppAction>,
        monsters: crate::data::MonsterDatabase,
        activity: Vec<(std::time::Instant, String)>,
    }

    impl Fixture {
        fn new() -> Self {
            Self {
                campaign: Campaign::new("Test".into()),
                dungeon: Dungeon::new("Keep".into()),
                notes: NoteVault::default(),
                floor: None,
                tab: Tab::Spatial,
                graph: GraphEditorState::default(),
                spatial: SpatialViewState::default(),
                decor: DecorViewState::default(),
                encounters: EncountersViewState::default(),
                styled: StyledViewState::default(),
                presentation: PresentationViewState::default(),
                app_action: None,
                monsters: crate::data::MonsterDatabase::empty(),
                activity: Vec::new(),
            }
        }

        fn call(&mut self, tool: &str, args: Value) -> Result<Value, String> {
            let mut ctx = ToolCtx {
                campaign: &self.campaign,
                monsters: &self.monsters,
                bestiary_loading: false,
                activity: &mut self.activity,
                dungeon: &mut self.dungeon,
                notes: &mut self.notes,
                current_floor: &mut self.floor,
                active_tab: &mut self.tab,
                presenting: false,
                current_file: None,
                graph: &mut self.graph,
                spatial: &mut self.spatial,
                decor: &mut self.decor,
                encounters: &mut self.encounters,
                styled: &mut self.styled,
                presentation: &mut self.presentation,
                app_action: None,
            };
            let result = execute(&mut ctx, tool, &args);
            self.app_action = ctx.app_action.take();
            result
        }
    }

    #[test]
    fn every_tool_listed_is_handled() {
        let mut f = Fixture::new();
        for def in definitions() {
            let name = def["name"].as_str().unwrap();
            if name == "render_map" {
                continue;
            }
            if let Err(e) = f.call(name, json!({})) {
                assert!(!e.starts_with("Unknown tool"), "{} isn't handled", name);
            }
        }
    }

    #[test]
    fn rooms_connect_and_resolve_by_label() {
        let mut f = Fixture::new();
        f.call("add_room", json!({ "label": "Gatehouse", "tags": ["entrance"], "floor": 1 })).unwrap();
        let r = f.call("add_room", json!({ "label": "Crypt", "connect_to": "gatehouse", "connection_type": "secret", "size_hint": "large" })).unwrap();
        assert_eq!(r["room"]["floor"], 1, "takes the floor of the room it joins");
        assert_eq!(r["room"]["size"], json!([6, 6]));
        let edge = &f.dungeon.graph.connections[0];
        assert_eq!(edge.connection.connection_type, ConnectionType::Secret);
        assert_eq!(f.dungeon.graph.rooms[0].tags, vec![RoomTag::Entrance]);
        // Both rooms have a place on the Graph view, apart from each other.
        let p = &f.dungeon.graph.graph_positions;
        let (a, b) = (p[&f.dungeon.graph.rooms[0].id], p[&f.dungeon.graph.rooms[1].id]);
        assert!((a.0 - b.0).hypot(a.1 - b.1) > 100.0);

        let detail = f.call("get_room", json!({ "room": "CRYPT" })).unwrap();
        assert_eq!(detail["connections"][0]["from"], "Gatehouse");
    }

    #[test]
    fn bad_arguments_change_nothing() {
        let mut f = Fixture::new();
        f.call("add_room", json!({ "label": "Hall", "width": 5, "height": 4 })).unwrap();
        let e = f.call("update_room", json!({ "room": "Hall", "label": "Great Hall", "shape": "hexagon" })).unwrap_err();
        assert!(e.contains("Rectangle"), "lists the valid shapes: {}", e);
        assert_eq!(f.dungeon.graph.rooms[0].label, "Hall");

        let e = f.call("add_decor", json!({ "room": "Hall", "type": "Brazier", "x": 9, "y": 1 })).unwrap_err();
        assert!(e.contains("outside"), "{}", e);
        assert!(f.dungeon.graph.rooms[0].decor.is_empty());
    }

    #[test]
    fn decor_is_placed_updated_and_removed() {
        let mut f = Fixture::new();
        f.call("add_room", json!({ "label": "Shrine" })).unwrap();
        let item = f.call("add_decor", json!({ "room": "Shrine", "type": "stairs up", "x": 1.5, "y": 2 })).unwrap();
        let id = item["id"].as_str().unwrap().to_string();
        assert_eq!(f.dungeon.graph.rooms[0].decor[0].decor_type, DecorType::StairsUp);

        f.call("update_decor", json!({ "room": "Shrine", "decor": id, "cover": "Full", "light": { "bright": 2, "dim": 4 } })).unwrap();
        let d = &f.dungeon.graph.rooms[0].decor[0];
        assert_eq!(d.cover, Some(CoverKind::Full));
        assert!(matches!(d.light, DecorLightSetting::Custom(l) if l.bright == 2.0 && l.dim == 4.0));

        f.call("remove_decor", json!({ "room": "Shrine", "decor": id })).unwrap();
        assert!(f.dungeon.graph.rooms[0].decor.is_empty());
    }

    #[test]
    fn deleting_a_selected_room_clears_the_selection() {
        let mut f = Fixture::new();
        f.call("add_room", json!({ "label": "A" })).unwrap();
        f.call("add_room", json!({ "label": "B", "connect_to": "A" })).unwrap();
        f.call("focus_room", json!({ "room": "B" })).unwrap();
        assert!(f.spatial.selected_room.is_some());
        let sel = f.call("get_selection", json!({})).unwrap();
        assert_eq!(sel["rooms"][0]["label"], "B");

        f.call("delete_room", json!({ "room": "B" })).unwrap();
        assert!(f.spatial.selected_room.is_none());
        assert!(f.dungeon.graph.connections.is_empty());
    }

    #[test]
    fn room_notes_are_written_and_previewed_on_the_room() {
        let mut f = Fixture::new();
        f.call("add_room", json!({ "label": "Vault" })).unwrap();
        f.call("write_note", json!({ "about": "room", "name": "Vault", "body": "Dust lies thick." })).unwrap();
        f.call("write_note", json!({ "about": "room", "name": "Vault", "body": "A ghoul waits.", "mode": "append" })).unwrap();
        let n = f.call("read_note", json!({ "about": "room", "name": "Vault" })).unwrap();
        assert_eq!(n["body"], "Dust lies thick.\n\nA ghoul waits.");
        assert_eq!(f.dungeon.graph.rooms[0].note_excerpt, "Dust lies thick.");
        let hits = f.call("search_notes", json!({ "query": "ghoul" })).unwrap();
        assert_eq!(hits["results"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn renders_the_laid_out_map_as_a_png() {
        let mut f = Fixture::new();
        f.call("add_room", json!({ "label": "A" })).unwrap();
        f.call("add_room", json!({ "label": "B", "connect_to": "A" })).unwrap();
        let e = render(&mut f).err().unwrap();
        assert!(e.contains("laid out"), "{}", e);

        f.dungeon.layout = Some(crate::solver::layout::solve_layout(&mut f.dungeon.graph, 2, None).unwrap());
        let job = render(&mut f).unwrap();
        let out = job().unwrap();
        use base64::Engine;
        let png = base64::engine::general_purpose::STANDARD.decode(out.content[0]["data"].as_str().unwrap()).unwrap();
        assert_eq!(&png[1..4], b"PNG");
        let caption = out.content[1]["text"].as_str().unwrap();
        assert!(caption.contains("grid square") && caption.contains("\n- B: grid"), "{}", caption);
    }

    fn render(f: &mut Fixture) -> Result<RenderJob, String> {
        let ctx = ToolCtx {
            campaign: &f.campaign,
            monsters: &f.monsters,
            bestiary_loading: false,
            activity: &mut f.activity,
            dungeon: &mut f.dungeon,
            notes: &mut f.notes,
            current_floor: &mut f.floor,
            active_tab: &mut f.tab,
            presenting: false,
            current_file: None,
            graph: &mut f.graph,
            spatial: &mut f.spatial,
            decor: &mut f.decor,
            encounters: &mut f.encounters,
            styled: &mut f.styled,
            presentation: &mut f.presentation,
            app_action: None,
        };
        render_job(&ctx, &json!({}))
    }

    #[test]
    fn switch_map_finds_maps_by_name_or_index() {
        let mut f = Fixture::new();
        f.campaign.maps = vec![Dungeon::new("Keep".into()), Dungeon::new("Sewers".into()), Dungeon::new("Tower".into())];
        f.dungeon.name = "Keep (renamed)".into();

        let r = f.call("switch_map", json!({ "map": "keep (RENAMED)" })).unwrap();
        assert_eq!(r["already_open"], "Keep (renamed)", "the open map goes by its working name");
        assert_eq!(f.app_action, None);

        f.call("switch_map", json!({ "map": "sewers" })).unwrap();
        assert_eq!(f.app_action, Some(AppAction::OpenMap(1)));
        f.call("switch_map", json!({ "map": 2 })).unwrap();
        assert_eq!(f.app_action, Some(AppAction::OpenMap(2)));
        f.call("create_map", json!({ "name": " Crypt " })).unwrap();
        assert_eq!(f.app_action, Some(AppAction::CreateMap("Crypt".into())));

        let e = f.call("switch_map", json!({ "map": "Crypt" })).unwrap_err();
        assert!(e.contains("Sewers (1)"), "lists the maps: {}", e);
        assert!(f.call("switch_map", json!({ "map": 3 })).is_err());
    }

    fn bestiary() -> crate::data::MonsterDatabase {
        let monster = |v: Value| serde_json::from_value::<crate::model::Monster>(v).unwrap();
        crate::data::MonsterDatabase::from_list(vec![
            monster(json!({ "name": "Goblin", "source": "MM", "size": ["S"], "type": "humanoid", "cr": "1/4" })),
            monster(json!({
                "name": "Goblin Warrior", "source": "XMM", "size": ["S"], "type": "fey", "cr": "1/4",
                "ac": [15], "hp": { "average": 10, "formula": "3d6" }, "environment": ["forest"],
                "action": [{ "name": "Scimitar", "entries": ["{@atkr m} {@hit 4}, reach 5 ft."] }],
            })),
            monster(json!({ "name": "Goblin", "source": "XMM", "size": ["S"], "type": "fey", "cr": "1/4" })),
            monster(json!({ "name": "Goblin Boss", "source": "XMM", "size": ["S"], "type": "fey", "cr": "1" })),
            monster(json!({ "name": "Ghoul", "source": "XMM", "size": ["M"], "type": "undead", "cr": "1" })),
        ])
    }

    #[test]
    fn encounters_are_built_from_the_bestiary() {
        let mut f = Fixture::new();
        f.monsters = bestiary();
        f.call("add_room", json!({ "label": "Den" })).unwrap();
        let e = f.call("create_encounter", json!({
            "room": "Den", "name": "Ambush", "notes": "They flee at half strength.",
            "monsters": [{ "name": "goblin", "count": 4 }, { "name": "Goblin", "source": "MM" }, { "name": "Goblin Boss" }],
        })).unwrap();
        // The 2024 Goblin wins unless a book is named.
        assert_eq!(e["monsters"][0]["source"], "XMM");
        assert_eq!(e["monsters"][1]["source"], "MM");
        assert_eq!(e["total_xp"], 4 * 50 + 50 + 200);

        let overview = f.call("get_overview", json!({})).unwrap();
        assert_eq!(overview["encounters"][0]["monsters"], "4× Goblin, 1× Goblin, 1× Goblin Boss");
        let room = f.call("get_room", json!({ "room": "Den" })).unwrap();
        assert_eq!(room["encounters"][0]["name"], "Ambush");

        let err = f.call("create_encounter", json!({ "room": "Den", "name": "X", "monsters": [{ "name": "Gobbo" }] }));
        assert!(err.is_err());
        let err = f.call("create_encounter", json!({ "room": "Den", "name": "X", "monsters": [{ "name": "Goblin King" }] })).unwrap_err();
        assert!(err.contains("Goblin Boss"), "suggests near names: {}", err);
        assert_eq!(f.dungeon.encounters.len(), 1, "a failed create adds nothing");
    }

    #[test]
    fn changing_or_removing_an_encounter_drops_its_stale_tokens() {
        use crate::model::{MapToken, MonsterInstanceId, TokenKind};
        let mut f = Fixture::new();
        f.monsters = bestiary();
        f.call("add_room", json!({ "label": "Den" })).unwrap();
        f.call("add_room", json!({ "label": "Crypt" })).unwrap();
        let e = f.call("create_encounter", json!({ "room": "Den", "name": "Pack", "monsters": [{ "name": "Ghoul", "count": 3 }] })).unwrap();
        let id = e["id"].as_str().unwrap().to_string();
        for instance in 0..3 {
            f.dungeon.tokens.push(MapToken {
                kind: TokenKind::Monster(MonsterInstanceId { encounter_id: id.clone(), monster_index: 0, instance }),
                x: 0.5, y: 0.5, floor: None,
            });
        }
        let e = f.call("update_encounter", json!({
            "encounter": "pack", "room": "Crypt", "wandering": true, "wander_range": 2,
            "monsters": [{ "name": "Ghoul", "count": 1 }],
            "hazard": { "damage": "1d6", "save_dc": 12, "save_ability": "con", "condition": "Poisoned" },
        })).unwrap();
        assert_eq!(e["room"], "Crypt");
        assert_eq!(e["wandering"]["range"], 2);
        assert_eq!(f.dungeon.encounters[0].hazard.as_ref().unwrap().save_ability, crate::model::SaveAbility::Con);
        assert_eq!(f.dungeon.tokens.len(), 1, "tokens past the new count go");

        f.call("update_encounter", json!({ "encounter": "Pack", "hazard": null })).unwrap();
        assert!(f.dungeon.encounters[0].hazard.is_none());
        f.call("delete_encounter", json!({ "encounter": "Pack" })).unwrap();
        assert!(f.dungeon.encounters.is_empty() && f.dungeon.tokens.is_empty());
    }

    #[test]
    fn monsters_can_be_searched_and_read() {
        let mut f = Fixture::new();
        f.monsters = bestiary();
        let r = f.call("search_monsters", json!({ "type": "fey", "cr_min": "1/4", "cr_max": 1 })).unwrap();
        let names: Vec<&str> = r["monsters"].as_array().unwrap().iter().map(|m| m["name"].as_str().unwrap()).collect();
        assert_eq!(names, ["Goblin", "Goblin Warrior", "Goblin Boss"], "sorted by CR, then name");
        let r = f.call("search_monsters", json!({ "environment": "forest" })).unwrap();
        assert_eq!(r["total"], 1);

        let block = f.call("get_monster", json!({ "name": "goblin warrior" })).unwrap();
        let text = block["stat_block"].as_str().unwrap();
        assert!(text.contains("AC 15, HP 10 (3d6)") && text.contains("- Scimitar. ") && text.contains("+4"), "{}", text);
        let both = f.call("get_monster", json!({ "name": "Goblin" })).unwrap();
        assert_eq!(both["also_in"], json!(["MM"]));
    }

    #[test]
    fn writes_are_logged_and_reads_are_not() {
        let mut f = Fixture::new();
        f.call("add_room", json!({ "label": "Hall" })).unwrap();
        f.call("get_overview", json!({})).unwrap();
        f.call("write_note", json!({ "about": "room", "name": "hall", "body": "x" })).unwrap();
        let log: Vec<&str> = f.activity.iter().map(|(_, s)| s.as_str()).collect();
        assert_eq!(log, ["added room Hall", "wrote notes for Hall"]);
    }

    #[test]
    fn save_needs_a_file() {
        let mut f = Fixture::new();
        assert!(f.call("save", json!({})).unwrap_err().contains("Save As"));
        assert_eq!(f.app_action, None);
    }

    #[test]
    fn focusing_a_room_on_another_floor_switches_floor() {
        let mut f = Fixture::new();
        f.call("add_room", json!({ "label": "Cellar", "floor": -1 })).unwrap();
        f.call("set_floor", json!({ "floor": 0 })).unwrap();
        f.call("focus_room", json!({ "room": "Cellar", "view": "decor" })).unwrap();
        assert_eq!(f.floor, Some(-1));
        assert_eq!(f.tab, Tab::Decor);
        assert!(f.decor.selected_room.is_some());
    }
}
