//! Lightning bolts: the strike visual (P20-03 slice 2).
//!
//! A bolt is a transient visual with a position: no health, no AI, no
//! physics, no pickup, no persistence. Damage is applied once, at spawn, by
//! the striker — the bolt itself never acts. The numbers below mirror
//! vanilla via Pumpkin's `LightningBoltEntity` (a vanilla port, so the shape
//! is evidence, not invention):
//!
//! - damage box `(x±3, y-3..y+9, z±3)` around the strike;
//! - 5.0 damage to living entities in the box, once each;
//! - armour reduces it (`lightning_bolt` is absent from the 26.1
//!   `bypasses_armor` tag), knockback does not apply (it is in
//!   `no_knockback`);
//! - fire, creeper charging, villager/witch and pig/piglin conversion are
//!   named gaps (no fire model, no powered state, unmodelled mobs).
//!
//! The bolt entity lives a few ticks so a real client draws the strike
//! before the removal broadcast clears it; the duration is visual only.

/// Ticks a bolt entity stays in the store before the sweep removes it.
///
/// Visual only: damage lands at spawn, and the client clears the bolt on the
/// removal broadcast. Four ticks reads as a strike rather than a flicker.
pub const BOLT_LIFE_TICKS: u8 = 4;

/// A lightning-bolt visual: a fuse counting down to removal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Bolt {
    /// Ticks left before the sweep removes this bolt.
    life: u8,
}

impl Bolt {
    /// A fresh bolt with a full visual fuse.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            life: BOLT_LIFE_TICKS,
        }
    }

    /// Burn one tick of the fuse.
    ///
    /// Returns whether the fuse ran out — the caller flags the entity
    /// removed, and the removal sweep broadcasts it.
    pub fn tick_life(&mut self) -> bool {
        self.life = self.life.saturating_sub(1);
        self.life == 0
    }
}

impl Default for Bolt {
    /// Equivalent to [`Bolt::new`].
    fn default() -> Self {
        Self::new()
    }
}
