//! Tools for light and look: ambient light, placed light sources, which decor gives
//! light (campaign-wide or for one map), and the map's drawing style.

use serde_json::{json, Value};

use super::tools::{label_of, names, opt_bool, opt_f32, opt_str, parse_enum, req_str, resolve_room, tool, ToolCtx};
use crate::model::{ChamferStyle, DecorLight, DecorLighting, DecorType, LightSource, ShadingStyle, TokenKind};

pub fn definitions() -> Vec<Value> {
    let room = json!({ "type": "string", "description": "Room id, or its label (case-insensitive)" });
    let color = json!({ "type": "array", "items": { "type": "integer" }, "description": "[r, g, b]" });
    let light_fields = json!({
        "room": room,
        "bright": { "type": "number", "description": "Bright radius in grid squares" },
        "dim": { "type": "number", "description": "Dim radius in grid squares (from the light); default twice bright" },
        "intensity": { "type": "number", "description": "0 to 1" },
        "color": color,
        "x": { "type": "number", "description": "Map grid position; omit x and y for the room's centre" },
        "y": { "type": "number" },
        "carried_by": { "type": ["string", "null"], "description": "A party member who carries it (it moves with their token); null for nobody" },
    });
    let mut light_update = light_fields.clone();
    light_update["light"] = json!({ "type": "string", "description": "Light id from list_lights" });
    light_update["centre"] = json!({ "type": "boolean", "description": "Put it back at its room's centre" });
    vec![
        tool("list_lights", "The map's ambient light, its placed light sources, and the decor-lighting settings in force.",
            json!({}), &[]),
        tool("set_ambient_light", "How lit the whole map is, from 0 (dark) to 1 (fully lit).",
            json!({ "level": { "type": "number" } }), &["level"]),
        tool("add_light", "Place a light source (torch, lamp, glowing crystal) in a room.", light_fields, &["room", "bright"]),
        tool("update_light", "Change a light source. Only the fields given change.", light_update, &["light"]),
        tool("remove_light", "Remove a light source.",
            json!({ "light": { "type": "string", "description": "Light id from list_lights" } }), &["light"]),
        tool("set_decor_lighting",
            "Which decor types give light, and how much: for the whole campaign, or this map only (which then stops following the campaign).",
            json!({
                "scope": { "type": "string", "enum": ["campaign", "map"] },
                "enabled": { "type": "boolean", "description": "Off: no decor gives light by default" },
                "set": {
                    "type": "object",
                    "description": "Decor type → its light, e.g. {\"Altar\": {\"bright\": 1, \"dim\": 2}}",
                    "additionalProperties": { "type": "object", "properties": {
                        "bright": { "type": "number" }, "dim": { "type": "number" }, "color": color.clone() } },
                },
                "remove": { "type": "array", "items": { "type": "string" }, "description": "Decor types that stop giving light" },
                "follow_campaign": { "type": "boolean", "description": "scope=map only: drop this map's own settings and use the campaign's" },
            }), &["scope"]),
        tool("set_theme", "The map's drawing style (the Styled view and exports). Only the fields given change.",
            json!({
                "exterior_shading": { "type": "boolean", "description": "Shade the rock around the rooms" },
                "shading_radius": { "type": "number", "description": "0.2 to 3 squares" },
                "shading_style": { "type": "string", "enum": names(&ShadingStyle::ALL) },
                "hatching_density": { "type": "number", "description": "0.3 to 3" },
                "corridor_chamfer": { "type": "string", "enum": names(&ChamferStyle::ALL), "description": "Corridor corner style" },
            }), &[]),
    ]
}

/// Run a lighting or theme tool; `None` if the tool isn't one of these.
pub fn run(ctx: &mut ToolCtx, tool: &str, args: &Value) -> Option<Result<Value, String>> {
    Some(match tool {
        "list_lights" => Ok(list_lights(ctx)),
        "set_ambient_light" => set_ambient_light(ctx, args),
        "add_light" => add_light(ctx, args),
        "update_light" => update_light(ctx, args),
        "remove_light" => remove_light(ctx, args),
        "set_decor_lighting" => set_decor_lighting(ctx, args),
        "set_theme" => set_theme(ctx, args),
        _ => return None,
    })
}

