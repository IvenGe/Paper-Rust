//! Writing Paper-RS plugins in Rust.
//!
//! A plugin is a library built as a `cdylib` (`crate-type = ["cdylib"]`) and put in the
//! server's `plugins` folder, next to the Java plugins. It implements [`Plugin`] and is
//! exported with [`export_plugin!`]:
//!
//! ```no_run
//! use paper_plugin::{export_plugin, Event, Info, Plugin, Sender, Server};
//!
//! #[derive(Default)]
//! struct Greeter;
//!
//! impl Plugin for Greeter {
//!     fn info(&self) -> Info {
//!         Info::new("Greeter", "1.0").listen("event.join").command("hello", "Says hello")
//!     }
//!
//!     fn event(&mut self, server: &mut Server, event: &mut Event) {
//!         if let Some(player) = event.player() {
//!             player.send_message(server, "Welcome, from a Rust plugin!");
//!         }
//!     }
//!
//!     fn command(&mut self, server: &mut Server, sender: &Sender, _label: &str, _args: &[String]) -> bool {
//!         sender.send_message(server, "Hello!");
//!         true
//!     }
//! }
//!
//! export_plugin!(Greeter);
//! ```
//!
//! Plugins run on the server's main thread, in-process, as Bukkit plugins run on Paper's
//! main thread: they see the game as it is and may change it at once.
//!
//! The server and a plugin talk through a small C interface ([`Entry`], [`Host`]) carrying
//! the same calls and values as the server's bridge to Java plugins (`paper-bridge`; see
//! `docs/plugin-bridge.md` for every call, event and value). So the whole API Java plugins
//! use is there ([`Server::call`]), a plugin need not be built with the server's compiler,
//! and it keeps working with servers of the same [`ABI_VERSION`].

use paper_bridge::{decode_value, encode_value};
pub use paper_bridge::{bmap, Value};
use std::ffi::c_void;
use std::panic::{catch_unwind, AssertUnwindSafe};

/// The version of the C interface. A server loads only plugins of its own version.
pub const ABI_VERSION: u32 = 1;

/// The symbol a plugin library exports: `extern "C" fn() -> Entry`.
pub const ENTRY_SYMBOL: &str = "paper_plugin_entry";

/// Bytes handed across the interface. The side that made a buffer frees it (with its own
/// `free`), so plugin and server may use different allocators.
#[repr(C)]
pub struct Buffer {
    pub ptr: *mut u8,
    pub len: usize,
    pub cap: usize,
}

impl Buffer {
    pub const EMPTY: Buffer = Buffer { ptr: std::ptr::null_mut(), len: 0, cap: 0 };

    pub fn from_vec(v: Vec<u8>) -> Buffer {
        let mut v = std::mem::ManuallyDrop::new(v);
        Buffer { ptr: v.as_mut_ptr(), len: v.len(), cap: v.capacity() }
    }

    /// The bytes of a buffer made by [`Buffer::from_vec`] (on this side of the interface).
    ///
    /// # Safety
    /// The buffer must come from `from_vec` of this side and not have been freed.
    pub unsafe fn into_vec(self) -> Vec<u8> {
        if self.ptr.is_null() {
            return Vec::new();
        }
        Vec::from_raw_parts(self.ptr, self.len, self.cap)
    }

    /// The bytes, borrowed.
    ///
    /// # Safety
    /// The buffer must be valid for `len` bytes.
    pub unsafe fn as_slice(&self) -> &[u8] {
        if self.ptr.is_null() {
            return &[];
        }
        std::slice::from_raw_parts(self.ptr, self.len)
    }
}

/// Frees a buffer this side made: the `free` of [`Host`] and [`Entry`].
///
/// # Safety
/// The buffer must come from [`Buffer::from_vec`] of this side.
pub unsafe extern "C" fn free_buffer(buffer: Buffer) {
    drop(buffer.into_vec());
}

/// A call across the interface: `ctx` (or the plugin), a method name, encoded arguments; the
/// encoded answer goes to `out`. Returns 0 if the call worked, 1 if it failed (`out` holds
/// the message as UTF-8), 2 if the plugin panicked.
pub type CallFn = unsafe extern "C" fn(target: *mut c_void, ctx: *mut c_void, method: *const u8, method_len: usize, args: *const u8, args_len: usize, out: *mut Buffer) -> i32;

/// What the server gives a plugin when it creates it.
#[repr(C)]
pub struct Host {
    pub abi: u32,
    /// Calls the server's API (`target` is unused, `ctx` the context of the server's call
    /// into the plugin that is running).
    pub call: CallFn,
    /// Frees the buffers `call` made.
    pub free: unsafe extern "C" fn(buffer: Buffer),
}

