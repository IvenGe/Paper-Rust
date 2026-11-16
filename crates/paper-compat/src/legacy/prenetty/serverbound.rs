//! What a 1.4.6 client sends, as 1.8 packets (for the 1.8 translator).

use super::wire::{self, read_string};
use super::PreNetty;
use crate::io::Result;
use crate::legacy::ids::sb;
use crate::legacy::pos18;
use paper_protocol::codec::{BlockPos, Reader, WriteExt};

/// A rider's keys from the motion 1.4.6 sends instead (forward, and sideways with left
/// positive), as 1.8 sends them.
fn steering(motion_x: f64, motion_z: f64, yaw: f32) -> (f32, f32) {
    let (sin, cos) = (yaw as f64).to_radians().sin_cos();
    let key = |v: f64| if v.abs() < 1e-3 { 0.0 } else { 0.98f32.copysign(v as f32) };
    (key(-motion_x * sin + motion_z * cos), key(motion_x * cos + motion_z * sin))
}

impl PreNetty {
    /// A 1.4.6 play packet as 1.8 payloads.
    pub(super) fn to_1_8(&mut self, packet: &[u8], out: &mut Vec<Vec<u8>>) -> Result<()> {
        let mut r = Reader::new(packet);
        let mut w = Vec::new();
        match r.u8()? {
            0x00 => {
                w.put_varint(sb::KEEP_ALIVE);
                w.put_varint(r.i32()?);
            }
            0x03 => {
                w.put_varint(sb::CHAT);
                w.put_string(&read_string(&mut r, 100)?);
            }
            0x07 => {
                let _user = r.i32()?;
                let target = r.i32()?;
                let attack = r.bool()?;
                w.put_varint(sb::USE_ENTITY);
                w.put_varint(target);
                w.put_varint(attack as i32);
            }
            0x0A => {
                w.put_varint(sb::FLYING);
                w.put_bool(r.bool()?);
            }
            0x0B => {
                // From the client: x, its feet, its eyes, z.
                let (x, y, _eyes, z) = (r.f64()?, r.f64()?, r.f64()?, r.f64()?);
                w.put_varint(sb::POSITION);
                w.put_f64(x);
                w.put_f64(y);
                w.put_f64(z);
                w.put_bool(r.bool()?);
            }
            0x0C => {
                w.put_varint(sb::LOOK);
                w.put_f32(r.f32()?);
                w.put_f32(r.f32()?);
                w.put_bool(r.bool()?);
            }
            0x0D => {
                let (x, y, eyes, z) = (r.f64()?, r.f64()?, r.f64()?, r.f64()?);
                let (yaw, pitch) = (r.f32()?, r.f32()?);
                let on_ground = r.bool()?;
                if y == -999.0 && eyes == -999.0 {
                    // A rider: 1.4.6 sends its motion where the position goes; 1.8 its look
                    // and its keys.
                    w.put_varint(sb::LOOK);
                    w.put_f32(yaw);
                    w.put_f32(pitch);
                    w.put_bool(on_ground);
                    out.push(std::mem::take(&mut w));
                    let (forward, sideways) = steering(x, z, yaw);
                    w.put_varint(sb::STEER_VEHICLE);
                    w.put_f32(sideways);
                    w.put_f32(forward);
                    w.put_u8(0);
                } else {
                    w.put_varint(sb::POSITION_LOOK);
                    w.put_f64(x);
                    w.put_f64(y);
                    w.put_f64(z);
                    w.put_f32(yaw);
                    w.put_f32(pitch);
                    w.put_bool(on_ground);
                }
            }
            0x0E => {
                let status = r.i8()?;
                let pos = BlockPos::new(r.i32()?, r.u8()? as i32, r.i32()?);
                let face = r.i8()?;
                w.put_varint(sb::BLOCK_DIG);
                w.put_i8(status);
                w.put_i64(pos18(pos));
                w.put_i8(face);
            }
            0x0F => {
                let pos = BlockPos::new(r.i32()?, r.u8()? as i32, r.i32()?);
                let face = r.i8()?;
                w.put_varint(sb::BLOCK_PLACE);
                w.put_i64(pos18(pos));
                w.put_i8(face);
                wire::slot_to_1_8(&mut r, &mut w)?;
                w.put_slice(r.take(3)?);
            }
            0x10 => {
                w.put_varint(sb::HELD_ITEM_SLOT);
                w.put_i16(r.i16()?);
            }
            0x12 => {
                r.i32()?;
                // Only swinging (1) is the client's to say.
                if r.i8()? != 1 {
                    return Ok(());
                }
                w.put_varint(sb::ARM_ANIMATION);
            }
            0x13 => {
                let entity = r.i32()?;
                // Sneak, unsneak, leave bed, sprint, stop: 1.8 counts from 0.
                let action = r.i8()?;
                if !(1..=5).contains(&action) {
                    return Ok(());
                }
                w.put_varint(sb::ENTITY_ACTION);
                w.put_varint(entity);
                w.put_varint(action as i32 - 1);
                w.put_varint(0);
            }
            0x65 => {
                w.put_varint(sb::CLOSE_WINDOW);
                w.put_i8(r.i8()?);
            }
            0x66 => {
                w.put_varint(sb::WINDOW_CLICK);
                w.put_i8(r.i8()?);
                w.put_i16(r.i16()?);
                w.put_i8(r.i8()?);
                w.put_i16(r.i16()?);
                // Shift held or not: 1.8's first two modes.
                w.put_i8((r.i8()? != 0) as i8);
                wire::slot_to_1_8(&mut r, &mut w)?;
            }
            0x6A => {
                w.put_varint(sb::TRANSACTION);
                w.put_slice(r.take(4)?);
            }
            0x6B => {
                w.put_varint(sb::SET_CREATIVE_SLOT);
                w.put_i16(r.i16()?);
                wire::slot_to_1_8(&mut r, &mut w)?;
            }
            0x6C => {
                w.put_varint(sb::ENCHANT_ITEM);
                w.put_slice(r.take(2)?);
            }
            0x82 => {
                let pos = BlockPos::new(r.i32()?, r.i16()? as i32, r.i32()?);
                w.put_varint(sb::UPDATE_SIGN);
                w.put_i64(pos18(pos));
                for _ in 0..4 {
                    let line = read_string(&mut r, 15)?;
                    w.put_string(&serde_json::json!({ "text": line }).to_string());
                }
            }
            0xCA => {
                w.put_varint(sb::ABILITIES);
                w.put_i8(r.i8()?);
                w.put_f32(r.u8()? as f32 / 255.0);
                w.put_f32(r.u8()? as f32 / 255.0);
            }
            0xCB => {
                w.put_varint(sb::TAB_COMPLETE);
                w.put_string(&read_string(&mut r, 32767)?);
                w.put_bool(false);
            }
            0xCC => {
                let locale = read_string(&mut r, 16)?;
                // Far, normal, short, tiny.
                let view = match r.i8()? {
                    0 => 16,
                    1 => 8,
                    2 => 4,
                    _ => 2,
                };
                let chat = r.i8()?;
                w.put_varint(sb::SETTINGS);
                w.put_string(&locale);
                w.put_i8(view);
                w.put_i8((chat & 7).min(2));
                w.put_bool(chat & 8 != 0);
                w.put_u8(0x7F);
            }
            0xCD => {
                // Respawning (1); the initial spawn (0) was the login.
                if r.i8()? != 1 {
                    return Ok(());
                }
                w.put_varint(sb::CLIENT_COMMAND);
                w.put_varint(0);
            }
            // Plugin channels, a disconnect (the socket closes next), handshake leftovers.
            _ => return Ok(()),
        }
        out.push(w);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn riders_steer_the_way_they_move() {
        // Facing south (yaw 0) and moving south: forward.
        assert_eq!(steering(0.0, 0.2, 0.0), (0.98, 0.0));
        // Facing south, moving east (+x): to its left, which 1.8 counts positive.
        assert_eq!(steering(0.2, 0.0, 0.0), (0.0, 0.98));
        assert_eq!(steering(0.0, 0.0, 90.0), (0.0, 0.0));
    }
}
