//! Dimension container, block changes and collision-resolved movement (P04-02/07).
//!
//! [`World`] owns the loaded chunks of one dimension. Chunks are keyed in a
//! `BTreeMap` so iteration order is deterministic — saves and traces must be
//! reproducible (AGENTS.md §3.6).
//!
//! ## No world generation
//!
//! [`World::ensure_chunk`] creates an **all-air** chunk when one is missing. That
//! is a placeholder so a player has somewhere to stand; it is not terrain. The
//! method name says so, and P07 replaces the body with the generation pipeline.
//! Nothing in this crate claims generated terrain.

use crate::chunk::{Chunk, ChunkPos, SECTION_HEIGHT, SECTION_WIDTH};
use crate::collision::{Aabb, Vec3, is_solid_or_unknown};
use crate::light::ChunkLight;
use crate::ray::{BlockSampler, Selector, ray_cast};
use mc_core::error::{ServerError, ServerResult};
use mc_persistence::dimension::Dimension;
use mc_registry::BlockRegistry;
use std::collections::BTreeMap;

/// One block change, for broadcasting and for save bookkeeping.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockChange {
    /// Chunk containing the block.
    pub pos: ChunkPos,
    /// World block x.
    pub x: i32,
    /// World block y.
    pub y: i32,
    /// World block z.
    pub z: i32,
    /// Previous block-state id.
    pub old_id: i32,
    /// New block-state id.
    pub new_id: i32,
}

/// Every chunk whose light a block change at world `(x, z)` can alter.
///
/// Light is computed over a chunk **plus a one-block margin on all four sides**
/// (see [`World::compute_light`]), so a block belongs to the light of every chunk
/// whose margin contains it: the chunk it is in, the neighbour it is within one
/// block of, and — the case an earlier version of this rule missed — the
/// **diagonal** neighbour when the block is within one block of a corner on both
/// axes. A torch at local `(0, 0)` lights the corner of the chunk at
/// `(x - 1, z - 1)`, whose margin reads both of those columns.
///
/// AUDIT-09 B-05: both `World::invalidate_light_around` and the server's
/// `light_update` queue previously spelled this out as four independent `if`s over
/// the four edges, which cannot express the diagonal and so left one chunk's
/// cached light stale after a corner change — and left the client drawing it. The
/// rule lives here, once, and both callers iterate it.
///
/// The enumeration is over all nine offsets with each axis tested separately, so
/// completeness is a property of the shape rather than of remembering four more
/// `if`s: a new edge case cannot be forgotten because there is nothing to
/// remember.
#[must_use]
pub fn chunks_a_block_can_light(pos: ChunkPos, x: i32, z: i32) -> Vec<ChunkPos> {
    let local_x = x.rem_euclid(SECTION_WIDTH);
    let local_z = z.rem_euclid(SECTION_WIDTH);
    let last = SECTION_WIDTH - 1;
    let mut out = Vec::with_capacity(9);
    for dx in -1..=1 {
        for dz in -1..=1 {
            let within_x = match dx {
                -1 => local_x == 0,
                0 => true,
                _ => local_x == last,
            };
            let within_z = match dz {
                -1 => local_z == 0,
                0 => true,
                _ => local_z == last,
            };
            if within_x && within_z {
                out.push(ChunkPos::new(pos.x + dx, pos.z + dz));
            }
        }
    }
    out
}

/// A block accessor with a one-chunk memo; see [`World::block_cursor`].
///
/// Owned separately from `World` so the borrow it holds can be scoped: the light engine reads the world
/// through it and then the computed light is written, which cannot happen while the world is still borrowed.
#[derive(Debug, Clone, Copy)]
pub struct BlockCursor<'a> {
    world: &'a World,
    /// The chunk position the memo below was resolved for.
    last: Option<(i32, i32)>,
    chunk: Option<&'a Chunk>,
}

impl BlockCursor<'_> {
    /// The block-state id at a world position, or `None` when the chunk is not loaded.
    ///
    /// A missing chunk is cached too: an unloaded neighbour is asked about as often as a loaded one along an
    /// edge, and re-looking it up every cell would give back much of what this exists to save.
    pub fn get(&mut self, x: i32, y: i32, z: i32) -> Option<i32> {
        let key = (x >> 4, z >> 4);
        if self.last != Some(key) {
            self.chunk = self.world.chunks.get(&ChunkPos::new(key.0, key.1));
            self.last = Some(key);
        }
        self.chunk.map(|chunk| chunk.get_block(x, y, z))
    }
}

