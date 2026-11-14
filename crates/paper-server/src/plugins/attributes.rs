//! The plugin API's attributes (`Attributable`, `AttributeInstance`): the attribute maps of
//! players and mobs, by entity id.

use crate::server::attributes::{attributes_of, attributes_of_mut};
use crate::server::Server;
use crate::world::attributes::{default_value, is_attribute, Modifier, Operation};
use anyhow::{anyhow, bail, Result};
use paper_bridge::{bmap, Value};

pub fn handle(server: &mut Server, method: &str, args: &Value) -> Result<Value> {
    let attribute = args.str("attribute").ok_or_else(|| anyhow!("missing attribute"))?.to_owned();
    if !is_attribute(&attribute) {
        bail!("unknown attribute {attribute}");
    }
    // `Attribute#getDefaultValue`.
    if method == "attribute.default" {
        return Ok(Value::from(default_value(&attribute)));
    }
    let id = args.i32("id").ok_or_else(|| anyhow!("missing id"))?;
    // Equipment or effects changed by the plugin a moment ago count at once.
    if server.players.contains_key(&id) {
        server.refresh_player_attributes(id);
    } else if let Some(world) = server.worlds.iter().position(|w| w.entities.contains_key(&id)) {
        server.refresh_mob_attributes(world, id);
    }
    match method {
        // The instance, or null if the entity does not have the attribute.
        "attribute.get" => {
            let Some(attributes) = attributes_of(server, id) else { return Ok(Value::Null) };
            let Some(base) = attributes.base(&attribute) else { return Ok(Value::Null) };
            let modifiers = attributes
                .modifiers(&attribute)
                .into_iter()
                .map(|m| bmap! { "id" => m.id, "amount" => m.amount, "operation" => m.operation.id() as i64 })
                .collect();
            Ok(bmap! {
                "base" => base,
                "value" => attributes.value(&attribute),
                "default" => default_value(&attribute),
                "modifiers" => Value::List(modifiers),
            })
        }
        "attribute.set_base" => {
            let base = args.f64("base").ok_or_else(|| anyhow!("missing base"))?;
            let attributes = attributes_of_mut(server, id).ok_or_else(|| anyhow!("entity {id} has no attributes"))?;
            if !attributes.set_base(&attribute, base) {
                bail!("entity {id} does not have {attribute}");
            }
            sync_now(server, id);
            Ok(Value::Null)
        }
        "attribute.add_modifier" => {
            let modifier = args.str("modifier").ok_or_else(|| anyhow!("missing modifier"))?.to_owned();
            let operation = Operation::from_id(args.i64("operation").unwrap_or(0)).ok_or_else(|| anyhow!("bad operation"))?;
            let amount = args.f64("amount").unwrap_or(0.0);
            let persistent = args.bool("persistent").unwrap_or(true);
            let attributes = attributes_of_mut(server, id).ok_or_else(|| anyhow!("entity {id} has no attributes"))?;
            if !attributes.add_modifier(&attribute, Modifier::new(modifier, amount, operation), persistent) {
                bail!("entity {id} does not have {attribute}");
            }
            sync_now(server, id);
            Ok(Value::Null)
        }
        "attribute.remove_modifier" => {
            let modifier = args.str("modifier").ok_or_else(|| anyhow!("missing modifier"))?.to_owned();
            if let Some(attributes) = attributes_of_mut(server, id) {
                attributes.remove_modifier(&attribute, &modifier);
            }
            sync_now(server, id);
            Ok(Value::Null)
        }
        "attribute.register" => {
            let attributes = attributes_of_mut(server, id).ok_or_else(|| anyhow!("entity {id} has no attributes"))?;
            attributes.register(&attribute);
            Ok(Value::Null)
        }
        _ => bail!("unknown method {method}"),
    }
}

/// Plugins read values right after changing them (`setBaseValue` then `getHealth`), so max
/// health caps the health at once rather than at the next tick.
fn sync_now(server: &mut Server, id: i32) {
    let Some(max) = attributes_of(server, id).map(|a| a.value("max_health") as f32) else { return };
    if let Some(p) = server.players.get_mut(&id) {
        p.health = p.health.min(max);
        return;
    }
    for w in &mut server.worlds {
        if let Some(mob) = w.entities.get_mut(&id).and_then(|e| e.mob_mut()) {
            mob.living.max_health = max;
            mob.living.health = mob.living.health.min(max);
        }
    }
}
