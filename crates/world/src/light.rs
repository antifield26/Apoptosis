//! Light propagation (P10-04).
//!
//! ## The model, and where it comes from
//!
//! The three per-state inputs are read from the jar's own accessors by
//! `tools/vanilla-probe/LightProbe.java`, which boots the registry the dedicated server boots and calls
//! `getLightEmission()`, `getLightDampening()` and `propagatesSkylightDown()` — the same methods
//! `LevelLightEngine` reads. Nothing here is transcribed from a wiki: a wrong dampening produces light that
//! is plausible everywhere and correct nowhere, so the values had to come from the object graph that the
//! client is built against.
//!
//! ## The two layers
//!
//! * **Sky light** starts at 15 at the top of the world and falls straight down undiminished while every
//!   block it passes `propagatesSkylightDown()`. Once something stops it, the column below is dark and light
//!   can only arrive sideways, losing at least one level per block.
//! * **Block light** starts at each state's emission and spreads the same way, losing `max(1, dampening)` per
//!   block.
//!
//! Both then spread horizontally by breadth-first search. Spreading *outward* from a cell that is already
//! at its final value is what makes one pass sufficient: a cell is only re-queued when its level actually
//! increases, so the search terminates.
//!
//! ## The approximation, stated rather than implied
//!
//! A chunk is computed with a **one-block margin** in x and z, read through the caller's accessor, so light
//! flows in from neighbours. Where the accessor reports a chunk that is not loaded, the cell is treated as
//! air — sky-visible and undamped. That is the same assumption the client makes about ungenerated space, and
//! it means a chunk at the edge of the loaded area can be brighter at its border than it would be once the
//! neighbour exists. The alternative, treating an unloaded neighbour as opaque, would make every frontier
//! chunk visibly dark, which is worse and less true.

use mc_core::error::{ServerError, ServerResult};
use std::collections::VecDeque;

/// The brightest light level.
pub const MAX_LIGHT: u8 = 15;

/// Bytes in one section's nibble-packed light array.
pub const LIGHT_ARRAY_BYTES: usize = 2048;

/// Blocks along one axis of a section.
const SECTION_WIDTH: i32 = 16;

/// The per-state light properties, re-exported from the registry crate.
///
/// The dependency runs `mc_world -> mc_registry` (a `World` is built from a `BlockRegistry`), so the table
/// belongs upstream; the engine here is what consumes it.
pub use mc_registry::light::LightTable;

/// One section's light, nibble-packed exactly as the wire carries it.
#[derive(Clone, PartialEq, Eq)]
pub struct LightArray {
    bytes: [u8; LIGHT_ARRAY_BYTES],
}

impl Default for LightArray {
    fn default() -> Self {
        Self {
            bytes: [0; LIGHT_ARRAY_BYTES],
        }
    }
}

impl std::fmt::Debug for LightArray {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The interesting property of a light array is its histogram, not 2048 bytes of hex.
        let mut seen = [0usize; 16];
        for byte in self.bytes {
            seen[usize::from(byte & 0x0F)] += 1;
            seen[usize::from(byte >> 4)] += 1;
        }
        write!(out, "LightArray{seen:?}")
    }
}

impl LightArray {
    /// The flat cell index Vanilla uses: `x` varies fastest, then `z`, then `y`.
    #[must_use]
    pub const fn index(x: i32, y: i32, z: i32) -> usize {
        (x + z * SECTION_WIDTH + y * SECTION_WIDTH * SECTION_WIDTH) as usize
    }

    /// The light at a local position.
    #[must_use]
    pub fn get(&self, x: i32, y: i32, z: i32) -> u8 {
        let cell = Self::index(x, y, z);
        let byte = self.bytes[cell / 2];
        if cell.is_multiple_of(2) {
            byte & 0x0F
        } else {
            byte >> 4
        }
    }

