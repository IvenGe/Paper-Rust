//! Villagers and wandering traders (`AbstractVillager`): professions from workstations,
//! trades from vanilla's trade data (`trade_set`, `villager_trade` and their tags in the
//! server jar), the trading window with its prices, experience and levels, restocking at
//! the workstation, gossip, and the wandering trader spawner.
//!
//! What is not here: breeding, sleeping in beds, meeting at bells, farming, iron golems and
//! zombie villagers (villagers wander, panic and work, but keep no home).

use super::containers::{MenuKind, MerchantRef};
use super::effects::ActiveEffect;
use super::entities::{max_stack, same_item};
use super::loot::{LootContext, LootTables};
use super::menu::{ClickResult, Layout};
use super::mobs::SpawnReason;
use super::survival::category;
use super::Server;
use crate::config::GameMode;
use crate::vanilla::VanillaData;
use crate::world::chunk::ChunkPos;
use crate::world::entity::{Mob, Vec3};
use crate::world::merchant::{GossipType, ItemCost, Merchant, Offer};
use crate::world::World;
use paper_data::{blocks, items};
use paper_protocol::codec::BlockPos;
use paper_protocol::packets::play;
use paper_protocol::text::Component;
use paper_protocol::types::{ItemStack, Slot};
use parking_lot::RwLock;
use rand::Rng;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, OnceLock};

pub const VILLAGER: &str = "minecraft:villager";
pub const WANDERING_TRADER: &str = "minecraft:wandering_trader";

/// Whether an entity type trades.
pub fn is_merchant(name: &str) -> bool {
    name == VILLAGER || name == WANDERING_TRADER
}

/// The workstations of each profession (`PoiTypes`; each takes one villager).
const JOB_SITES: [(&str, &[&str]); 13] = [
    ("minecraft:armorer", &["minecraft:blast_furnace"]),
    ("minecraft:butcher", &["minecraft:smoker"]),
    ("minecraft:cartographer", &["minecraft:cartography_table"]),
    ("minecraft:cleric", &["minecraft:brewing_stand"]),
    ("minecraft:farmer", &["minecraft:composter"]),
    ("minecraft:fisherman", &["minecraft:barrel"]),
    ("minecraft:fletcher", &["minecraft:fletching_table"]),
    ("minecraft:leatherworker", &["minecraft:cauldron", "minecraft:water_cauldron", "minecraft:lava_cauldron", "minecraft:powder_snow_cauldron"]),
    ("minecraft:librarian", &["minecraft:lectern"]),
    ("minecraft:mason", &["minecraft:stonecutter"]),
    ("minecraft:shepherd", &["minecraft:loom"]),
    ("minecraft:toolsmith", &["minecraft:smithing_table"]),
    ("minecraft:weaponsmith", &["minecraft:grindstone"]),
];

/// The profession a workstation gives.
pub fn profession_of_job_site(block: &str) -> Option<&'static str> {
    JOB_SITES.iter().find(|(_, b)| b.contains(&block)).map(|(p, _)| *p)
}

/// Whether a villager of `profession` can work at `block` (the unemployed at any).
fn job_site_for(profession: &str, block: &str) -> bool {
    match profession {
        "minecraft:none" => profession_of_job_site(block).is_some(),
        "minecraft:nitwit" => false,
        p => profession_of_job_site(block) == Some(p),
    }
}

/// Every block state of every workstation, to find them quickly in chunk sections.
fn job_site_states() -> &'static [u16] {
    static STATES: OnceLock<Vec<u16>> = OnceLock::new();
    STATES.get_or_init(|| {
        JOB_SITES
            .iter()
            .flat_map(|(_, b)| b.iter())
            .filter_map(|name| blocks::by_name(name))
            .flat_map(|b| (b.min_state..b.min_state + b.state_count).map(|s| s as u16))
            .collect()
    })
}

/// `VillagerType.byBiome`: villagers look like where they were born.
pub fn villager_type_for_biome(biome: &str) -> &'static str {
    match biome {
        "minecraft:badlands" | "minecraft:desert" | "minecraft:eroded_badlands" | "minecraft:wooded_badlands" => "minecraft:desert",
        "minecraft:bamboo_jungle" | "minecraft:jungle" | "minecraft:sparse_jungle" => "minecraft:jungle",
        "minecraft:savanna_plateau" | "minecraft:savanna" | "minecraft:windswept_savanna" => "minecraft:savanna",
        "minecraft:deep_frozen_ocean" | "minecraft:frozen_ocean" | "minecraft:frozen_river" | "minecraft:ice_spikes" | "minecraft:snowy_beach" | "minecraft:snowy_taiga" | "minecraft:snowy_plains" | "minecraft:grove" | "minecraft:snowy_slopes" | "minecraft:frozen_peaks" | "minecraft:jagged_peaks" => "minecraft:snow",
        "minecraft:swamp" | "minecraft:mangrove_swamp" => "minecraft:swamp",
        "minecraft:old_growth_spruce_taiga" | "minecraft:old_growth_pine_taiga" | "minecraft:windswept_gravelly_hills" | "minecraft:windswept_hills" | "minecraft:taiga" | "minecraft:windswept_forest" => "minecraft:taiga",
        _ => "minecraft:plains",
    }
}

fn path_of(id: &str) -> &str {
    id.strip_prefix("minecraft:").unwrap_or(id)
}

// --- trade data ----------------------------------------------------------------------------

/// Vanilla's trades, read from the server jar when first needed.
pub struct Trades {
    data: Option<Arc<VanillaData>>,
    cache: RwLock<HashMap<String, Option<Arc<Value>>>>,
}

/// The cost of a trade (`wants`, `additional_wants`): an item, a count and component values.
fn item_cost(v: &Value) -> Option<ItemCost> {
    let item = items::id(v["id"].as_str()?)? as i32;
    let count = v["count"].as_i64().unwrap_or(1) as i32;
    Some(ItemCost { item, count, components: components(&v["components"])? })
}

/// What a trade gives, before its item modifiers.
fn trade_stack(v: &Value) -> Option<ItemStack> {
    let item = items::id(v["id"].as_str()?)? as i32;
    Some(ItemStack { item, count: v["count"].as_i64().unwrap_or(1) as i32, added: components(&v["components"])?, removed: Vec::new() })
}

/// Component values of a trade item in their network form. Only `potion_contents` with a
/// plain potion appears in vanilla's trades; anything else makes the trade unusable.
fn components(v: &Value) -> Option<Vec<(i32, Vec<u8>)>> {
    let Some(map) = v.as_object() else { return Some(Vec::new()) };
    let mut out = Vec::new();
    for (name, value) in map {
        let ty = paper_data::registries::id("minecraft:data_component_type", name)? as i32;
        let data = match name.as_str() {
            "minecraft:potion_contents" => {
                let potion = value.as_str().or_else(|| value["potion"].as_str())?;
                super::loot::potion_contents(paper_data::registries::id("minecraft:potion", potion)? as i32)
            }
            _ => {
                log::debug!("Trade item component {name} is not supported");
                return None;
            }
        };
        out.push((ty, data));
    }
    Some(out)
}

/// A trade's `merchant_predicate`: in vanilla's data, which villager types offer it.
fn merchant_matches(p: &Value, merchant: &Merchant) -> bool {
    match p["type"].as_str().or_else(|| p["condition"].as_str()).unwrap_or("") {
        "minecraft:entity_properties" => {
            let predicates = &p["predicate"]["minecraft:predicates"];
            match &predicates["minecraft:villager/variant"] {
                Value::Null => true,
                Value::String(s) if s.starts_with('#') => true,
                Value::String(s) => *s == merchant.villager_type,
                Value::Array(list) => list.iter().any(|v| v.as_str() == Some(&merchant.villager_type)),
                _ => true,
            }
        }
        "minecraft:all_of" => p["terms"].as_array().is_none_or(|t| t.iter().all(|t| merchant_matches(t, merchant))),
        "minecraft:any_of" => p["terms"].as_array().is_some_and(|t| t.iter().any(|t| merchant_matches(t, merchant))),
        "minecraft:inverted" => !merchant_matches(&p["term"], merchant),
        _ => true,
    }
}

