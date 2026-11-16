//! What clients from before Netty show for the blocks, items and entities that came after
//! them: the nearest of their own, in look and, where it matters to the client (how long
//! breaking takes, what it can stand on), in kind.

use super::Era;
use std::sync::OnceLock;

/// A 1.8 block (`id << 4 | meta`) as `era`'s.
pub fn block(era: Era, legacy: u16) -> u16 {
    static TABLES: [OnceLock<Vec<u16>>; 3] = [OnceLock::new(), OnceLock::new(), OnceLock::new()];
    let table = TABLES[era as usize].get_or_init(|| (0..4096u16).map(|b| convert(era, b >> 4, b & 15)).collect());
    table.get(legacy as usize).copied().unwrap_or(1 << 4)
}

/// 1.4.6's blocks (the chunk conversion takes a plain function).
pub fn block_1_4(legacy: u16) -> u16 {
    block(Era::V1_4, legacy)
}

pub fn block_1_5(legacy: u16) -> u16 {
    block(Era::V1_5, legacy)
}

pub fn block_1_6(legacy: u16) -> u16 {
    block(Era::V1_6, legacy)
}

/// Dispensers face only sideways before 1.5, and have no "triggered" bit.
fn dispenser(era: Era, meta: u16) -> u16 {
    if era != Era::V1_4 {
        return meta;
    }
    match meta & 7 {
        0 | 1 => 3,
        facing => facing,
    }
}

/// Stone slabs: 1.4.6 has stone, sandstone, wooden, cobblestone, brick and stone brick ones;
/// 1.5 added nether brick and quartz.
fn stone_slab(era: Era, meta: u16) -> u16 {
    let kind = match meta & 7 {
        6 if era == Era::V1_4 => 4,
        7 if era == Era::V1_4 => 1,
        kind => kind,
    };
    (meta & 8) | kind
}

fn convert(era: Era, id: u16, meta: u16) -> u16 {
    let (id, meta) = match id {
        // Variants 1.7 and 1.8 gave blocks these versions have.
        1 | 3 | 12 | 19 | 38 => (id, 0),
        5 => (5, meta & 3),
        6 => (6, (meta & 8) | (meta & 3)),
        23 | 158 if era == Era::V1_4 => (23, dispenser(era, meta)),
        43 | 44 => (id, stone_slab(era, meta)),
        97 => (97, meta.min(2)),
        125 | 126 => (id, (meta & 8) | (meta & 3)),
        // 1.5.
        146..=158 if era == Era::V1_4 => match id {
            146 => (54, meta),
            147 | 148 => (70, (meta > 0) as u16),
            149 => (93, meta & 3),
            150 => (94, meta & 3),
            151 => (44, 0),
            152 => (35, 14),
            153 => (87, 0),
            154 => (118, 0),
            155 => (24, 2),
            156 => (128, meta),
            _ => (28, meta),
        },
        // 1.6.
        159 | 170..=173 if era < Era::V1_6 => match id {
            159 => (35, meta),
            170 => (35, 4),
            171 => (78, 0),
            172 => (45, 0),
            _ => (35, 15),
        },
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
        178 if era == Era::V1_4 => (44, 0),
        178 => (151, meta),
        179 => (24, meta.min(2)),
        180 => (128, meta),
        181 => (43, (meta & 8) | 1),
        182 => (44, (meta & 8) | 1),
        183..=187 => (107, meta),
        188..=192 => (85, 0),
        193..=197 => (64, meta),
        id if id > era.last_block() => (1, 0),
        other => (other, meta),
    };
    id << 4 | meta
}

