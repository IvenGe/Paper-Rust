//! Merchants: the offers of villagers and wandering traders (`MerchantOffer`, `ItemCost`)
//! and what a villager remembers (`VillagerData`, experience, restocks, gossip), with their
//! saved forms as vanilla writes them.

use super::entity_nbt::{uuid_from, uuid_tag};
use crate::server::playerdata::{item_from_nbt, item_to_nbt};
use paper_data::items;
use paper_nbt::{Compound, Tag};
use paper_protocol::codec::BlockPos;
use paper_protocol::packets::play::{TradeItem, TradeOffer};
use paper_protocol::types::{ItemStack, Slot};
use uuid::Uuid;

/// Experience a villager needs for each level (`VillagerData.NEXT_LEVEL_XP_THRESHOLDS`).
pub const LEVEL_XP: [i32; 5] = [0, 10, 70, 150, 250];

/// Experience at which a villager of `level` reaches the next one (0 at the top).
pub fn max_xp_for_level(level: i32) -> i32 {
    if (1..5).contains(&level) {
        LEVEL_XP[level as usize]
    } else {
        0
    }
}

fn max_stack(item: i32) -> i32 {
    items::get(item as u32).map_or(64, |i| i.max_stack_size as i32).max(1)
}

/// What a trade asks for (`ItemCost`): an item, how many, and component values the payment
/// must have (in their network form).
#[derive(Clone, Debug, PartialEq)]
pub struct ItemCost {
    pub item: i32,
    pub count: i32,
    pub components: Vec<(i32, Vec<u8>)>,
}

impl ItemCost {
    pub fn new(item: i32, count: i32) -> Self {
        Self { item, count, components: Vec::new() }
    }

    /// `ItemCost.test`: the same item with the listed component values.
    pub fn matches(&self, stack: &ItemStack) -> bool {
        stack.item == self.item && self.components.iter().all(|(ty, data)| stack.added.iter().any(|(t, d)| t == ty && d == data))
    }

    /// The item as a stack of `count`.
    pub fn stack(&self, count: i32) -> ItemStack {
        ItemStack { item: self.item, count, added: self.components.clone(), removed: Vec::new() }
    }

    fn to_packet(&self, count: i32) -> TradeItem {
        TradeItem { item: self.item, count, components: self.components.clone() }
    }

    /// `{id, count, components}` (components in this server's raw form, as items keep them).
    pub fn to_nbt(&self) -> Compound {
        let mut c = item_to_nbt(&Slot(Some(self.stack(self.count.max(1))))).unwrap_or_default();
        c.insert("count", Tag::Int(self.count));
        c
    }

    pub fn from_nbt(c: &Compound) -> Option<Self> {
        let count = c.i32("count").unwrap_or(1);
        let mut with_count = c.clone();
        with_count.insert("count", Tag::Int(count.max(1)));
        let stack = item_from_nbt(&with_count).0?;
        Some(Self { item: stack.item, count, components: stack.added })
    }
}

/// One trade of a merchant (`MerchantOffer`).
#[derive(Clone, Debug, PartialEq)]
pub struct Offer {
    pub cost_a: ItemCost,
    pub cost_b: Option<ItemCost>,
    pub result: ItemStack,
    pub uses: i32,
    pub max_uses: i32,
    /// Whether trading drops experience orbs.
    pub reward_xp: bool,
    /// Price change from reputation and Hero of the Village while a player trades.
    pub special_price: i32,
    pub demand: i32,
    pub price_multiplier: f32,
    /// Experience the merchant gains from the trade.
    pub xp: i32,
    /// Paper's `MerchantRecipe#shouldIgnoreDiscounts`: no reputation or hero discounts.
    pub ignore_discounts: bool,
}

impl Offer {
    pub fn new(cost_a: ItemCost, cost_b: Option<ItemCost>, result: ItemStack, max_uses: i32, xp: i32, price_multiplier: f32) -> Self {
        Self { cost_a, cost_b, result, uses: 0, max_uses, reward_xp: true, special_price: 0, demand: 0, price_multiplier, xp, ignore_discounts: false }
    }