/// Whether a stack has (or stores) an enchantment of an enchantment tag.
fn has_enchantment_in(stack: &ItemStack, tag: &str) -> bool {
    let Some(ids) = paper_data::synced::tag("minecraft:enchantment", tag.trim_start_matches('#')) else { return false };
    ["minecraft:enchantments", "minecraft:stored_enchantments"].iter().any(|c| super::enchanting::read_enchantments(stack, c).iter().any(|(id, _)| ids.contains(id)))
}

impl Trades {
    pub fn new(data: Option<Arc<VanillaData>>) -> Self {
        Self { data, cache: RwLock::new(HashMap::new()) }
    }

    /// `data/<namespace>/<kind>/<path>.json`.
    fn json(&self, kind: &str, id: &str) -> Option<Arc<Value>> {
        let (ns, path) = id.split_once(':').unwrap_or(("minecraft", id));
        let file = format!("data/{ns}/{kind}/{path}.json");
        if let Some(v) = self.cache.read().get(&file) {
            return v.clone();
        }
        let v = self.data.as_ref().and_then(|d| d.read_json(&file)).map(Arc::new);
        self.cache.write().insert(file, v.clone());
        v
    }

    /// The trades a `trades` value names: a tag (`#minecraft:farmer/level_1`, tags inside
    /// it resolved), a trade, or a list of them.
    fn resolve(&self, v: &Value, out: &mut Vec<String>, depth: u32) {
        match v {
            Value::String(s) => match s.strip_prefix('#') {
                Some(tag) if depth < 8 => {
                    let Some(t) = self.json("tags/villager_trade", tag) else { return };
                    for value in t["values"].as_array().into_iter().flatten() {
                        let id = value.as_str().or_else(|| value["id"].as_str()).unwrap_or("");
                        self.resolve(&Value::String(id.to_owned()), out, depth + 1);
                    }
                }
                Some(_) => {}
                None if !out.contains(s) && !s.is_empty() => out.push(s.clone()),
                None => {}
            },
            Value::Array(list) => list.iter().for_each(|v| self.resolve(v, out, depth)),
            _ => {}
        }
    }

    /// A `trade_set`: `amount` different trades picked at random from it (fewer when not
    /// enough of them work out for this merchant).
    pub fn roll(&self, set: &str, merchant: &Merchant, loot: &LootTables, rng: &mut impl Rng) -> Vec<Offer> {
        let Some(v) = self.json("trade_set", set) else { return Vec::new() };
        let amount = super::loot::number(&v["amount"], rng).max(0.0) as usize;
        let mut candidates = Vec::new();
        self.resolve(&v["trades"], &mut candidates, 0);
        let mut out = Vec::new();
        while out.len() < amount && !candidates.is_empty() {
            let id = candidates.remove(rng.gen_range(0..candidates.len()));
            if let Some(offer) = self.trade(&id, merchant, loot, rng) {
                out.push(offer);
            }
        }
        out
    }

    /// One `villager_trade`, or `None` when it is not for this merchant or its item
    /// modifiers discarded the item (an explorer map that found nothing).
    pub fn trade(&self, id: &str, merchant: &Merchant, loot: &LootTables, rng: &mut impl Rng) -> Option<Offer> {
        let v = self.json("villager_trade", id)?;
        if let Some(p) = v.get("merchant_predicate") {
            if !merchant_matches(p, merchant) {
                return None;
            }
        }
        let mut wants = item_cost(&v["wants"])?;
        let additional = match v.get("additional_wants") {
            Some(w) => Some(item_cost(w)?),
            None => None,
        };
        let mut slot = Slot(Some(trade_stack(&v["gives"])?));
        if let Some(modifier) = v.get("given_item_modifier") {
            loot.apply(modifier, &LootContext::default(), rng, std::slice::from_mut(&mut slot));
        }
        let mut result = slot.0.filter(|s| s.count > 0)?;
        // Enchanted goods cost more (`additional_trade_cost`), treasure twice as much.
        let mut extra = super::loot::take_trade_cost(&mut result);
        if let Some(tag) = v["double_trade_price_enchantments"].as_str() {
            if has_enchantment_in(&result, tag) {
                extra *= 2;
            }
        }
        let limit = items::get(wants.item as u32).map_or(64, |i| i.max_stack_size as i32);
        wants.count = (wants.count + extra).clamp(1, limit.max(1));
        let max_uses = v["max_uses"].as_i64().unwrap_or(4) as i32;
        let xp = v["xp"].as_i64().unwrap_or(1) as i32;
        let multiplier = v["reputation_discount"].as_f64().unwrap_or(0.0) as f32;
        Some(Offer::new(wants, additional, result, max_uses, xp, multiplier))
    }

    /// The trade sets of a merchant at a level: a villager's profession level, or all of a
    /// wandering trader's.
    fn sets_for(name: &str, merchant: &Merchant, level: i32) -> Vec<String> {
        if name == WANDERING_TRADER {
            return ["buying", "common", "uncommon"].iter().map(|s| format!("minecraft:wandering_trader/{s}")).collect();
        }
        match merchant.profession.as_str() {
            "minecraft:none" | "minecraft:nitwit" => Vec::new(),
            p => vec![format!("minecraft:{}/level_{level}", path_of(p))],
        }
    }
}

// --- the game ------------------------------------------------------------------------------

/// A villager's or trader's state, if `id` is one.
fn merchant_of(world: &World, id: i32) -> Option<&Merchant> {
    world.entities.get(&id)?.mob()?.merchant.as_deref()
}

fn merchant_mut(world: &mut World, id: i32) -> Option<&mut Merchant> {
    world.entities.get_mut(&id)?.mob_mut()?.merchant.as_deref_mut()
}

fn center(pos: BlockPos) -> Vec3 {
    Vec3::new(pos.x as f64 + 0.5, pos.y as f64 + 0.5, pos.z as f64 + 0.5)
}

/// Picks the trade a payment pays for (`MerchantContainer.updateSellItem`): the selected
/// trade, or the first one it satisfies; either payment slot may hold the first cost.
pub fn sell_item(offers: &[Offer], a: &Slot, b: &Slot, selection: Option<usize>) -> (Option<usize>, Slot) {
    let (buy_a, buy_b) = if a.is_empty() { (b, &Slot::EMPTY) } else { (a, b) };
    if buy_a.is_empty() {
        return (None, Slot::EMPTY);
    }
    // `MerchantOffers.getRecipeFor` (a hint of 0 searches everything, as in vanilla).
    let find = |x: &Slot, y: &Slot| match selection {
        Some(i) if i > 0 && i < offers.len() => offers[i].satisfied_by(x, y).then_some(i),
        _ => offers.iter().position(|o| o.satisfied_by(x, y)),
    };
    let mut found = find(buy_a, buy_b);
    if found.is_none_or(|i| offers[i].out_of_stock()) {
        found = find(buy_b, buy_a);
    }
    match found {
        Some(i) if !offers[i].out_of_stock() => (Some(i), Slot(Some(offers[i].result.clone()))),
        _ => (None, Slot::EMPTY),
    }
}

impl Server {
    /// Sets up a new villager or wandering trader (`finalizeSpawn`): its type follows the
    /// biome it is born in.
    pub(super) fn init_merchant(&self, world: usize, name: &str, pos: Vec3, mob: &mut Mob, reason: SpawnReason) {
        if !is_merchant(name) {
            return;
        }
        let biome = self.biome_at(world, pos.x.floor() as i32, pos.y.floor() as i32, pos.z.floor() as i32);
        let mut m = Merchant::new(villager_type_for_biome(biome), "minecraft:none");
        if name == WANDERING_TRADER && reason == SpawnReason::Natural {
            m.despawn_delay = 48000;
        }
        mob.merchant = Some(Box::new(m));
    }

