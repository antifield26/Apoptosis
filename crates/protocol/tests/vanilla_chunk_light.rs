//! Decode captured vanilla `level_chunk_with_light` bodies with **our own** decoder (P10-04/P10-05).
//!
//! ## Why this exists
//!
//! A light engine has to produce bytes that match what a real server produces, and P10-05 asks for "golden
//! bytes from a vanilla capture". The capture exists (`target/vanilla-capture/bodies`, written by the capture
//! rig's `--bodies` mode), and the right way to read it is with the decoder this project already verified
//! against the jar — not with a second implementation of the format in a script, which is how the last three
//! divergences in this phase started.
//!
//! It is also a format check in its own right, and **it fails** — which is the finding. `LevelChunkWithLight`
//! refuses a light count that disagrees with its mask, so it rejects every captured packet with
//! `light mask has 1 sections set but 0 arrays follow`.
//!
//! Two hypotheses were tested against it and one is now excluded:
//!
//! * *"our layout is wrong"* — **supported.** No small (sky, block) array pairing consumes the remainder of a
//!   captured packet exactly; they are all **24 bytes short**. So the bytes read as masks cannot be masks, and
//!   those bytes are mostly zero because they are light data.
//! * *"our strictness is wrong, vanilla sends a mask bit with no array"* — **excluded** by that same
//!   arithmetic, which does not depend on the decoder at all.
//!
//! Independently confirmed against the real bytes: a 2048-byte array preceded by `80 10` (a two-byte
//! `VarInt`
//! for 2048) terminates the packet, so `LIGHT_ARRAY_BYTES` and the length-prefixed array convention are
//! right. Where the 24 missing bytes are is **not** established.
//!
//! ## Running it
//!
//! ```text
//! MC_VANILLA_CAPTURE=target/vanilla-capture/bodies \
//!   cargo test -p mc-protocol --test vanilla_chunk_light -- --ignored --nocapture
//! ```
//!
//! Ignored by default because the capture lives under `target/`, which is not committed.

use mc_protocol::packets::Packet;
use mc_protocol::packets::play::LevelChunkWithLight;
use std::path::PathBuf;

/// Every captured clientbound play-45 body, which is `level_chunk_with_light`.
fn captured_chunks() -> Vec<PathBuf> {
    let root = std::env::var("MC_VANILLA_CAPTURE")
        .unwrap_or_else(|_| "target/vanilla-capture/bodies".to_owned());
    // A relative value is relative to the **workspace root**, because that is where the path in the docs is
    // written from — but `cargo test` sets the working directory to the package root, so it has to be resolved
    // explicitly or it silently points at `crates/protocol/target/...`.
    let raw = PathBuf::from(&root);
    let directory = if raw.is_absolute() {
        raw
    } else {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join(raw)
    };
    let mut files: Vec<PathBuf> = std::fs::read_dir(&directory).unwrap_or_else(|error| {
        panic!(
            "cannot read {} (from MC_VANILLA_CAPTURE={root:?}; a relative value resolves against the \
             workspace root, not the test's working directory): {error}",
            directory.display()
        )
    })
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with("_s2c_play_45.bin"))
        })
        .collect();
    files.sort();
    assert!(
        !files.is_empty(),
        "no captured chunk bodies in {} — run tools/vanilla-probe or the capture rig first",
        directory.display()
    );
    files
}