/// What a plugin library gives the server ([`ENTRY_SYMBOL`]).
#[repr(C)]
pub struct Entry {
    pub abi: u32,
    /// Makes the plugin; the server keeps the pointer and hands it back to `call`.
    pub create: unsafe extern "C" fn(host: *const Host) -> *mut c_void,
    /// Calls the plugin (`target` is the plugin, `ctx` what the plugin passes back to the
    /// server's `call` meanwhile): `plugin.info`, `plugin.enable`, `plugin.disable`,
    /// `plugin.event`, `plugin.command`, `plugin.complete`, `plugin.tick`.
    pub call: CallFn,
    pub destroy: unsafe extern "C" fn(plugin: *mut c_void),
    /// Frees the buffers `call` made.
    pub free: unsafe extern "C" fn(buffer: Buffer),
}

/// What a plugin is called, and what it wants of the server.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Info {
    pub name: String,
    pub version: String,
    pub description: String,
    pub authors: Vec<String>,
    /// The events it is given: a call of `docs/plugin-bridge.md` (`event.join`,
    /// `event.block_break`, ...) or the Bukkit event that call stands for
    /// (`BlockBreakEvent`, ...).
    pub events: Vec<String>,
    pub commands: Vec<CommandInfo>,
    /// Whether [`Plugin::tick`] is called every tick.
    pub ticks: bool,
}

/// A command a plugin adds.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CommandInfo {
    pub name: String,
    pub description: String,
    pub usage: String,
    pub aliases: Vec<String>,
    /// Who may use it (operators of this level and up; 0: everyone).
    pub op_level: u8,
}

impl Info {
    pub fn new(name: &str, version: &str) -> Info {
        Info { name: name.to_owned(), version: version.to_owned(), ..Info::default() }
    }

    pub fn description(mut self, text: &str) -> Info {
        self.description = text.to_owned();
        self
    }

    pub fn author(mut self, name: &str) -> Info {
        self.authors.push(name.to_owned());
        self
    }

    /// Gives the plugin an event (see [`Info::events`]).
    pub fn listen(mut self, event: &str) -> Info {
        self.events.push(event.to_owned());
        self
    }

    pub fn command(mut self, name: &str, description: &str) -> Info {
        self.commands.push(CommandInfo { name: name.to_owned(), description: description.to_owned(), usage: format!("/{name}"), ..CommandInfo::default() });
        self
    }

    /// Adds a command with aliases, usage and the operator level it needs.
    pub fn command_with(mut self, command: CommandInfo) -> Info {
        self.commands.push(command);
        self
    }

    /// Asks for [`Plugin::tick`] every tick.
    pub fn ticks(mut self) -> Info {
        self.ticks = true;
        self
    }

    pub fn to_value(&self) -> Value {
        let strings = |v: &[String]| Value::List(v.iter().map(|s| Value::Str(s.clone())).collect());
        let commands = self
            .commands
            .iter()
            .map(|c| bmap! { "name" => c.name.clone(), "description" => c.description.clone(), "usage" => c.usage.clone(), "aliases" => strings(&c.aliases), "op_level" => c.op_level as i64 })
            .collect();
        bmap! {
            "name" => self.name.clone(),
            "version" => self.version.clone(),
            "description" => self.description.clone(),
            "authors" => strings(&self.authors),
            "events" => strings(&self.events),
            "commands" => Value::List(commands),
            "ticks" => self.ticks,
        }
    }
}

/// A Rust plugin. Everything has a default but [`Plugin::info`].
pub trait Plugin: 'static {
    /// Name, version, events and commands. Asked once, before [`Plugin::enable`].
    fn info(&self) -> Info;

    /// The plugin starts (worlds are loaded; players may not be online yet).
    fn enable(&mut self, server: &mut Server) {
        let _ = server;
    }

    /// The server stops (or the plugin is taken out).
    fn disable(&mut self, server: &mut Server) {
        let _ = server;
    }

    /// An event the plugin listens to. Java plugins have had it first: [`Event::result`]
    /// holds what they decided, and what this plugin sets is the final word.
    fn event(&mut self, server: &mut Server, event: &mut Event) {
        let _ = (server, event);
    }

    /// One of the plugin's commands (by its name, whatever alias was typed). Returns false to
    /// show the usage.
    fn command(&mut self, server: &mut Server, sender: &Sender, label: &str, args: &[String]) -> bool {
        let _ = (server, sender, label, args);
        false
    }

    /// Suggestions for the last argument of one of its commands.
    fn complete(&mut self, server: &mut Server, sender: &Sender, label: &str, args: &[String]) -> Vec<String> {
        let _ = (server, sender, label, args);
        Vec::new()
    }

    /// Every tick, if [`Info::ticks`].
    fn tick(&mut self, server: &mut Server) {
        let _ = server;
    }
}