    /// `getOffers`: works the offers out the first time they are needed (each new trade goes
    /// through `VillagerAcquireTradeEvent`).
    pub fn merchant_offers(&mut self, world: usize, id: i32) -> Vec<Offer> {
        let Some(e) = self.worlds[world].entities.get(&id) else { return Vec::new() };
        let name = e.info.name.clone();
        let Some(m) = e.mob().and_then(|m| m.merchant.as_deref()) else { return Vec::new() };
        if let Some(offers) = &m.offers {
            return offers.clone();
        }
        let snapshot = m.clone();
        let mut rng = rand::thread_rng();
        let rolled: Vec<Offer> = Trades::sets_for(&name, &snapshot, snapshot.level).iter().flat_map(|set| self.trades.roll(set, &snapshot, &self.loot, &mut rng)).collect();
        // Set first, so plugins asking for the offers during the event do not roll again.
        if let Some(m) = merchant_mut(&mut self.worlds[world], id) {
            m.offers = Some(Vec::new());
        }
        let offers = crate::plugins::events::on_acquire_trades(self, world, id, rolled);
        if let Some(m) = merchant_mut(&mut self.worlds[world], id) {
            m.offers = Some(offers.clone());
        }
        offers
    }

    /// The offers of any merchant.
    pub fn offers_of(&mut self, merchant: MerchantRef) -> Vec<Offer> {
        match merchant {
            MerchantRef::Entity { world, id } => self.merchant_offers(world, id),
            MerchantRef::Custom(c) => self.plugin_data.merchants.get(&c).map(|m| m.offers.clone()).unwrap_or_default(),
        }
    }

    /// Replaces the offers of a merchant (`Merchant.setRecipes`), and shows them to whoever
    /// trades with it.
    pub fn set_offers(&mut self, merchant: MerchantRef, offers: Vec<Offer>) {
        match merchant {
            MerchantRef::Entity { world, id } => {
                if let Some(m) = self.worlds.get_mut(world).and_then(|w| merchant_mut(w, id)) {
                    m.offers = Some(offers);
                }
            }
            MerchantRef::Custom(c) => {
                if let Some(m) = self.plugin_data.merchants.get_mut(&c) {
                    m.offers = offers;
                }
            }
        }
        if let Some(player) = self.merchant_trader(merchant) {
            self.send_offers(player);
        }
    }

    /// The player trading with a merchant.
    pub fn merchant_trader(&self, merchant: MerchantRef) -> Option<i32> {
        match merchant {
            MerchantRef::Entity { world, id } => self.worlds.get(world).and_then(|w| merchant_of(w, id)).and_then(|m| m.trading_player),
            MerchantRef::Custom(c) => self.plugin_data.merchants.get(&c).and_then(|m| m.trader),
        }
    }

    fn set_merchant_trader(&mut self, merchant: MerchantRef, player: Option<i32>) {
        match merchant {
            MerchantRef::Entity { world, id } => {
                if let Some(m) = self.worlds.get_mut(world).and_then(|w| merchant_mut(w, id)) {
                    m.trading_player = player;
                }
            }
            MerchantRef::Custom(c) => {
                if let Some(m) = self.plugin_data.merchants.get_mut(&c) {
                    m.trader = player;
                }
            }
        }
    }

    /// A trading window's title: the merchant's name (a villager's is its profession) or the
    /// title a plugin gave its merchant.
    fn merchant_title(&self, merchant: MerchantRef) -> Option<Component> {
        match merchant {
            MerchantRef::Entity { world, id } => {
                let e = self.worlds.get(world)?.entities.get(&id)?;
                let m = e.mob()?.merchant.as_deref()?;
                Some(e.custom_name.clone().unwrap_or_else(|| {
                    let key = if e.info.name == VILLAGER { format!("entity.minecraft.villager.{}", path_of(&m.profession)) } else { "entity.minecraft.wandering_trader".to_owned() };
                    Component::translatable(&key, vec![])
                }))
            }
            MerchantRef::Custom(c) => self.plugin_data.merchants.get(&c).map(|m| m.title.clone()),
        }
    }

    /// Head shaking and a "no" (`setUnhappy`).
    pub fn set_unhappy(&mut self, world: usize, id: i32) {
        let Some(e) = self.worlds[world].entities.get_mut(&id) else { return };
        let pos = e.pos;
        let villager = e.info.name == VILLAGER;
        if let Some(m) = e.mob_mut().and_then(|m| m.merchant.as_deref_mut()) {
            m.unhappy = 40;
            e.metadata_dirty = true;
        }
        let sound = if villager { "minecraft:entity.villager.no" } else { "minecraft:entity.wandering_trader.no" };
        self.play_sound(world, pos, sound, category::NEUTRAL, 1.0, 1.0, None);
    }

    /// Right-click on a villager or wandering trader (`Villager.mobInteract`). Returns
    /// whether the click was used.
    pub(super) fn merchant_interact(&mut self, player: i32, world: usize, target: i32, hand: i32, held: Option<&ItemStack>) -> bool {
        let Some(e) = self.worlds[world].entities.get(&target) else { return false };
        let villager = e.info.name == VILLAGER;
        let Some(mob) = e.mob() else { return false };
        let Some(m) = mob.merchant.as_deref() else { return false };
        if held.is_some_and(|s| items::get(s.item as u32).is_some_and(|i| i.name == "minecraft:villager_spawn_egg")) || mob.living.is_dead() || m.trading_player.is_some() {
            return false;
        }
        if mob.baby {
            if villager {
                self.set_unhappy(world, target);
                return true;
            }
            return false;
        }
        if self.players.get(&player).is_none_or(|p| p.game_mode == GameMode::Spectator) {
            return false;
        }
        if hand == 0 {
            self.award_stat(player, super::stats::CUSTOM, "minecraft:talked_to_villager", 1);
        }
        let no_offers = self.merchant_offers(world, target).is_empty();
        if no_offers {
            if villager && hand == 0 {
                self.set_unhappy(world, target);
            }
            return true;
        }
        self.start_trading(player, world, target);
        true
    }

    /// `startTrading`: special prices for this player, then the trading window.
    pub fn start_trading(&mut self, player: i32, world: usize, id: i32) -> bool {
        let Some(p) = self.players.get(&player) else { return false };
        let uuid = p.uuid();
        let hero = paper_data::effects::by_name("minecraft:hero_of_the_village").and_then(|h| p.effects.iter().find(|e| e.id == h.id)).map(|e| e.amplifier);
        self.merchant_offers(world, id);
        let Some(e) = self.worlds[world].entities.get_mut(&id) else { return false };
        let villager = e.info.name == VILLAGER;
        let Some(m) = e.mob_mut().and_then(|m| m.merchant.as_deref_mut()) else { return false };
        // `updateSpecialPrices`: reputation and Hero of the Village lower prices.
        if villager {
            let reputation = m.reputation(uuid);
            for offer in m.offers.iter_mut().flatten().filter(|o| !o.ignore_discounts) {
                if reputation != 0 {
                    offer.special_price -= (reputation as f32 * offer.price_multiplier).floor() as i32;
                }
                if let Some(amplifier) = hero {
                    let discount = 0.3 + 0.0625 * amplifier as f64;
                    offer.special_price -= ((discount * offer.cost_a.count as f64).floor() as i32).max(1);
                }
            }
        }
        self.show_trading_window(player, MerchantRef::Entity { world, id })
    }

    /// `HumanEntity.openMerchant`: trading with any merchant, without special prices. A
    /// merchant trading with someone else stays with them unless `force` closes their window.
    pub fn open_merchant(&mut self, player: i32, merchant: MerchantRef, force: bool) -> bool {
        if let Some(other) = self.merchant_trader(merchant) {
            if !force {
                return false;
            }
            if self.players.get(&other).and_then(|p| p.open_menu.as_ref()).is_some_and(|m| m.kind == MenuKind::Merchant(merchant)) {
                self.close_menu(other, true);
            }
            self.stop_trading(merchant);
        }
        match merchant {
            MerchantRef::Entity { world, id } => {
                if self.worlds.get(world).and_then(|w| w.entities.get(&id)).is_none_or(|e| !e.is_alive()) {
                    return false;
                }
                self.merchant_offers(world, id);
            }
            MerchantRef::Custom(c) if !self.plugin_data.merchants.contains_key(&c) => return false,
            MerchantRef::Custom(_) => {}
        }
        self.show_trading_window(player, merchant)
    }

