//! Axis-aligned boxes, movement vectors and collision resolution (P04-07).
//!
//! ## Scope, stated plainly
//!
//! Solid means "full cube, blocks movement". A block is solid unless it is
//! air-like per [`mc_registry::BlockRegistry::is_empty`] or appears in
//! [`NON_SOLID`]. That covers walking, jumping and not falling through the world,
//! which is what Phase 04 needs. It does **not** model:
//!
//! - partial shapes (slabs, stairs, fences, walls, panes, doors, trapdoors, beds)
//!   — these collide as full cubes, so a player cannot walk onto a slab;
//! - fluid physics (water/lava are non-solid here, so a player falls through);
//! - step-up assistance, so a 0.5-block lip must be jumped;
//! - the true player hitbox (Vanilla: 0.6 × 1.8 × 0.6, eye height 1.62), which is
//!   supplied by the caller as an [`Aabb`];
//! - climbing (ladders/vines), which are non-solid here.
//!
//! Every one of those is recorded in `docs/vanilla-parity/PARITY-MATRIX.md`.

use mc_registry::BlockRegistry;

/// A 3-component double vector (position, size or delta).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Vec3 {
    /// X component.
    pub x: f64,
    /// Y component.
    pub y: f64,
    /// Z component.
    pub z: f64,
}

impl Vec3 {
    /// Construct.
    #[must_use]
    pub const fn new(x: f64, y: f64, z: f64) -> Self {
        Self { x, y, z }
    }

    /// Component-wise sum.
    #[must_use]
    pub fn plus(self, other: Self) -> Self {
        Self::new(self.x + other.x, self.y + other.y, self.z + other.z)
    }

    /// Component-wise difference.
    #[must_use]
    pub fn minus(self, other: Self) -> Self {
        Self::new(self.x - other.x, self.y - other.y, self.z - other.z)
    }

    /// Horizontal distance to another point (ignores y).
    #[must_use]
    pub fn horizontal_distance(self, other: Self) -> f64 {
        let dx = self.x - other.x;
        let dz = self.z - other.z;
        (dx * dx + dz * dz).sqrt()
    }

    /// Whether every component is finite.
    ///
    /// The collision solver refuses non-finite input outright (Audit 02): a NaN
    /// position makes every later comparison false, which would leave an entity
    /// permanently outside the world and immune to every collision test.
    #[must_use]
    pub fn is_finite(self) -> bool {
        self.x.is_finite() && self.y.is_finite() && self.z.is_finite()
    }
}

/// An axis-aligned bounding box.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Aabb {
    /// Minimum x.
    pub min_x: f64,
    /// Minimum y.
    pub min_y: f64,
    /// Minimum z.
    pub min_z: f64,
    /// Maximum x.
    pub max_x: f64,
    /// Maximum y.
    pub max_y: f64,
    /// Maximum z.
    pub max_z: f64,
}

impl Aabb {
    /// Player hitbox standing at `position` (feet centre): 0.6 × 1.8 × 0.6.
    #[must_use]
    pub fn player(position: Vec3) -> Self {
        Self::sized(position, 0.6, 1.8)
    }

    /// A box of the given width and height centred on `position` horizontally,
    /// with `position.y` as the feet.
    #[must_use]
    pub fn sized(position: Vec3, width: f64, height: f64) -> Self {
        let half = width / 2.0;
        Self {
            min_x: position.x - half,
            min_y: position.y,
            min_z: position.z - half,
            max_x: position.x + half,
            max_y: position.y + height,
            max_z: position.z + half,
        }
    }

    /// A single block's box.
    #[must_use]
    pub fn block(x: i32, y: i32, z: i32) -> Self {
        Self {
            min_x: f64::from(x),
            min_y: f64::from(y),
            min_z: f64::from(z),
            max_x: f64::from(x) + 1.0,
            max_y: f64::from(y) + 1.0,
            max_z: f64::from(z) + 1.0,
        }
    }

    /// Translate.
    #[must_use]
    pub fn offset(self, delta: Vec3) -> Self {
        Self {
            min_x: self.min_x + delta.x,
            min_y: self.min_y + delta.y,
            min_z: self.min_z + delta.z,
            max_x: self.max_x + delta.x,
            max_y: self.max_y + delta.y,
            max_z: self.max_z + delta.z,
        }
    }