    /// `getModifiedCostCount`: the first cost with demand and special prices applied.
    pub fn cost_a_count(&self) -> i32 {
        let base = self.cost_a.count;
        let demand = ((base as f32 * self.demand as f32 * self.price_multiplier).floor() as i32).max(0);
        (base + demand + self.special_price).clamp(1, max_stack(self.cost_a.item))
    }

    pub fn out_of_stock(&self) -> bool {
        self.uses >= self.max_uses
    }

    /// `satisfiedBy`: whether the two payment stacks pay for the trade.
    pub fn satisfied_by(&self, a: &Slot, b: &Slot) -> bool {
        let Some(a) = a.0.as_ref().filter(|s| s.count > 0) else { return false };
        if !self.cost_a.matches(a) || a.count < self.cost_a_count() {
            return false;
        }
        match &self.cost_b {
            None => b.is_empty(),
            Some(cost) => b.0.as_ref().is_some_and(|b| b.count > 0 && cost.matches(b) && b.count >= cost.count),
        }
    }

    /// `take`: uses up the payment if it pays for the trade.
    pub fn take(&self, a: &mut Slot, b: &mut Slot) -> bool {
        if !self.satisfied_by(a, b) {
            return false;
        }
        shrink(a, self.cost_a_count());
        if let Some(cost) = &self.cost_b {
            shrink(b, cost.count);
        }
        true
    }

    /// `updateDemand`, when restocking: trades used a lot get dearer, unused ones cheaper.
    pub fn update_demand(&mut self) {
        self.demand = self.demand + self.uses - (self.max_uses - self.uses);
    }

    pub fn to_packet(&self) -> TradeOffer {
        TradeOffer {
            cost_a: self.cost_a.to_packet(self.cost_a.count),
            result: Slot(Some(self.result.clone())),
            cost_b: self.cost_b.as_ref().map(|c| c.to_packet(c.count)),
            out_of_stock: self.out_of_stock(),
            uses: self.uses,
            max_uses: self.max_uses,
            xp: self.xp,
            special_price: self.special_price,
            price_multiplier: self.price_multiplier,
            demand: self.demand,
        }
    }

    /// Vanilla's `MerchantOffer.CODEC`.
    pub fn to_nbt(&self) -> Compound {
        let mut c = Compound::new().with("buy", Tag::Compound(self.cost_a.to_nbt()));
        if let Some(b) = &self.cost_b {
            c.insert("buyB", Tag::Compound(b.to_nbt()));
        }
        c.insert("sell", Tag::Compound(item_to_nbt(&Slot(Some(self.result.clone()))).unwrap_or_default()));
        c.insert("uses", Tag::Int(self.uses));
        c.insert("maxUses", Tag::Int(self.max_uses));
        c.insert("rewardExp", Tag::Byte(self.reward_xp as i8));
        c.insert("specialPrice", Tag::Int(self.special_price));
        c.insert("demand", Tag::Int(self.demand));
        c.insert("priceMultiplier", Tag::Float(self.price_multiplier));
        c.insert("xp", Tag::Int(self.xp));
        if self.ignore_discounts {
            c.insert("Paper.IgnoreDiscounts", Tag::Byte(1));
        }
        c
    }

    pub fn from_nbt(c: &Compound) -> Option<Self> {
        let cost_a = ItemCost::from_nbt(c.compound("buy")?)?;
        let cost_b = c.compound("buyB").and_then(ItemCost::from_nbt).filter(|b| b.count > 0);
        let result = item_from_nbt(c.compound("sell")?).0?;
        Some(Self {
            cost_a,
            cost_b,
            result,
            uses: c.i32("uses").unwrap_or(0),
            max_uses: c.i32("maxUses").unwrap_or(4),
            reward_xp: c.bool("rewardExp").unwrap_or(true),
            special_price: c.i32("specialPrice").unwrap_or(0),
            demand: c.i32("demand").unwrap_or(0),
            price_multiplier: c.f64("priceMultiplier").unwrap_or(0.0) as f32,
            xp: c.i32("xp").unwrap_or(1),
            ignore_discounts: c.bool("Paper.IgnoreDiscounts").unwrap_or(false),
        })
    }
}

