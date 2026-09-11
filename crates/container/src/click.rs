//! Click decoding and validation (P06-02).
//!
//! A `container_click` packet carries five integers, all of them
//! client-controlled:
//!
//! | field | wire | meaning |
//! |---|---|---|
//! | `window_id` | `i32` (`VarInt`) | which open container this refers to (0 = player inventory) |
//! | `state_id` | `i32` (`VarInt`) | the server's revision counter the client believes it has |
//! | `slot` | `i16` | menu slot index, or −1 for "outside the window" |
//! | `button` | `i8` | meaning depends on the click type |
//! | `click_type` | `i32` (`VarInt`) | which gesture |
//!
//! [`Click::new`] is the single place those integers become a typed value, and the
//! only place that decides whether they are legal *in principle*. Whether they are
//! legal *for the current menu state* is [`crate::Menu`]'s job, because only the
//! menu knows its slot count. Splitting it that way means the packet layer cannot
//! accidentally accept a `click_type` it does not implement, and the menu cannot
//! accidentally reinterpret button bits per click type.

use mc_core::error::{ServerError, ServerResult};

use crate::{HARD_MAX_COUNT, SLOT_OUTSIDE};

/// Which gesture a click represents (Vanilla `ClickType`).
///
/// Ids are the wire values and are part of the protocol, not an internal enum
/// ordering choice.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ClickType {
    /// 0 — pick up / put down. `button` 0 = whole stack, 1 = one item (or half).
    Pickup,
    /// 1 — shift-click: move the stack to the menu's transfer destination.
    QuickMove,
    /// 2 — number key: swap the clicked slot with a hotbar slot (`button`), or
    /// with the offhand when `button` is 40.
    Swap,
    /// 3 — middle click: creative-only duplication of the clicked slot.
    Clone,
    /// 4 — throw. `button` 0 = one item, 1 = the whole stack.
    Throw,
    /// 5 — drag: `button` packs the drag type and the stage together.
    QuickCraft,
    /// 6 — double-click: gather every matching stack into the cursor.
    PickupAll,
}

impl ClickType {
    /// All click types, in id order.
    pub const ALL: [Self; 7] = [
        Self::Pickup,
        Self::QuickMove,
        Self::Swap,
        Self::Clone,
        Self::Throw,
        Self::QuickCraft,
        Self::PickupAll,
    ];

    /// Recognise a wire id.
    #[must_use]
    pub const fn from_id(id: i32) -> Option<Self> {
        match id {
            0 => Some(Self::Pickup),
            1 => Some(Self::QuickMove),
            2 => Some(Self::Swap),
            3 => Some(Self::Clone),
            4 => Some(Self::Throw),
            5 => Some(Self::QuickCraft),
            6 => Some(Self::PickupAll),
            _ => None,
        }
    }

    /// The wire id.
    #[must_use]
    pub const fn id(self) -> i32 {
        match self {
            Self::Pickup => 0,
            Self::QuickMove => 1,
            Self::Swap => 2,
            Self::Clone => 3,
            Self::Throw => 4,
            Self::QuickCraft => 5,
            Self::PickupAll => 6,
        }
    }

    /// Stable name for logs.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Pickup => "pickup",
            Self::QuickMove => "quick_move",
            Self::Swap => "swap",
            Self::Clone => "clone",
            Self::Throw => "throw",
            Self::QuickCraft => "quick_craft",
            Self::PickupAll => "pickup_all",
        }
    }

    /// Whether this click type can change the total item count.
    ///
    /// Only throwing removes items and only creative cloning adds them, so this is
    /// used by the conservation checks and by the tests that assert the stronger
    /// invariant for every other gesture.
    #[must_use]
    pub const fn changes_total_count(self) -> bool {
        matches!(self, Self::Throw | Self::Clone)
    }

    /// Whether this click type may move the cursor stack.
    #[must_use]
    pub const fn uses_cursor(self) -> bool {
        !matches!(self, Self::Throw)
    }
}

