//! Does the surface the server actually generates come out lit?
//!
//! ## Why this test exists, and why it is not the differential one
//!
//! The differential suite reports sky light **262144 / 262144 exact** against a real server, which reads as
//! conclusive and is not: 262 144 is 32 chunks x 2 sections, because the capture is a **superflat** world whose
//! remaining sections are empty. It verified a thin slice of a world with no terrain in it, and terrain is what
//! the owner reports a problem with —
//!
//! > a small number of surface blocks are dead black
//!
//! A block renders black when the client is told the light at the cell in front of it is `0`. For a block with
//! only air above it that cell is open to the sky, and open to the sky means sky light **15** — a block can only
//! reach 15 by being seeded with it, since spreading always loses at least one. So the invariant is sharp:
//!
//! **a cell with nothing but air above it must have sky light 15.**
//!
//! ## Why the other harness cannot be used
//!
//! [`survival_e2e`]'s harness builds its game with `Game::new`, which **borrows** storage. `can_read_stored_chunks`
//! is `storage.is_some()`, and a game that cannot tell "nothing is stored here" from "I cannot look" keeps an
//! all-air placeholder rather than generating (`game.rs:3141`). A first version of this check therefore examined
//! zero columns and failed on its own `checked > 0` guard, which is the one mercy of counting what you checked.
//!
//! This uses `Game::with_seed_and_storage` — the production tick-thread constructor — so terrain is generated,
//! and a join to drive the loading, because joining is what the server loads chunks for.

// Wire-format arithmetic: section, nibble and block indices, every one bounded by the format itself —
// sixteen cells an axis, sixteen levels a section, 2 048 bytes an array. Converting each cast would bury
// the arithmetic that is the subject. The same exemption, for the same reason, is at the top of `game.rs`.
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    clippy::cast_lossless
)]

use mc_network::bridge::{
    ClientEvent, ClientEventKind, ConnectionId, InboundReceiver, OutboundSender, game_channel,
};

use mc_persistence::chunk::ChunkPos;
use mc_server::config::StorageConfig;
use mc_server::game::Game;
use mc_server::storage::WorldService;
use mc_test_support::fixtures::TempDir;
use mc_world::light::MAX_LIGHT;

/// **The seed the production server actually uses**, not one of my own choosing.
///
/// The first version of this test picked its own, so it examined a different world from the one the capture and
/// the running server were looking at — and the two could not be compared, which is the comparison that decides
/// whether a wrong light value comes from the engine or from the wire.
const SEED: i64 = mc_server::game::DEFAULT_RANDOM_SEED;