fn color_arg(v: &Value) -> Result<[u8; 3], String> {
    match v.as_array() {
        Some(c) if c.len() == 3 => {
            let ch = |i: usize| c[i].as_u64().map(|n| n.min(255) as u8).ok_or("color must be [r,g,b]");
            Ok([ch(0)?, ch(1)?, ch(2)?])
        }
        _ => Err("color must be [r,g,b]".into()),
    }
}

fn lighting_json(l: &DecorLighting) -> Value {
    json!({
        "enabled": l.enabled,
        "types": l.by_type.iter().map(|(t, d)| (format!("{:?}", t), json!({ "bright": d.bright, "dim": d.dim, "color": d.color })))
            .collect::<serde_json::Map<_, _>>(),
    })
}

fn light_json(ctx: &ToolCtx, l: &LightSource) -> Value {
    let carrier = match &l.carrier {
        Some(TokenKind::Player(id)) => ctx.dungeon.party.iter().find(|p| p.id == *id).map(|p| p.name.clone()),
        _ => None,
    };
    json!({
        "id": l.id,
        "room": label_of(ctx.dungeon, &l.room_id),
        "bright": l.radius,
        "dim": l.dim_radius(),
        "intensity": l.intensity,
        "color": l.color,
        "position": l.pos.map(|(x, y)| json!([x, y])).unwrap_or(json!("room centre")),
        "carried_by": carrier,
    })
}

fn list_lights(ctx: &ToolCtx) -> Value {
    let d = &*ctx.dungeon;
    json!({
        "ambient_light": d.ambient_light,
        "lights": d.light_sources.iter().map(|l| light_json(ctx, l)).collect::<Vec<_>>(),
        "decor_lighting": {
            "campaign": d.campaign_decor_lighting.as_ref().map(lighting_json),
            "map": d.decor_lighting.as_ref().map(lighting_json),
            "in_force": if d.decor_lighting.is_some() { "map" } else { "campaign" },
        },
    })
}

fn set_ambient_light(ctx: &mut ToolCtx, args: &Value) -> Result<Value, String> {
    let level = opt_f32(args, "level")?.ok_or("Missing 'level'")?;
    ctx.dungeon.ambient_light = level.clamp(0.0, 1.0);
    Ok(json!({ "ambient_light": ctx.dungeon.ambient_light }))
}

/// Apply light fields onto `l`; checks everything before the caller keeps it.
fn apply_light_fields(ctx: &ToolCtx, l: &mut LightSource, args: &Value) -> Result<(), String> {
    if let Some(r) = opt_str(args, "room") {
        l.room_id = resolve_room(ctx.dungeon, r)?;
    }
    if let Some(b) = opt_f32(args, "bright")? {
        l.radius = b.clamp(0.0, 60.0);
    }
    if let Some(d) = opt_f32(args, "dim")? {
        l.dim_radius = Some(d.max(l.radius));
    }
    if let Some(i) = opt_f32(args, "intensity")? {
        l.intensity = i.clamp(0.0, 1.0);
    }
    if let Some(c) = args.get("color").filter(|v| !v.is_null()) {
        l.color = color_arg(c)?;
    }
    match (opt_f32(args, "x")?, opt_f32(args, "y")?) {
        (Some(x), Some(y)) => l.pos = Some((x, y)),
        (None, None) => {}
        _ => return Err("Give both x and y, or neither".into()),
    }
    if opt_bool(args, "centre")? == Some(true) {
        l.pos = None;
    }
    match args.get("carried_by") {
        None => {}
        Some(Value::Null) => l.carrier = None,
        Some(v) => {
            let name = v.as_str().ok_or("'carried_by' must be a party member's name")?;
            let pc = ctx.dungeon.party.iter().find(|p| p.name.eq_ignore_ascii_case(name.trim()))
                .ok_or_else(|| format!("No party member called '{}'", name))?;
            l.carrier = Some(TokenKind::Player(pc.id.clone()));
        }
    }
    Ok(())
}

fn add_light(ctx: &mut ToolCtx, args: &Value) -> Result<Value, String> {
    let room_id = resolve_room(ctx.dungeon, req_str(args, "room")?)?;
    let mut l = LightSource {
        id: uuid::Uuid::new_v4().to_string(),
        room_id,
        radius: 4.0,
        intensity: 1.0,
        color: DecorLight::TORCH.color,
        pos: None,
        dim_radius: None,
        carrier: None,
    };
    apply_light_fields(ctx, &mut l, args)?;
    let v = light_json(ctx, &l);
    ctx.dungeon.light_sources.push(l);
    Ok(v)
}

