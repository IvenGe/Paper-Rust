//! Remaining features and shared placement helpers.

use crate::feature::parse::Parser;
use crate::feature::world::{Dir, Pos, World};
use crate::feature::Place;
use crate::random::Random;
use anyhow::Result;
use serde_json::Value;

pub(super) fn parse(_p: &mut Parser, _t: &str, _v: &Value) -> Result<Option<Box<dyn Place>>> {
    Ok(None)
}

/// `MossyCarpetBlock.getUpdatedState` for the side walls of a pale moss carpet.
fn carpet_sides(w: &World, mut state: u32, pos: Pos, create_sides: bool) -> u32 {
    let b = w.b;
    let carpet = b.known.pale_moss_carpet;
    let bottom = b.prop_bool(state, "bottom") == Some(true);
    let create = create_sides || bottom;
    let above = w.get(pos.up());
    let below = w.get(pos.down());
    let above_carpet = b.is_block(above, carpet);
    let below_carpet = b.is_block(below, carpet);
    for d in Dir::HORIZONTAL {
        let name = d.name();
        if b.sturdy(w.get(pos.rel(d)), d.opposite()) {
            if create {
                state = b.with(state, name, "low");
            }
            if b.prop(state, name) == Some("low") {
                if above_carpet && b.prop(above, name) != Some("none") && b.prop_bool(above, "bottom") != Some(true) {
                    state = b.with(state, name, "tall");
                }
                if !bottom && below_carpet && b.prop(below, name) == Some("none") {
                    state = b.with(state, name, "none");
                }
            }
        } else {
            state = b.with(state, name, "none");
        }
    }
    state
}

/// `MossyCarpetBlock.placeAt`: a carpet with walls, plus a topper with random walls.
pub(crate) fn place_mossy_carpet(w: &mut World, pos: Pos) {
    let b = w.b;
    let carpet = b.known.pale_moss_carpet;
    let base = carpet_sides(w, carpet, pos, true);
    w.set(pos, base);
    let above = pos.up();
    let prev = w.get(above);
    let is_carpet = b.is_block(prev, carpet);
    let can_top = (!is_carpet || b.prop_bool(prev, "bottom") != Some(true)) && (is_carpet || b.is_replaceable(prev));
    if !can_top {
        return;
    }
    let mut topper = carpet_sides(w, b.with_bool(carpet, "bottom", false), above, true);
    let mut r = w.region_random();
    for d in Dir::HORIZONTAL {
        if b.prop(topper, d.name()) != Some("none") && !r.next_bool() {
            topper = b.with(topper, d.name(), "none");
        }
    }
    w.store_region_random(r);
    let has_faces = b.prop_bool(topper, "bottom") == Some(true) || Dir::HORIZONTAL.iter().any(|d| b.prop(topper, d.name()) != Some("none"));
    if has_faces && topper != prev {
        w.set(above, topper);
        let updated = carpet_sides(w, base, pos, true);
        w.set(pos, updated);
    }
}