impl std::fmt::Display for ClickType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

/// Which mouse button a drag uses (`button >> 2` in the wire encoding).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DragType {
    /// Even split across every dragged slot.
    Even,
    /// One item into each dragged slot.
    One,
    /// Fill each dragged slot to its limit.
    Full,
}

impl DragType {
    /// Recognise the packed value; anything above 2 is not a drag this supports.
    #[must_use]
    pub const fn from_packed(packed: i32) -> Option<Self> {
        match packed {
            0 => Some(Self::Even),
            1 => Some(Self::One),
            2 => Some(Self::Full),
            _ => None,
        }
    }

    /// The packed value.
    #[must_use]
    pub const fn packed(self) -> i32 {
        match self {
            Self::Even => 0,
            Self::One => 1,
            Self::Full => 2,
        }
    }
}

/// Where a drag is in its three-packet sequence (`button & 3`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DragStage {
    /// Begin the drag: remember the type, clear the per-slot accumulator.
    Start,
    /// Add one slot to the drag.
    AddSlot,
    /// Finish the drag: distribute the cursor stack across the dragged slots.
    End,
}

impl DragStage {
    /// Recognise the stage bits.
    #[must_use]
    pub const fn from_bits(bits: i32) -> Option<Self> {
        match bits {
            0 => Some(Self::Start),
            1 => Some(Self::AddSlot),
            2 => Some(Self::End),
            _ => None,
        }
    }

    /// The stage bits.
    #[must_use]
    pub const fn bits(self) -> i32 {
        match self {
            Self::Start => 0,
            Self::AddSlot => 1,
            Self::End => 2,
        }
    }
}

/// What a [`ClickType::Swap`] button refers to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SwapTarget {
    /// A hotbar slot, 0..=8.
    Hotbar(u8),
    /// The offhand slot (button 40).
    Offhand,
}

impl SwapTarget {
    /// Interpret a swap button.
    #[must_use]
    pub const fn from_button(button: i8) -> Option<Self> {
        match button {
            0..=8 => Some(Self::Hotbar(button as u8)),
            40 => Some(Self::Offhand),
            _ => None,
        }
    }
}

/// Why a click was refused in principle (before menu state is consulted).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClickRejection {
    /// `click_type` is not one this build implements.
    UnknownClickType(i32),
    /// `button` encodes something meaningless for this click type (a drag type
    /// above 2, a swap button that is neither a hotbar slot nor the offhand).
    BadButton {
        /// The click type the button was interpreted against.
        click_type: ClickType,
        /// The offending button value.
        button: i8,
    },
    /// The slot index is below −1, or above the largest menu this build can hold.
    SlotOutOfRange(i16),
    /// `state_id` is negative; the server never issues a negative revision.
    NegativeStateId(i32),
    /// The window id does not fit in the `u8` Vanilla uses.
    BadWindowId(i32),
}

impl std::fmt::Display for ClickRejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownClickType(id) => write!(f, "unknown click type {id}"),
            Self::BadButton { click_type, button } => {
                write!(f, "button {button} is meaningless for {click_type}")
            }
            Self::SlotOutOfRange(slot) => write!(f, "slot {slot} is out of range"),
            Self::NegativeStateId(id) => write!(f, "negative state id {id}"),
            Self::BadWindowId(id) => write!(f, "window id {id} does not fit in a u8"),
        }
    }
}

