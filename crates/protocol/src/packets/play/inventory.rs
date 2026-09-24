//! inventory packets: stacks, slots, windows, entity metadata (P15-04).
//!
//! Mechanical split of `super::play`: every item here moved
//! byte-identical. No logic changed.

use crate::ids::clientbound;
use crate::nbt::Nbt;
use crate::text::TextComponent;
use crate::wire::{PacketReader, PacketWriter};
use mc_core::error::{ServerError, ServerResult};

use super::super::Packet;
use super::{packed_len, read_count};

/// Wire type id for [`MetadataValue::Byte`].
pub const METADATA_TYPE_BYTE: i32 = 0;

/// Wire type id for [`MetadataValue::VarInt`]: vanilla's `INT` serializer.
///
/// The 26.1 serializer table (pumpkin `meta_data_type`, `v26_1` column)
/// names no fixed-width `i32` serializer at all: `INT` **is** a `VarInt`
/// (id 1), `LONG` is id 2. An earlier revision of this file modelled a
/// fixed-int type 2 and sent orb values through it; a real client
/// disconnected with a protocol error on the first orb spawn (killing any
/// mob), because type 2 decodes as a `VarLong`. The fixed variant is gone;
/// orb values ride [`MetadataValue::VarInt`].
pub const METADATA_TYPE_VARINT: i32 = 1;

/// Wire type id for [`MetadataValue::Float`].
pub const METADATA_TYPE_FLOAT: i32 = 3;

/// Wire type id for [`MetadataValue::ItemStack`].
///
/// Read off the wire rather than from the serializer table alone: a captured `set_entity_data` for a dropped item
/// carries `08 07` -- index 8, this type -- followed by the stack.
pub const METADATA_TYPE_ITEM_STACK: i32 = 7;

/// Wire type ids for [`MetadataValue::Variant`]: the data-driven registry
/// variants and their sound siblings (`cat` 21/22, `cow` 23/24, `wolf` 25/26,
/// `frog` 27, `pig` 28/29, `chicken` 30/31, `zombie-nautilus` 32, painting 34).
///
/// Names and ids from the 26.1 serializer table (pumpkin's generated
/// `meta_data_type`, `v26_1` column); the shared shape — a `VarInt` registry
/// id — is the registry-holder mechanism every member uses, the same shape
/// `cat` variants have ridden since 1.14. Directly evidenced for 24: two
/// vanilla-capture cow bodies carry index 19, this type, one `0x01` byte.
pub const METADATA_TYPE_VARIANTS: &[i32] = &[21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31, 32, 34];

/// Terminator that ends a metadata entry list.
pub const METADATA_TERMINATOR: u8 = 0xFF;

/// The metadata **slot index** a living entity's health rides on: **9**, with [`METADATA_TYPE_FLOAT`].
///
/// Measured, not read off a table: every mob kind Phase 11 spawns (zombie, skeleton, creeper, spider, pig,
/// chicken, sheep, cow, slime) sent its spawn health at index 9 with the float type in the console-summon
/// captures, across several hundred bodies whose walk ended on a clean terminator. The full per-type slot table
/// lives in `crates/test-support/fixtures/registry/entity_metadata.tsv`, and
/// `crates/protocol/tests/entity_metadata_golden.rs` pins both the constant and two whole captured bodies.
pub const METADATA_INDEX_HEALTH: u8 = 9;

/// The metadata slot index an experience orb's value rides on: **8**, with
/// [`METADATA_TYPE_VARINT`] (vanilla's `INT` serializer is a `VarInt`).
///
/// From the 26.x tracked-data table (pumpkin `experience_orb.DATA_VALUE`,
/// index 8, `INT` serializer). A fixed-int type 2 rode here before and
/// disconnected real clients on the first orb spawn — type 2 is `LONG` —
/// so the encoding is pinned byte-exact by `orb_value_encodes_varint`
/// below rather than by assertion alone.
pub const METADATA_INDEX_ORB_VALUE: u8 = 8;

/// The metadata slot index a creeper's fuse/state rides on: **16**, with
/// [`METADATA_TYPE_VARINT`] (`0` unlit, `1` lit).
///
/// From `crates/test-support/fixtures/registry/entity_metadata.tsv`
/// (`minecraft:creeper 16 1 V fuse/state varint`); the announce path asserts
/// it by construction when a fuse lights or stands down.
pub const METADATA_INDEX_CREEPER_FUSE: u8 = 16;

