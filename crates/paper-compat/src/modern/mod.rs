//! Clients of the versions before 26.3 back to 1.21.5: the same protocol with ids that moved,
//! packets that were added, split or reordered, and new fields. One pass turns a 26.3 packet
//! into the client's version, each change gated on the version that brought it. What changed
//! follows ViaBackwards' protocol steps, from `Protocol26_3To26_2` down to
//! `Protocol1_21_6To1_21_5`.

mod blocks;
mod entities;
mod items;
mod misc;

use crate::data::VersionData;
use crate::io::{self, Rw};
use crate::{Outcome, Translator, Version};
use anyhow::{bail, Result};
use paper_data::packets::{configuration as cfg, login, play};
use paper_protocol::codec::{Reader, WriteExt};
use paper_protocol::packets::State;
use std::collections::HashMap;

/// The protocols of the versions: a client older than one lacks what it brought.
pub(crate) const V26_2: i32 = 776;
pub(crate) const V26_1: i32 = 775;
pub(crate) const V1_21_11: i32 = 774;
pub(crate) const V1_21_9: i32 = 773;
pub(crate) const V1_21_6: i32 = 771;

/// One connection of a 26.2 or 26.1 client.
pub struct Modern {
    version: &'static Version,
    data: &'static VersionData,
    state: State,
    /// The 26.3 type of every entity the client was told about, for its entity data.
    entity_types: HashMap<i32, i32>,
    /// The bottom of the world the player is in (for beds, which 26.1 renders from block
    /// entities).
    min_y: i32,
    /// Section count of the player's world.
    sections: usize,
    /// A teleport the client confirmed, held until the position it sends next (26.3
    /// confirms both in one packet).
    teleport: Option<i32>,
    /// The open brewing stand's window: 26.3 sends two more of its properties.
    brewing_stand: Option<i32>,
    /// The world's age as the client counts it (from the time packets and its ticks): before
    /// 1.21.11, anger is sent as ticks left rather than an end time.
    game_time: i64,
    /// The time of day and whether it runs, last sent (before 26.1 the client needs one
    /// even where 26.3 has no clock).
    day_time: (i64, bool),
    /// Where the player looks (before 1.21.9, rotations are absolute).
    rotation: (f32, f32),
    pub(crate) game_mode: i32,
    /// The coordinate scale of the player's dimension (before 1.21.9, the world border's
    /// centre is scaled by it).
    coordinate_scale: f64,
}

impl Modern {
    pub fn new(version: &'static Version, data: &'static VersionData) -> Modern {
        Modern {
            version,
            data,
            state: State::Login,
            entity_types: HashMap::new(),
            min_y: -64,
            sections: 24,
            teleport: None,
            brewing_stand: None,
            game_time: 0,
            day_time: (6000, true),
            rotation: (0.0, 0.0),
            game_mode: 0,
            coordinate_scale: 1.0,
        }
    }

    fn protocol(&self) -> i32 {
        self.version.protocol
    }

    /// The old id of the 26.3 clientbound packet `id` in the current state.
    fn old_id(&self, id: i32) -> i32 {
        self.data.packets(self.state).clientbound.get(id as usize).copied().unwrap_or(-1)
    }

    /// The old id of a clientbound packet by its name (for packets 26.3 replaced).
    fn old_id_named(&self, name: &str) -> Result<i32> {
        match self.data.packets(self.state).clientbound_id(name) {
            Some(id) => Ok(id),
            None => bail!("{} has no {name} packet", self.version.name),
        }
    }

    /// The 26.3 id of an old serverbound packet.
    fn new_serverbound_id(&self, old: i32) -> i32 {
        self.data.packets(self.state).serverbound.get(old as usize).copied().unwrap_or(-1)
    }

    /// The id a registry entry has in the old version (-1: it has none). `registry` is the
    /// 26.3 registry's name.
    pub(crate) fn registry_id(&self, registry: &str, id: i32) -> i32 {
        match mapping_key(registry) {
            Some(key) => self.data.mapping(key).old(id),
            None if is_synced(registry) => self.data.registry(registry).old(id),
            None => self.data.mapping(&format!("registry:{registry}")).old(id),
        }
    }

