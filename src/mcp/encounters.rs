//! Tools for encounters and the monsters in them: searching the bestiary, reading
//! stat blocks, and creating, changing and removing a map's encounters.

use serde_json::{json, Value};

use super::tools::{label_of, opt_bool, opt_f32, opt_str, req_str, resolve_room, tool, ToolCtx};
use crate::model::monster::{
    alignment_display, cr_to_numeric, damage_list_display, entry_to_text, size_label, strip_5e_markup, Feature,
};
use crate::model::{
    ArmorClass, ChallengeRating, CustomMonster, Encounter, EncounterMonster, EncounterType, Hazard, HitPoints,
    MergeConfig, Monster, MonsterRef, MonsterType, SaveAbility, SpeedValue,
};
use crate::data::monster_db::SOURCES_2024;

const SEARCH_LIMIT: usize = 50;

pub fn definitions() -> Vec<Value> {
    let room = json!({ "type": "string", "description": "Room id, or its label (case-insensitive)" });
    let encounter = json!({ "type": "string", "description": "Encounter id, or its name (case-insensitive)" });
    let cr = json!({ "description": "Challenge rating, e.g. 2 or \"1/4\"", "oneOf": [{ "type": "number" }, { "type": "string" }] });
    let monsters = json!({
        "type": "array",
        "description": "Replaces the encounter's monsters. Names as in search_monsters; source picks one edition when a name is in several books (2024 books are preferred otherwise).",
        "items": { "type": "object", "properties": {
            "name": { "type": "string" },
            "source": { "type": "string" },
            "count": { "type": "integer", "minimum": 1 },
            "notes": { "type": "string" },
        }, "required": ["name"] },
    });
    let hazard = json!({
        "description": "Makes it a hazard that affects other encounters in the room: damage dice, optional save, optional condition. null removes it.",
        "type": ["object", "null"],
        "properties": {
            "damage": { "type": "string", "description": "Dice, e.g. \"2d6\"" },
            "save_dc": { "type": "integer" },
            "save_ability": { "type": "string", "enum": ["Str", "Dex", "Con", "Int", "Wis", "Cha"] },
            "condition": { "type": "string", "description": "e.g. Poisoned, Restrained" },
        },
    });
    let encounter_fields = json!({
        "name": { "type": "string" },
        "room": room,
        "monsters": monsters,
        "notes": { "type": "string", "description": "DM notes: tactics, morale, treasure" },
        "wandering": { "type": "boolean", "description": "Wanders between rooms instead of staying put" },
        "wander_range": { "type": ["integer", "null"], "description": "How many rooms away it wanders; null = anywhere" },
        "hazard": hazard,
    });
    let mut update_fields = encounter_fields.clone();
    update_fields["encounter"] = encounter.clone();
    let features = json!({ "type": "array", "items": { "type": "object", "properties": {
        "name": { "type": "string" }, "text": { "type": "string" } }, "required": ["name", "text"] } });
    let custom_fields = json!({
        "size": { "type": "string", "enum": ["Tiny", "Small", "Medium", "Large", "Huge", "Gargantuan"] },
        "type": { "type": "string", "description": "e.g. undead, fiend (swarm of Tiny beasts)" },
        "ac": { "type": "integer" },
        "hp": { "type": "integer", "description": "Average hit points" },
        "hp_formula": { "type": "string", "description": "e.g. 4d8 + 8. Giving hp without it drops the old formula" },
        "speed": { "type": "object", "description": "Feet per mode; 0 removes it", "properties": {
            "walk": { "type": "integer" }, "fly": { "type": "integer" }, "swim": { "type": "integer" },
            "climb": { "type": "integer" }, "burrow": { "type": "integer" } } },
        "abilities": { "type": "object", "properties": {
            "str": { "type": "integer" }, "dex": { "type": "integer" }, "con": { "type": "integer" },
            "int": { "type": "integer" }, "wis": { "type": "integer" }, "cha": { "type": "integer" } } },
        "cr": cr.clone(),
        "traits": features.clone(), "actions": features.clone(), "reactions": features.clone(), "legendary": features,
    });
    let mut custom_create = custom_fields.clone();
    custom_create["name"] = json!({ "type": "string" });
    custom_create["based_on"] = json!({ "type": "string", "description": "A bestiary monster to start from" });
    custom_create["based_on_source"] = json!({ "type": "string", "description": "Its source book, when it's in several" });
    let mut custom_update = custom_fields;
    custom_update["monster"] = json!({ "type": "string", "description": "The custom creature's name or id" });
    custom_update["name"] = json!({ "type": "string", "description": "A new name" });
    vec![
        tool("search_monsters",
            "Search the bestiary (and this campaign's custom creatures). Returns name, source book, CR, XP, type, size, AC and HP. Sorted by CR.",
            json!({
                "query": { "type": "string", "description": "Part of the name" },
                "cr_min": cr, "cr_max": cr,
                "type": { "type": "string", "description": "e.g. undead, fiend, humanoid" },
                "size": { "type": "string", "enum": ["Tiny", "Small", "Medium", "Large", "Huge", "Gargantuan"] },
                "environment": { "type": "string", "description": "e.g. underdark, swamp, urban" },
                "only_2024": { "type": "boolean", "description": "Only the 2024 books" },
                "limit": { "type": "integer", "minimum": 1, "maximum": SEARCH_LIMIT },
            }), &[]),
        tool("get_monster", "A monster's full stat block as text.",
            json!({ "name": { "type": "string" }, "source": { "type": "string", "description": "Source book code, e.g. XMM or MM" } }),
            &["name"]),
        tool("create_encounter", "Add an encounter to a room, with its monsters.", encounter_fields, &["room", "name"]),
        tool("update_encounter",
            "Change an encounter: rename, move to another room, replace its monsters, notes, wandering or hazard. Only the fields given change.",
            update_fields, &["encounter"]),
        tool("delete_encounter", "Remove an encounter (and its tokens on the map).",
            json!({ "encounter": encounter.clone() }), &["encounter"]),
        tool("create_custom_monster",
            "Make a custom creature for this campaign, from scratch or as a copy of a bestiary monster (based_on) with changes. It can then be used in encounters by name.",
            custom_create, &["name"]),
        tool("update_custom_monster", "Change one of the campaign's custom creatures. Only the fields given change; feature lists given replace the old ones.",
            custom_update, &["monster"]),
        tool("merge_monsters",
            "Blend two monsters into a new custom creature, as the Monster Workshop does (higher numbers win; feature lists are combined).",
            json!({
                "a": { "type": "string" }, "a_source": { "type": "string" },
                "b": { "type": "string" }, "b_source": { "type": "string" },
                "name": { "type": "string", "description": "Name for the result" },
            }), &["a", "b"]),
        tool("simulate_encounter",
            "Fight the party against an encounter many times (the Encounters view's Monte Carlo simulator) and report how often each side wins. A rough guide: everyone makes their best attack at a random enemy; no spells or tactics.",
            json!({ "encounter": encounter, "runs": { "type": "integer", "minimum": 10, "maximum": 5000 } }),
            &["encounter"]),
    ]
}

