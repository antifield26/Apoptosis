//! Entity identity that a deterministic simulation can live with.
//!
//! # Why this is not `Uuid::new_v4()`
//!
//! The engineering contract requires that **the same initial state and the same ordered inputs over the same tick
//! count produce the same normalized simulation state**. A random UUID per spawn breaks exactly that: two runs of
//! the same script would differ in a field that a client sees and that a trace would record.
//!
//! So an entity's UUID is **derived** from inputs the run already has — the world seed and the entity id — which
//! makes it stable for a given seed and input order, and distinct between entities.
//!
//! # Why a counter alone would not do either
//!
//! "The first spawn is UUID 1" is deterministic but says nothing about **which** run a UUID belongs to, so two
//! worlds at different seeds would produce colliding identities in any trace that compared them. Mixing the seed
//! in costs one multiply and removes that.
//!
//! # The mixing
//!
//! The finaliser from `SplitMix64`: two xor-shift-multiply rounds and a closing shift. It is not cryptographic and
//! does not need to be — what it needs is that **distinct pairs give distinct outputs**, which the constant
//! multipliers make overwhelmingly likely and which the tests below check for the ranges this server uses.

use uuid::Uuid;

/// The UUID an entity with this id has in a world with this seed.
///
/// Deterministic in both arguments, and distinct across both: see the module docs for why neither a random UUID
/// nor a plain counter satisfies the contract.
#[must_use]
pub fn entity_uuid(seed: i64, id: i32) -> Uuid {
    /// `SplitMix64`'s finaliser.
    const fn mix(mut z: u64) -> u64 {
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    // Two independent words, so the id and the seed cannot cancel each other out in a single mix.
    let low = mix((seed as u64) ^ ((id as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)));
    let high = mix(low ^ 0xA076_1D64_78BD_642F);

    let mut bytes = [0u8; 16];
    bytes[..8].copy_from_slice(&low.to_be_bytes());
    bytes[8..].copy_from_slice(&high.to_be_bytes());

    // **Version 4 and the RFC 4122 variant.** The wire carries sixteen opaque bytes, so these bits are not
    // checked by a client; setting them is what makes the value a well-formed UUID rather than sixteen bytes that
    // happen to be the right length, and it costs two masks.
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;

    Uuid::from_bytes(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn the_same_seed_and_id_always_give_the_same_uuid() {
        // The property the contract asks for: a replay produces the same identity, which is what makes a trace
        // comparable at all.
        for (seed, id) in [
            (0i64, 1i32),
            (1, 1),
            (1_361_882_806, 42),
            (i64::MIN, i32::MAX),
        ] {
            assert_eq!(
                entity_uuid(seed, id),
                entity_uuid(seed, id),
                "seed {seed}, id {id} is not stable"
            );
        }
    }

    #[test]
    fn distinct_entities_in_one_world_get_distinct_uuids() {
        let seen: HashSet<Uuid> = (1..=10_000).map(|id| entity_uuid(7, id)).collect();
        assert_eq!(
            seen.len(),
            10_000,
            "two entity ids in one world produced the same UUID"
        );
    }

    #[test]
    fn the_same_id_in_two_worlds_gets_two_uuids() {
        // The reason the seed is mixed in rather than only a counter: two worlds must not share identities, or a
        // trace comparing them would report agreement that means nothing.
        let a: HashSet<Uuid> = (1..=2_000).map(|id| entity_uuid(1, id)).collect();
        let b: HashSet<Uuid> = (1..=2_000).map(|id| entity_uuid(2, id)).collect();
        assert!(
            a.is_disjoint(&b),
            "two seeds produced overlapping entity identities"
        );
    }

    #[test]
    fn the_result_is_a_well_formed_uuid() {
        for (seed, id) in [(0i64, 0i32), (-1, -1), (12345, 999)] {
            let uuid = entity_uuid(seed, id);
            assert_eq!(
                uuid.get_version_num(),
                4,
                "seed {seed}, id {id}: version bits"
            );
            assert_eq!(
                uuid.get_variant(),
                uuid::Variant::RFC4122,
                "seed {seed}, id {id}: variant bits"
            );
        }
    }

    #[test]
    fn neither_argument_alone_determines_the_result() {
        // A cheap guard against a derivation that silently drops one input, which would still pass the stability
        // test above and the distinctness test below.
        assert_ne!(
            entity_uuid(5, 1),
            entity_uuid(5, 2),
            "the id is not mixed in"
        );
        assert_ne!(
            entity_uuid(5, 1),
            entity_uuid(6, 1),
            "the seed is not mixed in"
        );
    }
}