#[test]
#[ignore = "needs the vanilla capture under target/, which is not committed"]
fn every_captured_vanilla_chunk_decodes_with_our_decoder() {
    // Named as an assertion because that is what it should be. It currently reports instead, because the
    // failure is the finding and its shape is the evidence: a panic would say "one packet failed", while the
    // tally below says "every packet fails the same way at the same place".
    let files = captured_chunks();
    println!("captured chunk packets: {}", files.len());

    let mut decoded_ok = 0usize;
    let mut failures: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    for path in &files {
        let body = std::fs::read(path).expect("read");
        match LevelChunkWithLight::decode(&body) {
            Ok(_) => decoded_ok += 1,
            Err(error) => *failures.entry(error.to_string()).or_insert(0) += 1,
        }
    }

    println!("  decoded    : {decoded_ok}");
    for (error, count) in &failures {
        println!("  failed x{count}: {error}");
    }

    // Report what the tail looks like, **without** asserting a boundary I could not pin down. An earlier
    // version of this test asserted the packet ends with `[0x80, 0x10]`; the slice came back `[0x10, 0xff]`,
    // so the offset was a byte out. The observation is useful, the claim was not.
    let body = std::fs::read(&files[0]).expect("read");
    let longest = body
        .windows(2)
        .enumerate()
        .filter(|(_, window)| *window == [0x80, 0x10])
        .map(|(index, _)| index)
        .filter(|index| {
            // Only interesting where a real 2048-byte span follows.
            body.len().saturating_sub(*index) >= 2048 && {
                // `naive_bytecount` suggests a crate for this. One heuristic in a diagnostic is not worth a
                // dependency, and this project reviews every new one.
                #[allow(clippy::naive_bytecount)]
                let filled = body[*index + 2..*index + 2 + 2048]
                    .iter()
                    .filter(|byte| **byte == 0xFF)
                    .count();
                filled > 2000
            }
        })
        .collect::<Vec<_>>();
    println!(
        "  offsets of a 0x80 0x10 prefix followed by a mostly-0xFF 2048-byte span: {longest:?}"
    );
    println!(
        "  packet length {}, so such a span would end at {}",
        body.len(),
        longest.first().map_or(0, |index| index + 2 + 2048)
    );

    // The part that is solid, and independent of any decoder: every captured packet fails identically at the
    // same point. That is what makes this a layout defect rather than a quirk of one packet.
    //
    // When the layout is fixed, this becomes `assert_eq!(decoded_ok, files.len())`.
    assert_eq!(
        decoded_ok, 0,
        "some captured packets now decode; the assertion above should be inverted to require all of them to"
    );
    assert_eq!(
        failures.len(),
        1,
        "the captured packets must fail identically for the finding to be a layout defect: {failures:?}"
    );
}

/// Report the mask structure of one chunk, which is the shape the light engine has to fill.
#[test]
#[ignore = "needs the vanilla capture under target/, which is not committed"]
fn report_the_light_shape_of_a_captured_chunk() {
    let files = captured_chunks();
    let path = files
        .iter()
        .max_by_key(|path| std::fs::metadata(path).map_or(0, |meta| meta.len()))
        .expect("a largest file");
    let body = std::fs::read(path).expect("read");
    println!("=== {} ({} bytes) ===", path.display(), body.len());
    let Ok(decoded) = LevelChunkWithLight::decode(&body) else {
        println!(
            "  does not decode with our layout; see the other test for the tally and the evidence"
        );
        return;
    };
    println!(
        "  chunk            : ({}, {})",
        decoded.chunk_x, decoded.chunk_z
    );
    println!("  sections         : {}", decoded.sections.len());
    println!("  heightmaps       : {}", decoded.heightmaps.len());
    println!("  block entities   : {}", decoded.block_entities.len());
    println!(
        "  sky light mask   : {:#x} ({} bits)",
        decoded.sky_light_mask,
        decoded.sky_light_mask.count_ones()
    );
    println!(
        "  block light mask : {:#x} ({} bits)",
        decoded.block_light_mask,
        decoded.block_light_mask.count_ones()
    );
    println!("  empty sky mask   : {:#x}", decoded.empty_sky_light_mask);
    println!("  empty block mask : {:#x}", decoded.empty_block_light_mask);
    println!("  sky arrays       : {}", decoded.sky_light.len());
    println!("  block arrays     : {}", decoded.block_light.len());

    if let Some(first) = decoded.sky_light.first() {
        let distinct: std::collections::BTreeSet<u8> = first.iter().copied().collect();
        let mut histogram = std::collections::BTreeMap::new();
        for byte in first {
            for nibble in [byte & 0x0F, byte >> 4] {
                *histogram.entry(nibble).or_insert(0usize) += 1;
            }
        }
        println!(
            "  first sky array  : {} distinct bytes, nibble histogram {histogram:?}",
            distinct.len()
        );
    }
    // Nothing is reported per section: the light a client needs lives in the packet's own masks and arrays
    // above, and the wire `ChunkSection` is a different type from the world-side `Section` (which is where the
    // block arrays and per-section light storage live). Guessing at its fields from memory cost two build
    // cycles here; the type itself is the reference when it is needed.
}