/// Run an encounter or monster tool; `None` if the tool isn't one of these.
pub fn run(ctx: &mut ToolCtx, tool: &str, args: &Value) -> Option<Result<Value, String>> {
    Some(match tool {
        "search_monsters" => search_monsters(ctx, args),
        "get_monster" => get_monster(ctx, args),
        "create_encounter" => create_encounter(ctx, args),
        "update_encounter" => update_encounter(ctx, args),
        "delete_encounter" => delete_encounter(ctx, args),
        "create_custom_monster" => create_custom_monster(ctx, args),
        "update_custom_monster" => update_custom_monster(ctx, args),
        "merge_monsters" => merge_monsters(ctx, args),
        "simulate_encounter" => simulate_encounter(ctx, args),
        _ => return None,
    })
}

// ------------------------------------------------------------------ monsters

fn bestiary_ready(ctx: &ToolCtx) -> Result<(), String> {
    if ctx.bestiary_loading {
        return Err("The bestiary is still loading; try again in a few seconds.".into());
    }
    Ok(())
}

fn cr_arg(args: &Value, key: &str) -> Result<Option<f32>, String> {
    match args.get(key) {
        Some(Value::String(s)) => Ok(Some(cr_to_numeric(s.trim()))),
        _ => opt_f32(args, key),
    }
}

/// A monster by name: a custom creature first, then the bestiary. When the name is in
/// several books, `source` picks one, else the 2024 books win, then the 2014 Monster
/// Manual. Also returns the other books it's in.
fn find_monster<'a>(
    ctx: &'a ToolCtx,
    name: &str,
    source: Option<&str>,
) -> Result<(MonsterRef, &'a Monster, Vec<String>), String> {
    let name = name.trim();
    if source.is_none_or(|s| s.eq_ignore_ascii_case("custom")) {
        if let Some(c) = ctx.dungeon.custom_monsters.iter().find(|c| c.monster.name.eq_ignore_ascii_case(name)) {
            return Ok((MonsterRef::Custom { id: c.id.clone() }, &c.monster, Vec::new()));
        }
    }
    find_in_bestiary(ctx, name, source)
}

