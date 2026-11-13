//! Entity metadata: a list of `(index, type, value)` entries terminated by `0xFF`.
//!
//! Type ids are the registration order of vanilla's entity data serializers, taken from the
//! data tables by name.

use crate::codec::WriteExt;
use crate::text::Component;

/// Metadata indices shared by every entity.
pub mod index {
    /// Byte bit field, see [`super::flags`].
    pub const FLAGS: u8 = 0;
    pub const AIR_TICKS: u8 = 1;
    pub const CUSTOM_NAME: u8 = 2;
    pub const CUSTOM_NAME_VISIBLE: u8 = 3;
    pub const SILENT: u8 = 4;
    pub const NO_GRAVITY: u8 = 5;
    pub const POSE: u8 = 6;
    pub const FROZEN_TICKS: u8 = 7;
    // Living entity
    pub const HAND_STATES: u8 = 8;
    pub const HEALTH: u8 = 9;
    // Avatar (players and mannequins)
    pub const MAIN_HAND: u8 = 15;
    pub const SKIN_PARTS: u8 = 16;
}

/// Bits of the shared flags byte.
pub mod flags {
    pub const ON_FIRE: i8 = 0x01;
    pub const SNEAKING: i8 = 0x02;
    pub const SPRINTING: i8 = 0x08;
    pub const SWIMMING: i8 = 0x10;
    pub const INVISIBLE: i8 = 0x20;
    pub const GLOWING: i8 = 0x40;
    pub const FALL_FLYING: i8 = -0x80;
}

/// Entity poses.
pub mod pose {
    pub const STANDING: i32 = 0;
    pub const FALL_FLYING: i32 = 1;
    pub const SLEEPING: i32 = 2;
    pub const SWIMMING: i32 = 3;
    pub const SPIN_ATTACK: i32 = 4;
    pub const SNEAKING: i32 = 5;
    pub const DYING: i32 = 7;
}

fn type_id(name: &str) -> i32 {
    paper_data::registries::id("paper-rs:entity_data_serializer", name).unwrap_or_else(|| panic!("unknown entity data serializer {name}")) as i32
}

/// Builder for the metadata list of a `SetEntityData` packet.
#[derive(Clone, Debug, Default)]
pub struct Metadata(Vec<u8>);

impl Metadata {
    pub fn new() -> Self {
        Self::default()
    }

    fn entry(mut self, index: u8, ty: &str) -> Self {
        self.0.put_u8(index);
        self.0.put_varint(type_id(ty));
        self
    }

    pub fn byte(self, index: u8, value: i8) -> Self {
        let mut m = self.entry(index, "byte");
        m.0.put_i8(value);
        m
    }

    pub fn varint(self, index: u8, value: i32) -> Self {
        let mut m = self.entry(index, "int");
        m.0.put_varint(value);
        m
    }

    pub fn float(self, index: u8, value: f32) -> Self {
        let mut m = self.entry(index, "float");
        m.0.put_f32(value);
        m
    }

    pub fn boolean(self, index: u8, value: bool) -> Self {
        let mut m = self.entry(index, "boolean");
        m.0.put_bool(value);
        m
    }

    pub fn pose(self, index: u8, pose: i32) -> Self {
        let mut m = self.entry(index, "pose");
        m.0.put_varint(pose);
        m
    }

    pub fn optional_component(self, index: u8, value: Option<&Component>) -> Self {
        let mut m = self.entry(index, "optional_component");
        m.0.put_option(value, |w, c| w.put_component(c));
        m
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The encoded list including its terminator.
    pub fn finish(mut self) -> Vec<u8> {
        self.0.put_u8(0xFF);
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_entries_and_terminator() {
        let bytes = Metadata::new().byte(index::FLAGS, flags::SNEAKING).pose(index::POSE, pose::SNEAKING).finish();
        // index 0, type byte(0), value 2; index 6, type pose(20), value 5; terminator
        assert_eq!(bytes, [0, 0, 2, 6, 20, 5, 0xFF]);
        assert_eq!(Metadata::new().finish(), [0xFF]);
    }
}