    /// The 26.3 id of an old registry entry.
    pub(crate) fn registry_id_new(&self, registry: &str, old: i32) -> i32 {
        match mapping_key(registry) {
            Some(key) => self.data.mapping(key).current(old),
            None if is_synced(registry) => self.data.registry(registry).current(old),
            None => self.data.mapping(&format!("registry:{registry}")).current(old),
        }
    }

    fn login_clientbound(&mut self, id: i32, body: &[u8], out: &mut Outcome) -> Result<()> {
        if id == login::clientbound::LOGIN_FINISHED && self.protocol() < V26_2 {
            // 26.2 added the session id after the profile.
            let mut rw = Rw::new(body, id);
            rw.uuid()?;
            rw.string()?;
            let properties = rw.varint()?;
            for _ in 0..properties {
                rw.string()?;
                rw.string()?;
                rw.copy(|r| r.option(|r| r.string(1024)))?;
            }
            rw.r.uuid()?;
            out.to_client.push(rw.w);
            return Ok(());
        }
        out.to_client.push(io::with_id(id, body));
        Ok(())
    }

    fn configuration_clientbound(&mut self, id: i32, body: &[u8], out: &mut Outcome) -> Result<()> {
        let old = self.old_id(id);
        if id == cfg::clientbound::CODE_OF_CONDUCT && old < 0 {
            // Before 1.21.9 there is no code of conduct to agree to: agree for the client.
            let mut accept = Vec::new();
            accept.put_varint(cfg::serverbound::ACCEPT_CODE_OF_CONDUCT);
            out.to_server.push(accept);
            return Ok(());
        }
        if old < 0 {
            // `post_effects` and other packets the old version does not have.
            return Ok(());
        }
        match id {
            cfg::clientbound::SELECT_KNOWN_PACKS => out.to_client.push(self.known_packs(old, body)?),
            cfg::clientbound::REGISTRY_DATA => {
                if let Some(payload) = self.registry_data(old, body)? {
                    out.to_client.push(payload);
                }
            }
            cfg::clientbound::UPDATE_TAGS => out.to_client.push(self.tags(old, body)?),
            _ => out.to_client.push(io::with_id(old, body)),
        }
        Ok(())
    }

    /// Offers the vanilla data pack in every version a client of this protocol may have, so
    /// it uses its own copy of the registries (whose entries the server then lists).
    fn known_packs(&self, old: i32, body: &[u8]) -> Result<Vec<u8>> {
        let mut r = Reader::new(body);
        let packs = r.list(64, |r| Ok((r.string(256)?, r.string(256)?, r.string(256)?)))?;
        let mut w = Vec::new();
        w.put_varint(old);
        let mut offered = Vec::new();
        for (namespace, id, version) in packs {
            if namespace == "minecraft" && id == "core" {
                for v in &self.data.pack_versions {
                    offered.push((namespace.clone(), id.clone(), v.clone()));
                }
            } else {
                offered.push((namespace, id, version));
            }
        }
        w.put_varint(offered.len() as i32);
        for (namespace, id, version) in offered {
            w.put_string(&namespace);
            w.put_string(&id);
            w.put_string(&version);
        }
        Ok(w)
    }

    /// Lists only the entries the old version knows, and none of a registry it lacks.
    fn registry_data(&self, old: i32, body: &[u8]) -> Result<Option<Vec<u8>>> {
        use crate::data::RegistryMap;
        let mut r = Reader::new(body);
        let registry = r.identifier()?;
        let map = self.data.registry(&registry);
        if matches!(map, RegistryMap::Same) {
            return Ok(Some(io::with_id(old, body)));
        }
        if matches!(map, RegistryMap::Dropped) {
            return Ok(None);
        }
        let mut w = Vec::new();
        w.put_varint(old);
        w.put_string(&registry);
        let count = r.len(1 << 16)?;
        let mut kept = Vec::new();
        for index in 0..count {
            let before = r.peek();
            r.identifier()?;
            if r.bool()? {
                r.nbt()?;
            }
            let entry = &before[..before.len() - r.peek().len()];
            if map.keeps(index) {
                kept.push(entry);
            }
        }
        w.put_varint(kept.len() as i32);
        for entry in kept {
            w.extend_from_slice(entry);
        }
        Ok(Some(w))
    }