/// A decoded, structurally legal click.
///
/// This is the *typed* form: the click type, drag type and stage are enums, so no
/// later code has to re-derive what the button bits meant. Being structurally legal
/// says nothing about whether the menu will accept it — see [`crate::Menu`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Click {
    /// Which open window this refers to.
    pub window_id: u8,
    /// The server revision the client claims to be looking at.
    pub state_id: i32,
    /// Menu slot, or [`crate::SLOT_OUTSIDE`] for a click outside the window.
    pub slot: i16,
    /// Raw button, kept for diagnostics.
    pub button: i8,
    /// The gesture.
    pub click_type: ClickType,
    /// For [`ClickType::Pickup`]: `true` when the right button was used.
    pub secondary: bool,
    /// For [`ClickType::Swap`]: what to swap with.
    pub swap_target: Option<SwapTarget>,
    /// For [`ClickType::QuickCraft`]: which distribution rule.
    pub drag_type: Option<DragType>,
    /// For [`ClickType::QuickCraft`]: which stage of the sequence.
    pub drag_stage: Option<DragStage>,
}

impl Click {
    /// Interpret the five wire integers.
    ///
    /// # Errors
    ///
    /// [`ServerError::InvalidAction`] wrapping a [`ClickRejection`] when the values
    /// cannot be a legal click of their stated type. Nothing is mutated and no menu
    /// state is consulted, so this is safe to call on raw packet fields.
    // One flat match over the seven click types; splitting it would scatter the
    // button interpretation that the type-specific arms exist to centralise.
    #[allow(clippy::too_many_lines)]
    pub fn new(
        window_id: i32,
        state_id: i32,
        slot: i16,
        button: i8,
        click_type: i32,
    ) -> ServerResult<Self> {
        let reject = |rejection: ClickRejection| {
            Err(ServerError::InvalidAction(format!(
                "malformed click: {rejection}"
            )))
        };

        let Ok(window_id) = u8::try_from(window_id) else {
            return reject(ClickRejection::BadWindowId(window_id));
        };
        if state_id < 0 {
            return reject(ClickRejection::NegativeStateId(state_id));
        }
        // `-1` means "outside the window" (a drop) and is legal; anything else
        // negative, or beyond the largest menu, is not.
        if slot != SLOT_OUTSIDE
            && (slot < 0 || usize::try_from(slot).unwrap_or(usize::MAX) > super::MAX_MENU_SLOTS)
        {
            return reject(ClickRejection::SlotOutOfRange(slot));
        }
        let Some(click_type) = ClickType::from_id(click_type) else {
            return reject(ClickRejection::UnknownClickType(click_type));
        };

        let mut click = Self {
            window_id,
            state_id,
            slot,
            button,
            click_type,
            secondary: false,
            swap_target: None,
            drag_type: None,
            drag_stage: None,
        };

        match click_type {
            ClickType::Pickup => {
                // 0 = left, 1 = right. Anything else is not a pickup.
                match button {
                    0 => {}
                    1 => click.secondary = true,
                    other => {
                        return reject(ClickRejection::BadButton {
                            click_type,
                            button: other,
                        });
                    }
                }
            }
            ClickType::QuickMove => {
                // The client always sends 0; the field carries no information.
                if button != 0 {
                    return reject(ClickRejection::BadButton { click_type, button });
                }
            }
            ClickType::Swap => {
                let Some(target) = SwapTarget::from_button(button) else {
                    return reject(ClickRejection::BadButton { click_type, button });
                };
                click.swap_target = Some(target);
            }
            ClickType::Clone => {
                // 0 = clone the slot, 1 = clone during a drag, 2 = clone to max.
                // Only 0 and 2 are implemented; 1 is refused rather than ignored.
                match button {
                    0 | 2 => {}
                    other => {
                        return reject(ClickRejection::BadButton {
                            click_type,
                            button: other,
                        });
                    }
                }
            }
            ClickType::Throw => {
                // 0 = one item, 1 = the stack. Note that this is the *opposite*
                // convention to `Pickup`'s secondary flag, which is why the button
                // is interpreted per click type rather than globally.
                match button {
                    0 => {}
                    1 => click.secondary = true,
                    other => {
                        return reject(ClickRejection::BadButton {
                            click_type,
                            button: other,
                        });
                    }
                }
            }
            ClickType::QuickCraft => {
                let packed = i32::from(button);
                let Some(drag_type) = DragType::from_packed(packed >> 2) else {
                    return reject(ClickRejection::BadButton { click_type, button });
                };
                let Some(drag_stage) = DragStage::from_bits(packed & 3) else {
                    return reject(ClickRejection::BadButton { click_type, button });
                };
                click.drag_type = Some(drag_type);
                click.drag_stage = Some(drag_stage);
            }
            ClickType::PickupAll => {
                // 0 = double-click gather, 1 = the same while the cursor is empty.
                match button {
                    0 => {}
                    1 => click.secondary = true,
                    other => {
                        return reject(ClickRejection::BadButton {
                            click_type,
                            button: other,
                        });
                    }
                }
            }
        }
        Ok(click)
    }