#[tokio::test]
async fn surface_air_open_to_the_sky_is_fully_lit() {
    let dir = TempDir::new("p10-sky-surface");
    let config = StorageConfig {
        world_dir: dir.path().join("world"),
        autosave_ticks: 0,
    };
    let storage = WorldService::open(&config).expect("world opens");
    let (events, rx) = game_channel(256);
    let mut game = Game::with_seed_and_storage(storage, 4, rx, SEED).expect("game builds");

    // Joining is what loads chunks, and loading a chunk is what generates it — a few per tick under the
    // per-tick budget, so the ticks matter.
    let id = ConnectionId(1);
    let (outbound, _receiver): (OutboundSender, InboundReceiver) = OutboundSender::pair(id, 8192);
    events
        .try_send(ClientEvent {
            id,
            kind: ClientEventKind::Joined {
                profile: mc_network::auth::offline_profile("Watcher"),
                outbound,
            },
        })
        .expect("join queued");
    for _ in 0..120 {
        game.tick().expect("tick");
    }

    let (sx, _, sz) = game.spawn();
    let centre = ChunkPos::new(sx >> 4, sz >> 4);
    let air = game.registries().blocks.air_id();

    // A 3x3, so chunk borders are inspected too: the light engine works on a one-block margin (KD-47), and a
    // border-only defect would hide inside a single chunk.
    // **-4..=4, not -1..=1.** The first version checked a 3x3 around spawn and passed, while the chunks that
    // actually fail on the wire are at the edge of the view — so the narrow version proved nothing about them.
    let positions: Vec<ChunkPos> = (-4..=4)
        .flat_map(|dx| (-4..=4).map(move |dz| ChunkPos::new(centre.x + dx, centre.z + dz)))
        .collect();

    let loaded = positions
        .iter()
        .filter(|pos| game.world().chunk(**pos).is_some())
        .count();
    assert!(
        loaded > 0,
        "no chunk in the 3x3 around spawn was loaded, so the join generated nothing and this proves nothing"
    );

    let table = game.registries().light.clone();
    for pos in &positions {
        if game.world().chunk(*pos).is_some() {
            game.world_mut()
                .compute_light(*pos, &table)
                .expect("light computes");
        }
    }

    let world = game.world();
    let min_y = i32::from(world.min_section_y()) * 16;
    let top_y =
        min_y + i32::try_from(world.section_count()).expect("section count fits in i32") * 16;

    let mut checked = 0_u32;
    let mut dark: Vec<String> = Vec::new();
    for pos in &positions {
        let Some(light) = world.cached_light(*pos) else {
            continue;
        };
        for x in 0..16_i32 {
            for z in 0..16_i32 {
                let wx = pos.x * 16 + x;
                let wz = pos.z * 16 + z;
                let Some(top) = (min_y..top_y)
                    .rev()
                    .find(|y| world.get_block(wx, *y, wz) != air)
                else {
                    continue;
                };
                let y = top + 1;
                if y >= top_y {
                    continue;
                }
                let offset = y - min_y;
                let section =
                    usize::try_from(offset / 16).expect("a section index is not negative");
                let local = offset % 16;
                let sky = light.sky[section].get(x, local, z);
                checked += 1;
                if sky != MAX_LIGHT {
                    dark.push(format!("({wx}, {y}, {wz}) sky={sky}"));
                }
            }
        }
    }

    assert!(
        checked > 0,
        "no columns were checked, so this proves nothing"
    );
    assert!(
        dark.is_empty(),
        "{} of {checked} cells open to the sky are not fully lit; first ten: {:?}",
        dark.len(),
        &dark[..dark.len().min(10)]
    );
}

