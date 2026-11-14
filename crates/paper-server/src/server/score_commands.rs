//! `/scoreboard` and `/team` on the main scoreboard (vanilla's `ScoreboardCommand` and
//! `TeamCommand`): objectives, display slots, scores and teams, with vanilla's messages.

use super::commands::CommandSource;
use super::scoreboard::{criteria_read_only, default_render_type, DISPLAY_SLOTS, MAIN};
use super::Server;
use paper_protocol::text::Component;

/// Team colours as `/team modify ... color` names them (`ChatFormatting`), by colour id.
const COLORS: [&str; 16] = ["black", "dark_blue", "dark_green", "dark_aqua", "dark_red", "dark_purple", "gold", "gray", "dark_gray", "blue", "green", "aqua", "red", "light_purple", "yellow", "white"];
const VISIBILITY: [&str; 4] = ["always", "never", "hideForOtherTeams", "hideForOwnTeam"];
const COLLISION: [&str; 4] = ["always", "never", "pushOtherTeams", "pushOwnTeam"];

/// Criteria `/scoreboard objectives add` accepts (statistics are kept but not counted).
fn valid_criteria(c: &str) -> bool {
    matches!(c, "dummy" | "trigger" | "deathCount" | "playerKillCount" | "totalKillCount" | "health" | "xp" | "level" | "food" | "air" | "armor")
        || c.strip_prefix("teamkill.").or_else(|| c.strip_prefix("killedByTeam.")).is_some_and(|color| COLORS.contains(&color))
        || c.starts_with("minecraft.") && c.contains(':')
}

fn text(s: &str) -> Component {
    Component::text(s.to_owned())
}

/// A text component argument: JSON, or plain text.
fn component_arg(raw: &str) -> Component {
    Component::from_json_str(raw).unwrap_or_else(|_| Component::text(raw.trim_matches('"').to_owned()))
}

fn tr(key: &str, args: Vec<Component>) -> Component {
    Component::translatable(key, args)
}

impl Server {
    /// Score holders named by a target: online players a selector or name finds, else the
    /// name itself (scores can be kept for anyone, `#fake` holders included).
    fn score_holders(&self, source: &CommandSource, token: &str) -> Vec<String> {
        if token == "*" {
            return self.scoreboards.get(MAIN).map(|b| b.scores.keys().cloned().collect()).unwrap_or_default();
        }
        let players = self.resolve_players(source, token);
        if !players.is_empty() {
            return players.iter().filter_map(|id| self.players.get(id)).map(|p| p.name().to_owned()).collect();
        }
        if token.starts_with('@') {
            return Vec::new();
        }
        vec![token.to_owned()]
    }

    fn objective_name(&self, name: &str) -> Option<Component> {
        self.scoreboards.get(MAIN)?.objectives.get(name).map(|o| o.display_name.clone())
    }

    fn score_of(&self, holder: &str, objective: &str) -> Option<i32> {
        self.scoreboards.get(MAIN)?.scores.get(holder)?.get(objective).map(|s| s.value)
    }

