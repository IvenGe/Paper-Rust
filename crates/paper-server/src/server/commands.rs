//! Commands: the registry, the built-in commands and the command graph sent to clients.
//!
//! Built-in commands are plain functions over the server. Commands registered by Java
//! plugins live in the plugin host; the registry only knows their names and forwards
//! their execution and tab completion over the bridge.

use super::player::Location;
use super::Server;
use crate::config::{Difficulty, GameMode};
use paper_data::{blocks, items};
use paper_protocol::codec::VarInt;
use paper_protocol::packets::play::{self, CommandNode, Commands, Parser};
use paper_protocol::text::Component;
use std::collections::HashMap;

/// Who is running a command.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CommandSource {
    Console,
    Player(i32),
}

type Handler = fn(&mut Server, &CommandSource, &[&str]);
type Completer = fn(&Server, &CommandSource, &[&str]) -> Vec<String>;

/// A command implemented by the server itself.
pub struct Builtin {
    pub name: &'static str,
    pub aliases: &'static [&'static str],
    pub description: &'static str,
    pub usage: &'static str,
    /// Operator level a player needs (0 = everyone).
    pub min_op: u8,
    handler: Handler,
    completer: Completer,
}

pub struct CommandRegistry {
    builtins: Vec<Builtin>,
    by_label: HashMap<String, usize>,
}

fn no_completions(_: &Server, _: &CommandSource, _: &[&str]) -> Vec<String> {
    Vec::new()
}

fn complete_players(server: &Server, _: &CommandSource, args: &[&str]) -> Vec<String> {
    if args.len() <= 1 {
        player_names(server)
    } else {
        Vec::new()
    }
}

fn player_names(server: &Server) -> Vec<String> {
    server.online_players().map(|p| p.name().to_owned()).collect()
}

impl CommandRegistry {
    pub fn with_builtins() -> Self {
        let builtins = vec![
            Builtin { name: "help", aliases: &["?"], description: "Shows the available commands", usage: "/help", min_op: 0, handler: cmd_help, completer: no_completions },
            Builtin { name: "list", aliases: &[], description: "Lists players on the server", usage: "/list", min_op: 0, handler: cmd_list, completer: no_completions },
            Builtin { name: "me", aliases: &[], description: "Displays a message about yourself", usage: "/me <action>", min_op: 0, handler: cmd_me, completer: no_completions },
            Builtin { name: "msg", aliases: &["tell", "w"], description: "Sends a private message", usage: "/msg <player> <message>", min_op: 0, handler: cmd_msg, completer: complete_players },
            Builtin { name: "say", aliases: &[], description: "Broadcasts a message", usage: "/say <message>", min_op: 2, handler: cmd_say, completer: no_completions },
            Builtin { name: "gamemode", aliases: &[], description: "Sets a player's game mode", usage: "/gamemode <mode> [player]", min_op: 2, handler: cmd_gamemode, completer: complete_gamemode },
            Builtin { name: "defaultgamemode", aliases: &[], description: "Sets the default game mode", usage: "/defaultgamemode <mode>", min_op: 2, handler: cmd_default_gamemode, completer: complete_gamemode },
            Builtin { name: "tp", aliases: &["teleport"], description: "Teleports players", usage: "/tp [player] <target | x y z>", min_op: 2, handler: cmd_teleport, completer: complete_players },
            Builtin { name: "give", aliases: &[], description: "Gives an item to a player", usage: "/give <player> <item> [count]", min_op: 2, handler: cmd_give, completer: complete_give },
            Builtin { name: "clear", aliases: &[], description: "Clears a player's inventory", usage: "/clear [player]", min_op: 2, handler: cmd_clear, completer: complete_players },
            Builtin { name: "setblock", aliases: &[], description: "Changes a block", usage: "/setblock <x> <y> <z> <block>", min_op: 2, handler: cmd_setblock, completer: complete_setblock },
            Builtin { name: "time", aliases: &[], description: "Changes or queries the world's time", usage: "/time <set|add|query> <value>", min_op: 2, handler: cmd_time, completer: complete_time },
            Builtin { name: "difficulty", aliases: &[], description: "Sets the difficulty", usage: "/difficulty [peaceful|easy|normal|hard]", min_op: 2, handler: cmd_difficulty, completer: complete_difficulty },
            Builtin { name: "seed", aliases: &[], description: "Displays the world seed", usage: "/seed", min_op: 2, handler: cmd_seed, completer: no_completions },
            Builtin { name: "setworldspawn", aliases: &[], description: "Sets the world spawn", usage: "/setworldspawn [x y z]", min_op: 2, handler: cmd_set_world_spawn, completer: no_completions },
            Builtin { name: "tps", aliases: &[], description: "Shows the server's ticks per second", usage: "/tps", min_op: 2, handler: cmd_tps, completer: no_completions },
            Builtin { name: "mspt", aliases: &[], description: "Shows the average tick duration", usage: "/mspt", min_op: 2, handler: cmd_mspt, completer: no_completions },
            Builtin { name: "kick", aliases: &[], description: "Removes a player from the server", usage: "/kick <player> [reason]", min_op: 3, handler: cmd_kick, completer: complete_players },
            Builtin { name: "ban", aliases: &[], description: "Bans a player", usage: "/ban <player> [reason]", min_op: 3, handler: cmd_ban, completer: complete_players },
            Builtin { name: "pardon", aliases: &[], description: "Lifts a player ban", usage: "/pardon <player>", min_op: 3, handler: cmd_pardon, completer: no_completions },
            Builtin { name: "ban-ip", aliases: &[], description: "Bans an IP address", usage: "/ban-ip <address | player> [reason]", min_op: 3, handler: cmd_ban_ip, completer: complete_players },
            Builtin { name: "pardon-ip", aliases: &[], description: "Lifts an IP ban", usage: "/pardon-ip <address>", min_op: 3, handler: cmd_pardon_ip, completer: no_completions },
            Builtin { name: "banlist", aliases: &[], description: "Lists bans", usage: "/banlist [ips|players]", min_op: 3, handler: cmd_banlist, completer: no_completions },
            Builtin { name: "op", aliases: &[], description: "Makes a player an operator", usage: "/op <player>", min_op: 3, handler: cmd_op, completer: complete_players },
            Builtin { name: "deop", aliases: &[], description: "Revokes operator status", usage: "/deop <player>", min_op: 3, handler: cmd_deop, completer: complete_players },
            Builtin { name: "whitelist", aliases: &[], description: "Manages the whitelist", usage: "/whitelist <on|off|list|add|remove|reload> [player]", min_op: 3, handler: cmd_whitelist, completer: complete_whitelist },
            Builtin { name: "save-all", aliases: &[], description: "Saves the world", usage: "/save-all", min_op: 4, handler: cmd_save_all, completer: no_completions },
            Builtin { name: "stop", aliases: &[], description: "Stops the server", usage: "/stop", min_op: 4, handler: cmd_stop, completer: no_completions },
            Builtin { name: "version", aliases: &["ver", "about"], description: "Shows the server version", usage: "/version", min_op: 0, handler: cmd_version, completer: no_completions },
            Builtin { name: "plugins", aliases: &["pl"], description: "Lists the loaded plugins", usage: "/plugins", min_op: 0, handler: cmd_plugins, completer: no_completions },
        ];
        let mut by_label = HashMap::new();
        for (i, b) in builtins.iter().enumerate() {
            by_label.insert(b.name.to_owned(), i);
            for alias in b.aliases {
                by_label.insert((*alias).to_owned(), i);
            }
        }
        Self { builtins, by_label }
    }

