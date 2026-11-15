//! A load benchmark, not run by default: bots walking about and mobs wandering, with the
//! time each part of the tick takes. Run it in release mode:
//!
//! ```text
//! cargo test --release -p paper-server --test load -- --ignored --nocapture
//! ```
//!
//! `BENCH_PLAYERS` (20), `BENCH_MOBS` (600), `BENCH_SECONDS` (30), `BENCH_VIEW` (8) and
//! `BENCH_WORLD` (`flat`, or `vanilla` to generate terrain as the bots walk) set the load.

mod common;

use common::*;
use paper_protocol::packets::play;
use paper_server::config::GameMode;
use paper_server::server::mobs::SpawnReason;
use paper_server::world::entity::Vec3;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

fn env(name: &str, default: u64) -> u64 {
    std::env::var(name).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}

/// CPU time the whole process has used (every thread, the bots' included), where Linux says.
fn cpu_time() -> Option<Duration> {
    let stat = std::fs::read_to_string("/proc/self/stat").ok()?;
    let fields: Vec<&str> = stat.rsplit_once(')')?.1.split_whitespace().collect();
    let ticks: u64 = fields.get(11)?.parse::<u64>().ok()? + fields.get(12)?.parse::<u64>().ok()?;
    Some(Duration::from_millis(ticks * 10))
}

#[test]
#[ignore]
fn load_benchmark() {
    let (players, mobs, seconds, view) = (env("BENCH_PLAYERS", 20) as usize, env("BENCH_MOBS", 600) as usize, env("BENCH_SECONDS", 30), env("BENCH_VIEW", 8));
    let vanilla = std::env::var("BENCH_WORLD").is_ok_and(|w| w == "vanilla");
    // Saved to a directory of its own, to see what saving costs.
    let root = std::env::temp_dir().join(format!("paper-rs-load-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let server = start_server(|c| {
        c.root = root.clone();
        c.properties.gamemode = GameMode::Creative;
        c.properties.view_distance = view as i32;
        c.properties.simulation_distance = view.min(8) as i32;
        if vanilla {
            c.paper.world.generator = "vanilla".into();
        }
        c.paper.threads.workers = 0;
        c.paper.threads.io = 0;
    });
    let stop = Arc::new(AtomicBool::new(false));
    // Bots join spread over a circle and walk outwards and back, as players exploring.
    let mut bots = Vec::new();
    for i in 0..players {
        let port = server.port();
        let stop = stop.clone();
        bots.push(std::thread::spawn(move || {
            let mut bot = Client::join(port, &format!("Bot{i}"));
            let start: play::PlayerPosition = bot.expect();
            let angle = i as f64 / players.max(1) as f64 * std::f64::consts::TAU;
            let (dx, dz) = (angle.cos() * 0.2, angle.sin() * 0.2);
            let (mut x, y, mut z) = (start.x + angle.cos() * 48.0, start.y, start.z + angle.sin() * 48.0);
            let mut step = 0u32;
            while !stop.load(Ordering::Relaxed) {
                let outward = (step / 400) % 2 == 0;
                let sign = if outward { 1.0 } else { -1.0 };
                x += dx * sign;
                z += dz * sign;
                bot.move_to(x, y, z);
                bot.drain(Duration::from_millis(50));
                bot.clear_backlog();
                step += 1;
            }
        }));
    }
    std::thread::sleep(Duration::from_secs(5));
    // Mobs around the bots: animals and zombies wandering and pathfinding.
    let kinds = ["minecraft:cow", "minecraft:sheep", "minecraft:pig", "minecraft:chicken", "minecraft:zombie", "minecraft:skeleton", "minecraft:villager"];
    server
        .handle
        .run(move |s| {
            let ids: Vec<i32> = s.players.keys().copied().collect();
            let mut rng = rand::thread_rng();
            for i in 0..mobs {
                let Some(p) = ids.get(i % ids.len().max(1)).and_then(|id| s.players.get(id)) else { break };
                let at = Vec3::new(p.loc.x + rand::Rng::gen_range(&mut rng, -24.0..24.0), p.loc.y + 1.0, p.loc.z + rand::Rng::gen_range(&mut rng, -24.0..24.0));
                s.spawn_mob(p.world, kinds[i % kinds.len()], at, SpawnReason::Command);
            }
        })
        .unwrap();
    // Warm up, then measure.
    std::thread::sleep(Duration::from_secs(10));
    server.handle.run(|s| drop(s.take_profile())).unwrap();
    let (started, cpu_before) = (Instant::now(), cpu_time());
    std::thread::sleep(Duration::from_secs(seconds));
    let (wall, cpu_after) = (started.elapsed(), cpu_time());
    let (profile, entities, chunks, mspt, save) = server
        .handle
        .run(|s| {
            let entities: usize = s.worlds.iter().map(|w| w.entities.len()).sum();
            let chunks: usize = s.worlds.iter().map(|w| w.chunks.len()).sum();
            let mspt = format!("mspt avg {:.2}, worst {:.2} over the last 20s", s.stats.mspt(400), s.stats.worst(400));
            // What an autosave of everything costs the main thread.
            s.worlds.iter_mut().flat_map(|w| w.chunks.values_mut()).for_each(|c| c.dirty = true);
            let started = Instant::now();
            s.save_all(false);
            (s.take_profile(), entities, chunks, mspt, started.elapsed())
        })
        .unwrap();
    stop.store(true, Ordering::Relaxed);
    for bot in bots {
        let _ = bot.join();
    }
    let ticks = profile.ticks;
    println!("\n== load: {players} players, {entities} entities, {chunks} chunks, view {view}, {} world ==", if vanilla { "vanilla" } else { "flat" });
    println!("{ticks} ticks in {:.1}s ({:.1} TPS); {mspt}", wall.as_secs_f64(), ticks as f64 / wall.as_secs_f64());
    println!("saving every chunk took the main thread {:.1} ms", save.as_secs_f64() * 1000.0);
    if let (Some(before), Some(after)) = (cpu_before, cpu_after) {
        println!("process CPU {:.1}s: {:.2} cores busy on average (bots included)", (after - before).as_secs_f64(), (after - before).as_secs_f64() / wall.as_secs_f64());
    }
    let mut total = 0.0;
    for (phase, ms) in profile.per_tick() {
        // Parts of parts (`entities/moving`) are in their part's time already.
        if !phase.contains('/') {
            total += ms;
        }
        if ms >= 0.005 {
            println!("{ms:>9.3} ms/tick  {phase}");
        }
    }
    println!("{total:>9.3} ms/tick  in all");
    if let Some((took, parts)) = &profile.worst {
        let parts: Vec<String> = parts.iter().map(|(phase, t)| format!("{phase} {:.1}", t.as_secs_f64() * 1000.0)).collect();
        println!("slowest tick {:.1} ms: {}", took.as_secs_f64() * 1000.0, parts.join(", "));
    }
    server.stop();
    let _ = std::fs::remove_dir_all(&root);
}