/// A dimension's loaded chunks.
#[derive(Debug)]
pub struct World {
    dimension: Dimension,
    registry: BlockRegistry,
    chunks: BTreeMap<ChunkPos, Chunk>,
    /// Computed light per chunk, dropped when the chunk or a neighbour it reads changes.
    ///
    /// `Chunk` would be the natural home, but it is built in persistence, worldgen and many tests, so a field
    /// there means touching every struct literal; `World` already owns the chunks and keys this by the same
    /// position.
    light: BTreeMap<ChunkPos, ChunkLight>,
    changes: Vec<BlockChange>,
    min_section_y: i8,
    section_count: usize,
    spawn: (i32, i32, i32),
}

impl World {
    /// Create an empty world for `dimension` with an overworld-shaped chunk range.
    #[must_use]
    pub fn new(dimension: Dimension, registry: BlockRegistry) -> Self {
        Self::with_height(
            dimension,
            registry,
            crate::OVERWORLD_MIN_SECTION_Y,
            crate::OVERWORLD_SECTION_COUNT,
        )
    }

    /// Create with an explicit section range (nether/end have different shapes,
    /// which Phase 04 does not need to model yet).
    #[must_use]
    pub fn with_height(
        dimension: Dimension,
        registry: BlockRegistry,
        min_section_y: i8,
        section_count: usize,
    ) -> Self {
        Self {
            dimension,
            registry,
            chunks: BTreeMap::new(),
            light: BTreeMap::new(),
            changes: Vec::new(),
            min_section_y,
            section_count,
            spawn: (0, 64, 0),
        }
    }

    /// Dimension this world holds.
    #[must_use]
    pub const fn dimension(&self) -> &Dimension {
        &self.dimension
    }

    /// Shared block registry.
    #[must_use]
    pub const fn registry(&self) -> &BlockRegistry {
        &self.registry
    }

    /// Number of loaded chunks.
    #[must_use]
    pub fn chunk_count(&self) -> usize {
        self.chunks.len()
    }

    /// Lowest section index of new chunks.
    #[must_use]
    pub const fn min_section_y(&self) -> i8 {
        self.min_section_y
    }

    /// Sections per new chunk.
    #[must_use]
    pub const fn section_count(&self) -> usize {
        self.section_count
    }

    /// The world spawn point.
    #[must_use]
    pub const fn spawn(&self) -> (i32, i32, i32) {
        self.spawn
    }

    /// Replace the spawn point.
    pub fn set_spawn(&mut self, x: i32, y: i32, z: i32) {
        self.spawn = (x, y, z);
    }

    /// Insert an already-loaded chunk.
    pub fn load_chunk(&mut self, chunk: Chunk) {
        self.chunks.insert(chunk.pos, chunk);
    }

    /// Get a loaded chunk.
    #[must_use]
    pub fn chunk(&self, pos: ChunkPos) -> Option<&Chunk> {
        self.chunks.get(&pos)
    }

    /// Get a loaded chunk mutably.
    pub fn chunk_mut(&mut self, pos: ChunkPos) -> Option<&mut Chunk> {
        self.chunks.get_mut(&pos)
    }

    /// Whether a chunk is loaded.
    #[must_use]
    pub fn is_loaded(&self, pos: ChunkPos) -> bool {
        self.chunks.contains_key(&pos)
    }

    /// Get a chunk, creating an all-air placeholder when absent.
    ///
    /// This is **not** world generation: see the module docs. The name is
    /// deliberately explicit so no caller mistakes the result for terrain.
    pub fn ensure_chunk(&mut self, pos: ChunkPos) -> &mut Chunk {
        let (min_section_y, section_count, registry) =
            (self.min_section_y, self.section_count, &self.registry);
        self.chunks
            .entry(pos)
            .or_insert_with(|| Chunk::air(pos, min_section_y, section_count, registry))
    }

    /// Remove a chunk from memory (it is not deleted from disk).
    pub fn unload_chunk(&mut self, pos: ChunkPos) -> Option<Chunk> {
        self.light.remove(&pos);
        self.chunks.remove(&pos)
    }