/// One entity metadata value.
///
/// Byte, `VarInt`, float, item-stack and registry-variant-holder shapes
/// are modelled. Vanilla's remaining type ids (2 `LONG`, 4, 5, 6, …:
/// string, component, …) are deliberately **not** decoded here: a wrong guess would
/// silently misparse a later entry and desynchronise the rest of the list, so
/// [`SetEntityData::decode`] rejects them with an explicit error instead
/// (AGENTS.md section 3.3). Adding one is a new variant plus its `type_id`
/// arm.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MetadataValue {
    /// Type id [`METADATA_TYPE_BYTE`]: a signed byte held as its raw `u8`.
    Byte(u8),
    /// Type id [`METADATA_TYPE_VARINT`]: vanilla's `INT` serializer.
    VarInt(i32),
    /// Type id [`METADATA_TYPE_FLOAT`].
    Float(f32),
    /// One of [`METADATA_TYPE_VARIANTS`]: a registry-variant holder, the
    /// variant's registry id as a `VarInt`. `kind` round-trips which family
    /// member it is, so re-encoding a decoded body reproduces the wire bytes.
    Variant {
        /// The wire type id this value arrived with.
        kind: i32,
        /// Variant registry id.
        id: i32,
    },
    /// Type id [`METADATA_TYPE_ITEM_STACK`]: an item and a count.
    ///
    /// **Data components are not modelled.** The wire carries a patch of added and removed components after the
    /// item id, and a captured stack with none sends `00 00`; this variant writes that empty patch, so a stack
    /// that carries components would be sent as though it carried none. Named here rather than discovered by a
    /// caller, and the reason the fields are the two this build can be honest about.
    ItemStack {
        /// How many items the stack holds.
        count: i32,
        /// Item registry id.
        item_id: i32,
    },
}

impl MetadataValue {
    /// The wire type id written before the value.
    #[must_use]
    pub const fn type_id(self) -> i32 {
        match self {
            Self::Byte(_) => METADATA_TYPE_BYTE,
            Self::VarInt(_) => METADATA_TYPE_VARINT,
            Self::Float(_) => METADATA_TYPE_FLOAT,
            Self::Variant { kind, .. } => kind,
            Self::ItemStack { .. } => METADATA_TYPE_ITEM_STACK,
        }
    }

    /// Write the value (not the index or type id) to `writer`.
    pub fn encode(self, writer: &mut PacketWriter) {
        match self {
            Self::Byte(value) => writer.write_u8(value),
            Self::VarInt(value) => writer.write_varint(value),
            Self::Float(value) => writer.write_f32(value),
            Self::Variant { id, .. } => writer.write_varint(id),
            Self::ItemStack { count, item_id } => {
                // The component patch: nothing added, nothing removed. A captured stack with no components sends
                // exactly these two bytes, and a dropped item is a stack with no components.
                writer.write_varint(count);
                writer.write_varint(item_id);
                writer.write_varint(0);
                writer.write_varint(0);
            }
        }
    }

    /// Read a value of the given wire type id.
    ///
    /// # Errors
    ///
    /// [`ServerError::Protocol`] for an unmodelled type id or a truncated value.
    pub fn decode(reader: &mut PacketReader<'_>, type_id: i32) -> ServerResult<Self> {
        match type_id {
            METADATA_TYPE_BYTE => Ok(Self::Byte(reader.read_u8()?)),
            METADATA_TYPE_VARINT => Ok(Self::VarInt(reader.read_varint()?)),
            METADATA_TYPE_FLOAT => Ok(Self::Float(reader.read_f32()?)),
            METADATA_TYPE_ITEM_STACK => {
                let count = reader.read_varint()?;
                let item_id = reader.read_varint()?;
                // The patch this build does not model. Refusing a non-empty one is the alternative to silently
                // discarding it, and nothing in this capture carries one.
                let added = reader.read_varint()?;
                let removed = reader.read_varint()?;
                if added != 0 || removed != 0 {
                    return Err(ServerError::Protocol(format!(
                        "item stack carries {added} added and {removed} removed data components, which this build \
 does not model"
                    )));
                }
                Ok(Self::ItemStack { count, item_id })
            }
            other if METADATA_TYPE_VARIANTS.contains(&other) => Ok(Self::Variant {
                kind: other,
                id: reader.read_varint()?,
            }),
            other => Err(ServerError::Protocol(format!(
                "unmodelled entity metadata type id {other}"
            ))),
        }
    }
}

/// `minecraft:set_entity_data` (clientbound play).
///
/// Body: `VarInt` entity id, then metadata entries terminated by
/// [`METADATA_TERMINATOR`]:
///
/// ```text
/// per entry: u8 index, VarInt type id, <value>
/// end:       u8 0xFF
/// ```
///
/// The index is the metadata field slot (health, air, custom name, …), not a
/// sequence number; slots may be sent in any order and a later packet may
/// resend a slot. The terminator is mandatory — a payload that simply runs out
/// of bytes is an error, because a client would keep reading.
#[derive(Debug, Clone, PartialEq)]
pub struct SetEntityData {
    /// Entity the entries apply to.
    pub entity_id: i32,
    /// `(index, value)` pairs in wire order.
    pub entries: Vec<(u8, MetadataValue)>,
}

