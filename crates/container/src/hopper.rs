//! Hopper transfer: the pure item move, with conservation by construction
//! (P06-08).
//!
//! ## Scope — what this module deliberately does not do
//!
//! This is **only** the transfer. It does not schedule anything, does not know
//! what a block entity is, does not read or write the world, does not implement
//! the "a hopper pulls from the container above and pushes into the one below"
//! wiring, and does not send packets. Vanilla's hopper runs a 1-tick "transfer
//! cooldown" and, when empty, a separate 8-tick cooldown before it tries to pull
//! (see [`HOPPER_TRANSFER_COOLDOWN_TICKS`]); running that timer, deciding *when*
//! a hopper ticks and finding the neighbour containers are the block-entity and
//! simulation layer's job — **P06-07** (block entities) and **P06-10** (the tick
//! scheduler), not this module.
//!
//! ## Conservation is the property that matters
//!
//! A hopper move must never create or destroy an item, and this module gets that
//! by construction rather than by testing alone:
//!
//! 1. The room in the destination is computed **before** anything is taken from
//!    the source, and the amount to move is `min(per_transfer, source count,
//!    room)`. Zero room means nothing is taken.
//! 2. The only mutation is `source -= taken` followed by `destination += taken` of
//!    the same stack, and both are written through [`Container::set`], so a
//!    rejected write is an error rather than a partial move.
//! 3. Every failure path returns before either `set` call, leaving both containers
//!    untouched.
//!
//! ## Slot roles
//!
//! A hopper must not pull from a furnace's input/fuel slots
//! ([`SlotRole::may_hopper_extract`]) and must not push into a
//! computed slot ([`SlotRole::CraftingResult`] and [`SlotRole::FurnaceOutput`] are
//! take-only, because the server computes them: a hopper pushing into one would be
//! creating items). Both containers therefore take a parallel `&[SlotRole]` slice
//! — the shape a menu already has in
//! [`SlotMapping::role`](crate::SlotMapping::role) — rather than assuming every
//! slot is [`SlotRole::Storage`]. A slice shorter than its container means the
//! missing roles are [`SlotRole::Storage`]; pass
//! [`SlotRole::FurnaceInput`]/[`SlotRole::FurnaceFuel`] for a furnace and only the
//! input and fuel slots are push targets, which is Vanilla's behaviour.
//!
//! ## Stack limits
//!
//! `per_transfer` is clamped to the Vanilla default of
//! [`mc_entity::stack::DEFAULT_MAX_STACK_SIZE`] and the destination is only filled
//! to that, because a `&mut Container` carries no item registry and therefore no
//! per-item limit. A caller that knows a slot's true limit (a furnace output of
//! buckets) must pass a `per_transfer` no larger than that limit; the transfer
//! never exceeds it. Passing a registry in would be the alternative and is
//! deliberately not done here — see the module's report note about the
//! signature.
//!
//! ## Determinism (AGENTS.md section 3.6)
//!
//! Sources and destinations are walked in slot order, nothing is shuffled, and
//! there is no clock or randomness. The same containers plus the same
//! `per_transfer` always produce the same move.

#![deny(missing_docs)]

use mc_core::error::{ServerError, ServerResult};
use mc_entity::stack::ItemStack;

use crate::container::{Container, SlotRole};

/// Ticks a hopper waits after a successful transfer before moving again
/// (**product decision**, labelled because it is not a verified Vanilla number).
///
/// Vanilla's `HopperBlockEntity` sets a `MOVE_ITEM_SPEED` cooldown of **8 ticks**
/// (0.4 s, i.e. 2.5 items/second) after a transfer, and a hopper that fails to
/// pull anything gets a `max(8, ...)`-style delay derived from its loot-table
/// "cooldown" — the second figure is the one this build has **not** verified.
/// Neither value is used by this module (there is no scheduler here); the constant
/// exists so the caller that does own the timer
///(P06-07/P06-10) has one labelled place to read it from.
///
/// **Unverified:** the exact "empty hopper pulls every N ticks" behaviour, and the
/// 4-tick cooldown a hopper takes when it pushes into a furnace it just took from.
pub const HOPPER_TRANSFER_COOLDOWN_TICKS: u32 = 8;

