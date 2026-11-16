//! What 1.7 shows for the blocks, items and entities 1.8 added: the nearest of its own (after
//! ViaRewind's choices where it makes them).

use std::sync::OnceLock;

/// A 1.8 block (`id << 4 | meta`) as 1.7's.
pub fn block(legacy: u16) -> u16 {
    static TABLE: OnceLock<Vec<u16>> = OnceLock::new();
    let table = TABLE.get_or_init(|| (0..4096u16).map(|b| convert(b >> 4, b & 15)).collect());
    table.get(legacy as usize).copied().unwrap_or(1 << 4)
}

fn convert(id: u16, meta: u16) -> u16 {
    let (id, meta) = match id {
        // Granite, diorite, andesite and their polished kinds; coarse dirt; wet sponges; the
        // stone brick kinds of monster eggs.
        1 | 19 => (id, 0),
        3 if meta == 1 => (3, 0),
        97 => (97, meta.min(2)),
        165 => (159, 5),
        166 => (20, 0),
        167 => (96, meta),
        168 => (159, 9),
        169 => (89, 0),
        176 | 177 => (0, 0),
        178 => (151, meta),
        179 => (24, meta.min(2)),
        180 => (128, meta),
        181 => (43, (meta & 8) | 1),
        182 => (44, (meta & 8) | 1),
        183..=187 => (107, meta),
        188..=192 => (85, 0),
        193..=197 => (64, meta),
        id if id > 175 => (1, 0),
        other => (other, meta),
    };
    id << 4 | meta
}

/// A 1.8 item (id, damage) as 1.7's.
pub fn item(id: i16, damage: i16) -> (i16, i16) {
    match id {
        1 | 19 => (id, 0),
        3 if damage == 1 => (3, 0),
        97 => (id, damage.clamp(0, 2)),
        165..=255 => {
            let b = block((id as u16) << 4 | damage.clamp(0, 15) as u16);
            ((b >> 4) as i16, (b & 15) as i16)
        }
        // Spawn eggs: the mob 1.7 shows instead.
        383 => (id, mob(damage.clamp(0, 255) as u8).map_or(90, |m| m as i16)),
        0..=164 | 256..=408 | 417..=422 | 2256..=2267 => (id, damage),
        409 => (318, 0),
        410 => (348, 0),
        411 => (365, 0),
        412 => (366, 0),
        413 => (282, 0),
        414 => (288, 0),
        415 => (334, 0),
        423 => (319, 0),
        424 => (320, 0),
        427..=431 => (324, 0),
        _ => (280, 0),
    }
}

/// A 1.8 mob type as 1.7's (`None`: the client is not told).
pub fn mob(kind: u8) -> Option<u8> {
    Some(match kind {
        // Endermites, guardians and rabbits.
        67 => 60,
        68 => 94,
        101 => 93,
        50..=66 | 90..=100 | 120 => kind,
        _ => return None,
    })
}

/// A 1.8 object type and its data as 1.7's (`None`: the client is not told).
pub fn object(kind: u8, data: i32) -> Option<(u8, i32)> {
    Some(match kind {
        // Falling blocks: 1.7 has the metadata 16 bits up, not 12.
        70 => {
            let b = block(((data & 0xFFF) as u16) << 4 | ((data >> 12) & 15) as u16);
            (70, (b >> 4) as i32 | ((b & 15) as i32) << 16)
        }
        // Armor stands.
        78 => return None,
        _ => (kind, data),
    })
}

/// 1.7's name of a 1.8 particle, with what follows its name for items and blocks.
pub fn particle(id: i32, data: &[i32]) -> Option<String> {
    const NAMES: [&str; 35] = [
        "explode", "largeexplode", "hugeexplosion", "fireworksSpark", "bubble", "splash", "bubble", "suspended", "depthsuspend", "crit",
        "magicCrit", "smoke", "largesmoke", "spell", "instantSpell", "mobSpell", "mobSpellAmbient", "witchMagic", "dripWater", "dripLava",
        "angryVillager", "happyVillager", "townaura", "note", "portal", "enchantmenttable", "flame", "lava", "footstep", "cloud", "reddust",
        "snowballpoof", "snowshovel", "slime", "heart",
    ];
    match id {
        0..=34 => Some(NAMES[id as usize].to_owned()),
        36 => {
            let (item, damage) = item(*data.first()? as i16, data.get(1).copied().unwrap_or(0) as i16);
            Some(format!("iconcrack_{item}_{damage}"))
        }
        37 | 38 => {
            let state = *data.first()?;
            let b = block(((state & 0xFFF) as u16) << 4 | ((state >> 12) & 15) as u16);
            Some(format!("blockcrack_{}_{}", b >> 4, b & 15))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_what_1_8_added_is_replaced() {
        assert_eq!(block(95 << 4 | 14), 95 << 4 | 14, "stained glass stays (1.7 has it)");
        assert_eq!(block(1 << 4 | 3), 1 << 4, "diorite is stone");
        assert_eq!(block(3 << 4 | 2), 3 << 4 | 2, "podzol stays");
        assert_eq!(block(188 << 4), 85 << 4, "spruce fences are fences");
        for b in 0..4096u16 {
            assert!(block(b) >> 4 <= 175, "{} {} became {}", b >> 4, b & 15, block(b) >> 4);
        }
        assert_eq!(item(420, 0), (420, 0), "leads stay");
        assert_eq!(item(416, 0), (280, 0), "armor stands do not");
        assert_eq!(mob(100), Some(100), "horses stay");
        assert_eq!(object(78, 0), None);
        assert_eq!(particle(37, &[1 | 2 << 12]).as_deref(), Some("blockcrack_1_0"));
    }
}