    /// Tags with the old ids, without entries or registries the old version lacks.
    fn tags(&self, old: i32, body: &[u8]) -> Result<Vec<u8>> {
        let mut r = Reader::new(body);
        let registries = r.len(1 << 12)?;
        let mut w = Vec::new();
        w.put_varint(old);
        let mut kept = Vec::new();
        for _ in 0..registries {
            let registry = r.identifier()?;
            let tags = r.list(1 << 16, |r| Ok((r.identifier()?, r.list(1 << 20, |r| r.varint())?)))?;
            if is_synced(&registry) && matches!(self.data.registry(&registry), crate::data::RegistryMap::Dropped) {
                continue;
            }
            let mut entry = Vec::new();
            entry.put_string(&registry);
            entry.put_varint(tags.len() as i32);
            for (name, ids) in tags {
                let name = if registry == "minecraft:block" { old_block_tag(self.protocol(), &name).map_or(name, str::to_owned) } else { name };
                entry.put_string(&name);
                let ids: Vec<i32> = ids.into_iter().map(|id| self.registry_id(&registry, id)).filter(|id| *id >= 0).collect();
                entry.put_varint(ids.len() as i32);
                for id in ids {
                    entry.put_varint(id);
                }
            }
            kept.push(entry);
        }
        w.put_varint(kept.len() as i32);
        for entry in kept {
            w.extend_from_slice(&entry);
        }
        Ok(w)
    }

    fn configuration_serverbound(&mut self, old: i32, body: &[u8], out: &mut Outcome) -> Result<()> {
        let id = self.new_serverbound_id(old);
        if id < 0 {
            return Ok(());
        }
        if id == cfg::serverbound::SELECT_KNOWN_PACKS {
            // Whichever version of the vanilla pack the client has is the one the server uses.
            let mut r = Reader::new(body);
            let packs = r.list(64, |r| Ok((r.string(256)?, r.string(256)?, r.string(256)?)))?;
            let mut w = Vec::new();
            w.put_varint(id);
            w.put_varint(packs.len() as i32);
            for (namespace, pack, version) in packs {
                let version = if namespace == "minecraft" && pack == "core" { paper_data::VERSION_NAME.to_owned() } else { version };
                w.put_string(&namespace);
                w.put_string(&pack);
                w.put_string(&version);
            }
            out.to_server.push(w);
        } else {
            out.to_server.push(io::with_id(id, body));
        }
        if id == cfg::serverbound::FINISH_CONFIGURATION {
            self.state = State::Play;
        }
        Ok(())
    }

