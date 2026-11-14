//! Inventories for the plugin API: the player inventory (in Bukkit's slot order), ender
//! chests, container blocks, the crafting grid of the open window, and custom inventories
//! that plugins create with `Bukkit.createInventory` and open with `Player.openInventory`.
//!
//! The host names an inventory with a descriptor map (`kind` and what identifies it, see
//! [`InvRef::parse`]); the contents always live here. Custom inventories are kept in
//! [`super::PluginData::inventories`] until the host drops its last reference and nobody
//! looks at them any more.

use super::api::{item_value, slot_from};
use super::component_value;
use crate::server::containers::MenuKind;
use crate::server::player::{HOTBAR_START, OFFHAND_SLOT};
use crate::server::Server;
use anyhow::{anyhow, bail, Result};
use paper_bridge::{bmap, Value};
use paper_protocol::codec::BlockPos;
use paper_protocol::text::Component;
use paper_protocol::types::Slot;
use uuid::Uuid;

/// Number of slots of the player inventory in Bukkit's order: hotbar (0-8), main (9-35),
/// boots, leggings, chestplate, helmet (36-39), off hand (40).
pub const PLAYER_SLOTS: usize = 41;

/// A custom inventory made by a plugin.
#[derive(Clone, Debug)]
pub struct CustomInventory {
    pub items: Vec<Slot>,
    pub title: Component,
    /// `minecraft:menu` entry the client opens it as (`minecraft:generic_9x3`, ...).
    pub menu: String,
    /// Bukkit `InventoryType` name, reported back in events.
    pub kind: String,
    /// The host dropped its last reference; the inventory goes once nobody views it.
    pub released: bool,
}

/// An inventory the host refers to.
#[derive(Clone, Debug, PartialEq)]
pub enum InvRef {
    /// A player's inventory, in Bukkit's slot order.
    Player(i32),
    /// A player's ender chest.
    Ender(i32),
    /// A container block (two positions for a double chest).
    Block { world: usize, positions: Vec<BlockPos> },
    Custom(u64),
    /// The crafting grid of the window a player has open: result first.
    Crafting(i32),
}

/// Bukkit `PlayerInventory` index to the player window slot.
pub fn player_window_slot(index: usize) -> Option<usize> {
    Some(match index {
        0..=8 => HOTBAR_START + index,
        9..=35 => index,
        36..=39 => 8 - (index - 36),
        40 => OFFHAND_SLOT,
        _ => return None,
    })
}

fn player_by_uuid(server: &Server, value: &Value, key: &str) -> Result<i32> {
    let uuid: Uuid = value.str(key).ok_or_else(|| anyhow!("missing {key}"))?.parse()?;
    server.by_uuid.get(&uuid).copied().ok_or_else(|| anyhow!("player {uuid} is not online"))
}

impl InvRef {
    /// Reads a descriptor: `{kind: "player"|"ender"|"crafting", uuid}`,
    /// `{kind: "block", world, x, y, z}` (or `positions: [[x, y, z], ...]`), `{kind: "custom", id}`.
    pub fn parse(server: &Server, value: &Value) -> Result<InvRef> {
        Ok(match value.str("kind") {
            Some("player") => InvRef::Player(player_by_uuid(server, value, "uuid")?),
            Some("ender") => InvRef::Ender(player_by_uuid(server, value, "uuid")?),
            Some("crafting") => InvRef::Crafting(player_by_uuid(server, value, "uuid")?),
            Some("custom") => {
                let id = value.i64("id").ok_or_else(|| anyhow!("missing id"))? as u64;
                if !server.plugin_data.inventories.contains_key(&id) {
                    bail!("unknown custom inventory {id}");
                }
                InvRef::Custom(id)
            }
            Some("block") => {
                let world = match value.str("world") {
                    Some(name) => server.worlds.iter().position(|w| w.name == name).ok_or_else(|| anyhow!("unknown world {name}"))?,
                    None => 0,
                };
                let positions = match value.list("positions") {
                    Some(list) => list.iter().filter_map(|p| Some(BlockPos::new(p.at(0).as_i32()?, p.at(1).as_i32()?, p.at(2).as_i32()?))).collect(),
                    None => vec![BlockPos::new(value.i32("x").unwrap_or(0), value.i32("y").unwrap_or(0), value.i32("z").unwrap_or(0))],
                };
                if positions.is_empty() {
                    bail!("block inventory without a position");
                }
                InvRef::Block { world, positions }
            }
            other => bail!("unknown inventory kind {other:?}"),
        })
    }