/// A bestiary monster by name, as [`find_monster`] picks it (custom creatures aside).
fn find_in_bestiary<'a>(
    ctx: &'a ToolCtx,
    name: &str,
    source: Option<&str>,
) -> Result<(MonsterRef, &'a Monster, Vec<String>), String> {
    let name = name.trim();
    bestiary_ready(ctx)?;
    let mut found: Vec<&Monster> = ctx.monsters.all().iter()
        .filter(|m| m.name.eq_ignore_ascii_case(name))
        .filter(|m| source.is_none_or(|s| m.source.eq_ignore_ascii_case(s)))
        .collect();
    let rank = |m: &Monster| {
        if SOURCES_2024.contains(&m.source.as_str()) { 0 } else if m.source == "MM" { 1 } else { 2 }
    };
    found.sort_by_key(|m| rank(m));
    let Some(best) = found.first() else {
        // Rank by how many of the name's words each monster shares.
        let words: Vec<String> = name.to_lowercase().split_whitespace().filter(|w| w.len() >= 3).map(String::from).collect();
        let mut close: Vec<(usize, &Monster)> = ctx.monsters.all().iter()
            .map(|m| {
                let lower = m.name.to_lowercase();
                (words.iter().filter(|w| lower.contains(w.as_str())).count(), m)
            })
            .filter(|(shared, _)| *shared > 0)
            .collect();
        close.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.name.cmp(&b.1.name)));
        let close: Vec<String> = close.iter().take(8).map(|(_, m)| format!("{} ({})", m.name, m.source)).collect();
        return Err(if close.is_empty() {
            format!("No monster called '{}'. Try search_monsters.", name)
        } else {
            format!("No monster called exactly '{}'. Close: {}", name, close.join(", "))
        });
    };
    let others = found[1..].iter().map(|m| m.source.clone()).collect();
    Ok((MonsterRef::Base { source: best.source.clone(), name: best.name.clone() }, best, others))
}

/// The monster a reference points at, if it still exists.
fn monster_for<'a>(ctx: &'a ToolCtx, mref: &MonsterRef) -> Option<&'a Monster> {
    match mref {
        MonsterRef::Base { source, name } => ctx.monsters.find(source, name),
        MonsterRef::Custom { id } | MonsterRef::Merged { id } => {
            ctx.dungeon.custom_monsters.iter().find(|c| c.id == *id).map(|c| &c.monster)
        }
    }
}

fn monster_brief(m: &Monster, source: &str) -> Value {
    json!({
        "name": m.name,
        "source": source,
        "cr": m.cr.cr_string(),
        "xp": m.cr.xp(),
        "type": m.monster_type.display(),
        "size": m.size.iter().map(|s| size_label(s)).collect::<Vec<_>>().join("/"),
        "ac": m.ac.first().and_then(|a| a.value()),
        "hp": m.hp.display(),
    })
}

fn search_monsters(ctx: &mut ToolCtx, args: &Value) -> Result<Value, String> {
    bestiary_ready(ctx)?;
    let query = opt_str(args, "query").unwrap_or("").trim().to_lowercase();
    let (cr_min, cr_max) = (cr_arg(args, "cr_min")?, cr_arg(args, "cr_max")?);
    let kind = opt_str(args, "type").map(str::to_lowercase);
    let size = opt_str(args, "size").map(|s| s.chars().next().unwrap_or('M').to_ascii_uppercase().to_string());
    let environment = opt_str(args, "environment").map(str::to_lowercase);
    let only_2024 = opt_bool(args, "only_2024")?.unwrap_or(false);
    let limit = args.get("limit").and_then(Value::as_u64).map_or(20, |n| (n as usize).clamp(1, SEARCH_LIMIT));

    let matches = |m: &Monster, source: &str| {
        let cr = m.cr.cr_numeric();
        (query.is_empty() || m.name.to_lowercase().contains(&query))
            && cr_min.is_none_or(|c| cr >= c)
            && cr_max.is_none_or(|c| cr <= c)
            && kind.as_ref().is_none_or(|k| m.monster_type.display().to_lowercase().contains(k))
            && size.as_ref().is_none_or(|s| m.size.contains(s))
            && environment.as_ref().is_none_or(|e| m.environment.iter().any(|x| x.to_lowercase().contains(e)))
            && (!only_2024 || SOURCES_2024.contains(&source))
    };
    let mut hits: Vec<(&Monster, String)> = ctx.dungeon.custom_monsters.iter()
        .map(|c: &CustomMonster| (&c.monster, "custom".to_string()))
        .chain(ctx.monsters.all().iter().map(|m| (m, m.source.clone())))
        .filter(|(m, source)| matches(m, source))
        .collect();
    hits.sort_by(|a, b| a.0.cr.cr_numeric().total_cmp(&b.0.cr.cr_numeric()).then(a.0.name.cmp(&b.0.name)));
    Ok(json!({
        "total": hits.len(),
        "showing": hits.len().min(limit),
        "monsters": hits.iter().take(limit).map(|(m, s)| monster_brief(m, s)).collect::<Vec<_>>(),
    }))
}

