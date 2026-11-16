//! The compat tables agree with what the translators expect of them.

use paper_compat::data;
use paper_protocol::packets::State;

/// Packets 26.3 sends that some older version does not have, which the translators replace
/// or drop: 26.3's own, 26.1's (game rules, disk space), 1.21.9's (debug values, test
/// highlights, the code of conduct, which is agreed to for the client) and 1.21.6's
/// (dialogs, waypoints).
const NEW_IN_26_3: &[&str] = &[
    "minecraft:add_transient_block",
    "minecraft:post_effects",
    "minecraft:swing_animation",
    "minecraft:game_rule_values",
    "minecraft:low_disk_space_warning",
    "minecraft:debug/block_value",
    "minecraft:debug/chunk_value",
    "minecraft:debug/entity_value",
    "minecraft:debug/event",
    "minecraft:game_test_highlight_pos",
    "minecraft:code_of_conduct",
    "minecraft:waypoint",
    "minecraft:clear_dialog",
    "minecraft:show_dialog",
];

#[test]
fn every_26_3_packet_has_an_older_counterpart_or_a_replacement() {
    for version in paper_compat::VERSIONS.iter().filter(|v| v.family == paper_compat::Family::Modern) {
        let data = data::load(version.data);
        assert_eq!(data.protocol, version.protocol);
        for state in [State::Login, State::Configuration, State::Play] {
            let packets = data.packets(state);
            let names = match state {
                State::Login => paper_data::packets::login::clientbound::NAMES,
                State::Configuration => paper_data::packets::configuration::clientbound::NAMES,
                _ => paper_data::packets::play::clientbound::NAMES,
            };
            for (id, name) in names.iter().enumerate() {
                let old = packets.clientbound[id];
                assert!(old >= 0 || NEW_IN_26_3.contains(name), "{} has no {name} ({state:?})", version.name);
                if old >= 0 {
                    assert_eq!(&packets.clientbound_names[old as usize], name);
                }
            }
        }
    }
}

#[test]
fn ids_map_into_the_older_registries() {
    for version in paper_compat::VERSIONS.iter().filter(|v| v.family == paper_compat::Family::Modern) {
        let data = data::load(version.data);
        for key in ["blockstates", "blocks", "items", "entities", "sounds", "particles"] {
            let size = data.size(key) as i32;
            assert!(size > 0, "{} {key}", version.name);
            let mapping = data.mapping(key);
            let count = match key {
                "blockstates" => paper_data::blocks::STATE_COUNT as usize,
                "items" => paper_data::registries::entries("minecraft:item").unwrap().len(),
                "entities" => paper_data::registries::entries("minecraft:entity_type").unwrap().len(),
                _ => continue,
            };
            for id in 0..count as i32 {
                let old = mapping.old(id);
                assert!(old < size, "{} {key} {id} -> {old} (of {size})", version.name);
            }
        }
    }
}

#[test]
fn the_first_blocks_keep_their_ids() {
    // Stone and dirt have not moved since the flattening; bedrock has (26.1: 85).
    let data = data::load("26.1");
    let stone = paper_data::blocks::default_state("minecraft:stone").unwrap() as i32;
    assert_eq!(data.mapping("blockstates").old(stone), stone);
    let bedrock = paper_data::blocks::default_state("minecraft:bedrock").unwrap() as i32;
    assert_eq!(data.mapping("blockstates").old(bedrock), 85);
}
