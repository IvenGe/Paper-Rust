//! Scoreboards: objectives, scores, display slots and teams, and the packets that show them.
//!
//! The server owns every scoreboard: the main one (id [`MAIN`], which new players see) and
//! the ones plugins create (`ScoreboardManager.getNewScoreboard`). Each player views one
//! board; every change is sent to the players viewing that board, and switching boards sends
//! the removal of the old board's objectives and teams and the whole new board, as vanilla's
//! `ServerScoreboard` and CraftBukkit's `CraftScoreboardManager` do.
//!
//! Scores of objectives with the `health`, `food`, `air`, `armor`, `level` and `xp` criteria
//! follow the players' values ([`Server::tick_scoreboards`]); other criteria (statistics,
//! kill and death counts) are not counted yet and only change when plugins set them.

use super::Server;
use bytes::Bytes;
use paper_protocol::codec::VarInt;
use paper_protocol::packets::{encode_packet, play};
use paper_protocol::text::Component;
use std::collections::{BTreeMap, BTreeSet, HashMap};

/// The id of the main scoreboard.
pub const MAIN: u64 = 0;

/// Display slot names (Bukkit `DisplaySlot` ids) by protocol id.
pub const DISPLAY_SLOTS: [&str; 19] = [
    "list", "sidebar", "below_name",
    "sidebar.team.black", "sidebar.team.dark_blue", "sidebar.team.dark_green", "sidebar.team.dark_aqua", "sidebar.team.dark_red", "sidebar.team.dark_purple",
    "sidebar.team.gold", "sidebar.team.gray", "sidebar.team.dark_gray", "sidebar.team.blue", "sidebar.team.green", "sidebar.team.aqua", "sidebar.team.red",
    "sidebar.team.light_purple", "sidebar.team.yellow", "sidebar.team.white",
];

/// Criteria whose scores are computed from player values and cannot be set.
pub fn criteria_read_only(criteria: &str) -> bool {
    matches!(criteria, "health" | "food" | "air" | "armor" | "level" | "xp")
}

/// The render type (0 integer, 1 hearts) a criteria uses by default.
pub fn default_render_type(criteria: &str) -> i32 {
    i32::from(criteria == "health")
}

#[derive(Clone, Debug)]
pub struct Objective {
    pub name: String,
    pub criteria: String,
    pub display_name: Component,
    /// 0 integer, 1 hearts.
    pub render_type: i32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Score {
    pub value: i32,
    pub display_name: Option<Component>,
    /// Whether `/trigger` refuses this score (vanilla's default) until it is enabled with
    /// `/scoreboard players enable` or `Score.setTriggerable`.
    pub locked: bool,
}

#[derive(Clone, Debug)]
pub struct Team {
    pub name: String,
    pub display_name: Component,
    pub prefix: Component,
    pub suffix: Component,
    /// Team colour id (0-15, `TeamColor` order), or none.
    pub color: Option<i32>,
    pub friendly_fire: bool,
    pub see_friendly_invisibles: bool,
    /// 0 always, 1 never, 2 hide for other teams, 3 hide for own team.
    pub name_tag_visibility: i32,
    /// Same values; only kept, the client is not told.
    pub death_message_visibility: i32,
    /// 0 always, 1 never, 2 push other teams, 3 push own team.
    pub collision_rule: i32,
    pub entries: BTreeSet<String>,
}

impl Team {
    fn new(name: &str) -> Self {
        Team {
            name: name.to_owned(),
            display_name: Component::text(name),
            prefix: Component::text(""),
            suffix: Component::text(""),
            color: None,
            friendly_fire: true,
            see_friendly_invisibles: true,
            name_tag_visibility: 0,
            death_message_visibility: 0,
            collision_rule: 0,
            entries: BTreeSet::new(),
        }
    }