    /// Whether the click is outside the window rather than on a slot.
    #[must_use]
    pub const fn is_outside(&self) -> bool {
        self.slot == SLOT_OUTSIDE
    }

    /// The slot index as a `usize`, when the click is on a slot.
    #[must_use]
    pub fn slot_index(&self) -> Option<usize> {
        if self.is_outside() {
            return None;
        }
        usize::try_from(self.slot).ok()
    }

    /// The drag type, when this is a drag click.
    #[must_use]
    pub const fn drag(&self) -> Option<(DragType, DragStage)> {
        match (self.drag_type, self.drag_stage) {
            (Some(kind), Some(stage)) => Some((kind, stage)),
            _ => None,
        }
    }

    /// A count that is safe to hand to a stack operation, ignoring the button.
    #[must_use]
    pub const fn hard_count_bound() -> i32 {
        HARD_MAX_COUNT
    }
}

#[cfg(test)]
mod tests {
    use super::{Click, ClickRejection, ClickType, DragStage, DragType, SwapTarget};
    use mc_core::error::ServerError;

    fn refused(window: i32, state: i32, slot: i16, button: i8, kind: i32) -> String {
        match Click::new(window, state, slot, button, kind) {
            Err(ServerError::InvalidAction(message)) => message,
            Err(other) => panic!("wrong error kind: {other:?}"),
            Ok(click) => panic!("expected a rejection, got {click:?}"),
        }
    }

    #[test]
    fn click_type_ids_round_trip_and_cover_the_wire_range() {
        for kind in ClickType::ALL {
            assert_eq!(ClickType::from_id(kind.id()), Some(kind));
            assert!(!kind.name().is_empty());
        }
        assert_eq!(ClickType::from_id(-1), None);
        assert_eq!(ClickType::from_id(7), None);
        assert_eq!(ClickType::from_id(i32::MAX), None);
    }

    #[test]
    fn only_throw_and_clone_change_the_total_count() {
        for kind in ClickType::ALL {
            let expected = matches!(kind, ClickType::Throw | ClickType::Clone);
            assert_eq!(
                kind.changes_total_count(),
                expected,
                "{kind} total-count classification"
            );
        }
    }

    #[test]
    fn pickup_buttons_map_to_primary_and_secondary() {
        let primary = Click::new(0, 1, 3, 0, 0).expect("primary");
        assert!(!primary.secondary);
        let secondary = Click::new(0, 1, 3, 1, 0).expect("secondary");
        assert!(secondary.secondary);
        // Any other button is not a pickup.
        assert!(Click::new(0, 1, 3, 2, 0).is_err());
        assert!(Click::new(0, 1, 3, -1, 0).is_err());
    }

    #[test]
    fn throw_button_convention_is_the_opposite_of_pickup() {
        // Throw 1 means "the whole stack", which sets the same flag pickup uses
        // for "right button". The two must not be conflated.
        let one = Click::new(0, 1, 3, 0, 4).expect("one");
        assert!(!one.secondary);
        let stack = Click::new(0, 1, 3, 1, 4).expect("stack");
        assert!(stack.secondary);
        assert!(!ClickType::Throw.uses_cursor());
        assert!(ClickType::Pickup.uses_cursor());
    }

