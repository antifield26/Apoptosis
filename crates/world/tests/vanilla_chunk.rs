//! Integration tests against a **real** vanilla 26.1.2 chunk (P04-02/08).
//!
//! Uses the fixture region written by the official server
//! (`crates/test-support/fixtures/anvil/region_26_1_2.mca`, chunk (-37, -24)).
//! The point is to prove the disk → runtime → disk path against real data, not
//! against data this code produced.

// Binary-format test: section counts and heightmap indices are exact small
// integers, and the crate root documents the same cast exemption.
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss
)]

use mc_persistence::chunk::{ChunkData, ChunkPos};
use mc_persistence::compression::Compression;
use mc_persistence::region::RegionFile;
use mc_registry::Registries;
use mc_test_support::fixtures::{TempDir, read_fixture};
use mc_world::World;
use mc_world::chunk::Chunk;
use mc_world::collision::{Aabb, Vec3};

/// Chunk stored in the fixture region file.
const FIXTURE_CHUNK: ChunkPos = ChunkPos::new(-37, -24);

fn vanilla_chunk_data() -> ChunkData {
    let dir = TempDir::new("world-fixture");
    let path = dir.path().join("r.-2.-1.mca");
    std::fs::write(
        &path,
        read_fixture("anvil", "region_26_1_2.mca").expect("fixture"),
    )
    .expect("write fixture");
    let mut region = RegionFile::open_readonly(&path).expect("opens");
    let stored = region
        .read_chunk(FIXTURE_CHUNK)
        .expect("reads")
        .expect("present");
    assert_eq!(stored.compression, Compression::Zlib);
    ChunkData::from_nbt_bytes(&stored.data).expect("decodes the vanilla chunk")
}

#[test]
fn a_real_vanilla_chunk_loads_into_the_runtime_form() {
    let registries = Registries::vanilla().expect("registry");
    let data = vanilla_chunk_data();
    let chunk = Chunk::from_chunk_data(&data, &registries.blocks).expect("loads");

    assert_eq!(chunk.pos, FIXTURE_CHUNK);
    assert_eq!(chunk.section_count(), 24);
    assert_eq!(chunk.min_y(), -64);
    assert_eq!(chunk.max_y(), 320);
    assert_eq!(chunk.status, "minecraft:full");

    // A real overworld chunk has terrain: the block under the surface must be
    // solid, and the columns must not all be air.
    let mut solid_columns = 0;
    for x in 0..16 {
        for z in 0..16 {
            let world_x = FIXTURE_CHUNK.x * 16 + x;
            let world_z = FIXTURE_CHUNK.z * 16 + z;
            let mut found = false;
            for y in -64..320 {
                let id = chunk.get_block(world_x, y, world_z);
                if registries.blocks.is_empty(id) {
                    continue;
                }
                // Every non-air id must be a real registry entry.
                assert!(
                    registries.blocks.block_name(id).is_ok(),
                    "id {id} at ({world_x}, {y}, {world_z}) is not in the registry"
                );
                found = true;
            }
            if found {
                solid_columns += 1;
            }
        }
    }
    assert_eq!(
        solid_columns, 256,
        "every column of a full chunk has terrain"
    );

    // Bedrock sits at the bottom of a vanilla overworld chunk.
    let bottom = chunk.get_block(FIXTURE_CHUNK.x * 16, -64, FIXTURE_CHUNK.z * 16);
    assert_eq!(
        registries.blocks.block_name(bottom).expect("named"),
        "minecraft:bedrock"
    );

    // The recorded non-air counts must match a recount of the stored ids.
    for (index, section) in chunk.sections.iter().enumerate() {
        let recounted = section
            .blocks
            .iter()
            .filter(|id| !registries.blocks.is_empty(**id))
            .count();
        assert_eq!(
            i32::from(chunk.non_empty_block_count(index)),
            recounted as i32,
            "section y={} count drifted",
            section.y
        );
    }
}