    /// Loaded chunk positions in deterministic order.
    pub fn chunk_positions(&self) -> impl Iterator<Item = ChunkPos> + '_ {
        self.chunks.keys().copied()
    }

    /// Chunks marked dirty since the last [`World::clear_dirty`].
    #[must_use]
    pub fn dirty_chunks(&self) -> Vec<ChunkPos> {
        self.chunks
            .values()
            .filter(|chunk| chunk.dirty)
            .map(|chunk| chunk.pos)
            .collect()
    }

    /// Clear every dirty flag (call after a successful save).
    pub fn clear_dirty(&mut self) {
        for chunk in self.chunks.values_mut() {
            chunk.mark_clean();
        }
    }

    /// A block accessor that remembers the chunk it last looked in.
    ///
    /// [`World::get_block_loaded`] builds a `ChunkPos` and walks a `BTreeMap` per call, and the light engine
    /// calls it **per cell** — about a quarter of a million times per chunk, since it seeds the sky and block
    /// layers in two passes. It walks a column at a time, so consecutive questions are almost always about the
    /// same chunk, and one remembered chunk removes essentially all of those lookups.
    #[must_use]
    pub fn block_cursor(&self) -> BlockCursor<'_> {
        BlockCursor {
            world: self,
            last: None,
            chunk: None,
        }
    }

    /// Block id at a position; air when the chunk is not loaded or `y` is outside
    /// the world.
    #[must_use]
    pub fn get_block(&self, x: i32, y: i32, z: i32) -> i32 {
        let pos = ChunkPos::new(x >> 4, z >> 4);
        self.chunks
            .get(&pos)
            .map_or(0, |chunk| chunk.get_block(x, y, z))
    }

    /// Block id at a position, or `None` when the chunk is not loaded.
    #[must_use]
    pub fn get_block_loaded(&self, x: i32, y: i32, z: i32) -> Option<i32> {
        let pos = ChunkPos::new(x >> 4, z >> 4);
        self.chunks.get(&pos).map(|chunk| chunk.get_block(x, y, z))
    }

    /// Set a block, creating the chunk if necessary, and record the change.
    ///
    /// Returns the change when the block actually changed (`None` for a no-op or
    /// when the chunk did not exist and was created — a fresh all-air chunk has
    /// nothing to report for a write of air).
    ///
    /// # Errors
    ///
    /// [`ServerError::InvalidAction`] when `y` is outside the world's section
    /// range.
    pub fn set_block(
        &mut self,
        x: i32,
        y: i32,
        z: i32,
        id: i32,
    ) -> ServerResult<Option<BlockChange>> {
        let pos = ChunkPos::new(x >> 4, z >> 4);
        // The registry is immutable and cheap to clone (it is a name→layout map),
        // so clone it out of `self` rather than holding two borrows at once.
        let registry = self.registry.clone();
        let chunk = self.ensure_chunk(pos);
        let Some((old_id, new_id)) = chunk.set_block(x, y, z, id, &registry)? else {
            return Ok(None);
        };
        let change = BlockChange {
            pos,
            x,
            y,
            z,
            old_id,
            new_id,
        };
        self.changes.push(change);
        self.invalidate_light_around(pos, x, z);
        Ok(Some(change))
    }

    /// Drop cached light for the chunks a change at `(x, z)` can affect.
    ///
    /// The changed chunk always, plus every neighbour whose one-block margin reads
    /// across the border the block is within one block of. The rule lives in
    /// [`chunks_a_block_can_light`] so this and the server's `light_update` queue
    /// cannot disagree about it.
    ///
    /// This is **invalidation, not incremental relighting**: the whole chunk is dropped and recomputed when
    /// next needed, where vanilla relights only the region a change can reach. It is correct, and cheaper than
    /// what it replaces by the ratio of how often a chunk is sent to how often it changes — but a torch placed
    /// in a large lit chunk still costs a full recompute.
    fn invalidate_light_around(&mut self, pos: ChunkPos, x: i32, z: i32) {
        for affected in chunks_a_block_can_light(pos, x, z) {
            self.light.remove(&affected);
        }
    }

    /// Compute and cache a chunk's light, unless it is already cached.
    ///
    /// Reads the chunk plus a one-block margin, so light crosses chunk borders; an unloaded neighbour reads as
    /// air, the same assumption the client makes about ungenerated space. `table` is passed in because the
    /// light properties live in `mc_registry::Registries` and a `World` holds only a `BlockRegistry`.
    ///
    /// # Errors
    ///
    /// [`ServerError::Invariant`] when the chunk is not loaded, or whatever
    /// [`crate::light::compute_chunk_light`] reports.
    pub fn compute_light(
        &mut self,
        pos: ChunkPos,
        table: &mc_registry::LightTable,
    ) -> ServerResult<()> {
        if self.light.contains_key(&pos) {
            return Ok(());
        }
        let Some(chunk) = self.chunks.get(&pos) else {
            return Err(ServerError::Invariant(format!(
                "no chunk at ({}, {}) to light",
                pos.x, pos.z
            )));
        };
        // Scoped so the cursor's borrow of `self` ends before the cache is written.
        let light = {
            let mut cursor = self.block_cursor();
            crate::light::compute_chunk_light(
                table,
                pos.x,
                pos.z,
                chunk.min_y(),
                chunk.sections.len(),
                |x, y, z| cursor.get(x, y, z),
            )?
        };
        self.light.insert(pos, light);
        Ok(())
    }

    /// The cached light for a chunk, if it has been computed.
    #[must_use]
    pub fn cached_light(&self, pos: ChunkPos) -> Option<&ChunkLight> {
        self.light.get(&pos)
    }

    /// Drop every cached light, for a caller that has changed the world wholesale.
    pub fn clear_light(&mut self) {
        self.light.clear();
    }

    /// How many chunks have cached light, for tests and diagnostics.
    #[must_use]
    pub fn light_cache_len(&self) -> usize {
        self.light.len()
    }

    /// Block changes recorded since the last [`World::take_block_changes`].
    #[must_use]
    pub fn block_changes(&self) -> &[BlockChange] {
        &self.changes
    }

    /// Drain the recorded block changes (one tick's worth of broadcasts).
    pub fn take_block_changes(&mut self) -> Vec<BlockChange> {
        std::mem::take(&mut self.changes)
    }

    /// Whether a block blocks movement.
    #[must_use]
    pub fn is_solid(&self, x: i32, y: i32, z: i32) -> bool {
        is_solid_or_unknown(&self.registry, self.get_block(x, y, z))
    }

    /// Cast a ray against loaded blocks only.
    #[must_use]
    pub fn ray_cast(
        &self,
        origin: Vec3,
        direction: Vec3,
        max_distance: f64,
        select: Selector<'_>,
    ) -> Option<crate::collision::BlockHit> {
        ray_cast(self, select, origin, direction, max_distance)
    }

    /// Move `box` by `delta`, stopping at solid blocks, per axis.
    ///
    /// Axis order is Vanilla's, bytecode-read from the 26.1.2 jar
    /// (`Direction.axisStepOrder`, P14-03): Y first, then the longer
    /// horizontal axis (`|x| >= |z|` resolves X before Z, else Z before X).
    /// The order is observable only when a box is obstructed on two axes in
    /// one step — typically falling diagonally against a wall, where Y-first
    /// lands before the wall instead of flying over it.
    ///
    /// Returns the movement actually applied, whether the box is now resting on
    /// something (`on_ground`), and whether any axis was obstructed (so the caller
    /// can zero a velocity component).
    #[must_use]
    pub fn move_with_collision(&self, box_: Aabb, delta: Vec3) -> MoveResult {
        const EPSILON: f64 = 1e-7;

        // Defence in depth (Audit 02). A non-finite delta or box would poison the
        // clip arithmetic and could leave an entity at NaN, where every later
        // comparison is false: unkillable, invisible, and outside the world. The
        // game loop rejects such input already, but every mover goes through here,
        // so this function refuses it itself rather than trusting its callers.
        if !delta.is_finite() || !box_.is_finite() {
            return MoveResult {
                delta: Vec3::default(),
                on_ground: false,
                collided: [false; 3],
            };
        }

        let mut moved = Vec3::default();
        let mut collided = [false; 3];
        let mut on_ground = false;

        // Vanilla's `axisStepOrder`: Y, then the longer horizontal axis. Zero
        // axes are skipped inside the loop exactly as before.
        // Vanilla's `axisStepOrder`: Y, then the longer horizontal axis. Zero
        // axes are skipped inside the loop exactly as before.
        let order: [usize; 3] = if delta.x.abs() >= delta.z.abs() {
            [1, 0, 2]
        } else {
            [1, 2, 0]
        };
        for axis in order {
            let amount = match axis {
                0 => delta.x,
                1 => delta.y,
                _ => delta.z,
            };
            if amount == 0.0 {
                continue;
            }
            // `base` is where the box stands *before* this axis moves; the clip
            // searches backwards from the full step, so it needs the start box.
            let base = box_.offset(moved);
            let allowed = self.clip_axis(base, axis, amount, EPSILON);
            match allowed {
                Some(actual) => {
                    moved = moved.plus(axis_delta(axis, actual));
                    if (actual - amount).abs() > EPSILON {
                        collided[axis] = true;
                    }
                    if axis == 1 && amount < 0.0 && (actual - amount).abs() > EPSILON {
                        on_ground = true;
                    }
                }
                None => collided[axis] = true,
            }
        }

        MoveResult {
            delta: moved,
            on_ground,
            collided,
        }
    }

    /// Longest sub-movement along one axis that keeps the box clear of solids.
    ///
    /// `base` is the box *before* the step. `None` means "do not move on this
    /// axis": the box already overlaps geometry, so there is no clear sub-move.
    ///
    /// The test is a **swept** box (the union of the start and end positions), not
    /// just the endpoint. Testing only the endpoint would let a fast-moving player
    /// tunnel straight through a floor: a 10-block fall from y=70 to y=60 does not
    /// overlap a floor at y=63 at either end, but it passes through it.
    fn clip_axis(&self, base: Aabb, axis: usize, amount: f64, epsilon: f64) -> Option<f64> {
        // If the box already overlaps geometry, refuse to move on this axis rather
        // than teleporting the player out of it.
        if self.intersects_solid(base.expand(-epsilon)) {
            return None;
        }
        // Shortcut: the swept box is clear, so the whole step is allowed.
        if !self.intersects_solid(swept(base, axis_delta(axis, amount)).expand(-epsilon)) {
            return Some(amount);
        }
        // Binary search the largest fraction whose swept box stays clear. Sixteen
        // iterations resolve to well under 0.001 blocks, far finer than a client can
        // notice, and keep the cost bounded (an exact swept-shape solution that
        // stops precisely at the surface is P05 work).
        let (mut low, mut high) = (0.0f64, 1.0f64);
        for _ in 0..16 {
            let mid = f64::midpoint(low, high);
            let probe = swept(base, axis_delta(axis, amount * mid));
            if self.intersects_solid(probe.expand(-epsilon)) {
                high = mid;
            } else {
                low = mid;
            }
        }
        Some(amount * low)
    }

    fn intersects_solid(&self, box_: Aabb) -> bool {
        let (min, max) = box_.block_range();
        for x in min.0..max.0 {
            for y in min.1..max.1 {
                for z in min.2..max.2 {
                    if self.is_solid(x, y, z) && box_.intersects(Aabb::block(x, y, z)) {
                        return true;
                    }
                }
            }
        }
        false
    }

    /// Whether the position has solid ground directly below it.
    #[must_use]
    pub fn has_ground(&self, x: i32, y: i32, z: i32) -> bool {
        self.is_solid(x, y - 1, z)
    }

    /// Highest non-air block y in a column, or `None` when the column is empty or
    /// its chunk is unloaded.
    #[must_use]
    pub fn highest_block(&self, x: i32, z: i32) -> Option<i32> {
        let pos = ChunkPos::new(x >> 4, z >> 4);
        let chunk = self.chunks.get(&pos)?;
        (chunk.min_y()..chunk.max_y())
            .rev()
            .find(|y| !self.registry.is_empty(chunk.get_block(x, *y, z)))
    }

    /// Find a safe standing position near `(x, z)`, scanning down from `start_y`.
    ///
    /// Used for spawn placement and respawn. Returns the y of the first air column
    /// with solid ground beneath, plus one block of headroom; `None` when nothing
    /// suitable is loaded.
    #[must_use]
    pub fn find_surface(&self, x: i32, z: i32, start_y: i32) -> Option<i32> {
        let min_y = i32::from(self.min_section_y) * SECTION_HEIGHT;
        let mut y = start_y
            .min((i32::from(self.min_section_y) + self.section_count as i32) * SECTION_HEIGHT - 2);
        while y > min_y {
            if !self.is_solid(x, y, z) && !self.is_solid(x, y + 1, z) && self.is_solid(x, y - 1, z)
            {
                return Some(y);
            }
            y -= 1;
        }
        None
    }

    /// Chunk-space size helper for callers building packets.
    #[must_use]
    pub const fn blocks_per_section() -> i32 {
        SECTION_WIDTH * SECTION_HEIGHT * SECTION_WIDTH
    }
}