/// The server, while it runs the plugin.
pub struct Server {
    host: *const Host,
    ctx: *mut c_void,
    tick: i64,
}

/// What went wrong with a call into the server.
#[derive(Clone, Debug, PartialEq)]
pub struct CallError(pub String);

impl std::fmt::Display for CallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for CallError {}

impl Server {
    /// Calls the server's API: any call of `docs/plugin-bridge.md` ("Calls from the host to
    /// the server"), with its arguments as a map.
    pub fn call(&mut self, method: &str, args: Value) -> Result<Value, CallError> {
        let mut encoded = Vec::new();
        encode_value(&mut encoded, &args);
        let mut out = Buffer::EMPTY;
        // SAFETY: `host` is what the server gave `create` and outlives the plugin; `ctx` is
        // the context of the server's call that is running.
        let (status, bytes) = unsafe {
            let host = &*self.host;
            let status = (host.call)(std::ptr::null_mut(), self.ctx, method.as_ptr(), method.len(), encoded.as_ptr(), encoded.len(), &mut out);
            let bytes = out.as_slice().to_vec();
            (host.free)(out);
            (status, bytes)
        };
        if status != 0 {
            return Err(CallError(String::from_utf8_lossy(&bytes).into_owned()));
        }
        decode_value(&bytes).map_err(|e| CallError(format!("the server's answer to {method} could not be read: {e}")))
    }

    /// The tick the server is at (when it called the plugin).
    pub fn current_tick(&self) -> i64 {
        self.tick
    }

    /// Writes to the server log, under the plugin's name.
    pub fn log(&mut self, message: &str) {
        let _ = self.call("server.log", bmap! { "message" => message });
    }

    /// Sends a chat message to everyone (`text` may be a JSON text component).
    pub fn broadcast(&mut self, text: &str) {
        let _ = self.call("server.broadcast", bmap! { "message" => text });
    }

    /// Runs a command as the console.
    pub fn dispatch_command(&mut self, command: &str) -> bool {
        self.call("server.dispatch_command", bmap! { "command" => command }).is_ok()
    }

    /// The players online: their uuids and names.
    pub fn players(&mut self) -> Vec<Player> {
        let list = self.call("server.players", Value::Null).unwrap_or_default();
        list.as_list().unwrap_or(&[]).iter().filter_map(|p| Some(Player { uuid: p.str("uuid")?.to_owned(), name: p.str("name").unwrap_or("").to_owned() })).collect()
    }

    /// An online player by name.
    pub fn player(&mut self, name: &str) -> Option<Player> {
        self.players().into_iter().find(|p| p.name.eq_ignore_ascii_case(name))
    }

    /// The block state at a position (`minecraft:stone`, `minecraft:oak_stairs[facing=east,...]`).
    pub fn block(&mut self, world: &str, x: i32, y: i32, z: i32) -> Option<String> {
        let state = self.call("world.get_block", bmap! { "world" => world, "x" => x, "y" => y, "z" => z }).ok()?;
        state.str("state").or_else(|| state.as_str()).map(str::to_owned)
    }

    /// Sets a block (with block updates, as `Block#setBlockData`).
    pub fn set_block(&mut self, world: &str, x: i32, y: i32, z: i32, state: &str) -> Result<(), CallError> {
        self.call("world.set_block", bmap! { "world" => world, "x" => x, "y" => y, "z" => z, "state" => state }).map(|_| ())
    }
}

/// An online player.
#[derive(Clone, Debug, PartialEq)]
pub struct Player {
    pub uuid: String,
    pub name: String,
}

impl Player {
    pub fn send_message(&self, server: &mut Server, text: &str) {
        let _ = server.call("player.send_message", bmap! { "uuid" => self.uuid.clone(), "message" => text });
    }

    /// Shows a title (and subtitle) for the default times.
    pub fn send_title(&self, server: &mut Server, title: &str, subtitle: &str) {
        let _ = server.call("player.send_title", bmap! { "uuid" => self.uuid.clone(), "title" => title, "subtitle" => subtitle, "fade_in" => 10, "stay" => 70, "fade_out" => 20 });
    }

