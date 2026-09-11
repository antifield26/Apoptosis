//! Voxel ray casting for block targeting (P04-09/P04-11).
//!
//! Amanatides–Woo grid traversal: repeatedly step to the nearest voxel boundary on
//! whichever axis is closest, recording which face was crossed. Implemented from
//! the algorithm's public description, not transcribed from any server.
//!
//! Exactness matters: block break/place validation uses this to decide which block
//! a client is pointing at, so an off-by-one at a boundary is exactly the kind of
//! bug that lets a player reach a block they should not.

use crate::collision::{
    BlockHit, FACE_DOWN, FACE_EAST, FACE_NORTH, FACE_SOUTH, FACE_UP, FACE_WEST, Vec3,
};

/// Anything that can report the block id at a position.
///
/// Returning `None` means "not available" (an unloaded chunk); the ray then stops,
/// so a client cannot target blocks through unloaded terrain.
pub trait BlockSampler {
    /// Block id at a world position, or `None` when it is not loaded.
    fn block_at(&self, x: i32, y: i32, z: i32) -> Option<i32>;
}

/// A closed region of blocks, used by tests and by single-chunk tooling.
#[derive(Debug, Clone)]
pub struct RegionSampler {
    min: (i32, i32, i32),
    size: (i32, i32, i32),
    blocks: Vec<i32>,
}

impl RegionSampler {
    /// Build from an explicit block list in `x + z*size_x + y*size_x*size_z` order.
    ///
    /// # Panics
    ///
    /// Panics when `blocks.len()` is not `size.x * size.y * size.z`. That is a
    /// caller bug in test/tooling setup, never player input.
    #[must_use]
    pub fn new(min: (i32, i32, i32), size: (i32, i32, i32), blocks: Vec<i32>) -> Self {
        assert_eq!(
            blocks.len(),
            (size.0 * size.1 * size.2) as usize,
            "RegionSampler block count"
        );
        Self { min, size, blocks }
    }

    /// A region filled with one id.
    #[must_use]
    pub fn filled(min: (i32, i32, i32), size: (i32, i32, i32), id: i32) -> Self {
        Self::new(min, size, vec![id; (size.0 * size.1 * size.2) as usize])
    }

    /// Set a block.
    ///
    /// # Panics
    ///
    /// Panics when the position is outside the region — test/tooling helper only.
    pub fn set(&mut self, x: i32, y: i32, z: i32, id: i32) {
        let index = self.index(x, y, z).expect("position inside region");
        self.blocks[index] = id;
    }

    fn index(&self, x: i32, y: i32, z: i32) -> Option<usize> {
        let lx = x - self.min.0;
        let ly = y - self.min.1;
        let lz = z - self.min.2;
        if lx < 0 || ly < 0 || lz < 0 || lx >= self.size.0 || ly >= self.size.1 || lz >= self.size.2
        {
            return None;
        }
        Some((lx + lz * self.size.0 + ly * self.size.0 * self.size.2) as usize)
    }
}

impl BlockSampler for RegionSampler {
    fn block_at(&self, x: i32, y: i32, z: i32) -> Option<i32> {
        self.index(x, y, z).map(|index| self.blocks[index])
    }
}

/// Optional predicate deciding which blocks the ray stops on.
pub type Selector<'a> = &'a mut dyn FnMut(i32) -> bool;