/// The same surface invariant, asked of the bytes the server actually sent.
///
/// See the module comment: the engine passes this test on generated terrain, so if the wire fails it the fault
/// is in the four masks or the array ordering, and the client would show exactly the reported black patches.
///
/// Needs a capture: `python tools/surface-capture/run.py`.
// One linear narrative — decode, rebuild the client's view, walk the columns, assert — on purpose;
// splitting it across helpers would hide the ordering that makes it a test. The same allowance and the
// same reason as `survival_e2e.rs`.
#[allow(clippy::too_many_lines)]
#[test]
#[ignore = "needs bodies captured by tools/surface-capture/run.py"]
fn the_wire_carries_light_a_client_would_read_correctly() {
    use mc_protocol::packets::Packet;
    use mc_protocol::packets::play::LevelChunkWithLight;
    use mc_registry::Registries;
    use std::collections::BTreeSet;

    const MIN_SECTION_Y: i32 = -4;

    // cargo test runs with the crate directory as the working directory, so the capture is reached through
    // the manifest rather than a path relative to the repository root.
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/surface-capture/bodies");
    let dir = dir.as_path();
    assert!(
        dir.is_dir(),
        "no capture at {} — run `python tools/surface-capture/run.py` first",
        dir.display()
    );
    let registries = Registries::vanilla().expect("the shipped registry tables load");
    let air = registries.blocks.air_id();

    let mut chunks = 0_u32;
    let mut checked = 0_u32;
    let mut dark: Vec<String> = Vec::new();
    let mut mismatched_masks = Vec::new();
    let mut failures: Vec<String> = Vec::new();

    for entry in std::fs::read_dir(dir).expect("the capture directory reads") {
        let path = entry.expect("entry").path();
        let name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        if !name.ends_with("_s2c_play_45.bin") {
            continue;
        }
        let body = std::fs::read(&path).expect("body reads");
        // The rig may write the packet **body** (id `VarInt` then payload, as the trace's `head` holds) or the
        // **payload alone**. Both are tried rather than assumed, and the failure is reported rather than
        // discarded: `let Ok(..) else { continue }` is what turned "this cannot decode" into "nothing was
        // found".
        let mut decoded = LevelChunkWithLight::decode(&body);
        if decoded.is_err() {
            let mut at = 0;
            while at < body.len() && body[at] & 0x80 != 0 {
                at += 1;
            }
            decoded = LevelChunkWithLight::decode(&body[(at + 1).min(body.len())..]);
        }
        let chunk = match decoded {
            Ok(chunk) => chunk,
            Err(error) => {
                failures.push(format!("{name}: {error}"));
                continue;
            }
        };
        chunks += 1;

        // A client reads light section `i` from bit `i`, consuming arrays in ascending bit order. A bit in the
        // `empty_*` mask means that layer's default for the whole section; a bit in neither means zero.
        let light_sections = chunk.sections.len() + 2;
        // **The masks here are lists of set section indices, not bitmask words.** Reading them as words made
        // a mask of nine indices look like one set bit and produced a confident, wrong failure.
        let set = |mask: &[u32], bit: usize| -> bool {
            mask.contains(&u32::try_from(bit).expect("a section index fits in u32"))
        };
        let mut sky: Vec<Option<Vec<u8>>> = vec![None; light_sections];
        let mut next = 0_usize;
        for (bit, slot) in sky.iter_mut().enumerate() {
            if set(&chunk.sky_light_mask, bit) {
                if let Some(array) = chunk.sky_light.get(next) {
                    *slot = Some(array.clone());
                }
                next += 1;
            } else if set(&chunk.empty_sky_light_mask, bit) {
                *slot = Some(vec![0xFF_u8; 2048]);
            } else {
                *slot = Some(vec![0; 2048]);
            }
        }
        // Every light section must be accounted for exactly once, in one mask or the other.
        let both: BTreeSet<usize> = (0..light_sections)
            .filter(|bit| {
                set(&chunk.sky_light_mask, *bit) && set(&chunk.empty_sky_light_mask, *bit)
            })
            .collect();
        if !both.is_empty() {
            mismatched_masks.push(format!("{name}: sections {both:?} in both masks"));
        }
        if next != chunk.sky_light.len() {
            mismatched_masks.push(format!(
                "{}: {} arrays for {} set bits",
                name,
                chunk.sky_light.len(),
                next
            ));
        }

        let nibble = |array: &[u8], x: i32, y: i32, z: i32| -> u8 {
            let index = (y as usize) * 256 + (z as usize) * 16 + x as usize;
            let byte = array[index / 2];
            if index.is_multiple_of(2) {
                byte & 0x0F
            } else {
                byte >> 4
            }
        };

        for x in 0..16_i32 {
            for z in 0..16_i32 {
                // The topmost non-air cell of the column, read from the section palettes.
                let mut top: Option<i32> = None;
                for section in (0..chunk.sections.len()).rev() {
                    let data = &chunk.sections[section];
                    if data.block_count <= 0 {
                        continue;
                    }
                    for local_y in (0..16_i32).rev() {
                        let index = (local_y as usize) * 256 + (z as usize) * 16 + x as usize;
                        let value = *data.block_states.values.get(index).unwrap_or(&0) as usize;
                        let state = *data.block_states.palette.get(value).unwrap_or(&0);
                        if state != air.cast_unsigned() {
                            let world_y = (MIN_SECTION_Y + section as i32) * 16 + local_y;
                            top = Some(world_y);
                            break;
                        }
                    }
                    if top.is_some() {
                        break;
                    }
                }
                let Some(top) = top else { continue };
                let y = top + 1;
                let offset = y - MIN_SECTION_Y * 16;
                // **The offset is the whole convention**: light section `i` holds world section `i - 1`, so
                // world section 8 is light section 9. Reading `offset / 16` directly looks up the section
                // *below* the one asked about — underground, where sky light is genuinely 0.
                let light_section = (offset / 16 + 1) as usize;
                let local_y = offset % 16;
                let Some(array) = sky.get(light_section).and_then(|a| a.as_ref()) else {
                    continue;
                };
                let level = nibble(array, x, local_y, z);
                checked += 1;
                if level != mc_world::light::MAX_LIGHT {
                    dark.push(format!(
                        "chunk({}, {}) ({}, {y}, {}) sky={level}",
                        chunk.chunk_x, chunk.chunk_z, x, z
                    ));
                }
            }
        }
    }

    assert!(
        chunks > 0,
        "no chunk bodies were decoded; {} failed, first three: {:?}",
        failures.len(),
        &failures[..failures.len().min(3)]
    );
    assert!(
        checked > 0,
        "no columns were checked, so this proves nothing"
    );
    assert!(
        mismatched_masks.is_empty(),
        "the masks and arrays disagree: {:?}",
        &mismatched_masks[..mismatched_masks.len().min(5)]
    );
    if let Some(first) = dark.first() {
        println!("=== first not-fully-lit cell on the wire: {first} ===");
        // Same parse as the dump in the engine-side test: pull the cell out of the message.
        let nums: Vec<i32> = first
            .split(|c: char| !c.is_ascii_digit() && c != '-')
            .filter(|part| !part.is_empty() && *part != "-")
            .filter_map(|part| part.parse().ok())
            .collect();
        if let (Some(cx), Some(cz), Some(lx), Some(wy), Some(lz)) = (
            nums.first(),
            nums.get(1),
            nums.get(2),
            nums.get(3),
            nums.get(4),
        ) {
            for entry in std::fs::read_dir(dir).expect("reads") {
                let path = entry.expect("entry").path();
                let name = path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string();
                if !name.ends_with("_s2c_play_45.bin") {
                    continue;
                }
                let Ok(chunk) = LevelChunkWithLight::decode(&std::fs::read(&path).expect("reads"))
                else {
                    continue;
                };
                if (chunk.chunk_x, chunk.chunk_z) != (*cx, *cz) {
                    continue;
                }
                // Blocks and light from the same numbering: world sections for blocks, light sections for
                // light, which differ by one and have been confused twice already.
                let block_at = |y: i32, x: i32, z: i32| -> Option<u32> {
                    let offset = y - MIN_SECTION_Y * 16;
                    if offset < 0 {
                        return None;
                    }
                    let data = chunk.sections.get((offset / 16) as usize)?;
                    let local_y = (offset % 16) as usize;
                    let index = local_y * 256 + (z as usize) * 16 + x as usize;
                    let value = *data.block_states.values.get(index).unwrap_or(&0) as usize;
                    Some(*data.block_states.palette.get(value).unwrap_or(&0))
                };
                let mut layers: Vec<Option<Vec<u8>>> = Vec::new();
                let mut next = 0_usize;
                for bit in 0..chunk.sections.len() + 2 {
                    if chunk.sky_light_mask.contains(&(bit as u32)) {
                        layers.push(chunk.sky_light.get(next).cloned());
                        next += 1;
                    } else if chunk.empty_sky_light_mask.contains(&(bit as u32)) {
                        layers.push(Some(vec![0xFF_u8; 2048]));
                    } else {
                        layers.push(Some(vec![0; 2048]));
                    }
                }
                let sky_at = |y: i32, x: i32, z: i32| -> Option<u8> {
                    let offset = y - MIN_SECTION_Y * 16;
                    if offset < 0 {
                        return None;
                    }
                    let array = layers.get((offset / 16 + 1) as usize)?.as_ref()?;
                    let index = ((offset % 16) as usize) * 256 + (z as usize) * 16 + x as usize;
                    let byte = array[index / 2];
                    Some(if index.is_multiple_of(2) {
                        byte & 0x0F
                    } else {
                        byte >> 4
                    })
                };
                println!("  {name}  sections={}", chunk.sections.len());
                for index in [6_usize, 7, 8] {
                    if let Some(data) = chunk.sections.get(index) {
                        let first_values: Vec<u32> =
                            data.block_states.values.iter().take(6).copied().collect();
                        println!(
                            "    section {index}: palette={:?} values[0..6]={first_values:?}",
                            data.block_states.palette
                        );
                    }
                }
                for (index, data) in chunk.sections.iter().enumerate() {
                    let y = (MIN_SECTION_Y + index as i32) * 16;
                    println!(
                        "    section {index:>2} (y {y:>4}..{:>4})  block_count={:<5} palette={:<4} values={}",
                        y + 15,
                        data.block_count,
                        data.block_states.palette.len(),
                        data.block_states.values.len()
                    );
                }
                println!("  sky_mask={:?}", chunk.sky_light_mask);
                println!("  empty_sky={:?}", chunk.empty_sky_light_mask);
                println!("  column local(x={lx}, z={lz}), flagged at y={wy}");
                for y in (40..=88).rev() {
                    let mark = if y == *wy { "  <-- flagged" } else { "" };
                    println!(
                        "    y={y:>4}  state={:<6} sky={}{mark}",
                        block_at(y, *lx, *lz).map_or_else(|| "?".to_owned(), |s| s.to_string()),
                        sky_at(y, *lx, *lz).map_or_else(|| "?".to_owned(), |s| s.to_string()),
                    );
                }
                break;
            }
        }
    }
    assert!(
        dark.is_empty(),
        "{} of {checked} surface cells open to the sky are not fully lit **on the wire**; first ten: {:?}",
        dark.len(),
        &dark[..dark.len().min(10)]
    );
}