    pub fn kick(&self, server: &mut Server, reason: &str) {
        let _ = server.call("player.kick", bmap! { "uuid" => self.uuid.clone(), "reason" => reason });
    }

    /// Everything the server tells about the player (`player.state`: position, health, game
    /// mode, ...).
    pub fn state(&self, server: &mut Server) -> Value {
        server.call("player.state", bmap! { "uuid" => self.uuid.clone() }).unwrap_or_default()
    }

    pub fn teleport(&self, server: &mut Server, world: &str, x: f64, y: f64, z: f64) -> bool {
        server.call("player.teleport", bmap! { "uuid" => self.uuid.clone(), "world" => world, "x" => x, "y" => y, "z" => z, "yaw" => 0.0, "pitch" => 0.0 }).is_ok()
    }

    /// `survival`, `creative`, `adventure` or `spectator`.
    pub fn set_game_mode(&self, server: &mut Server, mode: &str) {
        let _ = server.call("player.set_game_mode", bmap! { "uuid" => self.uuid.clone(), "mode" => mode.to_ascii_uppercase() });
    }

    /// Runs a command as the player.
    pub fn perform_command(&self, server: &mut Server, command: &str) {
        let _ = server.call("player.perform_command", bmap! { "uuid" => self.uuid.clone(), "command" => command });
    }
}

/// Who ran a command.
#[derive(Clone, Debug, PartialEq)]
pub enum Sender {
    Console,
    Player(Player),
}

impl Sender {
    pub fn send_message(&self, server: &mut Server, text: &str) {
        match self {
            Sender::Console => server.log(text),
            Sender::Player(p) => p.send_message(server, text),
        }
    }

    pub fn player(&self) -> Option<&Player> {
        match self {
            Sender::Player(p) => Some(p),
            Sender::Console => None,
        }
    }
}

/// An event: its call (`event.join`) and Bukkit event, its arguments as
/// `docs/plugin-bridge.md` lists them, and the outcome so far.
#[derive(Clone, Debug, PartialEq)]
pub struct Event {
    /// The call (`event.block_break`).
    pub name: String,
    /// The Bukkit event it stands for, if the server names one (`BlockBreakEvent`).
    pub bukkit: String,
    pub args: Value,
    result: Value,
}

impl Event {
    pub fn new(name: &str, bukkit: &str, args: Value, result: Value) -> Event {
        let result = if result.as_map().is_some() { result } else { Value::map() };
        Event { name: name.to_owned(), bukkit: bukkit.to_owned(), args, result }
    }

    /// Whether this is the event called `name` (the call or the Bukkit event).
    pub fn is(&self, name: &str) -> bool {
        self.name == name || self.bukkit == name
    }

    /// An argument.
    pub fn get(&self, key: &str) -> &Value {
        self.args.get(key)
    }

    /// The player the event is about: `uuid` of the arguments, or of their `player`.
    pub fn player(&self) -> Option<Player> {
        let uuid = self.args.str("uuid").or_else(|| self.args.get("player").str("uuid")).or_else(|| self.args.get("player").str("player"))?;
        let name = self.args.get("player").str("name").unwrap_or("");
        Some(Player { uuid: uuid.to_owned(), name: name.to_owned() })
    }

    /// The outcome so far (what Java plugins decided, and this plugin since).
    pub fn result(&self) -> &Value {
        &self.result
    }

    /// Sets a field of the outcome (`message` of a join, `amount` of damage, ...).
    pub fn set(&mut self, key: &str, value: impl Into<Value>) {
        self.result.set(key, value);
    }

    pub fn is_cancelled(&self) -> bool {
        self.result.bool("cancelled") == Some(true)
    }

    pub fn set_cancelled(&mut self, cancelled: bool) {
        self.result.set("cancelled", cancelled);
    }

    pub fn cancel(&mut self) {
        self.set_cancelled(true);
    }

    pub fn into_result(self) -> Value {
        self.result
    }
}

/// The plugin and the server's interface, behind the pointer the server keeps.
struct Instance<T> {
    plugin: T,
    host: *const Host,
}

fn sender_from(server: &mut Server, value: &Value) -> Sender {
    match value.as_str() {
        Some(uuid) if uuid != "console" => {
            let name = server.players().into_iter().find(|p| p.uuid == uuid).map(|p| p.name).unwrap_or_default();
            Sender::Player(Player { uuid: uuid.to_owned(), name })
        }
        _ => Sender::Console,
    }
}

fn strings(value: &Value) -> Vec<String> {
    value.as_list().unwrap_or(&[]).iter().filter_map(|v| v.as_str().map(str::to_owned)).collect()
}