/// The most items one hopper transfer may move.
///
/// Vanilla moves one item per transfer per slot pair; the wider form here exists
/// because "move one" is a scheduling decision (`per_transfer = 1`) rather than a
/// property of the move. The ceiling is
/// [`mc_entity::stack::DEFAULT_MAX_STACK_SIZE`].
pub const MAX_ITEMS_PER_TRANSFER: i32 = mc_entity::stack::DEFAULT_MAX_STACK_SIZE;

/// What one hopper transfer did.
///
/// An empty transfer is `moved == 0` with both slots `None`; it is not an error,
/// because "there is nothing to move" is the normal state of a hopper.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct HopperTransfer {
    /// Items moved. Never negative, never more than the call's `per_transfer`.
    pub moved: i32,
    /// Source slot the items came out of.
    pub source_slot: Option<u16>,
    /// Destination slot they went into.
    pub destination_slot: Option<u16>,
}

impl HopperTransfer {
    /// A transfer that moved nothing.
    #[must_use]
    pub const fn none() -> Self {
        Self {
            moved: 0,
            source_slot: None,
            destination_slot: None,
        }
    }

    /// Whether anything moved.
    #[must_use]
    pub const fn moved_anything(&self) -> bool {
        self.moved > 0
    }
}

/// One source slot the hopper may pull from.
#[derive(Debug, Clone, PartialEq, Eq)]
struct PullCandidate {
    index: usize,
    stack: ItemStack,
}

/// A hopper's pure transfer.
///
/// The type carries no state: Vanilla's hopper state (its cooldown, its position)
/// belongs to the block entity.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Hopper;

impl Hopper {
    /// Move at most `per_transfer` items from the first acceptable source slot to
    /// the first acceptable destination slot that has room.
    ///
    /// Source and destination are separate containers because that is what a
    /// hopper does (pull from above, push below) and because a single container
    /// with two role slices would let a caller alias one slot as both ends.
    /// One call performs **one** move: at most one source slot and one destination
    /// slot change, so the caller can report both to the client. Choose the
    /// destination before the source to model a hopper pulling from the container
    /// above (Vanilla tries "take from above" and "push below" in the same tick;
    /// this call does one of them).
    ///
    /// `per_transfer` is clamped into `0..=`[`MAX_ITEMS_PER_TRANSFER`]: `0` or a
    /// negative value moves nothing, and an absurd value moves at most a full
    /// stack. Neither is an error.
    ///
    /// # Errors
    ///
    /// [`ServerError::InvalidAction`] if a write is refused (impossible for an
    /// index taken from the container itself) — a caller can treat that as an
    /// invariant violation, and in every such case neither container has been
    /// changed.
    pub fn transfer(
        source: &mut Container,
        source_roles: &[SlotRole],
        destination: &mut Container,
        destination_roles: &[SlotRole],
        per_transfer: i32,
    ) -> ServerResult<HopperTransfer> {
        let candidates = pull_candidates(source, source_roles, per_transfer);
        if candidates.is_empty() {
            return Ok(HopperTransfer::none());
        }
        for candidate in candidates {
            let Some((destination_slot, room)) =
                push_target(destination, destination_roles, &candidate.stack)
            else {
                continue;
            };
            let moved = per_transfer.min(candidate.stack.count()).min(room);
            if moved <= 0 {
                continue;
            }
            let mut from = candidate.stack.clone();
            let mut taken = from.split(moved);
            let moved = taken.count();
            if moved <= 0 {
                continue;
            }
            let mut to = destination.get(destination_slot);
            // `room` was computed above and `moved <= room`, so this consumes all
            // of `taken`; the return value is the shortfall, which must be zero.
            let shortfall = to.merge_capped(&mut taken, MAX_ITEMS_PER_TRANSFER);
            if shortfall != 0 || !taken.is_empty() {
                // Unreachable with the room computed above. Returning an error
                // rather than continuing keeps "conservation" from depending on
                // an argument about arithmetic: neither container has been written
                // yet, so nothing has moved and nothing can be lost.
                return Err(ServerError::Invariant(format!(
                    "a hopper computed room for {moved} items but the destination refused \
                     {shortfall}"
                )));
            }
            source.set(candidate.index, from)?;
            destination.set(destination_slot, to)?;
            return Ok(HopperTransfer {
                moved,
                source_slot: Some(slot_index(candidate.index)),
                destination_slot: Some(slot_index(destination_slot)),
            });
        }
        Ok(HopperTransfer::none())
    }