/// Adjacent cells may not differ by more than the cost of entering the darker one.
///
/// See the module comment: this is the invariant the "open to the sky" test cannot express, and it is the one a
/// one-block margin breaks. The check runs **across a chunk border**, because that is where two independent
/// computations meet and where light that should have arrived from outside is missing.
#[tokio::test]
async fn light_does_not_jump_between_adjacent_cells_at_a_chunk_border() {
    let dir = TempDir::new("p10-sky-border");
    let config = StorageConfig {
        world_dir: dir.path().join("world"),
        autosave_ticks: 0,
    };
    let storage = WorldService::open(&config).expect("world opens");
    let (events, rx) = game_channel(256);
    let mut game = Game::with_seed_and_storage(storage, 4, rx, SEED).expect("game builds");

    let id = ConnectionId(1);
    let (outbound, _receiver): (OutboundSender, InboundReceiver) = OutboundSender::pair(id, 8192);
    events
        .try_send(ClientEvent {
            id,
            kind: ClientEventKind::Joined {
                profile: mc_network::auth::offline_profile("Watcher"),
                outbound,
            },
        })
        .expect("join queued");
    for _ in 0..120 {
        game.tick().expect("tick");
    }

    let (sx, _, sz) = game.spawn();
    let centre = ChunkPos::new(sx >> 4, sz >> 4);
    let table = game.registries().light.clone();
    let positions = [centre, ChunkPos::new(centre.x + 1, centre.z)];

    for pos in &positions {
        game.world_mut()
            .compute_light(*pos, &table)
            .expect("light computes");
    }

    let world = game.world();
    let min_y = i32::from(world.min_section_y()) * 16;
    let top_y = min_y + i32::try_from(world.section_count()).expect("fits") * 16;

    // Light at a world position, from whichever of the two chunks holds it.
    let light_at = |x: i32, y: i32, z: i32| -> Option<u8> {
        let pos = ChunkPos::new(x.div_euclid(16), z.div_euclid(16));
        let light = world.cached_light(pos)?;
        let offset = y - min_y;
        if offset < 0 {
            return None;
        }
        let section = usize::try_from(offset / 16).ok()?;
        let local = offset % 16;
        let lx = x.rem_euclid(16);
        let lz = z.rem_euclid(16);
        Some(light.sky[section].get(lx, local, lz))
    };

    let border = centre.x * 16 + 15;
    let air = game.registries().blocks.air_id();
    let mut checked = 0_u32;
    let mut jumps: Vec<String> = Vec::new();

    for y in (min_y..top_y).step_by(1) {
        for z in (centre.z * 16)..(centre.z * 16 + 16) {
            // Only air matters: an opaque cell's own stored level is not what a client renders.
            if world.get_block(border, y, z) != air || world.get_block(border + 1, y, z) != air {
                continue;
            }
            let (Some(left), Some(right)) = (light_at(border, y, z), light_at(border + 1, y, z))
            else {
                continue;
            };
            checked += 1;
            // The darker side must be reachable from the brighter one, at a cost of at least one level.
            let (hi, lo) = (left.max(right), left.min(right));
            if hi > lo + 1 {
                jumps.push(format!("y={y} z={z}: {left} | {right}"));
            }
        }
    }

    println!("checked {checked} air cells across the border at x={border}");
    assert!(checked > 0, "no cells were checked, so this proves nothing");
    assert!(
        jumps.is_empty(),
        "{} of {checked} adjacent air cells across the border differ by more than one level; first ten: {:?}",
        jumps.len(),
        &jumps[..jumps.len().min(10)]
    );
}