    /// Grow by `amount` on every side (a small epsilon widens a swept box enough
    /// that touching-but-not-overlapping blocks are still tested).
    #[must_use]
    pub fn expand(self, amount: f64) -> Self {
        Self {
            min_x: self.min_x - amount,
            min_y: self.min_y - amount,
            min_z: self.min_z - amount,
            max_x: self.max_x + amount,
            max_y: self.max_y + amount,
            max_z: self.max_z + amount,
        }
    }

    /// Whether every bound is finite.
    #[must_use]
    pub fn is_finite(self) -> bool {
        self.min_x.is_finite()
            && self.min_y.is_finite()
            && self.min_z.is_finite()
            && self.max_x.is_finite()
            && self.max_y.is_finite()
            && self.max_z.is_finite()
    }

    /// Whether two boxes overlap on all three axes.
    ///
    /// Touching faces do **not** count as overlapping (strict inequality), which
    /// is what keeps a player standing exactly on a floor from being pushed out.
    #[must_use]
    pub fn intersects(self, other: Self) -> bool {
        self.min_x < other.max_x
            && self.max_x > other.min_x
            && self.min_y < other.max_y
            && self.max_y > other.min_y
            && self.min_z < other.max_z
            && self.max_z > other.min_z
    }

    /// The integer block range this box covers, as inclusive min / exclusive max.
    #[must_use]
    pub fn block_range(self) -> ((i32, i32, i32), (i32, i32, i32)) {
        (
            (
                self.min_x.floor() as i32,
                self.min_y.floor() as i32,
                self.min_z.floor() as i32,
            ),
            (
                self.max_x.ceil() as i32,
                self.max_y.ceil() as i32,
                self.max_z.ceil() as i32,
            ),
        )
    }
}

/// Blocks that never block movement, by registry name.
///
/// Deliberately explicit and small: everything not listed is a full cube. A test
/// asserts every name here exists in the loaded registry, so the list cannot rot
/// into silently-dead entries when the fixture is regenerated.
pub const NON_SOLID: &[&str] = &[
    "minecraft:air",
    "minecraft:cave_air",
    "minecraft:void_air",
    "minecraft:water",
    "minecraft:lava",
    "minecraft:short_grass",
    "minecraft:tall_grass",
    "minecraft:fern",
    "minecraft:large_fern",
    "minecraft:dead_bush",
    "minecraft:dandelion",
    "minecraft:poppy",
    "minecraft:torch",
    "minecraft:wall_torch",
    "minecraft:soul_torch",
    "minecraft:redstone_torch",
    "minecraft:redstone_wire",
    "minecraft:rail",
    "minecraft:powered_rail",
    "minecraft:detector_rail",
    "minecraft:activator_rail",
    "minecraft:ladder",
    "minecraft:vine",
    "minecraft:oak_sapling",
    "minecraft:spruce_sapling",
    "minecraft:birch_sapling",
    "minecraft:jungle_sapling",
    "minecraft:acacia_sapling",
    "minecraft:dark_oak_sapling",
    "minecraft:sugar_cane",
    "minecraft:wheat",
    "minecraft:carrots",
    "minecraft:potatoes",
    "minecraft:beetroots",
    "minecraft:snow",
    "minecraft:lever",
    "minecraft:stone_pressure_plate",
    "minecraft:oak_pressure_plate",
    "minecraft:tripwire",
    "minecraft:tripwire_hook",
    "minecraft:lily_pad",
    "minecraft:kelp",
    "minecraft:seagrass",
    "minecraft:scaffolding",
    "minecraft:cobweb",
    "minecraft:soul_fire",
    "minecraft:fire",
];

/// Whether a block-state id blocks movement.
///
/// # Errors
///
/// [`mc_core::error::ServerError::CorruptData`] when the id is outside the
/// registry — never silently treated as air, because that would let a player walk
/// through an unknown block.
pub fn is_solid(registry: &BlockRegistry, id: i32) -> mc_core::error::ServerResult<bool> {
    if registry.is_empty(id) {
        return Ok(false);
    }
    let name = registry.block_name(id)?;
    Ok(!NON_SOLID.contains(&name))
}

/// Whether a block-state id blocks movement, treating an unknown id as solid.
///
/// Convenience for hot movement code that already validated the id when the chunk
/// was loaded; keeping unknown ids solid fails safe.
#[must_use]
pub fn is_solid_or_unknown(registry: &BlockRegistry, id: i32) -> bool {
    is_solid(registry, id).unwrap_or(true)
}