    /// Set the light at a local position.
    pub fn set(&mut self, x: i32, y: i32, z: i32, level: u8) {
        let cell = Self::index(x, y, z);
        let byte = &mut self.bytes[cell / 2];
        if cell.is_multiple_of(2) {
            *byte = (*byte & 0xF0) | (level & 0x0F);
        } else {
            *byte = (*byte & 0x0F) | ((level & 0x0F) << 4);
        }
    }

    /// The bytes as the wire carries them.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8; LIGHT_ARRAY_BYTES] {
        &self.bytes
    }

    /// An array with every cell at `level`.
    ///
    /// Used for the light sections outside the world, which contain no blocks: full sky and no block light.
    #[must_use]
    pub fn filled(level: u8) -> Self {
        let level = level & 0x0F;
        Self {
            bytes: [level | (level << 4); LIGHT_ARRAY_BYTES],
        }
    }

    /// The single level every cell holds, when they agree.
    ///
    /// This is what the `empty_*` masks describe: a section whose light is uniform needs no array, and the
    /// mask tells the client which uniform value to use.
    #[must_use]
    pub fn uniform(&self) -> Option<u8> {
        let first = self.get(0, 0, 0);
        self.bytes
            .iter()
            .all(|byte| byte & 0x0F == first && byte >> 4 == first)
            .then_some(first)
    }
}

/// Computed light for one chunk column, one array per section.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ChunkLight {
    /// Sky light, bottom section first.
    pub sky: Vec<LightArray>,
    /// Block light, bottom section first.
    pub block: Vec<LightArray>,
}

/// Compute sky and block light for one chunk column.
///
/// `block_at` answers with a **block-state id** at world coordinates, or `None` for a chunk that is not
/// loaded. The region read is the chunk plus a one-block margin in x and z, so light crosses chunk borders
/// instead of stopping at them.
///
/// # Errors
///
/// [`ServerError::Invariant`] when `section_count` is zero or the vertical extent overflows.
pub fn compute_chunk_light(
    table: &LightTable,
    chunk_x: i32,
    chunk_z: i32,
    min_y: i32,
    section_count: usize,
    block_at: impl Fn(i32, i32, i32) -> Option<i32>,
) -> ServerResult<ChunkLight> {
    // Declared first: a `const` exists from the start of its scope wherever it is written, and putting it
    // below the guard reads as though it were conditional on it.
    const MARGIN: i32 = 1;

    if section_count == 0 {
        return Err(ServerError::Invariant(
            "a chunk with no sections has no light".to_owned(),
        ));
    }
    let height = i32::try_from(section_count)
        .ok()
        .and_then(|sections| sections.checked_mul(SECTION_WIDTH))
        .ok_or_else(|| ServerError::Invariant("chunk height overflows".to_owned()))?;

    // Work region: one block of margin in x and z, the full column in y.
    let width = SECTION_WIDTH + 2 * MARGIN; // 18
    let origin_x = chunk_x * SECTION_WIDTH - MARGIN;
    let origin_z = chunk_z * SECTION_WIDTH - MARGIN;
    let cells = usize::try_from(width * width * height)
        .map_err(|_| ServerError::Invariant("chunk region is too large".to_owned()))?;

    let index = |x: i32, y: i32, z: i32| -> usize {
        let lx = x - origin_x;
        let lz = z - origin_z;
        let ly = y - min_y;
        (lx + lz * width + ly * width * width) as usize
    };

    let mut sky = vec![0u8; cells];
    let mut block = vec![0u8; cells];
    let mut sky_queue: VecDeque<(i32, i32, i32)> = VecDeque::new();
    let mut block_queue: VecDeque<(i32, i32, i32)> = VecDeque::new();

    let top = min_y + height - 1;

    // --- seeding ------------------------------------------------------------------------
    for lx in 0..width {
        for lz in 0..width {
            let x = origin_x + lx;
            let z = origin_z + lz;

            // Sky: fall straight down while every block lets it through undiminished.
            let level = MAX_LIGHT;
            let mut y = top;
            while y >= min_y {
                let state = block_at(x, y, z).unwrap_or(0);
                if level == MAX_LIGHT && table.propagates_skylight_down(state) {
                    // Recorded, not queued: the frontier scan below decides which cells can spread. Queueing
                    // every lit cell here cost 124 000 queue entries per chunk for an open column.
                    sky[index(x, y, z)] = MAX_LIGHT;
                } else {
                    // Stopped: nothing below this column is lit from above, and spreading takes over.
                    break;
                }
                y -= 1;
            }

            // Block: every emitter seeds its own level.
            for y in min_y..=top {
                let state = block_at(x, y, z).unwrap_or(0);
                let emission = table.emission(state);
                if emission > 0 {
                    let cell = index(x, y, z);
                    if emission > block[cell] {
                        block[cell] = emission;
                    }
                }
            }
        }
    }

    queue_frontier(
        &sky,
        &block,
        &mut sky_queue,
        &mut block_queue,
        width,
        origin_x,
        origin_z,
        min_y,
        top,
    );

    // --- spreading ----------------------------------------------------------------------
    // Both layers use the same rule; only the sky's straight-down case differs, and only when the source is
    // already at full strength.
    spread(
        &mut sky,
        &mut sky_queue,
        width,
        origin_x,
        origin_z,
        min_y,
        top,
        &block_at,
        table,
        true,
    );
    spread(
        &mut block,
        &mut block_queue,
        width,
        origin_x,
        origin_z,
        min_y,
        top,
        &block_at,
        table,
        false,
    );

    // --- copy the interior out ----------------------------------------------------------
    Ok(copy_interior(
        &sky,
        &block,
        chunk_x,
        chunk_z,
        min_y,
        section_count,
        width,
        origin_x,
        origin_z,
    ))
}