fn features(title: &str, list: &[Feature]) -> String {
    if list.is_empty() {
        return String::new();
    }
    let mut out = format!("\n{}:\n", title);
    for f in list {
        out.push_str(&format!("- {}. {}\n", strip_5e_markup(&f.name), f.entries_text().replace('\n', " ")));
    }
    out
}

fn spellcasting_text(entries: &[Value]) -> String {
    let mut out = String::new();
    for sc in entries {
        let name = sc.get("name").and_then(Value::as_str).unwrap_or("Spellcasting");
        let header: Vec<String> = sc.get("headerEntries").and_then(Value::as_array)
            .map(|a| a.iter().map(entry_to_text).collect())
            .unwrap_or_default();
        out.push_str(&format!("- {}. {}\n", strip_5e_markup(name), header.join(" ")));
        let spells = |v: &Value| v.as_array().map(|a| a.iter().map(entry_to_text).collect::<Vec<_>>().join(", ")).unwrap_or_default();
        if let Some(will) = sc.get("will") {
            out.push_str(&format!("  At will: {}\n", spells(will)));
        }
        if let Some(daily) = sc.get("daily").and_then(Value::as_object) {
            for (k, v) in daily {
                let per_day = k.trim_end_matches('e');
                out.push_str(&format!("  {}/day{}: {}\n", per_day, if k.ends_with('e') { " each" } else { "" }, spells(v)));
            }
        }
        if let Some(levels) = sc.get("spells").and_then(Value::as_object) {
            for (level, v) in levels {
                let label = if level == "0" { "Cantrips".to_string() } else { format!("Level {}", level) };
                out.push_str(&format!("  {}: {}\n", label, spells(v.get("spells").unwrap_or(&Value::Null))));
            }
        }
    }
    out
}

fn stat_block(m: &Monster, source: &str) -> String {
    let joined = |map: &std::collections::HashMap<String, String>| {
        let mut v: Vec<String> = map.iter().map(|(k, v)| format!("{} {}", k, v)).collect();
        v.sort();
        v.join(", ")
    };
    let ab = |name: &str, score: u8| format!("{} {} ({})", name, score, Monster::modifier_str(score));
    let mut s = format!(
        "{} ({})\n{} {}, {}\nAC {}, HP {}, Speed {}\n{}  {}  {}  {}  {}  {}\n",
        m.name, source,
        m.size.iter().map(|s| size_label(s)).collect::<Vec<_>>().join("/"),
        m.monster_type.display(),
        alignment_display(&m.alignment),
        m.ac.iter().map(|a| a.display()).collect::<Vec<_>>().join(", "),
        m.hp.display(),
        m.speed.display(),
        ab("STR", m.str_score), ab("DEX", m.dex_score), ab("CON", m.con_score),
        ab("INT", m.int_score), ab("WIS", m.wis_score), ab("CHA", m.cha_score),
    );
    let mut line = |label: &str, text: String| {
        if !text.is_empty() {
            s.push_str(&format!("{}: {}\n", label, text));
        }
    };
    line("Saves", joined(&m.save));
    line("Skills", joined(&m.skill));
    line("Vulnerabilities", damage_list_display(&m.vulnerable));
    line("Resistances", damage_list_display(&m.resist));
    line("Immunities", damage_list_display(&m.immune));
    line("Condition immunities", damage_list_display(&m.condition_immune));
    let mut senses = m.senses.join(", ");
    if let Some(p) = m.passive {
        senses = if senses.is_empty() { format!("passive Perception {}", p) } else { format!("{}; passive Perception {}", senses, p) };
    }
    line("Senses", senses);
    line("Languages", m.languages.join(", "));
    line("Challenge", format!("{} ({} XP)", m.cr.cr_string(), m.cr.xp()));
    line("Environments", m.environment.join(", "));
    s.push_str(&features("Traits", &m.traits));
    if !m.spellcasting.is_empty() {
        s.push_str("\nSpellcasting:\n");
        s.push_str(&spellcasting_text(&m.spellcasting));
    }
    s.push_str(&features("Actions", &m.action));
    s.push_str(&features("Reactions", &m.reaction));
    s.push_str(&features("Legendary actions", &m.legendary));
    s.push_str(&features("Mythic actions", &m.mythic));
    s
}