    /// Looks a command up by label, accepting the `minecraft:` namespace.
    pub fn find(&self, label: &str) -> Option<&Builtin> {
        let label = label.to_ascii_lowercase();
        let label = label.strip_prefix("minecraft:").or_else(|| label.strip_prefix("bukkit:")).unwrap_or(&label);
        self.by_label.get(label).map(|i| &self.builtins[*i])
    }

    pub fn builtins(&self) -> &[Builtin] {
        &self.builtins
    }
}

impl Server {
    /// Operator level of a command source (the console has every permission).
    pub fn source_op_level(&self, source: &CommandSource) -> u8 {
        match source {
            CommandSource::Console => 4,
            CommandSource::Player(id) => self.players.get(id).map_or(0, |p| p.op_level),
        }
    }

    /// The name a command source is referred to by.
    pub fn source_name(&self, source: &CommandSource) -> String {
        match source {
            CommandSource::Console => "Server".to_owned(),
            CommandSource::Player(id) => self.players.get(id).map_or_else(|| "?".to_owned(), |p| p.name().to_owned()),
        }
    }

    /// Sends command output back to whoever ran the command.
    pub fn feedback(&self, source: &CommandSource, message: Component) {
        match source {
            CommandSource::Console => log::info!("{}", message.to_ansi()),
            CommandSource::Player(id) => self.send_message(*id, &message),
        }
    }

    fn fail(&self, source: &CommandSource, message: impl Into<String>) {
        self.feedback(source, Component::text(message.into()).color("red"));
    }

    /// Parses and runs a command line (with or without the leading slash), like Bukkit's
    /// `Server#dispatchCommand`: plugin commands take precedence over built-in commands of
    /// the same name, which stay reachable as `minecraft:<name>`.
    pub fn dispatch_command(&mut self, source: &CommandSource, line: &str) {
        let line = line.trim();
        let line = line.strip_prefix('/').unwrap_or(line);
        let Some(label) = line.split_whitespace().next() else { return };
        if crate::plugins::events::is_plugin_command(self, label) && crate::plugins::events::run_plugin_command(self, source, line) {
            return;
        }
        if !self.dispatch_builtin_command(source, line) {
            self.unknown_command(source, line);
        }
    }

    /// Runs a built-in command. Returns `false` if there is no built-in command by that name
    /// that the source may use.
    pub fn dispatch_builtin_command(&mut self, source: &CommandSource, line: &str) -> bool {
        let line = line.trim();
        let line = line.strip_prefix('/').unwrap_or(line);
        let mut parts = line.split_whitespace();
        let Some(label) = parts.next() else { return false };
        let args: Vec<&str> = parts.collect();
        let op = self.source_op_level(source);
        let Some(handler) = self.commands.find(label).filter(|c| op >= c.min_op).map(|c| c.handler) else { return false };
        handler(self, source, &args);
        true
    }

    fn unknown_command(&self, source: &CommandSource, line: &str) {
        {
            match source {
                CommandSource::Console => log::info!("Unknown command. Type \"help\" for help."),
                CommandSource::Player(_) => {
                    self.feedback(source, Component::translatable("command.unknown.command", vec![]).color("red"));
                    let context = Component::text(line.to_owned()).color("red").underlined(true).append(Component::translatable("command.context.here", vec![]).color("red").italic(true).underlined(false));
                    self.feedback(source, context);
                }
            }
        }
    }

    /// The command graph for a client with the given operator level, built-in commands
    /// only. Every command is a literal followed by one greedy argument whose completions
    /// come from the server.
    pub fn command_tree(&self, op_level: u8) -> Commands {
        Self::build_command_tree(self.commands.builtins().iter().filter(|b| op_level >= b.min_op).flat_map(|b| std::iter::once(b.name).chain(b.aliases.iter().copied())).map(str::to_owned))
    }

    /// The command graph for one player: the built-in commands their operator level allows
    /// plus the plugin commands they have permission for.
    pub fn command_tree_for(&mut self, id: i32, op_level: u8) -> Commands {
        let plugin_labels = crate::plugins::events::visible_plugin_commands(self, id);
        let overridden: std::collections::HashSet<&str> = plugin_labels.iter().map(String::as_str).collect();
        let builtins = self
            .commands
            .builtins()
            .iter()
            .filter(|b| op_level >= b.min_op)
            .flat_map(|b| std::iter::once(b.name).chain(b.aliases.iter().copied()))
            .filter(|label| !overridden.contains(label))
            .map(str::to_owned)
            .collect::<Vec<_>>();
        Self::build_command_tree(builtins.into_iter().chain(plugin_labels.iter().cloned()))
    }

