//! Text components.
//!
//! A [`Component`] is kept in its JSON shape (the same shape Adventure's Gson serializer and
//! Minecraft's own JSON files use) and converted to NBT when it goes on the wire. Keeping the
//! JSON tree intact means components produced by plugins pass through the server without the
//! server having to understand every field.

use paper_nbt::{Compound, Tag};
use serde_json::{json, Map, Value};

/// The sixteen legacy colours, indexed by their `§` code digit.
pub const NAMED_COLORS: [&str; 16] = [
    "black", "dark_blue", "dark_green", "dark_aqua", "dark_red", "dark_purple", "gold", "gray", "dark_gray", "blue", "green", "aqua",
    "red", "light_purple", "yellow", "white",
];

const BOOLEAN_KEYS: [&str; 6] = ["bold", "italic", "underlined", "strikethrough", "obfuscated", "interpret"];

/// A chat/text component.
#[derive(Clone, Debug, PartialEq)]
pub struct Component(pub Value);

impl Default for Component {
    fn default() -> Self {
        Component::text("")
    }
}

impl From<&str> for Component {
    fn from(s: &str) -> Self {
        Component::text(s)
    }
}

impl From<String> for Component {
    fn from(s: String) -> Self {
        Component::text(s)
    }
}

impl Component {
    /// Plain text without styling.
    pub fn text(s: impl Into<String>) -> Self {
        Component(json!({ "text": s.into() }))
    }

    /// A translatable component; the client substitutes `args` into its own language file.
    pub fn translatable(key: impl Into<String>, args: Vec<Component>) -> Self {
        let mut obj = Map::new();
        obj.insert("translate".into(), Value::String(key.into()));
        if !args.is_empty() {
            obj.insert("with".into(), Value::Array(args.into_iter().map(|c| c.0).collect()));
        }
        Component(Value::Object(obj))
    }

    /// Parses a JSON component (object, array or bare string).
    pub fn from_json_str(s: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str::<Value>(s).map(Component)
    }

    pub fn to_json_string(&self) -> String {
        self.0.to_string()
    }

    fn object_mut(&mut self) -> &mut Map<String, Value> {
        if !self.0.is_object() {
            // Promote strings (and anything unusual) to the object form so it can carry style.
            let old = std::mem::take(&mut self.0);
            self.0 = match old {
                Value::String(s) => json!({ "text": s }),
                Value::Array(mut items) if !items.is_empty() => {
                    let first = items.remove(0);
                    let mut head = Component(first);
                    let obj = head.object_mut();
                    let extra = obj.entry("extra").or_insert_with(|| Value::Array(Vec::new()));
                    if let Value::Array(list) = extra {
                        list.extend(items);
                    }
                    head.0
                }
                other => json!({ "text": plain_of(&other) }),
            };
        }
        self.0.as_object_mut().expect("component was just normalised to an object")
    }

    fn set(mut self, key: &str, value: Value) -> Self {
        self.object_mut().insert(key.into(), value);
        self
    }

    /// A named colour (`"red"`) or `#rrggbb`.
    pub fn color(self, color: &str) -> Self {
        self.set("color", Value::String(color.into()))
    }
    pub fn bold(self, on: bool) -> Self {
        self.set("bold", Value::Bool(on))
    }
    pub fn italic(self, on: bool) -> Self {
        self.set("italic", Value::Bool(on))
    }
    pub fn underlined(self, on: bool) -> Self {
        self.set("underlined", Value::Bool(on))
    }
    pub fn strikethrough(self, on: bool) -> Self {
        self.set("strikethrough", Value::Bool(on))
    }
    pub fn obfuscated(self, on: bool) -> Self {
        self.set("obfuscated", Value::Bool(on))
    }
    /// Text inserted into the chat box on shift-click.
    pub fn insertion(self, text: &str) -> Self {
        self.set("insertion", Value::String(text.into()))
    }
    pub fn click_run_command(self, command: &str) -> Self {
        self.set("click_event", json!({ "action": "run_command", "command": command }))
    }
    pub fn click_suggest_command(self, command: &str) -> Self {
        self.set("click_event", json!({ "action": "suggest_command", "command": command }))
    }
    pub fn click_open_url(self, url: &str) -> Self {
        self.set("click_event", json!({ "action": "open_url", "url": url }))
    }
    pub fn click_copy(self, text: &str) -> Self {
        self.set("click_event", json!({ "action": "copy_to_clipboard", "value": text }))
    }
    pub fn hover_text(self, text: Component) -> Self {
        self.set("hover_event", json!({ "action": "show_text", "value": text.0 }))
    }