    #[test]
    fn swap_buttons_accept_hotbar_and_offhand_only() {
        for slot in 0..=8u8 {
            let click = Click::new(0, 1, 3, slot as i8, 2).expect("hotbar swap");
            assert_eq!(click.swap_target, Some(SwapTarget::Hotbar(slot)));
        }
        let offhand = Click::new(0, 1, 3, 40, 2).expect("offhand swap");
        assert_eq!(offhand.swap_target, Some(SwapTarget::Offhand));
        for bad in [9i8, 39, 41, 127, -1] {
            assert!(
                Click::new(0, 1, 3, bad, 2).is_err(),
                "swap button {bad} must be refused"
            );
        }
    }

    #[test]
    fn drag_button_packs_type_and_stage() {
        for kind in [DragType::Even, DragType::One, DragType::Full] {
            for stage in [DragStage::Start, DragStage::AddSlot, DragStage::End] {
                let packed = ((kind.packed() << 2) | stage.bits()) as i8;
                let click = Click::new(0, 1, 3, packed, 5).expect("drag");
                assert_eq!(click.drag(), Some((kind, stage)), "packed {packed}");
            }
        }
        // A drag type above 2 is not a drag this build implements.
        assert!(Click::new(0, 1, 3, (3 << 2) as i8, 5).is_err());
    }

    #[test]
    fn quick_move_requires_a_zero_button() {
        assert!(Click::new(0, 1, 3, 0, 1).is_ok());
        assert!(Click::new(0, 1, 3, 1, 1).is_err());
    }

    #[test]
    fn clone_accepts_only_the_two_implemented_buttons() {
        assert!(Click::new(0, 1, 3, 0, 3).is_ok());
        assert!(Click::new(0, 1, 3, 2, 3).is_ok());
        assert!(Click::new(0, 1, 3, 1, 3).is_err());
    }

    #[test]
    fn hostile_scalars_are_refused_with_a_reason() {
        // Slot below −1.
        assert!(refused(0, 1, -2, 0, 0).contains("slot"));
        assert!(refused(0, 1, i16::MIN, 0, 0).contains("slot"));
        // Slot far beyond any menu.
        assert!(refused(0, 1, i16::MAX, 0, 0).contains("slot"));
        // Negative state id.
        assert!(refused(0, -1, 0, 0, 0).contains("state id"));
        // Window id outside u8.
        assert!(refused(256, 1, 0, 0, 0).contains("window id"));
        assert!(refused(-1, 1, 0, 0, 0).contains("window id"));
        assert!(refused(i32::MAX, 1, 0, 0, 0).contains("window id"));
        // Unknown click type.
        assert!(refused(0, 1, 0, 0, 99).contains("click type"));
        // A bad button names the click type it was interpreted against.
        let message = refused(0, 1, 0, 5, 0);
        assert!(message.contains("pickup"), "{message}");
    }

    #[test]
    fn slot_outside_is_accepted_and_reported() {
        let click = Click::new(0, 1, -1, 0, 0).expect("outside");
        assert!(click.is_outside());
        assert_eq!(click.slot_index(), None);
        let inside = Click::new(0, 1, 0, 0, 0).expect("inside");
        assert!(!inside.is_outside());
        assert_eq!(inside.slot_index(), Some(0));
    }

    #[test]
    fn rejection_display_is_informative() {
        let cases = [
            ClickRejection::UnknownClickType(9),
            ClickRejection::BadButton {
                click_type: ClickType::Swap,
                button: 77,
            },
            ClickRejection::SlotOutOfRange(-5),
            ClickRejection::NegativeStateId(-1),
            ClickRejection::BadWindowId(999),
        ];
        for case in cases {
            let text = case.to_string();
            assert!(!text.is_empty());
        }
    }
}