    fn build_command_tree(labels: impl Iterator<Item = String>) -> Commands {
        let mut nodes = vec![CommandNode::root()];
        let mut seen = std::collections::HashSet::new();
        for label in labels {
            if !seen.insert(label.clone()) {
                continue;
            }
            let literal = nodes.len() as i32;
            let mut node = CommandNode::literal(&label).executable();
            node.children.push(literal + 1);
            nodes.push(node);
            nodes.push(CommandNode::argument("args", Parser::greedy_string()).executable().ask_server());
            nodes[0].children.push(literal);
        }
        Commands { nodes, root_index: 0 }
    }

    /// Answers a tab-completion request.
    pub(super) fn handle_suggestion_request(&mut self, id: i32, transaction_id: i32, text: &str) {
        let source = CommandSource::Player(id);
        let body = text.strip_prefix('/').unwrap_or(text);
        let Some((label, rest)) = body.split_once(' ') else { return };
        if crate::plugins::events::is_plugin_command(self, label) {
            if let Some((start, length, matches)) = crate::plugins::events::plugin_suggestions(self, &source, text) {
                self.send(id, &play::CommandSuggestions { transaction_id, start, length, matches });
                return;
            }
        }
        let op = self.source_op_level(&source);
        let Some(command) = self.commands.find(label).filter(|c| op >= c.min_op) else { return };
        // The last argument is the word being typed (empty right after a space).
        let mut args: Vec<&str> = rest.split(' ').collect();
        let typed = args.pop().unwrap_or("");
        args.retain(|a| !a.is_empty());
        args.push(typed);
        let lower = typed.to_ascii_lowercase();
        let mut matches: Vec<String> = (command.completer)(self, &source, &args).into_iter().filter(|c| c.to_ascii_lowercase().starts_with(&lower)).collect();
        matches.sort();
        matches.dedup();
        matches.truncate(100);
        let start = (text.encode_utf16().count() - typed.encode_utf16().count()) as i32;
        self.send(id, &play::CommandSuggestions { transaction_id, start, length: typed.encode_utf16().count() as i32, matches: matches.into_iter().map(|m| (m, None)).collect() });
    }

    /// Resolves a player argument: a name, a UUID, or one of `@a`, `@p`, `@r`, `@s`.
    pub fn resolve_players(&self, source: &CommandSource, token: &str) -> Vec<i32> {
        match token {
            "@a" | "@e" => self.player_order.clone(),
            "@s" => match source {
                CommandSource::Player(id) => vec![*id],
                CommandSource::Console => Vec::new(),
            },
            "@p" => match source {
                CommandSource::Player(id) => vec![*id],
                CommandSource::Console => self.player_order.first().copied().into_iter().collect(),
            },
            "@r" => {
                if self.player_order.is_empty() {
                    Vec::new()
                } else {
                    vec![self.player_order[rand::random::<usize>() % self.player_order.len()]]
                }
            }
            name => {
                if let Ok(uuid) = uuid::Uuid::parse_str(name) {
                    return self.player_by_uuid(uuid).map(|p| p.entity_id).into_iter().collect();
                }
                self.player_by_name(name).map(|p| p.entity_id).into_iter().collect()
            }
        }
    }

    fn require_players(&self, source: &CommandSource, token: &str) -> Option<Vec<i32>> {
        let found = self.resolve_players(source, token);
        if found.is_empty() {
            self.feedback(source, Component::translatable("argument.entity.notfound.player", vec![]).color("red"));
            return None;
        }
        Some(found)
    }

    fn source_location(&self, source: &CommandSource) -> Option<(usize, Location)> {
        match source {
            CommandSource::Player(id) => self.players.get(id).map(|p| (p.world, p.loc)),
            CommandSource::Console => None,
        }
    }
}

/// Parses a coordinate that may be relative (`~`, `~5`). Whole numbers given for a
/// horizontal axis are centred on the block, as in vanilla.
fn parse_coordinate(token: &str, base: f64, center: bool) -> Option<f64> {
    if let Some(offset) = token.strip_prefix('~') {
        return if offset.is_empty() { Some(base) } else { offset.parse::<f64>().ok().map(|o| base + o) };
    }
    let value: f64 = token.parse().ok()?;
    if !value.is_finite() {
        return None;
    }
    Some(if center && !token.contains('.') { value + 0.5 } else { value })
}

fn parse_block_coordinate(token: &str, base: i32) -> Option<i32> {
    parse_coordinate(token, base as f64, false).map(|v| v.floor() as i32)
}

// --- completers --------------------------------------------------------------------------

fn complete_gamemode(server: &Server, _: &CommandSource, args: &[&str]) -> Vec<String> {
    match args.len() {
        0 | 1 => ["survival", "creative", "adventure", "spectator"].map(String::from).to_vec(),
        2 => player_names(server),
        _ => Vec::new(),
    }
}

fn complete_difficulty(_: &Server, _: &CommandSource, args: &[&str]) -> Vec<String> {
    if args.len() <= 1 {
        ["peaceful", "easy", "normal", "hard"].map(String::from).to_vec()
    } else {
        Vec::new()
    }
}

fn complete_time(_: &Server, _: &CommandSource, args: &[&str]) -> Vec<String> {
    match args.len() {
        0 | 1 => ["set", "add", "query"].map(String::from).to_vec(),
        2 => match args[0] {
            "set" => ["day", "noon", "night", "midnight"].map(String::from).to_vec(),
            "query" => ["daytime", "gametime", "day"].map(String::from).to_vec(),
            _ => Vec::new(),
        },
        _ => Vec::new(),
    }
}