    fn parameters(&self) -> play::TeamParameters {
        play::TeamParameters {
            display_name: self.display_name.clone(),
            prefix: self.prefix.clone(),
            suffix: self.suffix.clone(),
            nametag_visibility: self.name_tag_visibility,
            collision_rule: self.collision_rule,
            color: self.color,
            options: i8::from(self.friendly_fire) | if self.see_friendly_invisibles { 2 } else { 0 },
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Scoreboard {
    pub objectives: BTreeMap<String, Objective>,
    /// Display slot id to objective name.
    pub display: BTreeMap<i32, String>,
    /// Entry (player name or entity UUID) to objective name to score.
    pub scores: BTreeMap<String, BTreeMap<String, Score>>,
    pub teams: BTreeMap<String, Team>,
    /// The plugin host dropped its last handle; the board goes once nobody views it.
    pub released: bool,
}

impl Scoreboard {
    pub fn team_of(&self, entry: &str) -> Option<&Team> {
        self.teams.values().find(|t| t.entries.contains(entry))
    }

    /// The packets that show the whole board to a client that saw none.
    fn show_packets(&self) -> Vec<Bytes> {
        let mut out = Vec::new();
        for o in self.objectives.values() {
            out.push(encode(&play::SetObjective { name: o.name.clone(), mode: 0, display_name: o.display_name.clone(), render_type: o.render_type }));
        }
        for (slot, name) in &self.display {
            out.push(encode(&play::SetDisplayObjective { slot: VarInt(*slot), objective: name.clone() }));
        }
        for (entry, scores) in &self.scores {
            for (objective, score) in scores {
                out.push(encode(&play::SetScore { owner: entry.clone(), objective: objective.clone(), value: score.value, display_name: score.display_name.clone() }));
            }
        }
        for t in self.teams.values() {
            out.push(encode(&play::SetPlayerTeam { name: t.name.clone(), method: 0, parameters: Some(t.parameters()), members: t.entries.iter().cloned().collect() }));
        }
        out
    }

    /// The packets that remove the whole board from a client.
    fn hide_packets(&self) -> Vec<Bytes> {
        let mut out = Vec::new();
        for o in self.objectives.values() {
            out.push(encode(&play::SetObjective { name: o.name.clone(), mode: 1, display_name: Component::text(""), render_type: 0 }));
        }
        for t in self.teams.values() {
            out.push(encode(&play::SetPlayerTeam { name: t.name.clone(), method: 1, parameters: None, members: Vec::new() }));
        }
        out
    }
}

fn encode<P: paper_protocol::packets::Packet + paper_protocol::codec::Encode>(packet: &P) -> Bytes {
    Bytes::from(encode_packet(packet))
}

// --- saving (`ScoreboardSaveData`, `data/minecraft/scoreboard.dat`) -----------------------------

/// Team colours by id (`ChatFormatting` names).
const TEAM_COLORS: [&str; 16] = ["black", "dark_blue", "dark_green", "dark_aqua", "dark_red", "dark_purple", "gold", "gray", "dark_gray", "blue", "green", "aqua", "red", "light_purple", "yellow", "white"];
/// Name tag and death message visibility by id (`Team.Visibility`).
const VISIBILITY: [&str; 4] = ["always", "never", "hideForOtherTeams", "hideForOwnTeam"];
/// Collision rules by id (`Team.CollisionRule`).
const COLLISION: [&str; 4] = ["always", "never", "pushOtherTeams", "pushOwnTeam"];

fn component_tag(c: &Component) -> paper_nbt::Tag {
    c.to_nbt()
}

fn component_from(tag: Option<&paper_nbt::Tag>) -> Option<Component> {
    match tag? {
        paper_nbt::Tag::String(s) => Component::from_json_str(s).ok().or_else(|| Some(Component::text(s.clone()))),
        t => Some(Component::from_nbt(t)),
    }
}

impl Scoreboard {
    /// The board as vanilla saves the main scoreboard.
    pub fn to_nbt(&self) -> paper_nbt::Compound {
        use paper_nbt::{Compound, Tag};
        let objectives = self
            .objectives
            .values()
            .map(|o| {
                Tag::Compound(
                    Compound::new()
                        .with("Name", Tag::String(o.name.clone()))
                        .with("CriteriaName", Tag::String(o.criteria.clone()))
                        .with("DisplayName", component_tag(&o.display_name))
                        .with("RenderType", Tag::String(if o.render_type == 1 { "hearts" } else { "integer" }.into()))
                        .with("DisplayAutoUpdate", Tag::Byte(0)),
                )
            })
            .collect();
        let mut scores = Vec::new();
        for (entry, by_objective) in &self.scores {
            for (objective, score) in by_objective {
                let mut c = Compound::new().with("Name", Tag::String(entry.clone())).with("Objective", Tag::String(objective.clone())).with("Score", Tag::Int(score.value));
                if score.locked {
                    c.insert("Locked", Tag::Byte(1));
                }
                if let Some(display) = &score.display_name {
                    c.insert("display", component_tag(display));
                }
                scores.push(Tag::Compound(c));
            }
        }
        let teams = self
            .teams
            .values()
            .map(|t| {
                let mut c = Compound::new()
                    .with("Name", Tag::String(t.name.clone()))
                    .with("DisplayName", component_tag(&t.display_name))
                    .with("MemberNamePrefix", component_tag(&t.prefix))
                    .with("MemberNameSuffix", component_tag(&t.suffix))
                    .with("AllowFriendlyFire", Tag::Byte(t.friendly_fire as i8))
                    .with("SeeFriendlyInvisibles", Tag::Byte(t.see_friendly_invisibles as i8))
                    .with("NameTagVisibility", Tag::String(VISIBILITY[t.name_tag_visibility.clamp(0, 3) as usize].into()))
                    .with("DeathMessageVisibility", Tag::String(VISIBILITY[t.death_message_visibility.clamp(0, 3) as usize].into()))
                    .with("CollisionRule", Tag::String(COLLISION[t.collision_rule.clamp(0, 3) as usize].into()))
                    .with("Players", Tag::List(t.entries.iter().map(|e| Tag::String(e.clone())).collect()));
                if let Some(color) = t.color.and_then(|c| TEAM_COLORS.get(c as usize)) {
                    c.insert("TeamColor", Tag::String((*color).into()));
                }
                Tag::Compound(c)
            })
            .collect();
        let mut slots = Compound::new();
        for (slot, objective) in &self.display {
            if let Some(name) = DISPLAY_SLOTS.get(*slot as usize) {
                slots.insert(*name, Tag::String(objective.clone()));
            }
        }
        let data = Compound::new().with("Objectives", Tag::List(objectives)).with("PlayerScores", Tag::List(scores)).with("Teams", Tag::List(teams)).with("DisplaySlots", Tag::Compound(slots));
        Compound::new().with("data", Tag::Compound(data)).with("DataVersion", Tag::Int(crate::world::storage::data_version()))
    }

    /// A board from a saved `scoreboard.dat`.
    pub fn from_nbt(root: &paper_nbt::Compound) -> Scoreboard {
        let mut board = Scoreboard::default();
        let Some(data) = root.compound("data") else { return board };
        for o in data.list("Objectives").unwrap_or(&[]).iter().filter_map(|t| t.as_compound()) {
            let (Some(name), Some(criteria)) = (o.string("Name"), o.string("CriteriaName")) else { continue };
            let render_type = i32::from(o.string("RenderType") == Some("hearts"));
            let display_name = component_from(o.get("DisplayName")).unwrap_or_else(|| Component::text(name));
            board.objectives.insert(name.to_owned(), Objective { name: name.to_owned(), criteria: criteria.to_owned(), display_name, render_type });
        }
        for s in data.list("PlayerScores").unwrap_or(&[]).iter().filter_map(|t| t.as_compound()) {
            let (Some(entry), Some(objective)) = (s.string("Name"), s.string("Objective")) else { continue };
            if !board.objectives.contains_key(objective) {
                continue;
            }
            let score = Score { value: s.i32("Score").unwrap_or(0), display_name: component_from(s.get("display")), locked: s.bool("Locked").unwrap_or(false) };
            board.scores.entry(entry.to_owned()).or_default().insert(objective.to_owned(), score);
        }
        for t in data.list("Teams").unwrap_or(&[]).iter().filter_map(|t| t.as_compound()) {
            let Some(name) = t.string("Name") else { continue };
            let mut team = Team::new(name);
            if let Some(c) = component_from(t.get("DisplayName")) {
                team.display_name = c;
            }
            if let Some(c) = component_from(t.get("MemberNamePrefix")) {
                team.prefix = c;
            }
            if let Some(c) = component_from(t.get("MemberNameSuffix")) {
                team.suffix = c;
            }
            team.color = t.string("TeamColor").and_then(|c| TEAM_COLORS.iter().position(|n| *n == c)).map(|i| i as i32);
            team.friendly_fire = t.bool("AllowFriendlyFire").unwrap_or(true);
            team.see_friendly_invisibles = t.bool("SeeFriendlyInvisibles").unwrap_or(true);
            let pick = |names: &[&str; 4], key: &str| t.string(key).and_then(|v| names.iter().position(|n| *n == v)).unwrap_or(0) as i32;
            team.name_tag_visibility = pick(&VISIBILITY, "NameTagVisibility");
            team.death_message_visibility = pick(&VISIBILITY, "DeathMessageVisibility");
            team.collision_rule = pick(&COLLISION, "CollisionRule");
            team.entries = t.list("Players").unwrap_or(&[]).iter().filter_map(|p| p.as_str().map(str::to_owned)).collect();
            board.teams.insert(name.to_owned(), team);
        }
        if let Some(slots) = data.compound("DisplaySlots") {
            for (slot, objective) in slots.iter() {
                let (Some(id), Some(objective)) = (DISPLAY_SLOTS.iter().position(|n| *n == slot.as_str()), objective.as_str()) else { continue };
                if board.objectives.contains_key(objective) {
                    board.display.insert(id as i32, objective.to_owned());
                }
            }
        }
        board
    }
}

/// Every scoreboard and who views which.
#[derive(Debug)]
pub struct Scoreboards {
    boards: HashMap<u64, Scoreboard>,
    next_id: u64,
    /// The board each player views, by entity id; players not listed view the main board.
    viewing: HashMap<i32, u64>,
}

impl Default for Scoreboards {
    fn default() -> Self {
        Self::new()
    }
}

impl Scoreboards {
    /// Every board's id.
    pub fn ids(&self) -> Vec<u64> {
        self.boards.keys().copied().collect()
    }

    /// Replaces the main board (with a saved one) before anyone views it.
    pub fn set_main(&mut self, board: Scoreboard) {
        self.boards.insert(MAIN, board);
    }

    pub fn new() -> Self {
        let mut boards = HashMap::new();
        boards.insert(MAIN, Scoreboard::default());
        Self { boards, next_id: 1, viewing: HashMap::new() }
    }

    pub fn create(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.boards.insert(id, Scoreboard::default());
        id
    }

    pub fn get(&self, id: u64) -> Option<&Scoreboard> {
        self.boards.get(&id)
    }

    pub fn get_mut(&mut self, id: u64) -> Option<&mut Scoreboard> {
        self.boards.get_mut(&id)
    }

    pub fn exists(&self, id: u64) -> bool {
        self.boards.contains_key(&id)
    }

    /// The board a player views.
    pub fn board_of(&self, player: i32) -> u64 {
        self.viewing.get(&player).copied().unwrap_or(MAIN)
    }

    /// The players viewing a board (among `online`).
    pub fn viewers<'a>(&'a self, board: u64, online: impl Iterator<Item = &'a i32> + 'a) -> impl Iterator<Item = i32> + 'a {
        online.copied().filter(move |p| self.board_of(*p) == board)
    }

    fn drop_if_unused(&mut self, board: u64) {
        if board != MAIN && self.boards.get(&board).is_some_and(|b| b.released) && !self.viewing.values().any(|b| *b == board) {
            self.boards.remove(&board);
        }
    }

    /// The plugin host no longer refers to a board.
    pub fn release(&mut self, board: u64) {
        if let Some(b) = self.boards.get_mut(&board) {
            b.released = true;
        }
        self.drop_if_unused(board);
    }

    /// The plugin host refers to a board again.
    pub fn retain(&mut self, board: u64) {
        if let Some(b) = self.boards.get_mut(&board) {
            b.released = false;
        }
    }

    /// Forgets a player who left.
    pub fn player_left(&mut self, player: i32) {
        if let Some(board) = self.viewing.remove(&player) {
            self.drop_if_unused(board);
        }
    }

    /// Drops every board but the main one and puts everyone back on the main board (when the
    /// plugin host that made them is gone). Returns the players who must be switched.
    pub fn reset_plugin_boards(&mut self) -> Vec<(i32, u64)> {
        let moved: Vec<(i32, u64)> = self.viewing.drain().collect();
        self.boards.retain(|id, _| *id == MAIN);
        moved
    }
}

impl Server {
    fn scoreboard_viewers(&self, board: u64) -> Vec<i32> {
        self.scoreboards.viewers(board, self.player_order.iter()).collect()
    }