fn get_monster(ctx: &mut ToolCtx, args: &Value) -> Result<Value, String> {
    let (mref, m, others) = find_monster(ctx, req_str(args, "name")?, opt_str(args, "source"))?;
    let source = match &mref {
        MonsterRef::Base { source, .. } => source.clone(),
        _ => "custom".to_string(),
    };
    let mut v = json!({ "stat_block": stat_block(m, &source) });
    if !others.is_empty() {
        v["also_in"] = json!(others);
    }
    Ok(v)
}

// ------------------------------------------------------------------ encounters

/// An encounter by id, or by its name when exactly one has it. Returns its index.
fn resolve_encounter(ctx: &ToolCtx, e: &str) -> Result<usize, String> {
    let list = &ctx.dungeon.encounters;
    if let Some(i) = list.iter().position(|x| x.id == e) {
        return Ok(i);
    }
    let matches: Vec<usize> = (0..list.len()).filter(|i| list[*i].name.eq_ignore_ascii_case(e.trim())).collect();
    match matches.as_slice() {
        [i] => Ok(*i),
        [] => Err(format!("No encounter '{}' on this map. get_overview lists them.", e)),
        _ => Err(format!("Several encounters are called '{}'; use its id from get_room or get_overview.", e)),
    }
}

/// A one-line monster list: "3× Goblin, 1× Goblin Boss".
pub fn monster_line(ctx: &ToolCtx, e: &Encounter) -> String {
    e.monsters.iter()
        .map(|em| format!("{}× {}", em.count, monster_for(ctx, &em.monster_ref).map_or("(missing)", |m| m.name.as_str())))
        .collect::<Vec<_>>()
        .join(", ")
}

pub fn total_xp(ctx: &ToolCtx, e: &Encounter) -> u32 {
    e.monsters.iter().map(|em| monster_for(ctx, &em.monster_ref).map_or(0, |m| m.cr.xp()) * em.count).sum()
}

pub fn encounter_summary(ctx: &ToolCtx, e: &Encounter) -> Value {
    let monsters: Vec<Value> = e.monsters.iter().map(|em| {
        let (source, name) = match &em.monster_ref {
            MonsterRef::Base { source, name } => (source.as_str(), name.clone()),
            MonsterRef::Custom { .. } | MonsterRef::Merged { .. } => {
                ("custom", monster_for(ctx, &em.monster_ref).map_or("(missing)".to_string(), |m| m.name.clone()))
            }
        };
        let m = monster_for(ctx, &em.monster_ref);
        let mut v = json!({
            "name": name,
            "source": source,
            "count": em.count,
            "cr": m.map(|m| m.cr.cr_string().to_string()),
            "xp_each": m.map_or(0, |m| m.cr.xp()),
        });
        if !em.notes.is_empty() {
            v["notes"] = json!(em.notes);
        }
        v
    }).collect();
    let mut v = json!({
        "id": e.id,
        "name": e.name,
        "room": label_of(ctx.dungeon, &e.home_room_id),
        "monsters": monsters,
        "total_xp": total_xp(ctx, e),
    });
    match e.encounter_type {
        EncounterType::Static => {}
        EncounterType::Wandering(range) => v["wandering"] = json!({ "range": range }),
    }
    if !e.notes.is_empty() {
        v["notes"] = json!(e.notes);
    }
    if let Some(h) = &e.hazard {
        v["hazard"] = serde_json::to_value(h).unwrap_or_default();
    }
    v
}