/// Registry names matching what has been typed, with and without the namespace.
fn complete_ids(names: impl Iterator<Item = &'static str>, typed: &str) -> Vec<String> {
    let typed = typed.to_ascii_lowercase();
    let bare = typed.strip_prefix("minecraft:").unwrap_or(&typed);
    let namespaced = typed.contains(':');
    names
        .filter_map(|name| {
            let short = name.strip_prefix("minecraft:").unwrap_or(name);
            short.starts_with(bare).then(|| if namespaced { name.to_owned() } else { short.to_owned() })
        })
        .take(100)
        .collect()
}

fn complete_give(server: &Server, _: &CommandSource, args: &[&str]) -> Vec<String> {
    match args.len() {
        0 | 1 => player_names(server),
        2 => complete_ids(items::ITEMS.iter().map(|i| i.name), args[1]),
        _ => Vec::new(),
    }
}

fn complete_setblock(_: &Server, _: &CommandSource, args: &[&str]) -> Vec<String> {
    match args.len() {
        1..=3 => vec!["~".to_owned()],
        4 => complete_ids(blocks::BLOCKS.iter().map(|b| b.name), args[3]),
        _ => Vec::new(),
    }
}

fn complete_whitelist(server: &Server, _: &CommandSource, args: &[&str]) -> Vec<String> {
    match args.len() {
        0 | 1 => ["on", "off", "list", "add", "remove", "reload"].map(String::from).to_vec(),
        2 if args[0] == "add" => player_names(server),
        2 if args[0] == "remove" => server.shared.access.read().whitelist.iter().map(|e| e.name.clone()).collect(),
        _ => Vec::new(),
    }
}

// --- handlers ----------------------------------------------------------------------------

fn usage(server: &Server, source: &CommandSource, usage: &str) {
    server.fail(source, format!("Usage: {usage}"));
}

fn cmd_help(server: &mut Server, source: &CommandSource, _: &[&str]) {
    let op = server.source_op_level(source);
    server.feedback(source, Component::text("--------- Help ---------").color("yellow"));
    let lines: Vec<Component> = server
        .commands
        .builtins()
        .iter()
        .filter(|b| op >= b.min_op)
        .map(|b| Component::text(b.usage).color("gold").append(Component::text(format!(": {}", b.description)).color("white")))
        .collect();
    for line in lines {
        server.feedback(source, line);
    }
}

fn cmd_list(server: &mut Server, source: &CommandSource, _: &[&str]) {
    let names: Vec<String> = player_names(server);
    let message = Component::translatable(
        "commands.list.players",
        vec![Component::text(names.len().to_string()), Component::text(server.config.properties.max_players.to_string()), Component::text(names.join(", "))],
    );
    server.feedback(source, message);
}

fn cmd_say(server: &mut Server, source: &CommandSource, args: &[&str]) {
    if args.is_empty() {
        return usage(server, source, "/say <message>");
    }
    let name = server.source_name(source);
    server.broadcast_message(&Component::translatable("chat.type.announcement", vec![Component::text(name), Component::text(args.join(" "))]));
}

fn cmd_me(server: &mut Server, source: &CommandSource, args: &[&str]) {
    if args.is_empty() {
        return usage(server, source, "/me <action>");
    }
    let name = server.source_name(source);
    server.broadcast_message(&Component::translatable("chat.type.emote", vec![Component::text(name), Component::text(args.join(" "))]));
}

fn cmd_msg(server: &mut Server, source: &CommandSource, args: &[&str]) {
    if args.len() < 2 {
        return usage(server, source, "/msg <player> <message>");
    }
    let Some(targets) = server.require_players(source, args[0]) else { return };
    let sender = server.source_name(source);
    let text = args[1..].join(" ");
    for target in targets {
        let Some(name) = server.players.get(&target).map(|p| p.name().to_owned()) else { continue };
        let incoming = Component::translatable("commands.message.display.incoming", vec![Component::text(sender.clone()), Component::text(text.clone())]).color("gray").italic(true);
        server.send_message(target, &incoming);
        let outgoing = Component::translatable("commands.message.display.outgoing", vec![Component::text(name), Component::text(text.clone())]).color("gray").italic(true);
        server.feedback(source, outgoing);
    }
}

fn cmd_stop(server: &mut Server, source: &CommandSource, _: &[&str]) {
    server.feedback(source, Component::translatable("commands.stop.stopping", vec![]));
    server.running = false;
}

fn cmd_kick(server: &mut Server, source: &CommandSource, args: &[&str]) {
    if args.is_empty() {
        return usage(server, source, "/kick <player> [reason]");
    }
    let Some(targets) = server.require_players(source, args[0]) else { return };
    let reason = if args.len() > 1 { Component::text(args[1..].join(" ")) } else { Component::translatable("multiplayer.disconnect.kicked", vec![]) };
    for target in targets {
        let Some(name) = server.players.get(&target).map(|p| p.name().to_owned()) else { continue };
        server.kick(target, &reason);
        server.feedback(source, Component::translatable("commands.kick.success", vec![Component::text(name), reason.clone()]));
    }
}

fn cmd_op(server: &mut Server, source: &CommandSource, args: &[&str]) {
    let [name] = args else { return usage(server, source, "/op <player>") };
    let Some(targets) = server.require_players(source, name) else { return };
    let level = server.config.properties.op_permission_level;
    for target in targets {
        let Some((uuid, name)) = server.players.get(&target).map(|p| (p.uuid(), p.name().to_owned())) else { continue };
        if server.shared.access.write().op(uuid, &name, level) {
            server.set_op_level(target, level);
            server.feedback(source, Component::translatable("commands.op.success", vec![Component::text(name)]));
        } else {
            server.feedback(source, Component::translatable("commands.op.failed", vec![]).color("red"));
        }
    }
}