    /// The descriptor of this inventory.
    pub fn value(&self, server: &Server) -> Value {
        let uuid = |id: &i32| server.players.get(id).map(|p| p.uuid().to_string());
        match self {
            InvRef::Player(id) => bmap! { "kind" => "player", "uuid" => uuid(id) },
            InvRef::Ender(id) => bmap! { "kind" => "ender", "uuid" => uuid(id) },
            InvRef::Crafting(id) => bmap! { "kind" => "crafting", "uuid" => uuid(id) },
            InvRef::Custom(id) => bmap! { "kind" => "custom", "id" => *id as i64 },
            InvRef::Block { world, positions } => {
                let p = positions[0];
                let mut v = bmap! { "kind" => "block", "world" => server.worlds[*world].name.clone(), "x" => p.x, "y" => p.y, "z" => p.z };
                if positions.len() > 1 {
                    v.set("positions", Value::List(positions.iter().map(|p| Value::List(vec![p.x.into(), p.y.into(), p.z.into()])).collect()));
                }
                v
            }
        }
    }

    /// The players looking at this inventory in an open window.
    pub fn viewers(&self, server: &Server) -> Vec<i32> {
        server
            .player_order
            .iter()
            .filter_map(|id| server.players.get(id))
            .filter(|p| {
                let kind = p.open_menu.as_ref().map(|m| &m.kind);
                match (self, kind) {
                    (InvRef::Player(id), None) | (InvRef::Crafting(id), None | Some(MenuKind::CraftingTable)) => p.entity_id == *id,
                    // Every window shows the player's own inventory below the container.
                    (InvRef::Player(id), Some(_)) => p.entity_id == *id,
                    (InvRef::Ender(id), Some(MenuKind::EnderChest)) => p.entity_id == *id,
                    (InvRef::Custom(id), Some(MenuKind::Custom { id: open })) => open == id,
                    (InvRef::Block { world, positions }, Some(MenuKind::Container { world: w, positions: open })) => w == world && positions.iter().any(|p| open.contains(p)),
                    (InvRef::Block { world, positions }, Some(MenuKind::Furnace { world: w, pos, .. } | MenuKind::Brewing { world: w, pos })) => w == world && positions.contains(pos),
                    _ => false,
                }
            })
            .map(|p| p.entity_id)
            .collect()
    }
}

/// The items of an inventory (empty slots included).
pub fn contents(server: &Server, inv: &InvRef) -> Result<Vec<Slot>> {
    Ok(match inv {
        InvRef::Player(id) => {
            let p = server.players.get(id).ok_or_else(|| anyhow!("player left"))?;
            (0..PLAYER_SLOTS).map(|i| p.inventory[player_window_slot(i).unwrap()].clone()).collect()
        }
        InvRef::Ender(id) => server.players.get(id).ok_or_else(|| anyhow!("player left"))?.ender_items.clone(),
        InvRef::Crafting(id) => {
            let p = server.players.get(id).ok_or_else(|| anyhow!("player left"))?;
            match &p.open_menu {
                Some(m) if matches!(m.kind, MenuKind::CraftingTable | MenuKind::Enchanting { .. } | MenuKind::Anvil { .. } | MenuKind::Merchant { .. }) => m.craft_grid.clone(),
                _ => p.inventory[0..5].to_vec(),
            }
        }
        InvRef::Custom(id) => server.plugin_data.inventories.get(id).ok_or_else(|| anyhow!("unknown custom inventory {id}"))?.items.clone(),
        InvRef::Block { world, positions } => {
            let mut out = Vec::new();
            for p in positions {
                if server.worlds[*world].block_at(p.x, p.y, p.z).is_none() {
                    bail!("the chunk at {}, {} is not loaded", p.x, p.z);
                }
                let items = server.container_items(*world, *p);
                if items.is_empty() {
                    bail!("the block at {}, {}, {} is not a container", p.x, p.y, p.z);
                }
                out.extend(items);
            }
            out
        }
    })
}