    /// Appends a child component (it inherits this component's style).
    pub fn append(mut self, child: impl Into<Component>) -> Self {
        let child = child.into();
        let extra = self.object_mut().entry("extra").or_insert_with(|| Value::Array(Vec::new()));
        if let Value::Array(list) = extra {
            list.push(child.0);
        }
        self
    }

    /// Converts text using legacy `§` formatting codes (including the `§x§r§r§g§g§b§b` hex
    /// form) into a component.
    pub fn from_legacy(s: &str) -> Self {
        if !s.contains('§') {
            return Component::text(s);
        }
        #[derive(Clone, Default, PartialEq)]
        struct Style {
            color: Option<String>,
            bold: bool,
            italic: bool,
            underlined: bool,
            strikethrough: bool,
            obfuscated: bool,
        }
        fn flush(parts: &mut Vec<Value>, text: &mut String, style: &Style) {
            if text.is_empty() {
                return;
            }
            let mut obj = Map::new();
            obj.insert("text".into(), Value::String(std::mem::take(text)));
            if let Some(c) = &style.color {
                obj.insert("color".into(), Value::String(c.clone()));
            }
            for (key, on) in [
                ("bold", style.bold),
                ("italic", style.italic),
                ("underlined", style.underlined),
                ("strikethrough", style.strikethrough),
                ("obfuscated", style.obfuscated),
            ] {
                if on {
                    obj.insert(key.into(), Value::Bool(true));
                }
            }
            parts.push(Value::Object(obj));
        }

        let chars: Vec<char> = s.chars().collect();
        let mut parts = Vec::new();
        let mut text = String::new();
        let mut style = Style::default();
        let mut i = 0;
        while i < chars.len() {
            if chars[i] == '§' && i + 1 < chars.len() {
                let code = chars[i + 1].to_ascii_lowercase();
                // §x§r§r§g§g§b§b
                if code == 'x' && i + 13 < chars.len() && (0..6).all(|k| chars[i + 2 + k * 2] == '§' && chars[i + 3 + k * 2].is_ascii_hexdigit()) {
                    flush(&mut parts, &mut text, &style);
                    let hex: String = (0..6).map(|k| chars[i + 3 + k * 2].to_ascii_lowercase()).collect();
                    style = Style { color: Some(format!("#{hex}")), ..Style::default() };
                    i += 14;
                    continue;
                }
                let recognised = match code {
                    '0'..='9' | 'a'..='f' => {
                        flush(&mut parts, &mut text, &style);
                        let idx = code.to_digit(16).unwrap() as usize;
                        // A colour code resets formatting, as in vanilla.
                        style = Style { color: Some(NAMED_COLORS[idx].to_string()), ..Style::default() };
                        true
                    }
                    'k' | 'l' | 'm' | 'n' | 'o' => {
                        flush(&mut parts, &mut text, &style);
                        match code {
                            'k' => style.obfuscated = true,
                            'l' => style.bold = true,
                            'm' => style.strikethrough = true,
                            'n' => style.underlined = true,
                            _ => style.italic = true,
                        }
                        true
                    }
                    'r' => {
                        flush(&mut parts, &mut text, &style);
                        style = Style::default();
                        true
                    }
                    _ => false,
                };
                if recognised {
                    i += 2;
                    continue;
                }
            }
            text.push(chars[i]);
            i += 1;
        }
        flush(&mut parts, &mut text, &style);
        match parts.len() {
            0 => Component::text(""),
            1 => Component(parts.pop().unwrap()),
            _ => Component(json!({ "text": "", "extra": parts })),
        }
    }

    /// The text content without any styling. Translatable components render through the
    /// small built-in table of messages the server itself emits, falling back to the
    /// component's `fallback` or its key.
    pub fn to_plain(&self) -> String {
        plain_of(&self.0)
    }

    /// Renders with ANSI colour escapes for terminal output.
    pub fn to_ansi(&self) -> String {
        let mut out = String::new();
        ansi_of(&self.0, &AnsiStyle::default(), &mut out);
        if out.contains('\x1b') {
            out.push_str("\x1b[0m");
        }
        out
    }

