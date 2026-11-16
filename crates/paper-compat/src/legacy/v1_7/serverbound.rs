//! What a 1.7 client sends, as 1.8 packets (for the 1.8 translator). The ids are the same.

use super::Legacy17;
use crate::io::Result;
use crate::legacy::ids::sb;
use crate::legacy::pos18;
use crate::legacy::prenetty::wire;
use paper_protocol::codec::{BlockPos, Reader, WriteExt};

impl Legacy17 {
    /// A 1.7 play packet as 1.8 payloads.
    pub(super) fn to_1_8(&mut self, packet: &[u8], out: &mut Vec<Vec<u8>>) -> Result<()> {
        let mut r = Reader::new(packet);
        let id = r.varint()?;
        let mut w = Vec::new();
        w.put_varint(id);
        match id {
            sb::KEEP_ALIVE => w.put_varint(r.i32()?),
            sb::USE_ENTITY => {
                w.put_varint(r.i32()?);
                // 0: interact, 1: attack, as 1.8's first two.
                w.put_varint(r.i8()? as i32);
            }
            sb::POSITION | sb::POSITION_LOOK => {
                // From the client: x, its feet, its eyes, z.
                let (x, y, _eyes, z) = (r.f64()?, r.f64()?, r.f64()?, r.f64()?);
                if y == -999.0 {
                    // A rider (its keys come apart): only where it looks.
                    if id == sb::POSITION {
                        return Ok(());
                    }
                    let mut look = Vec::new();
                    look.put_varint(sb::LOOK);
                    look.put_slice(r.rest());
                    out.push(look);
                    return Ok(());
                }
                w.put_f64(x);
                w.put_f64(y);
                w.put_f64(z);
                w.put_slice(r.rest());
            }
            sb::BLOCK_DIG => {
                w.put_i8(r.i8()?);
                w.put_i64(pos18(BlockPos::new(r.i32()?, r.u8()? as i32, r.i32()?)));
                w.put_i8(r.i8()?);
            }
            sb::BLOCK_PLACE => {
                w.put_i64(pos18(BlockPos::new(r.i32()?, r.u8()? as i32, r.i32()?)));
                w.put_i8(r.i8()?);
                wire::slot_to_1_8(&mut r, &mut w)?;
                w.put_slice(r.take(3)?);
            }
            sb::ARM_ANIMATION => {
                r.i32()?;
                if r.i8()? != 1 {
                    return Ok(());
                }
            }
            sb::ENTITY_ACTION => {
                w.put_varint(r.i32()?);
                // Sneak, unsneak, leave bed, sprint, stop, horse jump, horse inventory: 1.8
                // counts from 0.
                let action = r.i8()?;
                if !(1..=7).contains(&action) {
                    return Ok(());
                }
                w.put_varint(action as i32 - 1);
                w.put_varint(r.i32()?);
            }
            sb::STEER_VEHICLE => {
                w.put_slice(r.take(8)?);
                let (jump, unmount) = (r.bool()?, r.bool()?);
                w.put_u8(jump as u8 | (unmount as u8) << 1);
            }
            sb::WINDOW_CLICK => {
                w.put_slice(r.take(7)?);
                wire::slot_to_1_8(&mut r, &mut w)?;
            }
            sb::SET_CREATIVE_SLOT => {
                w.put_i16(r.i16()?);
                wire::slot_to_1_8(&mut r, &mut w)?;
            }
            sb::UPDATE_SIGN => {
                w.put_i64(pos18(BlockPos::new(r.i32()?, r.i16()? as i32, r.i32()?)));
                for _ in 0..4 {
                    let line = r.string(15)?;
                    w.put_string(&serde_json::json!({ "text": line }).to_string());
                }
            }
            sb::TAB_COMPLETE => {
                w.put_string(&r.string(32767)?);
                w.put_bool(false);
            }
            sb::SETTINGS => {
                w.put_string(&r.string(16)?);
                w.put_i8(r.i8()?);
                w.put_i8(r.i8()? & 3);
                w.put_bool(r.bool()?);
                // Difficulty and the cape: 1.8 has the skin's parts instead.
                w.put_u8(0x7F);
            }
            sb::CLIENT_COMMAND => w.put_varint(r.i8()? as i32),
            sb::CUSTOM_PAYLOAD => {
                let channel = r.string(20)?;
                let data = wire::read_array(&mut r)?;
                w.put_string(&channel);
                if channel == "MC|Brand" {
                    // 1.7 sends the brand's bytes, 1.8 a string.
                    w.put_string(&String::from_utf8_lossy(data));
                } else {
                    w.put_slice(data);
                }
            }
            // The same in 1.8: chat, on the ground or not, looks, held items, closing windows,
            // transactions, enchanting, abilities.
            sb::CHAT | sb::FLYING | sb::LOOK | sb::HELD_ITEM_SLOT | sb::CLOSE_WINDOW | sb::TRANSACTION | sb::ENCHANT_ITEM | sb::ABILITIES => {
                w.put_slice(r.rest())
            }
            _ => return Ok(()),
        }
        out.push(w);
        Ok(())
    }
}
