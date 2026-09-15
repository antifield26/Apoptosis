//! A scripted multi-step scenario against the real 26.1.2 data pack (P07-19).
//!
//! Four single-purpose differential suites already exist (`vanilla_pack`, `vanilla_data`,
//! `vanilla_smelting`, `structure_pack`). This is the **expansion** P07-19 asks for: five stages run in
//! sequence against one world, asserting the end state.
//!
//! What a scenario catches that a unit suite cannot: a stage that works alone and not in company. The
//! last stage is the clearest example — generate a chunk, save it, discard the server, reopen, and
//! assert the **stored** world came back. Each layer passes its own tests; only running them in order
//! shows they agree about what a chunk is.
//!
//! Each stage is its own function so a failure names the stage, and they run inside one `#[test]`
//! because the *sequence* is the claim.
//!
//! (Absolute: the test's working directory is the crate's, not the repository
//! root -- AUDIT-09 D-06. Above the fence, because it is prose, not part of the command.)
//!
//! ```text
//! set MC_VANILLA_DATA=%CD%\target\vanilla-26.1.2\extract\data\minecraft
//! cargo test -p mc-server --test scenario_vanilla -- --ignored --nocapture
//! ```

use mc_container::furnace::SmeltingRegistry;
use mc_container::smelting_data::TagResolver;
use mc_core::ids::ResourceId;
use mc_data::recipe::{RecipeLoadReport, load_directory as load_recipes};
use mc_data::tag::{RegistryContents, load_directory as load_tags};
use mc_data::{Limits, RecipeBook, SmeltingKind, TagKey, TagLoadReport, TagSet};
use mc_registry::{ItemRegistry, Registries};
use mc_server::game::Game;
use mc_server::storage::WorldService;
use mc_test_support::fixtures::TempDir;
use mc_world::ChunkPos;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// The pack root, or `None` when the variable is unset.
fn pack_root() -> Option<PathBuf> {
    let root = PathBuf::from(std::env::var("MC_VANILLA_DATA").ok()?);
    assert!(
        root.is_dir(),
        "MC_VANILLA_DATA points at {}, which is not a directory; see \
         docs/research/data-pack-baseline.md section 0",
        root.display()
    );
    Some(root)
}

fn items() -> ItemRegistry {
    ItemRegistry::load(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../test-support/fixtures/registry/items.tsv"),
    )
    .expect("the item fixture loads")
}

fn world_config(dir: &TempDir) -> mc_server::config::StorageConfig {
    mc_server::config::StorageConfig {
        world_dir: dir.path().join("world"),
        autosave_ticks: 0,
    }
}

/// A game that **owns** storage, at a fixed seed so the scenario is reproducible.
///
/// Owning is required, not incidental: `Game::with_seed` borrows, and generation is gated on
/// `can_read_stored_chunks` precisely so a borrowing game cannot generate over a saved world. A
/// borrowing game produces a placeholder here — which `a_borrowing_game_cannot_generate` asserts.
fn owning_game(dir: &TempDir) -> Game {
    let service = WorldService::open(&world_config(dir)).expect("world opens");
    Game::with_seed_and_storage(service, 3, mc_network::bridge::game_channel(64).1, 7)
        .expect("game builds")
}

/// **Stage 1**: the pack's tags resolve against its own registries.
fn stage_tags(root: &Path) -> TagSet {
    let item_ids: BTreeSet<ResourceId> = items()
        .names()
        .filter_map(|name| ResourceId::parse(name).ok())
        .collect();
    let mut contents = RegistryContents::new();
    contents.insert("item", item_ids);
    let mut report = TagLoadReport::default();
    let files = load_tags(root, "minecraft", Limits::DEFAULT, &mut report);
    let tags = TagSet::from_map(mc_data::resolve_all(&files, &contents, &mut report));

    assert_eq!(report.files, 758, "the pack's tag count");
    assert!(
        report.problems.is_empty(),
        "every reference must resolve: {:?}",
        report.problems
    );
    println!("stage 1: {} tags resolved, 0 problems", tags.len());
    tags
}