impl BlockSampler for World {
    fn block_at(&self, x: i32, y: i32, z: i32) -> Option<i32> {
        self.get_block_loaded(x, y, z)
    }
}

/// A step vector along one axis (`0` = x, `1` = y, `2` = z).
fn axis_delta(axis: usize, amount: f64) -> Vec3 {
    match axis {
        0 => Vec3::new(amount, 0.0, 0.0),
        1 => Vec3::new(0.0, amount, 0.0),
        _ => Vec3::new(0.0, 0.0, amount),
    }
}

/// The union of `base` and `base + delta`: everything the box passes through.
///
/// Collision must test this rather than the endpoint alone, otherwise a step larger
/// than one block tunnels through thin geometry.
fn swept(base: Aabb, delta: Vec3) -> Aabb {
    let moved = base.offset(delta);
    Aabb {
        min_x: base.min_x.min(moved.min_x),
        min_y: base.min_y.min(moved.min_y),
        min_z: base.min_z.min(moved.min_z),
        max_x: base.max_x.max(moved.max_x),
        max_y: base.max_y.max(moved.max_y),
        max_z: base.max_z.max(moved.max_z),
    }
}

/// Outcome of [`World::move_with_collision`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MoveResult {
    /// Movement actually applied.
    pub delta: Vec3,
    /// Whether the box came to rest on a solid block.
    pub on_ground: bool,
    /// Which axes were obstructed, in `[x, y, z]` order.
    pub collided: [bool; 3],
}

