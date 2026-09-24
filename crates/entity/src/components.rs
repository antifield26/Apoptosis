//! Item data components (P18-01a): the wire/disk subset the survival loop reads.
//!
//! Closed set, matching `PHASE-18.md` P18-01a: `damage`/`max_damage`,
//! `enchantments`/`stored_enchantments`, `custom_name`, `repair_cost`,
//! `attack_range`, and `food`/`consumable` (the fields P18-06 reads). Anything
//! else the client or a file carries is [`DataComponent::Unknown`] and is
//! preserved **byte-identical** across save→load.
//!
//! ## Wire payloads
//!
//! Vanilla's `DataComponentPatch` is `(type id, value)*` with no per-value
//! length: framing is the type's own stream codec. This module therefore owns
//! a codec per modelled type (pumpkin `data_component_impl` /
//! `data_component` codecs, 26.1 registry ids from
//! `pumpkin-data/src/generated/data_component.rs`) and treats an unmodelled
//! type as opaque bytes only when the rest of the patch is empty — the last
//! added component with nothing after it — so `payload = remaining`. Mid-list
//! unknowns are refused rather than guessed (AGENTS.md section 3.3).
//!
//! ## Disk shape
//!
//! Each stack may carry a `components` compound keyed by the vanilla name
//! (`minecraft:damage`, …). Modelled types write readable NBT; unknowns keep
//! their exact wire payload under `_wire/<type_id>` (and their original NBT
//! under the original name when a file supplied one). Re-encoding a decoded
//! compound is byte-stable, which is what `unknown_component_survives_save_load`
//! pins.

use mc_core::error::{ServerError, ServerResult};
use mc_nbt::NbtTag;
use std::fmt;

/// `minecraft:max_damage` type id (26.1 registry order).
pub const TYPE_MAX_DAMAGE: i32 = 2;
/// `minecraft:damage` type id.
pub const TYPE_DAMAGE: i32 = 3;
/// `minecraft:custom_name` type id.
pub const TYPE_CUSTOM_NAME: i32 = 6;
/// `minecraft:enchantments` type id.
pub const TYPE_ENCHANTMENTS: i32 = 13;
/// `minecraft:repair_cost` type id.
pub const TYPE_REPAIR_COST: i32 = 19;
/// `minecraft:food` type id.
pub const TYPE_FOOD: i32 = 23;
/// `minecraft:consumable` type id.
pub const TYPE_CONSUMABLE: i32 = 24;
/// `minecraft:attack_range` type id.
pub const TYPE_ATTACK_RANGE: i32 = 30;
/// `minecraft:stored_enchantments` type id.
pub const TYPE_STORED_ENCHANTMENTS: i32 = 42;

/// Cap on enchantment entries in one component (mirrors pumpkin's limit).
pub const MAX_ENCHANTMENTS: usize = 256;

/// Cap on components in one stack patch.
pub const MAX_COMPONENTS: usize = 1024;

/// Extra entity reach granted by a held item's `attack_range`, over vanilla's
/// default 3.0-block entity interaction range.
///
/// The component schema is real 26.x data (pumpkin `AttackRangeImpl`).
/// `max_reach` is the survival entity reach the item claims; the default 3.0
/// matches vanilla's bare `isWithinEntityInteractionRange` base, so a stack
/// without the component keeps today's gate and one with `max_reach = 5.0`
/// adds 2.0. The remaining five fields (`min_reach`, creative reaches,
/// `hitbox_margin`, `mob_factor`) are stored and shown but not combined —
/// no reachable reference states the full formula (named gap, P16-01 hook
/// closed for the additive half only).
/// `minecraft:attack_range` schema (pumpkin `AttackRangeImpl`).
#[derive(Debug, Clone)]
pub struct AttackRange {
    /// Closest the hit may be (vanilla default 0.0).
    pub min_reach: f32,
    /// Survival entity reach (vanilla default 3.0).
    pub max_reach: f32,
    /// Closest creative hit (vanilla default 0.0).
    pub min_creative_reach: f32,
    /// Creative entity reach (vanilla default 5.0).
    pub max_creative_reach: f32,
    /// Hitbox expansion applied when measuring (vanilla default 0.3).
    pub hitbox_margin: f32,
    /// Mob-facing scale (vanilla default 1.0).
    pub mob_factor: f32,
}

impl PartialEq for AttackRange {
    fn eq(&self, other: &Self) -> bool {
        self.values()
            .iter()
            .zip(other.values())
            .all(|(a, b)| a.to_bits() == b.to_bits())
    }
}

impl Default for AttackRange {
    fn default() -> Self {
        Self {
            min_reach: 0.0,
            max_reach: 3.0,
            min_creative_reach: 0.0,
            max_creative_reach: 5.0,
            hitbox_margin: 0.3,
            mob_factor: 1.0,
        }
    }
}

impl AttackRange {
    /// The six floats in wire/NBT field order.
    fn values(&self) -> [f32; 6] {
        [
            self.min_reach,
            self.max_reach,
            self.min_creative_reach,
            self.max_creative_reach,
            self.hitbox_margin,
            self.mob_factor,
        ]
    }

    fn from_values(values: [f32; 6]) -> Self {
        Self {
            min_reach: values[0],
            max_reach: values[1],
            min_creative_reach: values[2],
            max_creative_reach: values[3],
            hitbox_margin: values[4],
            mob_factor: values[5],
        }
    }
}

// Bit equality so `ItemStack` can stay `Eq`/`Hash` with float fields.
impl Eq for AttackRange {}

impl std::hash::Hash for AttackRange {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        for value in self.values() {
            value.to_bits().hash(state);
        }
    }
}

/// `minecraft:food` (P18-06 reads these fields).
#[derive(Debug, Clone)]
pub struct Food {
    /// Hunger points restored (vanilla bread = 5).
    pub nutrition: i32,
    /// Saturation modifier (vanilla bread = 6.0).
    pub saturation: f32,
    /// Whether the player may eat at full hunger (`always_eat`).
    pub can_always_eat: bool,
}

impl PartialEq for Food {
    fn eq(&self, other: &Self) -> bool {
        self.nutrition == other.nutrition
            && self.saturation.to_bits() == other.saturation.to_bits()
            && self.can_always_eat == other.can_always_eat
    }
}

