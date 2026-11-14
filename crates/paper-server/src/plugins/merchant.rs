//! The plugin API's merchants: villagers and wandering traders (`Villager`,
//! `WanderingTrader`, `AbstractVillager`), their trades as `MerchantRecipe`s, merchants
//! plugins make (`Server.createMerchant`) and trading windows opened for players
//! (`HumanEntity.openMerchant`).
//!
//! A recipe crosses the bridge as a map: `result` and `ingredients` (item values; an
//! ingredient's components are what the payment must have), `uses`, `max_uses`,
//! `reward_exp`, `villager_xp`, `price_multiplier`, `demand`, `special_price` and
//! `ignore_discounts`.

use super::api::{item_value, slot_from};
use crate::server::containers::{MenuKind, MerchantRef};
use crate::server::Server;
use crate::world::merchant::{GossipType, ItemCost, Offer};
use anyhow::{anyhow, bail, Result};
use paper_bridge::{bmap, Value};
use paper_protocol::text::Component;
use paper_protocol::types::Slot;
use std::collections::BTreeMap;

/// A merchant a plugin made: its title and trades, and who trades with it. Once the plugin
/// host lets go of it (`released`), it goes when nobody trades with it any more.
#[derive(Clone, Debug)]
pub struct CustomMerchant {
    pub title: Component,
    pub offers: Vec<Offer>,
    pub trader: Option<i32>,
    pub released: bool,
}

/// A trade as plugins see it (`CraftMerchantRecipe`).
pub fn offer_value(offer: &Offer) -> Value {
    let mut ingredients = vec![item_value(&Slot(Some(offer.cost_a.stack(offer.cost_a.count.max(1)))))];
    if let Some(b) = &offer.cost_b {
        ingredients.push(item_value(&Slot(Some(b.stack(b.count.max(1))))));
    }
    bmap! {
        "result" => item_value(&Slot(Some(offer.result.clone()))),
        "ingredients" => Value::List(ingredients),
        "uses" => offer.uses,
        "max_uses" => offer.max_uses,
        "reward_exp" => offer.reward_xp,
        "villager_xp" => offer.xp,
        "price_multiplier" => offer.price_multiplier as f64,
        "demand" => offer.demand,
        "special_price" => offer.special_price,
        "ignore_discounts" => offer.ignore_discounts,
    }
}

fn cost_from(value: &Value) -> Result<Option<ItemCost>> {
    Ok(slot_from(value)?.0.map(|s| ItemCost { item: s.item, count: s.count, components: s.added }))
}

/// The inverse of [`offer_value`] (`CraftMerchantRecipe.toMinecraft`).
pub fn offer_from(value: &Value) -> Result<Offer> {
    let ingredients = value.list("ingredients").unwrap_or(&[]);
    let cost_a = match ingredients.first() {
        Some(v) => cost_from(v)?.ok_or_else(|| anyhow!("the first ingredient is empty"))?,
        None => bail!("No offered ingredients"),
    };
    let cost_b = match ingredients.get(1) {
        Some(v) => cost_from(v)?,
        None => None,
    };
    let result = slot_from(value.get("result"))?.0.unwrap_or_else(|| paper_protocol::types::ItemStack::new(0, 0));
    Ok(Offer {
        cost_a,
        cost_b,
        result,
        uses: value.i32("uses").unwrap_or(0),
        max_uses: value.i32("max_uses").unwrap_or(4),
        reward_xp: value.bool("reward_exp").unwrap_or(true),
        special_price: value.i32("special_price").unwrap_or(0),
        demand: value.i32("demand").unwrap_or(0),
        price_multiplier: value.f64("price_multiplier").unwrap_or(0.0) as f32,
        xp: value.i32("villager_xp").unwrap_or(1),
        ignore_discounts: value.bool("ignore_discounts").unwrap_or(false),
    })
}