/// Blocks that are fluids, by registry name.
///
/// Short because 26.x has no separate `flowing_water`/`flowing_lava` block: a
/// fluid's level is a **block state** of the one fluid block, exactly as the
/// registry row shows (`minecraft:water` with `level=0..15`).
pub const LIQUIDS: &[&str] = &["minecraft:water", "minecraft:lava"];

/// Whether a block-state id is a fluid.
///
/// Both fluids are also in [`NON_SOLID`] — you can walk *into* water — so
/// solidity alone cannot answer "may a mob walk here?". A land mob that walks
/// into an ocean and keeps walking is the visible symptom this separates out
/// (M-4: the AI's one-cell passability lookahead uses it).
///
/// An unknown id is **not** a fluid: [`is_solid_or_unknown`] already refuses
/// those, and reporting them as liquid as well would tell a caller two different
/// stories about the same block.
#[must_use]
pub fn is_liquid(registry: &BlockRegistry, id: i32) -> bool {
    // An id outside the registry fails this lookup and is reported as "not a
    // fluid" — the caller's solidity check is what refuses it.
    registry
        .block_name(id)
        .is_ok_and(|name| LIQUIDS.contains(&name))
}

/// Result of a ray cast against blocks.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BlockHit {
    /// Block coordinates hit.
    pub x: i32,
    /// Block y.
    pub y: i32,
    /// Block z.
    pub z: i32,
    /// Face entered, Vanilla ids: 0 = -Y, 1 = +Y, 2 = -Z, 3 = +Z, 4 = -X, 5 = +X.
    pub face: u8,
    /// Distance along the ray.
    pub distance: f64,
}

/// Vanilla face id for the -Y face.
pub const FACE_DOWN: u8 = 0;
/// Vanilla face id for the +Y face.
pub const FACE_UP: u8 = 1;
/// Vanilla face id for the -Z face.
pub const FACE_NORTH: u8 = 2;
/// Vanilla face id for the +Z face.
pub const FACE_SOUTH: u8 = 3;
/// Vanilla face id for the -X face.
pub const FACE_WEST: u8 = 4;
/// Vanilla face id for the +X face.
pub const FACE_EAST: u8 = 5;

/// A look direction's unit vector from Vanilla yaw/pitch degrees.
///
/// Vanilla convention: yaw 0 looks toward +Z, increasing yaw turns clockwise
/// (toward -X); pitch is positive downward.
#[must_use]
pub fn look_vector(yaw: f32, pitch: f32) -> Vec3 {
    let yaw = f64::from(yaw).to_radians();
    let pitch = f64::from(pitch).to_radians();
    let cos_pitch = pitch.cos();
    Vec3::new(-yaw.sin() * cos_pitch, -pitch.sin(), yaw.cos() * cos_pitch)
}

#[cfg(test)]
mod tests {
    use super::{
        Aabb, FACE_DOWN, FACE_UP, LIQUIDS, NON_SOLID, Vec3, is_liquid, is_solid,
        is_solid_or_unknown, look_vector,
    };
    use mc_registry::Registries;

    fn registry() -> Registries {
        Registries::vanilla().expect("registry loads")
    }

    #[test]
    fn every_non_solid_name_exists_in_the_registry() {
        // Guards the list against silent rot when the fixture is regenerated.
        let registries = registry();
        for name in NON_SOLID {
            assert!(
                registries.blocks.contains(name),
                "NON_SOLID lists {name:?}, which is not in the block registry"
            );
        }
    }