fn cmd_deop(server: &mut Server, source: &CommandSource, args: &[&str]) {
    let [name] = args else { return usage(server, source, "/deop <player>") };
    let Some(targets) = server.require_players(source, name) else { return };
    for target in targets {
        let Some((uuid, name)) = server.players.get(&target).map(|p| (p.uuid(), p.name().to_owned())) else { continue };
        if server.shared.access.write().deop(uuid) {
            server.set_op_level(target, 0);
            server.feedback(source, Component::translatable("commands.deop.success", vec![Component::text(name)]));
        } else {
            server.feedback(source, Component::translatable("commands.deop.failed", vec![]).color("red"));
        }
    }
}

fn cmd_gamemode(server: &mut Server, source: &CommandSource, args: &[&str]) {
    let Some(mode) = args.first().and_then(|m| GameMode::parse(m)) else { return usage(server, source, "/gamemode <survival|creative|adventure|spectator> [player]") };
    let targets = match args.get(1) {
        Some(token) => match server.require_players(source, token) {
            Some(t) => t,
            None => return,
        },
        None => match source {
            CommandSource::Player(id) => vec![*id],
            CommandSource::Console => return server.fail(source, "A player is required to run this command here"),
        },
    };
    let mode_name = Component::translatable(format!("gameMode.{}", mode.name()), vec![]);
    for target in targets {
        server.set_game_mode(target, mode);
        if *source == CommandSource::Player(target) {
            server.feedback(source, Component::translatable("commands.gamemode.success.self", vec![mode_name.clone()]));
        } else {
            let name = server.players.get(&target).map_or_else(String::new, |p| p.name().to_owned());
            server.send_message(target, &Component::translatable("gameMode.changed", vec![mode_name.clone()]));
            server.feedback(source, Component::translatable("commands.gamemode.success.other", vec![Component::text(name), mode_name.clone()]));
        }
    }
}

fn cmd_default_gamemode(server: &mut Server, source: &CommandSource, args: &[&str]) {
    let Some(mode) = args.first().and_then(|m| GameMode::parse(m)) else { return usage(server, source, "/defaultgamemode <mode>") };
    server.default_game_mode = mode;
    server.feedback(source, Component::text(format!("The default game mode is now {}", mode.name())));
}

fn cmd_teleport(server: &mut Server, source: &CommandSource, args: &[&str]) {
    // Forms: <target> | <x y z> | <who> <target> | <who> <x y z>
    let (who, destination): (Vec<i32>, &[&str]) = match args.len() {
        1 | 3 => match source {
            CommandSource::Player(id) => (vec![*id], args),
            CommandSource::Console => return server.fail(source, "A player is required to run this command here"),
        },
        2 | 4 => match server.require_players(source, args[0]) {
            Some(who) => (who, &args[1..]),
            None => return,
        },
        _ => return usage(server, source, "/tp [player] <target | x y z>"),
    };
    if let [target] = destination {
        let Some(targets) = server.require_players(source, target) else { return };
        let Some((world, loc, name)) = server.players.get(&targets[0]).map(|p| (p.world, p.loc, p.name().to_owned())) else { return };
        for id in who {
            let Some(mover) = server.players.get(&id).map(|p| (p.world, p.name().to_owned())) else { continue };
            if mover.0 != world {
                server.fail(source, "Teleporting between worlds is not supported yet");
                continue;
            }
            server.teleport(id, loc);
            server.feedback(source, Component::translatable("commands.teleport.success.entity.single", vec![Component::text(mover.1), Component::text(name.clone())]));
        }
        return;
    }
    for id in who {
        let Some((base, name)) = server.players.get(&id).map(|p| (p.loc, p.name().to_owned())) else { continue };
        let reference = server.source_location(source).map_or(base, |(_, loc)| loc);
        let (Some(x), Some(y), Some(z)) =
            (parse_coordinate(destination[0], reference.x, true), parse_coordinate(destination[1], reference.y, false), parse_coordinate(destination[2], reference.z, true))
        else {
            return server.fail(source, "Expected three coordinates");
        };
        if x.abs() > 3.0e7 || z.abs() > 3.0e7 || y.abs() > 2.0e7 {
            return server.fail(source, "That position is outside the world");
        }
        server.teleport(id, Location { x, y, z, ..base });
        let fmt = |v: f64| Component::text(format!("{v:.2}"));
        server.feedback(source, Component::translatable("commands.teleport.success.location.single", vec![Component::text(name), fmt(x), fmt(y), fmt(z)]));
    }
}

fn cmd_give(server: &mut Server, source: &CommandSource, args: &[&str]) {
    if args.len() < 2 {
        return usage(server, source, "/give <player> <item> [count]");
    }
    let Some(targets) = server.require_players(source, args[0]) else { return };
    let Some(item) = items::id(args[1]) else { return server.fail(source, format!("Unknown item '{}'", args[1])) };
    let info = &items::ITEMS[item as usize];
    let count = match args.get(2).map(|c| c.parse::<i32>()) {
        None => 1,
        Some(Ok(n)) if n >= 1 && n <= info.max_stack_size as i32 * 100 => n,
        _ => return server.fail(source, "Invalid item count"),
    };
    for target in targets {
        let left = server.give_item(target, item as i32, count);
        let name = server.players.get(&target).map_or_else(String::new, |p| p.name().to_owned());
        let item_name = info.name.strip_prefix("minecraft:").unwrap_or(info.name);
        server.feedback(source, Component::translatable("commands.give.success.single", vec![Component::text((count - left).to_string()), Component::text(item_name), Component::text(name)]));
        if left > 0 {
            server.fail(source, format!("{left} item(s) did not fit into the inventory"));
        }
    }
}