#[cfg(test)]
mod tests {
    use super::World;
    use crate::chunk::ChunkPos;
    use crate::collision::{Aabb, Vec3};
    use mc_persistence::dimension::Dimension;
    use mc_registry::Registries;

    fn world() -> World {
        let registries = Registries::vanilla().expect("registry");
        let mut world = World::new(Dimension::Overworld, registries.blocks);
        // A stone floor at y = 63 spanning several chunks.
        for x in -33..33 {
            for z in -33..33 {
                world.set_block(x, 63, z, 1).expect("stone");
            }
        }
        world
    }

    fn player_at(x: f64, y: f64, z: f64) -> Aabb {
        Aabb::player(Vec3::new(x, y, z))
    }

    #[test]
    fn falling_stops_on_the_floor() {
        let world = world();
        let box_ = player_at(0.5, 70.0, 0.5);
        let result = world.move_with_collision(box_, Vec3::new(0.0, -10.0, 0.0));
        // Floor top is y = 64, so a fall from 70 may drop exactly 6 blocks.
        assert!(
            (result.delta.y - -6.0).abs() < 0.01,
            "expected to land on 64, moved {}",
            result.delta.y
        );
        assert!(result.on_ground, "landing reports ground");
        assert!(result.collided[1]);
        // And standing on the floor, a second downward move goes nowhere.
        let resting = player_at(0.5, 64.0, 0.5);
        let again = world.move_with_collision(resting, Vec3::new(0.0, -0.5, 0.0));
        assert!(again.delta.y.abs() < 0.01, "no sinking: {}", again.delta.y);
        assert!(again.on_ground);
    }

