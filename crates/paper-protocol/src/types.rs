//! Composite wire types shared by several packets.

use crate::codec::{Decode, DecodeError, Encode, Reader, Result, WriteExt};
use uuid::Uuid;

/// A signed or unsigned property of a game profile (in practice: `textures`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Property {
    pub name: String,
    pub value: String,
    pub signature: Option<String>,
}

impl Encode for Property {
    fn encode(&self, w: &mut Vec<u8>) {
        w.put_string(&self.name);
        w.put_string(&self.value);
        self.signature.encode(w);
    }
}

impl Decode for Property {
    fn decode(r: &mut Reader<'_>) -> Result<Self> {
        Ok(Self { name: r.string(64)?, value: r.string(32767)?, signature: r.option(|r| r.string(1024))? })
    }
}

/// A player identity: UUID, name and skin properties.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GameProfile {
    pub uuid: Uuid,
    pub name: String,
    pub properties: Vec<Property>,
}

impl Encode for GameProfile {
    fn encode(&self, w: &mut Vec<u8>) {
        w.put_uuid(self.uuid);
        w.put_string(&self.name);
        self.properties.encode(w);
    }
}

impl Decode for GameProfile {
    fn decode(r: &mut Reader<'_>) -> Result<Self> {
        Ok(Self { uuid: r.uuid()?, name: r.string(16)?, properties: r.list(16, Property::decode)? })
    }
}

/// A data pack both sides may already have, used to elide registry data.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KnownPack {
    pub namespace: String,
    pub id: String,
    pub version: String,
}

impl Encode for KnownPack {
    fn encode(&self, w: &mut Vec<u8>) {
        w.put_string(&self.namespace);
        w.put_string(&self.id);
        w.put_string(&self.version);
    }
}

impl Decode for KnownPack {
    fn decode(r: &mut Reader<'_>) -> Result<Self> {
        Ok(Self { namespace: r.string(32767)?, id: r.string(32767)?, version: r.string(32767)? })
    }
}

/// An angle transmitted as 1/256ths of a turn; holds degrees.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Angle(pub f32);

impl Encode for Angle {
    fn encode(&self, w: &mut Vec<u8>) {
        w.put_angle(self.0);
    }
}

impl Decode for Angle {
    fn decode(r: &mut Reader<'_>) -> Result<Self> {
        r.angle().map(Angle)
    }
}

/// A velocity or offset in the low-precision `LpVec3` encoding.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LpVec3(pub f64, pub f64, pub f64);

impl Encode for LpVec3 {
    fn encode(&self, w: &mut Vec<u8>) {
        w.put_lp_vec3(self.0, self.1, self.2);
    }
}

impl Decode for LpVec3 {
    fn decode(r: &mut Reader<'_>) -> Result<Self> {
        let (x, y, z) = r.lp_vec3()?;
        Ok(LpVec3(x, y, z))
    }
}

/// A text component transmitted as a JSON string (login disconnect) rather than NBT.
#[derive(Clone, Debug, PartialEq)]
pub struct JsonText(pub crate::text::Component);

impl Encode for JsonText {
    fn encode(&self, w: &mut Vec<u8>) {
        w.put_string(&self.0.to_json_string());
    }
}

impl Decode for JsonText {
    fn decode(r: &mut Reader<'_>) -> Result<Self> {
        let s = r.string(262144)?;
        crate::text::Component::from_json_str(&s).map(JsonText).map_err(|e| DecodeError::Other(format!("invalid JSON text component: {e}")))
    }
}

/// An item stack.
///
/// Data components are carried as opaque `(component type id, encoded value)` pairs: the
/// server relays them between clients byte for byte and only interprets the few it needs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ItemStack {
    /// Protocol id in the `minecraft:item` registry.
    pub item: i32,
    pub count: i32,
    /// Components added or overridden relative to the item's defaults.
    pub added: Vec<(i32, Vec<u8>)>,
    /// Default components that were removed.
    pub removed: Vec<i32>,
}

impl ItemStack {
    pub fn new(item: i32, count: i32) -> Self {
        Self { item, count, added: Vec::new(), removed: Vec::new() }
    }

    /// Whether two stacks hold the same item with the same components (ignoring count).
    pub fn same_item(&self, other: &ItemStack) -> bool {
        self.item == other.item && self.added == other.added && self.removed == other.removed
    }

    pub fn with_count(&self, count: i32) -> ItemStack {
        ItemStack { count, ..self.clone() }
    }
}

