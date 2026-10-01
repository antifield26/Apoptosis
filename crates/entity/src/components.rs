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
//!
//! ## Read rule (AUDIT-19 B19-4)
//!
//! The P18 strictness round made `food.saturation` and every `attack_range`
//! field required. That was **backward-incompatible**: a save written before the
//! round omitted nothing its own writer wrote, but it *could* omit a field that
//! build read with a default — and a save format that stops loading its own
//! history is not a save format. The rule, now applied to every modelled key:
//!
//! * **Absent** → the value this crate read before the strict round (the
//!   historical default, reported at `debug` so a legacy file is visible in a
//!   trace). Absence is a property of old files.
//! * **Present but unreadable** → [`ServerError::CorruptData`] naming the
//!   component, the field and the shape found (`minecraft:food.saturation is
//!   TAG_String, expected a number`). A value the file did carry is never
//!   replaced by a guess, and the message is never the wrong shape's name: the
//!   earlier "minecraft:food is not a compound" was said about a compound.
//!
//! [`attach_saved_components`] is the one policy every caller of [`from_nbt`]
//! shares for a patch that cannot be read (B19-5).

use crate::stack::ItemStack;
use mc_core::error::{ServerError, ServerResult};
use mc_nbt::NbtTag;
use std::fmt;
use tracing::debug;

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