/// Copy the chunk's own 16x16 cells out of a work region that carries a one-block margin.
///
/// The margin exists so light crosses chunk borders; the client only wants the interior, indexed the way a
/// [`LightArray`] expects.
#[allow(clippy::too_many_arguments)]
fn copy_interior(
    sky: &[u8],
    block: &[u8],
    chunk_x: i32,
    chunk_z: i32,
    min_y: i32,
    section_count: usize,
    width: i32,
    origin_x: i32,
    origin_z: i32,
) -> ChunkLight {
    let index = |x: i32, y: i32, z: i32| -> usize {
        let lx = x - origin_x;
        let lz = z - origin_z;
        let ly = y - min_y;
        (lx + lz * width + ly * width * width) as usize
    };

    let mut result = ChunkLight {
        sky: vec![LightArray::default(); section_count],
        block: vec![LightArray::default(); section_count],
    };
    for section in 0..section_count {
        let section_y = min_y + i32::try_from(section).unwrap_or(0) * SECTION_WIDTH;
        for x in 0..SECTION_WIDTH {
            for z in 0..SECTION_WIDTH {
                for y in 0..SECTION_WIDTH {
                    let world_x = chunk_x * SECTION_WIDTH + x;
                    let world_z = chunk_z * SECTION_WIDTH + z;
                    let world_y = section_y + y;
                    let cell = index(world_x, world_y, world_z);
                    result.sky[section].set(x, y, z, sky[cell]);
                    result.block[section].set(x, y, z, block[cell]);
                }
            }
        }
    }
    result
}

/// Queue every cell that can raise a neighbour's light.
///
/// A cell can only spread if some neighbour is strictly darker: with `candidate = level - max(1, dampening)` a
/// neighbour at or above `level` can never be raised, so a cell whose neighbours are all at least as bright
/// cannot start a chain.
///
/// Seeding does not queue; this does. Queueing every lit cell during seeding cost 124 000 entries per chunk
/// for an open column, which is what made chunk sends slow enough to time out an unrelated command test.
#[allow(clippy::too_many_arguments)]
fn queue_frontier(
    sky: &[u8],
    block: &[u8],
    sky_queue: &mut VecDeque<(i32, i32, i32)>,
    block_queue: &mut VecDeque<(i32, i32, i32)>,
    width: i32,
    origin_x: i32,
    origin_z: i32,
    min_y: i32,
    top: i32,
) {
    let plane = (width * width) as usize;
    let height_cells = (top - min_y + 1) as usize;
    for lz in 0..width {
        for lx in 0..width {
            let column_base = (lx + lz * width) as usize;
            for ly in 0..height_cells {
                let cell = column_base + ly * plane;
                let x = origin_x + lx;
                let z = origin_z + lz;
                let y = min_y + ly as i32;

                if sky[cell] > 0 && darker_neighbour(sky, cell, width as usize, plane) {
                    sky_queue.push_back((x, y, z));
                }
                if block[cell] > 0 && darker_neighbour(block, cell, width as usize, plane) {
                    block_queue.push_back((x, y, z));
                }
            }
        }
    }
}

