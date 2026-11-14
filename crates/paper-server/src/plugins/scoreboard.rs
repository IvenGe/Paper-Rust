//! The `scoreboard.*` API calls: the plugin host's `Scoreboard`, `Objective`, `Score` and
//! `Team` objects are handles; the boards live in [`crate::server::scoreboard`].

use super::{component_from, component_value};
use crate::server::scoreboard::{self, Objective, Team};
use crate::server::Server;
use anyhow::{anyhow, bail, Result};
use paper_bridge::{bmap, Value};

fn board(server: &Server, args: &Value) -> Result<u64> {
    let id = args.i64("board").ok_or_else(|| anyhow!("missing board"))? as u64;
    if !server.scoreboards.exists(id) {
        bail!("unknown scoreboard {id}");
    }
    Ok(id)
}

fn text<'a>(args: &'a Value, key: &str) -> Result<&'a str> {
    args.str(key).ok_or_else(|| anyhow!("missing {key}"))
}

fn objective_value(server: &Server, board: u64, o: &Objective) -> Value {
    let slots = server.scoreboards.get(board).map(|b| b.display.iter().filter(|(_, n)| **n == o.name).map(|(s, _)| Value::from(*s as i64)).collect()).unwrap_or_default();
    bmap! {
        "name" => o.name.clone(),
        "criteria" => o.criteria.clone(),
        "display_name" => component_value(&o.display_name),
        "render_type" => o.render_type,
        "display_slots" => Value::List(slots),
    }
}

fn team_value(t: &Team) -> Value {
    bmap! {
        "name" => t.name.clone(),
        "display_name" => component_value(&t.display_name),
        "prefix" => component_value(&t.prefix),
        "suffix" => component_value(&t.suffix),
        "color" => t.color,
        "friendly_fire" => t.friendly_fire,
        "see_friendly_invisibles" => t.see_friendly_invisibles,
        "name_tag_visibility" => t.name_tag_visibility,
        "death_message_visibility" => t.death_message_visibility,
        "collision_rule" => t.collision_rule,
        "entries" => Value::List(t.entries.iter().map(|e| Value::Str(e.clone())).collect()),
    }
}

fn strings(args: &Value, key: &str) -> Vec<String> {
    args.list(key).unwrap_or(&[]).iter().filter_map(|v| v.as_str().map(str::to_owned)).collect()
}

