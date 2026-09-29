//! P18-03 wiring: the live chunk pipeline runs the pack-driven ore and
//! carver passes, not just the library.
//!
//! `ore_carver_stats` pins the modules (tolerances, determinism, the
//! neutralised-assembly red) but generates through `mc-worldgen` directly, so
//! a server that never called either pass stayed green — which is exactly
//! what happened: the live `ensure_chunk` ran terrain → structures → trees
//! with no carve or ore step. These tests generate through [`Game`], so
//! removing the wiring call in `persist.rs` turns the first one red.
//!
//! Both sets are hand-built (no pack on disk): the point is the *wiring*,
//! not the pack contents, and a pack-dependent test would skip vacuously
//! where the extract is absent. The carver shape is the same forced cave
//! `mc-worldgen`'s own unit test uses; the ore is one coal feature with a
//! fixed attempt count over a y band that is stone on this terrain.

use std::collections::BTreeSet;

use mc_network::bridge::game_channel;
use mc_server::game::{DEFAULT_RANDOM_SEED, Game};
use mc_server::storage::WorldService;
use mc_test_support::fixtures::TempDir;
use mc_world::ChunkPos;
use mc_worldgen::carver::{CarverConfig, CarverKind, CarverSet};
use mc_worldgen::ore::{OreConfig, OreFeature, OreSet, OreTarget};
use mc_worldgen::{Attempts, FloatRange, HeightDist, YAnchor};

/// A live game on a fresh world, mirroring the `loot_and_pickup` harness:
/// owned storage is what licenses generation at all.
fn open_world(tag: &str) -> (Game, TempDir) {
    let dir = TempDir::new(tag);
    let config = mc_server::config::StorageConfig {
        world_dir: dir.path().join("world"),
        autosave_ticks: 0,
        seed: None,
    };
    let storage = WorldService::open(&config).expect("world opens");
    let (_tx, rx) = game_channel(256);
    let game =
        Game::with_seed_and_storage(storage, 4, rx, DEFAULT_RANDOM_SEED).expect("game builds");
    (game, dir)
}

/// One coal feature over a y band that is stone on the P07 terrain, with a
/// fixed attempt count so the pin cannot flake on a count roll.
fn coal_set(game: &Game) -> OreSet {
    let blocks = &game.registries().blocks;
    let stone = blocks.default_state("minecraft:stone").expect("stone");
    let coal = blocks
        .default_state("minecraft:coal_ore")
        .expect("coal ore");
    OreSet::from_features(vec![OreFeature {
        name: "test_coal".to_owned(),
        config: OreConfig {
            name: "test_coal".to_owned(),
            size: 8,
            discard_chance_on_air_exposure: 0.0,
            targets: vec![OreTarget {
                ore: coal,
                replaceable_tag: "minecraft:stone_ore_replaceables".to_owned(),
                replaceable: BTreeSet::from([stone]),
            }],
        },
        attempts: Attempts::Fixed(64),
        height: HeightDist::Uniform {
            min: YAnchor::Absolute(0),
            max: YAnchor::Absolute(40),
        },
    }])
}

/// The forced cave from `mc-worldgen`'s own carver unit test: probability 1.0
/// over a low-stone band that overlaps the ore band, so order matters — a
/// carve that runs after the ores would leave veins standing inside tunnels
/// (the carver only replaces stone), while carve-first starves those cells
/// of ore. The band tops out at 16, below the lowest surface (~22), so every
/// cell in it is solid stone before either pass runs.
fn cave_set(game: &Game) -> CarverSet {
    let blocks = &game.registries().blocks;
    let stone = blocks.default_state("minecraft:stone").expect("stone");
    CarverSet::from_carvers(vec![CarverConfig {
        name: "test_cave".to_owned(),
        kind: CarverKind::Cave,
        probability: 1.0,
        y: HeightDist::Uniform {
            min: YAnchor::Absolute(0),
            max: YAnchor::Absolute(16),
        },
        y_scale: FloatRange { min: 0.5, max: 1.0 },
        horizontal_radius_multiplier: FloatRange { min: 0.8, max: 1.2 },
        vertical_radius_multiplier: FloatRange { min: 0.8, max: 1.2 },
        floor_level: FloatRange {
            min: -1.0,
            max: -1.0,
        },
        replaceable_tag: "test:stone".to_owned(),
        replaceable: BTreeSet::from([stone]),
        thickness: FloatRange { min: 1.0, max: 2.0 },
        vertical_rotation: FloatRange { min: 0.0, max: 0.0 },
        lava_level: 8,
    }])
}

