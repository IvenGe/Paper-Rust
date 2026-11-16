//! Text components as old clients read them: 1.8 JSON (camelCase events, named colours, no
//! fonts or shadow colours) and, before JSON text (1.6 and older), strings with `§` codes.

use serde_json::{json, Map, Value};

/// The named colours, with their RGB values, in their legacy code order (`§0` to `§f`).
const COLORS: [(&str, u32); 16] = [
    ("black", 0x000000),
    ("dark_blue", 0x0000AA),
    ("dark_green", 0x00AA00),
    ("dark_aqua", 0x00AAAA),
    ("dark_red", 0xAA0000),
    ("dark_purple", 0xAA00AA),
    ("gold", 0xFFAA00),
    ("gray", 0xAAAAAA),
    ("dark_gray", 0x555555),
    ("blue", 0x5555FF),
    ("green", 0x55FF55),
    ("aqua", 0x55FFFF),
    ("red", 0xFF5555),
    ("light_purple", 0xFF55FF),
    ("yellow", 0xFFFF55),
    ("white", 0xFFFFFF),
];

/// The named colour of a colour value (`red`, or `#rrggbb`, which gets the nearest).
fn named_color(color: &str) -> Option<&'static str> {
    if let Some((name, _)) = COLORS.iter().find(|(name, _)| *name == color) {
        return Some(name);
    }
    let rgb = u32::from_str_radix(color.strip_prefix('#')?, 16).ok()?;
    let channel = |v: u32, shift: u32| ((v >> shift) & 0xFF) as i32;
    COLORS
        .iter()
        .min_by_key(|(_, c)| {
            let (dr, dg, db) = (channel(rgb, 16) - channel(*c, 16), channel(rgb, 8) - channel(*c, 8), channel(rgb, 0) - channel(*c, 0));
            dr * dr + dg * dg + db * db
        })
        .map(|(name, _)| *name)
}

/// A component (as 26.3 writes it in JSON) as 1.8 reads it.
pub fn to_1_8(value: &Value) -> Value {
    match value {
        Value::String(_) => value.clone(),
        Value::Array(items) if !items.is_empty() => {
            let mut first = to_1_8(&items[0]);
            if !first.is_object() {
                first = json!({ "text": first.as_str().unwrap_or("") });
            }
            let extra: Vec<Value> = items[1..].iter().map(to_1_8).collect();
            if let Some(o) = first.as_object_mut() {
                let mut all = o.get("extra").and_then(Value::as_array).cloned().unwrap_or_default();
                all.extend(extra);
                o.insert("extra".into(), Value::Array(all));
            }
            first
        }
        Value::Object(o) => {
            let mut out = Map::new();
            for (key, v) in o {
                match key.as_str() {
                    "text" | "translate" | "bold" | "italic" | "underlined" | "strikethrough" | "obfuscated" | "insertion" | "selector" | "score" => {
                        out.insert(key.clone(), v.clone());
                    }
                    "color" => {
                        if let Some(name) = v.as_str().and_then(named_color) {
                            out.insert("color".into(), Value::String(name.into()));
                        }
                    }
                    "with" | "extra" => {
                        if let Some(items) = v.as_array() {
                            out.insert(key.clone(), Value::Array(items.iter().map(to_1_8).collect()));
                        }
                    }
                    "keybind" => {
                        out.insert("text".into(), Value::String(v.as_str().unwrap_or("").trim_start_matches("key.").to_owned()));
                    }
                    "click_event" | "clickEvent" => {
                        if let Some(event) = click_event(v) {
                            out.insert("clickEvent".into(), event);
                        }
                    }
                    "hover_event" | "hoverEvent" => {
                        let show_text = v.get("action").and_then(Value::as_str) == Some("show_text");
                        if let (true, Some(text)) = (show_text, v.get("value").or_else(|| v.get("contents"))) {
                            out.insert("hoverEvent".into(), json!({ "action": "show_text", "value": to_1_8(text) }));
                        }
                    }
                    _ => {}
                }
            }
            if !["text", "translate", "score", "selector"].iter().any(|k| out.contains_key(*k)) {
                out.insert("text".into(), Value::String(String::new()));
            }
            Value::Object(out)
        }
        other => json!({ "text": other.to_string() }),
    }
}

fn click_event(v: &Value) -> Option<Value> {
    let action = v.get("action")?.as_str()?;
    let value = match action {
        "open_url" => v.get("url").or_else(|| v.get("value"))?.as_str()?.to_owned(),
        "run_command" | "suggest_command" => {
            let command = v.get("command").or_else(|| v.get("value"))?.as_str()?;
            if command.starts_with('/') || action == "suggest_command" {
                command.to_owned()
            } else {
                format!("/{command}")
            }
        }
        "change_page" => match v.get("page").or_else(|| v.get("value"))? {
            Value::Number(n) => n.to_string(),
            Value::String(s) => s.clone(),
            _ => return None,
        },
        _ => return None,
    };
    Some(json!({ "action": action, "value": value }))
}

/// A 1.8 JSON string of a component.
pub fn json_1_8(value: &Value) -> String {
    to_1_8(value).to_string()
}

/// The plain text of a 1.8 JSON component (sign lines a client sends).
pub fn plain(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        other => paper_protocol::text::Component(other.clone()).to_plain(),
    }
}

/// The text of a component with `§` codes (for clients from before JSON text); translated
/// components show their English text where the server knows it.
pub fn legacy_string(value: &Value) -> String {
    let mut out = String::new();
    write_legacy(value, &mut out);
    out
}

fn write_legacy(value: &Value, out: &mut String) {
    match value {
        Value::String(s) => out.push_str(s),
        Value::Array(items) => items.iter().for_each(|v| write_legacy(v, out)),
        Value::Object(o) => {
            if let Some(code) = o.get("color").and_then(Value::as_str).and_then(named_color).and_then(|n| COLORS.iter().position(|(c, _)| *c == n)) {
                out.push('§');
                out.push(char::from_digit(code as u32, 16).unwrap_or('f'));
            }
            for (key, code) in [("obfuscated", 'k'), ("bold", 'l'), ("strikethrough", 'm'), ("underlined", 'n'), ("italic", 'o')] {
                if o.get(key).and_then(Value::as_bool) == Some(true) {
                    out.push('§');
                    out.push(code);
                }
            }
            if o.contains_key("translate") {
                out.push_str(&paper_protocol::text::Component(value.clone()).to_plain());
            } else if let Some(text) = o.get("text").and_then(Value::as_str) {
                out.push_str(text);
            }
            if let Some(extra) = o.get("extra").and_then(Value::as_array) {
                extra.iter().for_each(|v| write_legacy(v, out));
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modern_components_become_1_8_ones() {
        let modern = json!({ "text": "hi", "color": "#fe5050", "click_event": { "action": "run_command", "command": "spawn" }, "hover_event": { "action": "show_text", "value": "tip" }, "font": "uniform" });
        assert_eq!(to_1_8(&modern), json!({ "text": "hi", "color": "red", "clickEvent": { "action": "run_command", "value": "/spawn" }, "hoverEvent": { "action": "show_text", "value": "tip" } }));
        assert_eq!(legacy_string(&json!({ "text": "a", "color": "gold", "extra": [{ "text": "b", "bold": true }] })), "§6a§lb");
    }
}
