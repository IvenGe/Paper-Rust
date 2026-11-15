//! World borders: each world's (vanilla's `WorldBorder`), the borders plugins give single
//! players instead (Paper's virtual borders, `Player#setWorldBorder`), and `/worldborder`.

use super::commands::CommandSource;
use super::Server;
use crate::world::{Border, MAX_BORDER_CENTER, MAX_BORDER_SIZE};
use bytes::Bytes;
use paper_protocol::codec::{VarInt, VarLong};
use paper_protocol::packets::{encode_packet, play};
use paper_protocol::text::Component;

/// `ClientboundInitializeBorderPacket` for a border.
pub fn border_init(b: &Border) -> play::InitializeBorder {
    play::InitializeBorder {
        center_x: b.center_x,
        center_z: b.center_z,
        old_size: b.size,
        new_size: target(b),
        lerp_time: VarLong(b.lerp_left.max(0)),
        absolute_max_size: VarInt(29_999_984),
        warning_blocks: VarInt(b.warning_blocks),
        warning_time: VarInt(b.warning_time),
    }
}

/// Where a border is going (`getLerpTarget`): its size when it is not moving.
fn target(b: &Border) -> f64 {
    if b.lerp_left > 0 {
        b.lerp_target
    } else {
        b.size
    }
}

/// The packets `CraftPlayer.setWorldBorder` sends to show a player a whole border.
pub fn border_sync(b: &Border) -> Vec<Bytes> {
    vec![
        encode_packet(&play::SetBorderSize { size: target(b) }).into(),
        lerp_packet(b),
        encode_packet(&play::SetBorderCenter { center_x: b.center_x, center_z: b.center_z }).into(),
        encode_packet(&play::SetBorderWarningDelay { warning_time: VarInt(b.warning_time) }).into(),
        encode_packet(&play::SetBorderWarningDistance { warning_blocks: VarInt(b.warning_blocks) }).into(),
    ]
}

/// `ClientboundSetBorderLerpSizePacket`: from the size now to where it is going.
fn lerp_packet(b: &Border) -> Bytes {
    encode_packet(&play::SetBorderLerpSize { old_size: b.size, new_size: target(b), lerp_time: VarLong(b.lerp_left.max(0)) }).into()
}

impl Server {
    /// `ClientboundInitializeBorderPacket` for a world's border.
    pub fn border_packet(&self, world: usize) -> play::InitializeBorder {
        border_init(&self.worlds[world].border)
    }

    /// The border a player is shown in `world`: their own if a plugin gave them one, else the
    /// world's. Paper sends the world's on respawn and world change even to a player with a
    /// border of their own (which `getWorldBorder` still returns); Paper-RS keeps showing it.
    pub fn border_packet_for(&self, id: i32, world: usize) -> play::InitializeBorder {
        match self.players.get(&id).and_then(|p| p.own_border.as_ref()) {
            Some(b) => border_init(b),
            None => self.border_packet(world),
        }
    }

    /// Sends changes of a world's border to the players in it who see it, not those with a
    /// border of their own (Paper's `PlayerList.broadcastWorldborder`).
    pub fn broadcast_border(&self, world: usize, payloads: &[Bytes]) {
        for p in self.players.values().filter(|p| p.world == world && p.own_border.is_none()) {
            for payload in payloads {
                p.conn.send_payload(payload.clone());
            }
        }
    }

    /// Moves borders on by a tick: each world's (plugins hear when one arrives,
    /// `WorldBorderBoundsChangeFinishEvent`) and players' own (`ServerPlayer.tick`); clients
    /// move theirs themselves.
    pub(super) fn tick_borders(&mut self) {
        let mut arrived = Vec::new();
        for (index, world) in self.worlds.iter_mut().enumerate() {
            let moving = world.border.lerp_left > 0;
            world.border.tick();
            if moving && world.border.lerp_left == 0 && !world.unloaded {
                arrived.push((index, world.border.lerp_from, world.border.lerp_target, world.border.lerp_duration));
            }
        }
        for p in self.players.values_mut() {
            if let Some(b) = &mut p.own_border {
                b.tick();
            }
        }
        for (world, from, to, duration) in arrived {
            crate::plugins::events::on_border_arrived(self, world, from, to, duration);
        }
    }