/// The `light_update` a block change produces must be as sound as the chunk packet it follows.
///
/// See the module comment: the first capture contained none of these, so KD-49 rested on a packet that was never
/// sent. This is the check that was missing.
///
/// Needs a capture that includes a block change: `python tools/surface-capture/run.py`.
#[allow(clippy::too_many_lines)]
#[test]
#[ignore = "needs bodies captured by tools/surface-capture/run.py"]
fn a_light_update_is_as_sound_as_the_chunk_packet_it_follows() {
    use mc_protocol::packets::Packet;
    use mc_protocol::packets::play::{LevelChunkWithLight, LightUpdate};
    use std::collections::BTreeMap;

    const MIN_SECTION_Y: i32 = -4;

    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/surface-capture/bodies");
    assert!(
        dir.is_dir(),
        "no capture at {} — run `python tools/surface-capture/run.py`",
        dir.display()
    );

    // The blocks the client already has, from the chunk packets, and the light the updates then carry.
    let mut blocks: BTreeMap<(i32, i32), LevelChunkWithLight> = BTreeMap::new();
    let mut updates: Vec<LightUpdate> = Vec::new();
    let mut structural: Vec<String> = Vec::new();
    for entry in std::fs::read_dir(&dir).expect("the capture directory reads") {
        let path = entry.expect("entry").path();
        let name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        let body = std::fs::read(&path).expect("body reads");
        if name.ends_with("_s2c_play_45.bin") {
            if let Ok(chunk) = LevelChunkWithLight::decode(&body) {
                blocks.insert((chunk.chunk_x, chunk.chunk_z), chunk);
            }
        } else if name.ends_with("_s2c_play_48.bin") {
            // **The file is the payload alone** — the id is in the file *name*. Stripping a byte here cut the
            // first `VarInt` off a body that begins with one, and nothing decoded.
            match LightUpdate::decode(&body) {
                Ok(update) => updates.push(update),
                Err(error) => structural.push(format!("{name}: {error}")),
            }
        }
    }
    assert!(
        !updates.is_empty(),
        "the capture has no light_update bodies; did the block change land?"
    );

    let air = mc_registry::Registries::vanilla()
        .expect("registries")
        .blocks
        .air_id();
    let mut checked = 0_u32;
    let mut dark: Vec<String> = Vec::new();

    for update in &updates {
        let light_sections = blocks
            .get(&(update.chunk_x, update.chunk_z))
            .map_or(26, |chunk| chunk.sections.len() + 2);

        let set = |mask: &[u32], bit: usize| -> bool {
            mask.contains(&u32::try_from(bit).expect("a section index fits in u32"))
        };
        // Every section in exactly one mask: in neither means the client reads zero.
        for bit in 0..light_sections {
            let sky = set(&update.sky_light_mask, bit) as u8
                + set(&update.empty_sky_light_mask, bit) as u8;
            let block = set(&update.block_light_mask, bit) as u8
                + set(&update.empty_block_light_mask, bit) as u8;
            if sky != 1 || block != 1 {
                structural.push(format!(
                    "chunk({}, {}) section {bit}: sky in {sky} masks, block in {block}",
                    update.chunk_x, update.chunk_z
                ));
            }
        }
        let sky_bits = (0..light_sections)
            .filter(|bit| set(&update.sky_light_mask, *bit))
            .count();
        if sky_bits != update.sky_light.len() {
            structural.push(format!(
                "chunk({}, {}): {} arrays for {sky_bits} set bits",
                update.chunk_x,
                update.chunk_z,
                update.sky_light.len()
            ));
        }

        // Rebuild the light the client would hold after this update and re-ask the surface question.
        let mut sky: Vec<Option<Vec<u8>>> = vec![None; light_sections];
        let mut next = 0_usize;
        for (bit, slot) in sky.iter_mut().enumerate() {
            if set(&update.sky_light_mask, bit) {
                if let Some(array) = update.sky_light.get(next) {
                    *slot = Some(array.clone());
                }
                next += 1;
            } else if set(&update.empty_sky_light_mask, bit) {
                *slot = Some(vec![0xFF_u8; 2048]);
            } else {
                *slot = Some(vec![0; 2048]);
            }
        }

        let Some(chunk) = blocks.get(&(update.chunk_x, update.chunk_z)) else {
            continue;
        };
        for x in 0..16_i32 {
            for z in 0..16_i32 {
                let mut top: Option<i32> = None;
                for section in (0..chunk.sections.len()).rev() {
                    let data = &chunk.sections[section];
                    if data.block_count <= 0 {
                        continue;
                    }
                    for local_y in (0..16_i32).rev() {
                        let index = (local_y as usize) * 256 + (z as usize) * 16 + x as usize;
                        let value = *data.block_states.values.get(index).unwrap_or(&0) as usize;
                        let state = *data.block_states.palette.get(value).unwrap_or(&0);
                        if state != air.cast_unsigned() {
                            top = Some((MIN_SECTION_Y + section as i32) * 16 + local_y);
                            break;
                        }
                    }
                    if top.is_some() {
                        break;
                    }
                }
                let Some(top) = top else { continue };
                let y = top + 1;
                let offset = y - MIN_SECTION_Y * 16;
                let light_section = (offset / 16 + 1) as usize;
                let local_y = offset % 16;
                let Some(array) = sky.get(light_section).and_then(|a| a.as_ref()) else {
                    continue;
                };
                let index = (local_y as usize) * 256 + (z as usize) * 16 + x as usize;
                let byte = array[index / 2];
                let level = if index.is_multiple_of(2) {
                    byte & 0x0F
                } else {
                    byte >> 4
                };
                checked += 1;
                if level != mc_world::light::MAX_LIGHT {
                    dark.push(format!(
                        "chunk({}, {}) ({x}, {y}, {z}) sky={level}",
                        update.chunk_x, update.chunk_z
                    ));
                }
            }
        }
    }

    assert!(checked > 0, "no surface cells were checked");
    assert!(
        structural.is_empty(),
        "{} structural problems in the light updates; first five: {:?}",
        structural.len(),
        &structural[..structural.len().min(5)]
    );
    assert!(
        dark.is_empty(),
        "{} of {checked} surface cells are dark after a light update; first ten: {:?}",
        dark.len(),
        &dark[..dark.len().min(10)]
    );
}
