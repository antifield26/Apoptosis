//! Does the server actually generate structures? (P07-16, integration)
//!
//! The structures library was fully tested — 1 182 templates load, selection is deterministic, placement
//! writes the right blocks — while `Game::load_or_create_chunk` never called any of it. A running server
//! generated **no structures at all** and the status table said DONE. This is the test that was missing.
//!
//! It found a second, deeper bug: with the wiring in place the decorator ran, the selection picked, and
//! **every placement was refused** (`selected: 2, blocks_written: 0, refused: 2`). `StructureSet` selects
//! from the first 64 names in sorted order — all `ancient_city/*`, which are multi-chunk — while the
//! default build policy is `CrossChunk::SingleChunk`. The library's own tests could not see it: they test
//! selection against synthetic names that fit and placement against individual templates, and only running
//! the two together shows they disagree.
//!
//! The assertions read `StructureStats`, not a terrain heuristic. A "column taller than its neighbours"
//! scan found nothing even while 944 blocks were written, because a bastion stair sits flush with the
//! ground — a heuristic can be wrong in both directions, and a counter cannot.

use mc_server::game::Game;
use mc_server::packs::{PackRoots, load_packs};
use mc_server::storage::WorldService;
use mc_test_support::fixtures::TempDir;
use mc_world::ChunkPos;
use std::path::PathBuf;

fn pack_root() -> Option<PathBuf> {
    let root = PathBuf::from(std::env::var("MC_VANILLA_DATA").ok()?);
    assert!(root.is_dir(), "{} is not a directory", root.display());
    Some(root)
}

fn config(dir: &TempDir) -> mc_server::config::StorageConfig {
    mc_server::config::StorageConfig {
        world_dir: dir.path().join("world"),
        autosave_ticks: 0,
    }
}

/// A game with the real pack loaded, and its storage path.
fn game_with_pack(dir: &TempDir, root: &std::path::Path) -> Game {
    let service = WorldService::open(&config(dir)).expect("world opens");
    let mut game =
        Game::with_seed_and_storage(service, 3, mc_network::bridge::game_channel(64).1, 7)
            .expect("game builds");
    let roots = PackRoots::new(dir.path().join("world")).with_vanilla_data(root);
    let outcome = load_packs(
        &mut game,
        &roots,
        &mc_data::enabled::EnabledPacks::default(),
    )
    .expect("packs load");
    println!(
        "pack: {} structure templates of {} seen ({} refused), {} functions",
        outcome.structures_loaded,
        outcome.structures_seen,
        outcome.structures_refused.len(),
        outcome.functions_loaded
    );
    game
}

#[test]
#[ignore = "differential: needs MC_VANILLA_DATA (extracted data/minecraft from the 26.1.2 jar)"]
fn generation_places_structures() {
    let Some(root) = pack_root() else {
        eprintln!("MC_VANILLA_DATA is not set; skipping");
        return;
    };
    let dir = TempDir::new("structures-wired");
    let mut game = game_with_pack(&dir, &root);

    // 1. The templates are installed, and the selectable set is non-empty.
    assert_eq!(
        game.structures().len(),
        1_182,
        "the pack's templates must be installed"
    );
    assert!(
        game.structure_set().names().len() > 1,
        "and a selectable rule derived from them"
    );

    // 2. Generate a region wide enough to contain selected cells.
    //
    // The grid is 12 chunks at this registry size, so 60x60 chunks covers several cells. Generation is
    // bounded by the *view distance*, so this drives `load_chunk` directly, which is the same path the
    // streamer uses.
    for cx in -30..30 {
        for cz in -30..30 {
            game.load_chunk(ChunkPos::new(cx, cz));
        }
    }

    let stats = game.structure_stats();
    println!("stats: {stats:?}");
    assert!(
        stats.considered > 0,
        "the decorator must be reached: generation placed nothing, which is the integration gap this \
         test exists for"
    );
    assert!(
        stats.selected > 0,
        "and the selection must pick something over {} chunks",
        stats.considered
    );

    // 3. **The claim the wiring bug violated.** A selection that is always refused means no structure
    //    ever appears, which is indistinguishable from no wiring at all unless it is asserted.
    assert_eq!(
        stats.refused, 0,
        "no selection may be refused: the set is derived from the templates the build policy can place, \
         so a refusal means the two have drifted apart again ({stats:?})"
    );
    assert!(
        stats.blocks_written > 0,
        "and the placed templates must write blocks: {stats:?}"
    );
    println!(
        "{} of {} chunks decorated, {} blocks written",
        stats.selected, stats.considered, stats.blocks_written
    );
}

#[test]
#[ignore = "differential: needs MC_VANILLA_DATA (extracted data/minecraft from the 26.1.2 jar)"]
fn the_selection_matches_what_the_build_policy_can_place() {
    // The invariant the bug violated, asserted directly rather than through a generated chunk: **every
    // name the selection can pick must be a template the policy can place**. That is the property the
    // fix establishes, and it is cheaper and sharper to check here than to infer from block counts.
    let Some(root) = pack_root() else {
        eprintln!("MC_VANILLA_DATA is not set; skipping");
        return;
    };
    let dir = TempDir::new("structures-consistent");
    let game = game_with_pack(&dir, &root);

    let selectable = game.structure_set().names();
    let fitting = game.structures().fitting_in_one_chunk();
    // Owned, because itting.names() returns a temporary slice.
    let fitting_names: std::collections::BTreeSet<mc_core::ids::ResourceId> =
        fitting.names().into_iter().collect();

    assert!(!selectable.is_empty());
    for name in selectable {
        assert!(
            fitting_names.contains(name),
            "{name} is selectable but does not fit in one chunk, so every placement of it would be \
             refused — the exact disagreement this test exists for"
        );
        assert!(
            game.structures().by_name(name).is_some(),
            "{name} is selectable but not in the registry"
        );
    }
    println!(
        "{} selectable names, all {} of them in the fitting subset of {}",
        selectable.len(),
        selectable.len(),
        fitting.len()
    );
}