    #[test]
    fn a_wall_stops_horizontal_movement() {
        let mut world = world();
        for y in 64..67 {
            world.set_block(3, y, 0, 1).expect("wall");
        }
        let box_ = player_at(1.0, 64.0, 0.5);
        let result = world.move_with_collision(box_, Vec3::new(5.0, 0.0, 0.0));
        assert!(result.collided[0], "the wall obstructed x");
        assert!(
            result.delta.x < 2.1 && result.delta.x > 1.5,
            "stopped just before the wall, moved {}",
            result.delta.x
        );
        // The player's new max_x must not be inside the wall.
        let after = box_.offset(result.delta);
        assert!(after.max_x <= 3.0 + 1e-6, "max_x {}", after.max_x);
    }

    #[test]
    fn non_finite_movement_is_refused_without_moving() {
        // Regression for Audit 02's defence-in-depth gap. A NaN delta would make
        // every clip comparison false, so the entity would be placed at NaN and
        // become permanently immune to collision, gravity and damage.
        let world = world();
        let box_ = player_at(1.0, 64.0, 0.5);
        for bad in [
            Vec3::new(f64::NAN, 0.0, 0.0),
            Vec3::new(0.0, f64::INFINITY, 0.0),
            Vec3::new(0.0, 0.0, f64::NEG_INFINITY),
            Vec3::new(f64::NAN, f64::NAN, f64::NAN),
        ] {
            let result = world.move_with_collision(box_, bad);
            assert_eq!(
                result.delta,
                Vec3::default(),
                "a non-finite delta {bad:?} must not move the box"
            );
            assert!(!result.on_ground);
            let after = box_.offset(result.delta);
            assert!(after.is_finite(), "the box must stay finite");
        }
        // A non-finite *box* is refused too, even with a legal delta.
        let mut broken = box_;
        broken.min_y = f64::NAN;
        assert!(!broken.is_finite());
        let result = world.move_with_collision(broken, Vec3::new(1.0, 0.0, 0.0));
        assert_eq!(result.delta, Vec3::default());
    }