/// Changes slots of an inventory and shows the change to everyone looking at it.
pub fn set_slots(server: &mut Server, inv: &InvRef, changes: Vec<(usize, Slot)>) -> Result<()> {
    let size = contents(server, inv)?.len();
    if let Some((index, _)) = changes.iter().find(|(i, _)| *i >= size) {
        bail!("slot {index} is outside the inventory (size {size})");
    }
    match inv {
        InvRef::Player(id) => {
            for (index, item) in changes {
                server.set_inventory_slot(*id, player_window_slot(index).unwrap(), item);
            }
            if server.players.get(id).is_some_and(|p| p.open_menu.is_some()) {
                server.send_menu_contents(*id);
            }
            return Ok(());
        }
        InvRef::Ender(id) => {
            let p = server.players.get_mut(id).ok_or_else(|| anyhow!("player left"))?;
            for (index, item) in changes {
                p.ender_items[index] = item;
            }
        }
        InvRef::Crafting(id) => {
            let p = server.players.get_mut(id).ok_or_else(|| anyhow!("player left"))?;
            match p.open_menu.as_mut() {
                Some(m) if matches!(m.kind, MenuKind::CraftingTable | MenuKind::Enchanting { .. } | MenuKind::Anvil { .. } | MenuKind::Merchant { .. }) => {
                    for (index, item) in changes {
                        m.craft_grid[index] = item;
                    }
                }
                _ => {
                    for (index, item) in changes {
                        p.inventory[index] = item;
                    }
                }
            }
        }
        InvRef::Custom(id) => {
            let custom = server.plugin_data.inventories.get_mut(id).ok_or_else(|| anyhow!("unknown custom inventory {id}"))?;
            for (index, item) in changes {
                custom.items[index] = item;
            }
        }
        InvRef::Block { world, positions } => {
            let mut items = contents(server, inv)?;
            for (index, item) in changes {
                items[index] = item;
            }
            let mut offset = 0;
            for p in positions.clone() {
                let size = server.container_items(*world, p).len();
                server.set_container_items(*world, p, &items[offset..offset + size]);
                offset += size;
            }
        }
    }
    for viewer in inv.viewers(server) {
        server.send_menu_contents(viewer);
    }
    Ok(())
}

/// Bukkit's inventory type for the storage of a container block.
fn block_inventory_type(block: &str) -> &'static str {
    match block {
        "minecraft:barrel" => "BARREL",
        "minecraft:hopper" => "HOPPER",
        "minecraft:dispenser" => "DISPENSER",
        "minecraft:dropper" => "DROPPER",
        _ => "CHEST",
    }
}

/// The menu type and Bukkit inventory type of a custom inventory with `size` slots of `kind`.
fn custom_menu(kind: &str, size: usize) -> Result<&'static str> {
    Ok(match kind {
        "CHEST" | "PLAYER" | "ENDER_CHEST" | "BARREL" | "SHULKER_BOX" => match size {
            9 => "minecraft:generic_9x1",
            18 => "minecraft:generic_9x2",
            27 => "minecraft:generic_9x3",
            36 => "minecraft:generic_9x4",
            45 => "minecraft:generic_9x5",
            54 => "minecraft:generic_9x6",
            _ => bail!("Size for custom inventory must be a multiple of 9 between 9 and 54 slots (got {size})"),
        },
        "HOPPER" if size == 5 => "minecraft:hopper",
        "DISPENSER" | "DROPPER" if size == 9 => "minecraft:generic_3x3",
        other => bail!("custom inventories of type {other} with {size} slots are not supported by Paper-RS yet"),
    })
}

/// Makes a custom inventory. Returns its id.
pub fn create(server: &mut Server, kind: &str, size: usize, title: Component) -> Result<u64> {
    let menu = custom_menu(kind, size)?;
    let id = server.plugin_data.next_inventory;
    server.plugin_data.next_inventory += 1;
    server.plugin_data.inventories.insert(id, CustomInventory { items: vec![Slot::EMPTY; size], title, menu: menu.to_owned(), kind: kind.to_owned(), released: false });
    Ok(id)
}

/// Drops custom inventories the host released once nobody views them any more.
pub fn collect_released(server: &mut Server) {
    let open: Vec<u64> = server.players.values().filter_map(|p| match p.open_menu.as_ref().map(|m| &m.kind) {
        Some(MenuKind::Custom { id }) => Some(*id),
        _ => None,
    }).collect();
    server.plugin_data.inventories.retain(|id, inv| !inv.released || open.contains(id));
}