    fn send_to_board(&self, board: u64, payloads: &[Bytes]) {
        for id in self.scoreboard_viewers(board) {
            if let Some(p) = self.players.get(&id) {
                for payload in payloads {
                    p.conn.send_payload(payload.clone());
                }
            }
        }
    }

    /// Shows a player the board they view (after joining).
    pub fn send_scoreboard(&self, player: i32) {
        let board = self.scoreboards.board_of(player);
        let Some(b) = self.scoreboards.get(board) else { return };
        if let Some(p) = self.players.get(&player) {
            for payload in b.show_packets() {
                p.conn.send_payload(payload);
            }
        }
    }

    /// Switches the board a player views.
    pub fn set_player_scoreboard(&mut self, player: i32, board: u64) -> anyhow::Result<()> {
        if !self.scoreboards.exists(board) {
            anyhow::bail!("unknown scoreboard {board}");
        }
        let old = self.scoreboards.board_of(player);
        if old == board {
            return Ok(());
        }
        if let (Some(p), Some(b)) = (self.players.get(&player), self.scoreboards.get(old)) {
            for payload in b.hide_packets() {
                p.conn.send_payload(payload);
            }
        }
        if board == MAIN {
            self.scoreboards.viewing.remove(&player);
        } else {
            self.scoreboards.viewing.insert(player, board);
        }
        self.scoreboards.drop_if_unused(old);
        self.send_scoreboard(player);
        Ok(())
    }

