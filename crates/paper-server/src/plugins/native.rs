//! Plugins written in Rust (see the `paper-plugin` crate): libraries in the plugins folder
//! (`.so`, `.dll`, `.dylib`), or plugins a server binary of one's own links in
//! (`paper_server::start_with_native_plugins`).
//!
//! They run on the main thread, in-process. The server calls them through the C interface of
//! `paper-plugin`, with the calls and values of the Java plugin bridge (`plugin.event` carries
//! an event's call, arguments and outcome as in `docs/plugin-bridge.md`), and they call the
//! same plugin API as Java plugins (`plugins::api`), plus `server.log` and `server.players`.
//!
//! A plugin that is running is not given events it causes itself (its call into the server
//! fires an event it listens to): it would be entered again while it runs, which Rust does not
//! allow. Java plugins see such events as usual.

use super::api;
use crate::server::commands::CommandSource;
use crate::server::Server;
use anyhow::{anyhow, bail, Context, Result};
use paper_bridge::{bmap, decode_value, encode_value, Value};
use paper_plugin::{free_buffer, Buffer, Entry, Host, ABI_VERSION, ENTRY_SYMBOL};
use std::collections::HashSet;
use std::ffi::c_void;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::Path;

/// A plugin's instance pointer. Plugins are only ever called on the main thread (the tick
/// pool, which shares the server, never reaches them), so the server stays `Sync`.
struct Instance(*mut c_void);

// SAFETY: see above; the pointer is only used on the main thread.
unsafe impl Send for Instance {}
unsafe impl Sync for Instance {}

/// A command a Rust plugin added.
#[derive(Clone, Debug)]
pub struct NativeCommand {
    pub name: String,
    pub description: String,
    pub usage: String,
    pub aliases: Vec<String>,
    pub op_level: u8,
}

/// A loaded Rust plugin.
pub struct NativePlugin {
    pub name: String,
    pub version: String,
    pub description: String,
    pub authors: Vec<String>,
    entry: Entry,
    instance: Instance,
    /// The library it came from, kept loaded as long as the plugin lives.
    library: Option<libloading::Library>,
    /// The events it is given, by call (`event.join`) or Bukkit event (`BlockBreakEvent`).
    pub events: HashSet<String>,
    pub commands: Vec<NativeCommand>,
    pub ticks: bool,
    pub enabled: bool,
    /// It is running (being entered again is refused).
    busy: bool,
}

// SAFETY: `Entry` holds function pointers only.
unsafe impl Send for NativePlugin {}
unsafe impl Sync for NativePlugin {}

/// The Rust plugins of the server, in load order.
#[derive(Default)]
pub struct NativePlugins {
    pub list: Vec<NativePlugin>,
}

impl NativePlugins {
    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }

    /// The Bukkit events some Rust plugin listens to (for `plugins::events::listens`).
    pub fn listened(&self) -> impl Iterator<Item = &String> {
        self.list.iter().filter(|p| p.enabled).flat_map(|p| p.events.iter())
    }

    /// The plugin and command a typed label stands for (`hello`, `hellorust:hello`, an alias).
    pub fn command(&self, label: &str) -> Option<(usize, &NativeCommand)> {
        let label = label.to_ascii_lowercase();
        let (namespace, bare) = match label.split_once(':') {
            Some((ns, bare)) => (Some(ns.to_owned()), bare.to_owned()),
            None => (None, label.clone()),
        };
        self.list.iter().enumerate().filter(|(_, p)| p.enabled && namespace.as_ref().is_none_or(|ns| *ns == p.name.to_ascii_lowercase())).find_map(|(i, p)| {
            p.commands.iter().find(|c| c.name.eq_ignore_ascii_case(&bare) || c.aliases.iter().any(|a| a.eq_ignore_ascii_case(&bare))).map(|c| (i, c))
        })
    }

    /// The command labels a player of operator level `op_level` may use, as the command tree
    /// shows them.
    pub fn labels(&self, op_level: u8) -> Vec<String> {
        let mut out = Vec::new();
        for p in self.list.iter().filter(|p| p.enabled) {
            for c in p.commands.iter().filter(|c| c.op_level <= op_level) {
                for name in std::iter::once(&c.name).chain(&c.aliases) {
                    out.push(name.to_ascii_lowercase());
                    out.push(format!("{}:{}", p.name.to_ascii_lowercase(), name.to_ascii_lowercase()));
                }
            }
        }
        out
    }
}