/// Opens an inventory in a window for a player (`Player.openInventory`). Returns false if a
/// plugin cancelled the `InventoryOpenEvent`.
pub fn open(server: &mut Server, id: i32, inv: &InvRef, title: Option<Component>) -> Result<bool> {
    let (kind, menu, default_title) = match inv {
        InvRef::Custom(cid) => {
            let custom = &server.plugin_data.inventories[cid];
            (MenuKind::Custom { id: *cid }, custom.menu.clone(), custom.title.clone())
        }
        InvRef::Ender(owner) if *owner == id => (MenuKind::EnderChest, "minecraft:generic_9x3".to_owned(), Component::translatable("container.enderchest", vec![])),
        InvRef::Block { world, positions } => {
            let state = server.worlds[*world].block_at(positions[0].x, positions[0].y, positions[0].z).ok_or_else(|| anyhow!("the chunk is not loaded"))?;
            match crate::server::containers::container_block(state) {
                Some(crate::server::containers::ContainerBlock::Furnace(recipe_type)) => {
                    let (menu, title) = match recipe_type {
                        "minecraft:smoking" => ("minecraft:smoker", "container.smoker"),
                        "minecraft:blasting" => ("minecraft:blast_furnace", "container.blast_furnace"),
                        _ => ("minecraft:furnace", "container.furnace"),
                    };
                    (MenuKind::Furnace { world: *world, pos: positions[0], recipe_type }, menu.to_owned(), Component::translatable(title, vec![]))
                }
                Some(crate::server::containers::ContainerBlock::Chest | crate::server::containers::ContainerBlock::Barrel) => {
                    let (menu, title) = if positions.len() > 1 { ("minecraft:generic_9x6", "container.chestDouble") } else { ("minecraft:generic_9x3", "container.chest") };
                    (MenuKind::Container { world: *world, positions: positions.clone() }, menu.to_owned(), Component::translatable(title, vec![]))
                }
                _ => bail!("opening this block's inventory is not supported by Paper-RS yet"),
            }
        }
        InvRef::Player(_) | InvRef::Crafting(_) | InvRef::Ender(_) => bail!("opening another player's inventory or a crafting grid is not supported by Paper-RS yet"),
    };
    Ok(server.open_menu(id, kind, &menu, title.unwrap_or(default_title)))
}

/// Describes the window a player has open, for events and the player state.
pub fn window_value(server: &Server, id: i32) -> Value {
    let Some(p) = server.players.get(&id) else { return Value::Null };
    let menu = p.open_menu.as_ref();
    let mut out = menu_value(server, id, menu.map(|m| &m.kind));
    out.set("window_id", menu.map_or(0, |m| m.window_id));
    out
}

/// Describes a window of kind `kind` (`None`: the player's own inventory) for player `id`:
/// its Bukkit type, size, title and the descriptor of its top inventory.
pub fn menu_value(server: &Server, id: i32, kind: Option<&MenuKind>) -> Value {
    let world_name = |w: usize| server.worlds.get(w).map(|w| w.name.clone()).unwrap_or_default();
    let (ty, size, top, title) = match kind {
        None => ("CRAFTING".to_owned(), 5, InvRef::Crafting(id), None),
        Some(MenuKind::Container { world, positions }) => {
            let p0 = positions[0];
            let state = server.worlds[*world].block_at(p0.x, p0.y, p0.z);
            let block = state.map(|s| paper_data::blocks::block_of_state(s).name).unwrap_or("minecraft:chest");
            let per_block = state.map_or(27, crate::server::containers::storage_size).max(1);
            (block_inventory_type(block).to_owned(), positions.len() * per_block, InvRef::Block { world: *world, positions: positions.clone() }, None)
        }
        Some(MenuKind::EnderChest) => ("ENDER_CHEST".to_owned(), 27, InvRef::Ender(id), None),
        Some(MenuKind::CraftingTable) => ("WORKBENCH".to_owned(), 10, InvRef::Crafting(id), None),
        Some(MenuKind::Enchanting { .. }) => ("ENCHANTING".to_owned(), 2, InvRef::Crafting(id), None),
        Some(MenuKind::Anvil { .. }) => ("ANVIL".to_owned(), 3, InvRef::Crafting(id), None),
        Some(MenuKind::Merchant { .. }) => ("MERCHANT".to_owned(), 3, InvRef::Crafting(id), None),
        Some(MenuKind::Brewing { world, pos }) => ("BREWING".to_owned(), 5, InvRef::Block { world: *world, positions: vec![*pos] }, None),
        Some(MenuKind::Furnace { world, pos, recipe_type }) => {
            let ty = match *recipe_type {
                "minecraft:smoking" => "SMOKER",
                "minecraft:blasting" => "BLAST_FURNACE",
                _ => "FURNACE",
            };
            (ty.to_owned(), 3, InvRef::Block { world: *world, positions: vec![*pos] }, None)
        }
        Some(MenuKind::Custom { id: cid }) => match server.plugin_data.inventories.get(cid) {
            Some(custom) => (custom.kind.clone(), custom.items.len(), InvRef::Custom(*cid), Some(custom.title.clone())),
            None => ("CHEST".to_owned(), 0, InvRef::Custom(*cid), None),
        },
    };
    let mut out = bmap! {
        "type" => ty,
        "size" => size as i64,
        "top" => top.value(server),
        "title" => title.as_ref().map(component_value),
    };
    if let Some(MenuKind::Merchant(merchant)) = kind {
        out.set("merchant", super::merchant::merchant_ref_value(server, *merchant));
    }
    // The fields inventory events carried before descriptors existed.
    if let InvRef::Block { world, positions } = &top {
        out.set("world", world_name(*world));
        out.set("x", positions[0].x);
        out.set("y", positions[0].y);
        out.set("z", positions[0].z);
    }
    if matches!(top, InvRef::Ender(_)) {
        out.set("owner", server.players.get(&id).map(|p| p.uuid().to_string()));
    }
    out
}

