//! Tools for hex maps: reading and painting the hexes, rivers and roads, battle maps
//! for the hexes, and the party and what the players have seen.

use serde_json::{json, Value};

use super::tools::{names, opt_bool, opt_f32, opt_str, parse_enum, req_str, tool, ToolCtx};
use crate::model::{Dungeon, Hex, HexCoord, HexFeature, HexMap, HexOrientation, HexPath, HexPathKind, HexSight, HexTerrain};

pub fn definitions() -> Vec<Value> {
    let coord = json!({ "type": "array", "items": { "type": "integer" }, "minItems": 2, "maxItems": 2, "description": "[q, r] axial coordinates" });
    let coords = json!({ "type": "array", "items": coord.clone() });
    let terrain = json!({ "type": "string", "enum": names(&HexTerrain::ALL) });
    let path = json!({ "type": "string", "description": "Path id from get_hex_map" });
    vec![
        tool("get_hex_map",
            "The open hex map: every hex (axial [q, r], terrain, landmark, label, sheep, battle map, whether it has a note), the rivers, roads and tracks, the party's hex, the hexes the players have seen (revealed) and of those the ones the party has been in (explored; the rest were only observed from afar), and the total sheep. \
             Axial coordinates: the centre is [0, 0]; the six neighbours of [q, r] are [q+1, r], [q+1, r-1], [q, r-1], [q-1, r], [q-1, r+1], [q, r+1]; steps between two hexes = (|dq| + |dr| + |dq + dr|) / 2. \
             Style: ink on paper, colour only for plants and water; a hex shows one landmark at most.",
            json!({}), &[]),
        tool("set_hexes",
            "Paint hexes: set terrain, landmark, label, sheep or battle map. Only the fields given change. A hex not on the map yet is added (Plains unless terrain is given).",
            json!({ "hexes": { "type": "array", "items": { "type": "object", "properties": {
                "q": { "type": "integer" }, "r": { "type": "integer" },
                "terrain": terrain.clone(),
                "landmark": { "type": ["string", "null"], "enum": names(&HexFeature::ALL).into_iter().map(Value::from).chain([Value::Null]).collect::<Vec<_>>(), "description": "Drawn in the hex's middle; null clears" },
                "label": { "type": "string", "description": "Empty clears it" },
                "sheep": { "type": "integer", "minimum": 0, "description": "How many sheep are in the hex" },
                "battle_map": { "type": ["string", "null"], "description": "The map (its name) to fight on when an encounter happens here; null unlinks" },
            }, "required": ["q", "r"] } } }),
            &["hexes"]),
        tool("remove_hexes", "Take hexes off the map (rivers and roads through them are cut there).",
            json!({ "hexes": coords.clone() }), &["hexes"]),
        tool("add_hex_path",
            "Draw a river, road or track through the hexes. Give its hexes in order; gaps between them fill in with a straight run of hexes.",
            json!({ "kind": { "type": "string", "enum": names(&HexPathKind::ALL) }, "hexes": coords.clone() }), &["kind", "hexes"]),
        tool("update_hex_path", "Change a river, road or track: what it is, or the hexes it runs through.",
            json!({ "path": path.clone(), "kind": { "type": "string", "enum": names(&HexPathKind::ALL) }, "hexes": coords.clone() }), &["path"]),
        tool("remove_hex_path", "Remove a river, road or track.", json!({ "path": path }), &["path"]),
        tool("set_hex_map", "The open hex map's settings. Only the fields given change.",
            json!({
                "orientation": { "type": "string", "enum": names(&HexOrientation::ALL) },
                "miles_per_hex": { "type": "number" },
                "reveal_radius": { "type": "integer", "minimum": 0, "maximum": 3, "description": "How far round the party the players see as it moves" },
            }), &[]),
        tool("create_battle_map",
            "Make a new outdoor map to fight on in these hexes and link them to it. Its ground suits the first hex's terrain. The hex map stays open; use switch_map to build the new one.",
            json!({ "hexes": coords.clone(), "name": { "type": "string", "description": "Defaults to the hex's name plus (battle)" } }), &["hexes"]),
        tool("set_hex_party",
            "At the table: move the party (exploring its hex, observing round it), and set what the players know of hexes: \
             hidden, observed (seen from afar, shaded lighter) or explored (the party has been there). Applied in order: hide_all, reveal_all, party, observe_from, observe, explore, hide.",
            json!({
                "party": { "description": "[q, r], or null to take the party off the map", "oneOf": [coord.clone(), { "type": "null" }] },
                "observe_from": { "type": "object", "description": "Observe every hex within radius of this one, as from a watch tower; explored hexes stay explored",
                    "properties": { "hex": coord.clone(), "radius": { "type": "integer", "minimum": 0, "maximum": 10 } }, "required": ["hex", "radius"] },
                "observe": { "type": "array", "items": coord.clone(), "description": "Mark observed (an explored hex goes back to observed)" },
                "explore": { "type": "array", "items": coord.clone(), "description": "Mark explored" },
                "hide": coords.clone(),
                "reveal_all": { "type": "boolean", "description": "Observe every hex; explored ones stay explored" },
                "hide_all": { "type": "boolean" },
            }), &[]),
        tool("roll_hex_encounter",
            "At the table: roll on the hex map's random encounter table (weighted) for a hex, by default the party's. The result shows in the app for the DM to fight or clear. \
             Encounters themselves are made with create_encounter: room \"random\" puts one on the table (with a weight), a hex puts one there for good (never rolled).",
            json!({ "hex": coord.clone(), "clear": { "type": "boolean", "description": "Clear the last roll instead of rolling" } }), &[]),
        tool("focus_hex", "Select a hex in the app so the user sees it (and its note in the notes drawer).",
            json!({ "q": { "type": "integer" }, "r": { "type": "integer" } }), &["q", "r"]),
    ]
}