impl Eq for Food {}

impl std::hash::Hash for Food {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.nutrition.hash(state);
        self.saturation.to_bits().hash(state);
        self.can_always_eat.hash(state);
    }
}

/// Vanilla `ConsumeAnimation` ordinal (pumpkin `ConsumeAnimation`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ConsumeAnimation {
    /// No animation.
    None,
    /// Eat.
    Eat,
    /// Drink.
    Drink,
    /// Block (shield-like).
    Block,
    /// Bow.
    Bow,
    /// Spear.
    Spear,
    /// Crossbow.
    Crossbow,
    /// Spyglass.
    Spyglass,
    /// Horn.
    Horn,
    /// Brush.
    Brush,
}

impl ConsumeAnimation {
    /// Vanilla ordinal.
    #[must_use]
    pub const fn id(self) -> i32 {
        match self {
            Self::None => 0,
            Self::Eat => 1,
            Self::Drink => 2,
            Self::Block => 3,
            Self::Bow => 4,
            Self::Spear => 5,
            Self::Crossbow => 6,
            Self::Spyglass => 7,
            Self::Horn => 8,
            Self::Brush => 9,
        }
    }

    /// Parse a vanilla ordinal.
    #[must_use]
    pub const fn from_id(id: i32) -> Option<Self> {
        match id {
            0 => Some(Self::None),
            1 => Some(Self::Eat),
            2 => Some(Self::Drink),
            3 => Some(Self::Block),
            4 => Some(Self::Bow),
            5 => Some(Self::Spear),
            6 => Some(Self::Crossbow),
            7 => Some(Self::Spyglass),
            8 => Some(Self::Horn),
            9 => Some(Self::Brush),
            _ => None,
        }
    }

    /// Disk name (`"eat"`, …).
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Eat => "eat",
            Self::Drink => "drink",
            Self::Block => "block",
            Self::Bow => "bow",
            Self::Spear => "spear",
            Self::Crossbow => "crossbow",
            Self::Spyglass => "spyglass",
            Self::Horn => "horn",
            Self::Brush => "brush",
        }
    }

    /// Parse a disk name.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "none" => Some(Self::None),
            "eat" => Some(Self::Eat),
            "drink" => Some(Self::Drink),
            "block" => Some(Self::Block),
            "bow" => Some(Self::Bow),
            "spear" => Some(Self::Spear),
            "crossbow" => Some(Self::Crossbow),
            "spyglass" => Some(Self::Spyglass),
            "horn" => Some(Self::Horn),
            "brush" => Some(Self::Brush),
            _ => None,
        }
    }
}

/// A sound the consumable plays (`IdOr<SoundEvent>` on the wire).
#[derive(Debug, Clone)]
pub enum SoundRef {
    /// Registry id (`VarInt(id + 1)` on the wire).
    Id(i32),
    /// Inline name plus optional range (`VarInt 0` then string + option f32).
    Named {
        /// Resource name, e.g. `minecraft:entity.generic.eat`.
        name: String,
        /// Optional audible range.
        range: Option<f32>,
    },
}

impl PartialEq for SoundRef {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Id(a), Self::Id(b)) => a == b,
            (
                Self::Named {
                    name: a_name,
                    range: a_range,
                },
                Self::Named {
                    name: b_name,
                    range: b_range,
                },
            ) => a_name == b_name && a_range.map(f32::to_bits) == b_range.map(f32::to_bits),
            _ => false,
        }
    }
}

impl Eq for SoundRef {}

impl std::hash::Hash for SoundRef {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        match self {
            Self::Id(id) => {
                0u8.hash(state);
                id.hash(state);
            }
            Self::Named { name, range } => {
                1u8.hash(state);
                name.hash(state);
                range.map(f32::to_bits).hash(state);
            }
        }
    }
}

/// `minecraft:consumable` (P18-06 reads these fields).
///
/// `on_consume_effects` is **not** modelled as typed effects yet: a non-empty
/// effect list is refused on the wire (framing) and preserved as opaque NBT on
/// disk. P18-06's closed effect set lands with that task.
#[derive(Debug, Clone)]
pub struct Consumable {
    /// Seconds the use animation runs (vanilla food = 1.6).
    pub consume_seconds: f32,
    /// Use animation.
    pub animation: ConsumeAnimation,
    /// Sound played while consuming.
    pub sound: SoundRef,
    /// Whether consume particles spawn.
    pub consume_particles: bool,
    /// Raw `on_consume_effects` NBT list as stored on disk (usually empty).
    pub on_consume_effects: Vec<NbtTag>,
}

impl Default for Consumable {
    fn default() -> Self {
        Self {
            consume_seconds: 1.6,
            animation: ConsumeAnimation::Eat,
            sound: SoundRef::Named {
                name: "minecraft:entity.generic.eat".to_owned(),
                range: None,
            },
            consume_particles: true,
            on_consume_effects: Vec::new(),
        }
    }
}

impl PartialEq for Consumable {
    fn eq(&self, other: &Self) -> bool {
        self.consume_seconds.to_bits() == other.consume_seconds.to_bits()
            && self.animation == other.animation
            && self.sound == other.sound
            && self.consume_particles == other.consume_particles
            && self.on_consume_effects == other.on_consume_effects
    }
}

impl Eq for Consumable {}

impl std::hash::Hash for Consumable {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.consume_seconds.to_bits().hash(state);
        self.animation.hash(state);
        self.sound.hash(state);
        self.consume_particles.hash(state);
        self.on_consume_effects.len().hash(state);
    }
}

/// One `(enchantment registry id, level)` pair.
pub type EnchantEntry = (i32, i32);