    /// `WorldBorder.setSize` with Paper's `WorldBorderBoundsChangeEvent`: plugins may stop
    /// it, change the size, or make it a move.
    fn resize_world_border(&mut self, world: usize, size: f64) {
        let old = self.worlds[world].border.size;
        let Some(change) = crate::plugins::events::on_border_resize(self, world, true, old, size, 0) else { return };
        if !change.instant && change.ticks > 0 {
            return self.lerp_world_border(world, change.from, change.size, change.ticks);
        }
        let b = &mut self.worlds[world].border;
        b.set_size(change.size, 0);
        let packet = encode_packet(&play::SetBorderSize { size: b.size }).into();
        self.broadcast_border(world, &[packet]);
    }

    /// `WorldBorder.lerpSizeBetween` with Paper's `WorldBorderBoundsChangeEvent`.
    fn lerp_world_border(&mut self, world: usize, from: f64, to: f64, ticks: i64) {
        let Some(change) = crate::plugins::events::on_border_resize(self, world, from == to, from, to, ticks) else { return };
        let b = &mut self.worlds[world].border;
        b.size = from;
        b.set_size(change.size, if from == change.size { 0 } else { change.ticks });
        let packet = lerp_packet(b);
        self.broadcast_border(world, &[packet]);
    }

    /// `WorldBorder.setCenter` with Paper's `WorldBorderCenterChangeEvent`.
    fn center_world_border(&mut self, world: usize, x: f64, z: f64) {
        let b = &self.worlds[world].border;
        let old = (b.center_x, b.center_z);
        let Some((x, z)) = crate::plugins::events::on_border_center(self, world, old, (x, z)) else { return };
        let b = &mut self.worlds[world].border;
        b.center_x = x;
        b.center_z = z;
        let packet = encode_packet(&play::SetBorderCenter { center_x: x, center_z: z }).into();
        self.broadcast_border(world, &[packet]);
    }

    /// `/worldborder` (vanilla's `WorldBorderCommand`), on the border of the source's world.
    pub(super) fn worldborder_command(&mut self, source: &CommandSource, args: &[&str]) {
        let (world, base) = match self.source_location(source) {
            Some((world, loc)) => (world, (loc.x, loc.z)),
            // The console stands at the overworld's spawn.
            None => (0, (self.worlds[0].spawn.x as f64, self.worlds[0].spawn.z as f64)),
        };
        let translated = |key: &str, args: Vec<String>| Component::translatable(key, args.into_iter().map(Component::text).collect());
        match args {
            ["get"] => {
                let size = self.worlds[world].border.size;
                self.feedback(source, translated("commands.worldborder.get", vec![format!("{size:.0}")]));
            }
            [op @ ("set" | "add"), distance, rest @ ..] if rest.len() <= 1 => {
                let Some(distance) = parse_number(self, source, distance, -MAX_BORDER_SIZE, MAX_BORDER_SIZE) else { return };
                let seconds = match rest.first() {
                    None => None,
                    Some(t) => match parse_number(self, source, t, 0.0, i32::MAX as f64) {
                        Some(t) if t.fract() == 0.0 => Some(t as i64),
                        Some(_) => return self.fail(source, format!("Invalid integer '{t}'")),
                        None => return,
                    },
                };
                let b = &self.worlds[world].border;
                let (size, ticks) = match (*op, seconds) {
                    ("set", seconds) => (distance, seconds.unwrap_or(0) * 20),
                    // Added time adds to what is left of a move.
                    (_, None) => (b.size + distance, 0),
                    (_, Some(seconds)) => (b.size + distance, b.lerp_left.max(0) + seconds * 20),
                };
                self.border_set_size(source, world, size, ticks);
            }
            ["center", x, z] => {
                let (Some(x), Some(z)) = (super::commands::parse_coordinate(x, base.0, true), super::commands::parse_coordinate(z, base.1, true)) else {
                    return self.fail(source, "Expected two coordinates");
                };
                let b = &self.worlds[world].border;
                if b.center_x == x && b.center_z == z {
                    return self.fail_component(source, translated("commands.worldborder.center.failed", vec![]));
                }
                if x.abs() > MAX_BORDER_CENTER || z.abs() > MAX_BORDER_CENTER {
                    return self.fail_component(source, translated("commands.worldborder.set.failed.far", vec!["2.9999984E7".into()]));
                }
                self.center_world_border(world, x, z);
                self.feedback(source, translated("commands.worldborder.center.success", vec![format!("{x:.2}"), format!("{z:.2}")]));
            }
            ["damage", which @ ("amount" | "buffer"), value] => {
                // A float argument, at least 0.
                let Some(value) = parse_number(self, source, value, 0.0, f32::MAX as f64) else { return };
                let value = value as f32 as f64;
                let b = &mut self.worlds[world].border;
                let (now, key) = match *which {
                    "amount" => (&mut b.damage_per_block, "commands.worldborder.damage.amount"),
                    _ => (&mut b.damage_safe_zone, "commands.worldborder.damage.buffer"),
                };
                if *now == value {
                    return self.fail_component(source, translated(&format!("{key}.failed"), vec![]));
                }
                *now = value;
                self.feedback(source, translated(&format!("{key}.success"), vec![format!("{value:.2}")]));
            }
            ["warning", which @ ("distance" | "time"), value] => {
                let value = match value.parse::<i32>() {
                    Ok(v) if v >= 0 => v,
                    Ok(v) => return self.fail(source, format!("Integer must not be less than 0, found {v}")),
                    Err(_) => return self.fail(source, format!("Invalid integer '{value}'")),
                };
                let b = &mut self.worlds[world].border;
                let (now, key) = match *which {
                    "distance" => (&mut b.warning_blocks, "commands.worldborder.warning.distance"),
                    _ => (&mut b.warning_time, "commands.worldborder.warning.time"),
                };
                if *now == value {
                    return self.fail_component(source, translated(&format!("{key}.failed"), vec![]));
                }
                *now = value;
                let packet: Bytes = if *which == "distance" {
                    encode_packet(&play::SetBorderWarningDistance { warning_blocks: VarInt(value) }).into()
                } else {
                    encode_packet(&play::SetBorderWarningDelay { warning_time: VarInt(value) }).into()
                };
                self.broadcast_border(world, &[packet]);
                self.feedback(source, translated(&format!("{key}.success"), vec![value.to_string()]));
            }
            _ => self.fail(source, "Usage: /worldborder <add|center|damage|get|set|warning> ..."),
        }
    }

