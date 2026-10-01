//! World-seed plumbing end to end (AUDIT-18 F-H1): a configured seed reaches
//! the live game on a fresh world, the recorded seed survives a reboot with the
//! config key removed (stored wins over configured), a world created here
//! records the seed it generated from (AUDIT-19 D-19-M1), and a world whose
//! seed is recorded **only** in the 26.1 location keeps its terrain
//! (AUDIT-19 D-19-H1).
//!
//! Falsification (each mechanism has one named test that fails without it):
//! hardcode seed 0 back into `open_world` and
//! `configured_seed_reaches_the_game_and_survives_unset` goes red; drop both
//! seed-recording sites (`WorldService::open`, `WorldService::record_seed`) and
//! `a_world_created_here_records_its_seed_so_a_later_config_key_cannot_fork_it`
//! goes red; drop the world-gen settings read in `WorldService::recorded_seed`
//! and `a_world_whose_seed_is_only_in_the_26_1_settings_file_keeps_its_terrain`
//! goes red with `0 != 1361882806` (a real 26.1.2 `level.dat` carries no seed at
//! all — asserted in the test itself, not assumed).

use mc_persistence::level::{DATA_VERSION_26_1_2, LevelDat};
use mc_server::lifecycle::Server;
use mc_test_support::fixtures::{TempDir, read_fixture};

fn config_for(dir: &TempDir, seed: Option<i64>) -> mc_server::config::ServerConfig {
    let mut config = mc_server::config::ServerConfig::default();
    config.storage.world_dir = dir.path().join("world");
    config.storage.autosave_ticks = 0;
    config.storage.seed = seed;
    config
}

fn game_seed(server: &Server<mc_server::lifecycle::NoopHook>) -> i64 {
    server
        .game()
        .expect("game built by open_world")
        .random_seed()
}

/// A synthetic `data/minecraft/world_gen_settings.dat`.
///
/// Synthetic on purpose: the repository holds **no** jar-derived fixture for
/// this file (AUDIT-19 section 9: "the seed key path inside
/// `world_gen_settings.dat`" is unverified), so the bytes are built here from
/// the two things the tree does record about it —
/// `docs/research/protocol-baseline.md` section 2 ("each wrapped in
/// `{DataVersion, data}`") and the candidate key paths
/// `mc_worldgen::seed::WorldSeed::from_level_dat` accepts — rather than
/// committing data derived from a Mojang jar.
fn synthetic_world_gen_settings(seed: i64) -> Vec<u8> {
    let document = mc_nbt::NbtTag::compound([
        (
            "DataVersion".to_owned(),
            mc_nbt::NbtTag::Int(DATA_VERSION_26_1_2),
        ),
        (
            "data".to_owned(),
            mc_nbt::NbtTag::compound([(
                "dimensions".to_owned(),
                mc_nbt::NbtTag::compound([(
                    "minecraft:overworld".to_owned(),
                    mc_nbt::NbtTag::compound([(
                        "generator".to_owned(),
                        mc_nbt::NbtTag::compound([("seed".to_owned(), mc_nbt::NbtTag::Long(seed))]),
                    )]),
                )]),
            )]),
        ),
    ]);
    mc_persistence::save::encode_gzip_nbt("", &document).expect("the document encodes")
}

/// Lay the vanilla 26.1.2 `level.dat` fixture down as a world, with the seed
/// recorded only where 26.1 keeps it.
fn vanilla_world_with_recorded_seed_only_in_settings(
    dir: &TempDir,
    seed: i64,
) -> std::path::PathBuf {
    let world_dir = dir.path().join("world");
    std::fs::create_dir_all(&world_dir).expect("mkdir world");
    let level_bytes = read_fixture("anvil", "level_26_1_2.dat").expect("vanilla level.dat fixture");
    // The premise of the whole test: this file records no seed, so a reader
    // that only looks here cannot know the world's seed.
    assert_eq!(
        LevelDat::from_bytes(&level_bytes)
            .expect("the fixture decodes")
            .seed,
        None,
        "a real 26.1.2 level.dat must carry no seed key, or this test proves nothing"
    );
    std::fs::write(world_dir.join("level.dat"), &level_bytes).expect("level.dat");
    let settings = world_dir.join("data").join("minecraft");
    std::fs::create_dir_all(&settings).expect("mkdir data/minecraft");
    std::fs::write(
        settings.join("world_gen_settings.dat"),
        synthetic_world_gen_settings(seed),
    )
    .expect("world_gen_settings.dat");
    world_dir
}