/// An inventory slot: an item stack or empty.
///
/// There are two encodings. Servers send the *trusted* form where component values are
/// written back to back; clients send the *untrusted* form (creative inventory) where every
/// component value is length-prefixed so a server can skip values it does not understand.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Slot(pub Option<ItemStack>);

impl Slot {
    pub const EMPTY: Slot = Slot(None);

    pub fn of(item: i32, count: i32) -> Slot {
        Slot(Some(ItemStack::new(item, count)))
    }

    pub fn is_empty(&self) -> bool {
        self.0.as_ref().is_none_or(|s| s.count <= 0)
    }

    fn write(&self, w: &mut Vec<u8>, length_prefixed: bool) {
        let Some(stack) = self.0.as_ref().filter(|s| s.count > 0) else {
            w.put_varint(0);
            return;
        };
        w.put_varint(stack.count);
        w.put_varint(stack.item);
        w.put_varint(stack.added.len() as i32);
        w.put_varint(stack.removed.len() as i32);
        for (ty, data) in &stack.added {
            w.put_varint(*ty);
            if length_prefixed {
                w.put_varint(data.len() as i32);
            }
            w.put_slice(data);
        }
        for ty in &stack.removed {
            w.put_varint(*ty);
        }
    }

    /// Trusted (clientbound) encoding.
    pub fn encode_trusted(&self, w: &mut Vec<u8>) {
        self.write(w, false);
    }

    /// Untrusted (serverbound) encoding with length-prefixed component values.
    pub fn encode_untrusted(&self, w: &mut Vec<u8>) {
        self.write(w, true);
    }

    /// Decodes the untrusted (serverbound) encoding.
    pub fn decode_untrusted(r: &mut Reader<'_>) -> Result<Slot> {
        let count = r.varint()?;
        if count <= 0 {
            return Ok(Slot(None));
        }
        let item = r.varint()?;
        let n_add = r.len(256)?;
        let n_remove = r.len(256)?;
        let mut added = Vec::with_capacity(n_add);
        for _ in 0..n_add {
            let ty = r.varint()?;
            let len = r.len(r.remaining())?;
            added.push((ty, r.take(len)?.to_vec()));
        }
        let mut removed = Vec::with_capacity(n_remove);
        for _ in 0..n_remove {
            removed.push(r.varint()?);
        }
        Ok(Slot(Some(ItemStack { item, count, added, removed })))
    }

    /// Decodes the trusted (clientbound) encoding. Component values have no length prefix,
    /// so stacks that carry added components cannot be skipped and are rejected; stacks
    /// without them (and removals only) decode fine. Used by test clients.
    pub fn decode_trusted(r: &mut Reader<'_>) -> Result<Slot> {
        let count = r.varint()?;
        if count <= 0 {
            return Ok(Slot(None));
        }
        let item = r.varint()?;
        let n_add = r.len(256)?;
        let n_remove = r.len(256)?;
        if n_add > 0 {
            return Err(DecodeError::Other("cannot decode data components in the trusted slot format".into()));
        }
        let mut removed = Vec::with_capacity(n_remove);
        for _ in 0..n_remove {
            removed.push(r.varint()?);
        }
        Ok(Slot(Some(ItemStack { item, count, added: Vec::new(), removed })))
    }
}

/// An item stack as clients describe it in container clicks: component values are replaced
/// by hashes, so only the item, count and component *types* are known.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HashedSlot(pub Option<HashedStack>);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HashedStack {
    pub item: i32,
    pub count: i32,
    pub added: Vec<(i32, i32)>,
    pub removed: Vec<i32>,
}

impl Encode for HashedSlot {
    fn encode(&self, w: &mut Vec<u8>) {
        match &self.0 {
            None => w.put_bool(false),
            Some(s) => {
                w.put_bool(true);
                w.put_varint(s.item);
                w.put_varint(s.count);
                w.put_list(&s.added, |w, (ty, hash)| {
                    w.put_varint(*ty);
                    w.put_i32(*hash);
                });
                w.put_list(&s.removed, |w, ty| w.put_varint(*ty));
            }
        }
    }
}

impl Decode for HashedSlot {
    fn decode(r: &mut Reader<'_>) -> Result<Self> {
        if !r.bool()? {
            return Ok(HashedSlot(None));
        }
        let item = r.varint()?;
        let count = r.varint()?;
        let added = r.list(256, |r| Ok((r.varint()?, r.i32()?)))?;
        let removed = r.list(256, |r| r.varint())?;
        Ok(HashedSlot(Some(HashedStack { item, count, added, removed })))
    }
}