impl Packet for SetEntityData {
    const ID: i32 = clientbound::play::SET_ENTITY_DATA;

    fn decode(payload: &[u8]) -> ServerResult<Self> {
        let mut reader = PacketReader::new(payload);
        let entity_id = reader.read_varint()?;
        let mut entries = Vec::new();
        loop {
            let index = reader.read_u8()?;
            if index == METADATA_TERMINATOR {
                break;
            }
            if entries.len() >= MAX_METADATA_ENTRIES {
                return Err(ServerError::Protocol(format!(
                    "more than {MAX_METADATA_ENTRIES} metadata entries without a terminator"
                )));
            }
            let type_id = reader.read_varint()?;
            entries.push((index, MetadataValue::decode(&mut reader, type_id)?));
        }
        if !reader.is_empty() {
            return Err(ServerError::Protocol(format!(
                "set_entity_data has {} trailing bytes",
                reader.remaining()
            )));
        }
        Ok(Self { entity_id, entries })
    }

    fn encode(&self) -> ServerResult<Vec<u8>> {
        let mut writer = PacketWriter::new();
        writer.write_varint(self.entity_id);
        for (index, value) in &self.entries {
            writer.write_u8(*index);
            writer.write_varint(value.type_id());
            value.encode(&mut writer);
        }
        writer.write_u8(METADATA_TERMINATOR);
        Ok(writer.finish())
    }
}

/// Cap on metadata entries in one [`SetEntityData`].
///
/// Vanilla's entity data table has well under a hundred slots; the cap only
/// exists so a payload that never reaches its terminator cannot loop forever.
pub const MAX_METADATA_ENTRIES: usize = 256;

/// An item stack as it appears in inventory packets (protocol 775).
///
/// One optional slot on the wire, exactly as 26.1.2's
/// `ItemStack.createOptionalStreamCodec` reads it (`ItemStack$1` +
/// `DataComponentPatch$3`, bytecode-read from the jar):
///
/// `count VarInt`; a count `<= 0` is the whole stack (bare `0x00`, the only
/// bytes an empty slot ever occupies). Otherwise `item id VarInt`, then the
/// component patch: `added-count VarInt`, that many `(type, value)` entries,
/// `removed-count VarInt`, that many bare type ids. Protocol 775 does **not**
/// carry the pre-1.20.5 `slot` / `change count` bytes after the count.
///
/// Component values are framed by each type's stream codec (P18-01a):
/// `damage`/`max_damage`/`repair_cost` are `VarInt`s, `enchantments` a counted
/// `(id, level)` list, `custom_name` a network-NBT string, `attack_range` six
/// `f32`s, `food`/`consumable` the pumpkin shapes. An unmodelled type is kept
/// as raw bytes only when it is the **last** added component and the removed
/// list is empty (then `payload = remaining` minus the trailing `0x00`);
/// mid-list unknowns are refused rather than guessed (AGENTS.md section 3.3).
///
/// Two consequences this crate got wrong before the P14-09 walk (a picked-up
/// cobblestone was the first non-empty stack a real client ever had to
/// decode from us, and it failed): the count comes **first**, not the id,
/// and an empty patch is two zero bytes (`00 00`), not one. Empty slots encode
/// identically either way (`0x00`), which is why every inventory sync before
/// that walk looked fine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemStack {
    /// Item registry id; `0` means "empty stack".
    pub item_id: i32,
    /// Stack size; ignored when `item_id` is 0.
    pub count: i32,
    /// Added `(component type id, pre-encoded component data)` pairs, in wire
    /// order. Removed components are unmodelled: this crate never sends any and
    /// refuses to receive any rather than dropping them silently.
    pub components: Vec<(i32, Vec<u8>)>,
}

impl ItemStack {
    /// The empty stack (`item_id == 0`), written as a bare `VarInt 0`.
    #[must_use]
    pub const fn empty() -> Self {
        Self {
            item_id: 0,
            count: 0,
            components: Vec::new(),
        }
    }