    /// `WorldBorderCommand.setSize`: now, or over `ticks`.
    fn border_set_size(&mut self, source: &CommandSource, world: usize, size: f64, ticks: i64) {
        let translated = |key: &str, args: Vec<String>| Component::translatable(key, args.into_iter().map(Component::text).collect());
        let now = self.worlds[world].border.size;
        if now == size {
            return self.fail_component(source, translated("commands.worldborder.set.failed.nochange", vec![]));
        }
        if size < 1.0 {
            return self.fail_component(source, translated("commands.worldborder.set.failed.small", vec![]));
        }
        if size > MAX_BORDER_SIZE {
            return self.fail_component(source, translated("commands.worldborder.set.failed.big", vec!["5.9999968E7".into()]));
        }
        if ticks > 0 {
            self.lerp_world_border(world, now, size, ticks);
            let key = if size > now { "commands.worldborder.set.grow" } else { "commands.worldborder.set.shrink" };
            self.feedback(source, translated(key, vec![format!("{size:.1}"), (ticks / 20).to_string()]));
        } else {
            self.resize_world_border(world, size);
            self.feedback(source, translated("commands.worldborder.set.immediate", vec![format!("{size:.1}")]));
        }
    }
}

/// A number argument within `min..=max`, with brigadier's messages when it is not one.
fn parse_number(server: &Server, source: &CommandSource, token: &str, min: f64, max: f64) -> Option<f64> {
    match token.parse::<f64>() {
        Ok(v) if !v.is_finite() => {
            server.fail(source, format!("Invalid number '{token}'"));
            None
        }
        Ok(v) if v < min => {
            server.fail(source, format!("Number must not be less than {min}, found {token}"));
            None
        }
        Ok(v) if v > max => {
            server.fail(source, format!("Number must not be more than {max}, found {token}"));
            None
        }
        Ok(v) => Some(v),
        Err(_) => {
            server.fail(source, format!("Invalid number '{token}'"));
            None
        }
    }
}

/// Completions for `/worldborder`.
pub(super) fn complete(args: &[&str]) -> Vec<String> {
    let words: &[&str] = match args {
        [] | [_] => &["add", "center", "damage", "get", "set", "warning"],
        ["damage", _] => &["amount", "buffer"],
        ["warning", _] => &["distance", "time"],
        _ => &[],
    };
    let typed = args.last().copied().unwrap_or("");
    words.iter().filter(|w| w.starts_with(typed)).map(|w| (*w).to_owned()).collect()
}