    /// `/scoreboard objectives|players ...`.
    pub(super) fn scoreboard_command(&mut self, source: &CommandSource, args: &[&str]) {
        match args {
            ["objectives", "list"] => {
                let names: Vec<String> = self.scoreboards.get(MAIN).map(|b| b.objectives.keys().cloned().collect()).unwrap_or_default();
                if names.is_empty() {
                    return self.feedback(source, tr("commands.scoreboard.objectives.list.empty", vec![]));
                }
                self.feedback(source, tr("commands.scoreboard.objectives.list.success", vec![text(&names.len().to_string()), text(&names.join(", "))]));
            }
            ["objectives", "add", name, criteria, rest @ ..] => {
                if !valid_criteria(criteria) {
                    return self.fail_component(source, tr("argument.criteria.invalid", vec![text(criteria)]));
                }
                if self.objective_name(name).is_some() {
                    return self.fail_component(source, tr("commands.scoreboard.objectives.add.duplicate", vec![]));
                }
                let display = if rest.is_empty() { text(name) } else { component_arg(&rest.join(" ")) };
                match self.register_objective(MAIN, name, criteria, display.clone(), default_render_type(criteria)) {
                    Ok(()) => self.feedback(source, tr("commands.scoreboard.objectives.add.success", vec![display])),
                    Err(e) => self.fail(source, e.to_string()),
                }
            }
            ["objectives", "remove", name] => {
                let Some(display) = self.objective_name(name) else { return self.fail_component(source, tr("arguments.objective.notFound", vec![text(name)])) };
                let _ = self.unregister_objective(MAIN, name);
                self.feedback(source, tr("commands.scoreboard.objectives.remove.success", vec![display]));
            }
            ["objectives", "setdisplay", slot, rest @ ..] => {
                let Some(id) = DISPLAY_SLOTS.iter().position(|s| s == slot) else { return self.fail(source, format!("Unknown display slot '{slot}'")) };
                match rest.first() {
                    None => {
                        let _ = self.set_display_slot(MAIN, id as i32, None);
                        self.feedback(source, tr("commands.scoreboard.objectives.display.cleared", vec![text(slot)]));
                    }
                    Some(name) => {
                        let Some(display) = self.objective_name(name) else { return self.fail_component(source, tr("arguments.objective.notFound", vec![text(name)])) };
                        let _ = self.set_display_slot(MAIN, id as i32, Some(name));
                        self.feedback(source, tr("commands.scoreboard.objectives.display.set", vec![text(slot), display]));
                    }
                }
            }
            ["objectives", "modify", name, "displayname", rest @ ..] if !rest.is_empty() => {
                if self.objective_name(name).is_none() {
                    return self.fail_component(source, tr("arguments.objective.notFound", vec![text(name)]));
                }
                let display = component_arg(&rest.join(" "));
                let _ = self.update_objective(MAIN, name, Some(display.clone()), None);
                self.feedback(source, tr("commands.scoreboard.objectives.modify.displayname", vec![text(name), display]));
            }
            ["objectives", "modify", name, "rendertype", kind @ ("hearts" | "integer")] => {
                let Some(display) = self.objective_name(name) else { return self.fail_component(source, tr("arguments.objective.notFound", vec![text(name)])) };
                let _ = self.update_objective(MAIN, name, None, Some(i32::from(*kind == "hearts")));
                self.feedback(source, tr("commands.scoreboard.objectives.modify.rendertype", vec![display]));
            }
            ["players", "list"] => {
                let holders: Vec<String> = self.scoreboards.get(MAIN).map(|b| b.scores.iter().filter(|(_, s)| !s.is_empty()).map(|(h, _)| h.clone()).collect()).unwrap_or_default();
                if holders.is_empty() {
                    return self.feedback(source, tr("commands.scoreboard.players.list.empty", vec![]));
                }
                self.feedback(source, tr("commands.scoreboard.players.list.success", vec![text(&holders.len().to_string()), text(&holders.join(", "))]));
            }
            ["players", "list", target] => {
                for holder in self.score_holders(source, target) {
                    let scores: Vec<(String, i32)> = self.scoreboards.get(MAIN).and_then(|b| b.scores.get(&holder)).map(|s| s.iter().map(|(o, v)| (o.clone(), v.value)).collect()).unwrap_or_default();
                    if scores.is_empty() {
                        self.feedback(source, tr("commands.scoreboard.players.list.entity.empty", vec![text(&holder)]));
                        continue;
                    }
                    self.feedback(source, tr("commands.scoreboard.players.list.entity.success", vec![text(&holder), text(&scores.len().to_string())]));
                    for (objective, value) in scores {
                        let name = self.objective_name(&objective).unwrap_or_else(|| text(&objective));
                        self.feedback(source, tr("commands.scoreboard.players.list.entity.entry", vec![name, text(&value.to_string())]));
                    }
                }
            }
            ["players", "get", target, objective] => {
                let Some(display) = self.objective_name(objective) else { return self.fail_component(source, tr("arguments.objective.notFound", vec![text(objective)])) };
                let Some(holder) = self.score_holders(source, target).into_iter().next() else { return self.fail_component(source, tr("argument.entity.notfound.entity", vec![])) };
                match self.score_of(&holder, objective) {
                    Some(v) => self.feedback(source, tr("commands.scoreboard.players.get.success", vec![text(&holder), text(&v.to_string()), display])),
                    None => self.fail_component(source, tr("commands.scoreboard.players.get.null", vec![text(objective), text(&holder)])),
                }
            }
            ["players", op @ ("set" | "add" | "remove"), target, objective, amount] => {
                let Some(display) = self.objective_name(objective) else { return self.fail_component(source, tr("arguments.objective.notFound", vec![text(objective)])) };
                let criteria = self.scoreboards.get(MAIN).and_then(|b| b.objectives.get(*objective)).map(|o| o.criteria.clone()).unwrap_or_default();
                if criteria_read_only(&criteria) {
                    return self.fail_component(source, tr("commands.scoreboard.objectives.readonly", vec![display]));
                }
                let Ok(n) = amount.parse::<i32>() else { return self.fail(source, format!("Invalid integer '{amount}'")) };
                if *op != "set" && n < 0 {
                    return self.fail(source, "Integer must not be less than 0");
                }
                let holders = self.score_holders(source, target);
                if holders.is_empty() {
                    return self.fail_component(source, tr("argument.entity.notfound.entity", vec![]));
                }
                let mut last = 0;
                for holder in &holders {
                    let old = self.score_of(holder, objective).unwrap_or(0);
                    let new = match *op {
                        "set" => n,
                        "add" => old.wrapping_add(n),
                        _ => old.wrapping_sub(n),
                    };
                    let _ = self.set_score(MAIN, objective, holder, Some(new), None);
                    last = new;
                }
                let key = format!("commands.scoreboard.players.{op}.success.{}", if holders.len() == 1 { "single" } else { "multiple" });
                let who = if holders.len() == 1 { text(&holders[0]) } else { text(&holders.len().to_string()) };
                let message = match *op {
                    "set" => tr(&key, vec![display, who, text(&n.to_string())]),
                    _ if holders.len() == 1 => tr(&key, vec![text(&n.to_string()), display, who, text(&last.to_string())]),
                    _ => tr(&key, vec![text(&n.to_string()), display, who]),
                };
                self.feedback(source, message);
            }
            ["players", "reset", target, rest @ ..] => {
                let holders = self.score_holders(source, target);
                let objective = rest.first().copied();
                if let Some(o) = objective {
                    if self.objective_name(o).is_none() {
                        return self.fail_component(source, tr("arguments.objective.notFound", vec![text(o)]));
                    }
                }
                for holder in &holders {
                    let _ = self.reset_score(MAIN, holder, objective);
                }
                let one = holders.len() == 1;
                let who = if one { text(&holders.first().cloned().unwrap_or_default()) } else { text(&holders.len().to_string()) };
                let message = match objective {
                    Some(o) => tr(&format!("commands.scoreboard.players.reset.specific.{}", if one { "single" } else { "multiple" }), vec![self.objective_name(o).unwrap_or_else(|| text(o)), who]),
                    None => tr(&format!("commands.scoreboard.players.reset.all.{}", if one { "single" } else { "multiple" }), vec![who]),
                };
                self.feedback(source, message);
            }
            ["players", "operation", targets, target_objective, operation, sources, source_objective] => {
                for o in [target_objective, source_objective] {
                    if self.objective_name(o).is_none() {
                        return self.fail_component(source, tr("arguments.objective.notFound", vec![text(o)]));
                    }
                }
                let holders = self.score_holders(source, targets);
                let from = self.score_holders(source, sources);
                for holder in &holders {
                    for other in &from {
                        let a = self.score_of(holder, target_objective).unwrap_or(0);
                        let b = self.score_of(other, source_objective).unwrap_or(0);
                        let (new, swapped) = match *operation {
                            "=" => (b, None),
                            "+=" => (a.wrapping_add(b), None),
                            "-=" => (a.wrapping_sub(b), None),
                            "*=" => (a.wrapping_mul(b), None),
                            "/=" => (if b == 0 { a } else { a.div_euclid(b) }, None),
                            "%=" => (if b == 0 { a } else { a.rem_euclid(b) }, None),
                            "<" => (a.min(b), None),
                            ">" => (a.max(b), None),
                            "><" => (b, Some(a)),
                            _ => return self.fail_component(source, tr("arguments.operation.invalid", vec![])),
                        };
                        let _ = self.set_score(MAIN, target_objective, holder, Some(new), None);
                        if let Some(old) = swapped {
                            let _ = self.set_score(MAIN, source_objective, other, Some(old), None);
                        }
                    }
                }
                if let [holder] = holders.as_slice() {
                    let value = self.score_of(holder, target_objective).unwrap_or(0);
                    let display = self.objective_name(target_objective).unwrap_or_else(|| text(target_objective));
                    self.feedback(source, tr("commands.scoreboard.players.operation.success.single", vec![display, text(holder), text(&value.to_string())]));
                } else {
                    let display = self.objective_name(target_objective).unwrap_or_else(|| text(target_objective));
                    self.feedback(source, tr("commands.scoreboard.players.operation.success.multiple", vec![display, text(&holders.len().to_string())]));
                }
            }
            _ => self.fail(source, "Usage: /scoreboard objectives (list|add|remove|setdisplay|modify) ... | /scoreboard players (list|get|set|add|remove|reset|operation) ..."),
        }
    }