fn cmd_clear(server: &mut Server, source: &CommandSource, args: &[&str]) {
    let targets = match args.first() {
        Some(token) => match server.require_players(source, token) {
            Some(t) => t,
            None => return,
        },
        None => match source {
            CommandSource::Player(id) => vec![*id],
            CommandSource::Console => return usage(server, source, "/clear <player>"),
        },
    };
    for target in targets {
        let Some(player) = server.players.get_mut(&target) else { continue };
        let removed: i32 = player.inventory.iter().filter_map(|s| s.0.as_ref()).map(|s| s.count).sum();
        player.inventory.iter_mut().for_each(|s| *s = paper_protocol::types::Slot::EMPTY);
        player.equipment_dirty = true;
        let name = player.name().to_owned();
        server.resync_inventory(target);
        server.feedback(source, Component::text(format!("Removed {removed} item(s) from player {name}")));
    }
}

fn cmd_setblock(server: &mut Server, source: &CommandSource, args: &[&str]) {
    let [x, y, z, block] = args else { return usage(server, source, "/setblock <x> <y> <z> <block>") };
    let (world, base) = server.source_location(source).unwrap_or((0, Location::default()));
    let (bx, by, bz) = (base.x.floor() as i32, base.y.floor() as i32, base.z.floor() as i32);
    let (Some(x), Some(y), Some(z)) = (parse_block_coordinate(x, bx), parse_block_coordinate(y, by), parse_block_coordinate(z, bz)) else {
        return server.fail(source, "Expected three block coordinates");
    };
    let Some(state) = blocks::parse_state(block) else { return server.fail(source, format!("Unknown block '{block}'")) };
    if server.worlds[world].block_at(x, y, z).is_none() || !server.worlds[world].in_bounds(y) {
        return server.fail(source, "That position is not loaded");
    }
    match server.set_block_and_update(world, paper_protocol::codec::BlockPos::new(x, y, z), state) {
        true => {
            let c = |v: i32| Component::text(v.to_string());
            server.feedback(source, Component::translatable("commands.setblock.success", vec![c(x), c(y), c(z)]));
        }
        false => server.feedback(source, Component::translatable("commands.setblock.failed", vec![]).color("red")),
    }
}

fn cmd_time(server: &mut Server, source: &CommandSource, args: &[&str]) {
    let world = server.source_location(source).map_or(0, |(w, _)| w);
    match args {
        ["set", value] => {
            let time = match *value {
                "day" => 1000,
                "noon" => 6000,
                "night" => 13000,
                "midnight" => 18000,
                other => match other.parse::<i64>() {
                    Ok(t) if t >= 0 => t,
                    _ => return server.fail(source, "Invalid time"),
                },
            };
            server.worlds[world].day_time = time;
            let payload = server.time_payload(world);
            server.broadcast_payload_in_world(world, payload);
            server.feedback(source, Component::translatable("commands.time.set", vec![Component::text(time.to_string())]));
        }
        ["add", value] => {
            let Ok(delta) = value.parse::<i64>() else { return server.fail(source, "Invalid time") };
            server.worlds[world].day_time = (server.worlds[world].day_time + delta).max(0);
            let time = server.worlds[world].day_time;
            let payload = server.time_payload(world);
            server.broadcast_payload_in_world(world, payload);
            server.feedback(source, Component::translatable("commands.time.set", vec![Component::text(time.to_string())]));
        }
        ["query", what] => {
            let w = &server.worlds[world];
            let value = match *what {
                "daytime" => w.day_time % 24000,
                "gametime" => w.age,
                "day" => w.day_time / 24000,
                _ => return usage(server, source, "/time query <daytime|gametime|day>"),
            };
            server.feedback(source, Component::translatable("commands.time.query", vec![Component::text(value.to_string())]));
        }
        _ => usage(server, source, "/time <set|add|query> <value>"),
    }
}

fn cmd_difficulty(server: &mut Server, source: &CommandSource, args: &[&str]) {
    match args.first() {
        None => server.feedback(source, Component::text(format!("The difficulty is {}", server.difficulty.name()))),
        Some(value) => {
            let Some(difficulty) = Difficulty::parse(value) else { return usage(server, source, "/difficulty [peaceful|easy|normal|hard]") };
            server.difficulty = difficulty;
            server.broadcast(&play::ChangeDifficulty { difficulty: difficulty.id(), locked: false });
            server.feedback(source, Component::translatable("commands.difficulty.success", vec![Component::text(difficulty.name())]));
        }
    }
}

fn cmd_seed(server: &mut Server, source: &CommandSource, _: &[&str]) {
    let world = server.source_location(source).map_or(0, |(w, _)| w);
    let seed = server.worlds[world].seed.to_string();
    server.feedback(source, Component::translatable("commands.seed.success", vec![Component::text(seed.clone()).color("green").click_copy(&seed)]));
}

fn cmd_set_world_spawn(server: &mut Server, source: &CommandSource, args: &[&str]) {
    let (world, base) = match server.source_location(source) {
        Some(found) => found,
        None if args.len() == 3 => (0, Location::default()),
        None => return usage(server, source, "/setworldspawn <x> <y> <z>"),
    };
    let (bx, by, bz) = (base.x.floor() as i32, base.y.floor() as i32, base.z.floor() as i32);
    let pos = match args {
        [] => paper_protocol::codec::BlockPos::new(bx, by, bz),
        [x, y, z] => match (parse_block_coordinate(x, bx), parse_block_coordinate(y, by), parse_block_coordinate(z, bz)) {
            (Some(x), Some(y), Some(z)) => paper_protocol::codec::BlockPos::new(x, y, z),
            _ => return server.fail(source, "Expected three block coordinates"),
        },
        _ => return usage(server, source, "/setworldspawn [x y z]"),
    };
    let w = &mut server.worlds[world];
    w.spawn = pos;
    let packet = play::SetDefaultSpawnPosition { dimension: w.dimension.clone(), pos, yaw: w.spawn_yaw, pitch: 0.0 };
    let payload = bytes::Bytes::from(paper_protocol::packets::encode_packet(&packet));
    server.broadcast_payload_in_world(world, payload);
    server.feedback(source, Component::text(format!("Set the world spawn point to {}, {}, {}", pos.x, pos.y, pos.z)));
}