    /// Whether this stack encodes as a bare `VarInt 0`.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.item_id == 0
    }

    /// A stack with `count` items and no data components.
    #[must_use]
    pub const fn simple(item_id: i32, count: i32) -> Self {
        Self {
            item_id,
            count,
            components: Vec::new(),
        }
    }

    /// Decode the typed component patch (P18-01a).
    ///
    /// # Errors
    ///
    /// [`ServerError::Protocol`] when a modelled payload is truncated.
    pub fn to_typed_components(&self) -> ServerResult<mc_entity::ItemComponents> {
        let mut entries = Vec::new();
        for (type_id, payload) in &self.components {
            if mc_entity::components::DataComponent::is_known_type(*type_id) {
                entries.push(mc_entity::components::decode_payload(*type_id, payload)?);
            } else {
                entries.push(mc_entity::unknown_from_wire(*type_id, payload));
            }
        }
        Ok(mc_entity::ItemComponents::from_entries(entries))
    }

    /// Build from typed components (P18-01a).
    ///
    /// # Errors
    ///
    /// [`ServerError::Invariant`] when a modelled payload cannot be encoded.
    pub fn from_typed_components(
        item_id: i32,
        count: i32,
        components: &mc_entity::ItemComponents,
    ) -> ServerResult<Self> {
        let mut wire = Vec::new();
        for component in components.entries() {
            wire.push((
                component.type_id(),
                mc_entity::components::encode_payload(component)?,
            ));
        }
        Ok(Self {
            item_id,
            count,
            components: wire,
        })
    }

    /// Encode the stack, returning the number of bytes written.
    ///
    /// # Errors
    ///
    /// [`ServerError::Invariant`] for a negative count or a component count
    /// that does not fit a `VarInt` (server-authored data).
    pub fn encode(&self, writer: &mut PacketWriter) -> ServerResult<usize> {
        let before = writer.len();
        if self.count < 0 {
            return Err(ServerError::Invariant(format!(
                "item stack count {} is negative",
                self.count
            )));
        }
        // Vanilla reads the count first and stops at `<= 0`; an id-0 stack
        // is air by registry lookup, so both spellings encode as bare zero.
        if self.count == 0 || self.item_id == 0 {
            writer.write_varint(0);
            return Ok(writer.len() - before);
        }
        writer.write_varint(self.count);
        writer.write_varint(self.item_id);
        writer.write_varint(packed_len(self.components.len())?);
        for (type_id, data) in &self.components {
            writer.write_varint(*type_id);
            writer.write_bytes(data);
        }
        // Removed components: always empty on send (unmodelled).
        writer.write_varint(0);
        Ok(writer.len() - before)
    }

    /// Decode one stack.
    ///
    /// Component values are framed by each modelled type's codec (P18-01a).
    /// An unmodelled type is kept as raw bytes only when it is the **last**
    /// added component and the removed list is empty (`payload = remaining`
    /// minus the trailing zero count); mid-list unknowns are refused rather
    /// than guessed (AGENTS.md section 3.3).
    ///
    /// # Errors
    ///
    /// [`ServerError::Protocol`] for a negative item id, a count outside
    /// `0..=`[`MAX_ITEM_COUNT`] (A12-03 hostile width), an absurd component
    /// count, a removed-component list (unmodelled — refused, never dropped),
    /// a mid-list unmodelled component, or a truncated payload.
    pub fn decode(reader: &mut PacketReader<'_>) -> ServerResult<Self> {
        let count = reader.read_varint()?;
        if count < 0 {
            return Err(ServerError::Protocol(format!(
                "item stack count {count} is negative"
            )));
        }
        if count == 0 {
            return Ok(Self::empty());
        }
        if count > MAX_ITEM_COUNT {
            return Err(ServerError::Protocol(format!(
                "item stack count {count} exceeds the {MAX_ITEM_COUNT} stack limit"
            )));
        }
        let item_id = reader.read_varint()?;
        if item_id < 0 {
            return Err(ServerError::Protocol(format!(
                "item stack id {item_id} is negative"
            )));
        }
        if item_id == 0 {
            // `Item.byId(0)` is air: a positive count of nothing is nothing.
            return Ok(Self::empty());
        }
        let added = read_count(reader, "item component", MAX_ITEM_COMPONENTS)?;
        let mut components = Vec::with_capacity(added.min(64));
        for index in 0..added {
            let type_id = reader.read_varint()?;
            let payload = if mc_entity::components::DataComponent::is_known_type(type_id) {
                let before = reader.remaining_slice();
                let len = mc_entity::components::payload_prefix_len(type_id, before)?;
                let bytes = reader.read_bytes(len)?.to_vec();
                // Fail fast on a mistyped payload rather than forwarding it.
                mc_entity::components::decode_payload(type_id, &bytes)?;
                bytes
            } else if index + 1 == added {
                // Last added component: the rest of the body is `payload` plus
                // the removed-count VarInt. An empty removed list is a single
                // trailing `0x00`, which is the only shape this build can
                // frame without guessing. Leave that byte for `read_count`.
                let rest = reader.remaining_slice();
                if rest.is_empty() {
                    return Err(ServerError::Protocol(
                        "unmodelled item component with no payload".to_owned(),
                    ));
                }
                if *rest.last().unwrap_or(&0xFF) != 0 {
                    return Err(ServerError::Protocol(format!(
                        "unmodelled item component type {type_id} cannot be framed when the \
                         removed-component list is non-empty"
                    )));
                }
                let payload_len = rest.len() - 1;
                reader.advance(payload_len)?;
                rest[..payload_len].to_vec()
            } else {
                return Err(ServerError::Protocol(format!(
                    "unmodelled item data component type id {type_id} in a multi-component \
                     patch (only the last added component may be opaque)"
                )));
            };
            components.push((type_id, payload));
        }
        let removed = read_count(reader, "removed item component", MAX_ITEM_COMPONENTS)?;
        if removed > 0 {
            return Err(ServerError::Protocol(format!(
                "{removed} removed item components present but removed \
                 components are unmodelled"
            )));
        }
        Ok(Self {
            item_id,
            count,
            components,
        })
    }
}