fn update_light(ctx: &mut ToolCtx, args: &Value) -> Result<Value, String> {
    let id = req_str(args, "light")?;
    let i = ctx.dungeon.light_sources.iter().position(|l| l.id == id)
        .ok_or_else(|| format!("No light '{}'. list_lights lists them.", id))?;
    let mut l = ctx.dungeon.light_sources[i].clone();
    apply_light_fields(ctx, &mut l, args)?;
    let v = light_json(ctx, &l);
    ctx.dungeon.light_sources[i] = l;
    Ok(v)
}

fn remove_light(ctx: &mut ToolCtx, args: &Value) -> Result<Value, String> {
    let id = req_str(args, "light")?;
    let i = ctx.dungeon.light_sources.iter().position(|l| l.id == id)
        .ok_or_else(|| format!("No light '{}'. list_lights lists them.", id))?;
    let l = ctx.dungeon.light_sources.remove(i);
    Ok(json!({ "removed_light_in": label_of(ctx.dungeon, &l.room_id) }))
}

fn set_decor_lighting(ctx: &mut ToolCtx, args: &Value) -> Result<Value, String> {
    let scope = req_str(args, "scope")?;
    if scope == "map" && opt_bool(args, "follow_campaign")? == Some(true) {
        ctx.dungeon.decor_lighting = None;
        return Ok(json!({ "map": "follows the campaign" }));
    }
    let d = &*ctx.dungeon;
    let mut lighting = match scope {
        // The working map carries the campaign's settings; the app copies them back.
        "campaign" => d.campaign_decor_lighting.clone().unwrap_or_default(),
        // A map without its own settings starts from the campaign's.
        "map" => d.decor_lighting.clone().or_else(|| d.campaign_decor_lighting.clone()).unwrap_or_default(),
        other => return Err(format!("Unknown scope '{}': campaign or map", other)),
    };
    if let Some(e) = opt_bool(args, "enabled")? {
        lighting.enabled = e;
    }
    if let Some(set) = args.get("set").and_then(Value::as_object) {
        for (name, light) in set {
            let t = parse_enum(&DecorType::ALL, name, "decor type")?;
            let bright = opt_f32(light, "bright")?.ok_or("each light needs 'bright'")?.max(0.0);
            let dim = opt_f32(light, "dim")?.unwrap_or(bright * 2.0).max(bright);
            let color = match light.get("color") {
                Some(c) if !c.is_null() => color_arg(c)?,
                _ => DecorLight::TORCH.color,
            };
            lighting.by_type.insert(t, DecorLight { bright, dim, color });
        }
    }
    if let Some(remove) = args.get("remove").and_then(Value::as_array) {
        for name in remove.iter().filter_map(Value::as_str) {
            lighting.by_type.remove(&parse_enum(&DecorType::ALL, name, "decor type")?);
        }
    }
    let mut v = serde_json::Map::new();
    v.insert(scope.to_string(), lighting_json(&lighting));
    match scope {
        "campaign" => ctx.dungeon.campaign_decor_lighting = Some(lighting),
        _ => ctx.dungeon.decor_lighting = Some(lighting),
    }
    Ok(Value::Object(v))
}

fn set_theme(ctx: &mut ToolCtx, args: &Value) -> Result<Value, String> {
    let mut theme = ctx.dungeon.theme.clone();
    if let Some(b) = opt_bool(args, "exterior_shading")? {
        theme.exterior_shading = b;
    }
    if let Some(r) = opt_f32(args, "shading_radius")? {
        theme.shading_radius = r.clamp(0.2, 3.0);
    }
    if let Some(s) = opt_str(args, "shading_style") {
        theme.shading_style = parse_enum(&ShadingStyle::ALL, s, "shading style")?;
    }
    if let Some(h) = opt_f32(args, "hatching_density")? {
        theme.hatching_density = h.clamp(0.3, 3.0);
    }
    if let Some(c) = opt_str(args, "corridor_chamfer") {
        theme.corridor_chamfer = parse_enum(&ChamferStyle::ALL, c, "corridor corner style")?;
    }
    ctx.dungeon.theme = theme;
    let t = &ctx.dungeon.theme;
    Ok(json!({
        "exterior_shading": t.exterior_shading,
        "shading_radius": t.shading_radius,
        "shading_style": format!("{:?}", t.shading_style),
        "hatching_density": t.hatching_density,
        "corridor_chamfer": format!("{:?}", t.corridor_chamfer),
    }))
}
