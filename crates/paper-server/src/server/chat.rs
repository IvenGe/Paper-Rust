//! Chat and system messages.

use super::Server;
use paper_protocol::packets::play;
use paper_protocol::text::Component;

/// Characters vanilla refuses in chat: the section sign, control characters and DEL.
fn is_allowed_chat_char(c: char) -> bool {
    c != '§' && c >= ' ' && c != '\u{7F}'
}

/// Collapses runs of whitespace and trims, like vanilla's chat normalisation.
fn normalize(message: &str) -> String {
    message.split_whitespace().collect::<Vec<_>>().join(" ")
}

impl Server {
    /// A chat message from a player (typed, or sent for them by a plugin).
    pub fn handle_player_chat(&mut self, id: i32, message: &str) {
        let Some(player) = self.players.get(&id) else { return };
        if !message.chars().all(is_allowed_chat_char) {
            self.kick(id, &Component::translatable("multiplayer.disconnect.illegal_characters", vec![]));
            return;
        }
        let message = normalize(message);
        if message.is_empty() {
            return;
        }
        // "Chat: Hidden" and "Commands Only" clients may not send chat.
        if player.settings.chat_mode != 0 {
            player.conn.send(&play::SystemChat { content: Component::translatable("chat.disabled.options", vec![]).color("red"), overlay: false });
            return;
        }
        let name = player.name().to_owned();
        let sender = player.chat_name().insertion(&name).click_suggest_command(&format!("/tell {name} "));
        let line = Component::translatable("chat.type.text", vec![sender, Component::text(message.clone())]);
        // With chat listeners the plugins decide who sees what (AsyncChatEvent).
        if crate::plugins::events::on_chat(self, id, &message, line.clone()) {
            return;
        }
        log::info!("<{name}> {message}");
        // Player chat only reaches clients that have chat fully enabled.
        let packet = play::SystemChat { content: line, overlay: false };
        let payload = bytes::Bytes::from(paper_protocol::packets::encode_packet(&packet));
        for other in self.players.values().filter(|p| p.settings.chat_mode == 0) {
            other.conn.send_payload(payload.clone());
        }
    }

    /// Sends a system message to every player and logs it.
    pub fn broadcast_message(&self, message: &Component) {
        self.broadcast(&play::SystemChat { content: message.clone(), overlay: false });
        log::info!("{}", message.to_ansi());
    }

    /// Sends a system message to one player.
    pub fn send_message(&self, id: i32, message: &Component) {
        self.send(id, &play::SystemChat { content: message.clone(), overlay: false });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chat_rules() {
        assert!("hello there".chars().all(is_allowed_chat_char));
        assert!(!"col§or".chars().all(is_allowed_chat_char));
        assert!(!"tab\there".chars().all(is_allowed_chat_char));
        assert_eq!(normalize("  a   b  "), "a b");
        assert_eq!(normalize("   "), "");
    }
}