/// Runs one call of the server in the plugin.
fn dispatch<T: Plugin>(plugin: &mut T, server: &mut Server, method: &str, args: &Value) -> Result<Value, String> {
    server.tick = args.i64("tick").unwrap_or(server.tick);
    match method {
        "plugin.info" => Ok(plugin.info().to_value()),
        "plugin.enable" => {
            plugin.enable(server);
            Ok(Value::Null)
        }
        "plugin.disable" => {
            plugin.disable(server);
            Ok(Value::Null)
        }
        "plugin.event" => {
            let mut event = Event::new(args.str("name").unwrap_or(""), args.str("bukkit").unwrap_or(""), args.get("args").clone(), args.get("result").clone());
            plugin.event(server, &mut event);
            Ok(event.into_result())
        }
        "plugin.command" => {
            let sender = sender_from(server, args.get("sender"));
            let handled = plugin.command(server, &sender, args.str("label").unwrap_or(""), &strings(args.get("args")));
            Ok(bmap! { "handled" => handled })
        }
        "plugin.complete" => {
            let sender = sender_from(server, args.get("sender"));
            let matches = plugin.complete(server, &sender, args.str("label").unwrap_or(""), &strings(args.get("args")));
            Ok(Value::List(matches.into_iter().map(Value::Str).collect()))
        }
        "plugin.tick" => {
            plugin.tick(server);
            Ok(Value::Null)
        }
        other => Err(format!("unknown plugin call {other}")),
    }
}

#[doc(hidden)]
pub mod glue {
    //! The C functions [`export_plugin!`](crate::export_plugin) exports.
    use super::*;

    /// # Safety
    /// `host` must be a valid [`Host`] that outlives the plugin.
    pub unsafe extern "C" fn create<T: Plugin + Default>(host: *const Host) -> *mut c_void {
        match catch_unwind(T::default) {
            Ok(plugin) => Box::into_raw(Box::new(Instance { plugin, host })) as *mut c_void,
            Err(_) => std::ptr::null_mut(),
        }
    }

    /// # Safety
    /// `plugin` must come from `create::<T>`, the slices must be valid and `out` writable.
    pub unsafe extern "C" fn call<T: Plugin>(plugin: *mut c_void, ctx: *mut c_void, method: *const u8, method_len: usize, args: *const u8, args_len: usize, out: *mut Buffer) -> i32 {
        let instance = &mut *(plugin as *mut Instance<T>);
        let method = std::str::from_utf8(std::slice::from_raw_parts(method, method_len)).unwrap_or("");
        let args = if args_len == 0 { Value::Null } else { decode_value(std::slice::from_raw_parts(args, args_len)).unwrap_or_default() };
        let mut server = Server { host: instance.host, ctx, tick: 0 };
        let outcome = catch_unwind(AssertUnwindSafe(|| dispatch(&mut instance.plugin, &mut server, method, &args)));
        let (status, bytes) = match outcome {
            Ok(Ok(value)) => {
                let mut bytes = Vec::new();
                encode_value(&mut bytes, &value);
                (0, bytes)
            }
            Ok(Err(message)) => (1, message.into_bytes()),
            Err(panic) => {
                let message = panic.downcast_ref::<String>().cloned().or_else(|| panic.downcast_ref::<&str>().map(|s| (*s).to_owned())).unwrap_or_else(|| "the plugin panicked".to_owned());
                (2, message.into_bytes())
            }
        };
        *out = Buffer::from_vec(bytes);
        status
    }

    /// # Safety
    /// `plugin` must come from `create::<T>` and not be used again.
    pub unsafe extern "C" fn destroy<T: Plugin>(plugin: *mut c_void) {
        if !plugin.is_null() {
            let _ = catch_unwind(AssertUnwindSafe(|| drop(Box::from_raw(plugin as *mut Instance<T>))));
        }
    }
}

/// The interface of a plugin type: what a plugin library exports, and what a server built
/// with its plugins linked in registers.
pub fn entry<T: Plugin + Default>() -> Entry {
    Entry { abi: ABI_VERSION, create: glue::create::<T>, call: glue::call::<T>, destroy: glue::destroy::<T>, free: free_buffer }
}

/// Exports a plugin type (`Default`, implementing [`Plugin`]) from a `cdylib`.
#[macro_export]
macro_rules! export_plugin {
    ($plugin:ty) => {
        #[no_mangle]
        pub extern "C" fn paper_plugin_entry() -> $crate::Entry {
            $crate::entry::<$plugin>()
        }
    };
}
