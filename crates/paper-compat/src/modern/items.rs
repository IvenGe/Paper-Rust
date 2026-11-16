//! Items: their ids, and their data components in the older version's terms (component
//! types it lacks are left out).

use super::Modern;
use crate::components::{self, Ids};
use crate::io::{self, Result, Rw};
use crate::Outcome;
use paper_protocol::codec::{Reader, WriteExt};

/// The id conversions of a 26.3 to old translation.
struct ToOld<'a>(&'a Modern);

impl Ids for ToOld<'_> {
    fn item(&self, id: i32) -> i32 {
        self.0.registry_id("minecraft:item", id)
    }
    fn block_state(&self, id: i32) -> i32 {
        self.0.registry_id_blockstate(id)
    }
    fn component(&self, id: i32) -> i32 {
        self.0.registry_id("minecraft:data_component_type", id)
    }
    fn registry(&self, registry: &str, id: i32) -> i32 {
        self.0.registry_id(registry, id)
    }
    fn to_old(&self) -> bool {
        true
    }
    fn protocol(&self) -> i32 {
        self.0.version.protocol
    }
}

/// The id conversions of an old to 26.3 translation (what a client sends).
struct ToNew<'a>(&'a Modern);

impl Ids for ToNew<'_> {
    fn item(&self, id: i32) -> i32 {
        self.0.registry_id_new("minecraft:item", id)
    }
    fn block_state(&self, id: i32) -> i32 {
        self.0.data.mapping("blockstates").current(id).max(0)
    }
    fn component(&self, id: i32) -> i32 {
        self.0.registry_id_new("minecraft:data_component_type", id)
    }
    fn registry(&self, registry: &str, id: i32) -> i32 {
        self.0.registry_id_new(registry, id)
    }
    fn to_old(&self) -> bool {
        false
    }
    fn protocol(&self) -> i32 {
        self.0.version.protocol
    }
}

/// A slot (the trusted form servers send).
pub(super) fn slot(m: &Modern, rw: &mut Rw<'_>) -> Result<()> {
    components::slot(&ToOld(m), rw)
}

/// An item template (in item particles and recipe displays).
pub(super) fn template(m: &Modern, rw: &mut Rw<'_>) -> Result<()> {
    components::template(&ToOld(m), rw)
}

pub(super) fn resolvable_profile(rw: &mut Rw<'_>) -> Result<()> {
    components::resolvable_profile(rw)
}

pub(super) fn equipment(m: &Modern, old: i32, body: &[u8]) -> Result<Vec<u8>> {
    let mut rw = Rw::new(body, old);
    rw.varint()?;
    loop {
        let slot_byte = rw.u8()?;
        slot(m, &mut rw)?;
        if slot_byte & 0x80 == 0 {
            break;
        }
    }
    Ok(rw.finish())
}

pub(super) fn container_content(m: &Modern, old: i32, body: &[u8]) -> Result<Vec<u8>> {
    let mut rw = Rw::new(body, old);
    rw.varint()?;
    rw.varint()?;
    let count = rw.varint()?;
    for _ in 0..count {
        slot(m, &mut rw)?;
    }
    slot(m, &mut rw)?;
    Ok(rw.finish())
}

pub(super) fn container_slot(m: &Modern, old: i32, body: &[u8]) -> Result<Vec<u8>> {
    let mut rw = Rw::new(body, old);
    rw.varint()?;
    rw.varint()?;
    rw.i16()?;
    slot(m, &mut rw)?;
    Ok(rw.finish())
}

pub(super) fn cursor(m: &Modern, old: i32, body: &[u8]) -> Result<Vec<u8>> {
    let mut rw = Rw::new(body, old);
    slot(m, &mut rw)?;
    Ok(rw.finish())
}

pub(super) fn player_inventory(m: &Modern, old: i32, body: &[u8]) -> Result<Vec<u8>> {
    let mut rw = Rw::new(body, old);
    rw.varint()?;
    slot(m, &mut rw)?;
    Ok(rw.finish())
}

/// A trade's cost: an item, a count and the component values the payment must have.
fn trade_cost(m: &Modern, rw: &mut Rw<'_>) -> Result<()> {
    components::item_cost(&ToOld(m), rw)
}