fn tps_color(tps: f32) -> &'static str {
    if tps >= 18.0 {
        "green"
    } else if tps >= 16.0 {
        "yellow"
    } else {
        "red"
    }
}

fn cmd_tps(server: &mut Server, source: &CommandSource, _: &[&str]) {
    let mut message = Component::text("TPS from last 5s, 10s, 1m: ").color("gold");
    for (i, ticks) in [100usize, 200, 1200].into_iter().enumerate() {
        let tps = server.stats.tps(ticks);
        if i > 0 {
            message = message.append(Component::text(", ").color("gold"));
        }
        message = message.append(Component::text(format!("{tps:.1}")).color(tps_color(tps)));
    }
    server.feedback(source, message);
}

fn cmd_mspt(server: &mut Server, source: &CommandSource, _: &[&str]) {
    let values: Vec<String> = [100usize, 200, 1200].iter().map(|t| format!("{:.2}", server.stats.mspt(*t))).collect();
    server.feedback(source, Component::text(format!("Average tick time from last 5s, 10s, 1m: {} ms", values.join(", "))).color("gold"));
}

fn cmd_ban(server: &mut Server, source: &CommandSource, args: &[&str]) {
    if args.is_empty() {
        return usage(server, source, "/ban <player> [reason]");
    }
    // Banning works for offline players in offline mode, where the UUID follows the name.
    let (uuid, name) = match server.player_by_name(args[0]) {
        Some(p) => (p.uuid(), p.name().to_owned()),
        None if !server.config.properties.online_mode => (crate::net::auth::offline_uuid(args[0]), args[0].to_owned()),
        None => return server.fail(source, "That player is not online; banning offline players needs their UUID"),
    };
    let reason = (args.len() > 1).then(|| args[1..].join(" "));
    let by = server.source_name(source);
    if !server.shared.access.write().ban(uuid, &name, &by, reason.as_deref()) {
        return server.fail(source, "Nothing changed. The player is already banned");
    }
    if let Some(id) = server.by_uuid.get(&uuid).copied() {
        server.kick(id, &Component::translatable("multiplayer.disconnect.banned", vec![]));
    }
    server.feedback(source, Component::text(format!("Banned {name}: {}", reason.as_deref().unwrap_or("Banned by an operator."))));
}

fn cmd_pardon(server: &mut Server, source: &CommandSource, args: &[&str]) {
    let [name] = args else { return usage(server, source, "/pardon <player>") };
    if server.shared.access.write().pardon(name) {
        server.feedback(source, Component::text(format!("Unbanned {name}")));
    } else {
        server.fail(source, "Nothing changed. The player isn't banned");
    }
}

fn cmd_ban_ip(server: &mut Server, source: &CommandSource, args: &[&str]) {
    if args.is_empty() {
        return usage(server, source, "/ban-ip <address | player> [reason]");
    }
    let ip = match args[0].parse::<std::net::IpAddr>() {
        Ok(ip) => ip.to_string(),
        Err(_) => match server.player_by_name(args[0]) {
            Some(p) => p.conn.addr.ip().to_string(),
            None => return server.fail(source, "Invalid IP address or unknown player"),
        },
    };
    let reason = (args.len() > 1).then(|| args[1..].join(" "));
    let by = server.source_name(source);
    if !server.shared.access.write().ban_ip(&ip, &by, reason.as_deref()) {
        return server.fail(source, "Nothing changed. That IP is already banned");
    }
    let affected: Vec<i32> = server.players.values().filter(|p| p.conn.addr.ip().to_string() == ip).map(|p| p.entity_id).collect();
    for id in &affected {
        server.kick(*id, &Component::translatable("multiplayer.disconnect.ip_banned", vec![]));
    }
    server.feedback(source, Component::text(format!("Banned IP {ip}: {}", reason.as_deref().unwrap_or("Banned by an operator."))));
}

fn cmd_pardon_ip(server: &mut Server, source: &CommandSource, args: &[&str]) {
    let [ip] = args else { return usage(server, source, "/pardon-ip <address>") };
    if server.shared.access.write().pardon_ip(ip) {
        server.feedback(source, Component::text(format!("Unbanned IP {ip}")));
    } else {
        server.fail(source, "Nothing changed. That IP isn't banned");
    }
}

fn cmd_banlist(server: &mut Server, source: &CommandSource, args: &[&str]) {
    let (players, ips) = match args.first().copied() {
        Some("ips") => (false, true),
        Some("players") => (true, false),
        _ => (true, true),
    };
    let mut lines = Vec::new();
    {
        let access = server.shared.access.read();
        if players {
            lines.extend(access.banned_players.iter().map(|b| format!("{} was banned by {}: {}", b.name, b.source, b.reason)));
        }
        if ips {
            lines.extend(access.banned_ips.iter().map(|b| format!("{} was banned by {}: {}", b.ip, b.source, b.reason)));
        }
    }
    if lines.is_empty() {
        return server.feedback(source, Component::text("There are no bans"));
    }
    server.feedback(source, Component::text(format!("There are {} ban(s):", lines.len())));
    for line in lines {
        server.feedback(source, Component::text(line));
    }
}

