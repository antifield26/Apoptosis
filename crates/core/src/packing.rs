//! Paletted-container bit packing (P03-10).
//!
//! Blocks and biomes are stored as a palette plus a packed index array:
//!
//! ```text
//! block_states: { palette: [ {Name: ...}, ... ], data: [LONG, ...] }
//! ```
//!
//! Rules, verified by unpacking a real 26.1.2 chunk and re-packing it
//! byte-identically (`crates/persistence/tests/anvil_fixture.rs`):
//!
//! - entries per container: 4096 block states (16³) or 64 biomes (4³);
//! - bit width is **derived from the palette length**, not stored:
//!   `bits = max(min_bits, ceil(log2(palette_len)))`, with `min_bits = 4` for
//!   blocks and `1` for biomes;
//! - a single-entry palette omits `data` entirely;
//! - indices are packed **LSB-first and never span a `long` boundary**, so every
//!   `long` holds `64 / bits` entries and the tail of each `long` is padding.
//!
//! `ceil(log2(n))` uses `usize::BITS - (n - 1).leading_zeros()`: a palette of 16
//! needs 4 bits, 17 needs 5.

use crate::error::{ServerError, ServerResult};

/// Block states per chunk section (16×16×16).
pub const BLOCK_ENTRIES: usize = 4096;

/// Biomes per chunk section (4×4×4).
pub const BIOME_ENTRIES: usize = 64;

/// Minimum bit width for block-state palettes.
pub const BLOCK_MIN_BITS: u32 = 4;

/// Minimum bit width for biome palettes.
pub const BIOME_MIN_BITS: u32 = 1;

/// Highest bit width this implementation packs (vanilla caps well below this).
pub const MAX_BITS: u32 = 32;

/// Bit width implied by a palette of `palette_len` entries.
///
/// Returns `0` for a palette of 0 or 1 entries, which callers resolve by
/// omitting the `data` array (a single value needs no indices).
#[must_use]
pub const fn bits_for(palette_len: usize, min_bits: u32) -> u32 {
    let needed = if palette_len <= 1 {
        0
    } else {
        usize::BITS - (palette_len - 1).leading_zeros()
    };
    if needed < min_bits { min_bits } else { needed }
}

/// Entries that fit in one `long` at this bit width.
#[must_use]
pub const fn values_per_long(bits: u32) -> usize {
    (64 / bits) as usize
}

/// Longs needed to pack `entries` values at this bit width.
#[must_use]
pub const fn longs_needed(entries: usize, bits: u32) -> usize {
    let per_long = values_per_long(bits);
    entries.div_ceil(per_long)
}

/// Pack indices LSB-first, never spanning a `long` boundary.
///
/// # Errors
///
/// [`ServerError::Invariant`] when `bits` is outside `1..=32`: the width is
/// always derived from a validated palette, so a violation is a bug in the
/// caller rather than bad input, and must not become a panic.
pub fn pack(values: &[u32], bits: u32) -> ServerResult<Vec<i64>> {
    if !(1..=MAX_BITS).contains(&bits) {
        return Err(ServerError::Invariant(format!(
            "packing bit width {bits} is outside 1..={MAX_BITS}"
        )));
    }
    let per_long = values_per_long(bits);
    let mask = (1u64 << bits) - 1;
    let mut out = vec![0i64; longs_needed(values.len(), bits)];
    for (index, value) in values.iter().enumerate() {
        let slot = index / per_long;
        let offset = ((index % per_long) as u32) * bits;
        out[slot] |= ((u64::from(*value) & mask) << offset) as i64;
    }
    Ok(out)
}