/// What a plugin's calls into the server run with.
struct CallContext {
    server: *mut Server,
    plugin: String,
}

static HOST: Host = Host { abi: ABI_VERSION, call: host_call, free: free_buffer };

/// A plugin calls the server.
unsafe extern "C" fn host_call(_target: *mut c_void, ctx: *mut c_void, method: *const u8, method_len: usize, args: *const u8, args_len: usize, out: *mut Buffer) -> i32 {
    let outcome = catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: `ctx` is the `CallContext` of the server's call into the plugin that is
        // running, whose server pointer is the only way to the server meanwhile.
        let call = unsafe { &mut *(ctx as *mut CallContext) };
        let server = unsafe { &mut *call.server };
        let method = std::str::from_utf8(unsafe { std::slice::from_raw_parts(method, method_len) }).map_err(|_| anyhow!("the call's name is not UTF-8"))?.to_owned();
        let args = if args_len == 0 { Value::Null } else { decode_value(unsafe { std::slice::from_raw_parts(args, args_len) }).map_err(|e| anyhow!("{e}"))? };
        native_api(server, &call.plugin, &method, &args)
    }));
    let (status, bytes) = match outcome {
        Ok(Ok(value)) => {
            let mut bytes = Vec::new();
            encode_value(&mut bytes, &value);
            (0, bytes)
        }
        Ok(Err(e)) => (1, format!("{e:#}").into_bytes()),
        Err(_) => (1, b"the server failed".to_vec()),
    };
    // SAFETY: the plugin passes a writable buffer.
    unsafe { *out = Buffer::from_vec(bytes) };
    status
}

/// The plugin API for Rust plugins: Java's (`plugins::api`) and a few calls of their own.
fn native_api(server: &mut Server, plugin: &str, method: &str, args: &Value) -> Result<Value> {
    match method {
        "server.log" => {
            log::info!("[{plugin}] {}", args.str("message").unwrap_or(""));
            Ok(Value::Null)
        }
        "server.players" => Ok(Value::List(server.player_order.iter().filter_map(|id| server.players.get(id)).map(|p| bmap! { "uuid" => p.uuid().to_string(), "name" => p.name() }).collect())),
        _ => api::handle(server, method, args),
    }
}

/// Calls plugin `index` of the server. Refuses if it is running already.
fn call_plugin(server: &mut Server, index: usize, method: &str, args: &Value) -> Result<Value> {
    let Some(p) = server.native.list.get_mut(index) else { bail!("no plugin {index}") };
    if p.busy {
        bail!("{} is running already", p.name);
    }
    p.busy = true;
    let (call, free, instance, name) = (p.entry.call, p.entry.free, p.instance.0, p.name.clone());
    let mut encoded = Vec::new();
    encode_value(&mut encoded, args);
    let server_ptr: *mut Server = server;
    let mut ctx = CallContext { server: server_ptr, plugin: name.clone() };
    let mut out = Buffer::EMPTY;
    // SAFETY: the plugin's entry points come from a library of our ABI version; `ctx` lives
    // through the call, and the server is reached only through it meanwhile.
    let (status, bytes) = unsafe {
        let status = call(instance, &mut ctx as *mut CallContext as *mut c_void, method.as_ptr(), method.len(), encoded.as_ptr(), encoded.len(), &mut out);
        let bytes = out.as_slice().to_vec();
        free(out);
        (status, bytes)
    };
    // SAFETY: the plugin returned; the pointer goes back to being the server.
    let server = unsafe { &mut *server_ptr };
    if let Some(p) = server.native.list.get_mut(index) {
        p.busy = false;
    }
    match status {
        0 => decode_value(&bytes).map_err(|e| anyhow!("{name}'s answer to {method} could not be read: {e}")),
        2 => bail!("{name} panicked in {method}: {}", String::from_utf8_lossy(&bytes)),
        _ => bail!("{name} failed in {method}: {}", String::from_utf8_lossy(&bytes)),
    }
}

