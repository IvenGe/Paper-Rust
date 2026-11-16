//! What 1.4.6 shows for the blocks, items and entities 1.5 to 1.8 added: the nearest of its
//! own, in look and, where it matters to the client (how long breaking takes, what it can
//! stand on), in kind.

use std::sync::OnceLock;

/// A 1.8 block (`id << 4 | meta`) as 1.4.6's.
pub fn block(legacy: u16) -> u16 {
    static TABLE: OnceLock<Vec<u16>> = OnceLock::new();
    let table = TABLE.get_or_init(|| (0..4096u16).map(|b| convert(b >> 4, b & 15)).collect());
    table.get(legacy as usize).copied().unwrap_or(1 << 4)
}

/// Dispensers face only sideways in 1.4.6.
fn dispenser(meta: u16) -> u16 {
    match meta & 7 {
        0 | 1 => 3,
        facing => facing,
    }
}

/// Stone slabs: 1.4.6 has stone, sandstone, wooden, cobblestone, brick and stone brick ones.
fn stone_slab(meta: u16) -> u16 {
    let kind = match meta & 7 {
        6 => 4,
        7 => 1,
        kind => kind,
    };
    (meta & 8) | kind
}

fn convert(id: u16, meta: u16) -> u16 {
    let (id, meta) = match id {
        // Variants later versions gave blocks 1.4.6 has.
        1 | 3 | 12 | 19 | 38 => (id, 0),
        5 => (5, meta & 3),
        6 => (6, (meta & 8) | (meta & 3)),
        23 => (23, dispenser(meta)),
        43 | 44 => (id, stone_slab(meta)),
        97 => (97, meta.min(2)),
        125 | 126 => (id, (meta & 8) | (meta & 3)),
        // 1.5.
        146 => (54, meta),
        147 | 148 => (70, (meta > 0) as u16),
        149 => (93, meta & 3),
        150 => (94, meta & 3),
        151 | 178 => (44, 0),
        152 => (35, 14),
        153 => (87, 0),
        154 => (118, 0),
        155 => (24, 2),
        156 => (128, meta),
        157 => (28, meta),
        158 => (23, dispenser(meta)),
        // 1.6.
        159 => (35, meta),
        170 => (35, 4),
        171 => (78, 0),
        172 => (45, 0),
        173 => (35, 15),
        // 1.7.
        95 => (20, 0),
        160 => (102, 0),
        161 => (18, (meta & 12) | (meta & 1)),
        162 => (17, (meta & 12) | (meta & 1)),
        163 | 164 => (53, meta),
        174 => (79, 0),
        175 => (31, 1),
        // 1.8.
        165 => (35, 5),
        166 => (20, 0),
        167 => (96, meta),
        168 => (98, 0),
        169 => (89, 0),
        176 | 177 => (0, 0),
        179 => (24, meta.min(2)),
        180 => (128, meta),
        181 => (43, (meta & 8) | 1),
        182 => (44, (meta & 8) | 1),
        183..=187 => (107, meta),
        188..=192 => (85, 0),
        193..=197 => (64, meta),
        id if id > 145 => (1, 0),
        other => (other, meta),
    };
    id << 4 | meta
}

/// A 1.8 item (id, damage) as 1.4.6's.
pub fn item(id: i16, damage: i16) -> (i16, i16) {
    match id {
        // Blocks: their variants as the blocks', what 1.5 and later added as blocks.
        1 | 3 | 12 | 19 | 38 => (id, 0),
        5 | 6 | 125 | 126 => (id, damage & 3),
        43 | 44 => (id, stone_slab(damage.clamp(0, 15) as u16) as i16 & 7),
        97 => (id, damage.clamp(0, 2)),
        95 | 146..=255 => {
            let b = block((id as u16) << 4 | damage.clamp(0, 15) as u16);
            ((b >> 4) as i16, (b & 15) as i16)
        }
        0..=145 => (id, damage),
        // Fish: 1.4.6 has only the plain one.
        349 | 350 => (id, 0),
        // Spawn eggs: the mob 1.4.6 shows instead.
        383 => (id, mob(damage.clamp(0, 255) as u8).map_or(90, |m| m as i16)),
        256..=403 | 2256..=2267 => (id, damage),
        // 1.5.
        404 => (356, 0),
        405 => (336, 0),
        406 => (337, 0),
        407 | 408 | 422 => (328, 0),
        // 1.6 to 1.8.
        409 => (318, 0),
        410 => (348, 0),
        411 => (365, 0),
        412 => (366, 0),
        413 => (282, 0),
        414 => (288, 0),
        415 => (334, 0),
        417 => (265, 0),
        418 => (266, 0),
        419 => (264, 0),
        420 => (287, 0),
        421 => (339, 0),
        423 => (319, 0),
        424 => (320, 0),
        427..=431 => (324, 0),
        _ => (280, 0),
    }
}

/// A 1.8 mob type as 1.4.6's (`None`: the client is not told).
pub fn mob(kind: u8) -> Option<u8> {
    Some(match kind {
        // Endermites, guardians, horses and rabbits.
        67 => 60,
        68 => 94,
        100 => 92,
        101 => 93,
        50..=66 | 90..=99 | 120 => kind,
        _ => return None,
    })
}

/// A 1.8 object type and its data as 1.4.6's (`None`: the client is not told).
pub fn object(kind: u8, data: i32) -> Option<(u8, i32)> {
    Some(match kind {
        // Minecarts were three types (plain, chest, furnace); 1.5 made them one with data.
        10 => (
            match data {
                1 | 5 => 11,
                2 => 12,
                _ => 10,
            },
            0,
        ),
        // Falling blocks: 1.4.6 has the metadata 16 bits up, not 12.
        70 => {
            let b = block(((data & 0xFFF) as u16) << 4 | ((data >> 12) & 15) as u16);
            (70, (b >> 4) as i32 | ((b & 15) as i32) << 16)
        }
        1 | 2 | 50 | 51 | 60..=66 | 71..=76 | 90 => (kind, data),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn later_blocks_become_ones_1_4_6_has() {
        assert_eq!(block(1 << 4 | 1), 1 << 4, "granite is stone");
        assert_eq!(block(146 << 4 | 3), 54 << 4 | 3, "a trapped chest is a chest, facing the same way");
        assert_eq!(block(159 << 4 | 14), 35 << 4 | 14, "red stained clay is red wool");
        assert_eq!(block(162 << 4 | 5), 17 << 4 | 5, "a dark oak log on its side is a spruce one");
        assert_eq!(block(44 << 4 | 15), 44 << 4 | 9, "a top quartz slab is a top sandstone one");
        assert_eq!(block(57 << 4), 57 << 4, "diamond blocks stay");
        for b in 0..4096u16 {
            assert!(block(b) >> 4 <= 145, "{} {} became {}", b >> 4, b & 15, block(b) >> 4);
        }
    }

    #[test]
    fn later_items_become_ones_1_4_6_has() {
        assert_eq!(item(421, 0), (339, 0), "name tags are paper");
        assert_eq!(item(276, 7), (276, 7), "a worn diamond sword stays");
        assert_eq!(item(162, 1), (17, 1));
        assert_eq!(item(383, 101), (383, 93), "a rabbit egg spawns chickens");
        assert_eq!(mob(100), Some(92));
        assert_eq!(object(10, 1), Some((11, 0)));
        assert_eq!(object(70, 12 | 1 << 12), Some((70, 12)), "red sand falls as sand");
        assert_eq!(object(70, 35 | 14 << 12), Some((70, 35 | 14 << 16)), "red wool keeps its colour, 16 bits up");
    }
}