    /// When the plugin host that made them is gone: removes the plugin boards from their
    /// viewers' screens, drops them and shows everyone the main board again.
    pub fn reset_plugin_scoreboards(&mut self) {
        for id in &self.player_order {
            let board = self.scoreboards.board_of(*id);
            if board == MAIN {
                continue;
            }
            if let (Some(p), Some(b)) = (self.players.get(id), self.scoreboards.get(board)) {
                for payload in b.hide_packets() {
                    p.conn.send_payload(payload);
                }
            }
        }
        for (player, _) in self.scoreboards.reset_plugin_boards() {
            self.send_scoreboard(player);
        }
    }

    // --- objectives ---------------------------------------------------------------------------

    pub fn register_objective(&mut self, board: u64, name: &str, criteria: &str, display_name: Component, render_type: i32) -> anyhow::Result<()> {
        let b = self.scoreboards.get_mut(board).ok_or_else(|| anyhow::anyhow!("unknown scoreboard {board}"))?;
        if name.is_empty() || name.chars().count() > 32767 {
            anyhow::bail!("objective name must be 1 to 32767 characters");
        }
        if b.objectives.contains_key(name) {
            anyhow::bail!("An objective of name '{name}' already exists");
        }
        let objective = Objective { name: name.to_owned(), criteria: criteria.to_owned(), display_name, render_type };
        let packet = encode(&play::SetObjective { name: name.to_owned(), mode: 0, display_name: objective.display_name.clone(), render_type });
        b.objectives.insert(name.to_owned(), objective);
        self.send_to_board(board, &[packet]);
        self.update_criteria_scores(board);
        Ok(())
    }