/// Makes a plugin from its entry points (and the library they came from) and asks what it is.
fn instantiate(entry: Entry, library: Option<libloading::Library>, source: &str) -> Result<NativePlugin> {
    if entry.abi != ABI_VERSION {
        bail!("{source} is built for plugin interface {} (this server has {ABI_VERSION})", entry.abi);
    }
    // SAFETY: an entry of our ABI version.
    let instance = unsafe { (entry.create)(&HOST) };
    if instance.is_null() {
        bail!("{source} could not make its plugin");
    }
    Ok(NativePlugin {
        name: String::new(),
        version: String::new(),
        description: String::new(),
        authors: Vec::new(),
        entry,
        instance: Instance(instance),
        library,
        events: HashSet::new(),
        commands: Vec::new(),
        ticks: false,
        enabled: false,
        busy: false,
    })
}

/// Loads a plugin library.
fn load_library(path: &Path) -> Result<NativePlugin> {
    // SAFETY: loading a library runs its initialisers; plugins are trusted code, as Java
    // plugins are.
    let library = unsafe { libloading::Library::new(path) }.with_context(|| format!("loading {}", path.display()))?;
    // SAFETY: the symbol is the plugin's entry function (`export_plugin!`).
    let entry = unsafe {
        let function: libloading::Symbol<unsafe extern "C" fn() -> Entry> = library.get(ENTRY_SYMBOL.as_bytes()).with_context(|| format!("{} has no {ENTRY_SYMBOL}: not a Paper-RS plugin", path.display()))?;
        function()
    };
    instantiate(entry, Some(library), &path.display().to_string())
}

fn is_library(path: &Path) -> bool {
    matches!(path.extension().and_then(|e| e.to_str()), Some("so" | "dll" | "dylib"))
}

/// Adds a plugin and asks it what it is. Returns its index.
fn add(server: &mut Server, plugin: NativePlugin) -> Result<usize> {
    server.native.list.push(plugin);
    let index = server.native.list.len() - 1;
    let info = match call_plugin(server, index, "plugin.info", &Value::Null) {
        Ok(info) => info,
        Err(e) => {
            remove(server, index);
            return Err(e);
        }
    };
    let strings = |v: &Value| v.as_list().unwrap_or(&[]).iter().filter_map(|s| s.as_str().map(str::to_owned)).collect::<Vec<_>>();
    let name = info.str("name").unwrap_or("").to_owned();
    if name.is_empty() || server.native.list[..index].iter().any(|p| p.name.eq_ignore_ascii_case(&name)) {
        remove(server, index);
        bail!("a Rust plugin needs a name of its own (got {name:?})");
    }
    let p = &mut server.native.list[index];
    p.name = name;
    p.version = info.str("version").unwrap_or("").to_owned();
    p.description = info.str("description").unwrap_or("").to_owned();
    p.authors = strings(info.get("authors"));
    p.events = strings(info.get("events")).into_iter().collect();
    p.ticks = info.bool("ticks").unwrap_or(false);
    p.commands = info
        .list("commands")
        .unwrap_or(&[])
        .iter()
        .filter_map(|c| {
            Some(NativeCommand {
                name: c.str("name")?.to_ascii_lowercase(),
                description: c.str("description").unwrap_or("").to_owned(),
                usage: c.str("usage").unwrap_or("").to_owned(),
                aliases: strings(c.get("aliases")),
                op_level: c.i64("op_level").unwrap_or(0).clamp(0, 4) as u8,
            })
        })
        .collect();
    Ok(index)
}

/// Destroys plugin `index` and drops it (its library goes last).
fn remove(server: &mut Server, index: usize) {
    let plugin = server.native.list.remove(index);
    // SAFETY: made by this entry's `create`, not used again.
    unsafe { (plugin.entry.destroy)(plugin.instance.0) };
    drop(plugin.library);
}