/// Run a hex tool; `None` if the tool isn't one of these.
pub fn run(ctx: &mut ToolCtx, tool: &str, args: &Value) -> Option<Result<Value, String>> {
    let f: fn(&mut ToolCtx, &Value) -> Result<Value, String> = match tool {
        "get_hex_map" => |ctx, _| Ok(hex_map_json(ctx, hex_map(ctx.dungeon)?)),
        "set_hexes" => set_hexes,
        "remove_hexes" => remove_hexes,
        "add_hex_path" => add_hex_path,
        "update_hex_path" => update_hex_path,
        "remove_hex_path" => remove_hex_path,
        "set_hex_map" => set_hex_map,
        "create_battle_map" => create_battle_map,
        "set_hex_party" => set_hex_party,
        "focus_hex" => focus_hex,
        "roll_hex_encounter" => roll_hex_encounter,
        _ => return None,
    };
    Some(f(ctx, args))
}

fn hex_map(d: &Dungeon) -> Result<&HexMap, String> {
    d.hex_map.as_ref().ok_or_else(|| format!("{} isn't a hex map. create_map with hex makes one.", d.name))
}

fn hex_map_mut(d: &mut Dungeon) -> Result<&mut HexMap, String> {
    let name = d.name.clone();
    d.hex_map.as_mut().ok_or_else(|| format!("{} isn't a hex map. create_map with hex makes one.", name))
}

fn parse_coord(v: &Value) -> Result<HexCoord, String> {
    let pair = match v {
        Value::Array(a) if a.len() == 2 => (a[0].as_i64(), a[1].as_i64()),
        Value::Object(_) => (v.get("q").and_then(Value::as_i64), v.get("r").and_then(Value::as_i64)),
        _ => (None, None),
    };
    match pair {
        (Some(q), Some(r)) => Ok(HexCoord::new(q as i32, r as i32)),
        _ => Err(format!("A hex is [q, r] in whole numbers, not {}", v)),
    }
}

fn parse_coords(args: &Value, key: &str) -> Result<Vec<HexCoord>, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Array(a)) => a.iter().map(parse_coord).collect(),
        Some(_) => Err(format!("'{}' must be a list of [q, r]", key)),
    }
}

fn coord_json(c: HexCoord) -> Value {
    json!([c.q, c.r])
}