/// Unpack `entries` indices from a packed array.
///
/// # Errors
///
/// [`ServerError::CorruptData`] when the array is too short for the declared
/// entry count, or when an index points outside the palette (checked by the
/// caller, which knows the palette length).
pub fn unpack(data: &[i64], bits: u32, entries: usize) -> ServerResult<Vec<u32>> {
    if !(1..=MAX_BITS).contains(&bits) {
        return Err(ServerError::CorruptData(format!(
            "packed container declares an invalid {bits}-bit width"
        )));
    }
    let needed = longs_needed(entries, bits);
    if data.len() < needed {
        return Err(ServerError::CorruptData(format!(
            "packed container needs {needed} longs for {entries} entries at {bits} bits, has {}",
            data.len()
        )));
    }
    let per_long = values_per_long(bits);
    let mask = (1u64 << bits) - 1;
    let mut out = Vec::with_capacity(entries);
    for index in 0..entries {
        let slot = index / per_long;
        let offset = ((index % per_long) as u32) * bits;
        out.push((((data[slot] as u64) >> offset) & mask) as u32);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::{
        BIOME_ENTRIES, BIOME_MIN_BITS, BLOCK_ENTRIES, BLOCK_MIN_BITS, bits_for, longs_needed, pack,
        unpack, values_per_long,
    };

    #[test]
    fn bit_width_follows_palette_length() {
        // Measured palette sizes in a real 26.1.2 chunk: 1, 7, 8, 10, 12, 14, 18.
        assert_eq!(bits_for(0, BLOCK_MIN_BITS), BLOCK_MIN_BITS);
        assert_eq!(bits_for(1, BLOCK_MIN_BITS), BLOCK_MIN_BITS);
        assert_eq!(bits_for(2, BLOCK_MIN_BITS), BLOCK_MIN_BITS);
        assert_eq!(bits_for(16, BLOCK_MIN_BITS), 4);
        assert_eq!(bits_for(17, BLOCK_MIN_BITS), 5);
        assert_eq!(bits_for(18, BLOCK_MIN_BITS), 5);
        assert_eq!(bits_for(32, BLOCK_MIN_BITS), 5);
        assert_eq!(bits_for(33, BLOCK_MIN_BITS), 6);
        assert_eq!(bits_for(256, BLOCK_MIN_BITS), 8);
        assert_eq!(bits_for(4096, BLOCK_MIN_BITS), 12);
        // Biomes use a 1-bit minimum.
        assert_eq!(bits_for(1, BIOME_MIN_BITS), 1);
        assert_eq!(bits_for(2, BIOME_MIN_BITS), 1);
        assert_eq!(bits_for(3, BIOME_MIN_BITS), 2);
        assert_eq!(bits_for(64, BIOME_MIN_BITS), 6);
    }

    #[test]
    fn geometry_matches_the_format() {
        // 64 / bits values per long, packed without spanning: 4 bits → 16,
        // 5 bits → 12, 8 bits → 8.
        assert_eq!(values_per_long(4), 16);
        assert_eq!(values_per_long(5), 12);
        assert_eq!(values_per_long(8), 8);
        assert_eq!(longs_needed(BLOCK_ENTRIES, 4), 256);
        assert_eq!(longs_needed(BLOCK_ENTRIES, 5), 342);
        assert_eq!(longs_needed(BLOCK_ENTRIES, 8), 512);
        assert_eq!(longs_needed(BIOME_ENTRIES, 1), 1);
        assert_eq!(longs_needed(BIOME_ENTRIES, 3), 4);
        assert_eq!(longs_needed(BIOME_ENTRIES, 6), 7);
    }

    #[test]
    fn round_trip_for_every_plausible_width() {
        for bits in 1..=12u32 {
            let entries = BLOCK_ENTRIES;
            let mask = (1u32 << bits) - 1;
            let values: Vec<u32> = (0..entries).map(|i| (i as u32) & mask).collect();
            let packed = pack(&values, bits).expect("packs");
            assert_eq!(packed.len(), longs_needed(entries, bits));
            assert_eq!(unpack(&packed, bits, entries).expect("unpacks"), values);
        }
    }

    #[test]
    fn values_never_span_a_long_boundary() {
        // 5 bits → 12 values per long with 4 bits of padding. If values spanned
        // boundaries, value 12 would appear in the low bits of long 1.
        let values: Vec<u32> = (0..24).map(|i| i % 32).collect();
        let packed = pack(&values, 5).expect("packs");
        assert_eq!(packed.len(), 2);
        assert_eq!((packed[1] as u64 >> 60) & 0xF, 0, "tail padding is zero");
        assert_eq!(unpack(&packed, 5, 24).expect("unpacks"), values);
    }

    #[test]
    fn out_of_range_bit_width_is_an_error_not_a_panic() {
        use crate::error::ServerError;
        assert!(matches!(
            pack(&[1, 2, 3], 0),
            Err(ServerError::Invariant(_))
        ));
        assert!(matches!(
            pack(&[1, 2, 3], 33),
            Err(ServerError::Invariant(_))
        ));
    }

    #[test]
    fn short_or_malformed_arrays_are_rejected() {
        use crate::error::ServerError;
        assert!(matches!(
            unpack(&[0i64; 3], 8, BLOCK_ENTRIES),
            Err(ServerError::CorruptData(_))
        ));
        assert!(matches!(
            unpack(&[0i64; 512], 0, BLOCK_ENTRIES),
            Err(ServerError::CorruptData(_))
        ));
        assert!(matches!(
            unpack(&[0i64; 512], 64, BLOCK_ENTRIES),
            Err(ServerError::CorruptData(_))
        ));
    }

    #[test]
    fn extra_longs_are_tolerated() {
        // Vanilla tolerates an array longer than needed; so do we, reading only
        // the first `entries` values.
        let values: Vec<u32> = (0..64).map(|i| i % 4).collect();
        let mut packed = pack(&values, 2).expect("packs");
        packed.push(0xDEAD_BEEF);
        assert_eq!(unpack(&packed, 2, 64).expect("unpacks"), values);
    }

    #[test]
    fn biome_container_geometry() {
        let values: Vec<u32> = (0..BIOME_ENTRIES as u32).map(|i| i % 3).collect();
        let packed = pack(&values, 2).expect("packs");
        assert_eq!(packed.len(), 2);
        assert_eq!(unpack(&packed, 2, BIOME_ENTRIES).expect("unpacks"), values);
    }
}