    /// Opens the trading window (`openTradingScreen`) and sends the offers.
    fn show_trading_window(&mut self, player: i32, merchant: MerchantRef) -> bool {
        let Some(title) = self.merchant_title(merchant) else { return false };
        self.set_merchant_trader(merchant, Some(player));
        if !self.open_menu(player, MenuKind::Merchant(merchant), "minecraft:merchant", title) {
            self.stop_trading(merchant);
            return false;
        }
        // Closing a window on the same merchant just now let it go: it trades with this player.
        self.set_merchant_trader(merchant, Some(player));
        self.send_offers(player);
        true
    }

    /// Sends the offers of the merchant a player trades with (`sendMerchantOffers`).
    pub fn send_offers(&mut self, player: i32) {
        let Some(menu) = self.players.get(&player).and_then(|p| p.open_menu.as_ref()) else { return };
        let MenuKind::Merchant(merchant) = menu.kind else { return };
        let window_id = menu.window_id;
        let offers = self.offers_of(merchant);
        let (villager, level, xp) = match merchant {
            MerchantRef::Entity { world, id } => {
                let Some(e) = self.worlds[world].entities.get(&id) else { return };
                let Some(m) = e.mob().and_then(|m| m.merchant.as_deref()) else { return };
                let villager = e.info.name == VILLAGER;
                (villager, if villager { m.level } else { 1 }, m.xp)
            }
            // A plugin's merchant shows no level or experience (`MinecraftMerchant`).
            MerchantRef::Custom(_) => (false, 1, 0),
        };
        let packet = play::MerchantOffers { window_id, offers: offers.iter().map(Offer::to_packet).collect(), level, xp, show_progress: villager, can_restock: villager };
        if let Some(p) = self.players.get(&player) {
            p.conn.send(&packet);
        }
    }

    /// The trading window closed (`stopTrading`): prices go back to normal.
    pub(super) fn stop_trading(&mut self, merchant: MerchantRef) {
        match merchant {
            MerchantRef::Entity { world, id } => {
                let Some(m) = self.worlds.get_mut(world).and_then(|w| merchant_mut(w, id)) else { return };
                m.trading_player = None;
                for offer in m.offers.iter_mut().flatten() {
                    offer.special_price = 0;
                }
            }
            MerchantRef::Custom(c) => {
                if let Some(m) = self.plugin_data.merchants.get_mut(&c) {
                    m.trader = None;
                }
                crate::plugins::merchant::collect_released(self);
            }
        }
    }

    /// The merchant whose window a player has open, its offers and the selected trade.
    fn open_offers(&mut self, player: i32) -> Option<(MerchantRef, Vec<Offer>, Option<usize>)> {
        let menu = self.players.get(&player)?.open_menu.as_ref()?;
        let MenuKind::Merchant(merchant) = menu.kind else { return None };
        let selection = menu.trade_selection;
        Some((merchant, self.offers_of(merchant), selection))
    }

    /// Works out the result of the payment in a trading window and shows it.
    pub(super) fn update_trade_result(&mut self, player: i32) {
        let Some((_, offers, selection)) = self.open_offers(player) else { return };
        let Some(p) = self.players.get_mut(&player) else { return };
        let Some(menu) = p.open_menu.as_mut() else { return };
        let (active, result) = sell_item(&offers, &menu.craft_grid[0], &menu.craft_grid[1], selection);
        menu.craft_grid[2] = result.clone();
        menu.active_offer = active;
        let (window_id, state_id) = (menu.window_id, menu.state_id);
        p.conn.send(&play::ContainerSetSlot { window_id, state_id, slot: 2, item: result });
    }

    /// Pays for the trade on offer from the payment `slots` (`MerchantResultSlot.onTake`) and
    /// works out the next result. Plugins see it first (`PlayerTradeEvent`, or
    /// `PlayerPurchaseEvent` for their own merchants) and may cancel or change it. Returns
    /// false if no trade happened.
    fn take_trade(&mut self, player: i32, slots: &mut [Slot]) -> bool {
        let Some((merchant, offers, selection)) = self.open_offers(player) else { return false };
        let Some(index) = self.players.get(&player).and_then(|p| p.open_menu.as_ref()).and_then(|m| m.active_offer) else { return false };
        let Some(offer) = offers.get(index).cloned() else { return false };
        if !(offer.satisfied_by(&slots[0], &slots[1]) || offer.satisfied_by(&slots[1], &slots[0])) {
            return false;
        }
        let Some((offer, reward_exp, increase_uses)) = crate::plugins::events::on_trade(self, player, merchant, index, offer) else {
            // Cancelled: the client took the result already; it gets the real state back.
            self.send_offers(player);
            return false;
        };
        let (mut a, mut b) = (slots[0].clone(), slots[1].clone());
        if !(offer.take(&mut a, &mut b) || offer.take(&mut b, &mut a)) {
            return false;
        }
        slots[0] = a;
        slots[1] = b;
        self.notify_trade(merchant, index, player, reward_exp, increase_uses);
        self.award_stat(player, super::stats::CUSTOM, "minecraft:traded_with_villager", 1);
        let offers = self.offers_of(merchant);
        let (active, result) = sell_item(&offers, &slots[0], &slots[1], selection);
        slots[2] = result;
        if let Some(menu) = self.players.get_mut(&player).and_then(|p| p.open_menu.as_mut()) {
            menu.active_offer = active;
        }
        true
    }

    /// A click on a trading window's result slot: pick up, shift-click (as many trades as
    /// fit), a number key into an empty hotbar slot, or drop.
    pub(super) fn trade_result_click(&mut self, player: i32, layout: &Layout, slots: &mut Vec<Slot>, carried: &mut Slot, mode: i32, button: i8) -> ClickResult {
        let mut result = ClickResult::default();
        let Some(first) = slots[2].0.clone() else { return result };
        match mode {
            0 => {
                let fits = match &carried.0 {
                    None => true,
                    Some(c) => same_item(c, &first) && c.count + first.count <= max_stack(c),
                };
                if fits && self.take_trade(player, slots) {
                    match &mut carried.0 {
                        Some(c) => c.count += first.count,
                        None => *carried = Slot(Some(first)),
                    }
                }
            }
            1 => {
                for _ in 0..64 {
                    let Some(out) = slots[2].0.clone() else { break };
                    if !same_item(&out, &first) {
                        break;
                    }
                    let mut trial = slots.clone();
                    let mut moving = Slot(Some(out));
                    let (mut no_carry, mut drag) = (Slot::EMPTY, super::menu::Drag::default());
                    super::menu::Menu { layout, slots: &mut trial, carried: &mut no_carry, recipes: None, creative: false, drag: &mut drag }.move_into(&mut moving, 3..39, true);
                    if !moving.is_empty() || !self.take_trade(player, slots) {
                        break;
                    }
                    slots[3..39].clone_from_slice(&trial[3..39]);
                }
            }
            2 => {
                let target = 30 + button as usize;
                if (0..9).contains(&button) && slots[target].is_empty() && self.take_trade(player, slots) {
                    slots[target] = Slot(Some(first));
                }
            }
            4 => {
                if self.take_trade(player, slots) {
                    result.dropped.push(Slot(Some(first)));
                }
            }
            _ => {}
        }
        result
    }