/// The merchant a call names: `entity` (a villager or wandering trader) or `merchant` (one a
/// plugin made).
pub fn merchant_arg(server: &Server, args: &Value) -> Result<MerchantRef> {
    if let Some(id) = args.i64("merchant") {
        let id = id as u64;
        if !server.plugin_data.merchants.contains_key(&id) {
            bail!("unknown merchant {id}");
        }
        return Ok(MerchantRef::Custom(id));
    }
    let id = args.i32("entity").ok_or_else(|| anyhow!("missing entity or merchant"))?;
    let world = server.worlds.iter().position(|w| w.entities.get(&id).is_some_and(|e| e.mob().is_some_and(|m| m.merchant.is_some()))).ok_or_else(|| anyhow!("entity {id} is not a villager or wandering trader"))?;
    Ok(MerchantRef::Entity { world, id })
}

/// How plugins name a merchant (in events and open windows).
pub fn merchant_ref_value(server: &Server, merchant: MerchantRef) -> Value {
    match merchant {
        MerchantRef::Entity { world, id } => match server.worlds.get(world).and_then(|w| w.entities.get(&id)) {
            Some(e) => bmap! { "entity" => id as i64, "value" => super::entity::entity_value(server, world, e) },
            None => Value::Null,
        },
        MerchantRef::Custom(c) => bmap! { "merchant" => c as i64 },
    }
}

fn entity_arg(server: &Server, args: &Value) -> Result<(usize, i32)> {
    match merchant_arg(server, args)? {
        MerchantRef::Entity { world, id } => Ok((world, id)),
        MerchantRef::Custom(_) => bail!("not a villager"),
    }
}

fn gossip_values(gossips: &[(uuid::Uuid, GossipType, i32)]) -> Value {
    let mut by_player: BTreeMap<String, Value> = BTreeMap::new();
    for (target, kind, value) in gossips {
        let entry = by_player.entry(target.to_string()).or_insert_with(Value::map);
        entry.set(kind.id(), *value);
    }
    Value::Map(by_player)
}