    /// The NBT form used on the wire: a string tag for bare text, a compound otherwise.
    pub fn to_nbt(&self) -> Tag {
        match &self.0 {
            Value::Object(obj) if obj.len() == 1 => match obj.get("text") {
                Some(Value::String(s)) => Tag::String(s.clone()),
                _ => json_to_nbt(&self.0),
            },
            other => json_to_nbt(other),
        }
    }

    pub fn from_nbt(tag: &Tag) -> Self {
        match tag {
            // Bare text travels as a string tag; restore the object form `text()` builds.
            Tag::String(s) => Component::text(s.clone()),
            other => Component(nbt_to_json(other, None)),
        }
    }
}

/// English text for the translatable messages the server emits, so console output and
/// plain-text consumers are readable without Mojang's language file.
fn builtin_translation(key: &str) -> Option<&'static str> {
    Some(match key {
        "multiplayer.player.joined" => "%s joined the game",
        "multiplayer.player.joined.renamed" => "%s (formerly known as %s) joined the game",
        "multiplayer.player.left" => "%s left the game",
        "multiplayer.disconnect.kicked" => "Kicked by an operator",
        "multiplayer.disconnect.server_shutdown" => "Server closed",
        "multiplayer.disconnect.duplicate_login" => "You logged in from another location",
        "multiplayer.disconnect.not_whitelisted" => "You are not white-listed on this server!",
        "multiplayer.disconnect.banned" => "You are banned from this server",
        "multiplayer.disconnect.server_full" => "The server is full!",
        "multiplayer.disconnect.invalid_player_movement" => "Invalid move player packet received",
        "multiplayer.disconnect.illegal_characters" => "Illegal characters in chat",
        "multiplayer.disconnect.unverified_username" => "Failed to verify username!",
        "multiplayer.disconnect.outdated_client" => "Incompatible client! Please use %s",
        "multiplayer.disconnect.incompatible" => "Incompatible client! Please use %s",
        "multiplayer.disconnect.outdated_server" => "Incompatible client! Please use %s",
        "multiplayer.disconnect.slow_login" => "Took too long to log in",
        "multiplayer.disconnect.idling" => "You have been idle for too long!",
        "multiplayer.disconnect.generic" => "Disconnected",
        "disconnect.timeout" => "Timed out",
        "disconnect.genericReason" => "%s",
        "chat.type.text" => "<%s> %s",
        "chat.type.announcement" => "[%s] %s",
        "chat.type.emote" => "* %s %s",
        "chat.type.admin" => "[%s: %s]",
        "commands.message.display.incoming" => "%s whispers to you: %s",
        "commands.message.display.outgoing" => "You whisper to %s: %s",
        "command.unknown.command" => "Unknown or incomplete command. See below for error",
        "command.unknown.argument" => "Incorrect argument for command",
        "command.context.here" => "<--[HERE]",
        "commands.help.failed" => "Unknown command or insufficient permissions",
        "argument.entity.notfound.player" => "No player was found",
        "death.attack.generic" => "%s died",
        "death.attack.outOfWorld" => "%s fell out of the world",
        "gameMode.survival" => "Survival Mode",
        "gameMode.creative" => "Creative Mode",
        "gameMode.adventure" => "Adventure Mode",
        "gameMode.spectator" => "Spectator Mode",
        "commands.gamemode.success.self" => "Set own game mode to %s",
        "commands.gamemode.success.other" => "Set %s's game mode to %s",
        "gameMode.changed" => "Your game mode has been updated to %s",
        "commands.teleport.success.entity.single" => "Teleported %s to %s",
        "commands.teleport.success.location.single" => "Teleported %s to %s, %s, %s",
        "commands.list.players" => "There are %s of a max of %s players online: %s",
        "commands.time.set" => "Set the time to %s",
        "commands.time.query" => "The time is %s",
        "commands.kick.success" => "Kicked %s: %s",
        "commands.op.success" => "Made %s a server operator",
        "commands.op.failed" => "Nothing changed. The player already is an operator",
        "commands.deop.success" => "Made %s no longer a server operator",
        "commands.deop.failed" => "Nothing changed. The player is not an operator",
        "commands.setblock.success" => "Changed the block at %s, %s, %s",
        "commands.setblock.failed" => "Could not set the block",
        "commands.give.success.single" => "Gave %s %s to %s",
        "commands.seed.success" => "Seed: %s",
        "commands.save.saving" => "Saving the game (this may take a moment!)",
        "commands.save.success" => "Saved the game",
        "commands.stop.stopping" => "Stopping the server",
        "commands.whitelist.enabled" => "Whitelist is now turned on",
        "commands.whitelist.disabled" => "Whitelist is now turned off",
        "commands.whitelist.add.success" => "Added %s to the whitelist",
        "commands.whitelist.remove.success" => "Removed %s from the whitelist",
        "commands.whitelist.reloaded" => "Reloaded the whitelist",
        "commands.difficulty.success" => "The difficulty has been set to %s",
        "commands.weather.set.clear" => "Set the weather to clear",
        "commands.weather.set.rain" => "Set the weather to rain",
        "commands.weather.set.thunder" => "Set the weather to rain & thunder",
        "permissions.requires.player" => "A player is required to run this command here",
        _ => return None,
    })
}

