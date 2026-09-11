//! Power-model acceptance: the 0..=15 range, refusal above it, and the weak/strong read.
//!
//! These are the tests required by the work item's "`PowerLevel` bounds" and
//! "`max(weak, strong)` reading" entries. Where a rule is this project's rather than
//! Vanilla's, the test name says `our_` and the comment says why — AGENTS.md §3.1 forbids
//! claiming Vanilla behaviour without evidence, and the `max` rule is a product decision.

mod common;

use common::{CEILING, level, on};
use mc_core::error::ServerError;
use mc_redstone::{MAX_POWER, PowerLevel, PowerSource, PowerState, SignalKind};

#[test]
fn power_levels_cover_the_whole_range_and_nothing_above_it() {
    assert_eq!(MAX_POWER, 15, "the signal range is 0..=15");
    assert_eq!(CEILING, 15);
    for value in 0..=MAX_POWER {
        let level = PowerLevel::new(value).expect("0..=15 is valid");
        assert_eq!(level.get(), value);
        assert_eq!(level.is_powered(), value != 0);
        assert_eq!(u8::from(level), value);
        // The three constructors agree inside the range.
        assert_eq!(PowerLevel::from_raw(value), level);
        assert_eq!(PowerLevel::try_from(value).expect("valid"), level);
    }
    assert_eq!(PowerLevel::ZERO.get(), 0);
    assert_eq!(PowerLevel::MAX.get(), 15);
}

#[test]
fn a_power_level_above_fifteen_is_refused_not_wrapped() {
    // The failure mode this prevents: 255 wrapping to 15 (or to 0) and producing a circuit
    // that looks plausible and is wrong.
    for value in [16u8, 17, 42, 254, 255] {
        let refused = PowerLevel::new(value);
        assert!(refused.is_err(), "{value} must be refused");
        assert!(
            matches!(refused, Err(ServerError::InvalidAction(_))),
            "{value} must be an InvalidAction, not an invariant violation"
        );
        assert!(PowerLevel::try_from(value).is_err(), "{value} via TryFrom");
    }
    // The error text names the offending value so an operator can act on the log line.
    let message = PowerLevel::new(255)
        .expect_err("255 is refused")
        .to_string();
    assert!(
        message.contains("255"),
        "error should name the value: {message}"
    );
    assert!(message.contains("invalid player action"), "{message}");
}

#[test]
fn the_hostile_input_door_clamps_instead_of_failing() {
    // `from_raw` is for values that came off the wire or out of a file: refusing would abort
    // a tick, so it clamps.
    assert_eq!(PowerLevel::from_raw(255), PowerLevel::MAX);
    assert_eq!(PowerLevel::from_raw(16), PowerLevel::MAX);
    assert_eq!(PowerLevel::from_raw(15), PowerLevel::MAX);
    assert_eq!(PowerLevel::from_raw(0), PowerLevel::ZERO);
    for value in 0..=u8::MAX {
        assert!(PowerLevel::from_raw(value).get() <= MAX_POWER, "{value}");
    }
}

#[test]
fn our_consumer_reads_the_maximum_of_weak_and_strong() {
    // Named `our_`: `max(weak, strong)` is this crate's product decision, not a verified
    // Vanilla rule (see `PowerState`'s docs). What it must do is never make an activating
    // signal smaller, which is what the assertions below check.
    let weak = level(9);
    let strong = level(4);
    assert_eq!(PowerState::new(weak, strong).effective(), weak);
    assert_eq!(PowerState::new(strong, weak).effective(), weak);
    assert_eq!(PowerState::weak_only(weak).effective(), weak);
    assert_eq!(PowerState::strong_at(strong).effective(), strong);
    assert_eq!(PowerState::OFF.effective(), PowerLevel::ZERO);
    // An activating signal in *either* kind is never lost.
    for value in 0..=MAX_POWER {
        let level = level(value);
        assert_eq!(PowerState::new(level, PowerLevel::ZERO).effective(), level);
        assert_eq!(PowerState::new(PowerLevel::ZERO, level).effective(), level);
        assert_eq!(
            PowerState::new(level, level).effective(),
            level,
            "the max of equal levels is that level"
        );
    }
    // And it is monotone: raising either kind can never lower the read.
    for weak in 0..=MAX_POWER {
        for strong in 0..=MAX_POWER {
            let state = PowerState::new(level(weak), level(strong));
            assert!(state.effective().get() >= weak && state.effective().get() >= strong);
            assert_eq!(state.is_powered(), weak != 0 || strong != 0);
        }
    }
}