    fn play_clientbound(&mut self, id: i32, body: &[u8], out: &mut Outcome) -> Result<()> {
        use play::clientbound as cb;
        let old = self.old_id(id);
        // A change of game mode (game event 3): attacks become a spectator's and back with it.
        if id == cb::GAME_EVENT {
            let mut r = Reader::new(body);
            if let (Ok(3), Ok(mode)) = (r.u8(), r.f32()) {
                self.game_mode = mode as i32;
            }
        }
        match id {
            cb::SWING_ANIMATION => return entities::swing(self, body, out),
            cb::ADD_TRANSIENT_BLOCK | cb::POST_EFFECTS => return Ok(()),
            _ if old < 0 => return Ok(()),
            _ => {}
        }
        let p = self.protocol();
        let payload = match id {
            cb::LOGIN | cb::RESPAWN => entities::spawn(self, id == cb::LOGIN, old, body)?,
            cb::SET_TIME => misc::set_time(self, old, body)?,
            cb::INITIALIZE_BORDER | cb::SET_BORDER_LERP_SIZE | cb::SET_BORDER_CENTER if p < V1_21_11 => misc::border(self, id, old, body)?,
            cb::UPDATE_MOB_EFFECT | cb::REMOVE_MOB_EFFECT => {
                let mut rw = Rw::new(body, old);
                rw.varint()?;
                let effect = rw.r.varint()?;
                let mapped = self.registry_id("minecraft:mob_effect", effect);
                if mapped < 0 {
                    return Ok(());
                }
                rw.w.put_varint(mapped);
                rw.finish()
            }
            cb::SET_ENTITY_MOTION if p < V1_21_9 => entities::entity_motion(old, body)?,
            cb::PLAYER_POSITION => misc::teleport(self, old, body)?,
            cb::PLAYER_ROTATION => misc::rotation(self, old, body)?,
            cb::SET_DEFAULT_SPAWN_POSITION if p < V1_21_9 => {
                let mut r = Reader::new(body);
                r.string(32767)?;
                let pos = r.position()?;
                let yaw = r.f32()?;
                let mut w = Vec::new();
                w.put_varint(old);
                w.put_position(pos);
                w.put_f32(yaw);
                w
            }
            cb::STOP_SOUND if p < V1_21_6 => misc::stop_sound(old, body)?,
            cb::LEVEL_CHUNK_WITH_LIGHT => blocks::chunk(self, old, body)?,
            cb::LIGHT_UPDATE => blocks::light_update(old, body)?,
            cb::BLOCK_UPDATE => blocks::block_update(self, old, body, out)?,
            cb::SECTION_BLOCKS_UPDATE => blocks::section_update(self, old, body, out)?,
            cb::BLOCK_EVENT => blocks::block_event(self, old, body)?,
            cb::BLOCK_ENTITY_DATA => blocks::block_entity_data(self, old, body)?,
            cb::LEVEL_EVENT => blocks::level_event(self, old, body)?,
            cb::OPEN_SIGN_EDITOR => blocks::open_sign_editor(old, body)?,
            cb::ADD_ENTITY => return entities::add_entity(self, old, body, out),
            cb::REMOVE_ENTITIES => entities::remove_entities(self, old, body)?,
            cb::SET_ENTITY_DATA => match entities::entity_data(self, old, body)? {
                Some(payload) => payload,
                None => return Ok(()),
            },
            cb::MOVE_ENTITY_POS | cb::MOVE_ENTITY_POS_ROT => entities::move_entity(old, body, id == cb::MOVE_ENTITY_POS_ROT)?,
            cb::MOVE_ENTITY_ROT => entities::move_entity_rot(old, body)?,
            cb::ENTITY_POSITION_SYNC => entities::position_sync(old, body)?,
            cb::ANIMATE => entities::animate(old, body)?,
            cb::UPDATE_ATTRIBUTES => entities::attributes(self, old, body)?,
            cb::DAMAGE_EVENT => entities::damage_event(self, old, body)?,
            cb::SET_EQUIPMENT => items::equipment(self, old, body)?,
            cb::CONTAINER_SET_CONTENT => items::container_content(self, old, body)?,
            cb::CONTAINER_SET_SLOT => items::container_slot(self, old, body)?,
            cb::SET_CURSOR_ITEM => items::cursor(self, old, body)?,
            cb::SET_PLAYER_INVENTORY => items::player_inventory(self, old, body)?,
            cb::MERCHANT_OFFERS => items::merchant_offers(self, old, body)?,
            cb::OPEN_SCREEN => items::open_screen(self, old, body)?,
            cb::CONTAINER_SET_DATA => match items::container_data(self, old, body)? {
                Some(payload) => payload,
                None => return Ok(()),
            },
            cb::UPDATE_RECIPES => items::update_recipes(self, old, body)?,
            cb::SOUND | cb::SOUND_ENTITY => misc::sound(self, old, body)?,
            cb::LEVEL_PARTICLES => return misc::particles(self, old, body, out),
            cb::EXPLODE => misc::explode(self, old, body)?,
            cb::COMMANDS => misc::commands(self, old, body)?,
            cb::UPDATE_TAGS => self.tags(old, body)?,
            cb::AWARD_STATS => misc::award_stats(self, old, body)?,
            cb::SET_PLAYER_TEAM => misc::player_team(self, old, body)?,
            cb::START_CONFIGURATION => {
                // The client answers with `configuration_acknowledged`, after which the
                // server talks configuration.
                io::with_id(old, body)
            }
            _ => io::with_id(old, body),
        };
        out.to_client.push(payload);
        Ok(())
    }