/// Executes a `scoreboard.*` API call.
pub fn handle(server: &mut Server, method: &str, args: &Value) -> Result<Value> {
    match method {
        "scoreboard.new" => Ok(bmap! { "board" => server.scoreboards.create() as i64 }),
        "scoreboard.release" => {
            if let Some(id) = args.i64("board") {
                server.scoreboards.release(id as u64);
            }
            Ok(Value::Null)
        }
        "scoreboard.retain" => {
            let b = board(server, args)?;
            server.scoreboards.retain(b);
            Ok(Value::Null)
        }
        "scoreboard.objectives" => {
            let b = board(server, args)?;
            let list = server.scoreboards.get(b).map(|board| board.objectives.values().map(|o| objective_value(server, b, o)).collect()).unwrap_or_default();
            Ok(Value::List(list))
        }
        "scoreboard.objective" => {
            let b = board(server, args)?;
            let name = text(args, "name")?;
            Ok(server.scoreboards.get(b).and_then(|board| board.objectives.get(name)).map_or(Value::Null, |o| objective_value(server, b, o)))
        }
        "scoreboard.register_objective" => {
            let b = board(server, args)?;
            let name = text(args, "name")?.to_owned();
            let criteria = args.str("criteria").unwrap_or("dummy").to_owned();
            let display = component_from(args.get("display_name")).unwrap_or_else(|| paper_protocol::text::Component::text(name.clone()));
            let render = args.i32("render_type").unwrap_or_else(|| scoreboard::default_render_type(&criteria));
            server.register_objective(b, &name, &criteria, display, render)?;
            Ok(Value::Null)
        }
        "scoreboard.unregister_objective" => {
            let b = board(server, args)?;
            server.unregister_objective(b, text(args, "name")?)?;
            Ok(Value::Null)
        }
        "scoreboard.update_objective" => {
            let b = board(server, args)?;
            server.update_objective(b, text(args, "name")?, component_from(args.get("display_name")), args.i32("render_type"))?;
            Ok(Value::Null)
        }
        "scoreboard.set_display" => {
            let b = board(server, args)?;
            let slot = args.i32("slot").ok_or_else(|| anyhow!("missing slot"))?;
            server.set_display_slot(b, slot, args.str("objective"))?;
            Ok(Value::Null)
        }
        "scoreboard.score" => {
            let b = board(server, args)?;
            let (objective, entry) = (text(args, "objective")?, text(args, "entry")?);
            let board = server.scoreboards.get(b).unwrap();
            if !board.objectives.contains_key(objective) {
                bail!("Unregistered objective {objective}");
            }
            Ok(match board.scores.get(entry).and_then(|s| s.get(objective)) {
                Some(score) => bmap! { "set" => true, "value" => score.value, "display_name" => score.display_name.as_ref().map(component_value), "locked" => score.locked },
                None => bmap! { "set" => false, "value" => 0 },
            })
        }
        "scoreboard.set_score" => {
            let b = board(server, args)?;
            let (objective, entry) = (text(args, "objective")?.to_owned(), text(args, "entry")?.to_owned());
            let criteria = server.scoreboards.get(b).and_then(|board| board.objectives.get(&objective)).map(|o| o.criteria.clone()).unwrap_or_default();
            if scoreboard::criteria_read_only(&criteria) && args.get("value").as_i64().is_some() && args.bool("force") != Some(true) {
                bail!("Cannot modify read-only score");
            }
            // `Score.setTriggerable`.
            if let Some(locked) = args.bool("locked") {
                server.set_score_locked(b, &objective, &entry, locked)?;
                return Ok(Value::Null);
            }
            let display = match args.get("display_name") {
                Value::Null if args.bool("clear_display_name") == Some(true) => Some(None),
                Value::Null => None,
                v => Some(component_from(v)),
            };
            server.set_score(b, &objective, &entry, args.i32("value"), display)?;
            Ok(Value::Null)
        }
        "scoreboard.reset_scores" => {
            let b = board(server, args)?;
            server.reset_score(b, text(args, "entry")?, args.str("objective"))?;
            Ok(Value::Null)
        }
        "scoreboard.entries" => {
            let b = board(server, args)?;
            Ok(Value::List(server.scoreboards.get(b).unwrap().scores.keys().map(|e| Value::Str(e.clone())).collect()))
        }
        "scoreboard.entry_scores" => {
            let b = board(server, args)?;
            let entry = text(args, "entry")?;
            let scores = server.scoreboards.get(b).unwrap().scores.get(entry).map(|s| s.iter().map(|(o, s)| bmap! { "objective" => o.clone(), "value" => s.value }).collect()).unwrap_or_default();
            Ok(Value::List(scores))
        }
        "scoreboard.teams" => {
            let b = board(server, args)?;
            Ok(Value::List(server.scoreboards.get(b).unwrap().teams.values().map(team_value).collect()))
        }
        "scoreboard.team" => {
            let b = board(server, args)?;
            let name = text(args, "name")?;
            Ok(server.scoreboards.get(b).unwrap().teams.get(name).map_or(Value::Null, team_value))
        }
        "scoreboard.entry_team" => {
            let b = board(server, args)?;
            let entry = text(args, "entry")?;
            Ok(server.scoreboards.get(b).unwrap().team_of(entry).map_or(Value::Null, team_value))
        }
        "scoreboard.register_team" => {
            let b = board(server, args)?;
            server.register_team(b, text(args, "name")?)?;
            Ok(Value::Null)
        }
        "scoreboard.unregister_team" => {
            let b = board(server, args)?;
            server.unregister_team(b, text(args, "name")?)?;
            Ok(Value::Null)
        }
        "scoreboard.update_team" => {
            let b = board(server, args)?;
            let name = text(args, "name")?.to_owned();
            let display = component_from(args.get("display_name"));
            let prefix = component_from(args.get("prefix"));
            let suffix = component_from(args.get("suffix"));
            let color = match args.get("color") {
                Value::Null if args.bool("clear_color") == Some(true) => Some(None),
                Value::Null => None,
                v => Some(v.as_i32().filter(|c| (0..16).contains(c))),
            };
            let (ff, invisibles) = (args.bool("friendly_fire"), args.bool("see_friendly_invisibles"));
            let (tags, deaths, collision) = (args.i32("name_tag_visibility"), args.i32("death_message_visibility"), args.i32("collision_rule"));
            server.update_team(b, &name, |t| {
                if let Some(d) = display {
                    t.display_name = d;
                }
                if let Some(p) = prefix {
                    t.prefix = p;
                }
                if let Some(s) = suffix {
                    t.suffix = s;
                }
                if let Some(c) = color {
                    t.color = c;
                }
                if let Some(f) = ff {
                    t.friendly_fire = f;
                }
                if let Some(i) = invisibles {
                    t.see_friendly_invisibles = i;
                }
                if let Some(v) = tags {
                    t.name_tag_visibility = v.clamp(0, 3);
                }
                if let Some(v) = deaths {
                    t.death_message_visibility = v.clamp(0, 3);
                }
                if let Some(v) = collision {
                    t.collision_rule = v.clamp(0, 3);
                }
            })?;
            Ok(Value::Null)
        }
        "scoreboard.add_entries" => {
            let b = board(server, args)?;
            server.add_team_entries(b, text(args, "team")?, &strings(args, "entries"))?;
            Ok(Value::Null)
        }
        "scoreboard.remove_entries" => {
            let b = board(server, args)?;
            let removed = server.remove_team_entries(b, text(args, "team")?, &strings(args, "entries"))?;
            Ok(Value::List(removed.into_iter().map(Value::Str).collect()))
        }
        other => bail!("unknown plugin API method {other}"),
    }
}