fn parse_monsters(ctx: &ToolCtx, list: &Value) -> Result<Vec<EncounterMonster>, String> {
    let list = list.as_array().ok_or("'monsters' must be a list")?;
    list.iter().map(|item| {
        let name = req_str(item, "name")?;
        let (monster_ref, ..) = find_monster(ctx, name, opt_str(item, "source"))?;
        let count = match item.get("count") {
            None | Some(Value::Null) => 1,
            Some(v) => v.as_u64().filter(|n| (1..=100).contains(n)).ok_or("'count' must be from 1 to 100")? as u32,
        };
        Ok(EncounterMonster { monster_ref, count, notes: opt_str(item, "notes").unwrap_or("").to_string() })
    }).collect()
}

fn parse_hazard(v: &Value) -> Result<Option<Hazard>, String> {
    if v.is_null() {
        return Ok(None);
    }
    let save_ability = match opt_str(v, "save_ability") {
        Some(s) => SaveAbility::ALL.iter()
            .find(|a| a.label().eq_ignore_ascii_case(s.trim()))
            .cloned()
            .ok_or_else(|| format!("Unknown save ability '{}'", s))?,
        None => SaveAbility::default(),
    };
    let save_dc = match v.get("save_dc") {
        None | Some(Value::Null) => None,
        Some(n) => Some(n.as_u64().filter(|n| *n <= 40).ok_or("'save_dc' must be a number up to 40")? as u8),
    };
    Ok(Some(Hazard {
        damage: opt_str(v, "damage").unwrap_or("").trim().to_string(),
        save_dc,
        save_ability,
        condition: opt_str(v, "condition").map(|c| c.trim().to_string()).filter(|c| !c.is_empty()),
    }))
}

/// Apply the fields shared by create and update to `e`.
fn apply_encounter_fields(ctx: &ToolCtx, e: &mut Encounter, args: &Value) -> Result<(), String> {
    if let Some(name) = opt_str(args, "name").map(str::trim).filter(|n| !n.is_empty()) {
        e.name = name.to_string();
    }
    if let Some(r) = opt_str(args, "room") {
        e.home_room_id = resolve_room(ctx.dungeon, r)?;
    }
    if let Some(list) = args.get("monsters").filter(|v| !v.is_null()) {
        e.monsters = parse_monsters(ctx, list)?;
    }
    if let Some(notes) = opt_str(args, "notes") {
        e.notes = notes.to_string();
    }
    let range = match args.get("wander_range") {
        Some(Value::Number(n)) => Some(n.as_u64().ok_or("'wander_range' must be a whole number")? as u32),
        _ => None,
    };
    match opt_bool(args, "wandering")? {
        Some(true) => e.encounter_type = EncounterType::Wandering(range),
        Some(false) => e.encounter_type = EncounterType::Static,
        None if args.get("wander_range").is_some() => {
            if let EncounterType::Wandering(_) = e.encounter_type {
                e.encounter_type = EncounterType::Wandering(range);
            }
        }
        None => {}
    }
    if let Some(h) = args.get("hazard") {
        e.hazard = parse_hazard(h)?;
    }
    Ok(())
}

/// Drop map tokens for monsters that no longer exist.
fn prune_tokens(ctx: &mut ToolCtx) {
    let mut tokens = std::mem::take(&mut ctx.dungeon.tokens);
    crate::presentation::tokens::prune_stale(&mut tokens, ctx.dungeon);
    ctx.dungeon.tokens = tokens;
}

fn create_encounter(ctx: &mut ToolCtx, args: &Value) -> Result<Value, String> {
    let room = resolve_room(ctx.dungeon, req_str(args, "room")?)?;
    let name = req_str(args, "name")?.trim();
    if name.is_empty() {
        return Err("An encounter needs a name".into());
    }
    let mut e = Encounter::new(name.to_string(), room);
    apply_encounter_fields(ctx, &mut e, args)?;
    let summary = encounter_summary(ctx, &e);
    ctx.dungeon.encounters.push(e);
    Ok(summary)
}

fn update_encounter(ctx: &mut ToolCtx, args: &Value) -> Result<Value, String> {
    let i = resolve_encounter(ctx, req_str(args, "encounter")?)?;
    let mut e = ctx.dungeon.encounters[i].clone();
    apply_encounter_fields(ctx, &mut e, args)?;
    ctx.dungeon.encounters[i] = e;
    prune_tokens(ctx);
    Ok(encounter_summary(ctx, &ctx.dungeon.encounters[i]))
}

fn delete_encounter(ctx: &mut ToolCtx, args: &Value) -> Result<Value, String> {
    let i = resolve_encounter(ctx, req_str(args, "encounter")?)?;
    let e = ctx.dungeon.encounters.remove(i);
    crate::presentation::tokens::remove_encounter_tokens(&mut ctx.dungeon.tokens, &e.id);
    Ok(json!({ "deleted": e.name, "room": label_of(ctx.dungeon, &e.home_room_id) }))
}