fn shrink(slot: &mut Slot, n: i32) {
    if let Some(s) = slot.0.as_mut() {
        s.count -= n;
        if s.count <= 0 {
            *slot = Slot::EMPTY;
        }
    }
}

/// Kinds of gossip villagers share about players (`GossipType`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum GossipType {
    MajorNegative,
    MinorNegative,
    MinorPositive,
    MajorPositive,
    Trading,
}

impl GossipType {
    pub const ALL: [GossipType; 5] = [GossipType::MajorNegative, GossipType::MinorNegative, GossipType::MinorPositive, GossipType::MajorPositive, GossipType::Trading];

    pub fn id(self) -> &'static str {
        match self {
            GossipType::MajorNegative => "major_negative",
            GossipType::MinorNegative => "minor_negative",
            GossipType::MinorPositive => "minor_positive",
            GossipType::MajorPositive => "major_positive",
            GossipType::Trading => "trading",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|t| t.id() == id)
    }

    /// How much one point counts towards reputation.
    pub fn weight(self) -> i32 {
        match self {
            GossipType::MajorNegative => -5,
            GossipType::MinorNegative => -1,
            GossipType::MinorPositive | GossipType::Trading => 1,
            GossipType::MajorPositive => 5,
        }
    }

    pub fn max(self) -> i32 {
        match self {
            GossipType::MajorNegative => 100,
            GossipType::MinorNegative => 200,
            GossipType::MinorPositive | GossipType::Trading => 25,
            GossipType::MajorPositive => 20,
        }
    }

    /// Points lost each day.
    pub fn decay_per_day(self) -> i32 {
        match self {
            GossipType::MajorNegative => 10,
            GossipType::MinorNegative => 20,
            GossipType::MinorPositive => 1,
            GossipType::MajorPositive => 0,
            GossipType::Trading => 2,
        }
    }
}

/// A villager's or wandering trader's state (both are `AbstractVillager`s).
#[derive(Clone, Debug, PartialEq)]
pub struct Merchant {
    /// `minecraft:villager_type` entry (`minecraft:plains`, ...).
    pub villager_type: String,
    /// `minecraft:villager_profession` entry (`minecraft:none` while unemployed).
    pub profession: String,
    pub level: i32,
    pub xp: i32,
    /// `None` until first needed: vanilla works offers out when someone looks at them.
    pub offers: Option<Vec<Offer>>,
    /// The claimed workstation.
    pub job_site: Option<BlockPos>,
    /// A free workstation the villager is walking to, to claim it, and since when.
    pub potential_job_site: Option<BlockPos>,
    pub potential_since: i64,
    /// Game time of the next search for a workstation.
    pub next_job_search: i64,
    pub last_restock: i64,
    pub last_restock_check_day: i64,
    pub restocks_today: i32,
    pub last_gossip_decay: i64,
    pub last_worked: i64,
    pub gossips: Vec<(Uuid, GossipType, i32)>,
    /// The player trading (entity id); while set the merchant stands still.
    pub trading_player: Option<i32>,
    /// Head shaking ticks (`DATA_UNHAPPY_COUNTER`).
    pub unhappy: i32,
    /// Ticks until a pending level-up happens (`updateMerchantTimer`).
    pub level_up_timer: i32,
    pub level_up_pending: bool,
    /// The last player that traded, for gossip and happy particles next tick.
    pub last_traded: Option<Uuid>,
    /// Wandering traders: ticks until they leave (0 = stay).
    pub despawn_delay: i32,
}