/// Loads the Rust plugins of the plugins folder, and those linked into the server binary.
/// They are enabled with `enable_all` once the worlds are there.
pub fn load(server: &mut Server, linked: Vec<Entry>) {
    for (i, entry) in linked.into_iter().enumerate() {
        match instantiate(entry, None, &format!("linked plugin {i}")).and_then(|p| add(server, p)) {
            Ok(index) => log::info!("Loaded Rust plugin {} v{}", server.native.list[index].name, server.native.list[index].version),
            Err(e) => log::error!("Could not load a Rust plugin: {e:#}"),
        }
    }
    let dir = server.config.resolve(&server.config.paper.plugins.plugins_dir);
    let Ok(read) = std::fs::read_dir(&dir) else { return };
    let mut paths: Vec<_> = read.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| is_library(p)).collect();
    paths.sort();
    for path in paths {
        match load_library(&path).and_then(|p| add(server, p)) {
            Ok(index) => log::info!("Loaded Rust plugin {} v{} ({})", server.native.list[index].name, server.native.list[index].version, path.display()),
            Err(e) => log::error!("Could not load Rust plugin {}: {e:#}", path.display()),
        }
    }
}

/// Enables the loaded Rust plugins (`plugin.enable`); a plugin that fails stays disabled.
pub fn enable_all(server: &mut Server) {
    for index in 0..server.native.list.len() {
        server.native.list[index].enabled = true;
        let name = server.native.list[index].name.clone();
        log::info!("Enabling {name} v{}", server.native.list[index].version);
        if let Err(e) = call_plugin(server, index, "plugin.enable", &Value::Null) {
            log::error!("Error enabling {name}: {e:#}");
            server.native.list[index].enabled = false;
        }
    }
    super::events::refresh_listened(server);
}

/// Disables and unloads every Rust plugin (at shutdown).
pub fn shutdown(server: &mut Server) {
    for index in (0..server.native.list.len()).rev() {
        if server.native.list[index].enabled {
            let name = server.native.list[index].name.clone();
            log::info!("Disabling {name}");
            if let Err(e) = call_plugin(server, index, "plugin.disable", &Value::Null) {
                log::error!("Error disabling {name}: {e:#}");
            }
            server.native.list[index].enabled = false;
        }
    }
    while !server.native.list.is_empty() {
        let last = server.native.list.len() - 1;
        remove(server, last);
    }
    super::events::refresh_listened(server);
}

/// The Bukkit event of the calls the server makes whatever Java plugins listen to.
fn bukkit_name(method: &str) -> &'static str {
    match method {
        "event.join" => "PlayerJoinEvent",
        "event.quit" => "PlayerQuitEvent",
        "event.inventory_click" => "InventoryClickEvent",
        "event.entity_damage" => "EntityDamageEvent",
        "event.pickup_item" => "EntityPickupItemEvent",
        "event.mount" => "EntityMountEvent",
        "event.dismount" => "EntityDismountEvent",
        "event.knockback" => "EntityKnockbackEvent",
        "event.sign_open" => "PlayerOpenSignEvent",
        "event.hanging_break" => "HangingBreakEvent",
        "event.entity_target" => "EntityTargetEvent",
        "event.egg" => "PlayerEggThrowEvent",
        _ => "",
    }
}

/// Gives an event to the Rust plugins that listen to it, after Java plugins (`java`: what
/// they decided). The outcome starts as theirs, or as the event's arguments (whose fields
/// have the names the outcome uses), and each plugin may change it. `None` if no plugin
/// heard of it.
pub fn fire(server: &mut Server, event: &str, method: &str, args: &Value, java: Option<Value>) -> Option<Value> {
    let bukkit = if event.is_empty() { bukkit_name(method) } else { event };
    let listeners: Vec<usize> = server
        .native
        .list
        .iter()
        .enumerate()
        .filter(|(_, p)| p.enabled && !p.busy && (p.events.contains(method) || (!bukkit.is_empty() && p.events.contains(bukkit))))
        .map(|(i, _)| i)
        .collect();
    if listeners.is_empty() {
        return java;
    }
    let mut result = java.unwrap_or_else(|| if args.as_map().is_some() { args.clone() } else { Value::map() });
    for index in listeners {
        let call = bmap! { "name" => method, "bukkit" => bukkit, "args" => args.clone(), "result" => result.clone(), "tick" => server.tick as i64 };
        match call_plugin(server, index, "plugin.event", &call) {
            Ok(answer) if answer.as_map().is_some() => result = answer,
            Ok(_) => {}
            Err(e) => log::error!("Could not pass {method} to a Rust plugin: {e:#}"),
        }
    }
    Some(result)
}