#[test]
fn strength_alone_decides_activation_not_the_kind() {
    // A mechanism activates on any non-zero signal; the kind only decides whether *dust* is
    // powered. So weak 1 activates, and the kind query is separate from the activation query.
    let barely_on = PowerState::weak_only(level(1));
    assert!(barely_on.is_powered());
    assert_eq!(barely_on.effective(), level(1));
    assert!(!SignalKind::Weak.powers_dust());
    assert!(SignalKind::Strong.powers_dust());
}

#[test]
fn attenuation_is_exactly_one_per_block_and_floors_at_zero() {
    let mut state = PowerState::strong_at(PowerLevel::MAX);
    for step in 0..MAX_POWER {
        assert_eq!(
            state.effective().get(),
            MAX_POWER - step,
            "after {step} blocks of attenuation"
        );
        state = state.attenuate(1);
    }
    assert_eq!(state, PowerState::OFF, "15 steps reaches 0 from 15");
    // Never negative, never wrapped.
    assert_eq!(
        PowerState::strong_at(PowerLevel::MAX).attenuate(200),
        PowerState::OFF
    );
    assert_eq!(PowerLevel::MAX.saturating_sub(u8::MAX), PowerLevel::ZERO);
    assert_eq!(PowerLevel::ZERO.saturating_sub(1), PowerLevel::ZERO);
}

#[test]
fn only_a_redstone_block_is_modelled_as_a_strong_source() {
    // The strong/weak split is the point of the model: a strong source powers dust next to
    // it, a weak one does not. This test pins the *implemented* table; `power.rs` labels the
    // redstone block's strength as not independently verified.
    let expected = [
        (PowerSource::RedstoneBlock, true),
        (PowerSource::Torch, false),
        (PowerSource::Lever, false),
        (PowerSource::Button, false),
        (PowerSource::PressurePlate, false),
        (PowerSource::Repeater, false),
        (PowerSource::Comparator, false),
        (PowerSource::LightningRod, false),
    ];
    for (source, strong) in expected {
        assert_eq!(source.is_strong_source(), strong, "{source}");
        let state = source.active_state();
        assert_eq!(state.weak, PowerLevel::MAX, "{source} emits 15 weakly");
        assert!(state.is_powered(), "{source} is powered while active");
        if strong {
            assert_eq!(state.strong, PowerLevel::MAX, "{source} emits 15 strongly");
            assert_eq!(state.effective(), PowerLevel::MAX);
        } else {
            assert_eq!(
                state.strong,
                PowerLevel::ZERO,
                "{source} has no strong field"
            );
        }
    }
    assert_eq!(on(), PowerState::weak_only(PowerLevel::MAX));
}

#[test]
fn combining_two_inputs_keeps_the_stronger_of_each_kind() {
    // A wire with two feeders must not lose the strong field to the weak one, which is why
    // the combination is component-wise rather than a single maximum.
    let combined = PowerState::weak_only(PowerLevel::MAX).max(PowerState::strong_at(level(3)));
    assert_eq!(combined.weak.get(), MAX_POWER);
    assert_eq!(combined.strong.get(), 3);
    // Idempotent, commutative, and with `OFF` as the identity: the three properties the
    // propagation loop's order-independence relies on.
    assert_eq!(combined.max(combined), combined);
    assert_eq!(combined.max(PowerState::OFF), combined);
    assert_eq!(PowerState::OFF.max(combined), combined);
    for weak in 0..=MAX_POWER {
        for strong in 0..=MAX_POWER {
            let a = PowerState::new(level(weak), level(strong));
            assert_eq!(a.max(PowerState::OFF), a);
            assert_eq!(a.max(a), a);
        }
    }
}