/// Walk from `origin` along `direction` (need not be normalised) up to
/// `max_distance`, returning the first block `select` accepts.
///
/// `direction` is normalised internally, so `max_distance` is in blocks.
/// A ray that starts inside a selected block hits it at distance 0 (Vanilla
/// targets the block a player is standing in), with face [`FACE_UP`] because no
/// face was actually crossed.
#[must_use]
pub fn ray_cast<S: BlockSampler + ?Sized>(
    sampler: &S,
    select: Selector<'_>,
    origin: Vec3,
    direction: Vec3,
    max_distance: f64,
) -> Option<BlockHit> {
    if !max_distance.is_finite() || max_distance <= 0.0 {
        return None;
    }
    let length =
        (direction.x * direction.x + direction.y * direction.y + direction.z * direction.z).sqrt();
    if !length.is_finite() || length <= f64::EPSILON {
        return None;
    }
    let dir = Vec3::new(
        direction.x / length,
        direction.y / length,
        direction.z / length,
    );

    let mut voxel = (
        origin.x.floor() as i32,
        origin.y.floor() as i32,
        origin.z.floor() as i32,
    );
    if let Some(id) = sampler.block_at(voxel.0, voxel.1, voxel.2)
        && select(id)
    {
        return Some(BlockHit {
            x: voxel.0,
            y: voxel.1,
            z: voxel.2,
            face: FACE_UP,
            distance: 0.0,
        });
    }

    let starts = [origin.x, origin.y, origin.z];
    let axes = [dir.x, dir.y, dir.z];
    let voxels = [voxel.0, voxel.1, voxel.2];
    let steps = [axis_step(dir.x), axis_step(dir.y), axis_step(dir.z)];
    let mut t_max = [f64::INFINITY; 3];
    let mut t_delta = [f64::INFINITY; 3];
    for axis in 0..3 {
        if axes[axis] == 0.0 {
            continue; // parallel to this axis: never leaves the current slab
        }
        let boundary = if steps[axis] > 0 {
            f64::from(voxels[axis] + 1)
        } else {
            f64::from(voxels[axis])
        };
        t_max[axis] = (boundary - starts[axis]) / axes[axis];
        t_delta[axis] = (1.0 / axes[axis]).abs();
    }

    // Entering a voxel while travelling in +axis means crossing that voxel's
    // negative-side face, and vice versa. Indices: 0 = X, 1 = Y, 2 = Z.
    //   X: -X face is WEST(4), +X face is EAST(5)
    //   Y: -Y face is DOWN(0), +Y face is UP(1)
    //   Z: -Z face is NORTH(2), +Z face is SOUTH(3)
    let faces = [
        [FACE_EAST, FACE_WEST],
        [FACE_UP, FACE_DOWN],
        [FACE_SOUTH, FACE_NORTH],
    ];

    loop {
        let step_axis = if t_max[0] <= t_max[1] && t_max[0] <= t_max[2] {
            0
        } else if t_max[1] <= t_max[2] {
            1
        } else {
            2
        };
        let distance = t_max[step_axis];
        if !distance.is_finite() || distance > max_distance {
            return None;
        }
        match step_axis {
            0 => voxel.0 += steps[0],
            1 => voxel.1 += steps[1],
            _ => voxel.2 += steps[2],
        }
        t_max[step_axis] += t_delta[step_axis];
        let face = faces[step_axis][usize::from(steps[step_axis] > 0)];
        if let Some(id) = sampler.block_at(voxel.0, voxel.1, voxel.2)
            && select(id)
        {
            return Some(BlockHit {
                x: voxel.0,
                y: voxel.1,
                z: voxel.2,
                face,
                distance,
            });
        }
    }
}