    /// The client picked a trade (`ServerboundSelectTradePacket`, `TradeSelectEvent`): the
    /// payment goes back to the inventory and what the trade wants comes out of it
    /// (`MerchantMenu.tryMoveItems`).
    pub(super) fn handle_select_trade(&mut self, player: i32, index: i32) {
        let Some((_, offers, _)) = self.open_offers(player) else { return };
        let Some(offer) = usize::try_from(index).ok().and_then(|i| offers.get(i)).cloned() else { return };
        if !crate::plugins::events::on_trade_select(self, player, index) {
            self.send_menu_contents(player);
            return;
        }
        if let Some(menu) = self.players.get_mut(&player).and_then(|p| p.open_menu.as_mut()) {
            menu.trade_selection = Some(index as usize);
        }
        let Some((kind, mut slots)) = self.menu_slots(player) else { return };
        let layout = Layout::merchant();
        let (mut no_carry, mut drag) = (Slot::EMPTY, super::menu::Drag::default());
        let mut engine = super::menu::Menu { layout: &layout, slots: &mut slots, carried: &mut no_carry, recipes: None, creative: false, drag: &mut drag };
        let mut cleared = true;
        for i in 0..2 {
            let mut item = std::mem::take(&mut engine.slots[i]);
            if !item.is_empty() {
                engine.move_into(&mut item, 3..39, true);
                if !item.is_empty() {
                    engine.slots[i] = item;
                    cleared = false;
                    break;
                }
            }
        }
        if cleared {
            fill_payment(&mut slots, 0, &offer.cost_a);
            if let Some(b) = &offer.cost_b {
                fill_payment(&mut slots, 1, b);
            }
        }
        self.store_menu_slots(player, kind.as_ref(), &slots);
        self.update_trade_result(player);
        self.send_menu_contents(player);
    }

    /// What the merchant does when a trade happens (`processTrade`, `rewardTradeXp`): the
    /// offer is used once, villagers gain experience (and level up soon after the player
    /// leaves), and experience orbs pop out. Plugins may have turned the use or the reward
    /// off.
    fn notify_trade(&mut self, merchant: MerchantRef, index: usize, player: i32, reward_exp: bool, increase_uses: bool) {
        let (world, id) = match merchant {
            MerchantRef::Entity { world, id } => (world, id),
            MerchantRef::Custom(c) => {
                if let Some(offer) = self.plugin_data.merchants.get_mut(&c).and_then(|m| m.offers.get_mut(index)) {
                    if increase_uses {
                        offer.uses += 1;
                    }
                }
                return;
            }
        };
        let uuid = self.players.get(&player).map(|p| p.uuid());
        let Some(e) = self.worlds[world].entities.get_mut(&id) else { return };
        let pos = e.pos;
        let villager = e.info.name == VILLAGER;
        let Some(m) = e.mob_mut().and_then(|m| m.merchant.as_deref_mut()) else { return };
        let Some(offer) = m.offers.as_mut().and_then(|o| o.get_mut(index)) else { return };
        if increase_uses {
            offer.uses += 1;
        }
        let (xp, orbs_on) = (offer.xp, offer.reward_xp);
        let mut orbs = None;
        if reward_exp {
            let mut amount = 3 + rand::thread_rng().gen_range(0..4);
            if villager {
                m.xp += xp;
                m.last_traded = uuid;
                if m.should_level_up() {
                    m.level_up_timer = 40;
                    m.level_up_pending = true;
                    amount += 5;
                }
            }
            if orbs_on {
                orbs = Some(amount);
            }
        }
        if let Some(amount) = orbs {
            self.award_experience(world, Vec3::new(pos.x, pos.y + 0.5, pos.z), amount);
        }
        let sound = if villager { "minecraft:entity.villager.yes" } else { "minecraft:entity.wandering_trader.yes" };
        self.play_sound(world, pos, sound, category::NEUTRAL, 1.0, 1.0, None);
    }

    /// `increaseMerchantCareer`: the next level, with new trades.
    pub fn level_up_villager(&mut self, world: usize, id: i32) {
        let Some(m) = self.worlds[world].entities.get(&id).and_then(|e| e.mob()).and_then(|m| m.merchant.as_deref()) else { return };
        if m.level >= 5 {
            return;
        }
        let mut next = m.clone();
        next.level += 1;
        let mut rng = rand::thread_rng();
        let rolled: Vec<Offer> = Trades::sets_for(VILLAGER, &next, next.level).iter().flat_map(|set| self.trades.roll(set, &next, &self.loot, &mut rng)).collect();
        if let Some(e) = self.worlds[world].entities.get_mut(&id) {
            if let Some(m) = e.mob_mut().and_then(|m| m.merchant.as_deref_mut()) {
                m.level = next.level;
                e.metadata_dirty = true;
            }
        }
        let added = crate::plugins::events::on_acquire_trades(self, world, id, rolled);
        if let Some(m) = merchant_mut(&mut self.worlds[world], id) {
            m.offers.get_or_insert_with(Vec::new).extend(added);
        }
    }

    /// `updateTrades(amount)` (Paper's `Villager.addTrades`): more trades of the current level.
    pub fn add_trades(&mut self, world: usize, id: i32, amount: usize) -> bool {
        self.merchant_offers(world, id);
        let Some(e) = self.worlds[world].entities.get(&id) else { return false };
        let name = e.info.name.clone();
        let Some(m) = e.mob().and_then(|m| m.merchant.as_deref()) else { return false };
        let snapshot = m.clone();
        let mut rng = rand::thread_rng();
        let mut rolled = Vec::new();
        for set in Trades::sets_for(&name, &snapshot, snapshot.level) {
            for offer in self.trades.roll(&set, &snapshot, &self.loot, &mut rng) {
                if rolled.len() < amount {
                    rolled.push(offer);
                }
            }
        }
        if rolled.is_empty() {
            return false;
        }
        let added = crate::plugins::events::on_acquire_trades(self, world, id, rolled);
        if let Some(m) = merchant_mut(&mut self.worlds[world], id) {
            m.offers.get_or_insert_with(Vec::new).extend(added);
        }
        true
    }

    /// Changes a villager's profession (`setVillagerData`): its trades start over.
    pub fn set_profession(&mut self, world: usize, id: i32, profession: &str) {
        let Some(e) = self.worlds[world].entities.get_mut(&id) else { return };
        if let Some(m) = e.mob_mut().and_then(|m| m.merchant.as_deref_mut()) {
            if m.profession != profession {
                m.profession = profession.to_owned();
                m.offers = None;
                e.metadata_dirty = true;
            }
        }
    }

    /// `updateDemand`: prices follow how much each trade was used.
    pub fn update_demand(&mut self, world: usize, id: i32) {
        if let Some(m) = merchant_mut(&mut self.worlds[world], id) {
            for offer in m.offers.iter_mut().flatten() {
                offer.update_demand();
            }
        }
    }

    /// `restock`: trades are available again (unless a plugin keeps one used up with
    /// `VillagerReplenishTradeEvent`), with prices following demand.
    pub fn restock(&mut self, world: usize, id: i32) {
        self.update_demand(world, id);
        let offers = self.merchant_offers(world, id);
        let replenished = crate::plugins::events::on_replenish(self, world, id, offers);
        let age = self.worlds[world].age;
        let Some(m) = merchant_mut(&mut self.worlds[world], id) else { return };
        if let Some(current) = m.offers.as_mut() {
            for (i, (offer, reset)) in replenished.into_iter().enumerate() {
                if let Some(slot) = current.get_mut(i) {
                    *slot = offer;
                    if reset {
                        slot.uses = 0;
                    }
                }
            }
        }
        m.last_restock = age;
        m.restocks_today += 1;
        if let Some(player) = m.trading_player {
            self.send_offers(player);
        }
    }

    /// `shouldRestock`: up to twice a day, when some trade was used.
    fn should_restock(&mut self, world: usize, id: i32) -> bool {
        let (age, day_time) = (self.worlds[world].age, self.worlds[world].day_time);
        let Some(m) = merchant_mut(&mut self.worlds[world], id) else { return false };
        let mut new_period = age > m.last_restock + 12000;
        if m.last_restock_check_day > 0 {
            new_period |= day_time / 24000 > m.last_restock_check_day / 24000;
        }
        m.last_restock_check_day = day_time;
        if new_period {
            m.last_restock = age;
            m.restocks_today = 0;
        }
        let allowed = m.restocks_today == 0 || m.restocks_today < 2 && age > m.last_restock + 2400;
        allowed && m.offers.iter().flatten().any(|o| o.uses > 0)
    }