    fn play_serverbound(&mut self, old: i32, body: &[u8], out: &mut Outcome) -> Result<()> {
        use play::serverbound as sb;
        let packets = self.data.packets(State::Play);
        let name = packets.serverbound_names.get(old as usize).map(String::as_str).unwrap_or("");
        let p = self.protocol();
        match name {
            "minecraft:swing" => return entities::swing_serverbound(self, body, out),
            "minecraft:spectate_entity" => return entities::spectate(self, body, out),
            "minecraft:interact" if p < V26_1 => return entities::interact_serverbound(self, body, out),
            "minecraft:player_command" if p < V1_21_6 => {
                // 1.21.5 also sneaks with this (0, 1): 26.3 sneaks by the input packet.
                let mut r = Reader::new(body);
                let entity = r.varint()?;
                let action = r.varint()?;
                if action < 2 {
                    return Ok(());
                }
                let mut w = Vec::new();
                w.put_varint(sb::PLAYER_COMMAND);
                w.put_varint(entity);
                w.put_varint(action - 2);
                w.extend_from_slice(r.rest());
                out.to_server.push(w);
                return Ok(());
            }
            "minecraft:client_tick_end" => self.game_time += 1,
            "minecraft:move_player_pos_rot" | "minecraft:move_player_rot" => {
                let mut r = Reader::new(body);
                if name == "minecraft:move_player_pos_rot" {
                    r.take(24)?;
                }
                self.rotation = (r.f32()?, r.f32()?);
            }
            _ => {}
        }
        let id = self.new_serverbound_id(old);
        if id < 0 {
            return Ok(());
        }
        match id {
            sb::ACCEPT_TELEPORTATION => {
                // 26.3 confirms a teleport together with the position that follows it.
                self.teleport = Some(Reader::new(body).varint()?);
                return Ok(());
            }
            sb::MOVE_PLAYER_POS_ROT => {
                if let Some(teleport) = self.teleport.take() {
                    let mut r = Reader::new(body);
                    let (x, y, z, yaw, pitch) = (r.f64()?, r.f64()?, r.f64()?, r.f32()?, r.f32()?);
                    let mut w = Vec::new();
                    w.put_varint(sb::ACCEPT_TELEPORTATION);
                    w.put_varint(teleport);
                    w.put_f64(x);
                    w.put_f64(y);
                    w.put_f64(z);
                    w.put_f32(yaw);
                    w.put_f32(pitch);
                    out.to_server.push(w);
                    return Ok(());
                }
            }
            sb::PLAYER_ACTION => {
                // 26.3 added an action at 1 (changing the direction of a block being broken).
                let mut rw = Rw::new(body, id);
                rw.map_varint(|action| if action >= 1 { action + 1 } else { action })?;
                out.to_server.push(rw.finish());
                return Ok(());
            }
            sb::SIGN_UPDATE => {
                let mut rw = Rw::new(body, id);
                rw.copy(|r| r.position())?;
                // 26.2: the side, then the lines; 26.3: the lines, then the side.
                let front = rw.r.bool()?;
                for _ in 0..4 {
                    rw.copy(|r| r.string(384))?;
                }
                rw.w.put_varint(front as i32);
                out.to_server.push(rw.w);
                return Ok(());
            }
            sb::SET_CREATIVE_MODE_SLOT => return items::creative_slot(self, id, body, out),
            sb::CONTAINER_CLICK => return items::container_click(self, id, body, out),
            sb::CONFIGURATION_ACKNOWLEDGED => self.state = State::Configuration,
            _ => {}
        }
        out.to_server.push(io::with_id(id, body));
        Ok(())
    }
}