fn axis_step(value: f64) -> i32 {
    if value > 0.0 {
        1
    } else if value < 0.0 {
        -1
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::{RegionSampler, ray_cast};
    use crate::collision::{
        FACE_DOWN, FACE_EAST, FACE_NORTH, FACE_SOUTH, FACE_UP, FACE_WEST, Vec3, look_vector,
    };

    const AIR: i32 = 0;
    const STONE: i32 = 1;
    const TORCH: i32 = 50;

    fn any(_id: i32) -> bool {
        true
    }

    fn solid_only(id: i32) -> bool {
        id != AIR && id != TORCH
    }

    #[test]
    fn straight_down_hits_the_floor_top_face() {
        // Floor at y = 0, player standing at y = 3 looking straight down.
        let mut region = RegionSampler::filled((0, 0, 0), (1, 4, 1), AIR);
        region.set(0, 0, 0, STONE);
        let hit = ray_cast(
            &region,
            &mut solid_only,
            Vec3::new(0.5, 3.0, 0.5),
            Vec3::new(0.0, -1.0, 0.0),
            5.0,
        )
        .expect("hits the floor");
        assert_eq!((hit.x, hit.y, hit.z), (0, 0, 0));
        assert_eq!(hit.face, FACE_UP, "entered through the top face");
        assert!(
            (hit.distance - 2.0).abs() < 1e-9,
            "distance {}",
            hit.distance
        );
    }

    #[test]
    fn straight_up_hits_the_ceiling_bottom_face() {
        let mut region = RegionSampler::filled((0, 0, 0), (1, 5, 1), AIR);
        region.set(0, 4, 0, STONE);
        let hit = ray_cast(
            &region,
            &mut solid_only,
            Vec3::new(0.5, 0.0, 0.5),
            Vec3::new(0.0, 1.0, 0.0),
            10.0,
        )
        .expect("hits the ceiling");
        assert_eq!(hit.y, 4);
        assert_eq!(hit.face, FACE_DOWN);
    }

    #[test]
    fn cardinal_faces_are_reported_per_vanilla_ids() {
        // Target block at x = 3; approach from -X → enter its WEST (-X) face (4).
        let mut region = RegionSampler::filled((0, 0, 0), (6, 1, 6), AIR);
        region.set(3, 0, 3, STONE);

        let east = ray_cast(
            &region,
            &mut solid_only,
            Vec3::new(0.5, 0.5, 3.5),
            Vec3::new(1.0, 0.0, 0.0),
            10.0,
        )
        .expect("hits from the west");
        assert_eq!((east.x, east.z), (3, 3));
        assert_eq!(east.face, FACE_WEST);

        let west = ray_cast(
            &region,
            &mut solid_only,
            Vec3::new(5.5, 0.5, 3.5),
            Vec3::new(-1.0, 0.0, 0.0),
            10.0,
        )
        .expect("hits from the east");
        assert_eq!(west.face, FACE_EAST);

        let south = ray_cast(
            &region,
            &mut solid_only,
            Vec3::new(3.5, 0.5, 0.5),
            Vec3::new(0.0, 0.0, 1.0),
            10.0,
        )
        .expect("hits from the north");
        assert_eq!(south.face, FACE_NORTH);

        let north = ray_cast(
            &region,
            &mut solid_only,
            Vec3::new(3.5, 0.5, 5.5),
            Vec3::new(0.0, 0.0, -1.0),
            10.0,
        )
        .expect("hits from the south");
        assert_eq!(north.face, FACE_SOUTH);
    }

    #[test]
    fn a_ray_through_empty_space_misses() {
        let region = RegionSampler::filled((0, 0, 0), (4, 4, 4), AIR);
        assert!(
            ray_cast(
                &region,
                &mut solid_only,
                Vec3::new(0.5, 0.5, 0.5),
                Vec3::new(1.0, 0.0, 0.0),
                10.0
            )
            .is_none()
        );
    }

    #[test]
    fn max_distance_is_respected() {
        let mut region = RegionSampler::filled((0, 0, 0), (10, 1, 1), AIR);
        region.set(5, 0, 0, STONE);
        // Reachable at 5 blocks.
        assert!(
            ray_cast(
                &region,
                &mut solid_only,
                Vec3::new(0.5, 0.5, 0.5),
                Vec3::new(1.0, 0.0, 0.0),
                6.0
            )
            .is_some()
        );
        // Not reachable with a 4-block reach.
        assert!(
            ray_cast(
                &region,
                &mut solid_only,
                Vec3::new(0.5, 0.5, 0.5),
                Vec3::new(1.0, 0.0, 0.0),
                4.0
            )
            .is_none()
        );
    }

    #[test]
    fn the_first_block_along_the_ray_wins() {
        let mut region = RegionSampler::filled((0, 0, 0), (6, 1, 1), AIR);
        region.set(2, 0, 0, STONE);
        region.set(4, 0, 0, STONE);
        let hit = ray_cast(
            &region,
            &mut solid_only,
            Vec3::new(0.5, 0.5, 0.5),
            Vec3::new(1.0, 0.0, 0.0),
            10.0,
        )
        .expect("hits");
        assert_eq!(hit.x, 2, "the nearer block is the target");
    }

    #[test]
    fn starting_inside_a_block_targets_it_at_zero_distance() {
        let mut region = RegionSampler::filled((0, 0, 0), (2, 2, 2), AIR);
        region.set(0, 0, 0, STONE);
        let hit = ray_cast(
            &region,
            &mut solid_only,
            Vec3::new(0.5, 0.5, 0.5),
            Vec3::new(0.0, -1.0, 0.0),
            5.0,
        )
        .expect("targets the block it is inside");
        assert!(hit.distance.abs() < f64::EPSILON);
        assert_eq!((hit.x, hit.y, hit.z), (0, 0, 0));
    }

    #[test]
    fn unloaded_positions_stop_the_ray() {
        // A 1×1×1 region: the very first step leaves it, so the ray reports a miss
        // rather than pretending the world continues.
        let region = RegionSampler::filled((0, 0, 0), (1, 1, 1), AIR);
        assert!(
            ray_cast(
                &region,
                &mut solid_only,
                Vec3::new(0.5, 0.5, 0.5),
                Vec3::new(1.0, 0.0, 0.0),
                10.0
            )
            .is_none()
        );
    }

    #[test]
    fn degenerate_inputs_are_refused() {
        let region = RegionSampler::filled((0, 0, 0), (2, 2, 2), STONE);
        let origin = Vec3::new(0.5, 0.5, 0.5);
        // Zero direction.
        assert!(ray_cast(&region, &mut any, origin, Vec3::new(0.0, 0.0, 0.0), 5.0).is_none());
        // Non-finite direction.
        assert!(
            ray_cast(
                &region,
                &mut any,
                origin,
                Vec3::new(f64::NAN, 0.0, 0.0),
                5.0
            )
            .is_none()
        );
        // Non-positive or non-finite reach.
        assert!(ray_cast(&region, &mut any, origin, Vec3::new(1.0, 0.0, 0.0), 0.0).is_none());
        assert!(ray_cast(&region, &mut any, origin, Vec3::new(1.0, 0.0, 0.0), -1.0).is_none());
        assert!(
            ray_cast(
                &region,
                &mut any,
                origin,
                Vec3::new(1.0, 0.0, 0.0),
                f64::INFINITY
            )
            .is_none()
        );
    }

    #[test]
    fn unnormalised_directions_are_normalised() {
        let mut region = RegionSampler::filled((0, 0, 0), (6, 1, 1), AIR);
        region.set(3, 0, 0, STONE);
        let a = ray_cast(
            &region,
            &mut solid_only,
            Vec3::new(0.5, 0.5, 0.5),
            Vec3::new(1.0, 0.0, 0.0),
            10.0,
        )
        .expect("unit direction");
        let b = ray_cast(
            &region,
            &mut solid_only,
            Vec3::new(0.5, 0.5, 0.5),
            Vec3::new(100.0, 0.0, 0.0),
            10.0,
        )
        .expect("scaled direction");
        assert_eq!(a, b, "scale must not change the hit");
    }

    #[test]
    fn a_look_vector_from_vanilla_angles_finds_the_floor() {
        // Player at y=3 looking straight down must hit the y=0 floor.
        let mut region = RegionSampler::filled((0, 0, 0), (4, 6, 4), AIR);
        for x in 0..4 {
            for z in 0..4 {
                region.set(x, 0, z, STONE);
            }
        }
        let hit = ray_cast(
            &region,
            &mut solid_only,
            Vec3::new(1.5, 3.0, 1.5),
            look_vector(0.0, 90.0),
            6.0,
        )
        .expect("hits the floor");
        assert_eq!(hit.y, 0);
        assert_eq!(hit.face, FACE_UP);
    }

    #[test]
    fn a_selector_can_target_replaceable_blocks_only() {
        let mut region = RegionSampler::filled((0, 0, 0), (6, 1, 1), AIR);
        region.set(2, 0, 0, STONE);
        region.set(4, 0, 0, TORCH);
        // A selector that accepts anything hits the air block the ray starts in
        // (distance 0), which is the documented "standing in a block" behaviour.
        let accepts_everything = ray_cast(
            &region,
            &mut any,
            Vec3::new(0.5, 0.5, 0.5),
            Vec3::new(1.0, 0.0, 0.0),
            10.0,
        )
        .expect("hits");
        assert_eq!(accepts_everything.x, 0);
        assert!(accepts_everything.distance.abs() < f64::EPSILON);

        // A solid-only selector skips the air and hits the nearer real block.
        let solid = ray_cast(
            &region,
            &mut solid_only,
            Vec3::new(0.5, 0.5, 0.5),
            Vec3::new(1.0, 0.0, 0.0),
            10.0,
        )
        .expect("hits the stone");
        assert_eq!(solid.x, 2);
        // Selecting only the torch skips the stone.
        let torch_only = ray_cast(
            &region,
            &mut |id| id == TORCH,
            Vec3::new(0.5, 0.5, 0.5),
            Vec3::new(1.0, 0.0, 0.0),
            10.0,
        )
        .expect("hits the torch");
        assert_eq!(torch_only.x, 4);
    }
}