/// Find a hex by "q,r", "Hex q,r" or its label: for notes about a hex.
pub(super) fn resolve_hex<'a>(d: &'a Dungeon, name: &str) -> Result<&'a Hex, String> {
    let map = hex_map(d)?;
    let bare = name.trim().trim_start_matches("Hex").trim();
    let parts: Vec<&str> = bare.split(',').map(str::trim).collect();
    if let [q, r] = parts.as_slice() {
        if let (Ok(q), Ok(r)) = (q.parse(), r.parse()) {
            return map.hex(HexCoord::new(q, r)).ok_or_else(|| format!("There's no hex {},{} on this map.", q, r));
        }
    }
    let found: Vec<&Hex> = map.hexes.iter().filter(|h| h.label.eq_ignore_ascii_case(name.trim())).collect();
    match found.as_slice() {
        [one] => Ok(one),
        [] => Err(format!("No hex labelled '{}'. Name a hex by its label or as q,r.", name)),
        _ => Err(format!("Several hexes are labelled '{}'; name it as q,r.", name)),
    }
}

/// The id of another map of the campaign, by name.
fn other_map_id(ctx: &ToolCtx, name: &str) -> Result<String, String> {
    let active = ctx.campaign.active_map;
    ctx.campaign.maps.iter().enumerate()
        .find(|(i, m)| *i != active && m.name.eq_ignore_ascii_case(name.trim()))
        .map(|(_, m)| m.id.clone())
        .ok_or_else(|| format!("No other map called '{}'. get_overview lists the maps.", name))
}

fn map_name(ctx: &ToolCtx, id: &str) -> String {
    ctx.campaign.maps.iter().find(|m| m.id == id).map_or_else(|| "(a missing map)".into(), |m| m.name.clone())
}