/// **Stage 2**: the furnace table is built from the pack's own smelting recipes.
fn stage_furnace(root: &Path, tags: &TagSet) -> SmeltingRegistry {
    let mut report = RecipeLoadReport::default();
    let loaded = load_recipes(root, "minecraft", Limits::DEFAULT, &mut report);
    let mut book = RecipeBook::new();
    for recipe in loaded {
        book.insert(recipe);
    }
    assert_eq!(
        report.total_loaded() + report.total_unmodelled(),
        1515,
        "every recipe file is loaded or counted"
    );

    let resolve_tag: TagResolver<'_> = &|tag: &ResourceId| {
        tags.get(&TagKey {
            registry: "item".to_owned(),
            name: tag.clone(),
        })
        .map(|set| set.iter().cloned().collect())
        .unwrap_or_default()
    };
    let registry = items();
    let (furnace, conversion) =
        SmeltingRegistry::from_recipes(&book, SmeltingKind::Smelting, &registry, Some(resolve_tag))
            .expect("the pack's smelting recipes convert");

    assert_eq!(conversion.recipes_seen, 73, "the pack's smelting count");
    let raw_iron = registry.id("minecraft:raw_iron").expect("raw iron");
    let recipe = furnace.recipe_for(raw_iron).expect("raw iron smelts");
    assert_eq!(recipe.cook_ticks, 200, "the jar's cookingtime");
    println!(
        "stage 2: {} recipes, {} unmodelled, {} furnace rows",
        report.total_loaded(),
        report.total_unmodelled(),
        furnace.len()
    );
    furnace
}

/// **Stage 3**: terrain generates, and a real structure template places into it.
///
/// Returns the marker position, so stage 5 can look for it.
fn stage_generate_and_place(root: &Path, dir: &TempDir) -> (i32, i32, i32) {
    let pos = ChunkPos::new(0, 0);
    let mut game = owning_game(dir);
    assert!(game.load_chunk(pos), "a chunk generates");

    let blocks = Registries::vanilla().expect("registry").blocks;
    let air = blocks.air_id();
    // Terrain, not air: the property a player standing on it depends on.
    let non_air = {
        let chunk = game.world().chunk(pos).expect("loaded");
        (0..16)
            .flat_map(|x| (0..16).map(move |z| (x, z)))
            .filter(|(x, z)| (-64..320).any(|y| chunk.get_block(*x, y, *z) != air))
            .count()
    };
    assert_eq!(non_air, 256, "every column has terrain");

    let (structures, report) =
        mc_worldgen::structures::load_structures(root, mc_worldgen::StructureLimits::PACK);
    assert_eq!(report.files, 1_202, "the pack's structure count");
    assert!(report.loaded > 1_000, "most templates load: {report:?}");
    println!(
        "stage 3: terrain in all 256 columns; {} of {} templates loaded",
        report.loaded, report.files
    );

    // A real template, placed into a real chunk. The origin is **derived from the chunk**, not written
    // as a literal: a template placed at the world origin lands nowhere near a chunk at `(4, 4)`, and
    // the `SingleChunk` policy then refuses it — correctly.
    let fitting = structures.fitting_in_one_chunk();
    let name = fitting
        .names()
        .into_iter()
        .find(|name| name.to_string().contains("igloo"))
        .or_else(|| fitting.names().into_iter().next())
        .expect("at least one single-chunk template");
    let template = fitting.by_name(&name).expect("just listed");
    let resolved = template.resolve(&blocks).expect("the pack resolves");
    let target_pos = ChunkPos::new(4, 4);
    let mut target = mc_world::Chunk::air(target_pos, -4, 24, &blocks);
    let placement = mc_worldgen::placement::place(
        &resolved,
        &mut target,
        (target_pos.x * 16, 64, target_pos.z * 16),
        mc_worldgen::AirPolicy::IgnoreAir,
        mc_worldgen::CrossChunk::SingleChunk,
        &blocks,
        &name.to_string(),
    );
    assert!(!placement.was_refused(), "a fitting template must place");
    assert!(
        placement.blocks_written > 0,
        "and write blocks: {placement:?}"
    );
    println!(
        "stage 4: placed {name} ({} blocks written)",
        placement.blocks_written
    );

    // **Stage 4**: a distinctive marker, so stage 5 can prove the stored chunk was read rather than
    // regenerated. A generated chunk is reproducible from the seed, so without a marker a reopened
    // world would look identical either way.
    let gold = blocks
        .default_state("minecraft:gold_block")
        .expect("gold_block resolves");
    let marker = {
        let chunk = game.world().chunk(pos).expect("loaded");
        (0..16)
            .flat_map(|x| (0..16).map(move |z| (x, z)))
            .find_map(|(x, z)| {
                (-64..320)
                    .rev()
                    .find(|y| chunk.get_block(x, *y, z) == air)
                    .map(|y| (x, y, z))
            })
            .expect("the chunk has air above its surface")
    };
    game.world_mut()
        .set_block(pos.x * 16 + marker.0, marker.1, pos.z * 16 + marker.2, gold)
        .expect("the marker is placed");
    game.save_all_owned().expect("save");
    marker
}