    #[test]
    fn a_diagonal_move_slides_along_a_wall() {
        let mut world = world();
        for z in -5..5 {
            for y in 64..67 {
                world.set_block(3, y, z, 1).expect("wall");
            }
        }
        let box_ = player_at(1.0, 64.0, 0.5);
        let result = world.move_with_collision(box_, Vec3::new(2.0, 0.0, 2.0));
        assert!(result.collided[0], "x blocked by the wall");
        assert!(!result.collided[2], "z is free");
        assert!(
            (result.delta.z - 2.0).abs() < 1e-6,
            "slid the full z distance: {}",
            result.delta.z
        );
        assert!(result.delta.x < 2.0 && result.delta.x > 1.5);
    }

    #[test]
    fn falling_diagonally_lands_before_a_wall() {
        // P14-03, bytecode-measured (Direction.axisStepOrder): Y resolves
        // first. A box falling one block onto the floor while moving into a
        // wall lands, then stops against the wall — it does not fly over the
        // wall at height and land behind it (the old X-first order did).
        let mut world = world();
        world.set_block(1, 64, 0, 1).expect("wall");
        let box_ = player_at(0.0, 65.0, 0.0);
        let result = world.move_with_collision(box_, Vec3::new(2.0, -2.0, 0.0));
        assert!(
            (result.delta.y - -1.0).abs() < 0.01,
            "landed on the floor (top 64), moved {}",
            result.delta.y
        );
        assert!(result.on_ground, "landing reports ground");
        assert!(
            result.delta.x < 1.0,
            "stopped before the wall instead of flying over it, moved {}",
            result.delta.x
        );
        assert!(result.collided[0], "x blocked by the wall");
        let after = box_.offset(result.delta);
        assert!(
            after.max_x <= 1.0 + 1e-6,
            "must not be inside the wall, max_x {}",
            after.max_x
        );
    }

    #[test]
    fn the_longer_horizontal_axis_resolves_first() {
        // P14-03, bytecode-measured: Y, then the longer of X/Z (|x| >= |z|
        // takes X first). A short wall beside the path stops an X-first box
        // but not a Z-first one: with |dz| > |dx| the box slips past.
        let mut world = world();
        world.set_block(2, 64, 0, 1).expect("short wall");
        let box_ = player_at(0.0, 64.0, 0.0);
        let result = world.move_with_collision(box_, Vec3::new(3.0, 0.0, 4.0));
        assert!(
            (result.delta.z - 4.0).abs() < 1e-6,
            "z runs free: {}",
            result.delta.z
        );
        assert!(
            (result.delta.x - 3.0).abs() < 1e-6,
            "x slips past the short wall once z is clear: {}",
            result.delta.x
        );
        assert!(
            !result.collided[0] && !result.collided[2],
            "nothing was hit"
        );
    }

    #[test]
    fn a_one_block_gap_is_not_passable() {
        let mut world = world();
        // Floor at 63, ceiling at 65 → one air layer at 64, which is too short.
        for x in -5..5 {
            for z in -5..5 {
                world.set_block(x, 65, z, 1).expect("ceiling");
            }
        }
        let box_ = player_at(0.0, 64.0, 0.0);
        let result = world.move_with_collision(box_, Vec3::new(4.0, 0.0, 0.0));
        // The player is 1.8 tall, so it cannot be inside a 1-block gap at all:
        // the starting box already overlaps the ceiling.
        assert!(
            result.delta.x.abs() < 1e-6,
            "a 1-block gap must not be walkable, moved {}",
            result.delta.x
        );
    }