// ------------------------------------------------------------------ custom creatures

fn resolve_custom(ctx: &ToolCtx, m: &str) -> Result<usize, String> {
    let list = &ctx.dungeon.custom_monsters;
    list.iter().position(|c| c.id == m)
        .or_else(|| list.iter().position(|c| c.monster.name.eq_ignore_ascii_case(m.trim())))
        .ok_or_else(|| format!("No custom creature '{}' in this campaign.", m))
}

fn features_arg(v: &Value, key: &str) -> Result<Option<Vec<crate::model::monster::Feature>>, String> {
    let Some(list) = v.get(key).filter(|x| !x.is_null()) else { return Ok(None) };
    let list = list.as_array().ok_or(format!("'{}' must be a list of {{name, text}}", key))?;
    list.iter().map(|f| {
        Ok(Feature { name: req_str(f, "name")?.to_string(), entries: vec![json!(req_str(f, "text")?)] })
    }).collect::<Result<Vec<_>, String>>().map(Some)
}

/// Apply stat-block fields onto `m`.
fn apply_monster_fields(m: &mut Monster, args: &Value) -> Result<(), String> {
    if let Some(n) = opt_str(args, "name").map(str::trim).filter(|n| !n.is_empty()) {
        m.name = n.to_string();
    }
    if let Some(size) = opt_str(args, "size") {
        let code = ["T", "S", "M", "L", "H", "G"].into_iter()
            .find(|c| size_label(c).eq_ignore_ascii_case(size.trim()) || c.eq_ignore_ascii_case(size.trim()))
            .ok_or_else(|| format!("Unknown size '{}'", size))?;
        m.size = vec![code.to_string()];
    }
    if let Some(t) = opt_str(args, "type") {
        m.monster_type = MonsterType::Simple(t.trim().to_string());
    }
    if let Some(ac) = args.get("ac").filter(|v| !v.is_null()) {
        let ac = ac.as_u64().filter(|n| *n <= 40).ok_or("'ac' must be a number up to 40")?;
        m.ac = vec![ArmorClass::Simple(ac as u8)];
    }
    let hp = args.get("hp").filter(|v| !v.is_null()).map(|v| v.as_i64().ok_or("'hp' must be a number")).transpose()?;
    let formula = opt_str(args, "hp_formula");
    if hp.is_some() || formula.is_some() {
        let (old_avg, old_formula) = match &m.hp {
            HitPoints::Formula { average, formula } => (*average, formula.clone()),
            _ => (1, String::new()),
        };
        // New hit points without new dice would leave a formula that no longer adds up.
        let formula = match (formula, hp) {
            (Some(f), _) => f.trim().to_string(),
            (None, Some(_)) => String::new(),
            (None, None) => old_formula,
        };
        m.hp = HitPoints::Formula { average: hp.map_or(old_avg, |h| h.max(1) as i32), formula };
    }
    if let Some(speed) = args.get("speed").and_then(Value::as_object) {
        for (mode, v) in speed {
            let feet = v.as_u64().ok_or("speeds must be whole numbers of feet")? as u32;
            let value = if feet == 0 { SpeedValue::None } else { SpeedValue::Simple(feet) };
            match mode.as_str() {
                "walk" => m.speed.walk = value,
                "fly" => m.speed.fly = value,
                "swim" => m.speed.swim = value,
                "climb" => m.speed.climb = value,
                "burrow" => m.speed.burrow = value,
                other => return Err(format!("Unknown speed '{}'", other)),
            }
        }
    }
    if let Some(ab) = args.get("abilities").and_then(Value::as_object) {
        for (name, v) in ab {
            let score = v.as_u64().filter(|n| (1..=30).contains(n)).ok_or("ability scores must be 1 to 30")? as u8;
            match name.to_lowercase().as_str() {
                "str" => m.str_score = score,
                "dex" => m.dex_score = score,
                "con" => m.con_score = score,
                "int" => m.int_score = score,
                "wis" => m.wis_score = score,
                "cha" => m.cha_score = score,
                other => return Err(format!("Unknown ability '{}'", other)),
            }
        }
    }
    match args.get("cr") {
        Some(Value::String(c)) => m.cr = ChallengeRating::Simple(c.trim().to_string()),
        Some(Value::Number(n)) => m.cr = ChallengeRating::Simple(n.to_string()),
        _ => {}
    }
    if let Some(f) = features_arg(args, "traits")? { m.traits = f; }
    if let Some(f) = features_arg(args, "actions")? { m.action = f; }
    if let Some(f) = features_arg(args, "reactions")? { m.reaction = f; }
    if let Some(f) = features_arg(args, "legendary")? { m.legendary = f; }
    Ok(())
}