    pub fn unregister_objective(&mut self, board: u64, name: &str) -> anyhow::Result<()> {
        let b = self.scoreboards.get_mut(board).ok_or_else(|| anyhow::anyhow!("unknown scoreboard {board}"))?;
        if b.objectives.remove(name).is_none() {
            anyhow::bail!("Unregistered objective {name}");
        }
        b.display.retain(|_, o| o != name);
        for scores in b.scores.values_mut() {
            scores.remove(name);
        }
        b.scores.retain(|_, s| !s.is_empty());
        let packet = encode(&play::SetObjective { name: name.to_owned(), mode: 1, display_name: Component::text(""), render_type: 0 });
        self.send_to_board(board, &[packet]);
        Ok(())
    }

    pub fn update_objective(&mut self, board: u64, name: &str, display_name: Option<Component>, render_type: Option<i32>) -> anyhow::Result<()> {
        let b = self.scoreboards.get_mut(board).ok_or_else(|| anyhow::anyhow!("unknown scoreboard {board}"))?;
        let o = b.objectives.get_mut(name).ok_or_else(|| anyhow::anyhow!("Unregistered objective {name}"))?;
        if let Some(d) = display_name {
            o.display_name = d;
        }
        if let Some(r) = render_type {
            o.render_type = r;
        }
        let packet = encode(&play::SetObjective { name: name.to_owned(), mode: 2, display_name: o.display_name.clone(), render_type: o.render_type });
        self.send_to_board(board, &[packet]);
        Ok(())
    }