/// A data component on a stack.
///
/// Known variants are the P18-01a closed set. [`DataComponent::Unknown`] is
/// how an unmodelled component survives save→load: its wire payload (and any
/// disk NBT) is kept exactly as received.
#[derive(Debug, Clone)]
pub enum DataComponent {
    /// `minecraft:damage`: points of durability already spent.
    Damage(i32),
    /// `minecraft:max_damage`: durability total.
    MaxDamage(i32),
    /// `minecraft:enchantments`: applied enchantments.
    Enchantments(Vec<EnchantEntry>),
    /// `minecraft:stored_enchantments`: book-stored enchantments.
    StoredEnchantments(Vec<EnchantEntry>),
    /// `minecraft:custom_name`: renamed display name (literal).
    CustomName(String),
    /// `minecraft:repair_cost`: anvil cost (stored for P21).
    RepairCost(i32),
    /// `minecraft:attack_range`: entity reach override.
    AttackRange(AttackRange),
    /// `minecraft:food`: nutrition/saturation/always-eat.
    Food(Food),
    /// `minecraft:consumable`: eat/drink behaviour.
    Consumable(Consumable),
    /// An unmodelled component, preserved byte-identical.
    Unknown {
        /// Vanilla resource name when a file supplied one, else `"#<type_id>"`.
        key: String,
        /// Wire type id (`0` when only a disk name is known).
        type_id: i32,
        /// Exact wire payload after the type id (may be empty).
        wire: Vec<u8>,
        /// Disk NBT value as a file supplied it (may be absent).
        nbt: Option<NbtTag>,
    },
}

impl PartialEq for DataComponent {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Damage(a), Self::Damage(b))
            | (Self::MaxDamage(a), Self::MaxDamage(b))
            | (Self::RepairCost(a), Self::RepairCost(b)) => a == b,
            (Self::Enchantments(a), Self::Enchantments(b))
            | (Self::StoredEnchantments(a), Self::StoredEnchantments(b)) => a == b,
            (Self::CustomName(a), Self::CustomName(b)) => a == b,
            (Self::AttackRange(a), Self::AttackRange(b)) => a == b,
            (Self::Food(a), Self::Food(b)) => a == b,
            (Self::Consumable(a), Self::Consumable(b)) => a == b,
            (
                Self::Unknown {
                    key: a_key,
                    type_id: a_id,
                    wire: a_wire,
                    nbt: a_nbt,
                },
                Self::Unknown {
                    key: b_key,
                    type_id: b_id,
                    wire: b_wire,
                    nbt: b_nbt,
                },
            ) => a_key == b_key && a_id == b_id && a_wire == b_wire && a_nbt == b_nbt,
            _ => false,
        }
    }
}

impl Eq for DataComponent {}

impl std::hash::Hash for DataComponent {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.type_id().hash(state);
        self.name().hash(state);
    }
}

impl DataComponent {
    /// Wire type id of this component.
    #[must_use]
    pub const fn type_id(&self) -> i32 {
        match self {
            Self::Damage(_) => TYPE_DAMAGE,
            Self::MaxDamage(_) => TYPE_MAX_DAMAGE,
            Self::Enchantments(_) => TYPE_ENCHANTMENTS,
            Self::StoredEnchantments(_) => TYPE_STORED_ENCHANTMENTS,
            Self::CustomName(_) => TYPE_CUSTOM_NAME,
            Self::RepairCost(_) => TYPE_REPAIR_COST,
            Self::AttackRange(_) => TYPE_ATTACK_RANGE,
            Self::Food(_) => TYPE_FOOD,
            Self::Consumable(_) => TYPE_CONSUMABLE,
            Self::Unknown { type_id, .. } => *type_id,
        }
    }

    /// Vanilla resource name of this component.
    #[must_use]
    pub fn name(&self) -> &str {
        match self {
            Self::Damage(_) => "minecraft:damage",
            Self::MaxDamage(_) => "minecraft:max_damage",
            Self::Enchantments(_) => "minecraft:enchantments",
            Self::StoredEnchantments(_) => "minecraft:stored_enchantments",
            Self::CustomName(_) => "minecraft:custom_name",
            Self::RepairCost(_) => "minecraft:repair_cost",
            Self::AttackRange(_) => "minecraft:attack_range",
            Self::Food(_) => "minecraft:food",
            Self::Consumable(_) => "minecraft:consumable",
            Self::Unknown { key, .. } => key,
        }
    }

    /// Whether this type is one of the modelled codecs (framable on the wire).
    #[must_use]
    pub const fn is_known_type(type_id: i32) -> bool {
        matches!(
            type_id,
            TYPE_MAX_DAMAGE
                | TYPE_DAMAGE
                | TYPE_CUSTOM_NAME
                | TYPE_ENCHANTMENTS
                | TYPE_REPAIR_COST
                | TYPE_FOOD
                | TYPE_CONSUMABLE
                | TYPE_ATTACK_RANGE
                | TYPE_STORED_ENCHANTMENTS
        )
    }
}

/// The ordered component patch on one stack.
///
/// Order is wire order, so re-encoding a decoded stack reproduces the bytes.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct ItemComponents {
    entries: Vec<DataComponent>,
}

impl ItemComponents {
    /// The empty patch.
    pub const EMPTY: Self = Self {
        entries: Vec::new(),
    };

    /// An empty patch (same as [`ItemComponents::EMPTY`]).
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Build from an ordered list.
    #[must_use]
    pub fn from_entries(entries: Vec<DataComponent>) -> Self {
        Self { entries }
    }

    /// The entries, in wire order.
    #[must_use]
    pub fn entries(&self) -> &[DataComponent] {
        &self.entries
    }

    /// Consume into the ordered list.
    #[must_use]
    pub fn into_entries(self) -> Vec<DataComponent> {
        self.entries
    }

    /// Whether the patch is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Number of components.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// First component with the given type id.
    #[must_use]
    pub fn get(&self, type_id: i32) -> Option<&DataComponent> {
        self.entries.iter().find(|c| c.type_id() == type_id)
    }

    /// Insert or replace the first component with the same type id.
    pub fn set(&mut self, component: DataComponent) {
        let type_id = component.type_id();
        if let Some(slot) = self.entries.iter().position(|c| c.type_id() == type_id) {
            self.entries[slot] = component;
        } else {
            self.entries.push(component);
        }
    }

    /// Remove every component with the given type id; returns how many went.
    pub fn remove(&mut self, type_id: i32) -> usize {
        let before = self.entries.len();
        self.entries.retain(|c| c.type_id() != type_id);
        before - self.entries.len()
    }

    /// `minecraft:damage`, if present.
    #[must_use]
    pub fn damage(&self) -> Option<i32> {
        match self.get(TYPE_DAMAGE) {
            Some(DataComponent::Damage(v)) => Some(*v),
            _ => None,
        }
    }