impl Translator for Modern {
    fn version(&self) -> &'static Version {
        self.version
    }

    fn clientbound(&mut self, payload: &[u8], out: &mut Outcome) -> Result<()> {
        let (id, body) = io::split(payload)?;
        match self.state {
            State::Login => self.login_clientbound(id, body, out),
            State::Configuration => self.configuration_clientbound(id, body, out),
            State::Play => self.play_clientbound(id, body, out),
            State::Handshake | State::Status => {
                out.to_client.push(payload.to_vec());
                Ok(())
            }
        }
    }

    fn serverbound(&mut self, payload: &[u8], out: &mut Outcome) -> Result<()> {
        let (old, body) = io::split(payload)?;
        match self.state {
            State::Login => {
                let id = self.new_serverbound_id(old);
                if id >= 0 {
                    out.to_server.push(io::with_id(id, body));
                }
                if id == login::serverbound::LOGIN_ACKNOWLEDGED {
                    self.state = State::Configuration;
                }
                Ok(())
            }
            State::Configuration => self.configuration_serverbound(old, body, out),
            State::Play => self.play_serverbound(old, body, out),
            State::Handshake | State::Status => {
                out.to_server.push(payload.to_vec());
                Ok(())
            }
        }
    }
}

/// A block tag's name in an older version, where it had another.
fn old_block_tag(protocol: i32, name: &str) -> Option<&'static str> {
    let short = name.strip_prefix("minecraft:")?;
    Some(match short {
        "convertible_to_mud" => "minecraft:convertable_to_mud",
        // Renamed in 26.1.
        "supports_dry_vegetation" if protocol < V26_1 => "minecraft:dry_vegetation_may_place_on",
        "supports_bamboo" if protocol < V26_1 => "minecraft:bamboo_plantable_on",
        "supports_small_dripleaf" if protocol < V26_1 => "minecraft:small_dripleaf_placeable",
        "supports_big_dripleaf" if protocol < V26_1 => "minecraft:big_dripleaf_placeable",
        "overrides_mushroom_light_requirement" if protocol < V26_1 => "minecraft:mushroom_grow_block",
        "support_override_snow_layer" if protocol < V26_1 => "minecraft:snow_layer_can_survive_on",
        "cannot_support_snow_layer" if protocol < V26_1 => "minecraft:snow_layer_cannot_survive_on",
        // Renamed in 1.21.6.
        "triggers_ambient_desert_dry_vegetation_block_sounds" if protocol < V1_21_6 => "minecraft:plays_ambient_desert_block_sounds",
        _ => return None,
    })
}

/// The key of a 26.3 id registry in the mapping files.
fn mapping_key(registry: &str) -> Option<&'static str> {
    Some(match registry {
        "minecraft:block" => "blocks",
        "minecraft:item" => "items",
        "minecraft:entity_type" => "entities",
        "minecraft:sound_event" => "sounds",
        "minecraft:particle_type" => "particles",
        "minecraft:data_component_type" => "data_component_type",
        "minecraft:command_argument_type" => "argumenttypes",
        "minecraft:custom_stat" => "statistics",
        "minecraft:recipe_serializer" => "recipe_serializers",
        "minecraft:attribute" => "attributes",
        "minecraft:block_entity_type" => "blockentities",
        _ => return None,
    })
}

/// Whether a registry is one of the data-driven ones the server sends in configuration.
fn is_synced(registry: &str) -> bool {
    paper_data::synced::SYNCED_REGISTRIES.iter().any(|(r, _)| *r == registry)
}
