//! Mob variants: the cat, chicken, cow, frog, pig or wolf a mob is (an entry of a variant
//! registry, `VariantUtils`), and the variants kept as numbers (rabbits, foxes, llamas,
//! parrots, axolotls, mooshrooms). They are picked where the mob spawns, passed on to its
//! young, shown to clients and saved.

use super::mobs::{biome_in_tag, moon_brightness};
use super::Server;
use crate::world::entity::Vec3;
use paper_data::variants::{self, Condition};
use rand::Rng;

/// The registry a mob's variant is an entry of.
pub fn registry_of(entity: &str) -> Option<&'static str> {
    Some(match entity {
        "minecraft:cat" => "minecraft:cat_variant",
        "minecraft:chicken" => "minecraft:chicken_variant",
        "minecraft:cow" => "minecraft:cow_variant",
        "minecraft:frog" => "minecraft:frog_variant",
        "minecraft:pig" => "minecraft:pig_variant",
        "minecraft:wolf" => "minecraft:wolf_variant",
        "minecraft:zombie_nautilus" => "minecraft:zombie_nautilus_variant",
        _ => return None,
    })
}

/// The entity data field a numbered variant is sent in.
pub fn numbered_field(entity: &str) -> Option<&'static str> {
    Some(match entity {
        "minecraft:rabbit" | "minecraft:fox" => "DATA_TYPE_ID",
        "minecraft:mooshroom" => "DATA_TYPE",
        "minecraft:llama" | "minecraft:trader_llama" | "minecraft:parrot" => "DATA_VARIANT_ID",
        "minecraft:axolotl" => "DATA_VARIANT",
        _ => return None,
    })
}

/// The variant a mob of a variant registry has when none is saved (`CowVariants.DEFAULT`,
/// ...).
pub fn default_id(registry: &str) -> i32 {
    let key = match registry {
        "minecraft:cat_variant" => "minecraft:black",
        "minecraft:wolf_variant" => "minecraft:pale",
        _ => "minecraft:temperate",
    };
    id_of(registry, key).unwrap_or(0)
}

/// A variant's key from its network id, and back.
pub fn key_of(registry: &str, id: i32) -> Option<&'static str> {
    paper_data::synced::entries(registry)?.get(usize::try_from(id).ok()?).copied()
}

pub fn id_of(registry: &str, key: &str) -> Option<i32> {
    paper_data::synced::id(registry, key)
}

/// How foxes and mooshrooms save their variant (`Type`).
pub fn type_name(entity: &str, variant: i32) -> Option<&'static str> {
    Some(match (entity, variant) {
        ("minecraft:fox", 1) => "snow",
        ("minecraft:fox", _) => "red",
        ("minecraft:mooshroom", 1) => "brown",
        ("minecraft:mooshroom", _) => "red",
        _ => return None,
    })
}

pub fn type_from_name(entity: &str, name: &str) -> i32 {
    matches!((entity, name), ("minecraft:fox", "snow") | ("minecraft:mooshroom", "brown")) as i32
}

impl Server {
    /// The variant a mob spawning here is given (`VariantUtils.selectVariantToSpawn`): one
    /// of the most preferred of those whose conditions hold. (Where structures are is not
    /// known outside world generation, so a condition on one does not hold.)
    pub fn pick_variant(&self, world: usize, registry: &str, pos: Vec3) -> Option<i32> {
        let biome = self.biome_at(world, pos.x.floor() as i32, pos.y.floor() as i32, pos.z.floor() as i32);
        let moon = moon_brightness(&self.worlds[world]) as f64;
        let holds = |c: &Condition| match c {
            Condition::Always => true,
            Condition::Biome(set) => set.iter().any(|b| match b.strip_prefix('#') {
                Some(tag) => biome_in_tag(biome, tag),
                None => b == biome,
            }),
            Condition::MoonBrightness { min, max } => min.is_none_or(|m| moon >= m) && max.is_none_or(|m| moon <= m),
            Condition::Structure(_) | Condition::Never => false,
        };
        let mut best = i32::MIN;
        let mut pool: Vec<&str> = Vec::new();
        for (name, selectors) in variants::of(registry) {
            for s in selectors {
                if s.priority < best || !holds(&s.condition) {
                    continue;
                }
                if s.priority > best {
                    best = s.priority;
                    pool.clear();
                }
                pool.push(name);
            }
        }
        if pool.is_empty() {
            return None;
        }
        id_of(registry, pool[rand::thread_rng().gen_range(0..pool.len())])
    }

    /// A numbered variant for a mob spawning here (`finalizeSpawn`): rabbits and foxes by
    /// biome, llamas, parrots and axolotls (all but the rare blue) at random.
    pub fn pick_numbered_variant(&self, world: usize, entity: &str, pos: Vec3) -> Option<i32> {
        let mut rng = rand::thread_rng();
        let biome = || self.biome_at(world, pos.x.floor() as i32, pos.y.floor() as i32, pos.z.floor() as i32);
        Some(match entity {
            "minecraft:rabbit" => rabbit_for(biome(), rng.gen_range(0..100)),
            "minecraft:fox" => biome_in_tag(biome(), "minecraft:spawns_snow_foxes") as i32,
            "minecraft:llama" | "minecraft:trader_llama" => rng.gen_range(0..4),
            "minecraft:parrot" => rng.gen_range(0..5),
            "minecraft:axolotl" => rng.gen_range(0..4),
            _ => return None,
        })
    }

    /// A young animal's variant, from its parents' (`getBreedOffspring`): either parent's;
    /// a horse's coat and markings each from either or, now and then, neither; now and then
    /// a rabbit of its biome, a rare blue axolotl, a mooshroom of the other colour.
    pub fn offspring_variant(&self, world: usize, entity: &str, pos: Vec3, a: i32, b: i32) -> Option<i32> {
        let mut rng = rand::thread_rng();
        let either = if rng.gen::<bool>() { a } else { b };
        Some(match entity {
            "minecraft:horse" => {
                let i = rng.gen_range(0..9);
                let coat = if i < 4 { a & 0xFF } else if i < 8 { b & 0xFF } else { rng.gen_range(0..7) };
                let j = rng.gen_range(0..5);
                let markings = if j < 2 { a >> 8 } else if j < 4 { b >> 8 } else { rng.gen_range(0..5) };
                coat | markings << 8
            }
            "minecraft:rabbit" if rng.gen_range(0..20) == 0 => {
                rabbit_for(self.biome_at(world, pos.x.floor() as i32, pos.y.floor() as i32, pos.z.floor() as i32), rng.gen_range(0..100))
            }
            "minecraft:axolotl" if rng.gen_range(0..1200) == 0 => 4,
            "minecraft:mooshroom" if a == b && rng.gen_range(0..1024) == 0 => 1 - a,
            e if e == "minecraft:sheep" || registry_of(e).is_some() || numbered_field(e).is_some() => either,
            _ => return None,
        })
    }
}

/// `Rabbit.getRandomRabbitVariant`: white (or splotched) in snowy places, gold in deserts,
/// else brown, salt and pepper or black. `roll` is 0-99.
fn rabbit_for(biome: &str, roll: i32) -> i32 {
    if biome_in_tag(biome, "minecraft:spawns_white_rabbits") {
        if roll < 80 {
            1
        } else {
            3
        }
    } else if biome_in_tag(biome, "minecraft:spawns_gold_rabbits") {
        4
    } else if roll < 50 {
        0
    } else if roll < 90 {
        5
    } else {
        2
    }
}