fn create_custom_monster(ctx: &mut ToolCtx, args: &Value) -> Result<Value, String> {
    let name = req_str(args, "name")?.trim();
    if name.is_empty() {
        return Err("A creature needs a name".into());
    }
    let (mut monster, based_on) = match opt_str(args, "based_on") {
        Some(base) => {
            let (_, m, _) = find_in_bestiary(ctx, base, opt_str(args, "based_on_source"))?;
            (m.clone(), Some((m.source.clone(), m.name.clone())))
        }
        None => {
            let blank = json!({ "name": name, "source": "custom", "size": ["M"], "type": "humanoid", "cr": "0",
                "str": 10, "dex": 10, "con": 10, "int": 10, "wis": 10, "cha": 10,
                "ac": [10], "hp": { "average": 1, "formula": "1d8" }, "speed": { "walk": 30 } });
            (serde_json::from_value(blank).map_err(|e| e.to_string())?, None)
        }
    };
    apply_monster_fields(&mut monster, args)?;
    monster.name = name.to_string();
    let block = stat_block(&monster, "custom");
    ctx.dungeon.custom_monsters.push(CustomMonster { id: uuid::Uuid::new_v4().to_string(), based_on, monster });
    Ok(json!({ "stat_block": block }))
}

fn update_custom_monster(ctx: &mut ToolCtx, args: &Value) -> Result<Value, String> {
    let i = resolve_custom(ctx, req_str(args, "monster")?)?;
    let mut monster = ctx.dungeon.custom_monsters[i].monster.clone();
    apply_monster_fields(&mut monster, args)?;
    let block = stat_block(&monster, "custom");
    let c = &mut ctx.dungeon.custom_monsters[i];
    c.monster = monster;
    ctx.combat_stats.invalidate_custom(&c.id);
    Ok(json!({ "stat_block": block }))
}

fn merge_monsters(ctx: &mut ToolCtx, args: &Value) -> Result<Value, String> {
    let (_, a, _) = find_monster(ctx, req_str(args, "a")?, opt_str(args, "a_source"))?;
    let (_, b, _) = find_monster(ctx, req_str(args, "b")?, opt_str(args, "b_source"))?;
    let mut merged = crate::model::merge_monsters(a, b, &MergeConfig::default());
    if let Some(name) = opt_str(args, "name").map(str::trim).filter(|n| !n.is_empty()) {
        merged.name = name.to_string();
    }
    let block = stat_block(&merged, "custom");
    ctx.dungeon.custom_monsters.push(CustomMonster { id: uuid::Uuid::new_v4().to_string(), based_on: None, monster: merged });
    Ok(json!({ "stat_block": block }))
}

fn simulate_encounter(ctx: &mut ToolCtx, args: &Value) -> Result<Value, String> {
    use crate::presentation::combat_sim;
    let i = resolve_encounter(ctx, req_str(args, "encounter")?)?;
    if ctx.dungeon.party.is_empty() {
        return Err("The campaign has no party yet (the user sets one up when presenting), so there's no one to fight.".into());
    }
    let runs = args.get("runs").and_then(Value::as_u64).map_or(1000, |n| n.clamp(10, 5000)) as u32;
    let encounter = ctx.dungeon.encounters[i].clone();
    let party = combat_sim::build_combatants_from_party(&ctx.dungeon.party, 0);
    let monsters = combat_sim::build_combatants_from_encounter(
        &encounter, ctx.monsters, &ctx.dungeon.custom_monsters, ctx.combat_stats, 1, None,
    );
    if monsters.is_empty() {
        return Err("That encounter has no monsters to fight.".into());
    }
    let r = combat_sim::run_monte_carlo(&party, &monsters, runs, "Party".into(), encounter.name.clone());
    let pct = |n: u32| (n as f32 * 1000.0 / r.num_sims as f32).round() / 10.0;
    Ok(json!({
        "encounter": encounter.name,
        "runs": r.num_sims,
        "party_wins_percent": pct(r.side_a_wins),
        "monsters_win_percent": pct(r.side_b_wins),
        "draws_percent": pct(r.draws),
        "average_rounds": (r.avg_rounds * 10.0).round() / 10.0,
    }))
}
