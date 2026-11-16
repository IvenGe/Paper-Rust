//! An example Rust plugin: welcomes players, keeps diamond blocks from being broken, and adds
//! `/hello` and `/ticks`. Build it with
//!
//! ```text
//! cargo build --release -p paper-plugin --example hello_plugin
//! ```
//!
//! and copy `target/release/examples/libhello_plugin.so` (`hello_plugin.dll` on Windows,
//! `libhello_plugin.dylib` on macOS) into the server's `plugins` folder.

use paper_plugin::{export_plugin, CommandInfo, Event, Info, Plugin, Sender, Server};

#[derive(Default)]
struct Hello {
    ticks: u64,
}

impl Plugin for Hello {
    fn info(&self) -> Info {
        Info::new("HelloRust", "1.0.0")
            .description("Says hello, from Rust")
            .author("Paper-RS")
            .listen("event.join")
            .listen("BlockBreakEvent")
            .command_with(CommandInfo { name: "hello".into(), description: "Says hello".into(), usage: "/hello [name]".into(), aliases: vec!["hi-rust".into()], op_level: 0 })
            .command("ticks", "How many ticks the plugin has seen")
            .ticks()
    }

    fn enable(&mut self, server: &mut Server) {
        server.log("HelloRust is enabled");
    }

    fn event(&mut self, server: &mut Server, event: &mut Event) {
        if event.is("event.join") {
            if let Some(player) = event.player() {
                player.send_message(server, "Welcome, from a Rust plugin!");
            }
        } else if event.is("BlockBreakEvent") && event.get("state").as_str() == Some("minecraft:diamond_block") {
            event.cancel();
            if let Some(player) = event.player() {
                player.send_message(server, "Diamond blocks stay where they are.");
            }
        }
    }

    fn command(&mut self, server: &mut Server, sender: &Sender, label: &str, args: &[String]) -> bool {
        match label {
            "hello" => {
                let name = args.first().cloned().or_else(|| sender.player().map(|p| p.name.clone())).unwrap_or_else(|| "console".into());
                sender.send_message(server, &format!("Hello, {name}, from Rust!"));
                true
            }
            "ticks" => {
                sender.send_message(server, &format!("{} ticks", self.ticks));
                true
            }
            _ => false,
        }
    }

    fn complete(&mut self, server: &mut Server, _sender: &Sender, label: &str, args: &[String]) -> Vec<String> {
        if label != "hello" || args.len() > 1 {
            return Vec::new();
        }
        let typed = args.first().map(String::as_str).unwrap_or("");
        server.players().into_iter().map(|p| p.name).filter(|n| n.to_lowercase().starts_with(&typed.to_lowercase())).collect()
    }

    fn tick(&mut self, _server: &mut Server) {
        self.ticks += 1;
    }
}

export_plugin!(Hello);