/// Substitutes `%s` and `%N$s` placeholders.
fn format_translation(pattern: &str, args: &[String]) -> String {
    let mut out = String::with_capacity(pattern.len());
    let chars: Vec<char> = pattern.chars().collect();
    let mut next = 0usize;
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '%' && i + 1 < chars.len() {
            if chars[i + 1] == '%' {
                out.push('%');
                i += 2;
                continue;
            }
            if chars[i + 1] == 's' {
                out.push_str(args.get(next).map(String::as_str).unwrap_or(""));
                next += 1;
                i += 2;
                continue;
            }
            // %N$s
            let mut j = i + 1;
            let mut n = 0usize;
            while j < chars.len() && chars[j].is_ascii_digit() {
                n = n * 10 + chars[j].to_digit(10).unwrap() as usize;
                j += 1;
            }
            if j > i + 1 && j + 1 < chars.len() && chars[j] == '$' && chars[j + 1] == 's' {
                out.push_str(n.checked_sub(1).and_then(|k| args.get(k)).map(String::as_str).unwrap_or(""));
                i = j + 2;
                continue;
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

fn own_text(obj: &Map<String, Value>) -> String {
    if let Some(Value::String(s)) = obj.get("text") {
        return s.clone();
    }
    if let Some(Value::String(key)) = obj.get("translate") {
        let args: Vec<String> = match obj.get("with") {
            Some(Value::Array(items)) => items.iter().map(plain_of).collect(),
            _ => Vec::new(),
        };
        if let Some(pattern) = builtin_translation(key) {
            return format_translation(pattern, &args);
        }
        if let Some(Value::String(fallback)) = obj.get("fallback") {
            return format_translation(fallback, &args);
        }
        return if args.is_empty() { key.clone() } else { format!("{key}[{}]", args.join(", ")) };
    }
    if let Some(Value::String(key)) = obj.get("keybind") {
        return key.clone();
    }
    if let Some(Value::String(sel)) = obj.get("selector") {
        return sel.clone();
    }
    String::new()
}

fn plain_of(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Array(items) => items.iter().map(plain_of).collect(),
        Value::Object(obj) => {
            let mut out = own_text(obj);
            if let Some(Value::Array(extra)) = obj.get("extra") {
                for child in extra {
                    out.push_str(&plain_of(child));
                }
            }
            out
        }
        Value::Null => String::new(),
    }
}

#[derive(Clone, Default)]
struct AnsiStyle {
    color: Option<String>,
    bold: bool,
    italic: bool,
    underlined: bool,
    strikethrough: bool,
}

impl AnsiStyle {
    fn merged(&self, obj: &Map<String, Value>) -> AnsiStyle {
        let mut s = self.clone();
        if let Some(Value::String(c)) = obj.get("color") {
            s.color = Some(c.clone());
        }
        let flag = |key: &str, cur: bool| obj.get(key).and_then(Value::as_bool).unwrap_or(cur);
        s.bold = flag("bold", s.bold);
        s.italic = flag("italic", s.italic);
        s.underlined = flag("underlined", s.underlined);
        s.strikethrough = flag("strikethrough", s.strikethrough);
        s
    }

    fn escape(&self) -> String {
        let mut codes: Vec<String> = vec!["0".into()];
        if let Some(c) = &self.color {
            if let Some(idx) = NAMED_COLORS.iter().position(|n| n == c) {
                const ANSI: [&str; 16] =
                    ["30", "34", "32", "36", "31", "35", "33", "37", "90", "94", "92", "96", "91", "95", "93", "97"];
                codes.push(ANSI[idx].into());
            } else if let Some(hex) = c.strip_prefix('#') {
                if let Ok(rgb) = u32::from_str_radix(hex, 16) {
                    codes.push(format!("38;2;{};{};{}", (rgb >> 16) & 255, (rgb >> 8) & 255, rgb & 255));
                }
            }
        }
        for (on, code) in [(self.bold, "1"), (self.italic, "3"), (self.underlined, "4"), (self.strikethrough, "9")] {
            if on {
                codes.push(code.into());
            }
        }
        if codes.len() == 1 {
            String::new()
        } else {
            format!("\x1b[{}m", codes.join(";"))
        }
    }
}

fn ansi_of(value: &Value, style: &AnsiStyle, out: &mut String) {
    match value {
        Value::Object(obj) => {
            let style = style.merged(obj);
            let text = own_text(obj);
            if !text.is_empty() {
                let esc = style.escape();
                if esc.is_empty() && out.contains('\x1b') {
                    out.push_str("\x1b[0m");
                }
                out.push_str(&esc);
                out.push_str(&text);
            }
            if let Some(Value::Array(extra)) = obj.get("extra") {
                for child in extra {
                    ansi_of(child, &style, out);
                }
            }
        }
        Value::Array(items) => {
            for item in items {
                ansi_of(item, style, out);
            }
        }
        other => {
            let text = plain_of(other);
            if !text.is_empty() {
                let esc = style.escape();
                if esc.is_empty() && out.contains('\x1b') {
                    out.push_str("\x1b[0m");
                }
                out.push_str(&esc);
                out.push_str(&text);
            }
        }
    }
}

/// Generic JSON to NBT conversion following the same rules as converting through vanilla's
/// `JsonOps` -> `NbtOps`: booleans become bytes, integral numbers ints (or longs when they do
/// not fit), other numbers doubles. Mixed-type lists are handled by the NBT writer.
pub fn json_to_nbt(value: &Value) -> Tag {
    match value {
        Value::Null => Tag::Compound(Compound::new()),
        Value::Bool(b) => Tag::Byte(*b as i8),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                match i32::try_from(i) {
                    Ok(v) => Tag::Int(v),
                    Err(_) => Tag::Long(i),
                }
            } else if let Some(u) = n.as_u64() {
                Tag::Long(u as i64)
            } else {
                Tag::Double(n.as_f64().unwrap_or(0.0))
            }
        }
        Value::String(s) => Tag::String(s.clone()),
        Value::Array(items) => Tag::List(items.iter().filter(|v| !v.is_null()).map(json_to_nbt).collect()),
        Value::Object(obj) => {
            let mut c = Compound::new();
            for (key, v) in obj {
                if !v.is_null() {
                    c.insert(key.clone(), json_to_nbt(v));
                }
            }
            Tag::Compound(c)
        }
    }
}