    /// The nearest workstation within 48 blocks that a villager of `profession` can take and
    /// nobody has claimed.
    fn find_job_site(&self, world: usize, from: Vec3, profession: &str, claimed: &HashSet<BlockPos>) -> Option<BlockPos> {
        let w = &self.worlds[world];
        let states = job_site_states();
        let (x, y, z) = (from.x.floor() as i32, from.y.floor() as i32, from.z.floor() as i32);
        const R: i32 = 48;
        let mut best: Option<(f64, BlockPos)> = None;
        for cx in (x - R) >> 4..=(x + R) >> 4 {
            for cz in (z - R) >> 4..=(z + R) >> 4 {
                let Some(chunk) = w.chunks.get(&ChunkPos::new(cx, cz)) else { continue };
                for (si, section) in chunk.sections.iter().enumerate() {
                    let base = chunk.min_y + si as i32 * 16;
                    if base + 15 < y - 16 || base > y + 16 || !section.contains_any(states) {
                        continue;
                    }
                    for (ly, lz, lx) in itertools_triples() {
                        let state = section.get(lx, ly, lz);
                        if !states.contains(&(state as u16)) || !job_site_for(profession, blocks::block_of_state(state).name) {
                            continue;
                        }
                        let pos = BlockPos::new(cx * 16 + lx as i32, base + ly as i32, cz * 16 + lz as i32);
                        let d = center(pos).distance_squared(from);
                        if d <= (R * R) as f64 && !claimed.contains(&pos) && best.is_none_or(|(bd, _)| d < bd) {
                            best = Some((d, pos));
                        }
                    }
                }
            }
        }
        best.map(|(_, p)| p)
    }

    /// A place to stand next to a workstation.
    fn spot_next_to(&self, world: usize, pos: BlockPos) -> Option<Vec3> {
        let w = &self.worlds[world];
        for (dx, dz) in [(1, 0), (-1, 0), (0, 1), (0, -1), (1, 1), (-1, -1), (1, -1), (-1, 1)] {
            for dy in [0, -1, 1] {
                let (x, y, z) = (pos.x + dx, pos.y + dy, pos.z + dz);
                if super::mobs::walkable(w, x, y, z, 2) {
                    return Some(Vec3::new(x as f64 + 0.5, y as f64, z as f64 + 0.5));
                }
            }
        }
        None
    }

    /// Every tick: villagers and wandering traders, then the wandering trader spawner.
    pub(super) fn tick_merchants(&mut self) {
        for world in 0..self.worlds.len() {
            let ids: Vec<i32> = self.worlds[world].entities.iter().filter(|(_, e)| e.is_alive() && e.mob().is_some_and(|m| m.merchant.is_some())).map(|(id, _)| *id).collect();
            for id in ids {
                self.tick_merchant(world, id);
            }
        }
        self.tick_trader_spawner();
    }

    fn tick_merchant(&mut self, world: usize, id: i32) {
        let (age, day_time) = (self.worlds[world].age, self.worlds[world].day_time);
        let Some(e) = self.worlds[world].entities.get(&id) else { return };
        let (pos, villager) = (e.pos, e.info.name == VILLAGER);
        let Some(mob) = e.mob() else { return };
        let baby = mob.baby;
        let Some(m) = mob.merchant.as_deref() else { return };

        // The trading player must stay close (`stillValid`).
        if let Some(player) = m.trading_player {
            let ok = self.players.get(&player).is_some_and(|p| {
                !p.dead && p.world == world && p.open_menu.as_ref().is_some_and(|o| o.kind == MenuKind::Merchant(MerchantRef::Entity { world, id })) && Vec3::new(p.loc.x, p.loc.y, p.loc.z).distance_squared(pos) <= 64.0
            });
            if !ok {
                if self.players.get(&player).and_then(|p| p.open_menu.as_ref()).is_some_and(|o| o.kind == MenuKind::Merchant(MerchantRef::Entity { world, id })) {
                    self.close_menu(player, true);
                }
                self.stop_trading(MerchantRef::Entity { world, id });
            }
        }
        let Some(e) = self.worlds[world].entities.get_mut(&id) else { return };
        let Some(m) = e.mob_mut().and_then(|m| m.merchant.as_deref_mut()) else { return };
        if m.unhappy > 0 {
            m.unhappy -= 1;
            if m.unhappy == 0 {
                e.metadata_dirty = true;
            }
        }
        let Some(m) = e.mob_mut().and_then(|m| m.merchant.as_deref_mut()) else { return };
        // After a trade: gossip about the trader and happy particles.
        let traded = m.last_traded.take();
        if let Some(uuid) = traded {
            m.add_gossip(uuid, GossipType::Trading, 2);
        }
        let mut level_up = false;
        let mut regenerate = false;
        if m.trading_player.is_none() && m.level_up_timer > 0 {
            m.level_up_timer -= 1;
            if m.level_up_timer <= 0 {
                level_up = std::mem::take(&mut m.level_up_pending);
                regenerate = true;
            }
        }
        if !villager {
            // Wandering traders leave after a while (`maybeDespawn`).
            if m.despawn_delay > 0 && m.trading_player.is_none() {
                m.despawn_delay -= 1;
                if m.despawn_delay == 0 {
                    e.removed = true;
                }
            }
            if traded.is_some() {
                self.entity_event(world, id, 14);
            }
            return;
        }
        if m.last_gossip_decay == 0 {
            m.last_gossip_decay = age;
        } else if age >= m.last_gossip_decay + 24000 {
            m.decay_gossip();
            m.last_gossip_decay = age;
        }
        if traded.is_some() {
            self.entity_event(world, id, 14);
        }
        if level_up {
            self.level_up_villager(world, id);
        }
        if regenerate {
            if let Some(effect) = ActiveEffect::new("minecraft:regeneration", 200, 0) {
                self.add_effect(world, id, effect);
            }
        }
        if baby {
            return;
        }
        self.tick_job_site(world, id, pos, age, day_time);
    }