/// **Stage 5**: the stored world comes back rather than a regenerated one.
fn stage_reopen(dir: &TempDir, marker: (i32, i32, i32)) {
    let pos = ChunkPos::new(0, 0);
    let mut game = owning_game(dir);
    assert!(game.load_chunk(pos));

    let gold = Registries::vanilla()
        .expect("registry")
        .blocks
        .default_state("minecraft:gold_block")
        .expect("gold_block resolves");
    assert_eq!(
        game.world()
            .chunk(pos)
            .expect("loaded")
            .get_block(marker.0, marker.1, marker.2),
        gold,
        "the stored chunk must be read, not regenerated: the marker at {marker:?} is gone"
    );
    println!(
        "stage 5: the marker at ({}, {}, {}) survived a save/reopen",
        marker.0, marker.1, marker.2
    );
    game.save_all_owned().expect("save");
}

#[test]
#[ignore = "differential: needs MC_VANILLA_DATA (extracted data/minecraft from the 26.1.2 jar)"]
fn the_whole_pipeline_runs_against_the_real_pack() {
    let Some(root) = pack_root() else {
        eprintln!("MC_VANILLA_DATA is not set; skipping");
        return;
    };
    let dir = TempDir::new("scenario-vanilla");

    let tags = stage_tags(&root);
    let _furnace = stage_furnace(&root, &tags);
    let marker = stage_generate_and_place(&root, &dir);
    stage_reopen(&dir, marker);

    println!("scenario complete: 5 stages, all against the real pack");
}

#[test]
#[ignore = "differential: needs MC_VANILLA_DATA (extracted data/minecraft from the 26.1.2 jar)"]
fn a_borrowing_game_cannot_generate() {
    // The gate from the worldgen work, demonstrated end to end: a borrowing game cannot tell "no chunk
    // stored" from "cannot look", so it must not generate — and the result is a placeholder with no
    // terrain. The scenario above fails without this behaviour, which is what makes it a real
    // constraint rather than a documented intention.
    let Some(_root) = pack_root() else {
        eprintln!("MC_VANILLA_DATA is not set; skipping");
        return;
    };
    let dir = TempDir::new("scenario-borrowing");
    let service = WorldService::open(&world_config(&dir)).expect("world opens");
    let mut game = Game::with_seed(&service, 3, mc_network::bridge::game_channel(64).1, 7)
        .expect("game builds");
    let pos = ChunkPos::new(0, 0);
    game.load_chunk(pos);

    let blocks = Registries::vanilla().expect("registry").blocks;
    let air = blocks.air_id();
    let chunk = game.world().chunk(pos).expect("a placeholder exists");
    let non_air = (0..16)
        .flat_map(|x| (0..16).map(move |z| (x, z)))
        .filter(|(x, z)| (-64..320).any(|y| chunk.get_block(*x, y, *z) != air))
        .count();
    assert_eq!(
        non_air, 0,
        "a borrowing game must not generate: generation is gated on being able to read storage, so a \
         placeholder is the correct outcome"
    );
}