pub(super) fn hex_map_json(ctx: &ToolCtx, map: &HexMap) -> Value {
    let notes = &*ctx.notes;
    json!({
        "orientation": format!("{:?}", map.orientation),
        "miles_per_hex": map.miles_per_hex,
        "reveal_radius": map.reveal_radius,
        "party": map.party.map(coord_json),
        "revealed": map.revealed.iter().map(|c| coord_json(*c)).collect::<Vec<_>>(),
        "explored": map.explored.iter().map(|c| coord_json(*c)).collect::<Vec<_>>(),
        "total_sheep": map.total_sheep(),
        "random_encounters": map.random_encounters.iter().filter_map(|r| {
            let e = ctx.dungeon.encounters.iter().find(|e| e.id == r.encounter)?;
            Some(json!({
                "id": e.id, "name": e.name, "weight": r.weight,
                "chance": format!("{:.0}%", 100.0 * r.weight as f32 / map.random_weight().max(1) as f32),
                "monsters": super::encounters::monster_line(ctx, e),
            }))
        }).collect::<Vec<_>>(),
        "rolled": map.rolled.as_ref().and_then(|r| {
            let e = ctx.dungeon.encounters.iter().find(|e| e.id == r.encounter)?;
            Some(json!({ "name": e.name, "hex": coord_json(r.hex) }))
        }),
        "hexes": map.hexes.iter().map(|h| {
            let mut v = json!({ "hex": coord_json(h.coord), "terrain": format!("{:?}", h.terrain) });
            if let Some(f) = h.feature {
                v["landmark"] = json!(format!("{:?}", f));
            }
            if !h.label.is_empty() {
                v["label"] = json!(h.label);
            }
            if h.sheep > 0 {
                v["sheep"] = json!(h.sheep);
            }
            let own: Vec<&str> = ctx.dungeon.encounters.iter().filter(|e| e.home_room_id == h.id).map(|e| e.name.as_str()).collect();
            if !own.is_empty() {
                v["encounters"] = json!(own);
            }
            if let Some(id) = &h.battle_map {
                v["battle_map"] = json!(map_name(ctx, id));
            }
            if notes.note_id_for(&crate::notes::NoteBind::Hex(h.id.clone())).is_some() {
                v["has_note"] = json!(true);
            }
            v
        }).collect::<Vec<_>>(),
        "paths": map.paths.iter().map(|p| json!({
            "id": p.id,
            "kind": format!("{:?}", p.kind),
            "hexes": p.hexes.iter().map(|c| coord_json(*c)).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
    })
}

fn set_hexes(ctx: &mut ToolCtx, args: &Value) -> Result<Value, String> {
    let items = args.get("hexes").and_then(Value::as_array).ok_or("'hexes' must be a list")?;
    // Check everything first, so a bad entry changes nothing
    let mut edits = Vec::new();
    for item in items {
        let c = parse_coord(item)?;
        let terrain = opt_str(item, "terrain").map(|t| parse_enum(&HexTerrain::ALL, t, "terrain")).transpose()?;
        let landmark = match item.get("landmark") {
            None => None,
            Some(Value::Null) => Some(None),
            Some(v) => Some(Some(parse_enum(&HexFeature::ALL, v.as_str().ok_or("'landmark' must be a name or null")?, "landmark")?)),
        };
        let battle = match item.get("battle_map") {
            None => None,
            Some(Value::Null) => Some(None),
            Some(v) => Some(Some(other_map_id(ctx, v.as_str().ok_or("'battle_map' must be a map's name or null")?)?)),
        };
        let sheep = match item.get("sheep") {
            None | Some(Value::Null) => None,
            Some(v) => Some(v.as_u64().and_then(|n| u32::try_from(n).ok()).ok_or("'sheep' must be a whole number, 0 or more")?),
        };
        edits.push((c, terrain, landmark, opt_str(item, "label").map(str::to_string), battle, sheep));
    }
    let map = hex_map_mut(ctx.dungeon)?;
    let mut added = 0;
    for (c, terrain, landmark, label, battle, sheep) in edits {
        if map.hex(c).is_none() {
            added += 1;
        }
        let hex = map.ensure_hex(c, terrain.unwrap_or_default());
        if let Some(t) = terrain {
            hex.terrain = t;
        }
        if let Some(l) = landmark {
            hex.feature = l;
        }
        if let Some(l) = label {
            hex.label = l.trim().to_string();
        }
        if let Some(b) = battle {
            hex.battle_map = b;
        }
        if let Some(n) = sheep {
            hex.sheep = n;
        }
    }
    Ok(json!({ "changed": items.len(), "added": added, "hexes": map.hexes.len(), "total_sheep": map.total_sheep() }))
}

fn remove_hexes(ctx: &mut ToolCtx, args: &Value) -> Result<Value, String> {
    let coords = parse_coords(args, "hexes")?;
    let map = hex_map_mut(ctx.dungeon)?;
    let gone: Vec<Hex> = coords.into_iter().filter_map(|c| map.remove_hex(c)).collect();
    let hexes = map.hexes.len();
    // A hex's own encounters go with it
    let before = ctx.dungeon.encounters.len();
    ctx.dungeon.encounters.retain(|e| !gone.iter().any(|h| h.id == e.home_room_id));
    let encounters_removed = before - ctx.dungeon.encounters.len();
    ctx.hex.clear_selection();
    Ok(json!({ "removed": gone.len(), "hexes": hexes, "encounters_removed": encounters_removed }))
}

/// A path's hexes from the arguments: on the map, at least two, gaps filled.
fn path_hexes(map: &HexMap, args: &Value, kind: HexPathKind) -> Result<HexPath, String> {
    let points = parse_coords(args, "hexes")?;
    if points.len() < 2 {
        return Err("A path needs at least two hexes".into());
    }
    let path = HexPath::through(kind, &points);
    if let Some(off) = path.hexes.iter().find(|c| map.hex(**c).is_none()) {
        return Err(format!("The path would cross [{}, {}], which isn't on the map", off.q, off.r));
    }
    Ok(path)
}

fn add_hex_path(ctx: &mut ToolCtx, args: &Value) -> Result<Value, String> {
    let kind = parse_enum(&HexPathKind::ALL, req_str(args, "kind")?, "path kind")?;
    let map = hex_map_mut(ctx.dungeon)?;
    let path = path_hexes(map, args, kind)?;
    let v = json!({ "id": path.id, "kind": format!("{:?}", kind), "hexes": path.hexes.iter().map(|c| coord_json(*c)).collect::<Vec<_>>() });
    map.paths.push(path);
    Ok(v)
}

fn update_hex_path(ctx: &mut ToolCtx, args: &Value) -> Result<Value, String> {
    let id = req_str(args, "path")?;
    let kind = opt_str(args, "kind").map(|k| parse_enum(&HexPathKind::ALL, k, "path kind")).transpose()?;
    let map = hex_map_mut(ctx.dungeon)?;
    let i = map.paths.iter().position(|p| p.id == id).ok_or_else(|| format!("No path '{}'. get_hex_map lists them.", id))?;
    if let Some(k) = kind {
        map.paths[i].kind = k;
    }
    if args.get("hexes").is_some_and(|v| !v.is_null()) {
        map.paths[i].hexes = path_hexes(map, args, map.paths[i].kind)?.hexes;
    }
    let p = &map.paths[i];
    Ok(json!({ "id": p.id, "kind": format!("{:?}", p.kind), "hexes": p.hexes.iter().map(|c| coord_json(*c)).collect::<Vec<_>>() }))
}

fn remove_hex_path(ctx: &mut ToolCtx, args: &Value) -> Result<Value, String> {
    let id = req_str(args, "path")?;
    let map = hex_map_mut(ctx.dungeon)?;
    let before = map.paths.len();
    map.paths.retain(|p| p.id != id);
    if map.paths.len() == before {
        return Err(format!("No path '{}'. get_hex_map lists them.", id));
    }
    Ok(json!({ "removed": id }))
}

fn set_hex_map(ctx: &mut ToolCtx, args: &Value) -> Result<Value, String> {
    let orientation = opt_str(args, "orientation").map(|o| parse_enum(&HexOrientation::ALL, o, "orientation")).transpose()?;
    let miles = opt_f32(args, "miles_per_hex")?;
    if miles.is_some_and(|m| !(0.1..=100.0).contains(&m)) {
        return Err("'miles_per_hex' must be 0.1 to 100".into());
    }
    let radius = match args.get("reveal_radius") {
        None | Some(Value::Null) => None,
        Some(v) => Some(v.as_u64().filter(|n| *n <= 3).ok_or("'reveal_radius' must be 0 to 3")? as u32),
    };
    let map = hex_map_mut(ctx.dungeon)?;
    if let Some(o) = orientation {
        map.orientation = o;
    }
    if let Some(m) = miles {
        map.miles_per_hex = m;
    }
    if let Some(r) = radius {
        map.reveal_radius = r;
    }
    Ok(json!({ "orientation": format!("{:?}", map.orientation), "miles_per_hex": map.miles_per_hex, "reveal_radius": map.reveal_radius }))
}

fn create_battle_map(ctx: &mut ToolCtx, args: &Value) -> Result<Value, String> {
    let coords = parse_coords(args, "hexes")?;
    let map = hex_map(ctx.dungeon)?;
    let first = coords.first().ok_or("Name at least one hex")?;
    let first = map.hex(*first).ok_or_else(|| format!("There's no hex [{}, {}] on this map", first.q, first.r))?;
    if let Some(off) = coords.iter().find(|c| map.hex(**c).is_none()) {
        return Err(format!("There's no hex [{}, {}] on this map", off.q, off.r));
    }
    let name = match opt_str(args, "name").map(str::trim).filter(|n| !n.is_empty()) {
        Some(n) => n.to_string(),
        None => format!("{} (battle)", first.name()),
    };
    if ctx.campaign.maps.iter().any(|m| m.name.eq_ignore_ascii_case(&name)) || ctx.dungeon.name.eq_ignore_ascii_case(&name) {
        return Err(format!("There's already a map called '{}'; give a name.", name));
    }
    let mut battle = Dungeon::new(name.clone());
    battle.theme.ground = first.terrain.battle_ground();
    let ground = battle.theme.ground;
    let id = battle.id.clone();
    ctx.campaign.maps.push(battle);
    let map = hex_map_mut(ctx.dungeon)?;
    for c in &coords {
        if let Some(h) = map.hex_mut(*c) {
            h.battle_map = Some(id.clone());
        }
    }
    Ok(json!({ "created": name, "ground": format!("{:?}", ground), "linked_hexes": coords.len() }))
}

fn set_hex_party(ctx: &mut ToolCtx, args: &Value) -> Result<Value, String> {
    let party = match args.get("party") {
        None => None,
        Some(Value::Null) => Some(None),
        Some(v) => Some(Some(parse_coord(v)?)),
    };
    let (observe, explore, hide) = (parse_coords(args, "observe")?, parse_coords(args, "explore")?, parse_coords(args, "hide")?);
    let lookout = match args.get("observe_from") {
        None | Some(Value::Null) => None,
        Some(v) => {
            let hex = parse_coord(v.get("hex").ok_or("'observe_from' needs a hex")?)?;
            let radius = v.get("radius").and_then(Value::as_u64).filter(|n| *n <= 10).ok_or("'observe_from' needs a radius of 0 to 10")?;
            Some((hex, radius as u32))
        }
    };
    let map = hex_map_mut(ctx.dungeon)?;
    if let Some(off) = observe.iter().chain(&explore).chain(lookout.as_ref().map(|(c, _)| c)).find(|c| map.hex(**c).is_none()) {
        return Err(format!("There's no hex [{}, {}] on this map", off.q, off.r));
    }
    if let Some(Some(c)) = party {
        if map.hex(c).is_none() {
            return Err(format!("There's no hex [{}, {}] on this map", c.q, c.r));
        }
    }
    if opt_bool(args, "hide_all")?.unwrap_or(false) {
        map.hide_all();
    }
    if opt_bool(args, "reveal_all")?.unwrap_or(false) {
        map.revealed = map.hexes.iter().map(|h| h.coord).collect();
    }
    match party {
        Some(Some(c)) => map.move_party(c),
        Some(None) => map.party = None,
        None => {}
    }
    if let Some((c, radius)) = lookout {
        map.observe_from(c, radius);
    }
    for c in observe {
        map.set_sight(c, HexSight::Observed);
    }
    for c in explore {
        map.set_sight(c, HexSight::Explored);
    }
    for c in hide {
        map.set_sight(c, HexSight::Hidden);
    }
    Ok(json!({ "party": map.party.map(coord_json), "revealed": map.revealed.len(), "explored": map.explored.len(), "hexes": map.hexes.len() }))
}

fn focus_hex(ctx: &mut ToolCtx, args: &Value) -> Result<Value, String> {
    let c = parse_coord(args)?;
    let map = hex_map(ctx.dungeon)?;
    let hex = map.hex(c).ok_or_else(|| format!("There's no hex [{}, {}] on this map", c.q, c.r))?;
    let name = hex.name();
    ctx.hex.selected = vec![c];
    ctx.hex.selected_path = None;
    Ok(json!({ "selected": name }))
}

/// Keep the hex map's random table in step with the map's encounters.
pub(super) fn sync_random(ctx: &mut ToolCtx) {
    let d = &mut *ctx.dungeon;
    if let Some(map) = d.hex_map.as_mut() {
        map.sync_random(&d.encounters);
    }
}

/// After an encounter is made or changed: bring the random table in step, and set
/// the encounter's weight on it if one was given.
pub(super) fn sync_encounter_weight(ctx: &mut ToolCtx, id: &str, args: &Value) -> Result<(), String> {
    sync_random(ctx);
    let Some(w) = args.get("weight").filter(|v| !v.is_null()) else { return Ok(()) };
    let w = w.as_u64().filter(|n| (1..=100).contains(n)).ok_or("'weight' must be 1 to 100")? as u32;
    let entry = ctx.dungeon.hex_map.as_mut()
        .and_then(|m| m.random_encounters.iter_mut().find(|r| r.encounter == id))
        .ok_or("'weight' is only for encounters on a hex map's random table (room \"random\")")?;
    entry.weight = w;
    Ok(())
}

/// An encounter's weight on the random table, if it's on one.
pub(super) fn random_weight_of(d: &Dungeon, id: &str) -> Option<u32> {
    d.hex_map.as_ref()?.random_encounters.iter().find(|r| r.encounter == id).map(|r| r.weight)
}

fn roll_hex_encounter(ctx: &mut ToolCtx, args: &Value) -> Result<Value, String> {
    sync_random(ctx);
    let map = hex_map_mut(ctx.dungeon)?;
    if opt_bool(args, "clear")?.unwrap_or(false) {
        map.rolled = None;
        return Ok(json!({ "cleared": true }));
    }
    let at = match args.get("hex").filter(|v| !v.is_null()) {
        Some(v) => parse_coord(v)?,
        None => map.party.ok_or("The party isn't on the map; name a hex")?,
    };
    let hex_name = map.hex(at).ok_or_else(|| format!("There's no hex [{}, {}] on this map", at.q, at.r))?.name();
    let id = map.roll_random(at).ok_or("The map has no random encounters yet; create_encounter with room \"random\" adds them")?.encounter.clone();
    let e = ctx.dungeon.encounters.iter().find(|e| e.id == id).ok_or("The rolled encounter is missing")?;
    Ok(json!({ "rolled": super::encounters::encounter_summary(ctx, e), "hex": hex_name }))
}