/// Vanilla food defaults by registry name (derived).
///
/// Vanilla items carry `minecraft:food` + `minecraft:consumable` as default
/// components. This crate has no item-component defaults yet (same gap
/// `mc-entity::wear::max_damage_of` documents for durability), so this table
/// stands in for them: `(nutrition, saturation, can_always_eat,
/// consume_seconds)`. Values are the jar's, as mirrored by Pumpkin's
/// generated per-item table (`generated/item.rs` `Food` + `Consumable`
/// rows, extracted mechanically — 40 foods found, 39 kept).
///
/// Every carried food uses the eat animation, `generic.eat` sound and
/// consume particles (mirrored), so those fields are fixed here rather than
/// tabulated. Two deliberate exclusions, named not hidden:
/// - `minecraft:honey_bottle` (Drink animation, 2.0 s, different sound):
///   the drink path is unmodelled.
/// - `on_consume_effects` (golden apples, spider eye, pufferfish, …): left
///   empty, so eat-effects from food do not apply. P18-06's effect mechanism
///   exists; wiring per-food effects is a later task.
#[must_use]
#[allow(clippy::match_same_arms, reason = "a data table, not logic")]
pub fn food_defaults_of(item_name: &str) -> Option<(Food, Consumable)> {
    let (nutrition, saturation, can_always_eat, consume_seconds): (i32, f32, bool, f32) =
        match item_name {
            "minecraft:apple" => (4, 2.4, false, 1.6),
            "minecraft:baked_potato" => (5, 6.0, false, 1.6),
            "minecraft:beef" => (3, 1.8, false, 1.6),
            "minecraft:beetroot" => (1, 1.2, false, 1.6),
            "minecraft:beetroot_soup" => (6, 7.2, false, 1.6),
            "minecraft:bread" => (5, 6.0, false, 1.6),
            "minecraft:carrot" => (3, 3.6, false, 1.6),
            "minecraft:chicken" => (2, 1.2, false, 1.6),
            "minecraft:chorus_fruit" => (4, 2.4, true, 1.6),
            "minecraft:cod" => (2, 0.4, false, 1.6),
            "minecraft:cooked_beef" => (8, 12.8, false, 1.6),
            "minecraft:cooked_chicken" => (6, 7.2, false, 1.6),
            "minecraft:cooked_cod" => (5, 6.0, false, 1.6),
            "minecraft:cooked_mutton" => (6, 9.6, false, 1.6),
            "minecraft:cooked_porkchop" => (8, 12.8, false, 1.6),
            "minecraft:cooked_rabbit" => (5, 6.0, false, 1.6),
            "minecraft:cooked_salmon" => (6, 9.6, false, 1.6),
            "minecraft:cookie" => (2, 0.4, false, 1.6),
            "minecraft:dried_kelp" => (1, 0.6, false, 0.8),
            "minecraft:enchanted_golden_apple" => (4, 9.6, true, 1.6),
            "minecraft:glow_berries" => (2, 0.4, false, 1.6),
            "minecraft:golden_apple" => (4, 9.6, true, 1.6),
            "minecraft:golden_carrot" => (6, 14.4, false, 1.6),
            "minecraft:melon_slice" => (2, 1.2, false, 1.6),
            "minecraft:mushroom_stew" => (6, 7.2, false, 1.6),
            "minecraft:mutton" => (2, 1.2, false, 1.6),
            "minecraft:poisonous_potato" => (2, 1.2, false, 1.6),
            "minecraft:porkchop" => (3, 1.8, false, 1.6),
            "minecraft:potato" => (1, 0.6, false, 1.6),
            "minecraft:pufferfish" => (1, 0.2, false, 1.6),
            "minecraft:pumpkin_pie" => (8, 4.8, false, 1.6),
            "minecraft:rabbit" => (3, 1.8, false, 1.6),
            "minecraft:rabbit_stew" => (10, 12.0, false, 1.6),
            "minecraft:rotten_flesh" => (4, 0.8, false, 1.6),
            "minecraft:salmon" => (2, 0.4, false, 1.6),
            "minecraft:spider_eye" => (2, 3.2, false, 1.6),
            "minecraft:suspicious_stew" => (6, 7.2, true, 1.6),
            "minecraft:sweet_berries" => (2, 0.4, false, 1.6),
            "minecraft:tropical_fish" => (1, 0.2, false, 1.6),
            _ => return None,
        };
    Some((
        Food {
            nutrition,
            saturation,
            can_always_eat,
        },
        Consumable {
            consume_seconds,
            animation: ConsumeAnimation::Eat,
            sound: SoundRef::Named {
                name: "minecraft:entity.generic.eat".to_owned(),
                range: None,
            },
            consume_particles: true,
            on_consume_effects: Vec::new(),
        },
    ))
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

fn attack_range_from_nbt(tag: &NbtTag) -> ServerResult<AttackRange> {
    // B19-4: absent fields read the default this crate used before the strict
    // round — the pre-fix writer could omit them, and a file that loaded then
    // must still load. A field that *is* there but is not a number is corrupt.
    let component = "minecraft:attack_range";
    if tag.entries().is_none() {
        return Err(ServerError::CorruptData(format!(
            "{component} is {}, expected a compound",
            tag.type_name()
        )));
    }
    let default = AttackRange::default();
    Ok(AttackRange {
        min_reach: legacy_f32(tag, component, "min_reach", default.min_reach)?,
        max_reach: legacy_f32(tag, component, "max_reach", default.max_reach)?,
        min_creative_reach: legacy_f32(
            tag,
            component,
            "min_creative_reach",
            default.min_creative_reach,
        )?,
        max_creative_reach: legacy_f32(
            tag,
            component,
            "max_creative_reach",
            default.max_creative_reach,
        )?,
        hitbox_margin: legacy_f32(tag, component, "hitbox_margin", default.hitbox_margin)?,
        mob_factor: legacy_f32(tag, component, "mob_factor", default.mob_factor)?,
    })
}

/// A float field of a component compound, with the pre-strict default for an
/// absent one (AUDIT-19 B19-4).
///
/// The default is not a guess: it is the value this crate read for that field
/// before the P18 round made it required, so a save written by that build loads
/// under the same numbers it was played with. The `debug` line is what keeps a
/// legacy file from being silent — the read succeeded, but it was completed.
fn legacy_f32(tag: &NbtTag, component: &str, field: &str, legacy: f32) -> ServerResult<f32> {
    let Some(value) = tag.get(field) else {
        debug!(
            component,
            field, legacy, "a saved component omits this field; using the pre-strict default"
        );
        return Ok(legacy);
    };
    value.as_f64().map(|v| v as f32).ok_or_else(|| {
        ServerError::CorruptData(format!(
            "{component}.{field} is {}, expected a number",
            value.type_name()
        ))
    })
}

/// An integer field of a component compound that every writer has always
/// written, so its absence is corruption rather than an old layout.
fn required_component_i32(tag: &NbtTag, component: &str, field: &str) -> ServerResult<i32> {
    let Some(value) = tag.get(field) else {
        return Err(ServerError::CorruptData(format!(
            "{component}.{field} is missing"
        )));
    };
    value
        .as_i64()
        .and_then(|v| i32::try_from(v).ok())
        .ok_or_else(|| {
            ServerError::CorruptData(format!(
                "{component}.{field} is {}, expected an integer",
                value.type_name()
            ))
        })
}

/// A string field of a component compound that every writer has always written.
fn required_component_str<'a>(
    tag: &'a NbtTag,
    component: &str,
    field: &str,
) -> ServerResult<&'a str> {
    let Some(value) = tag.get(field) else {
        return Err(ServerError::CorruptData(format!(
            "{component}.{field} is missing"
        )));
    };
    match value {
        NbtTag::String(text) => Ok(text),
        other => Err(ServerError::CorruptData(format!(
            "{component}.{field} is {}, expected a string",
            other.type_name()
        ))),
    }
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