/// Executes an `inventory.*` API call.
pub fn handle(server: &mut Server, method: &str, args: &Value) -> Result<Value> {
    match method {
        "inventory.contents" => {
            let inv = InvRef::parse(server, args.get("inventory"))?;
            Ok(bmap! { "items" => Value::List(contents(server, &inv)?.iter().map(item_value).collect()) })
        }
        "inventory.set" => {
            let inv = InvRef::parse(server, args.get("inventory"))?;
            let mut changes = Vec::new();
            if let Some(items) = args.list("items") {
                for (i, item) in items.iter().enumerate() {
                    changes.push((i, slot_from(item)?));
                }
            }
            for entry in args.list("slots").unwrap_or(&[]) {
                let index = entry.at(0).as_i64().ok_or_else(|| anyhow!("slot entries are [index, item]"))?;
                changes.push((index as usize, slot_from(entry.at(1))?));
            }
            set_slots(server, &inv, changes)?;
            Ok(Value::Null)
        }
        "inventory.create" => {
            let size = args.i64("size").ok_or_else(|| anyhow!("missing size"))?;
            if !(1..=54).contains(&size) {
                bail!("inventory size {size} is out of range");
            }
            let title = super::component_from(args.get("title")).unwrap_or_else(|| Component::translatable("container.chest", vec![]));
            let id = create(server, args.str("type").unwrap_or("CHEST"), size as usize, title)?;
            Ok(bmap! { "id" => id as i64 })
        }
        "inventory.release" => {
            if let Some(inv) = args.i64("id").and_then(|id| server.plugin_data.inventories.get_mut(&(id as u64))) {
                inv.released = true;
            }
            collect_released(server);
            Ok(Value::Null)
        }
        "inventory.of_block" => {
            // The inventory of a container block: its type and descriptor (both halves of a
            // double chest), or null for blocks without one.
            let world = match args.str("world") {
                Some(name) => server.worlds.iter().position(|w| w.name == name).ok_or_else(|| anyhow!("unknown world {name}"))?,
                None => 0,
            };
            let pos = BlockPos::new(args.i32("x").unwrap_or(0), args.i32("y").unwrap_or(0), args.i32("z").unwrap_or(0));
            let Some((kind, positions)) = server.container_positions(world, pos) else { return Ok(Value::Null) };
            // Plugins see a loot chest filled, as in Paper.
            for p in &positions {
                server.unpack_loot(world, *p);
            }
            use crate::server::containers::ContainerBlock;
            let block = server.worlds[world].block_at(pos.x, pos.y, pos.z).map(|s| paper_data::blocks::block_of_state(s).name).unwrap_or("");
            let (ty, size) = match kind {
                ContainerBlock::Chest => ("CHEST", positions.len() * 27),
                ContainerBlock::Barrel => ("BARREL", 27),
                ContainerBlock::Furnace("minecraft:smoking") => ("SMOKER", 3),
                ContainerBlock::Furnace("minecraft:blasting") => ("BLAST_FURNACE", 3),
                ContainerBlock::Furnace(_) => ("FURNACE", 3),
                ContainerBlock::Hopper => ("HOPPER", 5),
                ContainerBlock::Dispenser => (block_inventory_type(block), 9),
                ContainerBlock::BrewingStand => ("BREWING", 5),
                ContainerBlock::EnderChest | ContainerBlock::CraftingTable | ContainerBlock::EnchantingTable | ContainerBlock::Anvil => return Ok(Value::Null),
            };
            let inv = InvRef::Block { world, positions };
            Ok(bmap! { "type" => ty, "size" => size as i64, "inventory" => inv.value(server) })
        }
        "inventory.viewers" => {
            let inv = InvRef::parse(server, args.get("inventory"))?;
            let viewers = inv.viewers(server);
            Ok(Value::List(viewers.iter().filter_map(|v| server.players.get(v)).map(|p| Value::Str(p.uuid().to_string())).collect()))
        }
        other => bail!("unknown plugin API method {other}"),
    }
}
