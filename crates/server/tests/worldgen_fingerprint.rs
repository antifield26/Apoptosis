//! Worldgen-set fingerprint end to end (AUDIT-18 F-M2b): the boot records
//! the installed ore/carver set into `level.dat`, and a boot whose set
//! differs records the new one (the mismatch warn in between is log-only).
//!
//! Falsification: skip the record write in `open_world` and the first
//! assertion goes red; serve a changed pack without updating and the third
//! goes red.

use mc_server::lifecycle::Server;
use mc_test_support::fixtures::TempDir;

fn config_for(dir: &TempDir, packroot: std::path::PathBuf) -> mc_server::config::ServerConfig {
    let mut config = mc_server::config::ServerConfig::default();
    config.storage.world_dir = dir.path().join("world");
    config.storage.autosave_ticks = 0;
    config.datapacks.vanilla_data = Some(packroot);
    config
}

fn write_ore_pair(placed: &std::path::Path, configured: &std::path::Path, name: &str) {
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

fn write_carver(carvers: &std::path::Path, name: &str) {
    std::fs::write(
        carvers.join(format!("{name}.json")),
        "{\"type\": \"minecraft:cave\", \"config\": {\"probability\": 1.0, \
         \"y\": {\"type\": \"uniform\", \"min_inclusive\": {\"absolute\": -20}, \
         \"max_inclusive\": {\"absolute\": 0}}}}",
    )
    .expect("carver fixture");
}

fn fingerprint_of(server: &Server<mc_server::lifecycle::NoopHook>) -> Option<String> {
    server
        .world()?
        .storage()
        .level()?
        .extra
        .iter()
        .find(|(key, _)| key == mc_server::packs::WORLDGEN_FINGERPRINT_KEY)
        .and_then(|(_, tag)| match tag {
            mc_nbt::NbtTag::String(previous) => Some(previous.clone()),
            _ => None,
        })
}

#[test]
fn boot_records_the_worldgen_set_and_updates_it_when_the_pack_changes() {
    let dir = TempDir::new("worldgen-fingerprint");
    let namespace = dir.path().join("packroot").join("data").join("minecraft");
    let placed = namespace.join("worldgen").join("placed_feature");
    let configured = namespace.join("worldgen").join("configured_feature");
    let carvers = namespace.join("worldgen").join("configured_carver");
    for d in [&placed, &configured, &carvers] {
        std::fs::create_dir_all(d).expect("fixture dirs");
    }
    write_ore_pair(&placed, &configured, "ore_dirt");
    write_ore_pair(&placed, &configured, "ore_gravel");
    write_carver(&carvers, "cave");

    let mut server = Server::new(config_for(&dir, dir.path().join("packroot")));
    server.open_world().expect("world opens");
    let first = fingerprint_of(&server).expect("the boot records the set");
    assert!(
        first.contains("ores=2") && first.contains("carvers=1"),
        "the fingerprint describes the install, saw {first}"
    );
    // Persist the recorded fingerprint: the reboot reads it from disk.
    server
        .game_mut()
        .expect("game")
        .save_all_owned()
        .expect("level saves");
    drop(server);

    // One more carver file: the set (and therefore the fingerprint) moves,
    // and the reboot records the new one instead of keeping the stale one.
    write_carver(&carvers, "canyon");
    let mut server = Server::new(config_for(&dir, dir.path().join("packroot")));
    server.open_world().expect("world reopens");
    let second = fingerprint_of(&server).expect("the reboot records the set");
    assert_ne!(
        first, second,
        "a changed pack must move the recorded fingerprint"
    );
    assert!(
        second.contains("carvers=2"),
        "the new set is what is recorded, saw {second}"
    );
}
