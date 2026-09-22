//! Admin commands end to end (P14-01).
//!
//! `/gamemode`, `/give`, `/kill`, `/seed` and `/difficulty` through real
//! dispatch: parse, permission check, effect. State asserts (mode field,
//! inventory counts, alive, difficulty, `level.dat`) carry the behaviour;
//! reply text is asserted where the message *is* the effect (`/seed`,
//! refusals, usage). An operator session comes from an `ops.json` listing
//! (the `ops_e2e` pattern: uuids from `offline_profile`); a plain session
//! proves the level-0 refusals.

use mc_command::PermissionLevel;
use mc_entity::{GameMode, MobKind};
use mc_network::bridge::{
    ClientEvent, ClientEventKind, ConnectionId, ConnectionIds, InboundReceiver, game_channel,
};
use mc_protocol::ids::clientbound;
use mc_protocol::packets::Packet;
use mc_protocol::packets::play::SystemChat;
use mc_server::game::{Game, TickReport};
use mc_server::ops::OperatorList;
use mc_server::storage::WorldService;
use mc_test_support::fixtures::TempDir;
use std::path::Path;

/// Everything a test needs: a live game, an operator-capable join path, and
/// the client's end of the channel for reply text.
struct Harness {
    game: Game,
    events: tokio::sync::mpsc::Sender<ClientEvent>,
    ids: ConnectionIds,
    dir: TempDir,
}

impl Harness {
    fn new(tag: &str, operators: OperatorList) -> Self {
        let dir = TempDir::new(tag);
        let config = mc_server::config::StorageConfig {
            world_dir: dir.path().join("world"),
            autosave_ticks: 0,
        };
        let service = WorldService::open(&config).expect("world opens");
        let (tx, rx) = game_channel(256);
        let game = Game::build_with_operators(None, Some(service), 3, rx, 7, operators)
            .expect("game builds");
        Self {
            game,
            events: tx,
            ids: ConnectionIds::new(),
            dir,
        }
    }

    /// Join `name` and return its connection plus the reply channel.
    fn join(&mut self, name: &str) -> (ConnectionId, InboundReceiver) {
        let id = self.ids.next_id();
        let (outbound, out) = mc_network::bridge::OutboundSender::pair(id, 8192);
        self.events
            .try_send(ClientEvent {
                id,
                kind: ClientEventKind::Joined {
                    profile: mc_network::auth::offline_profile(name),
                    outbound,
                },
            })
            .expect("join event queued");
        self.game.tick().expect("tick");
        (id, out)
    }

    /// Run a command as `id`, returning every `disguised_chat` line it produced.
    fn command(&mut self, id: ConnectionId, out: &mut InboundReceiver, text: &str) -> Vec<String> {
        let mut report = TickReport::default();
        self.game
            .dispatch_command(id, text, &mut report)
            .expect("a command is answered");
        let mut lines = Vec::new();
        while let Some(raw) = out.try_recv() {
            if raw.id == clientbound::play::DISGUISED_CHAT {
                match SystemChat::decode(&raw.payload) {
                    Ok(chat) => lines.push(chat.content.as_plain().to_owned()),
                    Err(error) => panic!("a disguised_chat must decode: {error}"),
                }
            }
        }
        lines
    }

    fn run(&mut self, ticks: usize) {
        for _ in 0..ticks {
            self.game.tick().expect("tick");
        }
    }

    fn stone_item(&self) -> i32 {
        self.game
            .registries()
            .items
            .id("minecraft:stone")
            .expect("stone is an item")
    }

    /// Total stones anywhere in the player's main inventory.
    fn count_item(&self, id: ConnectionId, item: i32) -> i32 {
        let player = self.game.player(id).expect("player");
        (0..36usize)
            .map(|slot| {
                let stack = player.inventory.slot(slot);
                if stack.item_id() == Some(item) {
                    stack.count()
                } else {
                    0
                }
            })
            .sum()
    }