fn food_from_nbt(tag: &NbtTag) -> ServerResult<Food> {
    // B19-4: `nutrition` was required before the strict round too, so its
    // absence is corruption; `saturation` was read with a default, so it stays
    // optional and the message below can no longer claim a compound is not one.
    let component = "minecraft:food";
    if tag.entries().is_none() {
        return Err(ServerError::CorruptData(format!(
            "{component} is {}, expected a compound",
            tag.type_name()
        )));
    }
    Ok(Food {
        nutrition: required_component_i32(tag, component, "nutrition")?,
        saturation: legacy_f32(tag, component, "saturation", 0.0)?,
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

fn consumable_from_nbt(tag: &NbtTag) -> ServerResult<Consumable> {
    // Same read rule as `food`: `animation` and the sound have always been
    // required; the rest carries its pre-strict default. The messages name the
    // field, so "minecraft:consumable is not a compound" is no longer said
    // about a compound that is merely missing one.
    let component = "minecraft:consumable";
    if tag.entries().is_none() {
        return Err(ServerError::CorruptData(format!(
            "{component} is {}, expected a compound",
            tag.type_name()
        )));
    }
    let animation_name = required_component_str(tag, component, "animation")?;
    let animation = ConsumeAnimation::from_name(animation_name).ok_or_else(|| {
        ServerError::CorruptData(format!(
            "{component}.animation is {animation_name:?}, which is not a consume animation"
        ))
    })?;
    let sound = if let Some(id) = tag.get_i32("sound_id") {
        SoundRef::Id(id)
    } else {
        let name = required_component_str(tag, component, "sound_name")?.to_owned();
        let range = match tag.get("sound_range") {
            None => None,
            Some(value) => Some(value.as_f64().map(|v| v as f32).ok_or_else(|| {
                ServerError::CorruptData(format!(
                    "{component}.sound_range is {}, expected a number",
                    value.type_name()
                ))
            })?),
        };
        SoundRef::Named { name, range }
    };
    Ok(Consumable {
        consume_seconds: legacy_f32(tag, component, "consume_seconds", 1.6)?,
        animation,
        sound,
        // B-M4: absent means vanilla-default true (what every encoder
        // writes), not false.
        consume_particles: tag.get_bool("consume_particles").unwrap_or(true),
        on_consume_effects: tag.get_list("on_consume_effects").unwrap_or(&[]).to_vec(),
    })
}

/// Read a `components` compound into a patch.
///
/// Unknown keys are preserved (name + value). `_wire/<type_id>` byte arrays
/// attach the wire payload to the matching unknown so re-encode is lossless.
///
/// The **read rule** is on this module: a field an older build could omit reads
/// that build's default (so a pre-strict save still loads), and a field that is
/// present but unreadable is refused with a message that names the component,
/// the field and the shape found (AUDIT-19 B19-4).
///
/// # Errors
///
/// [`ServerError::CorruptData`] when a modelled key carries a value this build
/// cannot read. The message names the key and the cause; a caller that must not
/// fail a larger load over one patch uses [`attach_saved_components`].
#[allow(
    clippy::too_many_lines,
    reason = "one match arm per component key; splitting it would scatter the key dispatch the strictness review (B-M4) reads as one table"
)]
pub fn from_nbt(tag: &NbtTag) -> ServerResult<ItemComponents> {
    let Some(entries) = tag.entries() else {
        return Err(ServerError::CorruptData(format!(
            "item components root is {}, expected a compound",
            tag.type_name()
        )));
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
                match value {
                    NbtTag::List(_) => {
                        out.push(DataComponent::Enchantments(enchantments_from_nbt(value)));
                    }
                    // B-M4: vanilla's name-keyed map shape is preserved as an
                    // unknown, not flattened into an empty (and wrong)
                    // "no enchantments".
                    other => out.push(DataComponent::Unknown {
                        key: "minecraft:enchantments".to_owned(),
                        type_id: 0,
                        wire: Vec::new(),
                        nbt: Some(other.clone()),
                    }),
                }
            }
            "minecraft:stored_enchantments" => match value {
                NbtTag::List(_) => {
                    out.push(DataComponent::StoredEnchantments(enchantments_from_nbt(
                        value,
                    )));
                }
                other => out.push(DataComponent::Unknown {
                    key: "minecraft:stored_enchantments".to_owned(),
                    type_id: 0,
                    wire: Vec::new(),
                    nbt: Some(other.clone()),
                }),
            },
            "minecraft:attack_range" => {
                out.push(DataComponent::AttackRange(attack_range_from_nbt(value)?));
            }
            "minecraft:food" => {
                out.push(DataComponent::Food(food_from_nbt(value)?));
            }
            "minecraft:consumable" => {
                out.push(DataComponent::Consumable(consumable_from_nbt(value)?));
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
        } else if let [only] = out
            .iter_mut()
            .filter(|c| {
                matches!(c, DataComponent::Unknown { type_id: 0, .. })
            })
            .collect::<Vec<_>>()
            .as_mut_slice()
        {
            // Exactly one nameless-id unknown: the wire payload must be its
            // other half (a named file entry plus its `_wire/<id>`), so attach
            // rather than pushing a second component (B-M3). Ambiguous
            // (several) stays separate — no guessing.
            if let DataComponent::Unknown {
                wire: slot,
                type_id: id,
                ..
            } = only
            {
                *slot = wire;
                *id = type_id;
            }
        } else {
            out.push(unknown_from_wire(type_id, &wire));
        }
    }
    Ok(ItemComponents::from_entries(out))
}