    /// Shows an objective in a display slot, or clears the slot.
    pub fn set_display_slot(&mut self, board: u64, slot: i32, objective: Option<&str>) -> anyhow::Result<()> {
        let b = self.scoreboards.get_mut(board).ok_or_else(|| anyhow::anyhow!("unknown scoreboard {board}"))?;
        if !(0..DISPLAY_SLOTS.len() as i32).contains(&slot) {
            anyhow::bail!("invalid display slot {slot}");
        }
        match objective {
            Some(name) => {
                if !b.objectives.contains_key(name) {
                    anyhow::bail!("Unregistered objective {name}");
                }
                b.display.insert(slot, name.to_owned());
            }
            None => {
                b.display.remove(&slot);
            }
        }
        let packet = encode(&play::SetDisplayObjective { slot: VarInt(slot), objective: objective.unwrap_or("").to_owned() });
        self.send_to_board(board, &[packet]);
        Ok(())
    }

    // --- scores -------------------------------------------------------------------------------

    pub fn set_score(&mut self, board: u64, objective: &str, entry: &str, value: Option<i32>, display_name: Option<Option<Component>>) -> anyhow::Result<()> {
        let b = self.scoreboards.get_mut(board).ok_or_else(|| anyhow::anyhow!("unknown scoreboard {board}"))?;
        if !b.objectives.contains_key(objective) {
            anyhow::bail!("Unregistered objective {objective}");
        }
        let score = b.scores.entry(entry.to_owned()).or_default().entry(objective.to_owned()).or_insert(Score { value: 0, display_name: None, locked: true });
        if let Some(v) = value {
            score.value = v;
        }
        if let Some(d) = display_name {
            score.display_name = d;
        }
        let packet = encode(&play::SetScore { owner: entry.to_owned(), objective: objective.to_owned(), value: score.value, display_name: score.display_name.clone() });
        self.send_to_board(board, &[packet]);
        Ok(())
    }

    /// Locks or unlocks an entry's score (`ScoreAccess.lock`/`unlock`), creating it with 0 if
    /// it is missing (a new score is shown to viewers). Answers whether the lock changed.
    pub fn set_score_locked(&mut self, board: u64, objective: &str, entry: &str, locked: bool) -> anyhow::Result<bool> {
        let b = self.scoreboards.get_mut(board).ok_or_else(|| anyhow::anyhow!("unknown scoreboard {board}"))?;
        if !b.objectives.contains_key(objective) {
            anyhow::bail!("Unregistered objective {objective}");
        }
        let scores = b.scores.entry(entry.to_owned()).or_default();
        let created = !scores.contains_key(objective);
        let score = scores.entry(objective.to_owned()).or_insert(Score { value: 0, display_name: None, locked: true });
        let changed = score.locked != locked;
        score.locked = locked;
        if created {
            let packet = encode(&play::SetScore { owner: entry.to_owned(), objective: objective.to_owned(), value: score.value, display_name: score.display_name.clone() });
            self.send_to_board(board, &[packet]);
        }
        Ok(changed)
    }

    /// Removes an entry's score from one objective, or from all.
    pub fn reset_score(&mut self, board: u64, entry: &str, objective: Option<&str>) -> anyhow::Result<()> {
        let b = self.scoreboards.get_mut(board).ok_or_else(|| anyhow::anyhow!("unknown scoreboard {board}"))?;
        match objective {
            Some(o) => {
                if let Some(scores) = b.scores.get_mut(entry) {
                    scores.remove(o);
                    if scores.is_empty() {
                        b.scores.remove(entry);
                    }
                }
            }
            None => {
                b.scores.remove(entry);
            }
        }
        let packet = encode(&play::ResetScore { owner: entry.to_owned(), objective: objective.map(str::to_owned) });
        self.send_to_board(board, &[packet]);
        Ok(())
    }

