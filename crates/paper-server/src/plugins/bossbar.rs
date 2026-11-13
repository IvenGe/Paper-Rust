//! Boss bars plugins show (`Audience.showBossBar` and Bukkit's `BossBar`). The host gives
//! every bar a UUID and reports its look; the server keeps the bar's state and viewers and
//! sends them `BossEvent`s.

use crate::server::Server;
use anyhow::{anyhow, Result};
use paper_bridge::Value;
use paper_protocol::packets::play::{BossEvent, BossEventAction};
use paper_protocol::text::Component;
use std::collections::{HashMap, HashSet};
use uuid::Uuid;

/// A boss bar as clients see it.
#[derive(Clone, Debug)]
pub struct BossBar {
    pub title: Component,
    pub progress: f32,
    /// `pink`, `blue`, ... as the protocol numbers them.
    pub color: i32,
    /// Notches: 0 none, 1 six, 2 ten, 3 twelve, 4 twenty.
    pub overlay: i32,
    /// 1 darken the sky, 2 boss music, 4 fog.
    pub flags: u8,
    pub viewers: HashSet<i32>,
}

/// The boss bars plugins made.
#[derive(Default)]
pub struct BossBars {
    pub bars: HashMap<Uuid, BossBar>,
}

fn color(name: &str) -> i32 {
    match name {
        "pink" => 0,
        "blue" => 1,
        "red" => 2,
        "green" => 3,
        "yellow" => 4,
        "purple" => 5,
        _ => 6,
    }
}

fn overlay(name: &str) -> i32 {
    match name {
        "notched_6" => 1,
        "notched_10" => 2,
        "notched_12" => 3,
        "notched_20" => 4,
        _ => 0,
    }
}

fn add(bar: &BossBar) -> BossEventAction {
    BossEventAction::Add { title: bar.title.clone(), health: bar.progress, color: bar.color, division: bar.overlay, flags: bar.flags }
}

fn send(server: &Server, viewers: impl IntoIterator<Item = i32>, packet: &BossEvent) {
    for v in viewers {
        if let Some(p) = server.players.get(&v) {
            p.conn.send(packet);
        }
    }
}

/// Executes a `bossbar.*` call.
pub fn handle(server: &mut Server, method: &str, args: &Value) -> Result<Value> {
    let id: Uuid = args.str("id").and_then(|s| s.parse().ok()).ok_or_else(|| anyhow!("missing boss bar id"))?;
    match method {
        // The look of a bar (created on first use); changes go to its viewers.
        "bossbar.update" => {
            let bar = server.plugin_data.boss_bars.bars.entry(id).or_insert_with(|| BossBar { title: Component::text(""), progress: 1.0, color: 6, overlay: 0, flags: 0, viewers: HashSet::new() });
            let mut actions = Vec::new();
            if let Some(title) = super::component_from(args.get("title")) {
                bar.title = title.clone();
                actions.push(BossEventAction::UpdateTitle(title));
            }
            if let Some(progress) = args.f64("progress") {
                bar.progress = (progress as f32).clamp(0.0, 1.0);
                actions.push(BossEventAction::UpdateHealth(bar.progress));
            }
            let (c, o) = (args.str("color").map(color), args.str("overlay").map(overlay));
            if c.is_some() || o.is_some() {
                bar.color = c.unwrap_or(bar.color);
                bar.overlay = o.unwrap_or(bar.overlay);
                actions.push(BossEventAction::UpdateStyle { color: bar.color, division: bar.overlay });
            }
            if let Some(flags) = args.i32("flags") {
                bar.flags = flags as u8;
                actions.push(BossEventAction::UpdateFlags(bar.flags));
            }
            let viewers: Vec<i32> = bar.viewers.iter().copied().collect();
            for action in actions {
                send(server, viewers.iter().copied(), &BossEvent { uuid: id, action });
            }
            Ok(Value::Null)
        }
        "bossbar.show" | "bossbar.hide" => {
            let player = args.str("player").and_then(|s| s.parse::<Uuid>().ok()).and_then(|u| server.by_uuid.get(&u).copied()).ok_or_else(|| anyhow!("unknown player"))?;
            let Some(bar) = server.plugin_data.boss_bars.bars.get_mut(&id) else { return Ok(Value::Bool(false)) };
            let changed = if method == "bossbar.show" { bar.viewers.insert(player) } else { bar.viewers.remove(&player) };
            if changed {
                let action = if method == "bossbar.show" { add(bar) } else { BossEventAction::Remove };
                send(server, [player], &BossEvent { uuid: id, action });
            }
            Ok(Value::Bool(changed))
        }
        "bossbar.remove" => {
            if let Some(bar) = server.plugin_data.boss_bars.bars.remove(&id) {
                send(server, bar.viewers, &BossEvent { uuid: id, action: BossEventAction::Remove });
            }
            Ok(Value::Null)
        }
        other => Err(anyhow!("unknown plugin API method {other}")),
    }
}

/// A player left: they see no bars any more.
pub fn player_left(server: &mut Server, player: i32) {
    for bar in server.plugin_data.boss_bars.bars.values_mut() {
        bar.viewers.remove(&player);
    }
}

/// The plugins that made the bars are gone (`/reload`, a crashed host): take them down.
pub fn clear(server: &mut Server) {
    let bars = std::mem::take(&mut server.plugin_data.boss_bars.bars);
    for (id, bar) in bars {
        send(server, bar.viewers, &BossEvent { uuid: id, action: BossEventAction::Remove });
    }
}