/// Cap on data components in one [`ItemStack`].
///
/// Only `0` is currently decodable (see [`ItemStack::decode`]); the constant
/// exists so the rejection message is a range check rather than a magic number.
pub const MAX_ITEM_COMPONENTS: usize = 1024;

/// Largest legal item count on the wire (vanilla `stacksTo` ceiling).
///
/// A decoded count above this is hostile input, not a rounding accident
/// (A12-03). Matches `mc_entity::stack::HARD_MAX_STACK_SIZE` without pulling
/// that crate into the protocol layer.
pub const MAX_ITEM_COUNT: i32 = 64;

/// `minecraft:container_set_slot` (clientbound play).
///
/// Body: container-id `VarInt`, state-id `VarInt`, `i16` slot index, then the
/// [`ItemStack`]. The container id is a `VarInt` on the wire
/// (`FriendlyByteBuf.readContainerId`, bytecode-read): window `0` is the
/// player inventory, `-1` the cursor. The state id is echoed by the client in
/// `container_click` so a stale click can be detected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContainerSetSlot {
    /// Window the slot belongs to.
    pub window_id: i32,
    /// Inventory state id the client last acknowledged.
    pub state_id: i32,
    /// Slot index within the window.
    pub slot: i16,
    /// New contents of the slot.
    pub item: ItemStack,
}

impl Packet for ContainerSetSlot {
    const ID: i32 = clientbound::play::CONTAINER_SET_SLOT;

    fn decode(payload: &[u8]) -> ServerResult<Self> {
        let mut reader = PacketReader::new(payload);
        let window_id = reader.read_varint()?;
        let state_id = reader.read_varint()?;
        let slot = reader.read_i16()?;
        let item = ItemStack::decode(&mut reader)?;
        if !reader.is_empty() {
            return Err(ServerError::Protocol(format!(
                "container_set_slot has {} trailing bytes",
                reader.remaining()
            )));
        }
        Ok(Self {
            window_id,
            state_id,
            slot,
            item,
        })
    }

    fn encode(&self) -> ServerResult<Vec<u8>> {
        let mut writer = PacketWriter::new();
        writer.write_varint(self.window_id);
        writer.write_varint(self.state_id);
        writer.write_i16(self.slot);
        self.item.encode(&mut writer)?;
        Ok(writer.finish())
    }
}

/// `minecraft:container_set_content` (clientbound play).
///
/// Body: container-id `VarInt`, state-id `VarInt`, `VarInt` slot count, that
/// many [`ItemStack`]s, then the carried (cursor) stack. The carried stack is
/// **not** part of the count — it is written unconditionally, even when empty —
/// so an off-by-one here shifts every remaining stack.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContainerSetContent {
    /// Window being filled.
    pub window_id: i32,
    /// Inventory state id the client last acknowledged.
    pub state_id: i32,
    /// Contents of the window's slots, in slot-index order.
    pub slots: Vec<ItemStack>,
    /// Stack held on the cursor.
    pub carried: ItemStack,
}

/// Largest slot count accepted in one [`ContainerSetContent`].
///
/// A large chest holds 54 slots and the player inventory 46; the cap leaves
/// room for any container vanilla currently defines.
pub const MAX_CONTAINER_SLOTS: usize = 256;

impl Packet for ContainerSetContent {
    const ID: i32 = clientbound::play::CONTAINER_SET_CONTENT;

    fn decode(payload: &[u8]) -> ServerResult<Self> {
        let mut reader = PacketReader::new(payload);
        let window_id = reader.read_varint()?;
        let state_id = reader.read_varint()?;
        let count = read_count(&mut reader, "container slot", MAX_CONTAINER_SLOTS)?;
        let mut slots = Vec::with_capacity(count);
        for _ in 0..count {
            slots.push(ItemStack::decode(&mut reader)?);
        }
        let carried = ItemStack::decode(&mut reader)?;
        if !reader.is_empty() {
            return Err(ServerError::Protocol(format!(
                "container_set_content has {} trailing bytes",
                reader.remaining()
            )));
        }
        Ok(Self {
            window_id,
            state_id,
            slots,
            carried,
        })
    }

