//! Experience orbs: value, splitting, merge, physics (P16-02).
//!
//! An orb is a value with a position. The numbers below mirror vanilla via
//! pumpkin's `ExperienceOrbEntity`: split thresholds
//! (2477/1237/617/307/149/73/37/17/7/3/1), 6000-tick lifetime, 0.03 gravity.
//! Drag and ground friction mirror the item path exactly (same constants,
//! cited there) pending an orb-specific measurement. What is deliberately
//! absent: mending (P18), player pickup delay on the orb itself (vanilla has
//! none — the 2-tick throttle lives on the picking-up player).

use mc_world::Vec3;

/// Ticks an orb survives before it is discarded, at 20 TPS.
///
/// The vanilla value: orb age discards at `age >= 6000`, i.e. 5 minutes —
/// the same lifetime as a dropped item.
pub const ORB_DESPAWN_AGE_TICKS: u32 = 6000;

/// Downward velocity added to an orb every tick, in blocks per tick squared.
///
/// Vanilla orb gravity (pumpkin `ExperienceOrbEntity::get_gravity`), distinct
/// from the item 0.04.
pub const ORB_GRAVITY: f64 = 0.03;

/// An experience orb's value: how many XP points picking it up grants.
///
/// Positive by caller contract (the server's spawn refuses the rest, the way
/// drops refuse empty stacks); merges only ever add.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Orb {
    /// XP points in this orb.
    pub value: i32,
    /// Ticks existed, saturating at [`ORB_DESPAWN_AGE_TICKS`].
    pub age: u32,
    /// Whether the last move reported solid ground beneath.
    pub on_ground: bool,
}

impl Orb {
    /// A fresh orb: zero age, airborne until the first move says otherwise.
    #[must_use]
    pub const fn new(value: i32) -> Self {
        Self {
            value,
            age: 0,
            on_ground: false,
        }
    }

    /// Advance the age clock by one tick, saturating at the despawn bound.
    pub const fn tick_age(&mut self) {
        self.age = if self.age >= ORB_DESPAWN_AGE_TICKS {
            ORB_DESPAWN_AGE_TICKS
        } else {
            self.age.saturating_add(1)
        };
    }

    /// Whether this orb is due for the sweep.
    #[must_use]
    pub const fn should_despawn(&self) -> bool {
        self.age >= ORB_DESPAWN_AGE_TICKS
    }

    /// Gravity and drag for one tick, mirroring the item path's shape with
    /// the orb's own gravity constant.
    #[must_use]
    pub fn tick_physics(&self, velocity: Vec3) -> Vec3 {
        let (mut vx, mut vy, mut vz) = (velocity.x, velocity.y, velocity.z);
        vy -= ORB_GRAVITY;
        vx *= crate::item_entity::ITEM_DRAG;
        vy *= crate::item_entity::ITEM_DRAG;
        vz *= crate::item_entity::ITEM_DRAG;
        if self.on_ground {
            vx *= crate::item_entity::ITEM_GROUND_FRICTION;
            vy = 0.0;
            vz *= crate::item_entity::ITEM_GROUND_FRICTION;
        }
        Vec3::new(vx, vy, vz)
    }

    /// Fold `other` into `self`, returning the combined value. Values only
    /// grow by merge, so the sum saturates rather than wraps.
    pub fn merge_from(&mut self, other: &Self) {
        self.value = self.value.saturating_add(other.value);
    }
}

/// Largest single orb of each band: vanilla splits an amount from the top
/// down, so 5 becomes 3+1+1 and 40 becomes 37+3.
const fn split_band(amount: i32) -> i32 {
    if amount >= 2477 {
        2477
    } else if amount >= 1237 {
        1237
    } else if amount >= 617 {
        617
    } else if amount >= 307 {
        307
    } else if amount >= 149 {
        149
    } else if amount >= 73 {
        73
    } else if amount >= 37 {
        37
    } else if amount >= 17 {
        17
    } else if amount >= 7 {
        7
    } else if amount >= 3 {
        3
    } else {
        1
    }
}

/// Split `amount` into orb values, largest first. Empty for non-positive
/// input (callers refuse those before spawning).
#[must_use]
pub fn split_orb_value(mut amount: i32) -> Vec<i32> {
    let mut out = Vec::new();
    while amount > 0 {
        let take = split_band(amount);
        out.push(take);
        amount -= take;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{ORB_DESPAWN_AGE_TICKS, Orb, split_orb_value};

    #[test]
    fn splits_follow_the_bands_top_down() {
        assert_eq!(split_orb_value(0), Vec::<i32>::new());
        assert_eq!(split_orb_value(-5), Vec::<i32>::new());
        assert_eq!(split_orb_value(1), vec![1]);
        assert_eq!(split_orb_value(5), vec![3, 1, 1]);
        assert_eq!(split_orb_value(40), vec![37, 3]);
        assert_eq!(split_orb_value(2477), vec![2477]);
        assert_eq!(split_orb_value(2478), vec![2477, 1]);
        let parts = split_orb_value(100);
        assert_eq!(parts.iter().sum::<i32>(), 100, "splits conserve the total");
    }

    #[test]
    fn age_saturates_and_fires_despawn() {
        let mut orb = Orb::new(3);
        assert!(!orb.should_despawn());
        for _ in 0..ORB_DESPAWN_AGE_TICKS {
            orb.tick_age();
        }
        assert!(orb.should_despawn());
        orb.tick_age();
        assert_eq!(orb.age, ORB_DESPAWN_AGE_TICKS, "saturates, never wraps");
    }

    #[test]
    fn merges_add_values() {
        let mut orb = Orb::new(3);
        orb.merge_from(&Orb::new(7));
        assert_eq!(orb.value, 10);
    }
}
