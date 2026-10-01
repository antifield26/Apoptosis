//! Gameplay properties end to end (P19-06).
//!
//! Config + Game plumbing: every key reads, writes and round-trips, with
//! enforcement P20-owned per key (named in `GameplayConfig` docs, not stubbed
//! here) — except `default_gamemode`, which is live at join and pinned below
//! (AUDIT-19 A-07). The boot path installs the config values into the live
//! game.
//!
//! Falsification shape: drop a field from the install call and the boot
//! test finds the default where the config value belongs; break a setter
//! and its read-back goes red.

use mc_entity::GameMode;
use mc_network::bridge::{
    ClientEvent, ClientEventKind, ConnectionId, InboundReceiver, OutboundSender, game_channel,
};
use mc_server::config::{DefaultGameMode, GameplayConfig};
use mc_server::game::Game;
use mc_server::lifecycle::Server;
use mc_server::ops::OperatorList;
use mc_server::storage::WorldService;
use mc_test_support::fixtures::TempDir;

fn bare_game() -> Game {
    let (_tx, rx) = game_channel(64);
    Game::build_with_operators(
        None,
        None,
        4,
        rx,
        mc_server::game::DEFAULT_RANDOM_SEED,
        mc_server::ops::OperatorList::new(),
    )
    .expect("game builds")
}

/// A game a test can join sessions into (AUDIT-19 A-07).
///
/// The two legs of the pin need different storage: one is a world directory,
/// where a **restart** re-reads the player from `playerdata/<uuid>.dat`, and
/// the other is a game with none, where the in-memory copy is the whole story.
/// The key has to lose to both.
struct Sessions {
    game: Game,
    events: tokio::sync::mpsc::Sender<ClientEvent>,
    /// Kept alive: a dropped receiver turns every join packet into a send
    /// error and the reason for a failure would be the harness, not the code.
    inboxes: Vec<InboundReceiver>,
}

impl Sessions {
    fn open(world_dir: &std::path::Path, default: DefaultGameMode) -> Self {
        let config = mc_server::config::StorageConfig {
            world_dir: world_dir.to_path_buf(),
            autosave_ticks: 0,
            seed: None,
        };
        let service = WorldService::open(&config).expect("world opens");
        let (tx, rx) = game_channel(256);
        let mut game = Game::build_with_operators(
            None,
            Some(service),
            4,
            rx,
            mc_server::game::DEFAULT_RANDOM_SEED,
            OperatorList::new(),
        )
        .expect("game builds");
        game.set_default_gamemode(default);
        Self {
            game,
            events: tx,
            inboxes: Vec::new(),
        }
    }

    fn bare(default: DefaultGameMode) -> Self {
        let (tx, rx) = game_channel(256);
        let mut game = Game::build_with_operators(
            None,
            None,
            4,
            rx,
            mc_server::game::DEFAULT_RANDOM_SEED,
            OperatorList::new(),
        )
        .expect("game builds");
        game.set_default_gamemode(default);
        Self {
            game,
            events: tx,
            inboxes: Vec::new(),
        }
    }

    fn join(&mut self, id: ConnectionId, name: &str) {
        let (outbound, out) = OutboundSender::pair(id, 8192);
        self.inboxes.push(out);
        self.events
            .try_send(ClientEvent {
                id,
                kind: ClientEventKind::Joined {
                    profile: mc_network::auth::offline_profile(name),
                    outbound,
                },
            })
            .expect("join queued");
        self.game.tick().expect("tick");
    }

    /// Disconnect: the in-memory copy is stored and, with a world directory,
    /// `playerdata/<uuid>.dat` is written.
    fn leave(&mut self, id: ConnectionId) {
        self.events
            .try_send(ClientEvent {
                id,
                kind: ClientEventKind::Left,
            })
            .expect("leave queued");
        self.game.tick().expect("tick");
    }

    fn mode(&self, id: ConnectionId) -> GameMode {
        self.game.player(id).expect("player").game_mode
    }
}

#[test]
fn fresh_game_holds_the_documented_defaults() {
    let game = bare_game();
    assert_eq!(game.spawn_protection(), 16);
    assert!(game.pvp());
    assert_eq!(game.idle_timeout_minutes(), 0);
    assert_eq!(game.simulation_distance(), 8);
    assert_eq!(game.default_gamemode(), DefaultGameMode::Survival);
    assert!(!game.hide_online_players());
}