    /// The same guard for [`LIQUIDS`], plus the two facts a caller relies on:
    /// a fluid is non-solid *and* liquid, and an unknown id is neither reported as
    /// liquid nor allowed to move through.
    #[test]
    fn liquids_are_non_solid_and_unknown_ids_are_not_liquid() {
        let registries = registry();
        assert!(!LIQUIDS.is_empty(), "the list must not be trivially empty");
        for name in LIQUIDS {
            assert!(
                registries.blocks.contains(name),
                "LIQUIDS lists {name:?}, which is not in the block registry"
            );
            let id = registries.blocks.default_state(name).expect(name);
            assert!(is_liquid(&registries.blocks, id), "{name} is a fluid");
            assert!(
                !is_solid(&registries.blocks, id).expect("solidity"),
                "{name} is a fluid, so it must not be solid: you can walk into it"
            );
        }
        // A solid block is not a fluid, and neither is air.
        let stone = registries
            .blocks
            .default_state("minecraft:stone")
            .expect("stone");
        assert!(!is_liquid(&registries.blocks, stone));
        assert!(!is_liquid(&registries.blocks, registries.blocks.air_id()));
        // An id outside the registry is not called liquid; `is_solid_or_unknown`
        // is the function that refuses it.
        let unknown = registries.blocks.state_count() as i32;
        assert!(
            registries.blocks.block_name(unknown).is_err(),
            "the probe id must be outside the registry"
        );
        assert!(!is_liquid(&registries.blocks, unknown));
        assert!(is_solid_or_unknown(&registries.blocks, unknown));
    }

    #[test]
    fn solidity_of_representative_blocks() {
        let registries = registry();
        let solid = |name: &str| {
            let id = registries.blocks.default_state(name).expect(name);
            is_solid(&registries.blocks, id).expect("solidity")
        };
        assert!(!solid("minecraft:air"));
        assert!(!solid("minecraft:water"));
        assert!(!solid("minecraft:torch"));
        assert!(solid("minecraft:stone"));
        assert!(solid("minecraft:dirt"));
        assert!(solid("minecraft:oak_log"));
        // Not modelled as partial: a slab collides as a full cube in P04.
        assert!(solid("minecraft:stone_slab"));
        assert!(
            is_solid_or_unknown(&registries.blocks, 999_999),
            "unknown fails safe"
        );
    }

    #[test]
    fn box_intersection_treats_touching_faces_as_clear() {
        let floor = Aabb::block(0, 0, 0);
        // Standing exactly on the floor: min_y == max_y of the block.
        let standing = Aabb::sized(Vec3::new(0.5, 1.0, 0.5), 0.6, 1.8);
        assert!(!standing.intersects(floor), "resting is not intersecting");
        // Sunk one millimetre into it: intersecting.
        let sunk = Aabb::sized(Vec3::new(0.5, 0.999, 0.5), 0.6, 1.8);
        assert!(sunk.intersects(floor));
    }

    #[test]
    fn player_box_matches_vanilla_dimensions() {
        let player = Aabb::player(Vec3::new(10.0, 64.0, -3.0));
        assert!((player.max_x - player.min_x - 0.6).abs() < 1e-9);
        assert!((player.max_y - player.min_y - 1.8).abs() < 1e-9);
        assert!((player.max_z - player.min_z - 0.6).abs() < 1e-9);
        assert!((player.min_y - 64.0).abs() < 1e-9, "feet at y");
    }

    #[test]
    fn block_range_covers_partial_overlap() {
        let player = Aabb::sized(Vec3::new(0.5, 0.0, 0.5), 0.6, 1.8);
        let (min, max) = player.block_range();
        assert_eq!(min, (0, 0, 0));
        assert_eq!(max, (1, 2, 1));
    }

    #[test]
    fn look_vector_matches_vanilla_conventions() {
        // Yaw 0 looks toward +Z.
        let forward = look_vector(0.0, 0.0);
        assert!(forward.z > 0.99, "{forward:?}");
        assert!(forward.x.abs() < 1e-9);
        // Yaw 90 looks toward -X.
        let left = look_vector(90.0, 0.0);
        assert!(left.x < -0.99, "{left:?}");
        // Yaw 180 looks toward -Z.
        let back = look_vector(180.0, 0.0);
        assert!(back.z < -0.99, "{back:?}");
        // Pitch 90 looks straight down.
        let down = look_vector(0.0, 90.0);
        assert!(down.y < -0.99, "{down:?}");
        // Pitch -90 looks straight up.
        let up = look_vector(0.0, -90.0);
        assert!(up.y > 0.99, "{up:?}");
    }

    #[test]
    fn face_ids_match_vanilla() {
        // The constants exist so callers can name faces the way the wire does.
        assert_eq!((FACE_DOWN, FACE_UP), (0, 1));
        assert_eq!(super::FACE_NORTH, 2);
        assert_eq!(super::FACE_SOUTH, 3);
        assert_eq!(super::FACE_WEST, 4);
        assert_eq!(super::FACE_EAST, 5);
    }
}