    /// `minecraft:max_damage`, if present.
    #[must_use]
    pub fn max_damage(&self) -> Option<i32> {
        match self.get(TYPE_MAX_DAMAGE) {
            Some(DataComponent::MaxDamage(v)) => Some(*v),
            _ => None,
        }
    }

    /// `minecraft:custom_name`, if present.
    #[must_use]
    pub fn custom_name(&self) -> Option<&str> {
        match self.get(TYPE_CUSTOM_NAME) {
            Some(DataComponent::CustomName(v)) => Some(v),
            _ => None,
        }
    }

    /// `minecraft:repair_cost`, if present (stored for P21 anvils).
    #[must_use]
    pub fn repair_cost(&self) -> Option<i32> {
        match self.get(TYPE_REPAIR_COST) {
            Some(DataComponent::RepairCost(v)) => Some(*v),
            _ => None,
        }
    }

    /// `minecraft:enchantments`, if present.
    #[must_use]
    pub fn enchantments(&self) -> Option<&[EnchantEntry]> {
        match self.get(TYPE_ENCHANTMENTS) {
            Some(DataComponent::Enchantments(v)) => Some(v),
            _ => None,
        }
    }

    /// `minecraft:stored_enchantments`, if present.
    #[must_use]
    pub fn stored_enchantments(&self) -> Option<&[EnchantEntry]> {
        match self.get(TYPE_STORED_ENCHANTMENTS) {
            Some(DataComponent::StoredEnchantments(v)) => Some(v),
            _ => None,
        }
    }

    /// `minecraft:attack_range`, if present.
    #[must_use]
    pub fn attack_range(&self) -> Option<&AttackRange> {
        match self.get(TYPE_ATTACK_RANGE) {
            Some(DataComponent::AttackRange(v)) => Some(v),
            _ => None,
        }
    }

    /// `minecraft:food`, if present — P18-06's nutrition/saturation/always-eat.
    #[must_use]
    pub fn food(&self) -> Option<&Food> {
        match self.get(TYPE_FOOD) {
            Some(DataComponent::Food(v)) => Some(v),
            _ => None,
        }
    }

    /// `minecraft:consumable`, if present — P18-06's `consume_seconds` etc.
    #[must_use]
    pub fn consumable(&self) -> Option<&Consumable> {
        match self.get(TYPE_CONSUMABLE) {
            Some(DataComponent::Consumable(v)) => Some(v),
            _ => None,
        }
    }

    /// Every unknown component, in order.
    pub fn unknowns(&self) -> impl Iterator<Item = &DataComponent> {
        self.entries
            .iter()
            .filter(|c| matches!(c, DataComponent::Unknown { .. }))
    }
}

// ---------------------------------------------------------------- wire codec

/// Cursor over a component payload.
struct WireRead<'a> {
    data: &'a [u8],
}