#[test]
fn every_key_reads_back_what_was_written() {
    let mut game = bare_game();
    game.set_spawn_protection(0);
    game.set_pvp(false);
    game.set_idle_timeout_minutes(30);
    game.set_simulation_distance(10);
    game.set_default_gamemode(DefaultGameMode::Creative);
    game.set_hide_online_players(true);
    assert_eq!(game.spawn_protection(), 0);
    assert!(!game.pvp());
    assert_eq!(game.idle_timeout_minutes(), 30);
    assert_eq!(game.simulation_distance(), 10);
    assert_eq!(game.default_gamemode(), DefaultGameMode::Creative);
    assert!(game.hide_online_players());
    // Whole-struct install is what the boot uses.
    game.set_gameplay_config(GameplayConfig::default());
    assert_eq!(game.spawn_protection(), 16);
    assert_eq!(game.gameplay().simulation_distance, 8);
}

#[test]
fn boot_installs_the_config_values() {
    let dir = TempDir::new("gameplay-boot");
    let mut config = mc_server::config::ServerConfig::default();
    config.storage.world_dir = dir.path().join("world");
    config.storage.autosave_ticks = 0;
    config.gameplay.spawn_protection = 0;
    config.gameplay.pvp = false;
    config.gameplay.simulation_distance = 10;
    config.gameplay.default_gamemode = DefaultGameMode::Adventure;
    let mut server = Server::new(config);
    server.open_world().expect("world opens");
    let game = server.game().expect("game built");
    assert_eq!(game.spawn_protection(), 0);
    assert!(!game.pvp());
    assert_eq!(game.simulation_distance(), 10);
    assert_eq!(game.default_gamemode(), DefaultGameMode::Adventure);
}

// ------------------------------------------------------- AUDIT-19 A-07 pins
//
// `[gameplay] default_gamemode` was installed (parsed, validated, stored, read
// back) and had **zero production readers**: join hardcoded
// `GameMode::Survival`, so the key was inert and `/defaultgamemode` does not
// exist. It is live now, for exactly one case — a player with no stored
// `playerdata` file and no in-memory state to restore. The two tests below pin
// both halves of that sentence: the key binds a *first* join, and it loses to
// any returning player's own mode.

/// The key binds a genuinely new player, and it is read rather than assumed.
#[test]
fn a_first_join_takes_the_configured_default_gamemode() {
    let dir = TempDir::new("gameplay-default-mode-first");
    let mut creative = Sessions::open(&dir.path().join("world"), DefaultGameMode::Creative);
    creative.join(ConnectionId(1), "Newcomer");
    assert_eq!(
        creative.mode(ConnectionId(1)),
        GameMode::Creative,
        "a join with no stored playerdata takes the configured mode, not Survival"
    );

    // A second game, configured differently: a hardcoded mode would pass the
    // assertion above only by coincidence and cannot pass both.
    let mut spectator = Sessions::bare(DefaultGameMode::Spectator);
    spectator.join(ConnectionId(1), "Newcomer");
    assert_eq!(
        spectator.mode(ConnectionId(1)),
        GameMode::Spectator,
        "and the mode follows the key"
    );
}

/// A returning player's own game mode wins — the stored file across a
/// restart, and the remembered copy inside one run.
#[test]
fn a_returning_players_own_gamemode_beats_the_default_gamemode() {
    // (a) The stored file. The first game is dropped before the second opens,
    // so only `playerdata/<uuid>.dat` can carry Adventure across; the second
    // game's default is Spectator, so "which one won" cannot be a
    // coincidence.
    let dir = TempDir::new("gameplay-default-mode-file");
    let world = dir.path().join("world");
    {
        let mut first = Sessions::open(&world, DefaultGameMode::Creative);
        first.join(ConnectionId(1), "Returner");
        assert_eq!(first.mode(ConnectionId(1)), GameMode::Creative);
        first
            .game
            .player_mut(ConnectionId(1))
            .expect("player")
            .game_mode = GameMode::Adventure;
        first.leave(ConnectionId(1));
    }
    let mut second = Sessions::open(&world, DefaultGameMode::Spectator);
    second.join(ConnectionId(1), "Returner");
    assert_eq!(
        second.mode(ConnectionId(1)),
        GameMode::Adventure,
        "a stored playerdata file's own mode outranks the key"
    );

    // (b) The in-memory copy, for a game with no world directory to write to:
    // a reconnect must not be re-defaulted either.
    let mut bare = Sessions::bare(DefaultGameMode::Creative);
    bare.join(ConnectionId(1), "Returner");
    assert_eq!(bare.mode(ConnectionId(1)), GameMode::Creative);
    bare.game
        .player_mut(ConnectionId(1))
        .expect("player")
        .game_mode = GameMode::Adventure;
    bare.leave(ConnectionId(1));
    bare.game.set_default_gamemode(DefaultGameMode::Spectator);
    bare.join(ConnectionId(2), "Returner");
    assert_eq!(
        bare.mode(ConnectionId(2)),
        GameMode::Adventure,
        "the remembered mode outranks the key too"
    );
}