    // --- teams --------------------------------------------------------------------------------

    pub fn register_team(&mut self, board: u64, name: &str) -> anyhow::Result<()> {
        let b = self.scoreboards.get_mut(board).ok_or_else(|| anyhow::anyhow!("unknown scoreboard {board}"))?;
        if b.teams.contains_key(name) {
            anyhow::bail!("Team by the name '{name}' already exists");
        }
        let team = Team::new(name);
        let packet = encode(&play::SetPlayerTeam { name: name.to_owned(), method: 0, parameters: Some(team.parameters()), members: Vec::new() });
        b.teams.insert(name.to_owned(), team);
        self.send_to_board(board, &[packet]);
        Ok(())
    }

    pub fn unregister_team(&mut self, board: u64, name: &str) -> anyhow::Result<()> {
        let b = self.scoreboards.get_mut(board).ok_or_else(|| anyhow::anyhow!("unknown scoreboard {board}"))?;
        if b.teams.remove(name).is_none() {
            anyhow::bail!("Unregistered team {name}");
        }
        let packet = encode(&play::SetPlayerTeam { name: name.to_owned(), method: 1, parameters: None, members: Vec::new() });
        self.send_to_board(board, &[packet]);
        Ok(())
    }

    /// Changes a team's settings with `edit` and tells the viewers.
    pub fn update_team(&mut self, board: u64, name: &str, edit: impl FnOnce(&mut Team)) -> anyhow::Result<()> {
        let b = self.scoreboards.get_mut(board).ok_or_else(|| anyhow::anyhow!("unknown scoreboard {board}"))?;
        let team = b.teams.get_mut(name).ok_or_else(|| anyhow::anyhow!("Unregistered team {name}"))?;
        edit(team);
        let packet = encode(&play::SetPlayerTeam { name: name.to_owned(), method: 2, parameters: Some(team.parameters()), members: Vec::new() });
        self.send_to_board(board, &[packet]);
        Ok(())
    }

    /// Adds entries to a team, taking them out of the team they were in.
    pub fn add_team_entries(&mut self, board: u64, name: &str, entries: &[String]) -> anyhow::Result<()> {
        let b = self.scoreboards.get_mut(board).ok_or_else(|| anyhow::anyhow!("unknown scoreboard {board}"))?;
        if !b.teams.contains_key(name) {
            anyhow::bail!("Unregistered team {name}");
        }
        let mut packets = Vec::new();
        for entry in entries {
            let previous = b.teams.values().find(|t| t.entries.contains(entry)).map(|t| t.name.clone());
            match previous {
                Some(p) if p == *name => continue,
                Some(p) => {
                    b.teams.get_mut(&p).unwrap().entries.remove(entry);
                    packets.push(encode(&play::SetPlayerTeam { name: p, method: 4, parameters: None, members: vec![entry.clone()] }));
                }
                None => {}
            }
            b.teams.get_mut(name).unwrap().entries.insert(entry.clone());
        }
        let added: Vec<String> = entries.iter().filter(|e| b.teams[name].entries.contains(*e)).cloned().collect();
        if !added.is_empty() {
            packets.push(encode(&play::SetPlayerTeam { name: name.to_owned(), method: 3, parameters: None, members: added }));
        }
        self.send_to_board(board, &packets);
        Ok(())
    }

    /// Removes entries from a team. Returns those that were members.
    pub fn remove_team_entries(&mut self, board: u64, name: &str, entries: &[String]) -> anyhow::Result<Vec<String>> {
        let b = self.scoreboards.get_mut(board).ok_or_else(|| anyhow::anyhow!("unknown scoreboard {board}"))?;
        let team = b.teams.get_mut(name).ok_or_else(|| anyhow::anyhow!("Unregistered team {name}"))?;
        let removed: Vec<String> = entries.iter().filter(|e| team.entries.remove(*e)).cloned().collect();
        if !removed.is_empty() {
            let packet = encode(&play::SetPlayerTeam { name: name.to_owned(), method: 4, parameters: None, members: removed.clone() });
            self.send_to_board(board, &[packet]);
        }
        Ok(removed)
    }