pub fn handle(server: &mut Server, method: &str, args: &Value) -> Result<Value> {
    match method {
        "merchant.create" => {
            let title = super::component_from(args.get("title")).unwrap_or_else(|| Component::translatable("entity.minecraft.villager", vec![]));
            server.plugin_data.next_merchant += 1;
            let id = server.plugin_data.next_merchant;
            server.plugin_data.merchants.insert(id, CustomMerchant { title, offers: Vec::new(), trader: None, released: false });
            Ok(bmap! { "id" => id as i64 })
        }
        "merchant.release" => {
            if let Some(m) = args.i64("merchant").and_then(|id| server.plugin_data.merchants.get_mut(&(id as u64))) {
                m.released = true;
            }
            collect_released(server);
            Ok(Value::Null)
        }
        "merchant.recipes" => {
            let merchant = merchant_arg(server, args)?;
            Ok(Value::List(server.offers_of(merchant).iter().map(offer_value).collect()))
        }
        "merchant.set_recipes" => {
            let merchant = merchant_arg(server, args)?;
            let offers = args.list("recipes").unwrap_or(&[]).iter().map(offer_from).collect::<Result<Vec<_>>>()?;
            server.set_offers(merchant, offers);
            Ok(Value::Null)
        }
        "merchant.set_recipe" => {
            let merchant = merchant_arg(server, args)?;
            let index = args.i64("index").ok_or_else(|| anyhow!("missing index"))? as usize;
            let offer = offer_from(args.get("recipe"))?;
            let mut offers = server.offers_of(merchant);
            let Some(slot) = offers.get_mut(index) else { bail!("Index {index} out of bounds for length {}", offers.len()) };
            *slot = offer;
            server.set_offers(merchant, offers);
            Ok(Value::Null)
        }
        "merchant.trader" => {
            let merchant = merchant_arg(server, args)?;
            Ok(server.merchant_trader(merchant).and_then(|p| server.players.get(&p)).map(|p| Value::from(p.uuid().to_string())).unwrap_or(Value::Null))
        }
        "merchant.open" => {
            let merchant = merchant_arg(server, args)?;
            let uuid = args.str("uuid").ok_or_else(|| anyhow!("missing uuid"))?;
            let player = server.online_players().find(|p| p.uuid().to_string() == uuid).map(|p| p.entity_id).ok_or_else(|| anyhow!("player {uuid} is not online"))?;
            let opened = server.open_merchant(player, merchant, args.bool("force").unwrap_or(false));
            Ok(bmap! { "opened" => opened })
        }
        "merchant.window" => {
            let uuid = args.str("uuid").ok_or_else(|| anyhow!("missing uuid"))?;
            let Some(p) = server.online_players().find(|p| p.uuid().to_string() == uuid) else { return Ok(Value::Null) };
            let Some(menu) = p.open_menu.as_ref() else { return Ok(Value::Null) };
            let MenuKind::Merchant(merchant) = menu.kind else { return Ok(Value::Null) };
            let (selection, active) = (menu.trade_selection, menu.active_offer);
            let offers = server.offers_of(merchant);
            Ok(bmap! {
                "merchant" => merchant_ref_value(server, merchant),
                "selected" => selection.unwrap_or(0) as i64,
                "recipe" => active.and_then(|i| offers.get(i)).map(offer_value),
            })
        }
        "villager.increase_level" => {
            let (world, id) = entity_arg(server, args)?;
            let amount = args.i32("amount").unwrap_or(1).max(0);
            let before = server.worlds[world].entities.get(&id).and_then(|e| e.mob()?.merchant.as_ref().map(|m| m.level)).unwrap_or(5);
            let target = (before + amount).clamp(1, 5);
            for _ in before..target {
                server.level_up_villager(world, id);
            }
            Ok(bmap! { "changed" => target != before })
        }
        "villager.add_trades" => {
            let (world, id) = entity_arg(server, args)?;
            let amount = args.i32("amount").unwrap_or(1).max(0) as usize;
            Ok(bmap! { "changed" => server.add_trades(world, id, amount) })
        }
        "villager.shake_head" => {
            let (world, id) = entity_arg(server, args)?;
            server.set_unhappy(world, id);
            Ok(Value::Null)
        }
        "villager.restock" => {
            let (world, id) = entity_arg(server, args)?;
            server.restock(world, id);
            Ok(Value::Null)
        }
        "villager.update_demand" => {
            let (world, id) = entity_arg(server, args)?;
            server.update_demand(world, id);
            Ok(Value::Null)
        }
        "villager.reset_offers" => {
            let (world, id) = entity_arg(server, args)?;
            if let Some(m) = server.worlds[world].entities.get_mut(&id).and_then(|e| e.mob_mut()).and_then(|m| m.merchant.as_deref_mut()) {
                m.offers = None;
            }
            Ok(Value::Null)
        }
        "villager.reputations" | "villager.set_reputation" | "villager.clear_reputations" => {
            let (world, id) = entity_arg(server, args)?;
            let m = server.worlds[world].entities.get_mut(&id).and_then(|e| e.mob_mut()).and_then(|m| m.merchant.as_deref_mut()).ok_or_else(|| anyhow!("not a villager"))?;
            match method {
                "villager.reputations" => return Ok(gossip_values(&m.gossips)),
                "villager.clear_reputations" => m.gossips.clear(),
                _ => {
                    let target: uuid::Uuid = args.str("uuid").ok_or_else(|| anyhow!("missing uuid"))?.parse()?;
                    m.gossips.retain(|(t, _, _)| *t != target);
                    for (kind, value) in args.get("values").as_map().into_iter().flatten() {
                        let kind = GossipType::from_id(kind).ok_or_else(|| anyhow!("unknown reputation type {kind}"))?;
                        if let Some(v) = value.as_i64().filter(|v| *v > 0) {
                            m.gossips.push((target, kind, v as i32));
                        }
                    }
                }
            }
            Ok(Value::Null)
        }
        _ => bail!("unknown method {method}"),
    }
}

/// Forgets merchants plugins let go of once nobody trades with them.
pub fn collect_released(server: &mut Server) {
    let open: Vec<u64> = server
        .players
        .values()
        .filter_map(|p| match p.open_menu.as_ref()?.kind {
            MenuKind::Merchant(MerchantRef::Custom(c)) => Some(c),
            _ => None,
        })
        .collect();
    server.plugin_data.merchants.retain(|id, m| !m.released || open.contains(id));
}