impl Merchant {
    pub fn new(villager_type: &str, profession: &str) -> Self {
        Self {
            villager_type: villager_type.to_owned(),
            profession: profession.to_owned(),
            level: 1,
            xp: 0,
            offers: None,
            job_site: None,
            potential_job_site: None,
            potential_since: 0,
            next_job_search: 0,
            last_restock: 0,
            last_restock_check_day: 0,
            restocks_today: 0,
            last_gossip_decay: 0,
            last_worked: 0,
            gossips: Vec::new(),
            trading_player: None,
            unhappy: 0,
            level_up_timer: 0,
            level_up_pending: false,
            last_traded: None,
            despawn_delay: 0,
        }
    }

    /// `GossipContainer.getReputation`: the weighted sum of what this villager heard about
    /// a player.
    pub fn reputation(&self, player: Uuid) -> i32 {
        self.gossips.iter().filter(|(t, _, _)| *t == player).map(|(_, kind, value)| kind.weight() * value).sum()
    }

    /// `GossipContainer.add`: more gossip about a player, up to the kind's maximum.
    pub fn add_gossip(&mut self, player: Uuid, kind: GossipType, amount: i32) {
        match self.gossips.iter_mut().find(|(t, k, _)| *t == player && *k == kind) {
            Some((_, _, value)) => *value = (*value + amount).min(kind.max()),
            None => self.gossips.push((player, kind, amount.min(kind.max()))),
        }
        self.gossips.retain(|(_, _, v)| *v > 0);
    }

    /// `GossipContainer.decay`: a day's worth of forgetting.
    pub fn decay_gossip(&mut self) {
        for (_, kind, value) in &mut self.gossips {
            *value -= kind.decay_per_day();
        }
        self.gossips.retain(|(_, _, v)| *v > 0);
    }

    /// Whether a villager has enough experience for the next level (`shouldIncreaseLevel`).
    pub fn should_level_up(&self) -> bool {
        (1..5).contains(&self.level) && self.xp >= max_xp_for_level(self.level)
    }

    /// Writes the fields of `Villager.addAdditionalSaveData` (and the wandering trader's).
    pub fn save(&self, c: &mut Compound, dimension: &str, villager: bool) {
        if let Some(offers) = self.offers.as_ref().filter(|o| !o.is_empty()) {
            let recipes = offers.iter().map(|o| Tag::Compound(o.to_nbt())).collect();
            c.insert("Offers", Tag::Compound(Compound::new().with("Recipes", Tag::List(recipes))));
        }
        if !villager {
            c.insert("DespawnDelay", Tag::Int(self.despawn_delay));
            return;
        }
        c.insert(
            "VillagerData",
            Tag::Compound(Compound::new().with("type", Tag::String(self.villager_type.clone())).with("profession", Tag::String(self.profession.clone())).with("level", Tag::Int(self.level))),
        );
        c.insert("Xp", Tag::Int(self.xp));
        c.insert("LastRestock", Tag::Long(self.last_restock));
        c.insert("LastGossipDecay", Tag::Long(self.last_gossip_decay));
        c.insert("RestocksToday", Tag::Int(self.restocks_today));
        let gossips = self
            .gossips
            .iter()
            .map(|(target, kind, value)| Tag::Compound(Compound::new().with("Type", Tag::String(kind.id().to_owned())).with("Target", uuid_tag(*target)).with("Value", Tag::Int(*value))))
            .collect();
        c.insert("Gossips", Tag::List(gossips));
        // The job site is a brain memory in vanilla's format.
        let mut memories = Compound::new();
        if let Some(pos) = self.job_site {
            let global = Compound::new().with("pos", Tag::IntArray(vec![pos.x, pos.y, pos.z])).with("dimension", Tag::String(dimension.to_owned()));
            memories.insert("minecraft:job_site", Tag::Compound(Compound::new().with("value", Tag::Compound(global))));
        }
        c.insert("Brain", Tag::Compound(Compound::new().with("memories", Tag::Compound(memories))));
    }