    // --- criteria -------------------------------------------------------------------------------

    /// Keeps the scores of read-only criteria (`health`, `food`, ...) in step with the players.
    pub fn tick_scoreboards(&mut self) {
        let boards: Vec<u64> = self.scoreboards.boards.iter().filter(|(_, b)| b.objectives.values().any(|o| criteria_read_only(&o.criteria))).map(|(id, _)| *id).collect();
        for board in boards {
            self.update_criteria_scores(board);
        }
    }

    fn update_criteria_scores(&mut self, board: u64) {
        let Some(b) = self.scoreboards.get(board) else { return };
        let objectives: Vec<(String, String)> = b.objectives.values().filter(|o| criteria_read_only(&o.criteria)).map(|o| (o.name.clone(), o.criteria.clone())).collect();
        if objectives.is_empty() {
            return;
        }
        let mut updates = Vec::new();
        for p in self.online_players() {
            for (objective, criteria) in &objectives {
                let value = match criteria.as_str() {
                    "health" => p.health.ceil() as i32,
                    "food" => p.food,
                    "air" => p.air.max(0),
                    "level" => p.xp_level,
                    "xp" => p.xp_total,
                    "armor" => {
                        p.attributes.value("armor") as i32
                    }
                    _ => continue,
                };
                let current = b.scores.get(p.name()).and_then(|s| s.get(objective)).map(|s| s.value);
                if current != Some(value) {
                    updates.push((objective.clone(), p.name().to_owned(), value));
                }
            }
        }
        for (objective, entry, value) in updates {
            let _ = self.set_score(board, &objective, &entry, Some(value), None);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_main_board_saves_like_vanilla() {
        let mut board = Scoreboard::default();
        board.objectives.insert("kills".into(), Objective { name: "kills".into(), criteria: "dummy".into(), display_name: Component::text("Kills"), render_type: 0 });
        board.display.insert(1, "kills".into());
        board.scores.entry("Alice".into()).or_default().insert("kills".into(), Score { value: 7, display_name: None, locked: true });
        board.objectives.insert("vote".into(), Objective { name: "vote".into(), criteria: "trigger".into(), display_name: Component::text("Vote"), render_type: 0 });
        board.scores.entry("Alice".into()).or_default().insert("vote".into(), Score { value: 0, display_name: None, locked: false });
        let mut red = Team::new("red");
        red.color = Some(12);
        red.friendly_fire = false;
        red.collision_rule = 1;
        red.entries.insert("Alice".into());
        board.teams.insert("red".into(), red);
        let nbt = board.to_nbt();
        let back = Scoreboard::from_nbt(&nbt);
        assert_eq!(back.objectives["kills"].criteria, "dummy");
        assert_eq!(back.display.get(&1).map(String::as_str), Some("kills"));
        assert_eq!(back.scores["Alice"]["kills"].value, 7);
        assert_eq!((back.scores["Alice"]["kills"].locked, back.scores["Alice"]["vote"].locked), (true, false), "trigger locks are kept");
        let red = &back.teams["red"];
        assert_eq!((red.color, red.friendly_fire, red.collision_rule, red.entries.contains("Alice")), (Some(12), false, 1, true));
        assert_eq!(nbt.compound("data").and_then(|d| d.compound("DisplaySlots")).and_then(|d| d.string("sidebar")), Some("kills"));
    }

    #[test]
    fn boards_are_dropped_once_released_and_unviewed() {
        let mut boards = Scoreboards::new();
        let id = boards.create();
        boards.viewing.insert(7, id);
        boards.release(id);
        assert!(boards.exists(id), "still viewed");
        boards.player_left(7);
        assert!(!boards.exists(id));
        boards.release(MAIN);
        assert!(boards.exists(MAIN), "the main board stays");
    }

    #[test]
    fn team_options_pack_like_vanilla() {
        let mut team = Team::new("red");
        team.friendly_fire = false;
        assert_eq!(team.parameters().options, 2);
        team.see_friendly_invisibles = false;
        team.friendly_fire = true;
        assert_eq!(team.parameters().options, 1);
    }
}