/// NBT to JSON, the inverse of [`json_to_nbt`] as far as the type information allows.
/// `key` is the compound key the tag was stored under, used to restore booleans.
pub fn nbt_to_json(tag: &Tag, key: Option<&str>) -> Value {
    match tag {
        Tag::Byte(v) => {
            if key.is_some_and(|k| BOOLEAN_KEYS.contains(&k)) {
                Value::Bool(*v != 0)
            } else {
                json!(*v)
            }
        }
        Tag::Short(v) => json!(*v),
        Tag::Int(v) => json!(*v),
        Tag::Long(v) => json!(*v),
        Tag::Float(v) => json!(*v),
        Tag::Double(v) => json!(*v),
        Tag::String(s) => Value::String(s.clone()),
        Tag::ByteArray(v) => Value::Array(v.iter().map(|b| json!(*b as i8)).collect()),
        Tag::IntArray(v) => Value::Array(v.iter().map(|i| json!(*i)).collect()),
        Tag::LongArray(v) => Value::Array(v.iter().map(|i| json!(*i)).collect()),
        Tag::List(items) => Value::Array(
            items
                .iter()
                .map(|item| match item {
                    // Elements of mixed lists are wrapped as {"": value} on the wire.
                    Tag::Compound(c) if c.len() == 1 && c.contains_key("") => nbt_to_json(c.get("").unwrap(), None),
                    other => nbt_to_json(other, None),
                })
                .collect(),
        ),
        Tag::Compound(c) => Value::Object(c.iter().map(|(k, v)| (k.clone(), nbt_to_json(v, Some(k)))).collect()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use paper_nbt::compound;

    #[test]
    fn plain_text_is_a_string_tag() {
        assert_eq!(Component::text("hi").to_nbt(), Tag::String("hi".into()));
    }

    #[test]
    fn styled_text_is_a_compound() {
        let c = Component::text("hi").color("red").bold(true);
        assert_eq!(c.to_nbt(), Tag::Compound(compound! { "text" => "hi", "color" => "red", "bold" => true }));
    }

    #[test]
    fn nbt_round_trip() {
        let c = Component::translatable("chat.type.text", vec![Component::text("Steve"), Component::text("hello").italic(true)])
            .color("#ff00aa")
            .append(Component::text("!"));
        let back = Component::from_nbt(&c.to_nbt());
        assert_eq!(back, c);
    }

    #[test]
    fn mixed_list_round_trip_through_wire() {
        // "extra" holding a bare string and an object must survive the wrapped wire encoding.
        let c = Component(json!({ "text": "a", "extra": ["b", { "text": "c", "bold": true }] }));
        let mut wire = Vec::new();
        paper_nbt::write_network(&mut wire, &c.to_nbt());
        let tag = paper_nbt::read_network(&mut &wire[..], usize::MAX).unwrap().unwrap();
        assert_eq!(Component::from_nbt(&tag), c);
        assert_eq!(c.to_plain(), "abc");
    }

    #[test]
    fn legacy_codes() {
        let c = Component::from_legacy("§aGreen §lbold§r plain");
        assert_eq!(
            c.0,
            json!({ "text": "", "extra": [
                { "text": "Green ", "color": "green" },
                { "text": "bold", "color": "green", "bold": true },
                { "text": " plain" },
            ]})
        );
        assert_eq!(c.to_plain(), "Green bold plain");
        assert_eq!(Component::from_legacy("no codes").0, json!({ "text": "no codes" }));
        // A colour code resets formatting.
        let c = Component::from_legacy("§l§cx");
        assert_eq!(c.0, json!({ "text": "x", "color": "red" }));
        // Unknown codes are kept verbatim.
        assert_eq!(Component::from_legacy("100§ ok §z").to_plain(), "100§ ok §z");
    }

    #[test]
    fn legacy_hex() {
        let c = Component::from_legacy("§x§1§2§3§a§b§cHex");
        assert_eq!(c.0, json!({ "text": "Hex", "color": "#123abc" }));
    }

    #[test]
    fn translations() {
        let c = Component::translatable("multiplayer.player.joined", vec![Component::text("Alex")]).color("yellow");
        assert_eq!(c.to_plain(), "Alex joined the game");
        let c = Component::translatable("some.unknown.key", vec![Component::text("a"), Component::text("b")]);
        assert_eq!(c.to_plain(), "some.unknown.key[a, b]");
        assert_eq!(format_translation("%2$s then %1$s, 100%%", &["one".into(), "two".into()]), "two then one, 100%");
    }

    #[test]
    fn json_numbers() {
        assert_eq!(json_to_nbt(&json!(5)), Tag::Int(5));
        assert_eq!(json_to_nbt(&json!(5_000_000_000i64)), Tag::Long(5_000_000_000));
        assert_eq!(json_to_nbt(&json!(0.5)), Tag::Double(0.5));
        assert_eq!(json_to_nbt(&json!(true)), Tag::Byte(1));
    }

    #[test]
    fn ansi_output() {
        let c = Component::text("warn").color("red").append(Component::text(" ok").color("green"));
        assert_eq!(c.to_ansi(), "\x1b[0;91mwarn\x1b[0;92m ok\x1b[0m");
        assert_eq!(Component::text("plain").to_ansi(), "plain");
    }

    #[test]
    fn builders_promote_strings() {
        let c = Component(Value::String("x".into())).color("blue");
        assert_eq!(c.0, json!({ "text": "x", "color": "blue" }));
    }
}