    fn encode(&self) -> ServerResult<Vec<u8>> {
        let mut writer = PacketWriter::new();
        writer.write_varint(self.window_id);
        writer.write_varint(self.state_id);
        writer.write_varint(packed_len(self.slots.len())?);
        for slot in &self.slots {
            slot.encode(&mut writer)?;
        }
        self.carried.encode(&mut writer)?;
        Ok(writer.finish())
    }
}

/// Vanilla `MenuType` registry ids for the windows P12 opens.
///
/// Source: `javap -c -p` on `net.minecraft.world.inventory.MenuType` from the
/// official 26.1.2 server jar — the static initialiser registers in this order,
/// and `BuiltInRegistries.MENU` assigns 0..N in registration order:
///
/// ```text
/// 0 generic_9x1, 1 generic_9x2, 2 generic_9x3, 3 generic_9x4, 4 generic_9x5,
/// 5 generic_9x6, 6 generic_3x3, 7 crafter_3x3, 8 anvil, 9 beacon,
/// 10 blast_furnace, 11 brewing_stand, 12 crafting, 13 enchantment, 14 furnace,
/// 15 grindstone, 16 hopper, 17 lectern, 18 loom, 19 merchant, 20 shulker_box,
/// 21 smithing, 22 smoker, 23 cartography_table, 24 stonecutter
/// ```
///
/// Only the four P12 needs are named here; adding a fifth menu is adding a
/// constant, not re-deriving the table. P17-01 adds the fifth
/// (`generic_3x3` = 6) from the same table.
pub const MENU_GENERIC_9X3: i32 = 2;

/// Double chest (54 slots).
pub const MENU_GENERIC_9X6: i32 = 5;

/// Furnace (3 slots: input, fuel, output).
pub const MENU_FURNACE: i32 = 14;

/// Hopper (5 slots).
pub const MENU_HOPPER: i32 = 16;

/// Dispenser, dropper and crafter 3×3 (9 slots, P17-01).
pub const MENU_GENERIC_3X3: i32 = 6;

/// Crafting table 3×3 plus result (P17-02 Step C, same jar table).
pub const MENU_CRAFTING: i32 = 12;

/// Highest `MenuType` registry id in the 26.1.2 table (0..=24).
///
/// `open_screen` carries this as a bare `VarInt`; without a ceiling a hostile
/// value is accepted and echoed to the client (A12-03).
pub const MAX_MENU_TYPE: i32 = 24;

/// Window ids this server hands out for block menus (`1..=127`; 0 is the
/// player inventory and is never opened via `open_screen`).
pub const MAX_WINDOW_ID: i32 = 127;

/// `minecraft:open_screen` (clientbound play 59).
///
/// Body, from `javap -c -p` on
/// `net.minecraft.network.protocol.game.ClientboundOpenScreenPacket` (26.1.2):
/// `containerId` via `ByteBufCodecs.CONTAINER_ID` (a `VarInt`), then `type` via
/// `ByteBufCodecs.registry(Registries.MENU)` (a `VarInt` registry id), then
/// `title` via `ComponentSerialization.TRUSTED_STREAM_CODEC` (network NBT —
/// the same encoding [`SystemChat`] uses).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenScreen {
    /// Window id the client will use in `container_click` (non-zero).
    pub window_id: i32,
    /// Vanilla `MenuType` registry id (see `MENU_*` above).
    pub menu_type: i32,
    /// Window title.
    pub title: TextComponent,
}

impl Packet for OpenScreen {
    const ID: i32 = clientbound::play::OPEN_SCREEN;

    /// Decode an `open_screen`.
    ///
    /// # Errors
    ///
    /// [`ServerError::Protocol`] when `window_id` or `menu_type` is outside the
    /// legal range (A12-03), the title is truncated, or trailing bytes remain.
    fn decode(payload: &[u8]) -> ServerResult<Self> {
        let mut reader = PacketReader::new(payload);
        let window_id = reader.read_varint()?;
        if window_id <= 0 || window_id > MAX_WINDOW_ID {
            return Err(ServerError::Protocol(format!(
                "open_screen window id {window_id} is outside 1..={MAX_WINDOW_ID}"
            )));
        }
        let menu_type = reader.read_varint()?;
        if !(0..=MAX_MENU_TYPE).contains(&menu_type) {
            return Err(ServerError::Protocol(format!(
                "open_screen menu type {menu_type} is outside 0..={MAX_MENU_TYPE}"
            )));
        }
        let rest = reader.take_remaining();
        let mut rest = rest;
        let nbt = Nbt::read_network(&mut rest)?;
        if !rest.is_empty() {
            return Err(ServerError::Protocol(format!(
                "open_screen has {} trailing bytes",
                rest.len()
            )));
        }
        let title = super::super::config::text_from_nbt(&nbt)?;
        Ok(Self {
            window_id,
            menu_type,
            title,
        })
    }