    /// Workstations: losing a broken one (and the profession, if never traded), finding and
    /// claiming a free one, and working there in the day (restocking).
    fn tick_job_site(&mut self, world: usize, id: i32, pos: Vec3, age: i64, day_time: i64) {
        let Some(m) = merchant_of(&self.worlds[world], id) else { return };
        let (profession, job_site, potential) = (m.profession.clone(), m.job_site, m.potential_job_site);
        let w = &self.worlds[world];
        let gone = |site: Option<BlockPos>| site.is_some_and(|p| w.block_at(p.x, p.y, p.z).is_some_and(|s| !job_site_for(&profession, blocks::block_of_state(s).name)));
        let (lost, lost_potential) = (gone(job_site), gone(potential));
        if lost || lost_potential {
            let Some(e) = self.worlds[world].entities.get_mut(&id) else { return };
            let Some(m) = e.mob_mut().and_then(|m| m.merchant.as_deref_mut()) else { return };
            if lost {
                m.job_site = None;
            }
            if lost_potential {
                m.potential_job_site = None;
            }
            // `ResetProfession`: a villager nobody traded with forgets its job.
            let reset = lost && !matches!(m.profession.as_str(), "minecraft:none" | "minecraft:nitwit") && m.xp == 0 && m.level <= 1;
            if let Some(brain) = self.worlds[world].entities.get_mut(&id).and_then(|e| e.mob_mut()).map(|m| &mut m.brain) {
                brain.walk_target = None;
            }
            if reset {
                if let Some(p) = crate::plugins::events::on_career_change(self, world, id, "minecraft:none", "LOSING_JOB") {
                    self.set_profession(world, id, &p);
                }
            }
            return;
        }
        let Some(m) = merchant_of(&self.worlds[world], id) else { return };
        if m.profession == "minecraft:nitwit" {
            return;
        }
        // Walking to a free workstation; claim it when there (`AssignProfessionFromJobSite`).
        if let Some(site) = m.potential_job_site {
            let claimed_by_other = self.claimed_job_sites(world, Some(id)).contains(&site);
            if claimed_by_other || age - m.potential_since > 600 {
                if let Some(m) = merchant_mut(&mut self.worlds[world], id) {
                    m.potential_job_site = None;
                    m.next_job_search = age + 200;
                }
                return;
            }
            if center(site).distance_squared(pos) < 4.0 {
                let block = self.worlds[world].block_at(site.x, site.y, site.z).map(|s| blocks::block_of_state(s).name);
                let new_profession = block.and_then(profession_of_job_site);
                let Some(e) = self.worlds[world].entities.get_mut(&id) else { return };
                let Some(mob) = e.mob_mut() else { return };
                mob.brain.walk_target = None;
                let Some(m) = mob.merchant.as_deref_mut() else { return };
                m.job_site = Some(site);
                m.potential_job_site = None;
                let unemployed = m.profession == "minecraft:none";
                if let (true, Some(p)) = (unemployed, new_profession) {
                    // `VillagerCareerChangeEvent` (EMPLOYED): plugins may cancel or change it.
                    if let Some(p) = crate::plugins::events::on_career_change(self, world, id, p, "EMPLOYED") {
                        self.set_profession(world, id, &p);
                    }
                }
                self.entity_event(world, id, 14);
            } else if let Some(spot) = self.spot_next_to(world, site) {
                if let Some(mob) = self.worlds[world].entities.get_mut(&id).and_then(|e| e.mob_mut()) {
                    mob.brain.walk_target = Some(spot);
                }
            }
            return;
        }
        // Without a workstation: look for one now and then (`AcquirePoi`).
        if m.job_site.is_none() {
            if age >= m.next_job_search && (age + id as i64) % 40 == 0 {
                let claimed = self.claimed_job_sites(world, Some(id));
                let found = self.find_job_site(world, pos, &profession, &claimed);
                if let Some(m) = merchant_mut(&mut self.worlds[world], id) {
                    m.next_job_search = age + 200;
                    if found.is_some() {
                        m.potential_job_site = found;
                        m.potential_since = age;
                    }
                }
            }
            return;
        }
        // Work hours: go to the workstation and use it (`WorkAtPoi`), restocking there.
        let site = m.job_site.unwrap();
        let time = day_time.rem_euclid(24000);
        if !(2000..9000).contains(&time) || m.trading_player.is_some() {
            return;
        }
        let last_worked = m.last_worked;
        if center(site).distance_squared(pos) > 4.0 {
            if let Some(spot) = self.spot_next_to(world, site) {
                if let Some(mob) = self.worlds[world].entities.get_mut(&id).and_then(|e| e.mob_mut()) {
                    if mob.brain.walk_target.is_none() {
                        mob.brain.walk_target = Some(spot);
                    }
                }
            }
            return;
        }
        if age - last_worked < 300 {
            return;
        }
        if let Some(mob) = self.worlds[world].entities.get_mut(&id).and_then(|e| e.mob_mut()) {
            mob.brain.walk_target = None;
            mob.brain.look_at = Some(center(site));
            if let Some(m) = mob.merchant.as_deref_mut() {
                m.last_worked = age;
            }
        }
        let sound = format!("minecraft:entity.villager.work_{}", path_of(&profession));
        self.play_sound(world, pos, &sound, category::NEUTRAL, 1.0, 1.0, None);
        if self.should_restock(world, id) {
            self.restock(world, id);
        }
    }

    /// Workstations claimed (or being walked to) by villagers other than `except`.
    fn claimed_job_sites(&self, world: usize, except: Option<i32>) -> HashSet<BlockPos> {
        self.worlds[world]
            .entities
            .iter()
            .filter(|(id, _)| Some(**id) != except)
            .filter_map(|(_, e)| e.mob()?.merchant.as_deref())
            .flat_map(|m| m.job_site.into_iter().chain(m.potential_job_site))
            .collect()
    }

    /// A player hurt a merchant (`VILLAGER_HURT`): it will remember, and shows it.
    pub(super) fn merchant_hurt(&mut self, world: usize, id: i32, attacker: i32) {
        let Some(uuid) = self.players.get(&attacker).map(|p| p.uuid()) else { return };
        let Some(m) = self.worlds.get_mut(world).and_then(|w| merchant_mut(w, id)) else { return };
        m.add_gossip(uuid, GossipType::MinorNegative, 25);
        self.entity_event(world, id, 13);
    }

    /// A player killed a villager (`VILLAGER_KILLED`): villagers that saw it remember.
    pub(super) fn merchant_killed(&mut self, world: usize, id: i32, killer: Option<i32>) {
        let Some(e) = self.worlds[world].entities.get(&id) else { return };
        if e.mob().and_then(|m| m.merchant.as_deref()).is_none() {
            return;
        }
        let pos = e.pos;
        if let Some(m) = e.mob().and_then(|m| m.merchant.as_deref()) {
            if let Some(player) = m.trading_player {
                self.close_menu(player, true);
            }
        }
        let Some(uuid) = killer.and_then(|k| self.players.get(&k)).map(|p| p.uuid()) else { return };
        let witnesses: Vec<i32> = self.worlds[world].entities.iter().filter(|(o, oe)| **o != id && oe.info.name == VILLAGER && oe.pos.distance_squared(pos) < 16.0 * 16.0).map(|(o, _)| *o).collect();
        for w in witnesses {
            if let Some(m) = merchant_mut(&mut self.worlds[world], w) {
                m.add_gossip(uuid, GossipType::MajorNegative, 25);
            }
        }
    }

    /// `WanderingTraderSpawner`: once a day a chance (25%, then 50%, then 75%) of a trader
    /// turning up near a player in the overworld.
    fn tick_trader_spawner(&mut self) {
        let Some(world) = self.worlds.iter().position(|w| w.dimension == "minecraft:overworld") else { return };
        let w = &mut self.worlds[world];
        if !w.rules.bool("spawn_wandering_traders") {
            return;
        }
        w.trader_tick_delay -= 1;
        if w.trader_tick_delay > 0 {
            return;
        }
        w.trader_tick_delay = 1200;
        w.trader_spawn_delay -= 1200;
        if w.trader_spawn_delay > 0 {
            return;
        }
        w.trader_spawn_delay = 24000;
        if !w.rules.bool("spawn_mobs") {
            return;
        }
        let chance = w.trader_spawn_chance;
        w.trader_spawn_chance = (chance + 25).clamp(25, 75);
        if rand::thread_rng().gen_range(0..100) > chance {
            return;
        }
        if self.spawn_wandering_trader(world).is_some() {
            self.worlds[world].trader_spawn_chance = 25;
        }
    }

    /// Spawns a wandering trader near a random player (one try in ten succeeds, as in
    /// vanilla). Returns its id.
    pub fn spawn_wandering_trader(&mut self, world: usize) -> Option<i32> {
        let mut rng = rand::thread_rng();
        let players: Vec<Vec3> = self.players.values().filter(|p| p.world == world && !p.dead && p.game_mode != GameMode::Spectator).map(|p| Vec3::new(p.loc.x, p.loc.y, p.loc.z)).collect();
        if players.is_empty() || rng.gen_range(0..10) != 0 {
            return None;
        }
        let at = players[rng.gen_range(0..players.len())];
        self.wandering_trader_near(world, at, &mut rng)
    }

    /// Places a wandering trader on open ground within 48 blocks of `at`.
    pub fn wandering_trader_near(&mut self, world: usize, at: Vec3, rng: &mut impl Rng) -> Option<i32> {
        let excluded = paper_data::synced::tag("minecraft:worldgen/biome", "minecraft:without_wandering_trader_spawns").unwrap_or(&[]);
        for _ in 0..10 {
            let x = at.x.floor() as i32 + rng.gen_range(-48..=48);
            let z = at.z.floor() as i32 + rng.gen_range(-48..=48);
            let w = &self.worlds[world];
            let Some(chunk) = w.chunks.get(&ChunkPos::of_block(x, z)) else { continue };
            let y = chunk.min_y + chunk.world_surface[((z & 15) * 16 + (x & 15)) as usize] as i32;
            if !super::mobs::walkable(w, x, y, z, 2) || !(-1..=1).all(|dx| (-1..=1).all(|dz| (0..3).all(|dy| w.block_at(x + dx, y + dy, z + dz).is_some_and(|s| !blocks::has_collision(s))))) {
                continue;
            }
            let biome = self.biome_at(world, x, y, z);
            if paper_data::synced::id("minecraft:worldgen/biome", biome).is_some_and(|b| excluded.contains(&b)) {
                return None;
            }
            return self.spawn_mob(world, WANDERING_TRADER, Vec3::new(x as f64 + 0.5, y as f64, z as f64 + 0.5), SpawnReason::Natural);
        }
        None
    }
}