#[test]
fn a_vanilla_chunk_round_trips_through_the_runtime_form() {
    let registries = Registries::vanilla().expect("registry");
    let data = vanilla_chunk_data();
    let chunk = Chunk::from_chunk_data(&data, &registries.blocks).expect("loads");
    let re_encoded = chunk.to_chunk_data(&registries.blocks).expect("encodes");
    let reloaded = Chunk::from_chunk_data(&re_encoded, &registries.blocks).expect("reloads");

    assert_eq!(reloaded.pos, chunk.pos);
    assert_eq!(reloaded.sections.len(), chunk.sections.len());
    for (before, after) in chunk.sections.iter().zip(&reloaded.sections) {
        assert_eq!(before.y, after.y);
        assert_eq!(
            before.blocks, after.blocks,
            "section y={} changed on round trip",
            before.y
        );
        assert_eq!(before.non_empty_block_count, after.non_empty_block_count);
    }
    // The heightmap is regenerated rather than carried from disk; it must still be
    // 37 longs (256 entries at 9 bits, 7 per long) and plausible.
    let heightmap = reloaded.heightmap_long_array();
    assert_eq!(heightmap.len(), 37);
    let min_y = reloaded.min_y();
    for column in 0..256 {
        let slot = column / 7;
        let offset = ((column % 7) as u32) * 9;
        let height = (heightmap[slot] >> offset) & 0x1FF;
        assert!(
            height >= i64::from(min_y) && height <= i64::from(reloaded.max_y()),
            "column {column} height {height} outside the world"
        );
    }
}

#[test]
fn a_vanilla_chunk_can_be_placed_in_a_world_and_walked_on() {
    let registries = Registries::vanilla().expect("registry");
    let data = vanilla_chunk_data();
    let chunk = Chunk::from_chunk_data(&data, &registries.blocks).expect("loads");
    let mut world = World::new(
        mc_persistence::dimension::Dimension::Overworld,
        registries.blocks.clone(),
    );
    world.load_chunk(chunk);

    // Pick a column with terrain, stand on top of it, and drop a player.
    let world_x = FIXTURE_CHUNK.x * 16 + 8;
    let world_z = FIXTURE_CHUNK.z * 16 + 8;
    let surface = world
        .find_surface(world_x, world_z, 200)
        .expect("vanilla terrain has a surface");
    let player = Aabb::player(Vec3::new(
        f64::from(world_x) + 0.5,
        f64::from(surface) + 20.0,
        f64::from(world_z) + 0.5,
    ));
    let result = world.move_with_collision(player, Vec3::new(0.0, -30.0, 0.0));
    assert!(result.on_ground, "the player must land on vanilla terrain");
    let landed_y = (player.min_y + result.delta.y).round() as i32;
    assert_eq!(
        landed_y, surface,
        "landed at {landed_y}, expected the surface at {surface}"
    );
    assert!(
        world.is_solid(world_x, surface - 1, world_z),
        "ground below the landing spot is solid"
    );
    assert!(
        !world.is_solid(world_x, surface, world_z),
        "the standing space is free"
    );
}

#[test]
fn an_unknown_block_name_is_refused_rather_than_becoming_air() {
    let registries = Registries::vanilla().expect("registry");
    let mut data = vanilla_chunk_data();
    // Corrupt one palette entry with a block that does not exist.
    let section = &mut data.sections[4];
    let mut values = section.block_states.to_values().expect("unpacks");
    values[0] = mc_persistence::chunk::BlockState::new("minecraft:not_a_real_block");
    section.block_states = mc_persistence::chunk::PalettedContainer::from_values(
        values,
        mc_core::packing::BLOCK_ENTRIES,
        mc_core::packing::BLOCK_MIN_BITS,
    )
    .expect("repacks");

    let error = Chunk::from_chunk_data(&data, &registries.blocks)
        .expect_err("an unknown block must not load silently");
    let message = format!("{error}");
    assert!(
        message.contains("not_a_real_block"),
        "the error must name the block: {message}"
    );
    assert!(
        message.contains("slot"),
        "the error must name the position: {message}"
    );
}