    #[test]
    fn jumping_onto_a_ledge_works_when_there_is_headroom() {
        let mut world = world();
        // A step up: block at y=64 covering x >= 2.
        for x in 2..8 {
            for z in -2..2 {
                world.set_block(x, 64, z, 1).expect("step");
            }
        }
        let box_ = player_at(0.5, 64.0, 0.5);
        // Rise 1.0 first (a jump), then move forward, then fall back down.
        let up = world.move_with_collision(box_, Vec3::new(0.0, 1.2, 0.0));
        assert!(
            (up.delta.y - 1.2).abs() < 1e-6,
            "free ascent: {}",
            up.delta.y
        );
        let forward = world.move_with_collision(box_.offset(up.delta), Vec3::new(2.0, 0.0, 0.0));
        assert!(
            forward.delta.x > 1.5,
            "moved over the ledge: {}",
            forward.delta.x
        );
        let down = world.move_with_collision(
            box_.offset(up.delta).offset(forward.delta),
            Vec3::new(0.0, -2.0, 0.0),
        );
        // Lands on the step top (y = 65) from y = 65.2.
        assert!(down.on_ground);
        assert!(down.delta.y.abs() < 0.3, "settled: {}", down.delta.y);
    }

    #[test]
    fn block_changes_are_recorded_and_drained() {
        let mut world = world();
        // The helper builds its floor through `set_block`, so start from a clear
        // slate to observe only this test's change.
        let _ = world.take_block_changes();
        assert!(world.block_changes().is_empty());
        let change = world
            .set_block(1, 64, 1, 1)
            .expect("sets")
            .expect("changed");
        assert_eq!(change.old_id, 0);
        assert_eq!(change.new_id, 1);
        assert_eq!(change.pos, ChunkPos::new(0, 0));
        assert_eq!(world.block_changes().len(), 1);
        // A no-op write reports nothing.
        assert!(world.set_block(1, 64, 1, 1).expect("sets").is_none());
        assert_eq!(world.take_block_changes().len(), 1);
        assert!(world.block_changes().is_empty());
        // The chunk is dirty.
        let touched = ChunkPos::new(0, 0);
        assert!(world.chunk(touched).expect("chunk").dirty);
        assert!(
            world.dirty_chunks().contains(&touched),
            "the chunk that changed must be in the dirty set"
        );
        // The helper's floor spans 36 chunks (x/z from -33..32), all dirty from
        // construction; `clear_dirty` must empty the whole set.
        world.clear_dirty();
        assert!(world.dirty_chunks().is_empty());
    }

    #[test]
    fn out_of_range_y_is_refused_without_panicking() {
        let mut world = world();
        for y in [-1000, -65, 320, 5000, i32::MAX, i32::MIN] {
            let result = world.set_block(0, y, 0, 1);
            assert!(result.is_err(), "y={y} must be refused");
        }
        // Reads outside the world are air, not an error.
        assert_eq!(world.get_block(0, 1000, 0), 0);
        assert_eq!(world.get_block(0, -1000, 0), 0);
    }

    #[test]
    fn unloaded_chunks_read_as_air_and_never_create() {
        let world = world();
        // Far from the loaded area: the world() helper never created chunk (100,100).
        assert_eq!(world.get_block(1600, 64, 1600), 0);
        assert_eq!(world.get_block_loaded(1600, 64, 1600), None);
        assert!(!world.is_loaded(ChunkPos::new(100, 100)));
    }

    #[test]
    fn ensure_chunk_creates_air_and_says_so() {
        let mut world = World::new(
            Dimension::Overworld,
            Registries::vanilla().expect("registry").blocks,
        );
        assert_eq!(world.chunk_count(), 0);
        let chunk = world.ensure_chunk(ChunkPos::new(4, -4));
        assert_eq!(chunk.section_count(), 24);
        assert_eq!(chunk.min_y(), -64);
        assert_eq!(chunk.max_y(), 320);
        assert_eq!(world.chunk_count(), 1);
        assert_eq!(world.get_block(4 * 16, 64, -4 * 16), 0, "all air");
        assert_eq!(world.highest_block(4 * 16, -4 * 16), None, "empty column");
    }

    #[test]
    fn surface_search_finds_standable_ground() {
        let mut world = world();
        world.set_block(5, 64, 5, 1).expect("block");
        // Highest non-air is 64; standable air is 65.
        assert_eq!(world.highest_block(5, 5), Some(64));
        let stand = world.find_surface(5, 5, 100).expect("finds a surface");
        assert_eq!(stand, 65, "one above the top solid block");
        assert!(!world.is_solid(5, stand, 5));
        assert!(world.is_solid(5, stand - 1, 5));
    }

    #[test]
    fn set_block_creates_the_chunk_implicitly() {
        let mut world = World::new(
            Dimension::Overworld,
            Registries::vanilla().expect("registry").blocks,
        );
        world.set_block(100, 70, 100, 1).expect("sets");
        assert!(world.is_loaded(ChunkPos::new(6, 6)));
        assert_eq!(world.get_block(100, 70, 100), 1);
    }
}