    /// [`Hopper::transfer`] over containers whose slots are all
    /// [`SlotRole::Storage`].
    ///
    /// The role-aware form is the primary one; this exists because a chest-to-chest
    /// move is common and spelling out two role slices for it is noise.
    ///
    /// # Errors
    ///
    /// As for [`Hopper::transfer`].
    pub fn transfer_storage(
        source: &mut Container,
        destination: &mut Container,
        per_transfer: i32,
    ) -> ServerResult<HopperTransfer> {
        Self::transfer(source, &[], destination, &[], per_transfer)
    }
}

/// The role at `index`, defaulting to [`SlotRole::Storage`] past the end of
/// `roles`.
fn role_at(roles: &[SlotRole], index: usize) -> SlotRole {
    roles.get(index).copied().unwrap_or(SlotRole::Storage)
}

/// A container's index as a wire slot number, saturating rather than wrapping.
fn slot_index(index: usize) -> u16 {
    u16::try_from(index).unwrap_or(u16::MAX)
}

/// The clamps `per_transfer` into `0..=MAX_ITEMS_PER_TRANSFER`.
fn wanted(per_transfer: i32) -> i32 {
    if per_transfer <= 0 {
        0
    } else {
        per_transfer.min(MAX_ITEMS_PER_TRANSFER)
    }
}

/// Every source slot the hopper may pull from, in slot order.
///
/// A slot qualifies when its role allows hopper extraction
/// ([`SlotRole::may_hopper_extract`] — furnace input/fuel refuse, so a
/// hopper above a furnace takes the output only) and it holds items. The
/// list is materialised before any mutation so that a destination search
/// that fails for one candidate can fall through to the next without
/// re-reading a container that has not changed.
fn pull_candidates(
    source: &Container,
    source_roles: &[SlotRole],
    per_transfer: i32,
) -> Vec<PullCandidate> {
    let limit = wanted(per_transfer);
    if limit == 0 {
        return Vec::new();
    }
    source
        .non_empty()
        .filter(|(index, _)| role_at(source_roles, *index).may_hopper_extract())
        .map(|(index, stack)| PullCandidate { index, stack })
        .collect()
}

/// The first destination slot that can accept `stack`, with its room.
///
/// A slot qualifies when its role allows placement, it is empty or holds the same
/// item, and it has at least one item of room. `None` means the items stay where
/// they are.
/// Pick the destination slot and how much of `stack` it can take.
///
/// Precondition (Audit 08, L2): the room computed here assumes every
/// destination slot accepts at least [`MAX_ITEMS_PER_TRANSFER`] — which holds
/// for hopper slots (64-stack) — because a `Container` does not carry a
/// per-slot limit the way a menu's slot mapping does. A caller pointing a
/// hopper at limited slots would over-report room and trip the invariant
/// error in [`transfer`].
fn push_target(
    destination: &Container,
    destination_roles: &[SlotRole],
    stack: &ItemStack,
) -> Option<(usize, i32)> {
    for (index, existing) in destination.slots().iter().enumerate() {
        if !role_at(destination_roles, index).may_place() {
            continue;
        }
        let room = if existing.is_empty() {
            MAX_ITEMS_PER_TRANSFER
        } else if existing.same_item(stack) {
            MAX_ITEMS_PER_TRANSFER - existing.count()
        } else {
            0
        };
        if room > 0 {
            return Some((index, room));
        }
    }
    None
}

#[cfg(test)]
mod tests;
