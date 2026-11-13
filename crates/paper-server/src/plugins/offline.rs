//! Offline players for the plugin API (`Bukkit.getOfflinePlayer`, `getOfflinePlayers`): what
//! the server knows about a player who may not be online, from the player data files
//! (`<world>/playerdata/<uuid>.dat`, with Bukkit's `bukkit.firstPlayed` / `lastPlayed` and
//! Paper's `Paper.LastLogin` / `LastSeen` timestamps), the name cache (`usercache.json`) and
//! the operator, ban and whitelist lists.

use crate::server::Server;
use anyhow::{anyhow, bail, Result};
use paper_bridge::{bmap, Value};
use std::time::UNIX_EPOCH;
use uuid::Uuid;

use crate::net::auth::offline_uuid;

/// The modification time of a player's data file in milliseconds, if it exists.
fn data_file_time(server: &Server, uuid: Uuid) -> Option<i64> {
    let storage = server.worlds.first()?.storage.as_ref()?;
    let path = storage.dir.join("playerdata").join(format!("{}.dat", uuid.hyphenated()));
    let modified = std::fs::metadata(path).ok()?.modified().ok()?;
    Some(modified.duration_since(UNIX_EPOCH).ok()?.as_millis() as i64)
}

/// Everything the host shows about one player.
pub fn info(server: &Server, uuid: Uuid, name: Option<&str>) -> Value {
    let online = server.player_by_uuid(uuid);
    let data = server.worlds.first().and_then(|w| w.storage.as_ref()).and_then(|s| s.read_player(uuid).ok().flatten());
    let bukkit = data.as_ref().and_then(|d| d.compound("bukkit"));
    let paper = data.as_ref().and_then(|d| d.compound("Paper"));
    let file_time = data_file_time(server, uuid);
    let session = server.user_cache.session(uuid);
    let name = online
        .map(|p| p.name().to_owned())
        .or_else(|| server.user_cache.by_uuid(uuid).map(|e| e.name.clone()))
        .or_else(|| bukkit.and_then(|b| b.string("lastKnownName")).map(str::to_owned))
        .or_else(|| name.map(str::to_owned));
    let now = chrono::Utc::now().timestamp_millis();
    let first_played = session.map(|s| s.first_played).or_else(|| bukkit.and_then(|b| b.i64("firstPlayed"))).or(file_time).unwrap_or(0);
    let last_played = if online.is_some() { Some(now) } else { bukkit.and_then(|b| b.i64("lastPlayed")).or(file_time) };
    let last_login = session.map(|s| s.last_login).or_else(|| paper.and_then(|p| p.i64("LastLogin"))).or(last_played);
    let last_seen = if online.is_some() { Some(now) } else { paper.and_then(|p| p.i64("LastSeen")).or(last_played) };
    let access = server.shared.access.read();
    let ban = access.ban_of(uuid);
    let mut out = bmap! {
        "uuid" => uuid.to_string(),
        "name" => name,
        "online" => online.is_some(),
        "played_before" => data.is_some() || session.is_some_and(|s| s.played_before),
        "first_played" => first_played,
        "last_played" => last_played.unwrap_or(0),
        "last_login" => last_login.unwrap_or(0),
        "last_seen" => last_seen.unwrap_or(0),
        "op" => access.op_level(uuid) > 0,
        "banned" => ban.is_some(),
        "whitelisted" => access.is_whitelisted(uuid),
    };
    if let Some(ban) = ban {
        out.set("ban", bmap! { "reason" => ban.reason.clone(), "source" => ban.source.clone(), "created" => ban.created.clone(), "expires" => ban.expires.clone() });
    }
    if let Some(d) = &data {
        if let Some(world) = d.string("paper-rs:world").or_else(|| d.string("Dimension")) {
            out.set("world", world.to_owned());
        }
        if let Some(pos) = d.list("Pos") {
            let coords: Vec<Value> = pos.iter().filter_map(|t| t.as_f64()).map(Value::Float).collect();
            if coords.len() == 3 {
                out.set("position", Value::List(coords));
            }
        }
    }
    out
}