    fn team_exists(&self, name: &str) -> bool {
        self.scoreboards.get(MAIN).is_some_and(|b| b.teams.contains_key(name))
    }

    /// `/team ...`.
    pub(super) fn team_command(&mut self, source: &CommandSource, args: &[&str]) {
        let not_found = |name: &str| tr("team.notFound", vec![text(name)]);
        match args {
            ["list"] => {
                let names: Vec<String> = self.scoreboards.get(MAIN).map(|b| b.teams.keys().cloned().collect()).unwrap_or_default();
                if names.is_empty() {
                    return self.feedback(source, tr("commands.team.list.teams.empty", vec![]));
                }
                self.feedback(source, tr("commands.team.list.teams.success", vec![text(&names.len().to_string()), text(&names.join(", "))]));
            }
            ["list", team] => {
                let Some(members) = self.scoreboards.get(MAIN).and_then(|b| b.teams.get(*team)).map(|t| t.entries.iter().cloned().collect::<Vec<_>>()) else { return self.fail_component(source, not_found(team)) };
                if members.is_empty() {
                    return self.feedback(source, tr("commands.team.list.members.empty", vec![text(team)]));
                }
                self.feedback(source, tr("commands.team.list.members.success", vec![text(team), text(&members.len().to_string()), text(&members.join(", "))]));
            }
            ["add", team, rest @ ..] => {
                if self.team_exists(team) {
                    return self.fail_component(source, tr("commands.team.add.duplicate", vec![]));
                }
                if let Err(e) = self.register_team(MAIN, team) {
                    return self.fail(source, e.to_string());
                }
                if !rest.is_empty() {
                    let display = component_arg(&rest.join(" "));
                    let _ = self.update_team(MAIN, team, |t| t.display_name = display);
                }
                self.feedback(source, tr("commands.team.add.success", vec![text(team)]));
            }
            ["remove", team] => {
                if !self.team_exists(team) {
                    return self.fail_component(source, not_found(team));
                }
                let _ = self.unregister_team(MAIN, team);
                self.feedback(source, tr("commands.team.remove.success", vec![text(team)]));
            }
            ["empty", team] => {
                let Some(members) = self.scoreboards.get(MAIN).and_then(|b| b.teams.get(*team)).map(|t| t.entries.iter().cloned().collect::<Vec<_>>()) else { return self.fail_component(source, not_found(team)) };
                if members.is_empty() {
                    return self.fail_component(source, tr("commands.team.empty.unchanged", vec![]));
                }
                let _ = self.remove_team_entries(MAIN, team, &members);
                self.feedback(source, tr("commands.team.empty.success", vec![text(&members.len().to_string()), text(team)]));
            }
            ["join", team, rest @ ..] => {
                if !self.team_exists(team) {
                    return self.fail_component(source, not_found(team));
                }
                let members: Vec<String> = if rest.is_empty() {
                    vec![self.source_name(source)]
                } else {
                    rest.iter().flat_map(|t| self.score_holders(source, t)).collect()
                };
                let _ = self.add_team_entries(MAIN, team, &members);
                let message = if let [one] = members.as_slice() { tr("commands.team.join.success.single", vec![text(one), text(team)]) } else { tr("commands.team.join.success.multiple", vec![text(&members.len().to_string()), text(team)]) };
                self.feedback(source, message);
            }
            ["leave", rest @ ..] if !rest.is_empty() => {
                let members: Vec<String> = rest.iter().flat_map(|t| self.score_holders(source, t)).collect();
                let teams: Vec<(String, Vec<String>)> = self
                    .scoreboards
                    .get(MAIN)
                    .map(|b| b.teams.iter().map(|(n, t)| (n.clone(), members.iter().filter(|m| t.entries.contains(*m)).cloned().collect())).collect())
                    .unwrap_or_default();
                for (team, leaving) in teams.into_iter().filter(|(_, l)| !l.is_empty()) {
                    let _ = self.remove_team_entries(MAIN, &team, &leaving);
                }
                let message = if let [one] = members.as_slice() { tr("commands.team.leave.success.single", vec![text(one)]) } else { tr("commands.team.leave.success.multiple", vec![text(&members.len().to_string())]) };
                self.feedback(source, message);
            }
            ["modify", team, option, rest @ ..] if !rest.is_empty() => {
                if !self.team_exists(team) {
                    return self.fail_component(source, not_found(team));
                }
                let value = rest.join(" ");
                let (result, key, args) = match *option {
                    "displayName" => {
                        let display = component_arg(&value);
                        (self.update_team(MAIN, team, |t| t.display_name = display), "commands.team.option.name.success", vec![text(team)])
                    }
                    "prefix" => {
                        let c = component_arg(&value);
                        (self.update_team(MAIN, team, |t| t.prefix = c.clone()), "commands.team.option.prefix.success", vec![component_arg(&value)])
                    }
                    "suffix" => {
                        let c = component_arg(&value);
                        (self.update_team(MAIN, team, |t| t.suffix = c.clone()), "commands.team.option.suffix.success", vec![component_arg(&value)])
                    }
                    "color" => {
                        let color = if value == "reset" { None } else { COLORS.iter().position(|c| *c == value).map(|i| i as i32) };
                        if color.is_none() && value != "reset" {
                            return self.fail(source, format!("Invalid color '{value}'"));
                        }
                        (self.update_team(MAIN, team, |t| t.color = color), "commands.team.option.color.success", vec![text(team), text(&value)])
                    }
                    "friendlyFire" | "seeFriendlyInvisibles" => {
                        let Ok(on) = value.parse::<bool>() else { return self.fail(source, format!("Invalid boolean '{value}'")) };
                        let friendly = *option == "friendlyFire";
                        let key = match (friendly, on) {
                            (true, true) => "commands.team.option.friendlyfire.enabled",
                            (true, false) => "commands.team.option.friendlyfire.disabled",
                            (false, true) => "commands.team.option.seeFriendlyInvisibles.enabled",
                            (false, false) => "commands.team.option.seeFriendlyInvisibles.disabled",
                        };
                        (self.update_team(MAIN, team, |t| if friendly { t.friendly_fire = on } else { t.see_friendly_invisibles = on }), key, vec![text(team)])
                    }
                    "nametagVisibility" | "deathMessageVisibility" => {
                        let Some(v) = VISIBILITY.iter().position(|n| *n == value) else { return self.fail(source, format!("Invalid visibility '{value}'")) };
                        let name_tag = *option == "nametagVisibility";
                        let key = if name_tag { "commands.team.option.nametagVisibility.success" } else { "commands.team.option.deathMessageVisibility.success" };
                        (self.update_team(MAIN, team, |t| if name_tag { t.name_tag_visibility = v as i32 } else { t.death_message_visibility = v as i32 }), key, vec![text(team), text(&value)])
                    }
                    "collisionRule" => {
                        let Some(v) = COLLISION.iter().position(|n| *n == value) else { return self.fail(source, format!("Invalid collision rule '{value}'")) };
                        (self.update_team(MAIN, team, |t| t.collision_rule = v as i32), "commands.team.option.collisionRule.success", vec![text(team), text(&value)])
                    }
                    _ => return self.fail(source, format!("Unknown team option '{option}'")),
                };
                match result {
                    Ok(()) => self.feedback(source, tr(key, args)),
                    Err(e) => self.fail(source, e.to_string()),
                }
            }
            _ => self.fail(source, "Usage: /team (list|add|remove|empty|join|leave|modify) ..."),
        }
    }
}