/// Light data attached to chunk and light-update packets.
///
/// 26.3 clients read one more byte at the start of the light data than the published
/// protocol documentation lists. Without it they misread every chunk packet (failing with
/// "ByteArray with size 16255 is bigger than allowed 2048" on the light arrays); it is
/// written as 0, which is valid whether it is a flag, a count or an absent value.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LightData {
    pub sky_mask: Vec<u64>,
    pub block_mask: Vec<u64>,
    pub empty_sky_mask: Vec<u64>,
    pub empty_block_mask: Vec<u64>,
    /// One 2048-byte nibble array per bit set in `sky_mask`, lowest section first.
    pub sky_arrays: Vec<Vec<u8>>,
    /// One 2048-byte nibble array per bit set in `block_mask`, lowest section first.
    pub block_arrays: Vec<Vec<u8>>,
}

impl Encode for LightData {
    fn encode(&self, w: &mut Vec<u8>) {
        w.put_u8(0);
        w.put_bitset(&self.sky_mask);
        w.put_bitset(&self.block_mask);
        w.put_bitset(&self.empty_sky_mask);
        w.put_bitset(&self.empty_block_mask);
        w.put_list(&self.sky_arrays, |w, a| w.put_byte_array(a));
        w.put_list(&self.block_arrays, |w, a| w.put_byte_array(a));
    }
}

impl Decode for LightData {
    fn decode(r: &mut Reader<'_>) -> Result<Self> {
        r.u8()?;
        Ok(Self {
            sky_mask: r.bitset()?,
            block_mask: r.bitset()?,
            empty_sky_mask: r.bitset()?,
            empty_block_mask: r.bitset()?,
            sky_arrays: r.list(64, |r| Ok(r.byte_array(2048)?.to_vec()))?,
            block_arrays: r.list(64, |r| Ok(r.byte_array(2048)?.to_vec()))?,
        })
    }
}

/// Bit flags of a teleport: which components are relative to the current value.
pub mod teleport_flags {
    pub const X: i32 = 0x01;
    pub const Y: i32 = 0x02;
    pub const Z: i32 = 0x04;
    pub const YAW: i32 = 0x08;
    pub const PITCH: i32 = 0x10;
    pub const VELOCITY_X: i32 = 0x20;
    pub const VELOCITY_Y: i32 = 0x40;
    pub const VELOCITY_Z: i32 = 0x80;
    pub const ROTATE_VELOCITY: i32 = 0x100;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slot_formats() {
        let stack = ItemStack { item: 42, count: 3, added: vec![(7, vec![1, 2, 3]), (9, vec![])], removed: vec![4] };
        let slot = Slot(Some(stack));

        let mut untrusted = Vec::new();
        slot.encode_untrusted(&mut untrusted);
        assert_eq!(untrusted, [3, 42, 2, 1, 7, 3, 1, 2, 3, 9, 0, 4]);
        let mut r = Reader::new(&untrusted);
        assert_eq!(Slot::decode_untrusted(&mut r).unwrap(), slot);
        assert!(r.is_empty());

        let mut trusted = Vec::new();
        slot.encode_trusted(&mut trusted);
        assert_eq!(trusted, [3, 42, 2, 1, 7, 1, 2, 3, 9, 4]);

        let mut empty = Vec::new();
        Slot::EMPTY.encode_trusted(&mut empty);
        assert_eq!(empty, [0]);
        assert_eq!(Slot::decode_untrusted(&mut Reader::new(&[0])).unwrap(), Slot::EMPTY);

        let plain = Slot::of(1, 64);
        let mut bytes = Vec::new();
        plain.encode_trusted(&mut bytes);
        assert_eq!(Slot::decode_trusted(&mut Reader::new(&bytes)).unwrap(), plain);
    }

    #[test]
    fn untrusted_slot_rejects_overlong_component() {
        // Component claims 100 bytes but the packet ends.
        let data = [1, 5, 1, 0, 7, 100, 1, 2];
        assert!(Slot::decode_untrusted(&mut Reader::new(&data)).is_err());
    }

    #[test]
    fn profile_round_trip() {
        let p = GameProfile {
            uuid: Uuid::from_u128(0x1234),
            name: "Steve".into(),
            properties: vec![Property { name: "textures".into(), value: "abc".into(), signature: Some("sig".into()) }],
        };
        let mut w = Vec::new();
        p.encode(&mut w);
        assert_eq!(GameProfile::decode(&mut Reader::new(&w)).unwrap(), p);
    }
}