/// Count coal-ore cells and carved-air cells in one generated chunk.
///
/// The 0..=16 band is solid stone on generated terrain — the surface never
/// dips that low — so any air there was carved, not provided.
fn scan_chunk(game: &Game, pos: ChunkPos) -> (usize, usize) {
    let blocks = &game.registries().blocks;
    let coal = blocks
        .default_state("minecraft:coal_ore")
        .expect("coal ore");
    let air = blocks.air_id();
    let chunk = game
        .world()
        .chunk(pos)
        .expect("the chunk was just generated");
    let mut coal_seen = 0;
    let mut carved_seen = 0;
    for y in chunk.min_y()..chunk.max_y() {
        for z in 0..16 {
            for x in 0..16 {
                let id = chunk.get_block(x, y, z);
                if id == coal {
                    coal_seen += 1;
                }
                if (0..=16).contains(&y) && id == air {
                    carved_seen += 1;
                }
            }
        }
    }
    (coal_seen, carved_seen)
}

#[test]
fn live_generation_places_ores_and_carves_caves() {
    let (mut game, _dir) = open_world("underground_wired");
    game.set_ores_and_carvers(coal_set(&game), cave_set(&game));
    assert_eq!(game.ores().len(), 1, "the ore set installed");
    assert_eq!(game.carvers().len(), 1, "the carver set installed");

    let pos = ChunkPos::new(0, 0);
    assert!(game.load_chunk(pos), "generation serves the chunk");
    let (coal, carved) = scan_chunk(&game, pos);
    assert!(coal > 0, "the live pipeline placed ore (got none)");
    assert!(carved > 0, "the live pipeline carved cave air (got none)");
}

#[test]
fn empty_sets_leave_p07_terrain_untouched() {
    // The no-pack contract: default empty sets are no-op passes, and this is
    // also the perturbation control — it asserts the exact zeros the wired
    // test above must not produce.
    let (mut game, _dir) = open_world("underground_empty");
    assert!(game.ores().is_empty());
    assert!(game.carvers().is_empty());

    let pos = ChunkPos::new(0, 0);
    assert!(game.load_chunk(pos), "generation serves the chunk");
    let (coal, carved) = scan_chunk(&game, pos);
    assert_eq!(coal, 0, "no ore set, no ore");
    assert_eq!(carved, 0, "no carver set, no carved air");
}