    fn encode(&self) -> ServerResult<Vec<u8>> {
        let mut writer = PacketWriter::new();
        writer.write_varint(self.window_id);
        writer.write_varint(self.menu_type);
        let mut bytes = Vec::new();
        self.title.to_nbt().write_network(&mut bytes)?;
        writer.write_bytes(&bytes);
        Ok(writer.finish())
    }
}

/// `minecraft:container_set_data` (clientbound play 19).
///
/// Body, from `javap -c -p` on
/// `net.minecraft.network.protocol.game.ClientboundContainerSetDataPacket`:
/// `readContainerId` (`VarInt`), then `readShort` property, then `readShort`
/// value. Furnace progress rides this: property 0 = burn remaining,
/// 1 = burn total, 2 = cook progress, 3 = cook total (vanilla
/// `FurnaceMenu` data slots, asserted by the furnace tests).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContainerSetData {
    /// Window id.
    pub window_id: i32,
    /// Property id.
    pub property: i16,
    /// Property value.
    pub value: i16,
}

impl Packet for ContainerSetData {
    const ID: i32 = clientbound::play::CONTAINER_SET_DATA;

    fn decode(payload: &[u8]) -> ServerResult<Self> {
        let mut reader = PacketReader::new(payload);
        let window_id = reader.read_varint()?;
        let property = reader.read_i16()?;
        let value = reader.read_i16()?;
        if !reader.is_empty() {
            return Err(ServerError::Protocol(format!(
                "container_set_data has {} trailing bytes",
                reader.remaining()
            )));
        }
        Ok(Self {
            window_id,
            property,
            value,
        })
    }

    fn encode(&self) -> ServerResult<Vec<u8>> {
        let mut writer = PacketWriter::new();
        writer.write_varint(self.window_id);
        writer.write_i16(self.property);
        writer.write_i16(self.value);
        Ok(writer.finish())
    }
}

/// `minecraft:container_close` (clientbound play 17).
///
/// Body, from `javap -c -p` on
/// `net.minecraft.network.protocol.game.ClientboundContainerClosePacket`:
/// a single `readContainerId` (`VarInt`). Tells the client to close a window
/// the server no longer tracks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContainerClose {
    /// Window id to close.
    pub window_id: i32,
}

impl Packet for ContainerClose {
    const ID: i32 = clientbound::play::CONTAINER_CLOSE;

    fn decode(payload: &[u8]) -> ServerResult<Self> {
        let mut reader = PacketReader::new(payload);
        let window_id = reader.read_varint()?;
        if !reader.is_empty() {
            return Err(ServerError::Protocol(format!(
                "container_close has {} trailing bytes",
                reader.remaining()
            )));
        }
        Ok(Self { window_id })
    }

    fn encode(&self) -> ServerResult<Vec<u8>> {
        let mut writer = PacketWriter::new();
        writer.write_varint(self.window_id);
        Ok(writer.finish())
    }
}

/// `minecraft:set_cursor_item` (clientbound play 96).
///
/// Body, from `javap -c -p` on
/// `net.minecraft.network.protocol.game.ClientboundSetCursorItemPacket`:
/// a single `ItemStack.OPTIONAL_STREAM_CODEC` — the same optional stack
/// encoding [`ItemStack`] already implements.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetCursorItem {
    /// Stack on the cursor.
    pub item: ItemStack,
}

impl Packet for SetCursorItem {
    const ID: i32 = clientbound::play::SET_CURSOR_ITEM;

    fn decode(payload: &[u8]) -> ServerResult<Self> {
        let mut reader = PacketReader::new(payload);
        let item = ItemStack::decode(&mut reader)?;
        if !reader.is_empty() {
            return Err(ServerError::Protocol(format!(
                "set_cursor_item has {} trailing bytes",
                reader.remaining()
            )));
        }
        Ok(Self { item })
    }

    fn encode(&self) -> ServerResult<Vec<u8>> {
        let mut writer = PacketWriter::new();
        self.item.encode(&mut writer)?;
        Ok(writer.finish())
    }
}

