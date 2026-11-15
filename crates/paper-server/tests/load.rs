//! A load benchmark, not run by default: bots walking about and mobs wandering, with the
//! time each part of the tick takes. Run it in release mode:
//!
//! ```text
//! cargo test --release -p paper-server --test load -- --ignored --nocapture
//! ```
//!
//! `BENCH_PLAYERS` (20), `BENCH_MOBS` (600), `BENCH_SECONDS` (30), `BENCH_VIEW` (8) and
//! `BENCH_WORLD` (`flat`, or `vanilla` to generate terrain as the bots walk) set the load.
//! `BENCH_PLUGINS=1` runs the test plugins in the plugin host too (build them first), with
//! the bukkit test plugin's benchmark load: listeners for moves, spawns and damage, a line on
//! every player's screen each tick and a look at the entities around each player every
//! second.

mod common;

use common::*;
use paper_protocol::packets::play;
use paper_server::config::GameMode;
use paper_server::config::Config;
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
    let with_plugins = std::env::var("BENCH_PLUGINS").is_ok_and(|p| p == "1");
    // Saved to a directory of its own, to see what saving costs.
    let root = std::env::temp_dir().join(format!("paper-rs-load-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let configure = |c: &mut Config| {
        c.root = root.clone();
        c.properties.gamemode = GameMode::Creative;
        c.properties.view_distance = view as i32;
        c.properties.simulation_distance = view.min(8) as i32;
        if vanilla {
            c.paper.world.generator = "vanilla".into();
        }
        c.paper.threads.workers = 0;
        c.paper.threads.io = 0;
    };
    let server = if with_plugins {
        let setup = setup().expect("BENCH_PLUGINS needs Java 25 and the plugin host and test plugins built");
        let plugins_dir = root.join("plugins");
        std::fs::create_dir_all(&plugins_dir).unwrap();
        for jar in &setup.plugins {
            std::fs::copy(jar, plugins_dir.join(jar.file_name().unwrap())).unwrap();
        }
        start_server_with_host(
            |c| {
                configure(c);
                c.paper.plugins.java = setup.java.clone();
                c.paper.plugins.host_dir = setup.host_dir.display().to_string();
                c.paper.plugins.plugins_dir = plugins_dir.display().to_string();
                c.paper.plugins.jvm_args = vec!["-Xmx1G".into(), "-Dpaperrs.bench=true".into()];
                // `BENCH_JFR=<file>` records the plugin host with Java Flight Recorder.
                if let Ok(file) = std::env::var("BENCH_JFR") {
                    c.paper.plugins.jvm_args.push(format!("-XX:StartFlightRecording=filename={file},settings=profile,delay=15s,duration=25s"));
                }
                c.paper.plugins.startup_timeout_seconds = 120;
            },
            paper_server::plugins::java_launcher(),
        )
    } else {
        start_server(configure)
    };
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
    server
        .handle
        .run(|s| {
            drop(s.take_profile());
            if let Some(host) = &s.plugins {
                host.take_stats();
            }
        })
        .unwrap();
    let (started, cpu_before) = (Instant::now(), cpu_time());
    std::thread::sleep(Duration::from_secs(seconds));
    let (wall, cpu_after) = (started.elapsed(), cpu_time());
    let (profile, entities, chunks, mspt, save, bridge, round_trip) = server
        .handle
        .run(|s| {
            let bridge = s.plugins.as_ref().map(|host| host.take_stats());
            // A bare round trip to the plugin host and back: a call it answers at once.
            let round_trip = s.plugins.clone().map(|host| {
                let args = paper_bridge::bmap! { "uuid" => "00000000-0000-0000-0000-000000000000" };
                let started = Instant::now();
                for _ in 0..200 {
                    host.call(s, "command.visible", args.clone()).expect("the host answers");
                }
                started.elapsed() / 200
            });
            let entities: usize = s.worlds.iter().map(|w| w.entities.len()).sum();
            let chunks: usize = s.worlds.iter().map(|w| w.chunks.len()).sum();
            let mspt = format!("mspt avg {:.2}, worst {:.2} over the last 20s", s.stats.mspt(400), s.stats.worst(400));
            // What an autosave of everything costs the main thread.
            s.worlds.iter_mut().flat_map(|w| w.chunks.values_mut()).for_each(|c| c.dirty = true);
            let started = Instant::now();
            s.save_all(false);
            (s.take_profile(), entities, chunks, mspt, started.elapsed(), bridge, round_trip)
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
    if let Some(round_trip) = round_trip {
        println!("a bare call to the plugin host and back takes {:.0} us", round_trip.as_secs_f64() * 1.0e6);
    }
    if let Some(b) = bridge {
        let per_tick = |n: u64| n as f64 / profile.ticks.max(1) as f64;
        let micros = |d: Duration, n: u64| d.as_secs_f64() * 1.0e6 / n.max(1) as f64;
        println!(
            "plugins: {:.1} calls to the host a tick, {:.0} us each (its calls served meanwhile included); {:.1} of its calls served a tick ({:.1} not waited for), {:.0} us each to run",
            per_tick(b.calls),
            micros(b.waited, b.calls),
            per_tick(b.served),
            per_tick(b.served_without_reply),
            micros(b.serving, b.served)
        );
        let mut by_method = b.by_method.clone();
        by_method.sort_by(|a, b| b.2.cmp(&a.2));
        for (method, calls, waited) in by_method.iter().take(8) {
            println!("{:>9.3} ms/tick  {method}: {:.2} a tick, {:.0} us each", waited.as_secs_f64() * 1000.0 / profile.ticks.max(1) as f64, per_tick(*calls), micros(*waited, *calls));
        }
        let mut served = b.served_by_method.clone();
        served.sort_by(|a, b| b.1.cmp(&a.1));
        let served: Vec<String> = served.iter().take(10).map(|(method, calls, took)| format!("{method} {:.2}/tick {:.0}us", per_tick(*calls), micros(*took, *calls))).collect();
        println!("served: {}", served.join(", "));
        println!("player states looked over {:.1} times a tick, {:.0} us each, {:.1} sent a tick", per_tick(b.state_flushes), micros(b.state_flushing, b.state_flushes), per_tick(b.states_sent));
    }
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