/// A 1.8 item (id, damage) as `era`'s.
pub fn item(era: Era, id: i16, damage: i16) -> (i16, i16) {
    match id {
        // Blocks: their variants as the blocks', what came later as blocks.
        1 | 3 | 12 | 19 | 38 => (id, 0),
        5 | 6 | 125 | 126 => (id, damage & 3),
        43 | 44 => (id, stone_slab(era, damage.clamp(0, 15) as u16) as i16 & 7),
        97 => (id, damage.clamp(0, 2)),
        95 | 146..=255 => {
            let b = block(era, (id as u16) << 4 | damage.clamp(0, 15) as u16);
            let (new, meta) = ((b >> 4) as i16, (b & 15) as i16);
            // The same block keeps the item's damage (its kind, not a direction).
            if new == id {
                (id, damage)
            } else {
                (new, meta)
            }
        }
        0..=145 => (id, damage),
        // Fish: these versions have only the plain one.
        349 | 350 => (id, 0),
        // Spawn eggs: the mob these versions show instead.
        383 => (id, mob(era, damage.clamp(0, 255) as u8).map_or(90, |m| m as i16)),
        256..=403 | 2256..=2267 => (id, damage),
        404..=408 if era >= Era::V1_5 => (id, damage),
        417..=421 if era >= Era::V1_6 => (id, damage),
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

/// A 1.8 mob type as `era`'s (`None`: the client is not told).
pub fn mob(era: Era, kind: u8) -> Option<u8> {
    Some(match kind {
        // Endermites, guardians, horses (before 1.6) and rabbits.
        67 => 60,
        68 => 94,
        100 if era < Era::V1_6 => 92,
        101 => 93,
        50..=66 | 90..=100 | 120 => kind,
        _ => return None,
    })
}

/// A 1.8 object type and its data as `era`'s (`None`: the client is not told).
pub fn object(era: Era, kind: u8, data: i32) -> Option<(u8, i32)> {
    Some(match kind {
        // Minecarts were three types (plain, chest, furnace) in 1.4.6; 1.5 made them one with
        // the kind as data (command block minecarts came in 1.7).
        10 if era == Era::V1_4 => (
            match data {
                1 | 5 => 11,
                2 => 12,
                _ => 10,
            },
            0,
        ),
        10 => (10, if data > 5 { 0 } else { data }),
        // Falling blocks: the metadata 16 bits up, not 12.
        70 => {
            let b = block(era, ((data & 0xFFF) as u16) << 4 | ((data >> 12) & 15) as u16);
            (70, (b >> 4) as i32 | ((b & 15) as i32) << 16)
        }
        77 if era >= Era::V1_6 => (77, data),
        1 | 2 | 50 | 51 | 60..=66 | 71..=76 | 90 => (kind, data),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn later_blocks_become_ones_1_4_6_has() {
        let block = block_1_4;
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
    fn each_version_keeps_what_it_has() {
        assert_eq!(block_1_5(146 << 4 | 3), 146 << 4 | 3, "1.5 has trapped chests");
        assert_eq!(block_1_5(170 << 4), 35 << 4 | 4, "but not hay bales");
        assert_eq!(block_1_5(44 << 4 | 15), 44 << 4 | 15, "and quartz slabs");
        assert_eq!(block_1_6(170 << 4), 170 << 4, "1.6 has hay bales");
        assert_eq!(block_1_6(95 << 4 | 3), 20 << 4, "but not stained glass");
        for (b, last) in [(block_1_5 as fn(u16) -> u16, 158), (block_1_6, 173)] {
            for legacy in 0..4096u16 {
                assert!(b(legacy) >> 4 <= last, "{} {} became {}", legacy >> 4, legacy & 15, b(legacy) >> 4);
            }
        }
        assert_eq!(item(Era::V1_6, 421, 0), (421, 0), "1.6 has name tags");
        assert_eq!(item(Era::V1_5, 421, 0), (339, 0), "1.5 shows paper");
        assert_eq!(mob(Era::V1_6, 100), Some(100));
        assert_eq!(object(Era::V1_5, 10, 3), Some((10, 3)), "a TNT minecart");
    }

    #[test]
    fn later_items_become_ones_1_4_6_has() {
        assert_eq!(item(Era::V1_4, 421, 0), (339, 0), "name tags are paper");
        assert_eq!(item(Era::V1_4, 276, 7), (276, 7), "a worn diamond sword stays");
        assert_eq!(item(Era::V1_4, 162, 1), (17, 1));
        assert_eq!(item(Era::V1_4, 23, 0), (23, 0), "a dispenser item keeps its damage");
        assert_eq!(item(Era::V1_4, 383, 101), (383, 93), "a rabbit egg spawns chickens");
        assert_eq!(mob(Era::V1_4, 100), Some(92));
        assert_eq!(object(Era::V1_4, 10, 1), Some((11, 0)));
        assert_eq!(object(Era::V1_4, 70, 12 | 1 << 12), Some((70, 12)), "red sand falls as sand");
        assert_eq!(object(Era::V1_4, 70, 35 | 14 << 12), Some((70, 35 | 14 << 16)), "red wool keeps its colour, 16 bits up");
    }
}