impl<'a> WireRead<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data }
    }

    fn remaining(&self) -> usize {
        self.data.len()
    }

    fn take(&mut self, n: usize) -> ServerResult<&'a [u8]> {
        if self.data.len() < n {
            return Err(ServerError::Protocol(format!(
                "component payload truncated: need {n} bytes, have {}",
                self.data.len()
            )));
        }
        let (head, tail) = self.data.split_at(n);
        self.data = tail;
        Ok(head)
    }

    fn u8(&mut self) -> ServerResult<u8> {
        Ok(self.take(1)?[0])
    }

    fn bool(&mut self) -> ServerResult<bool> {
        Ok(self.u8()? != 0)
    }

    fn i32_be(&mut self) -> ServerResult<i32> {
        let bytes = self.take(4)?;
        Ok(i32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    fn f32_be(&mut self) -> ServerResult<f32> {
        Ok(f32::from_bits(self.i32_be()? as u32))
    }

    fn varint(&mut self) -> ServerResult<i32> {
        let mut value = 0i32;
        let mut shift = 0u32;
        loop {
            let byte = self.u8()?;
            value |= i32::from(byte & 0x7F) << shift;
            if byte & 0x80 == 0 {
                return Ok(value);
            }
            shift += 7;
            if shift >= 35 {
                return Err(ServerError::Protocol("varint too long".to_owned()));
            }
        }
    }

    fn string(&mut self) -> ServerResult<String> {
        let len = self.varint()?;
        if len < 0 {
            return Err(ServerError::Protocol(format!(
                "negative component string length {len}"
            )));
        }
        let len = usize::try_from(len)
            .map_err(|_| ServerError::Protocol("string length overflow".to_owned()))?;
        if len > 32_767 * 4 {
            return Err(ServerError::Protocol(format!(
                "component string of {len} bytes exceeds limit"
            )));
        }
        let bytes = self.take(len)?;
        String::from_utf8(bytes.to_vec()).map_err(|error| {
            ServerError::Protocol(format!("component string is not UTF-8: {error}"))
        })
    }

    fn f32_be_opt(&mut self) -> ServerResult<Option<f32>> {
        if self.bool()? {
            Ok(Some(self.f32_be()?))
        } else {
            Ok(None)
        }
    }
}

/// Writer for a component payload (no type id).
#[derive(Default)]
struct WireWrite {
    buf: Vec<u8>,
}

impl WireWrite {
    fn u8(&mut self, value: u8) {
        self.buf.push(value);
    }

    fn bool(&mut self, value: bool) {
        self.u8(u8::from(value));
    }

    fn i32_be(&mut self, value: i32) {
        self.buf.extend_from_slice(&value.to_be_bytes());
    }

    fn f32_be(&mut self, value: f32) {
        self.i32_be(value.to_bits() as i32);
    }

    fn varint(&mut self, mut value: i32) {
        loop {
            let byte = (value & 0x7F) as u8;
            value = ((value as u32) >> 7) as i32;
            if value == 0 {
                self.u8(byte);
                return;
            }
            self.u8(byte | 0x80);
        }
    }

    fn string(&mut self, value: &str) {
        self.varint(i32::try_from(value.len()).unwrap_or(i32::MAX));
        self.buf.extend_from_slice(value.as_bytes());
    }

    fn f32_be_opt(&mut self, value: Option<f32>) {
        match value {
            Some(v) => {
                self.bool(true);
                self.f32_be(v);
            }
            None => self.bool(false),
        }
    }
}

fn encode_sound(sound: &SoundRef, out: &mut WireWrite) {
    match sound {
        SoundRef::Id(id) => out.varint(id.saturating_add(1)),
        SoundRef::Named { name, range } => {
            out.varint(0);
            out.string(name);
            out.f32_be_opt(*range);
        }
    }
}

fn decode_sound(read: &mut WireRead<'_>) -> ServerResult<SoundRef> {
    let tag = read.varint()?;
    match tag {
        0 => {
            let name = read.string()?;
            let range = read.f32_be_opt()?;
            Ok(SoundRef::Named { name, range })
        }
        tag if tag > 0 => Ok(SoundRef::Id(tag - 1)),
        tag => Err(ServerError::Protocol(format!(
            "negative sound IdOr tag {tag}"
        ))),
    }
}

fn encode_enchantments(entries: &[EnchantEntry], out: &mut WireWrite) -> ServerResult<()> {
    if entries.len() > MAX_ENCHANTMENTS {
        return Err(ServerError::Invariant(format!(
            "{} enchantments exceeds the {MAX_ENCHANTMENTS} cap",
            entries.len()
        )));
    }
    out.varint(i32::try_from(entries.len()).unwrap_or(i32::MAX));
    for (id, level) in entries {
        out.varint(*id);
        out.varint(*level);
    }
    Ok(())
}

fn decode_enchantments(read: &mut WireRead<'_>) -> ServerResult<Vec<EnchantEntry>> {
    let len = read.varint()?;
    if len < 0 || len as usize > MAX_ENCHANTMENTS {
        return Err(ServerError::Protocol(format!(
            "enchantment count {len} is outside 0..={MAX_ENCHANTMENTS}"
        )));
    }
    let mut entries = Vec::with_capacity(len as usize);
    for _ in 0..len {
        let id = read.varint()?;
        let level = read.varint()?;
        entries.push((id, level));
    }
    Ok(entries)
}

/// Encode one component's wire **payload** (the bytes after its type id).
///
/// # Errors
///
/// [`ServerError::Invariant`] when a string cannot be encoded (server-authored).
pub fn encode_payload(component: &DataComponent) -> ServerResult<Vec<u8>> {
    let mut out = WireWrite::default();
    match component {
        DataComponent::Damage(v) | DataComponent::MaxDamage(v) | DataComponent::RepairCost(v) => {
            out.varint(*v);
        }
        DataComponent::Enchantments(entries) | DataComponent::StoredEnchantments(entries) => {
            encode_enchantments(entries, &mut out)?;
        }
        DataComponent::CustomName(name) => {
            let mut bytes = Vec::new();
            NbtTag::String(name.clone()).write_network(&mut bytes)?;
            return Ok(bytes);
        }
        DataComponent::AttackRange(range) => {
            for value in range.values() {
                out.f32_be(value);
            }
        }
        DataComponent::Food(food) => {
            out.varint(food.nutrition);
            out.f32_be(food.saturation);
            out.bool(food.can_always_eat);
        }
        DataComponent::Consumable(consumable) => {
            if !consumable.on_consume_effects.is_empty() {
                // Wire framing for ConsumeEffect is unmodelled; a non-empty list
                // is a named gap rather than a silent empty write.
                return Err(ServerError::Invariant(
                    "consumable on_consume_effects cannot be re-encoded on the wire yet".to_owned(),
                ));
            }
            out.f32_be(consumable.consume_seconds);
            out.varint(consumable.animation.id());
            encode_sound(&consumable.sound, &mut out);
            out.bool(consumable.consume_particles);
            out.varint(0);
        }
        DataComponent::Unknown { wire, .. } => return Ok(wire.clone()),
    }
    Ok(out.buf)
}

/// Decode one component's wire **payload** (the bytes after its type id).
///
/// The whole payload is expected to be consumed: a trailing byte means the
/// codec and the sender disagree, which is refused rather than ignored.
///
/// # Errors
///
/// [`ServerError::Protocol`] for a truncated or mistyped payload, or a type id
/// this build does not model (except when the caller frames an unknown as the
/// last component — see [`ItemStack`] decode in `mc-protocol`).
pub fn decode_payload(type_id: i32, payload: &[u8]) -> ServerResult<DataComponent> {
    let mut read = WireRead::new(payload);
    let component = decode_payload_inner(type_id, &mut read)?;
    if read.remaining() != 0 {
        return Err(ServerError::Protocol(format!(
            "component type {type_id} payload has {} trailing bytes",
            read.remaining()
        )));
    }
    Ok(component)
}

fn decode_payload_inner(type_id: i32, read: &mut WireRead<'_>) -> ServerResult<DataComponent> {
    match type_id {
        TYPE_DAMAGE => Ok(DataComponent::Damage(read.varint()?)),
        TYPE_MAX_DAMAGE => Ok(DataComponent::MaxDamage(read.varint()?)),
        TYPE_REPAIR_COST => Ok(DataComponent::RepairCost(read.varint()?)),
        TYPE_ENCHANTMENTS => Ok(DataComponent::Enchantments(decode_enchantments(read)?)),
        TYPE_STORED_ENCHANTMENTS => Ok(DataComponent::StoredEnchantments(decode_enchantments(
            read,
        )?)),
        TYPE_CUSTOM_NAME => {
            let mut slice = read.take(read.remaining())?;
            let tag = NbtTag::read_network(&mut slice)?;
            let name = match tag {
                NbtTag::String(text) => text,
                other => {
                    return Err(ServerError::Protocol(format!(
                        "custom_name payload is {}, expected a literal TAG_String",
                        other.type_name()
                    )));
                }
            };
            Ok(DataComponent::CustomName(name))
        }
        TYPE_ATTACK_RANGE => {
            let values = [
                read.f32_be()?,
                read.f32_be()?,
                read.f32_be()?,
                read.f32_be()?,
                read.f32_be()?,
                read.f32_be()?,
            ];
            Ok(DataComponent::AttackRange(AttackRange::from_values(values)))
        }
        TYPE_FOOD => Ok(DataComponent::Food(Food {
            nutrition: read.varint()?,
            saturation: read.f32_be()?,
            can_always_eat: read.bool()?,
        })),
        TYPE_CONSUMABLE => {
            let consume_seconds = read.f32_be()?;
            let animation = ConsumeAnimation::from_id(read.varint()?).ok_or_else(|| {
                ServerError::Protocol("consumable animation id out of range".to_owned())
            })?;
            let sound = decode_sound(read)?;
            let consume_particles = read.bool()?;
            let effects = read.varint()?;
            if effects != 0 {
                return Err(ServerError::Protocol(format!(
                    "consumable carries {effects} on_consume_effects, which this build does not \
                     frame on the wire"
                )));
            }
            Ok(DataComponent::Consumable(Consumable {
                consume_seconds,
                animation,
                sound,
                consume_particles,
                on_consume_effects: Vec::new(),
            }))
        }
        other => Err(ServerError::Protocol(format!(
            "unmodelled item data component type id {other}"
        ))),
    }
}

/// Number of leading payload bytes that form one complete value of `type_id`.
///
/// The known codecs are self-delimiting from the front (a `VarInt`, a counted
/// list, a network NBT root, a fixed `f32` run), so the length is a pure
/// function of the leading bytes. Used by `mc-protocol` to frame a multi-
/// component patch without a per-value length field.
///
/// # Errors
///
/// [`ServerError::Protocol`] when the leading bytes are truncated or the type
/// is unmodelled.
pub fn payload_prefix_len(type_id: i32, payload: &[u8]) -> ServerResult<usize> {
    let mut read = WireRead::new(payload);
    // Known codecs are front-delimited: run the same field reads and count.
    match type_id {
        TYPE_DAMAGE | TYPE_MAX_DAMAGE | TYPE_REPAIR_COST => {
            read.varint()?;
        }
        TYPE_ENCHANTMENTS | TYPE_STORED_ENCHANTMENTS => {
            let len = read.varint()?;
            if len < 0 || len as usize > MAX_ENCHANTMENTS {
                return Err(ServerError::Protocol(format!(
                    "enchantment count {len} is outside 0..={MAX_ENCHANTMENTS}"
                )));
            }
            for _ in 0..len {
                read.varint()?;
                read.varint()?;
            }
        }
        TYPE_CUSTOM_NAME => {
            let mut slice = payload;
            NbtTag::read_network(&mut slice)?;
            return Ok(payload.len() - slice.len());
        }
        TYPE_ATTACK_RANGE => {
            for _ in 0..6 {
                read.f32_be()?;
            }
        }
        TYPE_FOOD => {
            read.varint()?;
            read.f32_be()?;
            read.bool()?;
        }
        TYPE_CONSUMABLE => {
            read.f32_be()?;
            read.varint()?;
            decode_sound(&mut read)?;
            read.bool()?;
            let effects = read.varint()?;
            if effects != 0 {
                return Err(ServerError::Protocol(format!(
                    "consumable carries {effects} on_consume_effects, which this build does not \
                     frame on the wire"
                )));
            }
        }
        other => {
            return Err(ServerError::Protocol(format!(
                "unmodelled item data component type id {other}"
            )));
        }
    }
    Ok(payload.len() - read.remaining())
}

/// Build an [`DataComponent::Unknown`] from a last-component wire payload.
#[must_use]
pub fn unknown_from_wire(type_id: i32, payload: &[u8]) -> DataComponent {
    DataComponent::Unknown {
        key: format!("#{type_id}"),
        type_id,
        wire: payload.to_vec(),
        nbt: None,
    }
}

// ---------------------------------------------------------------- disk codec

/// Write the `components` compound for a patch, or `None` when empty.
#[must_use]
pub fn to_nbt(components: &ItemComponents) -> Option<NbtTag> {
    if components.is_empty() {
        return None;
    }
    let mut entries: Vec<(String, NbtTag)> = Vec::new();
    for component in components.entries() {
        match component {
            DataComponent::Damage(v)
            | DataComponent::MaxDamage(v)
            | DataComponent::RepairCost(v) => {
                entries.push((component.name().to_owned(), NbtTag::Int(*v)));
            }
            DataComponent::CustomName(name) => {
                entries.push((component.name().to_owned(), NbtTag::String(name.clone())));
            }
            DataComponent::Enchantments(entries_list)
            | DataComponent::StoredEnchantments(entries_list) => {
                entries.push((
                    component.name().to_owned(),
                    enchantments_to_nbt(entries_list),
                ));
            }
            DataComponent::AttackRange(range) => {
                entries.push((component.name().to_owned(), attack_range_to_nbt(range)));
            }
            DataComponent::Food(food) => {
                entries.push((component.name().to_owned(), food_to_nbt(food)));
            }
            DataComponent::Consumable(consumable) => {
                entries.push((component.name().to_owned(), consumable_to_nbt(consumable)));
            }
            DataComponent::Unknown {
                key,
                type_id,
                wire,
                nbt,
            } => {
                if let Some(value) = nbt {
                    entries.push((key.clone(), value.clone()));
                }
                if *type_id != 0 {
                    // Always keep the wire payload so save→load is byte-stable
                    // even when the NBT half is a vanilla shape we only pass on.
                    entries.push((format!("_wire/{type_id}"), NbtTag::ByteArray(wire.clone())));
                }
            }
        }
    }
    Some(NbtTag::Compound(entries))
}

fn enchantments_to_nbt(entries: &[EnchantEntry]) -> NbtTag {
    NbtTag::List(
        entries
            .iter()
            .map(|(id, level)| {
                NbtTag::Compound(vec![
                    ("id".to_owned(), NbtTag::Int(*id)),
                    ("level".to_owned(), NbtTag::Int(*level)),
                ])
            })
            .collect(),
    )
}

fn enchantments_from_nbt(tag: &NbtTag) -> Vec<EnchantEntry> {
    match tag {
        NbtTag::List(items) => items
            .iter()
            .filter_map(|item| {
                let id = item.get_i32("id")?;
                let level = item.get_i32("level")?;
                Some((id, level))
            })
            .collect(),
        // Vanilla's name-keyed map shape: keys are enchantment names we do not
        // resolve yet, so values alone cannot recover the wire id. Kept out of
        // the typed path rather than invented.
        _ => Vec::new(),
    }
}

fn attack_range_to_nbt(range: &AttackRange) -> NbtTag {
    NbtTag::Compound(vec![
        ("min_reach".to_owned(), NbtTag::Float(range.min_reach)),
        ("max_reach".to_owned(), NbtTag::Float(range.max_reach)),
        (
            "min_creative_reach".to_owned(),
            NbtTag::Float(range.min_creative_reach),
        ),
        (
            "max_creative_reach".to_owned(),
            NbtTag::Float(range.max_creative_reach),
        ),
        (
            "hitbox_margin".to_owned(),
            NbtTag::Float(range.hitbox_margin),
        ),
        ("mob_factor".to_owned(), NbtTag::Float(range.mob_factor)),
    ])
}

fn attack_range_from_nbt(tag: &NbtTag) -> AttackRange {
    let mut range = AttackRange::default();
    range.min_reach = nbt_f32(tag.get("min_reach"), range.min_reach);
    range.max_reach = nbt_f32(tag.get("max_reach"), range.max_reach);
    range.min_creative_reach = nbt_f32(tag.get("min_creative_reach"), range.min_creative_reach);
    range.max_creative_reach = nbt_f32(tag.get("max_creative_reach"), range.max_creative_reach);
    range.hitbox_margin = nbt_f32(tag.get("hitbox_margin"), range.hitbox_margin);
    range.mob_factor = nbt_f32(tag.get("mob_factor"), range.mob_factor);
    range
}

fn nbt_f32(tag: Option<&NbtTag>, default: f32) -> f32 {
    tag.and_then(NbtTag::as_f64).map_or(default, |v| v as f32)
}

fn food_to_nbt(food: &Food) -> NbtTag {
    NbtTag::Compound(vec![
        ("nutrition".to_owned(), NbtTag::Int(food.nutrition)),
        ("saturation".to_owned(), NbtTag::Float(food.saturation)),
        (
            "can_always_eat".to_owned(),
            NbtTag::Byte(i8::from(food.can_always_eat)),
        ),
    ])
}

fn food_from_nbt(tag: &NbtTag) -> Option<Food> {
    Some(Food {
        nutrition: tag.get_i32("nutrition")?,
        saturation: nbt_f32(tag.get("saturation"), 0.0),
        can_always_eat: tag.get_bool("can_always_eat").unwrap_or(false),
    })
}

fn consumable_to_nbt(consumable: &Consumable) -> NbtTag {
    let mut entries = vec![
        (
            "consume_seconds".to_owned(),
            NbtTag::Float(consumable.consume_seconds),
        ),
        (
            "animation".to_owned(),
            NbtTag::String(consumable.animation.name().to_owned()),
        ),
        (
            "consume_particles".to_owned(),
            NbtTag::Byte(i8::from(consumable.consume_particles)),
        ),
    ];
    match &consumable.sound {
        SoundRef::Id(id) => {
            entries.push(("sound_id".to_owned(), NbtTag::Int(*id)));
        }
        SoundRef::Named { name, range } => {
            entries.push(("sound_name".to_owned(), NbtTag::String(name.clone())));
            if let Some(range) = range {
                entries.push(("sound_range".to_owned(), NbtTag::Float(*range)));
            }
        }
    }
    if !consumable.on_consume_effects.is_empty() {
        entries.push((
            "on_consume_effects".to_owned(),
            NbtTag::List(consumable.on_consume_effects.clone()),
        ));
    }
    NbtTag::Compound(entries)
}

fn consumable_from_nbt(tag: &NbtTag) -> Option<Consumable> {
    let animation = ConsumeAnimation::from_name(tag.get_str("animation")?)?;
    let sound = if let Some(id) = tag.get_i32("sound_id") {
        SoundRef::Id(id)
    } else {
        SoundRef::Named {
            name: tag.get_str("sound_name")?.to_owned(),
            range: tag
                .get("sound_range")
                .and_then(NbtTag::as_f64)
                .map(|v| v as f32),
        }
    };
    Some(Consumable {
        consume_seconds: nbt_f32(tag.get("consume_seconds"), 1.6),
        animation,
        sound,
        consume_particles: tag.get_bool("consume_particles").unwrap_or(false),
        on_consume_effects: tag.get_list("on_consume_effects").unwrap_or(&[]).to_vec(),
    })
}

/// Read a `components` compound into a patch.
///
/// Unknown keys are preserved (name + value). `_wire/<type_id>` byte arrays
/// attach the wire payload to the matching unknown so re-encode is lossless.
///
/// # Errors
///
/// [`ServerError::CorruptData`] when a modelled key carries the wrong shape.
pub fn from_nbt(tag: &NbtTag) -> ServerResult<ItemComponents> {
    let Some(entries) = tag.entries() else {
        return Err(ServerError::CorruptData(
            "item components root is not a compound".to_owned(),
        ));
    };
    let mut wire_payloads: Vec<(i32, Vec<u8>)> = Vec::new();
    let mut out: Vec<DataComponent> = Vec::new();
    for (key, value) in entries {
        if let Some(id_text) = key.strip_prefix("_wire/") {
            let type_id: i32 = id_text.parse().map_err(|_| {
                ServerError::CorruptData(format!("components key {key:?} is not a wire id"))
            })?;
            let NbtTag::ByteArray(bytes) = value else {
                return Err(ServerError::CorruptData(format!(
                    "components key {key:?} is not a byte array"
                )));
            };
            wire_payloads.push((type_id, bytes.clone()));
            continue;
        }
        match key.as_str() {
            "minecraft:damage" => {
                out.push(DataComponent::Damage(nbt_component_i32(key, value)?));
            }
            "minecraft:max_damage" => {
                out.push(DataComponent::MaxDamage(nbt_component_i32(key, value)?));
            }
            "minecraft:repair_cost" => {
                out.push(DataComponent::RepairCost(nbt_component_i32(key, value)?));
            }
            "minecraft:custom_name" => {
                let NbtTag::String(name) = value else {
                    return Err(ServerError::CorruptData(
                        "minecraft:custom_name is not a string".to_owned(),
                    ));
                };
                out.push(DataComponent::CustomName(name.clone()));
            }
            "minecraft:enchantments" => {
                out.push(DataComponent::Enchantments(enchantments_from_nbt(value)));
            }
            "minecraft:stored_enchantments" => {
                out.push(DataComponent::StoredEnchantments(enchantments_from_nbt(
                    value,
                )));
            }
            "minecraft:attack_range" => {
                out.push(DataComponent::AttackRange(attack_range_from_nbt(value)));
            }
            "minecraft:food" => {
                let food = food_from_nbt(value).ok_or_else(|| {
                    ServerError::CorruptData("minecraft:food is not a compound".to_owned())
                })?;
                out.push(DataComponent::Food(food));
            }
            "minecraft:consumable" => {
                let consumable = consumable_from_nbt(value).ok_or_else(|| {
                    ServerError::CorruptData("minecraft:consumable is not a compound".to_owned())
                })?;
                out.push(DataComponent::Consumable(consumable));
            }
            other => {
                out.push(DataComponent::Unknown {
                    key: other.to_owned(),
                    type_id: 0,
                    wire: Vec::new(),
                    nbt: Some(value.clone()),
                });
            }
        }
    }
    // Attach wire payloads to matching unknowns (or create them when a file
    // only carried `_wire/…`).
    for (type_id, wire) in wire_payloads {
        if let Some(DataComponent::Unknown {
            wire: slot,
            type_id: id,
            ..
        }) = out
            .iter_mut()
            .find(|c| matches!(c, DataComponent::Unknown { type_id: t, .. } if *t == type_id))
        {
            *slot = wire;
            *id = type_id;
        } else if let Some(DataComponent::Unknown {
            wire: slot,
            type_id: id,
            ..
        }) = out.iter_mut().find(|c| {
            matches!(c, DataComponent::Unknown { key, type_id: t, .. } if *t == 0 && key == &format!("#{type_id}"))
        }) {
            *slot = wire;
            *id = type_id;
        } else {
            out.push(unknown_from_wire(type_id, &wire));
        }
    }
    Ok(ItemComponents::from_entries(out))
}

fn nbt_component_i32(key: &str, value: &NbtTag) -> ServerResult<i32> {
    value
        .as_i64()
        .and_then(|v| i32::try_from(v).ok())
        .ok_or_else(|| ServerError::CorruptData(format!("{key} is not an integer")))
}

impl fmt::Display for DataComponent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

#[cfg(test)]
#[allow(
    clippy::float_cmp,
    clippy::needless_borrow,
    clippy::unnecessary_mut_passed,
    reason = "exact f32 fields and a slice cursor the nbt reader advances"
)]
mod tests {
    use super::*;