fn cmd_whitelist(server: &mut Server, source: &CommandSource, args: &[&str]) {
    match args {
        ["on"] | ["off"] => {
            // The switch itself lives in server.properties, which is read at startup.
            server.fail(source, "Set white-list in server.properties and restart to change this");
        }
        ["list"] => {
            let names: Vec<String> = server.shared.access.read().whitelist.iter().map(|e| e.name.clone()).collect();
            if names.is_empty() {
                server.feedback(source, Component::text("There are no whitelisted players"));
            } else {
                server.feedback(source, Component::text(format!("There are {} whitelisted player(s): {}", names.len(), names.join(", "))));
            }
        }
        ["add", name] => {
            let (uuid, name) = match server.player_by_name(name) {
                Some(p) => (p.uuid(), p.name().to_owned()),
                None if !server.config.properties.online_mode => (crate::net::auth::offline_uuid(name), (*name).to_owned()),
                None => return server.fail(source, "That player is not online; whitelisting offline players needs their UUID"),
            };
            if server.shared.access.write().whitelist_add(uuid, &name) {
                server.feedback(source, Component::translatable("commands.whitelist.add.success", vec![Component::text(name)]));
            } else {
                server.fail(source, "Player is already whitelisted");
            }
        }
        ["remove", name] => {
            if server.shared.access.write().whitelist_remove(name) {
                server.feedback(source, Component::translatable("commands.whitelist.remove.success", vec![Component::text(*name)]));
            } else {
                server.fail(source, "Player is not whitelisted");
            }
        }
        ["reload"] => {
            server.shared.access.write().reload_whitelist();
            server.feedback(source, Component::translatable("commands.whitelist.reloaded", vec![]));
        }
        _ => usage(server, source, "/whitelist <on|off|list|add|remove|reload> [player]"),
    }
}

fn cmd_save_all(server: &mut Server, source: &CommandSource, _: &[&str]) {
    server.feedback(source, Component::translatable("commands.save.saving", vec![]));
    let saved = server.save_worlds();
    if saved {
        server.feedback(source, Component::translatable("commands.save.success", vec![]));
    } else {
        server.fail(source, "This server keeps its worlds in memory only (it has no server directory)");
    }
}

fn cmd_version(server: &mut Server, source: &CommandSource, _: &[&str]) {
    server.feedback(
        source,
        Component::text(format!(
            "This server is running Paper-RS version {} (MC: {}) (Implementing API version {}-R0.1-SNAPSHOT)",
            env!("CARGO_PKG_VERSION"),
            paper_data::VERSION_NAME,
            paper_data::VERSION_NAME
        )),
    );
}

/// Lists plugins like Paper's `/plugins`: Paper plugins and Bukkit plugins separately.
fn cmd_plugins(server: &mut Server, source: &CommandSource, _: &[&str]) {
    let mut plugins = server.plugin_list.clone();
    plugins.sort_by_key(|p| p.name.to_ascii_lowercase());
    server.feedback(source, Component::text(format!("\u{2139} Server Plugins ({}):", plugins.len())).color("white"));
    for (kind, title) in [("paper", "Paper Plugins:"), ("bukkit", "Bukkit Plugins:")] {
        let group: Vec<_> = plugins.iter().filter(|p| p.kind == kind).collect();
        if group.is_empty() {
            continue;
        }
        server.feedback(source, Component::text(title).color("#0288D1"));
        let mut line = Component::text(" - ").color("dark_gray");
        for (i, plugin) in group.iter().enumerate() {
            if i > 0 {
                line = line.append(Component::text(", ").color("white"));
            }
            line = line.append(Component::text(plugin.name.clone()).color(if plugin.enabled { "green" } else { "red" }).hover_text(Component::text(format!("{} {}", plugin.name, plugin.version))));
        }
        server.feedback(source, line);
    }
}

impl Server {
    /// Changes a player's operator level and refreshes what depends on it client-side.
    pub fn set_op_level(&mut self, id: i32, level: u8) {
        let Some(player) = self.players.get_mut(&id) else { return };
        player.op_level = level;
        player.conn.send(&play::EntityEvent { entity_id: id, status: 24 + level.min(4) as i8 });
        let tree = self.command_tree_for(id, level);
        self.send(id, &tree);
    }

    /// Names of the loaded plugins and whether each is enabled.
    pub fn plugin_names(&self) -> Vec<(String, bool)> {
        let mut names: Vec<(String, bool)> = self.plugin_list.iter().map(|p| (p.name.clone(), p.enabled)).collect();
        names.sort_by_key(|(name, _)| name.to_ascii_lowercase());
        names
    }

    /// Saves all worlds and waits until they are on disk. Returns whether persistence is
    /// available.
    pub fn save_worlds(&mut self) -> bool {
        self.save_all(true)
    }
}

#[allow(dead_code)]
fn _varint_is_used(_: VarInt) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coordinates() {
        assert_eq!(parse_coordinate("10", 0.0, true), Some(10.5));
        assert_eq!(parse_coordinate("10", 0.0, false), Some(10.0));
        assert_eq!(parse_coordinate("10.25", 0.0, true), Some(10.25));
        assert_eq!(parse_coordinate("~", 7.5, true), Some(7.5));
        assert_eq!(parse_coordinate("~-2", 7.5, true), Some(5.5));
        assert_eq!(parse_coordinate("abc", 0.0, true), None);
        assert_eq!(parse_coordinate("NaN", 0.0, true), None);
        assert_eq!(parse_block_coordinate("~1", -3), Some(-2));
        assert_eq!(parse_block_coordinate("-0.5", 0), Some(-1));
    }

    #[test]
    fn registry_lookup() {
        let registry = CommandRegistry::with_builtins();
        assert_eq!(registry.find("tp").unwrap().name, "tp");
        assert_eq!(registry.find("teleport").unwrap().name, "tp");
        assert_eq!(registry.find("minecraft:GameMode").unwrap().name, "gamemode");
        assert!(registry.find("nope").is_none());
        // No label is claimed twice.
        let mut labels: Vec<&str> = registry.builtins().iter().flat_map(|b| std::iter::once(b.name).chain(b.aliases.iter().copied())).collect();
        let total = labels.len();
        labels.sort_unstable();
        labels.dedup();
        assert_eq!(labels.len(), total);
    }

    #[test]
    fn id_completion() {
        let found = complete_ids(items::ITEMS.iter().map(|i| i.name), "diamond_sw");
        assert_eq!(found, ["diamond_sword"]);
        let found = complete_ids(items::ITEMS.iter().map(|i| i.name), "minecraft:diamond_sw");
        assert_eq!(found, ["minecraft:diamond_sword"]);
    }
}