/// Whether any of a cell's six neighbours holds less light than the cell itself.
///
/// The precise precondition for a cell being able to spread: with `candidate = level - max(1, dampening)` a
/// neighbour at or above `level` can never be raised, so a cell whose neighbours are all at least as bright
/// cannot start a chain and does not need queueing.
///
/// Neighbours are the three flat-array strides — `1` for x, `width` for z, `width * width` for y — so the
/// interior needs no coordinate arithmetic at all, which is the point: this scan runs over 124 320 cells per
/// layer per chunk.
fn darker_neighbour(light: &[u8], cell: usize, width: usize, plane: usize) -> bool {
    let own = light[cell];
    let in_plane = cell % plane;
    let row = in_plane / width;
    let column = in_plane % width;

    if cell >= plane && light[cell - plane] < own {
        return true;
    }
    if cell + plane < light.len() && light[cell + plane] < own {
        return true;
    }
    if column + 1 < width && light[cell + 1] < own {
        return true;
    }
    if column >= 1 && light[cell - 1] < own {
        return true;
    }
    if row + 1 < width && light[cell + width] < own {
        return true;
    }
    if row >= 1 && light[cell - width] < own {
        return true;
    }
    false
}

/// Breadth-first spread for one layer.
#[allow(clippy::too_many_arguments)]
fn spread(
    light: &mut [u8],
    queue: &mut VecDeque<(i32, i32, i32)>,
    width: i32,
    origin_x: i32,
    origin_z: i32,
    min_y: i32,
    top: i32,
    block_at: &impl Fn(i32, i32, i32) -> Option<i32>,
    table: &LightTable,
    is_sky: bool,
) {
    let index = |x: i32, y: i32, z: i32| -> usize {
        let lx = x - origin_x;
        let lz = z - origin_z;
        let ly = y - min_y;
        (lx + lz * width + ly * width * width) as usize
    };

    while let Some((x, y, z)) = queue.pop_front() {
        let level = light[index(x, y, z)];
        if level == 0 {
            continue;
        }
        for (dx, dy, dz) in [
            (1, 0, 0),
            (-1, 0, 0),
            (0, 1, 0),
            (0, -1, 0),
            (0, 0, 1),
            (0, 0, -1),
        ] {
            let (nx, ny, nz) = (x + dx, y + dy, z + dz);
            if nx < origin_x || nz < origin_z || ny < min_y || ny > top {
                continue;
            }
            if nx >= origin_x + width || nz >= origin_z + width {
                continue;
            }
            let state = block_at(nx, ny, nz).unwrap_or(0);
            // Sky light falls straight down at full strength while nothing blocks it. Everything else loses
            // at least one level, which is why a shadow never brightens as it spreads.
            let candidate = if is_sky
                && dy == -1
                && level == MAX_LIGHT
                && table.propagates_skylight_down(state)
            {
                MAX_LIGHT
            } else {
                level.saturating_sub(table.dampening(state).max(1))
            };
            let cell = index(nx, ny, nz);
            if candidate > light[cell] {
                light[cell] = candidate;
                queue.push_back((nx, ny, nz));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ChunkLight, LIGHT_ARRAY_BYTES, LightArray, LightTable, MAX_LIGHT, compute_chunk_light,
    };

    /// Three synthetic states, so the propagation rules are pinned rather than the jar's ids.
    ///
    /// 0 = air (transparent, no emission), 1 = stone (opaque), 2 = a torch (emits 14, transparent).
    fn table() -> LightTable {
        LightTable::parse(
            "# synthetic\nstate\t0\t0\t0\t1\nstate\t1\t0\t15\t0\nstate\t2\t14\t0\t1\n",
            3,
        )
        .expect("parses")
    }

    const AIR: i32 = 0;
    const STONE: i32 = 1;
    const TORCH: i32 = 2;

    /// A flat floor of `STONE` from the bottom of the world up to `floor_top`, air above.
    fn flat_world(
        min_y: i32,
        height: i32,
        floor_top: i32,
    ) -> impl Fn(i32, i32, i32) -> Option<i32> {
        move |_x, y, _z| {
            if y < min_y || y >= min_y + height {
                None
            } else if y <= floor_top {
                Some(STONE)
            } else {
                Some(AIR)
            }
        }
    }

    #[test]
    fn a_flat_floor_is_full_sky_above_and_dark_below() {
        let (min_y, height) = (-64, 48);
        // The floor fills section 0 exactly (`y <= -49`), so the two sections under test are unambiguous:
        // one entirely solid, one entirely open. An earlier version used a thinner floor and then asserted
        // the whole section was dark, which is not what a thinner floor means.
        let light = compute_chunk_light(&table(), 0, 0, min_y, 3, flat_world(min_y, height, -49))
            .expect("computes");

        // Section 0 spans y = -64..-49, all stone: sky light cannot pass through it, and in an infinite
        // floor there is nowhere for it to arrive from sideways.
        for x in 0..16 {
            for z in 0..16 {
                for y in 0..16 {
                    assert_eq!(
                        light.sky[0].get(x, y, z),
                        0,
                        "a cell below the surface must be dark, at {x},{y},{z}"
                    );
                }
            }
        }

        // Section 1 spans y = -48..-33, entirely above the floor: every cell is full sky light.
        for x in 0..16 {
            for z in 0..16 {
                for y in 0..16 {
                    assert_eq!(
                        light.sky[1].get(x, y, z),
                        MAX_LIGHT,
                        "a cell above the surface is fully sky-lit, at {x},{y},{z}"
                    );
                }
            }
        }

        // And the whole sky column is uniform, which is what puts it in an `empty_*` mask.
        assert_eq!(light.sky[0].uniform(), Some(0));
        assert_eq!(light.sky[1].uniform(), Some(MAX_LIGHT));
        // Nothing emits, so block light is uniformly dark.
        assert_eq!(light.block[0].uniform(), Some(0));
        assert_eq!(light.block[1].uniform(), Some(0));
    }

    #[test]
    fn light_decays_by_one_per_block_sideways_under_a_roof() {
        let (min_y, height) = (-64, 48);
        // A floor at -64..-60, and a roof at y = -50 covering every x <= 8. Columns x >= 9 are open to the
        // sky all the way down, so they are lit from above at 15 and the cells under the roof can only be
        // reached sideways from them. That is the arrangement that tests the decrement: a "spread without
        // losing a level" bug leaves every one of these cells at 15 and looks perfectly like light.
        //
        // The roof is expressed in **world** x, not local. It has to cover the one-block margin at x = -1 as
        // well, or the engine correctly finds a second open edge there and the cell under test is lit from
        // whichever side is nearer. The margin is deliberate engine behaviour (light crosses chunk borders),
        // so it is the test world that has to account for it.
        let world = move |x: i32, y: i32, _z: i32| -> Option<i32> {
            if y < min_y || y >= min_y + height {
                return None;
            }
            if y <= -60 {
                return Some(STONE);
            }
            if y == -50 && x <= 8 {
                return Some(STONE);
            }
            Some(AIR)
        };
        let light = compute_chunk_light(&table(), 0, 0, min_y, 3, world).expect("computes");

        // y = -55 is section 0 (y = -64..-49) at local y = 9, under the roof.
        let level_at = |x: i32| light.sky[0].get(x, 9, 8);
        assert_eq!(
            level_at(10),
            MAX_LIGHT,
            "a column open to the sky is fully lit"
        );
        assert_eq!(level_at(9), MAX_LIGHT, "and so is the last open one");
        for x in (0..=8).rev() {
            let expected = MAX_LIGHT - u8::try_from(9 - x).expect("fits");
            assert_eq!(
                level_at(x),
                expected,
                "each block further under the roof must lose exactly one level, at x={x}"
            );
        }
    }

    #[test]
    fn a_torch_lights_its_own_cell_and_decays_outward() {
        let (min_y, height) = (-64, 48);
        // Floor at -64..-60, air above, one torch standing on the floor at (4, -59, 4).
        let world = move |x: i32, y: i32, z: i32| -> Option<i32> {
            if y < min_y || y >= min_y + height {
                return None;
            }
            if y <= -60 {
                return Some(STONE);
            }
            if y == -59 && x.rem_euclid(16) == 4 && z.rem_euclid(16) == 4 {
                return Some(TORCH);
            }
            Some(AIR)
        };
        let light = compute_chunk_light(&table(), 0, 0, min_y, 3, world).expect("computes");

        // The torch is at y = -59, which is section 0 (y = -64..-49) at local y = 5.
        assert_eq!(
            light.block[0].get(4, 5, 4),
            14,
            "the torch emits its own level"
        );
        // Air damps by 0, but the rule is `max(1, dampening)`, so light still loses one per block.
        assert_eq!(
            light.block[0].get(5, 5, 4),
            13,
            "one block away loses exactly one level"
        );
        assert_eq!(light.block[0].get(6, 5, 4), 12, "two blocks away loses two");
        assert_eq!(
            light.block[0].get(4, 6, 4),
            13,
            "and it spreads upward through air too"
        );
        // Downward it meets the floor, whose dampening is 15: `max(1, 15)` absorbs the whole level, which is
        // what makes an opaque block opaque. An earlier version of this test expected 13 here, which would
        // have meant light passing through stone.
        assert_eq!(
            light.block[0].get(4, 4, 4),
            0,
            "a block with full dampening absorbs all light, however bright its neighbour"
        );
        // Far enough away it must have run out rather than wrapping around.
        assert_eq!(
            light.block[0].get(15, 5, 15),
            0,
            "light does not reach across the chunk from a torch"
        );

        // The torch's own cell is above the floor, so it also has full sky light; the floor below it does not.
        assert_eq!(light.sky[0].get(4, 5, 4), MAX_LIGHT);
        assert_eq!(light.sky[0].get(4, 4, 4), 0);
    }

    #[test]
    fn sky_light_falls_at_full_strength_through_transparent_blocks_only() {
        let (min_y, height) = (-64, 48);
        // A glass-like layer is NOT modelled here; instead check the two extremes: air passes sky at 15 all
        // the way down, and stone stops it dead.
        let all_air = |_x: i32, y: i32, _z: i32| {
            if y < min_y || y >= min_y + height {
                None
            } else {
                Some(AIR)
            }
        };
        let light = compute_chunk_light(&table(), 0, 0, min_y, 3, all_air).expect("computes");
        for section in 0..3 {
            assert_eq!(
                light.sky[section].uniform(),
                Some(MAX_LIGHT),
                "an empty world is fully lit in every section"
            );
        }

        let all_stone = |_x: i32, y: i32, _z: i32| {
            if y < min_y || y >= min_y + height {
                None
            } else {
                Some(STONE)
            }
        };
        let light = compute_chunk_light(&table(), 0, 0, min_y, 3, all_stone).expect("computes");
        for section in 0..3 {
            assert_eq!(
                light.sky[section].uniform(),
                Some(0),
                "a solid world is dark in every section"
            );
        }
    }

    #[test]
    fn light_crosses_a_chunk_border() {
        let (min_y, height) = (-64, 48);
        // A torch just outside the chunk, at x = -1 — the margin cell. A chunk-local computation would miss
        // it entirely, which is the bug the one-block margin exists to prevent.
        let world = move |x: i32, y: i32, z: i32| -> Option<i32> {
            if y < min_y || y >= min_y + height {
                return None;
            }
            if y == -50 && x == -1 && z == 8 {
                return Some(TORCH);
            }
            Some(AIR)
        };
        let light = compute_chunk_light(&table(), 0, 0, min_y, 3, world).expect("computes");

        // Section 2 spans y = -32..-17; y = -50 is section 0 at local y = 14.
        let at_border = light.block[0].get(0, 14, 8);
        assert!(
            at_border > 0,
            "light from just outside the chunk must reach inside it, found {at_border}"
        );
        assert!(at_border < 14, "and it must have decayed on the way in");
    }

    #[test]
    fn an_unloaded_neighbour_is_treated_as_air() {
        let (min_y, height) = (-64, 48);
        // Nothing outside this chunk is loaded, and inside it is all air.
        let world = move |x: i32, y: i32, z: i32| -> Option<i32> {
            if y < min_y || y >= min_y + height {
                return None;
            }
            if (0..16).contains(&x) && (0..16).contains(&z) {
                Some(AIR)
            } else {
                None
            }
        };
        let light = compute_chunk_light(&table(), 0, 0, min_y, 3, world).expect("computes");
        assert_eq!(
            light.sky[2].uniform(),
            Some(MAX_LIGHT),
            "an unloaded neighbour is sky-visible, so the frontier is not dark"
        );
    }

    #[test]
    fn light_arrays_pack_nibbles_the_way_the_wire_carries_them() {
        let mut array = LightArray::default();
        assert_eq!(array.as_bytes().len(), LIGHT_ARRAY_BYTES);
        assert_eq!(array.uniform(), Some(0));

        // Even cells go in the low nibble, odd cells in the high nibble, and the flat index is
        // `x + z*16 + y*256`.
        array.set(0, 0, 0, 0xF);
        array.set(1, 0, 0, 0x3);
        assert_eq!(array.as_bytes()[0], 0x3F, "x=0 low, x=1 high");
        assert_eq!(array.get(0, 0, 0), 0xF);
        assert_eq!(array.get(1, 0, 0), 0x3);

        array.set(15, 0, 0, 0xA);
        assert_eq!(array.get(15, 0, 0), 0xA);
        assert_ne!(
            array.uniform(),
            Some(0),
            "a non-uniform array is not uniform"
        );

        // Round trip through every cell, which also pins the index order.
        let mut full = LightArray::default();
        for x in 0..16 {
            for z in 0..16 {
                for y in 0..16 {
                    full.set(x, y, z, ((x + y + z) % 16) as u8);
                }
            }
        }
        for x in 0..16 {
            for z in 0..16 {
                for y in 0..16 {
                    assert_eq!(full.get(x, y, z), ((x + y + z) % 16) as u8);
                }
            }
        }
    }

    #[test]
    fn a_uniform_array_is_recognised_in_both_nibbles() {
        // `uniform` has to check both nibbles of every byte; checking only the low one would call a
        // half-filled array uniform and put it in an `empty_*` mask, losing half the light silently.
        let mut array = LightArray::default();
        assert_eq!(array.uniform(), Some(0));
        array.set(1, 0, 0, 7); // high nibble of byte 0 only
        assert_eq!(
            array.uniform(),
            None,
            "a value in the high nibble is not uniform"
        );
    }

    #[test]
    fn a_chunk_with_no_sections_is_refused() {
        assert!(compute_chunk_light(&table(), 0, 0, 0, 0, |_, _, _| Some(AIR)).is_err());
    }

    #[test]
    fn chunk_light_defaults_are_dark_and_rectangular() {
        let light = ChunkLight::default();
        assert!(light.sky.is_empty());
        assert!(light.block.is_empty());
    }
}