#[test]
fn configured_seed_reaches_the_game_and_survives_unset() {
    let dir = TempDir::new("seed-plumbing");
    let world_dir = dir.path().join("world");

    // Fresh world + configured seed: the game generates from it, and the
    // seed is recorded in level.dat.
    let mut server = Server::new(config_for(&dir, Some(1_361_882_806)));
    server.open_world().expect("world opens");
    assert_eq!(game_seed(&server), 1_361_882_806);
    drop(server);

    // Same directory, config key removed: the recorded seed still wins over
    // the historical seed-0 default.
    let mut plain = mc_server::config::ServerConfig::default();
    plain.storage.world_dir = world_dir;
    plain.storage.autosave_ticks = 0;
    let mut server = Server::new(plain);
    server.open_world().expect("world reopens");
    assert_eq!(
        game_seed(&server),
        1_361_882_806,
        "the stored seed must survive dropping the config key"
    );
}

/// AUDIT-19 D-19-H1: a vanilla 26.1 world keeps its terrain.
///
/// Its seed is recorded only in `data/minecraft/world_gen_settings.dat` — the
/// fixture `level.dat` has no seed key at all — so a resolver that reads only
/// `level.dat` falls through to the seed-0 default and generates seed-0
/// terrain for every new chunk. This test is the named pin for that mechanism:
/// neutralise the world-gen settings read and it fails with `0 != 1361882806`.
#[test]
fn a_world_whose_seed_is_only_in_the_26_1_settings_file_keeps_its_terrain() {
    let dir = TempDir::new("seed-26-1-settings");
    let world_dir = vanilla_world_with_recorded_seed_only_in_settings(&dir, 1_361_882_806);

    // No configured seed: the world's own record is the only source.
    let mut server = Server::new(config_for(&dir, None));
    server.open_world().expect("the vanilla world opens");
    assert_eq!(
        game_seed(&server),
        1_361_882_806,
        "a 26.1 world's seed lives in world_gen_settings.dat; reading only level.dat \
         yields the seed-0 default and forks its terrain"
    );
    drop(server);

    // ... and it is now recorded in level.dat as well (AUDIT-19 D-19-M1), so
    // the world keeps generating from it even if the settings file is lost.
    let level = LevelDat::from_bytes(
        &std::fs::read(world_dir.join("level.dat")).expect("level.dat after the boot"),
    )
    .expect("decodes");
    assert_eq!(level.seed, Some(1_361_882_806));

    std::fs::remove_file(
        world_dir
            .join("data")
            .join("minecraft")
            .join("world_gen_settings.dat"),
    )
    .expect("remove the 26.1 document");
    let mut server = Server::new(config_for(&dir, None));
    server
        .open_world()
        .expect("world reopens without the document");
    assert_eq!(
        game_seed(&server),
        1_361_882_806,
        "the recorded seed must survive losing the world-gen settings document"
    );
}

/// AUDIT-19 D-19-M1: a world created here records the seed it generated from,
/// so adding (or clearing) `seed = N` in the config cannot fork it.
#[test]
fn a_world_created_here_records_its_seed_so_a_later_config_key_cannot_fork_it() {
    let dir = TempDir::new("seed-recorded-at-creation");
    let world_dir = dir.path().join("world");
    let default_seed = mc_server::game::DEFAULT_RANDOM_SEED;

    // First boot: no configured seed, fresh world, historical default seed.
    let mut server = Server::new(config_for(&dir, None));
    server.open_world().expect("world opens");
    assert_eq!(game_seed(&server), default_seed);
    drop(server);

    let level = LevelDat::from_bytes(
        &std::fs::read(world_dir.join("level.dat")).expect("level.dat after the first boot"),
    )
    .expect("decodes");
    assert_eq!(
        level.seed,
        Some(default_seed),
        "a world created here must record the seed it generated from, \
         or a later `seed = N` forks it"
    );

    // Second boot: the config now names a seed. The world already records one,
    // so it must keep generating from what it recorded.
    let mut server = Server::new(config_for(&dir, Some(4_242)));
    server.open_world().expect("world reopens");
    assert_eq!(
        game_seed(&server),
        default_seed,
        "adding `seed` after the world exists must not fork it"
    );
}