pub(super) fn merchant_offers(m: &Modern, old: i32, body: &[u8]) -> Result<Vec<u8>> {
    let mut rw = Rw::new(body, old);
    rw.varint()?;
    let offers = rw.varint()?;
    for _ in 0..offers {
        trade_cost(m, &mut rw)?;
        slot(m, &mut rw)?;
        if rw.bool()? {
            trade_cost(m, &mut rw)?;
        }
        rw.bool()?;
        rw.bytes(4 * 4 + 4 + 4)?;
    }
    Ok(rw.finish())
}

/// The menu type of brewing stands, whose windows 26.3 gives two more properties.
fn brewing_stand_menu() -> i32 {
    paper_data::registries::id("minecraft:menu", "minecraft:brewing_stand").map_or(11, |i| i as i32)
}

pub(super) fn open_screen(m: &mut Modern, old: i32, body: &[u8]) -> Result<Vec<u8>> {
    let mut r = Reader::new(body);
    let window = r.varint()?;
    let menu = r.varint()?;
    m.brewing_stand = (menu == brewing_stand_menu()).then_some(window);
    Ok(io::with_id(old, body))
}

/// Brewing stands' total brewing time and total fuel (properties 2 and up) are new in 26.3.
pub(super) fn container_data(m: &Modern, old: i32, body: &[u8]) -> Result<Option<Vec<u8>>> {
    let mut r = Reader::new(body);
    let window = r.varint()?;
    let property = r.i16()?;
    if property >= 2 && m.brewing_stand == Some(window) {
        return Ok(None);
    }
    Ok(Some(io::with_id(old, body)))
}

/// Recipes: the item sets of slots and the stonecutter's recipes.
pub(super) fn update_recipes(m: &Modern, old: i32, body: &[u8]) -> Result<Vec<u8>> {
    let mut rw = Rw::new(body, old);
    let sets = rw.varint()?;
    for _ in 0..sets {
        rw.string()?;
        let items = rw.r.list(1 << 16, |r| r.varint())?;
        let items: Vec<i32> = items.into_iter().map(|i| m.registry_id("minecraft:item", i)).filter(|i| *i >= 0).collect();
        rw.w.put_varint(items.len() as i32);
        for item in items {
            rw.w.put_varint(item);
        }
    }
    let recipes = rw.varint()?;
    for _ in 0..recipes {
        components::holder_set(&ToOld(m), &mut rw, "minecraft:item")?;
        components::slot_display(&ToOld(m), &mut rw)?;
    }
    Ok(rw.finish())
}

/// A creative-mode slot: an untrusted slot, from the old version's ids to 26.3's.
pub(super) fn creative_slot(m: &Modern, id: i32, body: &[u8], out: &mut Outcome) -> Result<()> {
    let mut rw = Rw::new(body, id);
    rw.i16()?;
    components::untrusted_slot(&ToNew(m), &mut rw)?;
    out.to_server.push(rw.finish());
    Ok(())
}

/// A click in a window: the hashed slots the client sends with 26.3's item and component
/// ids (the hashes stay: where they differ the server sends the slot again).
pub(super) fn container_click(m: &Modern, id: i32, body: &[u8], out: &mut Outcome) -> Result<()> {
    let mut rw = Rw::new(body, id);
    rw.varint()?;
    rw.varint()?;
    rw.i16()?;
    rw.i8()?;
    rw.varint()?;
    let changed = rw.varint()?;
    for _ in 0..changed {
        rw.i16()?;
        hashed_slot(m, &mut rw)?;
    }
    hashed_slot(m, &mut rw)?;
    out.to_server.push(rw.finish());
    Ok(())
}

fn hashed_slot(m: &Modern, rw: &mut Rw<'_>) -> Result<()> {
    if !rw.bool()? {
        return Ok(());
    }
    let ids = ToNew(m);
    rw.map_varint(|item| ids.item(item).max(0))?;
    rw.varint()?;
    let added = rw.varint()?;
    for _ in 0..added {
        rw.map_varint(|ty| ids.component(ty).max(0))?;
        rw.i32()?;
    }
    let removed = rw.varint()?;
    for _ in 0..removed {
        rw.map_varint(|ty| ids.component(ty).max(0))?;
    }
    Ok(())
}