    #[test]
    fn known_type_ids_match_the_26_1_registry_order() {
        assert_eq!(TYPE_MAX_DAMAGE, 2);
        assert_eq!(TYPE_DAMAGE, 3);
        assert_eq!(TYPE_CUSTOM_NAME, 6);
        assert_eq!(TYPE_ENCHANTMENTS, 13);
        assert_eq!(TYPE_REPAIR_COST, 19);
        assert_eq!(TYPE_FOOD, 23);
        assert_eq!(TYPE_CONSUMABLE, 24);
        assert_eq!(TYPE_ATTACK_RANGE, 30);
        assert_eq!(TYPE_STORED_ENCHANTMENTS, 42);
    }

    #[test]
    fn damage_payload_round_trips() {
        let component = DataComponent::Damage(7);
        let payload = encode_payload(&component).expect("encodes");
        // VarInt 7 is a single 0x07 byte.
        assert_eq!(payload, [0x07]);
        assert_eq!(
            decode_payload(TYPE_DAMAGE, &payload).expect("decodes"),
            component
        );
    }

    #[test]
    fn food_and_consumable_fields_are_public_api() {
        let mut components = ItemComponents::new();
        components.set(DataComponent::Food(Food {
            nutrition: 5,
            saturation: 6.0,
            can_always_eat: false,
        }));
        components.set(DataComponent::Consumable(Consumable {
            consume_seconds: 1.6,
            animation: ConsumeAnimation::Eat,
            sound: SoundRef::Named {
                name: "minecraft:entity.generic.eat".to_owned(),
                range: None,
            },
            consume_particles: true,
            on_consume_effects: Vec::new(),
        }));
        let food = components.food().expect("food is readable");
        assert_eq!(food.nutrition, 5);
        assert_eq!(food.saturation, 6.0);
        assert!(!food.can_always_eat);
        let consumable = components.consumable().expect("consumable is readable");
        assert_eq!(consumable.consume_seconds, 1.6);
        assert_eq!(consumable.animation, ConsumeAnimation::Eat);
        // Perturbation: a changed nutrition is visible through the same API.
        components.set(DataComponent::Food(Food {
            nutrition: 4,
            saturation: 6.0,
            can_always_eat: true,
        }));
        let food = components.food().expect("food still readable");
        assert_eq!(food.nutrition, 4);
        assert!(food.can_always_eat);
    }