    /// Reads what [`Merchant::save`] wrote (or vanilla did).
    pub fn load(c: &Compound, villager: bool) -> Self {
        let data = c.compound("VillagerData");
        let mut m = Merchant::new(
            data.and_then(|d| d.string("type")).unwrap_or("minecraft:plains"),
            if villager { data.and_then(|d| d.string("profession")).unwrap_or("minecraft:none") } else { "minecraft:none" },
        );
        m.level = data.and_then(|d| d.i32("level")).unwrap_or(1).clamp(1, 5);
        m.xp = c.i32("Xp").unwrap_or(0);
        if let Some(recipes) = c.compound("Offers").and_then(|o| o.list("Recipes")) {
            m.offers = Some(recipes.iter().filter_map(|r| r.as_compound().and_then(Offer::from_nbt)).collect());
        }
        m.last_restock = c.i64("LastRestock").unwrap_or(0);
        m.last_gossip_decay = c.i64("LastGossipDecay").unwrap_or(0);
        m.restocks_today = c.i32("RestocksToday").unwrap_or(0);
        for g in c.list("Gossips").unwrap_or(&[]) {
            let Some(g) = g.as_compound() else { continue };
            if let (Some(kind), Some(target), Some(value)) = (g.string("Type").and_then(GossipType::from_id), uuid_from(g.get("Target")), g.i32("Value")) {
                m.add_gossip(target, kind, value);
            }
        }
        let job_site = c.compound("Brain").and_then(|b| b.compound("memories")).and_then(|m| m.compound("minecraft:job_site")).and_then(|j| j.compound("value")).and_then(|v| match v.get("pos") {
            Some(Tag::IntArray(p)) if p.len() == 3 => Some(BlockPos::new(p[0], p[1], p[2])),
            _ => None,
        });
        m.job_site = job_site;
        m.despawn_delay = c.i32("DespawnDelay").unwrap_or(0);
        m
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(name: &str) -> i32 {
        items::id(name).unwrap() as i32
    }

    #[test]
    fn prices_follow_demand_and_special_prices() {
        let mut offer = Offer::new(ItemCost::new(item("minecraft:emerald"), 10), None, ItemStack::new(item("minecraft:bread"), 6), 16, 1, 0.05);
        assert_eq!(offer.cost_a_count(), 10);
        // Used up and restocked: demand rises and so does the price.
        offer.uses = 16;
        offer.update_demand();
        assert_eq!(offer.demand, 16);
        assert_eq!(offer.cost_a_count(), 18);
        // A good reputation lowers it, but never below one or above a stack.
        offer.special_price = -30;
        assert_eq!(offer.cost_a_count(), 1);
        offer.special_price = 100;
        assert_eq!(offer.cost_a_count(), 64);
    }

    #[test]
    fn payment_is_checked_and_taken() {
        let emerald = item("minecraft:emerald");
        let book = item("minecraft:book");
        let offer = Offer::new(ItemCost::new(emerald, 5), Some(ItemCost::new(book, 1)), ItemStack::new(item("minecraft:enchanted_book"), 1), 12, 1, 0.2);
        let (mut a, mut b) = (Slot::of(emerald, 7), Slot::of(book, 1));
        assert!(!offer.satisfied_by(&Slot::of(emerald, 4), &b));
        assert!(!offer.satisfied_by(&a, &Slot::EMPTY));
        assert!(offer.take(&mut a, &mut b));
        assert_eq!((a, b), (Slot::of(emerald, 2), Slot::EMPTY));
    }

    #[test]
    fn merchants_round_trip() {
        let mut m = Merchant::new("minecraft:taiga", "minecraft:librarian");
        m.level = 3;
        m.xp = 80;
        m.job_site = Some(BlockPos::new(4, 64, -9));
        let player = Uuid::from_u128(0x1234);
        m.add_gossip(player, GossipType::Trading, 40);
        let mut offer = Offer::new(ItemCost::new(item("minecraft:paper"), 24), None, ItemStack::new(item("minecraft:emerald"), 1), 16, 2, 0.05);
        offer.uses = 3;
        offer.demand = 5;
        m.offers = Some(vec![offer]);
        let mut c = Compound::new();
        m.save(&mut c, "minecraft:overworld", true);
        let back = Merchant::load(&c, true);
        assert_eq!(back, m);
        assert_eq!(back.reputation(player), 25, "gossip is capped at the kind's maximum");
    }
}