/// Tab completion for `/scoreboard`.
pub(super) fn complete_scoreboard(server: &Server, _: &CommandSource, args: &[&str]) -> Vec<String> {
    let objectives = || server.scoreboards.get(MAIN).map(|b| b.objectives.keys().cloned().collect::<Vec<_>>()).unwrap_or_default();
    let words = |w: &[&str]| w.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
    match args {
        [] | [_] => words(&["objectives", "players"]),
        ["objectives", _] => words(&["list", "add", "remove", "setdisplay", "modify"]),
        ["objectives", "remove" | "modify", _] => objectives(),
        ["objectives", "setdisplay", _] => DISPLAY_SLOTS.iter().map(|s| (*s).to_owned()).collect(),
        ["objectives", "setdisplay", _, _] => objectives(),
        ["objectives", "add", _, _] => words(&["dummy", "trigger", "deathCount", "playerKillCount", "totalKillCount", "health", "xp", "level", "food", "air", "armor"]),
        ["objectives", "modify", _, _] => words(&["displayname", "rendertype"]),
        ["players", _] => words(&["list", "get", "set", "add", "remove", "reset", "operation"]),
        ["players", _, _] => server.online_players().map(|p| p.name().to_owned()).chain(["@a", "@p", "@s", "*"].map(String::from)).collect(),
        ["players", "get" | "set" | "add" | "remove" | "reset" | "operation", _, _] => objectives(),
        _ => Vec::new(),
    }
}