/// The UUID a name stands for: an online player, the name cache, else the offline-mode UUID.
pub fn uuid_for_name(server: &Server, name: &str) -> (Uuid, bool) {
    if let Some(p) = server.player_by_name(name) {
        return (p.uuid(), true);
    }
    if let Some(entry) = server.user_cache.by_name(name) {
        return (entry.uuid, true);
    }
    (offline_uuid(name), false)
}

/// Every player with a data file, plus everyone in the name cache.
pub fn all(server: &Server) -> Vec<Uuid> {
    let mut out: Vec<Uuid> = Vec::new();
    if let Some(storage) = server.worlds.first().and_then(|w| w.storage.as_ref()) {
        if let Ok(dir) = std::fs::read_dir(storage.dir.join("playerdata")) {
            for entry in dir.flatten() {
                let name = entry.file_name();
                let Some(stem) = name.to_str().and_then(|n| n.strip_suffix(".dat")) else { continue };
                if let Ok(uuid) = stem.parse::<Uuid>() {
                    out.push(uuid);
                }
            }
        }
    }
    for entry in server.user_cache.entries() {
        if !out.contains(&entry.uuid) {
            out.push(entry.uuid);
        }
    }
    for p in server.online_players() {
        if !out.contains(&p.uuid()) {
            out.push(p.uuid());
        }
    }
    out
}

fn uuid_arg(args: &Value) -> Result<Uuid> {
    Ok(args.str("uuid").ok_or_else(|| anyhow!("missing uuid"))?.parse()?)
}

/// Executes an `offline.*` API call.
pub fn handle(server: &mut Server, method: &str, args: &Value) -> Result<Value> {
    match method {
        "offline.get" => {
            if let Some(name) = args.str("name").filter(|_| args.get("uuid").is_null()) {
                let (uuid, known) = uuid_for_name(server, name);
                if !known && args.bool("cached_only") == Some(true) {
                    return Ok(Value::Null);
                }
                return Ok(info(server, uuid, Some(name)));
            }
            Ok(info(server, uuid_arg(args)?, None))
        }
        "offline.list" => Ok(Value::List(all(server).into_iter().map(|u| info(server, u, None)).collect())),
        "offline.set_op" => {
            let uuid = uuid_arg(args)?;
            let name = info(server, uuid, args.str("name")).str("name").unwrap_or("").to_owned();
            if args.bool("op").unwrap_or(false) {
                let level = server.config.properties.op_permission_level;
                server.shared.access.write().op(uuid, &name, level);
                if let Some(id) = server.by_uuid.get(&uuid).copied() {
                    server.set_op_level(id, level);
                }
            } else {
                server.shared.access.write().deop(uuid);
                if let Some(id) = server.by_uuid.get(&uuid).copied() {
                    server.set_op_level(id, 0);
                }
            }
            Ok(Value::Null)
        }
        "offline.set_whitelisted" => {
            let uuid = uuid_arg(args)?;
            let name = info(server, uuid, args.str("name")).str("name").unwrap_or("").to_owned();
            let mut access = server.shared.access.write();
            if args.bool("value").unwrap_or(false) {
                access.whitelist_add(uuid, &name);
            } else if !name.is_empty() {
                access.whitelist_remove(&name);
            }
            Ok(Value::Null)
        }
        "offline.ban" => {
            let uuid = uuid_arg(args)?;
            let name = info(server, uuid, args.str("name")).str("name").unwrap_or("").to_owned();
            if name.is_empty() {
                bail!("cannot ban a player without a known name");
            }
            server.shared.access.write().ban(uuid, &name, args.str("source").unwrap_or("Plugin"), args.str("reason"));
            Ok(Value::Null)
        }
        "offline.pardon" => {
            let uuid = uuid_arg(args)?;
            let name = info(server, uuid, args.str("name")).str("name").unwrap_or("").to_owned();
            server.shared.access.write().pardon(&name);
            Ok(Value::Null)
        }
        other => bail!("unknown plugin API method {other}"),
    }
}