/// Every tick: the plugins that asked for it.
pub fn tick(server: &mut Server) {
    let tick = server.tick as i64;
    for index in 0..server.native.list.len() {
        let p = &server.native.list[index];
        if p.enabled && p.ticks {
            if let Err(e) = call_plugin(server, index, "plugin.tick", &bmap! { "tick" => tick }) {
                log::error!("{e:#}");
            }
        }
    }
}

fn sender_value(server: &Server, source: &CommandSource) -> Value {
    match source {
        CommandSource::Console => Value::Str("console".into()),
        CommandSource::Player(id) => server.players.get(id).map_or(Value::Str("console".into()), |p| Value::Str(p.uuid().to_string())),
    }
}

fn op_level(server: &Server, source: &CommandSource) -> u8 {
    match source {
        CommandSource::Console => 4,
        CommandSource::Player(id) => server.players.get(id).map_or(0, |p| p.op_level),
    }
}

/// Runs a Rust plugin's command. `None` if the label is none of theirs.
pub fn run_command(server: &mut Server, source: &CommandSource, line: &str) -> Option<bool> {
    let mut words = line.split_whitespace();
    let label = words.next()?;
    let (index, command) = server.native.command(label).map(|(i, c)| (i, c.clone()))?;
    if op_level(server, source) < command.op_level {
        server.feedback(source, paper_protocol::text::Component::text("I'm sorry, but you do not have permission to perform this command. Please contact the server administrators if you believe that this is in error.").color("red"));
        return Some(true);
    }
    let args: Vec<Value> = words.map(|w| Value::Str(w.to_owned())).collect();
    let call = bmap! { "sender" => sender_value(server, source), "label" => command.name.clone(), "args" => Value::List(args), "tick" => server.tick as i64 };
    match call_plugin(server, index, "plugin.command", &call) {
        Ok(answer) if answer.bool("handled") == Some(true) => {}
        Ok(_) if !command.usage.is_empty() => server.feedback(source, paper_protocol::text::Component::text(command.usage.clone())),
        Ok(_) => {}
        Err(e) => {
            log::error!("{e:#}");
            server.feedback(source, paper_protocol::text::Component::text("An internal error occurred while attempting to perform this command").color("red"));
        }
    }
    Some(true)
}

/// Suggestions for a Rust plugin's command: `(start, length, matches)`.
pub fn suggestions(server: &mut Server, source: &CommandSource, text: &str) -> Option<(i32, i32, Vec<String>)> {
    let line = text.strip_prefix('/').unwrap_or(text);
    let label = line.split_whitespace().next()?;
    let (index, command) = server.native.command(label).map(|(i, c)| (i, c.clone()))?;
    if op_level(server, source) < command.op_level || !line.contains(' ') {
        return None;
    }
    let rest = &line[label.len()..];
    let mut args: Vec<String> = rest.split_whitespace().map(str::to_owned).collect();
    if rest.ends_with(' ') {
        args.push(String::new());
    }
    let last = args.last().cloned().unwrap_or_default();
    // Positions count UTF-16 units, as the client's do.
    let start = (text.encode_utf16().count() - last.encode_utf16().count()) as i32;
    let call = bmap! { "sender" => sender_value(server, source), "label" => command.name.clone(), "args" => Value::List(args.into_iter().map(Value::Str).collect()), "tick" => server.tick as i64 };
    let answer = call_plugin(server, index, "plugin.complete", &call).ok()?;
    let matches = answer.as_list().unwrap_or(&[]).iter().filter_map(|m| m.as_str().map(str::to_owned)).collect();
    Some((start, last.encode_utf16().count() as i32, matches))
}
