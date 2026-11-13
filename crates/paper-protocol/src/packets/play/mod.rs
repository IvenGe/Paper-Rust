//! Play state.

use super::common::define_common_packets;

define_common_packets!(play);

mod clientbound;
mod serverbound;

pub use clientbound::*;
pub use serverbound::*;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codec::{BlockPos, Decode, Encode, Reader, VarInt};
    use crate::packets::{encode_packet, Packet};
    use crate::text::Component;
    use crate::types::{Angle, LpVec3, Slot};
    use uuid::Uuid;

    fn round<P: Packet + Encode + Decode + PartialEq + std::fmt::Debug>(p: P) {
        let payload = encode_packet(&p);
        let mut r = Reader::new(&payload);
        assert_eq!(r.varint().unwrap(), P::ID, "{}", P::NAME);
        let back = P::decode(&mut r).unwrap_or_else(|e| panic!("{}: {e}", P::NAME));
        r.finish().unwrap_or_else(|e| panic!("{}: {e}", P::NAME));
        assert_eq!(back, p, "{}", P::NAME);
    }

    #[test]
    fn clientbound_round_trips() {
        round(Login {
            entity_id: 7,
            hardcore: false,
            dimension_names: vec!["minecraft:overworld".into()],
            max_players: VarInt(20),
            view_distance: VarInt(10),
            simulation_distance: VarInt(10),
            reduced_debug_info: false,
            enable_respawn_screen: true,
            limited_crafting: false,
            spawn: SpawnInfo {
                dimension_type: 0,
                dimension_name: "minecraft:overworld".into(),
                hashed_seed: -5,
                game_mode: 1,
                previous_game_mode: None,
                is_debug: false,
                is_flat: true,
                death_location: Some(("minecraft:the_nether".into(), BlockPos::new(1, -2, 3))),
                portal_cooldown: 0,
                sea_level: 63,
            },
            online_mode: false,
            enforces_secure_chat: false,
        });
        round(AddEntity {
            entity_id: VarInt(9),
            uuid: Uuid::from_u128(5),
            entity_type: VarInt(100),
            x: 1.5,
            y: 64.0,
            z: -3.25,
            velocity: LpVec3(0.0, 0.0, 0.0),
            pitch: Angle(0.0),
            yaw: Angle(90.0),
            head_yaw: Angle(180.0),
            data: VarInt(0),
        });
        round(EntityPositionSync { entity_id: 3, x: 1.0, y: 2.0, z: 3.0, yaw: 45.0, pitch: -10.0, on_ground: true });
        round(MoveEntityPos { entity_id: 3, dx: 100, dy: -200, dz: 4096, on_ground: true });
        round(MoveEntityPosRot { entity_id: 3, dx: 1, dy: 2, dz: 3, yaw: 90.0, pitch: 0.0, on_ground: false });
        round(SectionBlocksUpdate { section_x: -3, section_y: -4, section_z: 1_000_000, blocks: vec![(0, 15, 7, 12345), (15, 0, 1, 0)] });
        round(ForgetLevelChunk { x: -1, z: 2 });
        round(BlockEvent { pos: BlockPos::new(1, -60, -3), action: 0, param: 4, block: VarInt(118) });
        let mut root = CommandNode::root();
        root.children = vec![1, 3];
        let mut tp = CommandNode::literal("tp").executable();
        tp.children = vec![2];
        let mut give = CommandNode::literal("give");
        give.children = vec![4];
        let mut count = CommandNode::argument("count", Parser::integer(Some(1), None)).executable();
        count.children = vec![5];
        round(Commands {
            nodes: vec![
                root,
                tp,
                CommandNode::argument("args", Parser::greedy_string()).executable().ask_server(),
                give,
                count,
                CommandNode::argument("target", Parser::entity(true, true)),
                CommandNode::argument("d", Parser::double(Some(-1.0), Some(1.0))),
                CommandNode::argument("item", Parser::registry("minecraft:resource", "minecraft:item")),
                CommandNode::argument("p", Parser::simple("minecraft:game_profile")),
                CommandNode { redirect: Some(1), ..CommandNode::literal("teleport") },
            ],
            root_index: 0,
        });
        round(PlayerInfoUpdate {
            actions: player_info::ADD_PLAYER | player_info::UPDATE_GAME_MODE | player_info::UPDATE_LISTED | player_info::UPDATE_LATENCY | player_info::UPDATE_DISPLAY_NAME,
            entries: vec![PlayerInfoEntry {
                uuid: Uuid::from_u128(1),
                name: "Steve".into(),
                properties: vec![],
                game_mode: 1,
                listed: true,
                latency: 42,
                display_name: Some(Component::text("Steve").color("red")),
                list_priority: 0,
                show_hat: false,
            }],
        });
        round(SystemChat { content: Component::text("hello").bold(true), overlay: false });
        round(DisguisedChat { message: Component::text("m"), chat_type: 0, sender_name: Component::text("s"), target_name: None });
        round(SetEquipment { entity_id: 1, equipment: vec![(0, Slot::of(5, 1)), (5, Slot::EMPTY)] });
        round(SetTime { world_age: 100, clocks: vec![ClockState { clock: 0, time: 6000, fraction: 0.0, rate: 1.0 }] });
        round(Sound {
            sound: SoundRef::Named { name: "minecraft:entity.player.levelup".into(), fixed_range: None },
            category: 0,
            x: 1.5,
            y: 2.0,
            z: -3.5,
            volume: 1.0,
            pitch: 1.0,
            seed: 9,
        });
        round(CommandSuggestions { transaction_id: 1, start: 5, length: 2, matches: vec![("abc".into(), None), ("abd".into(), Some(Component::text("tip")))] });
        round(KeepAlive { id: 123456789 });
        round(Disconnect { reason: Component::translatable("multiplayer.disconnect.kicked", vec![]) });
    }

    #[test]
    fn section_position_packing_matches_spec() {
        // ((sectionX & 0x3FFFFF) << 42) | (sectionY & 0xFFFFF) | ((sectionZ & 0x3FFFFF) << 20)
        let p = SectionBlocksUpdate { section_x: 1, section_y: 2, section_z: 3, blocks: vec![(1, 2, 3, 5)] };
        let mut w = Vec::new();
        p.encode(&mut w);
        let expected: i64 = (1i64 << 42) | 2 | (3i64 << 20);
        assert_eq!(w[..8], expected.to_be_bytes());
        // blockStateId << 12 | (x << 8 | z << 4 | y) = 5 << 12 | 0x132 = 20786 -> varlong b2 a2 01
        assert_eq!(w[8..], [1, 0xb2, 0xa2, 0x01]);
    }

    #[test]
    fn serverbound_decoding() {
        let cases: Vec<(Vec<u8>, Serverbound)> = vec![
            (
                encode_packet(&AcceptTeleportation { teleport_id: VarInt(4), x: 1.0, y: 2.0, z: 3.0, yaw: 0.0, pitch: 0.0 }),
                Serverbound::AcceptTeleportation(AcceptTeleportation { teleport_id: VarInt(4), x: 1.0, y: 2.0, z: 3.0, yaw: 0.0, pitch: 0.0 }),
            ),
            (
                encode_packet(&Chat { message: "hi".into(), timestamp: 1, salt: 2, signed: false }),
                Serverbound::Chat(Chat { message: "hi".into(), timestamp: 1, salt: 2, signed: false }),
            ),
            (encode_packet(&ChatCommand { command: "say hi".into() }), Serverbound::ChatCommand(ChatCommand { command: "say hi".into() })),
            (encode_packet(&ClientTickEnd {}), Serverbound::ClientTickEnd),
            (
                encode_packet(&MovePlayerPosRot { x: 0.5, y: 64.0, z: 0.5, yaw: 1.0, pitch: 2.0, flags: 1 }),
                Serverbound::MovePlayerPosRot(MovePlayerPosRot { x: 0.5, y: 64.0, z: 0.5, yaw: 1.0, pitch: 2.0, flags: 1 }),
            ),
            (
                encode_packet(&PlayerAction { status: VarInt(0), pos: BlockPos::new(1, 2, 3), face: 1, sequence: VarInt(5) }),
                Serverbound::PlayerAction(PlayerAction { status: VarInt(0), pos: BlockPos::new(1, 2, 3), face: 1, sequence: VarInt(5) }),
            ),
            (
                encode_packet(&SetCreativeModeSlot { slot: 36, item: Slot::of(1, 64) }),
                Serverbound::SetCreativeModeSlot(SetCreativeModeSlot { slot: 36, item: Slot::of(1, 64) }),
            ),
        ];
        for (payload, expected) in cases {
            assert_eq!(Serverbound::decode(&payload).unwrap(), expected);
        }
        // A valid id we do not act on is reported as Other, not as an error.
        let id = paper_data::packets::play::serverbound::EDIT_BOOK;
        assert_eq!(Serverbound::decode(&[id as u8, 1, 2, 3]).unwrap(), Serverbound::Other { id });
        // Ids outside the table are errors.
        assert!(Serverbound::decode(&[0x7F]).is_err());
        // Trailing garbage after a known packet is an error.
        let mut bad = encode_packet(&ClientTickEnd {});
        bad.push(0);
        assert!(Serverbound::decode(&bad).is_err());
    }
}