/// F-M2: `load_packs` installs the ore/carver sets from a vanilla-data dir
/// into the live game — the pack→`Game` seam the walk needed, pinned here
/// with a generated fixture pack (all 25 placed + 25 configured ore files,
/// all 3 carvers) so no jar extract is required.
#[test]
fn pack_installs_ore_and_carver_sets_into_the_game() {
    use mc_server::packs::{PackRoots, load_packs};

    let (mut game, dir) = open_world("underground_pack_install");
    let namespace = dir.path().join("packroot").join("data").join("minecraft");
    let placed = namespace.join("worldgen").join("placed_feature");
    let configured = namespace.join("worldgen").join("configured_feature");
    let carvers = namespace.join("worldgen").join("configured_carver");
    for dir in [&placed, &configured, &carvers] {
        std::fs::create_dir_all(dir).expect("fixture dirs");
    }
    // One minimal valid feature per hardcoded name: count-2 blobs of coal
    // into stone over a fixed band.
    for name in mc_worldgen::OVERWORLD_ORE_PLACED_FEATURES {
        std::fs::write(
            placed.join(format!("{name}.json")),
            format!(
                "{{\"feature\": \"minecraft:{name}\", \"placement\": \
                 [{{\"type\": \"minecraft:count\", \"count\": 2}}, \
                 {{\"type\": \"minecraft:height_range\", \"height\": \
                 {{\"type\": \"uniform\", \"min_inclusive\": {{\"absolute\": -10}}, \
                 \"max_inclusive\": {{\"absolute\": 10}}}}}}]}}"
            ),
        )
        .expect("placed fixture");
        std::fs::write(
            configured.join(format!("{name}.json")),
            "{\"type\": \"minecraft:ore\", \"config\": {\"size\": 4, \"targets\": \
             [{\"state\": {\"Name\": \"minecraft:coal_ore\"}, \
             \"target\": {\"block\": \"minecraft:stone\"}}]}}",
        )
        .expect("configured fixture");
    }
    // Every carver name loads as a plain cave (the file's own type decides
    // the kind, not the filename).
    for name in mc_worldgen::OVERWORLD_CARVERS {
        std::fs::write(
            carvers.join(format!("{name}.json")),
            "{\"type\": \"minecraft:cave\", \"config\": {\"probability\": 1.0, \
             \"y\": {\"type\": \"uniform\", \"min_inclusive\": {\"absolute\": -20}, \
             \"max_inclusive\": {\"absolute\": 0}}}}",
        )
        .expect("carver fixture");
    }

    let roots =
        PackRoots::new(dir.path().join("world")).with_vanilla_data(dir.path().join("packroot"));
    let outcome = load_packs(
        &mut game,
        &roots,
        &mc_data::enabled::EnabledPacks::default(),
    )
    .expect("fixture pack loads");
    assert!(
        outcome.ores_skipped.is_empty(),
        "every fixture ore must load: {:?}",
        outcome.ores_skipped
    );
    assert!(
        outcome.carvers_skipped.is_empty(),
        "every fixture carver must load: {:?}",
        outcome.carvers_skipped
    );
    assert_eq!(outcome.ores_loaded, 25);
    assert_eq!(outcome.carvers_loaded, 3);
    assert_eq!(game.ores().len(), 25, "the game holds the ore set");
    assert_eq!(game.carvers().len(), 3, "the game holds the carver set");
}
#[test]
fn live_generation_matches_the_documented_pass_order() {    use mc_worldgen::seed::WorldgenContext;
    use mc_worldgen::terrain::ChunkGenerator;
    use mc_worldgen::{WorldSeed, carve_chunk, populate_ores};

    let (mut game, _dir) = open_world("underground_order");
    let ores = coal_set(&game);
    let carvers = cave_set(&game);
    game.set_ores_and_carvers(ores.clone(), carvers.clone());

    let blocks = game.registries().blocks.clone();
    // The documented order, replayed outside the server (structures are
    // empty without packs, so the direct replay skips them honestly).
    let context =
        WorldgenContext::overworld(WorldSeed::from_raw(mc_server::game::DEFAULT_RANDOM_SEED));
    let generator =
        mc_worldgen::TerrainGenerator::new(context.clone(), &blocks).expect("generator");

    for pos in [ChunkPos::new(0, 0), ChunkPos::new(3, -2)] {
        assert!(game.load_chunk(pos), "live game serves {pos:?}");
        let live = game.world().chunk(pos).expect("live chunk").clone();
        let mut direct = generator
            .generate_chunk(pos, &blocks)
            .expect("direct terrain");
        let _ = carve_chunk(&mut direct, pos, &context, &carvers, &blocks);
        let _ = populate_ores(&mut direct, pos, &context, &ores, &blocks);
        generator.decorate(&mut direct, pos, &blocks);
        // The live path marks generated chunks clean (persistence contract);
        // match that so the comparison is about blocks, not flags.
        direct.mark_clean();
        assert_eq!(
            live, direct,
            "live chunk must equal the documented pass order at {pos:?}"
        );
    }
}

/// Same seed twice through `Game` is bit-stable (determinism half of F-M1).
#[test]
fn live_generation_is_bit_stable_across_games() {
    let (mut first, _dir_a) = open_world("underground_det_a");
    let ores = coal_set(&first);
    let carvers = cave_set(&first);
    first.set_ores_and_carvers(ores.clone(), carvers.clone());
    let (mut second, _dir_b) = open_world("underground_det_b");
    second.set_ores_and_carvers(ores, carvers);

    for pos in [
        ChunkPos::new(0, 0),
        ChunkPos::new(3, -2),
        ChunkPos::new(-5, 4),
    ] {
        assert!(first.load_chunk(pos), "first game serves {pos:?}");
        assert!(second.load_chunk(pos), "second game serves {pos:?}");
        let a = first.world().chunk(pos).expect("first chunk");
        let b = second.world().chunk(pos).expect("second chunk");
        assert_eq!(
            a, b,
            "same seed twice through Game must give the same chunk at {pos:?}"
        );
    }
}