    #[test]
    fn unknown_component_survives_save_load() {
        // A stack with a known component and an unknown one. Neutralising the
        // drop path (keeping only known entries on `from_nbt`) fails this test.
        let unknown_payload = vec![0xAA, 0xBB, 0xCC, 0x01];
        let mut components = ItemComponents::new();
        components.set(DataComponent::Damage(3));
        components.set(unknown_from_wire(99, &unknown_payload));
        let nbt = to_nbt(&components).expect("writes a compound");
        let mut bytes = Vec::new();
        mc_nbt::write_unnamed(&nbt, &mut bytes).expect("encodes nbt");
        let mut slice = &bytes[..];
        let decoded_tag =
            mc_nbt::read_unnamed(&mut slice, mc_nbt::Limits::DISK).expect("reads nbt");
        let reloaded = from_nbt(&decoded_tag).expect("decodes components");
        assert_eq!(reloaded.damage(), Some(3));
        let unknown = reloaded
            .unknowns()
            .next()
            .expect("unknown component was not dropped");
        match unknown {
            DataComponent::Unknown { wire, type_id, .. } => {
                assert_eq!(*type_id, 99);
                assert_eq!(
                    wire, &unknown_payload,
                    "wire payload bytes must be identical"
                );
            }
            other => panic!("expected Unknown, got {other:?}"),
        }
        // And the disk NBT is itself byte-stable.
        let again = to_nbt(&reloaded).expect("rewrites");
        let mut bytes2 = Vec::new();
        mc_nbt::write_unnamed(&again, &mut bytes2).expect("encodes nbt");
        assert_eq!(bytes, bytes2, "save→load→save is byte-stable");
    }
}