/// Every `(y, z, x)` of a chunk section.
fn itertools_triples() -> impl Iterator<Item = (usize, usize, usize)> {
    (0..16).flat_map(|y| (0..16).flat_map(move |z| (0..16).map(move |x| (y, z, x))))
}

/// `moveFromInventoryToPaymentSlot`: matching items from the inventory part of a trading
/// window's slots (3..39) fill a payment slot up to a stack.
fn fill_payment(slots: &mut [Slot], payment: usize, cost: &ItemCost) {
    for i in 3..39 {
        let Some(inv) = slots[i].0.clone() else { continue };
        if !cost.matches(&inv) {
            continue;
        }
        let paid = slots[payment].0.clone();
        if paid.as_ref().is_some_and(|p| !same_item(p, &inv)) {
            continue;
        }
        let max = max_stack(&inv);
        let have = paid.map_or(0, |p| p.count);
        let take = (max - have).min(inv.count);
        if take <= 0 {
            break;
        }
        slots[payment] = Slot(Some(inv.with_count(have + take)));
        slots[i] = if inv.count > take { Slot(Some(inv.with_count(inv.count - take))) } else { Slot::EMPTY };
        if have + take >= max {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn trades() -> Option<(Trades, LootTables)> {
        let jar = crate::vanilla::test_jar()?;
        let cache = std::env::temp_dir().join(format!("paper-rs-trades-{}", std::process::id()));
        let data = VanillaData::open(&jar, &cache).ok()?;
        Some((Trades::new(Some(data.clone())), LootTables::new(Some(data))))
    }

    #[test]
    fn professions_and_types() {
        assert_eq!(profession_of_job_site("minecraft:lectern"), Some("minecraft:librarian"));
        assert_eq!(profession_of_job_site("minecraft:water_cauldron"), Some("minecraft:leatherworker"));
        assert!(job_site_for("minecraft:none", "minecraft:composter"));
        assert!(!job_site_for("minecraft:nitwit", "minecraft:composter"));
        assert!(!job_site_for("minecraft:farmer", "minecraft:lectern"));
        assert_eq!(villager_type_for_biome("minecraft:snowy_taiga"), "minecraft:snow");
        assert_eq!(villager_type_for_biome("minecraft:forest"), "minecraft:plains");
        assert!(job_site_states().len() > 20);
    }

    #[test]
    fn trade_sets_come_from_the_jar() {
        let Some((trades, loot)) = trades() else { return };
        let mut rng = rand::thread_rng();
        let farmer = Merchant::new("minecraft:plains", "minecraft:farmer");
        let offers = trades.roll("minecraft:farmer/level_1", &farmer, &loot, &mut rng);
        assert_eq!(offers.len(), 2, "farmers start with two trades");
        let emerald = items::id("minecraft:emerald").unwrap() as i32;
        assert!(offers.iter().all(|o| o.cost_a.item == emerald || o.result.item == emerald));
        assert!(offers.iter().all(|o| o.max_uses == 16 && o.price_multiplier == 0.05));

        // Librarians sell enchanted books priced by their enchantment, with a book.
        let librarian = Merchant::new("minecraft:plains", "minecraft:librarian");
        let book = trades.trade("minecraft:librarian/1/emerald_and_book_enchanted_book", &librarian, &loot, &mut rng).expect("an enchanted book trade");
        assert_eq!(items::get(book.result.item as u32).unwrap().name, "minecraft:enchanted_book");
        assert!(!super::super::enchanting::read_enchantments(&book.result, "minecraft:stored_enchantments").is_empty());
        assert!((2..=64).contains(&book.cost_a.count), "{}", book.cost_a.count);
        assert_eq!(book.cost_b.as_ref().map(|b| items::get(b.item as u32).unwrap().name), Some("minecraft:book"));
        let cost = paper_data::registries::id("minecraft:data_component_type", "minecraft:additional_trade_cost").unwrap() as i32;
        assert!(book.result.added.iter().all(|(t, _)| *t != cost), "the price component is taken off");

        // Armorers enchant diamond armour; the levels add to the price.
        let armorer = Merchant::new("minecraft:plains", "minecraft:armorer");
        let chestplate = trades.trade("minecraft:armorer/5/emerald_enchanted_diamond_chestplate", &armorer, &loot, &mut rng).expect("an enchanted chestplate");
        assert!((21..=35).contains(&chestplate.cost_a.count), "{}", chestplate.cost_a.count);
        assert!(!super::super::enchanting::read_enchantments(&chestplate.result, "minecraft:enchantments").is_empty());
        // Smiths share trades through a tag inside their own.
        let mut ids = Vec::new();
        trades.resolve(&Value::String("#minecraft:armorer/level_1".into()), &mut ids, 0);
        assert!(ids.iter().any(|i| i == "minecraft:smith/1/coal_emerald"), "{ids:?}");

        // Some trades are only for some villager types.
        let snowy = Merchant::new("minecraft:snow", "minecraft:cartographer");
        assert!(trades.trade("minecraft:cartographer/2/emerald_and_compass_village_desert_map", &snowy, &loot, &mut rng).is_none());

        // Wandering traders buy water bottles (a component on the cost).
        let trader = Merchant::new("minecraft:plains", "minecraft:none");
        let water = trades.trade("minecraft:wandering_trader/water_bottle_emerald", &trader, &loot, &mut rng).unwrap();
        assert_eq!(water.cost_a.components.len(), 1);
        let all: Vec<Offer> = Trades::sets_for(WANDERING_TRADER, &trader, 1).iter().flat_map(|s| trades.roll(s, &trader, &loot, &mut rng)).collect();
        assert_eq!(all.len(), 9);
    }

    #[test]
    fn payment_picks_the_trade() {
        let emerald = items::id("minecraft:emerald").unwrap() as i32;
        let wheat = items::id("minecraft:wheat").unwrap() as i32;
        let bread = items::id("minecraft:bread").unwrap() as i32;
        let offers = vec![
            Offer::new(ItemCost::new(wheat, 20), None, ItemStack::new(emerald, 1), 16, 2, 0.05),
            Offer::new(ItemCost::new(emerald, 1), None, ItemStack::new(bread, 6), 16, 1, 0.05),
        ];
        let (active, result) = sell_item(&offers, &Slot::of(wheat, 25), &Slot::EMPTY, None);
        assert_eq!((active, result), (Some(0), Slot::of(emerald, 1)));
        // The payment can be in the second slot.
        assert_eq!(sell_item(&offers, &Slot::EMPTY, &Slot::of(emerald, 3), None).0, Some(1));
        assert_eq!(sell_item(&offers, &Slot::of(wheat, 19), &Slot::EMPTY, None).1, Slot::EMPTY);
        let mut used = offers.clone();
        used[0].uses = 16;
        assert_eq!(sell_item(&used, &Slot::of(wheat, 25), &Slot::EMPTY, None).1, Slot::EMPTY, "out of stock");

        let mut slots = vec![Slot::EMPTY; 39];
        slots[5] = Slot::of(wheat, 50);
        slots[7] = Slot::of(wheat, 30);
        fill_payment(&mut slots, 0, &offers[0].cost_a);
        assert_eq!(slots[0], Slot::of(wheat, 64));
        assert_eq!((slots[5].clone(), slots[7].clone()), (Slot::EMPTY, Slot::of(wheat, 16)));
    }
}