/// Tab completion for `/team`.
pub(super) fn complete_team(server: &Server, _: &CommandSource, args: &[&str]) -> Vec<String> {
    let teams = || server.scoreboards.get(MAIN).map(|b| b.teams.keys().cloned().collect::<Vec<_>>()).unwrap_or_default();
    let words = |w: &[&str]| w.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
    match args {
        [] | [_] => words(&["list", "add", "remove", "empty", "join", "leave", "modify"]),
        ["list" | "remove" | "empty" | "join" | "modify", _] => teams(),
        ["modify", _, _] => words(&["displayName", "color", "friendlyFire", "seeFriendlyInvisibles", "nametagVisibility", "deathMessageVisibility", "collisionRule", "prefix", "suffix"]),
        ["modify", _, "color", _] => COLORS.iter().map(|c| (*c).to_owned()).chain(["reset".to_owned()]).collect(),
        ["modify", _, "nametagVisibility" | "deathMessageVisibility", _] => words(&VISIBILITY),
        ["modify", _, "collisionRule", _] => words(&COLLISION),
        ["modify", _, "friendlyFire" | "seeFriendlyInvisibles", _] => words(&["true", "false"]),
        ["join", _, _] | ["leave", _] => server.online_players().map(|p| p.name().to_owned()).collect(),
        _ => Vec::new(),
    }
}