/// Attach a saved `components` compound to a stack under the **one** policy
/// every disk reader shares (AUDIT-19 B19-5).
///
/// A patch that cannot be read does not fail the load around it. The same
/// [`from_nbt`] used to get two different answers: the player's inventory
/// propagated the error, so one unreadable patch discarded the whole
/// `playerdata` file, while a block entity warned and kept a component-free
/// stack. Both keep the stack now — losing a slot's identity, count and every
/// other component over one unreadable component is the larger loss — and both
/// get the reason, which names the component and what was wrong with it, so the
/// outcome and the message cannot drift apart again.
///
/// Returns the stack to keep and, when the patch could not be read, the reason
/// the caller must report. `#[must_use]` on the tuple because dropping the
/// reason silently is exactly the failure this replaced.
#[must_use]
pub fn attach_saved_components(
    mut stack: ItemStack,
    components_tag: &NbtTag,
) -> (ItemStack, Option<ServerError>) {
    match from_nbt(components_tag) {
        Ok(components) => {
            if !stack.is_empty() {
                *stack.components_mut() = components;
            }
            (stack, None)
        }
        Err(reason) => (stack, Some(reason)),
    }
}

fn nbt_component_i32(key: &str, value: &NbtTag) -> ServerResult<i32> {
    value
        .as_i64()
        .and_then(|v| i32::try_from(v).ok())
        .ok_or_else(|| {
            ServerError::CorruptData(format!(
                "{key} is {}, expected an integer",
                value.type_name()
            ))
        })
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
    fn food_defaults_mirror_the_jar_table() {
        // Bread is the walk food: 5 nutrition, 6.0 saturation, 1.6 s eat.
        let (food, consumable) = food_defaults_of("minecraft:bread").expect("bread is food");
        assert_eq!(food.nutrition, 5);
        assert!((food.saturation - 6.0).abs() < 1e-6);
        assert!(!food.can_always_eat);
        assert!((consumable.consume_seconds - 1.6).abs() < 1e-6);
        assert_eq!(consumable.animation, ConsumeAnimation::Eat);
        // Spot rows across the table shape: always-eat, fast eat, roast.
        assert!(
            food_defaults_of("minecraft:chorus_fruit").is_some_and(|(food, _)| food.can_always_eat)
        );
        assert!(
            food_defaults_of("minecraft:dried_kelp").is_some_and(|(_, consumable)| (consumable
                .consume_seconds
                - 0.8)
                .abs()
                < 1e-6)
        );
        assert_eq!(
            food_defaults_of("minecraft:cooked_beef").map(|(food, _)| food.nutrition),
            Some(8)
        );
        // Not food, and the deliberately excluded drink.
        assert!(food_defaults_of("minecraft:stone").is_none());
        assert!(food_defaults_of("minecraft:wooden_shovel").is_none());
        assert!(food_defaults_of("minecraft:honey_bottle").is_none());
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
    fn named_unknown_plus_wire_merges_into_one_component() {
        // B-M3: a file entry with a real name plus its `_wire/<id>` payload
        // is one logical component. Splitting it into a named half and a
        // `#id` half (the old behaviour) fails this test.
        let tag = NbtTag::Compound(vec![
            ("minecraft:foo".to_owned(), NbtTag::Int(1)),
            ("_wire/99".to_owned(), NbtTag::ByteArray(vec![0xAA])),
        ]);
        let components = from_nbt(&tag).expect("decodes");
        let unknowns: Vec<_> = components.unknowns().collect();
        assert_eq!(unknowns.len(), 1, "one logical component, not two");
        match unknowns[0] {
            DataComponent::Unknown {
                key, type_id, wire, ..
            } => {
                assert_eq!(key, "minecraft:foo");
                assert_eq!(*type_id, 99);
                assert_eq!(wire, &[0xAA]);
            }
            other => panic!("expected Unknown, got {other:?}"),
        }
    }

    #[test]
    fn disk_reads_are_strict_or_unknown_preserving() {
        // B-M4: malformed modelled components neither heal silently nor
        // flatten into wrong empties.
        //
        // 1. A vanilla name-keyed enchant map is preserved as an unknown,
        //    not converted to an empty (and wrong) "no enchantments".
        let map = NbtTag::Compound(vec![(
            "minecraft:enchantments".to_owned(),
            NbtTag::Compound(vec![("minecraft:sharpness".to_owned(), NbtTag::Int(2))]),
        )]);
        let components = from_nbt(&map).expect("decodes");
        assert!(
            components.enchantments().is_none(),
            "no typed enchantments may be invented from names"
        );
        assert_eq!(
            components.unknowns().count(),
            1,
            "the map survives as one unknown"
        );
        //
        // 2. A partial attack_range is **not** the wrong-empty case: it is a
        //    pre-strict save shape. Every missing field reads the default this
        //    crate used before the P18 round; only a field that is present and
        //    unreadable is refused. See
        //    `a_save_written_before_the_strict_round_still_loads`.
        let partial = NbtTag::Compound(vec![(
            "minecraft:attack_range".to_owned(),
            NbtTag::Compound(vec![("max_reach".to_owned(), NbtTag::Float(5.0))]),
        )]);
        let range = from_nbt(&partial)
            .expect("a pre-strict attack_range still loads")
            .attack_range()
            .cloned()
            .expect("attack_range");
        assert_eq!(range.max_reach, 5.0);
        assert_eq!(
            range.min_reach,
            AttackRange::default().min_reach,
            "the absent fields read the pre-strict default, not a typed empty"
        );
        let unreadable = NbtTag::Compound(vec![(
            "minecraft:attack_range".to_owned(),
            NbtTag::Compound(vec![(
                "max_reach".to_owned(),
                NbtTag::String("far".to_owned()),
            )]),
        )]);
        let error = from_nbt(&unreadable).expect_err("a string is not a reach");
        assert!(
            error
                .to_string()
                .contains("minecraft:attack_range.max_reach"),
            "{error}"
        );
        //
        // 3. A consumable without `consume_particles` reads the
        // vanilla-default true that every encoder writes.
        let bare = NbtTag::Compound(vec![(
            "minecraft:consumable".to_owned(),
            NbtTag::Compound(vec![
                ("consume_seconds".to_owned(), NbtTag::Float(1.6)),
                ("animation".to_owned(), NbtTag::String("eat".to_owned())),
                (
                    "sound_name".to_owned(),
                    NbtTag::String("minecraft:entity.generic.eat".to_owned()),
                ),
            ]),
        )]);
        let components = from_nbt(&bare).expect("decodes");
        assert!(
            components
                .consumable()
                .expect("consumable")
                .consume_particles,
            "absent particles flag reads true"
        );
        //
        // 4. Food without saturation is a pre-strict save shape too: it loads
        //    with the zero this crate read for it, and the B19-4 pin for the
        //    other half of the rule is
        //    `an_unreadable_component_names_the_component_and_the_cause`.
        let thin = NbtTag::Compound(vec![(
            "minecraft:food".to_owned(),
            NbtTag::Compound(vec![("nutrition".to_owned(), NbtTag::Int(5))]),
        )]);
        let food = from_nbt(&thin)
            .expect("a pre-strict food still loads")
            .food()
            .cloned()
            .expect("food");
        assert_eq!(food.nutrition, 5);
        assert_eq!(
            food.saturation, 0.0,
            "the saturation this crate read before it was made required"
        );
        assert!(
            from_nbt(&NbtTag::Compound(vec![(
                "minecraft:food".to_owned(),
                NbtTag::Compound(vec![(
                    "nutrition".to_owned(),
                    NbtTag::String("5".to_owned())
                )]),
            )]))
            .is_err(),
            "a nutrition that is not an integer is still corruption"
        );
    }

    #[test]
    fn a_save_written_before_the_strict_round_still_loads() {
        // AUDIT-19 B19-4, the compatibility half. The P18 strictness round made
        // `food.saturation` and all six `attack_range` fields required; a save
        // written before it omits exactly those fields (the writer of the day
        // wrote them, but the reader of the day defaulted them, which is what
        // "an old save" means here). Requiring them refuses the server's own
        // history, so the pre-strict default is the rule for an absent field.
        let pre_fix_food = NbtTag::Compound(vec![(
            "minecraft:food".to_owned(),
            NbtTag::Compound(vec![
                ("nutrition".to_owned(), NbtTag::Int(6)),
                ("can_always_eat".to_owned(), NbtTag::Byte(1)),
            ]),
        )]);
        let food = from_nbt(&pre_fix_food)
            .expect("a food compound without saturation loads")
            .food()
            .cloned()
            .expect("food");
        assert_eq!(food.nutrition, 6);
        assert_eq!(food.saturation, 0.0, "the pre-strict default");
        assert!(food.can_always_eat);

        let pre_fix_range = NbtTag::Compound(vec![(
            "minecraft:attack_range".to_owned(),
            NbtTag::Compound(vec![
                ("max_reach".to_owned(), NbtTag::Float(4.5)),
                ("mob_factor".to_owned(), NbtTag::Float(2.0)),
            ]),
        )]);
        let range = from_nbt(&pre_fix_range)
            .expect("a partial attack_range loads")
            .attack_range()
            .cloned()
            .expect("attack_range");
        assert_eq!(range.max_reach, 4.5);
        assert_eq!(range.mob_factor, 2.0);
        let default = AttackRange::default();
        assert_eq!(range.min_reach, default.min_reach);
        assert_eq!(range.min_creative_reach, default.min_creative_reach);
        assert_eq!(range.max_creative_reach, default.max_creative_reach);
        assert_eq!(range.hitbox_margin, default.hitbox_margin);
    }

    #[test]
    fn an_unreadable_component_names_the_component_and_the_cause() {
        // AUDIT-19 B19-4, the message half. "minecraft:food is not a compound"
        // was said about a **compound** that was merely missing a field, so an
        // operator reading the log was told the wrong shape. Every refusal now
        // names the component, the field and what the file actually carried.
        let missing_nutrition = NbtTag::Compound(vec![(
            "minecraft:food".to_owned(),
            NbtTag::Compound(vec![("saturation".to_owned(), NbtTag::Float(1.0))]),
        )]);
        let error = from_nbt(&missing_nutrition).expect_err("no nutrition");
        assert!(
            error
                .to_string()
                .contains("minecraft:food.nutrition is missing"),
            "{error}"
        );

        let wrong_shape = NbtTag::Compound(vec![(
            "minecraft:food".to_owned(),
            NbtTag::Compound(vec![
                ("nutrition".to_owned(), NbtTag::Int(5)),
                ("saturation".to_owned(), NbtTag::String("lots".to_owned())),
            ]),
        )]);
        let error = from_nbt(&wrong_shape).expect_err("saturation is a string");
        let text = error.to_string();
        assert!(text.contains("minecraft:food.saturation"), "{text}");
        assert!(text.contains("TAG_String"), "names the shape: {text}");
        assert!(text.contains("expected a number"), "{text}");

        let not_a_compound = NbtTag::Compound(vec![(
            "minecraft:food".to_owned(),
            NbtTag::String("minecraft:bread".to_owned()),
        )]);
        let error = from_nbt(&not_a_compound).expect_err("food must be a compound");
        let text = error.to_string();
        assert!(text.contains("minecraft:food is TAG_String"), "{text}");
        assert!(text.contains("expected a compound"), "{text}");

        // `consumable` had the same false message for the same reason.
        let consumable_missing_animation = NbtTag::Compound(vec![(
            "minecraft:consumable".to_owned(),
            NbtTag::Compound(vec![("consume_seconds".to_owned(), NbtTag::Float(1.6))]),
        )]);
        let error = from_nbt(&consumable_missing_animation).expect_err("no animation");
        assert!(
            error
                .to_string()
                .contains("minecraft:consumable.animation is missing"),
            "{error}"
        );
    }

    #[test]
    fn an_unreadable_patch_keeps_the_stack_and_reports_the_component() {
        // AUDIT-19 B19-5, the shared policy. The player's inventory refused the
        // whole file over one unreadable patch while a block entity warned and
        // kept a component-free stack; both now keep the stack and get the
        // reason, which names the component. Dropping the stack, or returning
        // `None` for the reason, is what this test is for.
        let diamond = 899;
        let stack = ItemStack::new(diamond, 3).expect("three diamonds");
        let unreadable = NbtTag::Compound(vec![(
            "minecraft:food".to_owned(),
            NbtTag::Compound(vec![
                ("nutrition".to_owned(), NbtTag::Int(4)),
                ("saturation".to_owned(), NbtTag::String("lots".to_owned())),
            ]),
        )]);
        let (kept, reason) = attach_saved_components(stack.clone(), &unreadable);
        assert_eq!(
            kept.item_id(),
            Some(diamond),
            "the stack survives its patch"
        );
        assert_eq!(kept.count(), 3, "and keeps its count");
        assert!(
            kept.components().is_empty(),
            "the unreadable patch is not half-applied"
        );
        let reason = reason.expect("the reason is the only signal that it was dropped");
        assert!(
            reason.to_string().contains("minecraft:food.saturation"),
            "the reason names the component and the field: {reason}"
        );

        // The readable half: a patch that decodes still attaches, so the
        // policy did not become "never read components".
        let readable = NbtTag::Compound(vec![("minecraft:damage".to_owned(), NbtTag::Int(7))]);
        let (patched, reason) = attach_saved_components(stack, &readable);
        assert!(reason.is_none(), "a readable patch has nothing to report");
        assert_eq!(patched.damage(), Some(7));
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