/// Flag bit: the effect is ambient (beacon-grade, fewer particles).
pub const MOB_EFFECT_FLAG_AMBIENT: i8 = 0x01;
/// Flag bit: show particles.
pub const MOB_EFFECT_FLAG_PARTICLES: i8 = 0x02;
/// Flag bit: show the HUD icon.
pub const MOB_EFFECT_FLAG_ICON: i8 = 0x04;
/// Flag bit: blend the screen overlay in.
///
/// Jar `ClientboundUpdateMobEffectPacket` builds the flags from
/// ambient/visible/icon plus a fourth constructor boolean into this
/// bit; vanilla passes true on a fresh add (`ServerPlayer.onEffectAdded`)
/// and false on re-sync (`PlayerList.sendActiveEffects`) — and a live
/// 26.1.2 server's `/effect give` bytes end in `0x0E`, i.e. this bit set.
pub const MOB_EFFECT_FLAG_BLEND: i8 = 0x08;

/// `minecraft:update_mob_effect` (clientbound 132): one effect icon for the
/// HUD.
///
/// Field order (entity, effect, amplifier, duration, flags) corroborated by
/// pumpkin's `CUpdateMobEffect` and jar-verified. The flag bits are
/// ambient/particles/icon plus blend (`0x08`, set on fresh adds, clear on
/// re-syncs — jar `ServerPlayer.onEffectAdded` vs
/// `PlayerList.sendActiveEffects`, and a live 26.1.2 capture ends the
/// give-packet in `0x0E`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UpdateMobEffect {
    /// Entity carrying the effect (a player, for this server).
    pub entity_id: i32,
    /// Effect registry raw id (`MobEffect.STREAM_CODEC` writes the raw id
    /// as a bare `VarInt` — see `mc_entity::effect::EffectKind::wire_id`).
    pub effect_id: i32,
    /// Amplifier, 0-based.
    pub amplifier: i32,
    /// Remaining duration in ticks.
    pub duration: i32,
    /// Flag bits ([`MOB_EFFECT_FLAG_AMBIENT`] etc.).
    pub flags: i8,
}

impl UpdateMobEffect {
    /// Flags for an effect: ambient when it says so, particles and icon on
    /// (this build models no hidden-icon effects), blend on fresh adds.
    ///
    /// `blend` mirrors the packet's fourth constructor boolean: vanilla
    /// passes true when an effect is newly added and false when merely
    /// re-syncing the active set (jar `ServerPlayer.onEffectAdded` vs
    /// `PlayerList.sendActiveEffects`).
    #[must_use]
    pub const fn flags_for(ambient: bool, blend: bool) -> i8 {
        (if ambient { MOB_EFFECT_FLAG_AMBIENT } else { 0 })
            | MOB_EFFECT_FLAG_PARTICLES
            | MOB_EFFECT_FLAG_ICON
            | (if blend { MOB_EFFECT_FLAG_BLEND } else { 0 })
    }
}

impl Packet for UpdateMobEffect {
    const ID: i32 = clientbound::play::UPDATE_MOB_EFFECT;

    fn decode(payload: &[u8]) -> ServerResult<Self> {
        let mut reader = PacketReader::new(payload);
        let packet = Self {
            entity_id: reader.read_varint()?,
            effect_id: reader.read_varint()?,
            amplifier: reader.read_varint()?,
            duration: reader.read_varint()?,
            flags: reader.read_i8()?,
        };
        if !reader.is_empty() {
            return Err(ServerError::Protocol(format!(
                "update_mob_effect has {} trailing bytes",
                reader.remaining()
            )));
        }
        Ok(packet)
    }

    fn encode(&self) -> ServerResult<Vec<u8>> {
        let mut writer = PacketWriter::new();
        writer.write_varint(self.entity_id);
        writer.write_varint(self.effect_id);
        writer.write_varint(self.amplifier);
        writer.write_varint(self.duration);
        writer.write_i8(self.flags);
        Ok(writer.finish())
    }
}

/// `minecraft:remove_mob_effect` (clientbound 78): clear one effect icon.
///
/// Without this, an expired or cleared effect would stick on the HUD until
/// relog: the icon's lifetime is bounded by this packet, not by the
/// duration the client counts down.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RemoveMobEffect {
    /// Entity that lost the effect.
    pub entity_id: i32,
    /// Effect registry raw id (same codec as [`UpdateMobEffect`]).
    pub effect_id: i32,
}

impl Packet for RemoveMobEffect {
    const ID: i32 = clientbound::play::REMOVE_MOB_EFFECT;

    fn decode(payload: &[u8]) -> ServerResult<Self> {
        let mut reader = PacketReader::new(payload);
        let packet = Self {
            entity_id: reader.read_varint()?,
            effect_id: reader.read_varint()?,
        };
        if !reader.is_empty() {
            return Err(ServerError::Protocol(format!(
                "remove_mob_effect has {} trailing bytes",
                reader.remaining()
            )));
        }
        Ok(packet)
    }

    fn encode(&self) -> ServerResult<Vec<u8>> {
        let mut writer = PacketWriter::new();
        writer.write_varint(self.entity_id);
        writer.write_varint(self.effect_id);
        Ok(writer.finish())
    }
}