    /// Fill every main slot with a full stack: the next give must overflow.
    fn fill_inventory(&mut self, id: ConnectionId, item: i32) {
        let player = self.game.player_mut(id).expect("player");
        for slot in 0..36usize {
            player
                .inventory
                .set_slot(
                    slot,
                    mc_entity::stack::ItemStack::new(item, 64).expect("full stack"),
                )
                .expect("slot takes a stack");
        }
    }
}

/// An operator list granting `name` at `level`.
fn ops_for(name: &str, level: u8) -> OperatorList {
    let uuid = mc_network::auth::offline_profile(name).id;
    let text = format!(r#"[{{"uuid": "{uuid}", "name": "{name}", "level": {level}}}]"#);
    OperatorList::parse(&text, Path::new("ops.json")).expect("the fixture parses")
}

#[test]
fn gamemode_flips_the_mode_field_and_denies_level_zero() {
    let mut harness = Harness::new("p14-gamemode", ops_for("Chief", 4));
    let (id, mut out) = harness.join("Chief");
    assert_eq!(
        harness.game.player_permission(id),
        PermissionLevel::Console,
        "fixture grants level 4"
    );

    let lines = harness.command(id, &mut out, "gamemode creative");
    assert_eq!(
        harness.game.player(id).expect("player").game_mode,
        GameMode::Creative,
        "creative must land on the player"
    );
    assert!(
        lines.iter().any(|line| line.contains("Creative Mode")),
        "reply names the mode, saw {lines:?}"
    );

    let lines = harness.command(id, &mut out, "gamemode s");
    assert_eq!(
        harness.game.player(id).expect("player").game_mode,
        GameMode::Survival,
        "the s shortcut flips back"
    );
    assert!(
        lines.iter().any(|line| line.contains("Survival Mode")),
        "saw {lines:?}"
    );

    harness.command(id, &mut out, "gamemode flying");
    assert_eq!(
        harness.game.player(id).expect("player").game_mode,
        GameMode::Survival,
        "a bad mode word must not change anything"
    );

    // Someone else's mode is refused with the reason (the /tp limitation).
    let lines = harness.command(id, &mut out, "gamemode creative SomeoneElse");
    assert_eq!(
        harness.game.player(id).expect("player").game_mode,
        GameMode::Survival
    );
    assert!(
        lines
            .iter()
            .any(|line| line.contains("only changes the invoking player")),
        "saw {lines:?}"
    );

    // A plain player is refused and unchanged.
    let (plain, mut plain_out) = harness.join("Nobody");
    let lines = harness.command(plain, &mut plain_out, "gamemode creative");
    assert_eq!(
        harness.game.player(plain).expect("player").game_mode,
        GameMode::Survival
    );
    assert!(
        lines
            .iter()
            .any(|line| line.contains("do not have permission")),
        "saw {lines:?}"
    );
}

#[test]
fn give_adds_counts_drops_overflow_and_validates() {
    let mut harness = Harness::new("p14-give", ops_for("Chief", 4));
    let (id, mut out) = harness.join("Chief");
    let stone = harness.stone_item();

    harness.command(id, &mut out, "give Chief minecraft:stone 5");
    assert_eq!(harness.count_item(id, stone), 5, "five stones land");

    // Default count is one; bare names resolve through the minecraft namespace.
    harness.command(id, &mut out, "give Chief stone");
    assert_eq!(harness.count_item(id, stone), 6);

    // Unknown items change nothing.
    let lines = harness.command(id, &mut out, "give Chief minecraft:unobtainium 5");
    assert_eq!(harness.count_item(id, stone), 6);
    assert!(
        lines.iter().any(|line| line.contains("Unknown item")),
        "saw {lines:?}"
    );

    // Someone else's inventory is refused.
    let lines = harness.command(id, &mut out, "give SomeoneElse stone 5");
    assert_eq!(harness.count_item(id, stone), 6);
    assert!(
        lines
            .iter()
            .any(|line| line.contains("only gives to the invoking player")),
        "saw {lines:?}"
    );

    // A full inventory drops the remainder at the player's feet (vanilla's
    // rule): 36 full stacks, then one more stone must announce an entity.
    harness.fill_inventory(id, stone);
    harness.command(id, &mut out, "give Chief stone 1");
    harness.run(3);
    let mut adds = 0;
    while let Some(raw) = out.try_recv() {
        if raw.id == clientbound::play::ADD_ENTITY {
            adds += 1;
        }
    }
    assert!(adds >= 1, "overflow must drop rather than vanish");
}

#[test]
fn kill_runs_the_death_path_even_in_creative() {
    let mut harness = Harness::new("p14-kill", ops_for("Chief", 4));
    let (id, mut out) = harness.join("Chief");
    harness.command(id, &mut out, "gamemode creative");
    assert!(harness.game.player(id).expect("player").is_alive());

    // Vanilla's kill bypasses creative invulnerability.
    let lines = harness.command(id, &mut out, "kill");
    assert!(
        !harness.game.player(id).expect("player").is_alive(),
        "creative does not save you from /kill"
    );
    assert!(
        lines.iter().any(|line| line.contains("Killed Chief")),
        "saw {lines:?}"
    );
    assert!(
        lines.iter().any(|line| line.contains("You died!")),
        "the death path runs its message, saw {lines:?}"
    );

    // Killing the dead reports it rather than double-dropping.
    let lines = harness.command(id, &mut out, "kill");
    assert!(
        lines.iter().any(|line| line.contains("already dead")),
        "saw {lines:?}"
    );
}

#[test]
fn seed_reports_the_simulation_seed() {
    let mut harness = Harness::new("p14-seed", ops_for("Chief", 4));
    let (id, mut out) = harness.join("Chief");
    let lines = harness.command(id, &mut out, "seed");
    assert!(
        lines
            .iter()
            .any(|line| line.contains(&harness.game.random_seed().to_string())),
        "the reply carries the seed, saw {lines:?}"
    );
}

#[test]
fn difficulty_queries_sets_and_persists() {
    let mut harness = Harness::new("p14-difficulty", ops_for("Chief", 4));
    let (id, mut out) = harness.join("Chief");

    // Query reports the world default first (whatever LevelDat::new says;
    // the assertion is a round-trip, not a second copy of the default).
    let lines = harness.command(id, &mut out, "difficulty");
    let key = format!("{:?}", harness.game.difficulty());
    assert!(
        lines.iter().any(|line| line.contains(&key)),
        "query must report the default, saw {lines:?}"
    );

    // A bad word changes nothing.
    harness.command(id, &mut out, "difficulty brutal");
    assert_eq!(
        format!("{:?}", harness.game.difficulty()),
        key,
        "a bad word must not change anything"
    );

    // Set persists to level.dat: drop the game but keep the directory alive
    // (TempDir deletes on drop), reopen the same directory, read the file's
    // difficulty back.
    harness.command(id, &mut out, "difficulty peaceful");
    assert_eq!(format!("{:?}", harness.game.difficulty()), "Peaceful");
    let Harness { game, dir, .. } = harness;
    drop(game);
    let path = dir.path().join("world");
    let service = mc_server::storage::WorldService::open(&mc_server::config::StorageConfig {
        world_dir: path,
        autosave_ticks: 0,
    })
    .expect("world reopens");
    assert_eq!(
        format!(
            "{:?}",
            service
                .storage()
                .level()
                .expect("level.dat exists")
                .difficulty
        ),
        "Peaceful",
        "the set must survive a restart through level.dat"
    );
}

#[test]
fn op_grants_live_persists_and_survives_reload() {
    // P14-02: a level-3 granter ops an online player. The grant lands on the
    // live session AND in ops.json; a fresh game on the same directory reads
    // the file and grants without any command.
    let mut harness = Harness::new("p14-op", ops_for("Boss", 3));
    harness
        .game
        .set_ops_directory(harness.dir.path().to_owned());
    let (boss, mut boss_out) = harness.join("Boss");
    let (rookie, _) = harness.join("Rookie");
    assert_eq!(
        harness.game.player_permission(rookie),
        PermissionLevel::All,
        "unlisted to start"
    );

    let lines = harness.command(boss, &mut boss_out, "op Rookie");
    assert_eq!(
        harness.game.player_permission(rookie),
        PermissionLevel::Console,
        "the live session jumps to level 4"
    );
    assert!(
        lines.iter().any(|line| line.contains("Made Rookie")),
        "saw {lines:?}"
    );
    let text = std::fs::read_to_string(harness.dir.path().join("ops.json")).expect("file written");
    let rookie_uuid = mc_network::auth::offline_profile("Rookie").id.to_string();
    assert!(
        text.contains(&rookie_uuid) && text.contains("\"level\": 4"),
        "uuid + level persisted, saw {text}"
    );

    // Re-op is idempotent, not a duplicate.
    let lines = harness.command(boss, &mut boss_out, "op Rookie");
    assert!(
        lines
            .iter()
            .any(|line| line.contains("already an operator")),
        "saw {lines:?}"
    );
    let text = std::fs::read_to_string(harness.dir.path().join("ops.json")).expect("file");
    assert_eq!(text.matches(&rookie_uuid).count(), 1, "one entry, not two");

    // The file alone grants on reload: load it the way the lifecycle does
    // and check the level (the join-from-file path itself is ops_e2e's
    // proven ground, not this test's).
    let Harness { dir, .. } = harness;
    let world_dir = dir.path().join("world");
    let operators = mc_server::ops::OperatorList::load(&mc_server::ops::ops_directory(&world_dir))
        .expect("ops load");
    assert!(operators.is_operator(&rookie_uuid));
    assert_eq!(
        operators.level_for(&rookie_uuid),
        PermissionLevel::Console,
        "the file grant survives the restart"
    );
}

#[test]
fn deop_revokes_live_persists_and_demotes() {
    let mut harness = Harness::new("p14-deop", ops_for("Boss", 4));
    harness
        .game
        .set_ops_directory(harness.dir.path().to_owned());
    let (boss, mut boss_out) = harness.join("Boss");
    let (rookie, _) = harness.join("Rookie");
    harness.command(boss, &mut boss_out, "op Rookie");
    assert_eq!(
        harness.game.player_permission(rookie),
        PermissionLevel::Console
    );

    let lines = harness.command(boss, &mut boss_out, "deop Rookie");
    assert_eq!(
        harness.game.player_permission(rookie),
        PermissionLevel::All,
        "the live session falls back to 0"
    );
    assert!(
        lines
            .iter()
            .any(|line| line.contains("no longer a server operator")),
        "saw {lines:?}"
    );
    let text = std::fs::read_to_string(harness.dir.path().join("ops.json")).expect("file written");
    let rookie_uuid = mc_network::auth::offline_profile("Rookie").id.to_string();
    assert!(
        !text.contains(&rookie_uuid),
        "the entry is gone, saw {text}"
    );

    // Revoking nobody changes nothing and says so.
    let lines = harness.command(boss, &mut boss_out, "deop Rookie");
    assert!(
        lines.iter().any(|line| line.contains("is not an operator")),
        "saw {lines:?}"
    );
}

#[test]
fn op_unknown_player_and_level_two_grant_are_refused() {
    let mut harness = Harness::new("p14-op-refuse", ops_for("Boss", 4));
    harness
        .game
        .set_ops_directory(harness.dir.path().to_owned());
    let (boss, mut boss_out) = harness.join("Boss");

    // Nobody by that name is online: nothing is written anywhere.
    let lines = harness.command(boss, &mut boss_out, "op Ghost");
    assert!(
        lines
            .iter()
            .any(|line| line.contains("only online players")),
        "saw {lines:?}"
    );
    assert!(
        !harness.dir.path().join("ops.json").exists(),
        "no file must appear for a refusal"
    );

    // Level 2 can no longer grant (P14-02 raised /op to 3: a level-2 holder
    // minting level-4 operators was a privilege escalation Vanilla's ladder
    // does not allow).
    let (moderat, mut mod_out) = harness.join("Moderator");
    assert_eq!(
        harness.game.player_permission(moderat),
        PermissionLevel::All
    );
    // Promote Moderator to exactly 2 through the file path is overkill;
    // instead grant via Boss then demote Boss? Simpler: a second listing.
    // Here the denial is proven the other way: a plain player cannot op.
    let lines = harness.command(moderat, &mut mod_out, "op Boss");
    assert!(
        lines
            .iter()
            .any(|line| line.contains("do not have permission")),
        "level 0 cannot op, saw {lines:?}"
    );
    assert!(
        !harness.dir.path().join("ops.json").exists(),
        "a denied grant writes nothing"
    );
}

#[test]
fn level_two_holder_cannot_op_after_the_raise() {
    // Direct ladder probe: a level-2 (Operator) session is denied /op and
    // /deop, which now require Administrator(3).
    let mut harness = Harness::new("p14-op-ladder", ops_for("Mod", 2));
    harness
        .game
        .set_ops_directory(harness.dir.path().to_owned());
    let (moderat, mut mod_out) = harness.join("Mod");
    assert_eq!(
        harness.game.player_permission(moderat),
        PermissionLevel::Operator
    );
    let (rookie, _) = harness.join("Rookie");

    let lines = harness.command(moderat, &mut mod_out, "op Rookie");
    assert!(
        lines
            .iter()
            .any(|line| line.contains("do not have permission")),
        "level 2 must be denied /op, saw {lines:?}"
    );
    assert_eq!(
        harness.game.player_permission(rookie),
        PermissionLevel::All,
        "nothing was granted"
    );
    let lines = harness.command(moderat, &mut mod_out, "deop Mod");
    assert!(
        lines
            .iter()
            .any(|line| line.contains("do not have permission")),
        "level 2 must be denied /deop, saw {lines:?}"
    );
    assert!(
        !harness.dir.path().join("ops.json").exists(),
        "denied grants write nothing"
    );
}

#[test]
fn op_without_an_ops_directory_reports_cannot_persist() {
    // No lifecycle ran, so no directory was ever set: the honest fallback is
    // a message, not a grant and not a failure.
    let mut harness = Harness::new("p14-op-nodir", ops_for("Boss", 4));
    let (boss, mut boss_out) = harness.join("Boss");
    let (rookie, _) = harness.join("Rookie");
    let lines = harness.command(boss, &mut boss_out, "op Rookie");
    assert!(
        lines.iter().any(|line| line.contains("cannot persist")),
        "saw {lines:?}"
    );
    assert_eq!(
        harness.game.player_permission(rookie),
        PermissionLevel::All,
        "nothing was granted without a directory"
    );
    assert!(
        !harness
            .game
            .operators()
            .is_operator(&mc_network::auth::offline_profile("Rookie").id.to_string()),
        "the in-memory list is untouched too"
    );
}

#[test]
fn peaceful_night_spawns_no_hostiles() {
    // The one runtime consumer of difficulty: the monster gate. Night +
    // 300 cycles spawns hostiles on the control seed (natural_spawn proves
    // it); peaceful must spawn none, deterministically — the gate blocks
    // every attempt rather than thinning them.
    let mut harness = Harness::new("p14-peaceful", ops_for("Chief", 4));
    harness.game.set_time_offset(15_000);
    let (id, mut out) = harness.join("Chief");
    harness.command(id, &mut out, "difficulty peaceful");
    harness.run(300);
    let hostiles = harness
        .game
        .mobs()
        .iter()
        .filter(|(kind, _)| {
            matches!(
                kind,
                MobKind::Zombie | MobKind::Skeleton | MobKind::Spider | MobKind::Creeper
            )
        })
        .count();
    assert_eq!(hostiles, 0, "peaceful night spawns no hostiles");
}

#[test]
fn locked_difficulty_refuses_the_set() {
    // A `level.dat` with the lock set refuses `/difficulty` with a reason;
    // the field is untouched. The lock lives in the file, so the test locks
    // the file before the game reads it.
    use mc_server::storage::WorldService;
    let dir = mc_test_support::fixtures::TempDir::new("p14-locked");
    let config = mc_server::config::StorageConfig {
        world_dir: dir.path().join("world"),
        autosave_ticks: 0,
    };
    let mut service = WorldService::open(&config).expect("world opens");
    let mut level = service
        .storage()
        .level()
        .expect("fresh world has a level")
        .clone();
    level.difficulty_locked = true;
    service.storage_mut().save_level(level).expect("locks");

    let (tx, rx) = mc_network::bridge::game_channel(256);
    let mut game = Game::build_with_operators(None, Some(service), 3, rx, 7, ops_for("Boss", 4))
        .expect("game builds");
    assert!(game.difficulty_locked(), "the lock reads back");
    let before = format!("{:?}", game.difficulty());

    let ids = mc_network::bridge::ConnectionIds::new();
    let id = ids.next_id();
    let (outbound, mut out) = mc_network::bridge::OutboundSender::pair(id, 8192);
    tx.try_send(ClientEvent {
        id,
        kind: ClientEventKind::Joined {
            profile: mc_network::auth::offline_profile("Boss"),
            outbound,
        },
    })
    .expect("join queued");
    game.tick().expect("tick");

    let mut report = TickReport::default();
    game.dispatch_command(id, "difficulty hard", &mut report)
        .expect("a command is answered");
    let mut lines = Vec::new();
    while let Some(raw) = out.try_recv() {
        if raw.id == mc_protocol::ids::clientbound::play::DISGUISED_CHAT {
            use mc_protocol::packets::Packet;
            use mc_protocol::packets::play::SystemChat;
            match SystemChat::decode(&raw.payload) {
                Ok(chat) => lines.push(chat.content.as_plain().to_owned()),
                Err(error) => panic!("a disguised_chat must decode: {error}"),
            }
        }
    }
    assert!(
        lines.iter().any(|line| line.contains("locked")),
        "the lock must refuse with a reason, saw {lines:?}"
    );
    assert_eq!(
        format!("{:?}", game.difficulty()),
        before,
        "a refused set changes nothing"
    );
}

#[test]
fn op_rolls_back_when_the_file_write_fails() {
    // Point the ops directory at a *file*: `create_dir_all` fails, the save
    // fails, and the grant must roll back — memory and file never disagree,
    // and the player is told exactly that.
    let mut harness = Harness::new("p14-op-rollback", ops_for("Boss", 4));
    let blocker = harness.dir.path().join("not-a-directory");
    std::fs::write(&blocker, "in the way").expect("blocker written");
    harness.game.set_ops_directory(blocker);
    let (boss, mut boss_out) = harness.join("Boss");
    let (rookie, _) = harness.join("Rookie");

    let lines = harness.command(boss, &mut boss_out, "op Rookie");
    assert!(
        lines
            .iter()
            .any(|line| line.contains("nothing was changed")),
        "the failure must say nothing was applied, saw {lines:?}"
    );
    assert_eq!(
        harness.game.player_permission(rookie),
        PermissionLevel::All,
        "a failed save grants nothing live"
    );
    assert!(
        !harness
            .game
            .operators()
            .is_operator(&mc_network::auth::offline_profile("Rookie").id.to_string()),
        "and nothing lingers in memory either"
    );
}

#[test]
fn effect_give_reaches_the_hud_and_clear_removes_it() {
    // P16-03: the operator-visible source. Giving poison announces
    // update_mob_effect (132) with the wire id; clearing announces
    // remove_mob_effect (78); the server state matches both packets.
    let mut harness = Harness::new("p16-effect", ops_for("Chief", 4));
    let (id, mut out) = harness.join("Chief");
    let lines = harness.command(id, &mut out, "effect give Chief minecraft:poison 60");
    assert!(
        lines.iter().any(|line| line.contains("poison")),
        "the give confirms, saw {lines:?}"
    );
    let player = harness.game.player(id).expect("player");
    let effect = player.effects.get(&19).expect("poison stored");
    assert_eq!((effect.amplifier, effect.duration), (0, 1200));
    // The icon packet itself: re-run the give on a fresh drain and read the
    // raw id, because `command()` consumes non-chat packets while draining.
    let mut report = TickReport::default();
    harness
        .game
        .dispatch_command(id, "effect give Chief minecraft:poison 60", &mut report)
        .expect("answered");
    let mut updates = 0;
    while let Some(raw) = out.try_recv() {
        if raw.id == clientbound::play::UPDATE_MOB_EFFECT {
            updates += 1;
            let body =
                mc_protocol::packets::play::UpdateMobEffect::decode(&raw.payload).expect("decodes");
            assert_eq!(
                (body.effect_id, body.amplifier, body.duration),
                (18, 0, 1200),
                "the HUD packet carries the raw registry id (stored 19 minus the legacy one)"
            );
        }
    }
    assert_eq!(updates, 1, "exactly one icon packet");

    let mut report = TickReport::default();
    harness
        .game
        .dispatch_command(id, "effect clear Chief minecraft:poison", &mut report)
        .expect("answered");
    assert!(
        harness.game.player(id).expect("player").effects.is_empty(),
        "cleared server-side"
    );
    let mut removals = 0;
    while let Some(raw) = out.try_recv() {
        if raw.id == clientbound::play::REMOVE_MOB_EFFECT {
            removals += 1;
            let body =
                mc_protocol::packets::play::RemoveMobEffect::decode(&raw.payload).expect("decodes");
            assert_eq!(
                body.effect_id, 18,
                "the removal names the same raw id as the icon"
            );
        }
    }
    assert_eq!(removals, 1, "exactly one removal packet");
}

#[test]
fn effect_give_accepts_the_self_selector() {
    // Owner session: the server refused `@s` as a stranger everywhere a
    // target is required. `@s` names the invoking player; anything wider
    // still needs a selector engine this build does not have.
    let mut harness = Harness::new("p16-effect-self", ops_for("Chief", 4));
    let (id, mut out) = harness.join("Chief");
    let lines = harness.command(id, &mut out, "effect give @s minecraft:poison 60");
    assert!(
        lines.iter().any(|line| line.contains("poison")),
        "the give confirms, saw {lines:?}"
    );
    assert!(
        harness
            .game
            .player(id)
            .expect("player")
            .effects
            .contains_key(&19),
        "poison stored on the invoker"
    );
}

#[test]
fn effect_refuses_unknown_names_and_strangers() {
    // P16-03: unmodelled effects are refused with the modelled list, and
    // targeting anyone but self is refused like give/kill.
    let mut harness = Harness::new("p16-effect-refuse", ops_for("Chief", 4));
    let (id, mut out) = harness.join("Chief");
    let lines = harness.command(id, &mut out, "effect give Chief minecraft:jump_boost");
    assert!(
        lines.iter().any(|line| line.contains("Unknown effect")),
        "jump_boost is storable but has no behaviour here, saw {lines:?}"
    );
    assert!(
        harness.game.player(id).expect("player").effects.is_empty(),
        "refused effects store nothing"
    );
    let lines = harness.command(id, &mut out, "effect give Rookie minecraft:poison");
    assert!(
        lines.iter().any(|line| line.contains("invoking player")),
        "cross-player targeting refused, saw {lines:?}"
    );
    let lines = harness.command(id, &mut out, "effect frobnicate");
    assert!(
        lines.iter().any(|line| line.contains("Usage")),
        "unknown verbs get usage, saw {lines:?}"
    );
}
