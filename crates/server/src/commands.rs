//! The server's command set (P07-05, P14-01, P18-02, P19-01).
//!
//! The P18-02 **closed list** — anything outside it stays refused by name:
//!
//! | Command | Argument shape it exercises | What it does |
//! |---|---|---|
//! | `help` | none | lists what the source may use |
//! | `list` | none | names the online players |
//! | `say` | greedy string | broadcasts a message |
//! | `time` | optional ranged integer | queries or sets the world time |
//! | `tp` / `teleport` | word + block position + optional rotation | moves the source |
//! | `op` | player name, administrator-only | grants operator status at level 4 and persists `ops.json` |
//! | `deop` | player name, administrator-only | revokes operator status and persists `ops.json` |
//! | `whitelist` | action + optional player, administrator-only | manages the login whitelist: `on`/`off` toggles enforcement live, `add`/`remove` persist `whitelist.json`, `list` names entries, `reload` re-reads the file |
//! | `ban` | player + optional greedy reason, administrator-only | bans a profile (offline-resolvable) and disconnects their live session, persisting `banned-players.json`; no reason files `DEFAULT_BAN_REASON`, Vanilla's optional-reason shape |
//! | `ban-ip` | address + greedy reason, administrator-only | bans an address and disconnects every live session holding it, persisting `banned-ips.json` |
//! | `pardon` | player, administrator-only | removes a profile ban row, persisting `banned-players.json` |
//! | `pardon-ip` | address, administrator-only | removes an address ban row, persisting `banned-ips.json` |
//! | `banlist` | optional kind, administrator-only | names banned profiles or addresses |
//! | `kick` | player + greedy reason, administrator-only | disconnects a live session with a reason; writes nothing |
//! | `stop` | none, console-only | asks the server to shut down |
//! | `save-all` | optional `flush`, operator-only | saves the world now, even with automatic saving held; `flush` is the only mode and names what the call does (push + sync before replying) |
//! | `save-off` | none, operator-only | holds automatic (autosave-tick) writes; dirty flags are kept, explicit and shutdown saves still write |
//! | `save-on` | none, operator-only | resumes automatic writes |
//! | `gamemode` | word + optional player name, operator-only | sets the invoking player's game mode |
//! | `give` | player name + resource + optional ranged integer, operator-only | gives items, dropping overflow at the player's feet |
//! | `kill` | optional player name, operator-only | kills the invoking player through the damage path, even in creative |
//! | `seed` | none, operator-only | reports the world seed |
//! | `difficulty` | optional word, operator-only | queries or sets the world difficulty |
//! | `clear` | optional target/item/count, operator-only | removes matching items from the invoking player |
//! | `xp` / `experience` | action + amount + optional unit, operator-only | adds, sets or queries experience |
//! | `enchant` | enchantment + optional level, operator-only | stores an enchantment component on the held item; Efficiency/Sharpness/Protection/Unbreaking apply (P18-01b), the rest stay inert |
//! | `setblock` | block position + block + optional mode, operator-only | sets one block (`replace`/`destroy`/`keep`) |
//! | `fill` | two positions + block + optional mode, operator-only | fills a volume, capped at [`MAX_FILL_VOLUME`] |
//! | `summon` | entity kind + optional position, operator-only | spawns one modelled mob |
//! | `setworldspawn` | optional position, operator-only | sets the world spawn |
//! | `msg` / `tell` / `w` | target + greedy message | whispers to one player |
//! | `me` | greedy action | broadcasts a narrative line |
//!
//! ## What each command deliberately does *not* do
//!
//! Vanilla's versions are larger, and the difference is stated rather than glossed:
//!
//! - **`help`** takes no page argument and does not paginate. Vanilla's does both.
//! - **`list`** does not report the maximum player count in Vanilla's exact format.
//! - **`say`** broadcasts to players only. The server console sees it in the log.
//! - **`time`** sets the overworld clock but not the day counter, so it does
//!   not advance the date the way Vanilla's `time set` does; `add` is not
//!   modelled. `set` takes an integer or a preset
//!   (`day`/`noon`/`night`/`midnight`); a bare integer still sets for
//!   back-compat. Anything else prints usage — it never answers the time,
//!   because a silent query once hid failed sets (P14-09 walk).
//! - **`tp`/`teleport`** move the *invoking* player, not a named target, because
//!   the server has no cross-player teleport authority model yet; the `target`
//!   argument is validated and must name the source itself. Optional `yaw`/`pitch`
//!   are stored on the source but not yet applied to the player's rotation
//!   (the client owns its camera). That is a real limitation, not a stub.
//! - **`op`** grants level 4 (Vanilla's default `op-permission-level`) to an
//!   *online* player and writes `ops.json` beside the world, taking effect
//!   immediately and surviving restarts; only online players can be named
//!   (uuids come from sessions, never from name matching). Both `/op` and
//!   `/deop` require level 3 like Vanilla — `/op` used to sit at level 2,
//!   which let a level-2 holder mint level-4 operators.
//! - **`deop`** removes the entry (and demotes a live session to 0);
//!   revoking someone who was never listed changes nothing and says so.
//!   A failed file write rolls the in-memory change back and says that
//!   instead of claiming success.
//! - **`stop`** sets the shutdown flag; it does not save first, because the lifecycle's
//!   shutdown path already saves after the network drains (ADR-0001 D-04).
//! - **`gamemode`** takes the invoking player only (same authority reason as `tp`);
//!   it sets the mode field and the menu's creative flag the way join and respawn do,
//!   but broadcasts no player-info or abilities update, so other clients learn of it
//!   late. Mode words are Vanilla's full names plus the `s`/`c`/`a`/`sp` shortcuts.
//! - **`give`** takes the invoking player only; the count caps at one stack (64) and
//!   the remainder of a full inventory drops at the player's feet, which is Vanilla's
//!   rule. Item names are registry ids (`minecraft:stone`, bare `stone` works).
//! - **`kill`** takes the invoking player only; it bypasses creative invulnerability
//!   the way Vanilla's kill does, and drops run through the normal death path.
//! - **`seed`** reports the simulation seed, which is also the worldgen seed.
//! - **`difficulty`** sets the world's difficulty (persisted to `level.dat` when
//!   storage is present) and gates hostile monster spawns on peaceful; mob damage
//!   numbers stay the hardcoded Normal values, and a locked `level.dat` refuses
//!   the set. Words are Vanilla's four full names, case-sensitive like Vanilla.
//! - **`clear`** targets the invoking player only. `maxCount` of `0` is Vanilla's
//!   "count only" form and reports without removing.
//! - **`xp`/`experience`** accept `add|set|query` with `levels` (default) or `points`.
//!   Only the invoking player is affected.
//! - **`enchant`** stores a `minecraft:enchantments` component on the held stack.
//!   Efficiency, Sharpness, Protection and Unbreaking **apply** (P18-01b);
//!   every other name is stored and inert, and each is a named gap in the
//!   parity matrix.
//! - **`setblock`/`fill`** write the world's block store and rely on the next tick's
//!   `broadcast_block_changes`. `fill` refuses a volume above [`MAX_FILL_VOLUME`]
//!   (named red: `fill_volume_over_the_named_cap_is_refused`). Modes are
//!   `replace`/`keep`/`destroy` only; `hollow`/`outline`/`filtered` are refused by name.
//! - **`summon`** accepts only the eight [`mc_entity::mob::MobKind`] names (with or
//!   without the `minecraft:` prefix). Everything else is refused by name.
//! - **`setworldspawn`** writes the in-memory spawn and `level.dat` when storage is
//!   present. It does not move players who are already online.
//! - **`msg`/`tell`/`w`** deliver to one online player. Offline targets are refused.
//! - **`me`** broadcasts `* name action` to every player, like Vanilla's emote.
//!
//! Every one of these is a line in the parity matrix (the Phase 07 report is in git history, tag `phase-09-final`).

use mc_command::source::{CommandSource, SourceKind};
use mc_core::error::ServerResult;
use mc_persistence::chunk::ChunkPos;
use mc_protocol::packets::play::SetTime;
use mc_protocol::text::TextComponent;
use tracing::{debug, info, warn};

use crate::game::{Game, TickReport};

/// Vanilla's `/fill` region ceiling (`maxBlockModifications`, also the figure
/// `clone` and `fillbiome` hard-code). A named constant rather than a literal
/// so the handler, the parity matrix and
/// `fill_volume_over_the_named_cap_is_refused` all cite the same number.
pub const MAX_FILL_VOLUME: i64 = 32_768;

/// Longest command string the dispatcher accepts, re-exported from
/// [`mc_command`].
///
/// It lives there; it is named here because the console reader in the binary
/// bounds a *line* against it (AUDIT-19 G-11) and the binary depends on
/// `mc-server`, not on `mc-command`.
pub const MAX_COMMAND_CHARS: usize = mc_command::MAX_COMMAND_CHARS;

/// Inclusive volume of a `/fill` region from its sorted bounds.
fn fill_volume(min_x: i32, max_x: i32, min_y: i32, max_y: i32, min_z: i32, max_z: i32) -> i64 {
    i64::from(max_x - min_x + 1) * i64::from(max_y - min_y + 1) * i64::from(max_z - min_z + 1)
}

/// Whether a `/fill` volume is within the cap (E-15: the boundary itself is
/// pinned here so the at-cap case does not need a 32 768-block debug fill).
fn fill_volume_accepted(volume: i64) -> bool {
    volume <= MAX_FILL_VOLUME
}

/// Chunks a `/fill` region spans, horizontally (AUDIT-19 A-09).
///
/// `/fill` reads (or generates) the chunk under **every** cell before writing
/// it — the load-before-write ordering P19-08 installed, without which a fill
/// into an unloaded chunk overwrites the stored terrain with an all-air
/// placeholder. The volume cap does not bound that cost: a `32768×1×1` bar is
/// 32 768 blocks and **2048 distinct chunks**, all read on the tick thread
/// while `persist.rs` claims reads stop at [`CHUNKS_PER_TICK`] per tick.
/// Only X/Z matter: a column lives in one chunk however tall it is.
fn fill_chunk_span(min_x: i32, max_x: i32, min_z: i32, max_z: i32) -> i64 {
    let width = i64::from((max_x >> 4) - (min_x >> 4) + 1);
    let depth = i64::from((max_z >> 4) - (min_z >> 4) + 1);
    width * depth
}

/// The `/fill` chunk budget: the per-tick chunk budget
/// ([`CHUNKS_PER_TICK`]), because a command runs inside one tick.
///
/// A region spanning more chunks than a tick is allowed to read is refused by
/// name rather than silently spending the whole tick budget (or more) in one
/// command. Vanilla bounds `/fill` by blocks alone and pays the chunk loads in
/// the same tick; this server states the limit instead of hiding it.
#[allow(
    clippy::cast_possible_wrap,
    reason = "the budget is the compile-time `CHUNKS_PER_TICK` (64); a wrap needs a usize wider than i64"
)]
pub const MAX_FILL_CHUNKS: i64 = crate::game::CHUNKS_PER_TICK as i64;

/// Whether a `/fill` region's chunk span is within the budget.
fn fill_chunk_span_accepted(chunks: i64) -> bool {
    chunks <= MAX_FILL_CHUNKS
}

/// Enchantment names this build stores on a held stack (P18-02 store + P18-01b
/// effects for four of them).
///
/// Ids are the 26.1 registry order (pumpkin-data `Enchantment::from_id`).
/// Efficiency/Sharpness/Protection/Unbreaking change behaviour (P18-01b); the
/// rest stay stored and inert, and each is a named gap in the parity matrix.
pub const MODELLED_ENCHANTMENTS: &[(&str, i32, i32)] = &[
    ("aqua_affinity", 0, 1),
    ("bane_of_arthropods", 1, 5),
    ("binding_curse", 2, 1),
    ("blast_protection", 3, 4),
    ("breach", 4, 4),
    ("channeling", 5, 1),
    ("density", 6, 5),
    ("depth_strider", 7, 3),
    ("efficiency", 8, 5),
    ("feather_falling", 9, 4),
    ("fire_aspect", 10, 2),
    ("fire_protection", 11, 4),
    ("flame", 12, 1),
    ("fortune", 13, 3),
    ("frost_walker", 14, 2),
    ("impaling", 15, 5),
    ("infinity", 16, 1),
    ("knockback", 17, 2),
    ("looting", 18, 3),
    ("loyalty", 19, 3),
    ("luck_of_the_sea", 20, 3),
    ("lunge", 21, 5),
    ("lure", 22, 3),
    ("mending", 23, 1),
    ("multishot", 24, 1),
    ("piercing", 25, 4),
    ("power", 26, 5),
    ("projectile_protection", 27, 4),
    ("protection", 28, 4),
    ("punch", 29, 2),
    ("quick_charge", 30, 3),
    ("respiration", 31, 3),
    ("riptide", 32, 3),
    ("sharpness", 33, 5),
    ("silk_touch", 34, 1),
    ("smite", 35, 5),
    ("soul_speed", 36, 3),
    ("sweeping_edge", 37, 3),
    ("swift_sneak", 38, 3),
    ("thorns", 39, 3),
    ("unbreaking", 40, 3),
    ("vanishing_curse", 41, 1),
    ("wind_burst", 42, 3),
];

/// The result of running a command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandResult {
    /// The command ran; the message goes to the source (or to everyone, for `say`).
    Ok {
        /// What to tell the source, if anything.
        feedback: Option<String>,
    },
    /// The command ran and wants the server to stop.
    Stop,
}

impl CommandResult {
    /// A command that reports something.
    #[must_use]
    pub fn message(text: impl Into<String>) -> Self {
        Self::Ok {
            feedback: Some(text.into()),
        }
    }

    /// A command that reports nothing.
    #[must_use]
    pub const fn silent() -> Self {
        Self::Ok { feedback: None }
    }
}

impl Game {
    /// Whether a command target names the invoking player: their own name,
    /// or `@s` (vanilla's self selector — what the client sends when the
    /// owner types `@s`, which used to be refused as a stranger).
    /// Anything else needs a selector engine or cross-player authority
    /// this build does not have, and each caller refuses it with its own
    /// reason.
    fn targets_self(target: &str, source: &str) -> bool {
        target == "@s" || target.eq_ignore_ascii_case(source)
    }

    /// Build the command tree this server supports.
    ///
    /// A free method rather than a `const`, because `CommandTree::insert` is fallible and
    /// a failure here should be impossible — the tests assert the tree builds.
    ///
    /// # Panics
    ///
    /// Never in practice: every command below is valid by construction, and
    /// `command_tree_is_valid` asserts it. Building it at startup rather than lazily
    /// means a malformed tree would stop the server instead of a player's command.
    #[allow(
        clippy::too_many_lines,
        reason = "the tree is a data table; splitting it would hide the closed list"
    )]
    #[must_use]
    pub fn build_command_tree() -> mc_command::CommandTree {
        use mc_command::{Argument, ArgumentKind, Command, PermissionLevel, ValueRange};

        let mut tree = mc_command::CommandTree::new();
        // The `expect` is a startup invariant with a test behind it, which is the one
        // place this project allows one outside a test: a server that cannot describe
        // its own commands is not fit to run.
        let mut add = |command: Command| {
            tree.insert(command)
                .expect("the built-in command tree is valid; see command_tree_is_valid");
        };

        add(Command::new("help", "List the commands you can use"));
        add(Command::new("list", "List the players online"));
        add(Command::new("say", "Broadcast a message").with_argument(Argument::greedy("message")));
        // Vanilla gates `/time` at level 2 (`ServerLevel`'s clock is not a
        // player-facing control), so it is operator-only here (AUDIT-19 G-09;
        // it was reachable at level 0 and is named in the P07 verdict).
        add(Command::new("time", "Query or set the world time")
            .with_argument(Argument::optional("action", ArgumentKind::Word))
            .with_argument(Argument::optional("value", ArgumentKind::Word))
            .requiring(PermissionLevel::Operator));
        // Vanilla gates `/weather` at level 2 as well (`Commands.LEVEL_GAMEMASTERS`,
        // jar `WeatherCommand.register`), so it is operator-only here. Duration
        // is plain integer ticks (Vanilla's `TimeArgument` also takes `1d`
        // suffixes — a named gap); absent means "sample the duration provider",
        // never a fixed 6000 (jar `getDuration`).
        add(Command::new("weather", "Set the weather")
            .with_argument(Argument::optional("kind", ArgumentKind::Word))
            .with_argument(Argument::optional("duration", ArgumentKind::Word))
            .requiring(PermissionLevel::Operator));
        // `/tp` is Vanilla level 2 (wiki: Commands/teleport), so level-0
        // players cannot reach it — AUDIT-19 G-09 found the default `All`
        // permission left it open. Recorded as a deliberate divergence from
        // the "phase-19 surface" note: this is a tightening, not a new
        // restriction.
        add(Command::new("tp", "Teleport to coordinates")
            .with_argument(Argument::word("target"))
            .with_argument(Argument::required("pos", ArgumentKind::BlockPos))
            .with_argument(Argument::optional(
                "yaw",
                ArgumentKind::Double {
                    min: -360.0,
                    max: 360.0,
                },
            ))
            .with_argument(Argument::optional(
                "pitch",
                ArgumentKind::Double {
                    min: -90.0,
                    max: 90.0,
                },
            ))
            .requiring(PermissionLevel::Operator));
        // P18-02: `teleport` is Vanilla's alias of `tp` with the same full argument form.
        add(Command::new("teleport", "Teleport to coordinates")
            .with_argument(Argument::word("target"))
            .with_argument(Argument::required("pos", ArgumentKind::BlockPos))
            .with_argument(Argument::optional(
                "yaw",
                ArgumentKind::Double {
                    min: -360.0,
                    max: 360.0,
                },
            ))
            .with_argument(Argument::optional(
                "pitch",
                ArgumentKind::Double {
                    min: -90.0,
                    max: 90.0,
                },
            ))
            .requiring(PermissionLevel::Operator));
        // The whole chain is one greedy argument: `execute` is parsed by its own regular
        // grammar (see `mc_command::execute`), not by the tree, because its modifiers may
        // appear in any order and any number of times.
        add(
            Command::new("execute", "Run a command in a modified context")
                .with_argument(Argument::greedy("command")),
        );
        // A function runs commands as the invoker, so it needs no permission of its own: each
        // command inside it passes its own check. Vanilla agrees, and it is what stops a function
        // from being a privilege-escalation route.
        add(Command::new("function", "Run a data function").with_argument(Argument::word("name")));
        add(Command::new("op", "Grant operator status")
            .with_argument(Argument::required("target", ArgumentKind::PlayerName))
            .requiring(PermissionLevel::Administrator));
        add(Command::new("deop", "Revoke operator status")
            .with_argument(Argument::required("target", ArgumentKind::PlayerName))
            .requiring(PermissionLevel::Administrator));
        // P19-01: the whitelist gate. Administrator like Vanilla's level 3.
        add(Command::new("whitelist", "Manage the login whitelist")
            .with_argument(Argument::word("action"))
            .with_argument(Argument::optional("target", ArgumentKind::PlayerName))
            .requiring(PermissionLevel::Administrator));
        // P19-02: bans and kicks. Administrator like Vanilla's level 3.
        // AUDIT-19 fix round: `reason` is **optional** greedy, which is
        // Vanilla's `/ban <player> [reason]` shape. Declared required greedy it
        // was rejected by the parser before the handler ran, so
        // `command_ban`'s default-reason branch could only be reached by a
        // caller that passed no arguments at all. Pinned from the chat path by
        // `ban_without_a_reason_files_the_default_reason`.
        add(Command::new("ban", "Ban a profile and disconnect them")
            .with_argument(Argument::required("target", ArgumentKind::PlayerName))
            .with_argument(Argument::optional("reason", ArgumentKind::GreedyString))
            .requiring(PermissionLevel::Administrator));
        add(
            Command::new("ban-ip", "Ban an address and disconnect its players")
                .with_argument(Argument::word("address"))
                .with_argument(Argument::greedy("reason"))
                .requiring(PermissionLevel::Administrator),
        );
        add(Command::new("pardon", "Pardon a banned profile")
            .with_argument(Argument::required("target", ArgumentKind::PlayerName))
            .requiring(PermissionLevel::Administrator));
        add(Command::new("pardon-ip", "Pardon a banned address")
            .with_argument(Argument::word("address"))
            .requiring(PermissionLevel::Administrator));
        add(Command::new("banlist", "List bans")
            .with_argument(Argument::optional("kind", ArgumentKind::Word))
            .requiring(PermissionLevel::Administrator));
        add(Command::new("kick", "Disconnect a player")
            .with_argument(Argument::required("target", ArgumentKind::PlayerName))
            .with_argument(Argument::greedy("reason"))
            .requiring(PermissionLevel::Administrator));
        add(Command::new("stop", "Stop the server").requiring(PermissionLevel::Console));
        // P19-03: saving controls, operator-only like the P18 admin set.
        add(Command::new("save-all", "Save the world now")
            .with_argument(Argument::optional("mode", ArgumentKind::Word))
            .requiring(PermissionLevel::Operator));
        add(Command::new("save-off", "Hold automatic world saves")
            .requiring(PermissionLevel::Operator));
        add(Command::new("save-on", "Resume automatic world saves")
            .requiring(PermissionLevel::Operator));
        // P14-01: the admin set, all operator-only like Vanilla's level 2.
        add(Command::new("gamemode", "Set your game mode")
            .with_argument(Argument::word("mode"))
            .with_argument(Argument::optional("target", ArgumentKind::PlayerName))
            .requiring(PermissionLevel::Operator));
        add(Command::new("give", "Give yourself items")
            .with_argument(Argument::required("target", ArgumentKind::PlayerName))
            .with_argument(Argument::required("item", ArgumentKind::Resource))
            .with_argument(Argument::optional(
                "count",
                ArgumentKind::Integer(ValueRange::new(1, 64)),
            ))
            .requiring(PermissionLevel::Operator));
        add(Command::new("kill", "Kill yourself")
            .with_argument(Argument::optional("target", ArgumentKind::PlayerName))
            .requiring(PermissionLevel::Operator));
        add(Command::new("effect", "Give or clear a status effect")
            .with_argument(Argument::word("action"))
            .with_argument(Argument::optional("target", ArgumentKind::PlayerName))
            .with_argument(Argument::optional("effect", ArgumentKind::Word))
            .with_argument(Argument::optional(
                "seconds",
                ArgumentKind::Integer(ValueRange::new(1, 1_000_000)),
            ))
            .with_argument(Argument::optional(
                "amplifier",
                ArgumentKind::Integer(ValueRange::new(0, 255)),
            ))
            .requiring(PermissionLevel::Operator));
        add(Command::new("seed", "Show the world seed").requiring(PermissionLevel::Operator));
        add(Command::new("difficulty", "Query or set the difficulty")
            .with_argument(Argument::optional("difficulty", ArgumentKind::Word))
            .requiring(PermissionLevel::Operator));
        // P18-02 closed list. Operator-only like Vanilla's level 2.
        add(Command::new("clear", "Clear your inventory")
            .with_argument(Argument::optional("target", ArgumentKind::PlayerName))
            .with_argument(Argument::optional("item", ArgumentKind::Resource))
            .with_argument(Argument::optional(
                "maxCount",
                ArgumentKind::Integer(ValueRange::new(0, 99 * 64)),
            ))
            .requiring(PermissionLevel::Operator));
        add(Command::new("experience", "Add, set or query experience")
            .with_argument(Argument::word("action"))
            .with_argument(Argument::optional("value", ArgumentKind::Word))
            .with_argument(Argument::optional("unit", ArgumentKind::Word))
            .requiring(PermissionLevel::Operator));
        add(Command::new("xp", "Add, set or query experience")
            .with_argument(Argument::word("action"))
            .with_argument(Argument::optional("value", ArgumentKind::Word))
            .with_argument(Argument::optional("unit", ArgumentKind::Word))
            .requiring(PermissionLevel::Operator));
        add(Command::new("enchant", "Enchant the held item")
            .with_argument(Argument::word("enchantment"))
            .with_argument(Argument::optional(
                "level",
                ArgumentKind::Integer(ValueRange::new(1, 5)),
            ))
            .requiring(PermissionLevel::Operator));
        add(Command::new("setblock", "Set a block")
            .with_argument(Argument::required("pos", ArgumentKind::BlockPos))
            .with_argument(Argument::required("block", ArgumentKind::Resource))
            .with_argument(Argument::optional("mode", ArgumentKind::Word))
            .requiring(PermissionLevel::Operator));
        add(Command::new("fill", "Fill a region with blocks")
            .with_argument(Argument::required("from", ArgumentKind::BlockPos))
            .with_argument(Argument::required("to", ArgumentKind::BlockPos))
            .with_argument(Argument::required("block", ArgumentKind::Resource))
            .with_argument(Argument::optional("mode", ArgumentKind::Word))
            .requiring(PermissionLevel::Operator));
        add(Command::new("summon", "Summon an entity")
            .with_argument(Argument::word("type"))
            .with_argument(Argument::optional("pos", ArgumentKind::BlockPos))
            .requiring(PermissionLevel::Operator));
        add(Command::new("setworldspawn", "Set the world spawn")
            .with_argument(Argument::optional("pos", ArgumentKind::BlockPos))
            .requiring(PermissionLevel::Operator));
        for name in ["msg", "tell", "w"] {
            add(Command::new(name, "Send a private message")
                .with_argument(Argument::required("target", ArgumentKind::PlayerName))
                .with_argument(Argument::greedy("message")));
        }
        add(Command::new("me", "Broadcast a narrative action")
            .with_argument(Argument::greedy("action")));
        tree
    }

    /// Dispatch a command string from a connection.
    ///
    /// This is the whole of P07-05's integration: parse, then run. The permission check
    /// happened during parsing, so by the time a handler runs the source is known to be
    /// allowed.
    ///
    /// # Errors
    ///
    /// [`ServerError::Invariant`](mc_core::error::ServerError::Invariant) when a reply
    /// cannot be encoded, which is a server-side bug rather than client input. A command
    /// that does not parse, is not permitted or does not fit its grammar is **not** an
    /// error: it is answered with a message, because that is normal player input.
    pub fn dispatch_command(
        &mut self,
        id: mc_network::bridge::ConnectionId,
        command: &str,
        report: &mut TickReport,
    ) -> ServerResult<()> {
        let Some(source) = self.command_source(id) else {
            return Ok(());
        };
        let dispatcher = mc_command::Dispatcher::new(Self::build_command_tree());
        let outcome = dispatcher.parse(command, &source);

        let feedback = match outcome {
            mc_command::CommandOutcome::Parsed(parsed) => {
                match self.run_command(id, &parsed, report)? {
                    CommandResult::Ok { feedback } => feedback,
                    CommandResult::Stop => {
                        info!(id = %id, "stop requested via command");
                        self.request_shutdown();
                        Some("Stopping the server…".to_owned())
                    }
                }
            }
            mc_command::CommandOutcome::UnknownCommand { name, suggestions } => {
                if suggestions.is_empty() {
                    Some(format!("Unknown command {name:?}. Try /help."))
                } else {
                    Some(format!(
                        "Unknown command {name:?}. Did you mean {}?",
                        suggestions.join(", ")
                    ))
                }
            }
            mc_command::CommandOutcome::PermissionDenied { name, required, .. } => {
                // The level is logged, not sent: a player does not need to know the
                // numeric ladder to be told no.
                debug!(id = %id, command = name, %required, "command denied by permission");
                Some(format!("You do not have permission to use /{name}."))
            }
            mc_command::CommandOutcome::BadArguments(error) => Some(error.to_string()),
        };

        if let Some(text) = feedback {
            self.send(
                id,
                &mc_protocol::packets::play::DisguisedChat {
                    message: TextComponent::literal(text),
                    chat_type: crate::game::CHAT_TYPE_CHAT,
                    sender_name: TextComponent::literal("Server"),
                    target_name: None,
                },
                report,
            )?;
        }
        Ok(())
    }

    /// Dispatch a command string from the server console (P19-03).
    ///
    /// Same parse-then-run as [`Game::dispatch_command`], but the source is
    /// the console (level 4: it may run everything, including
    /// console-only `stop`) and feedback returns as lines instead of chat —
    /// there is no connection to send to. Handlers that need a live session
    /// run against a sentinel id no session holds
    /// (`ConnectionId(0)`; allocation starts at 1), so session-targeted
    /// commands report "not online" rather than acting on a stranger.
    ///
    /// # Errors
    ///
    /// As for [`Game::dispatch_command`]: only an unencodable reply is an
    /// error; refusals come back as lines.
    pub fn dispatch_console(
        &mut self,
        command: &str,
        report: &mut TickReport,
    ) -> ServerResult<Vec<String>> {
        use mc_command::source::CommandSource;
        let source = CommandSource::console();
        let dispatcher = mc_command::Dispatcher::new(Self::build_command_tree());
        let outcome = dispatcher.parse(command, &source);
        let mut lines = Vec::new();
        // `run_command`'s session lookups all miss on the sentinel, by
        // construction above — but if a future handler ever *creates* state
        // for an unknown id, this is where it would surface.
        let id = mc_network::bridge::ConnectionId(0);
        match outcome {
            mc_command::CommandOutcome::Parsed(parsed) => {
                match self.run_command(id, &parsed, report)? {
                    CommandResult::Ok { feedback } => {
                        if let Some(text) = feedback {
                            lines.push(text);
                        }
                    }
                    CommandResult::Stop => {
                        info!("stop requested via console");
                        self.request_shutdown();
                        lines.push("Stopping the server…".to_owned());
                    }
                }
            }
            mc_command::CommandOutcome::UnknownCommand { name, suggestions } => {
                if suggestions.is_empty() {
                    lines.push(format!("Unknown command {name:?}. Try /help."));
                } else {
                    lines.push(format!(
                        "Unknown command {name:?}. Did you mean {}?",
                        suggestions.join(", ")
                    ));
                }
            }
            mc_command::CommandOutcome::PermissionDenied { name, .. } => {
                // Unreachable: the console outranks every gate. A refusal
                // here means a new gate above Console, and it must be loud.
                lines.push(format!("The console cannot use /{name}."));
            }
            mc_command::CommandOutcome::BadArguments(error) => {
                lines.push(error.to_string());
            }
        }
        Ok(lines)
    }

    /// The command source for a connection, or `None` when it has no session.
    ///
    /// The level comes from the session, which the join path set from
    /// `ops.json`: listed uuids hold their file level, everyone else is level
    /// 0 (`All`). A stale comment here once claimed no storage was read; the
    /// `ops_e2e` suite proves otherwise (a level-4 operator stops the server).
    #[must_use]
    pub fn command_source(&self, id: mc_network::bridge::ConnectionId) -> Option<CommandSource> {
        let session = self.sessions.get(&id)?;
        let position = session.player.position;
        Some(
            CommandSource::player(
                session.player.profile.name.clone(),
                mc_command::source::SourcePosition::new(position.x, position.y, position.z),
            )
            .with_permission(self.player_permission(id)),
        )
    }

    /// The permission level a connection holds.
    ///
    /// Level 0 unless the join path granted more from `ops.json` (P07-04,
    /// proven by `ops_e2e`). A command that needs more than the source holds
    /// is answered with a refusal, never run (P08-08).
    #[must_use]
    pub fn player_permission(
        &self,
        id: mc_network::bridge::ConnectionId,
    ) -> mc_command::PermissionLevel {
        self.sessions
            .get(&id)
            .map_or(mc_command::PermissionLevel::All, |session| {
                session.permission
            })
    }

    /// Run a parsed command.
    ///
    /// `pub(crate)` so `/execute` runs its inner command through this same match rather than
    /// duplicating it: two dispatch paths would be two places for a new command to be forgotten.
    pub(crate) fn run_command(
        &mut self,
        id: mc_network::bridge::ConnectionId,
        parsed: &mc_command::dispatch::ParsedCommand,
        report: &mut TickReport,
    ) -> ServerResult<CommandResult> {
        match parsed.name {
            "help" => Ok(Self::command_help(&parsed.source)),
            "list" => Ok(self.command_list()),
            "say" => Ok(self.command_say(parsed, report)?),
            "time" => Ok(self.command_time(parsed, report)?),
            "weather" => Ok(self.command_weather(parsed, report)),
            "tp" | "teleport" => Ok(self.command_tp(id, parsed)),
            "execute" => {
                // The chain parser owns the grammar; the token list is the raw argument text so
                // `run` can hand the inner command its original words.
                let tokens: Vec<String> = parsed
                    .raw_arguments
                    .split_whitespace()
                    .map(str::to_owned)
                    .collect();
                self.dispatch_execute(id, &tokens, report, 0)?;
                Ok(CommandResult::silent())
            }
            "function" => self.command_function(id, parsed, report),
            "op" => Ok(self.command_op(id, parsed)),
            "deop" => Ok(self.command_deop(id, parsed)),
            "whitelist" => Ok(self.command_whitelist(id, parsed)),
            "ban" => Ok(self.command_ban(id, parsed)),
            "ban-ip" => Ok(self.command_ban_ip(id, parsed)),
            "pardon" => Ok(self.command_pardon(id, parsed)),
            "pardon-ip" => Ok(self.command_pardon_ip(id, parsed)),
            "banlist" => Ok(self.command_banlist(id, parsed)),
            "kick" => Ok(self.command_kick(id, parsed)),
            "gamemode" => Ok(self.command_gamemode(id, parsed, report)),
            "give" => Ok(self.command_give(id, parsed, report)),
            "effect" => Ok(self.command_effect(id, parsed, report)),
            "kill" => Ok(self.command_kill(id, parsed)),
            "seed" => Ok(self.command_seed()),
            "difficulty" => Ok(self.command_difficulty(parsed)),
            "clear" => Ok(self.command_clear(id, parsed, report)),
            "xp" | "experience" => Ok(self.command_xp(id, parsed, report)),
            "enchant" => Ok(self.command_enchant(id, parsed, report)),
            "setblock" => Ok(self.command_setblock(parsed)),
            "fill" => Ok(self.command_fill(parsed)),
            "summon" => Ok(self.command_summon(parsed)),
            "setworldspawn" => Ok(self.command_setworldspawn(parsed)),
            "msg" | "tell" | "w" => Ok(self.command_msg(id, parsed, report)),
            "me" => Ok(self.command_me(parsed, report)),
            "stop" => Ok(CommandResult::Stop),
            "save-all" => Ok(self.command_save_all(parsed)),
            "save-off" => Ok(self.command_save_off()),
            "save-on" => Ok(self.command_save_on()),
            // Unreachable: the tree only contains the names above, and `parse` resolved
            // this one through it. Returning a refusal rather than panicking keeps a
            // future tree/`match` divergence from taking the server down.
            other => {
                warn!(command = other, "a parsed command has no handler");
                Ok(CommandResult::message(format!(
                    "/{other} is declared but not implemented."
                )))
            }
        }
    }

    /// `/help`
    ///
    /// Associated rather than a method: nothing about the server's state affects the
    /// answer, only the source's permissions.
    fn command_help(source: &CommandSource) -> CommandResult {
        let tree = Self::build_command_tree();
        let mut available: Vec<String> = tree
            .commands()
            .iter()
            .filter(|command| source.may_use(command.permission))
            .map(|command| format!("{} — {}", command.usage(), command.description))
            .collect();
        available.sort();
        let count = available.len();
        // No pagination: Vanilla's `/help` takes a page number, and this build does not
        // have enough commands for one to matter. Stated in the module docs.
        CommandResult::message(format!(
            "{count} commands available:\n{}",
            available.join("\n")
        ))
    }

    /// `/list`
    fn command_list(&self) -> CommandResult {
        let names = self.player_names();
        CommandResult::message(format!(
            "There are {} of a maximum of {} players online: {}",
            names.len(),
            self.max_players(),
            names.join(", ")
        ))
    }

    /// `/say <message>`
    fn command_say(
        &mut self,
        parsed: &mc_command::dispatch::ParsedCommand,
        report: &mut TickReport,
    ) -> ServerResult<CommandResult> {
        let Some(message) = parsed.string(0) else {
            // Unreachable: `say` declares a required greedy argument, so parsing refuses
            // an invocation without one.
            return Ok(CommandResult::message("Usage: /say <message>"));
        };
        let line = format!("[{}] {message}", parsed.source.name);
        let ids: Vec<mc_network::bridge::ConnectionId> = self.sessions.keys().copied().collect();
        for target in ids {
            self.send(
                target,
                &mc_protocol::packets::play::DisguisedChat {
                    message: TextComponent::literal(line.clone()),
                    chat_type: crate::game::CHAT_TYPE_CHAT,
                    sender_name: TextComponent::literal("Server"),
                    target_name: None,
                },
                report,
            )?;
        }
        // The console sees it through the log rather than a packet.
        info!(from = %parsed.source.name, %message, "say");
        Ok(CommandResult::silent())
    }

    /// `/time [query]`, `/time <ticks>`, `/time set <ticks|day|noon|night|midnight>`
    ///
    /// Vanilla's `add` is not modelled (declared): advancing the clock by a
    /// delta is a second verb on the same offset, and this handler covers set
    /// plus query — the two the walk asked for.
    fn command_time(
        &mut self,
        parsed: &mc_command::dispatch::ParsedCommand,
        report: &mut TickReport,
    ) -> ServerResult<CommandResult> {
        let action = parsed.string(0);
        let value = parsed.string(1);
        let query = || {
            let time = (self.tick_count() % 24_000).cast_signed() + self.time_offset();
            Ok(CommandResult::message(format!(
                "The time is {} (daytime)",
                time.rem_euclid(24_000)
            )))
        };
        // Bare `/time` and `/time query` read the clock. Anything else must
        // resolve to a set or say usage: the old fallback answered every typo
        // with the time, which made a failed set indistinguishable from a
        // successful one on the live client (P14-09 walk).
        let target: Option<i64> = match (action, value) {
            (None | Some("query"), None) => return query(),
            (Some("set"), Some(word)) => Self::time_value(word),
            (Some(word), None) => word.parse::<i64>().ok().or_else(|| Self::time_preset(word)),
            _ => {
                return Ok(CommandResult::message(
                    "Usage: /time [query | <ticks> | set <ticks|day|noon|night|midnight>]"
                        .to_owned(),
                ));
            }
        };
        let Some(target) = target else {
            return Ok(CommandResult::message(
                "Usage: /time [query | <ticks> | set <ticks|day|noon|night|midnight>]".to_owned(),
            ));
        };
        // Setting records an offset from the tick counter, so the value sticks
        // while the clock keeps advancing — and the broadcast below carries it
        // to every client in the overworld clock entry (P14-09 walk: the old
        // shape decoded as an empty clock map, so `/time` moved the server's
        // mobs but never the client's sky).
        self.set_time_offset(target - (self.tick_count() % 24_000).cast_signed());
        info!(
            from = %parsed.source.name,
            target,
            offset = self.time_offset(),
            "time offset set"
        );
        let packet = SetTime {
            world_age: self.tick_count().cast_signed(),
            clocks: vec![self.overworld_clock_entry(self.tick_count())],
        };
        let ids: Vec<mc_network::bridge::ConnectionId> = self.sessions.keys().copied().collect();
        for target in ids {
            self.send(target, &packet, report)?;
        }
        Ok(CommandResult::message(format!(
            "Set the time to {}",
            (self.tick_count().cast_signed() + self.time_offset()).rem_euclid(24_000)
        )))
    }

    /// `/weather <clear|rain|thunder> [duration]` (P20-03).
    ///
    /// Jar `WeatherCommand`: `clear` → `(RAIN_DELAY.sample or t, 0, false,
    /// false)`, `rain` → `(0, RAIN_DURATION.sample or t, true, false)`,
    /// `thunder` → `(0, THUNDER_DURATION.sample or t, true, true).` An absent
    /// duration samples the provider (never a fixed number); a present one is
    /// integer ticks, minimum 1 like the jar's `TimeArgument.time(1)`.
    fn command_weather(
        &mut self,
        parsed: &mc_command::dispatch::ParsedCommand,
        report: &mut TickReport,
    ) -> CommandResult {
        const USAGE: &str = "Usage: /weather <clear|rain|thunder> [duration ticks]";
        let Some(kind) = parsed.string(0) else {
            return CommandResult::message(USAGE.to_owned());
        };
        let duration = match parsed.string(1) {
            None => None,
            Some(word) => match word.parse::<i32>() {
                Ok(ticks) if ticks >= 1 => Some(ticks),
                _ => {
                    return CommandResult::message(
                        "The duration must be at least 1 tick".to_owned(),
                    );
                }
            },
        };
        // `getDuration`: absent samples the duration provider with the level's
        // RNG — here the game's seeded source, which is also what the cycle
        // itself draws from.
        let mut sample = |min: i32, max: i32| self.sample_uniform(min, max);
        let (clear, rain, raining, thundering, label) = match kind {
            "clear" => (
                duration.unwrap_or_else(|| {
                    sample(crate::game::RAIN_DELAY_MIN, crate::game::RAIN_DELAY_MAX)
                }),
                0,
                false,
                false,
                "clear",
            ),
            "rain" => (
                0,
                duration.unwrap_or_else(|| {
                    sample(
                        crate::game::RAIN_DURATION_MIN,
                        crate::game::RAIN_DURATION_MAX,
                    )
                }),
                true,
                false,
                "rain",
            ),
            "thunder" => (
                0,
                duration.unwrap_or_else(|| {
                    sample(
                        crate::game::THUNDER_DURATION_MIN,
                        crate::game::THUNDER_DURATION_MAX,
                    )
                }),
                true,
                true,
                "thunder",
            ),
            _ => return CommandResult::message(USAGE.to_owned()),
        };
        self.set_weather_parameters(clear, rain, raining, thundering, report);
        info!(from = %parsed.source.name, kind = label, "weather set");
        CommandResult::message(format!("Set the weather to {label}"))
    }

    /// A `/time` value word: an integer, or one of Vanilla's presets
    /// (`day` 1000, `noon` 6000, `night` 13000, `midnight` 18000).
    fn time_value(word: &str) -> Option<i64> {
        if let Ok(value) = word.parse::<i64>() {
            return Some(value.rem_euclid(24_000));
        }
        Self::time_preset(word)
    }

    /// Vanilla's named times, or `None`.
    fn time_preset(word: &str) -> Option<i64> {
        match word {
            "day" => Some(1_000),
            "noon" => Some(6_000),
            "night" => Some(13_000),
            "midnight" => Some(18_000),
            _ => None,
        }
    }

    /// `/tp` / `/teleport <target> <pos> [yaw] [pitch]`
    ///
    /// The target must name the invoking player: there is no cross-player teleport
    /// authority model yet, so teleporting someone else is refused with that reason
    /// rather than silently doing nothing. Optional `yaw`/`pitch` are recorded on the
    /// command source; the client owns its camera, so they are not written back to
    /// the player entity (named gap).
    fn command_tp(
        &mut self,
        _id: mc_network::bridge::ConnectionId,
        parsed: &mc_command::dispatch::ParsedCommand,
    ) -> CommandResult {
        let Some(target) = parsed.string(0) else {
            return CommandResult::message("Usage: /tp <target> <pos> [yaw] [pitch]");
        };
        if !Self::targets_self(target, &parsed.source.name) {
            return CommandResult::message(format!(
                "Cannot teleport {target:?}: this build only teleports the invoking \
                 player ({})",
                parsed.source.name
            ));
        }
        let Some(mc_command::ArgumentValue::BlockPos { x, y, z }) = parsed.argument(1) else {
            return CommandResult::message("Usage: /tp <target> <pos> [yaw] [pitch]");
        };
        let base = parsed.source.block_position().unwrap_or((0, 0, 0));
        // `~` takes the source's own coordinate; a bare number is absolute. The two are carried separately by
        // `Coordinate` — an `Option<i32>` could not tell them apart, which is why `~1` used to land at the
        // absolute `1`.
        let (tx, ty, tz) = (x.resolve(base.0), y.resolve(base.1), z.resolve(base.2));
        match self.teleport_source(&parsed.source, tx, ty, tz) {
            Ok(()) => CommandResult::message(format!("Teleported to {tx} {ty} {tz}")),
            Err(reason) => CommandResult::message(reason),
        }
    }

    /// `/op <player>` and `/deop <player>` (P14-02).
    ///
    /// Both require level 3, matching Vanilla (`/op` used to sit at level 2
    /// here, which let a level-2 holder mint level-4 operators — a privilege
    /// escalation Vanilla's ladder does not allow). Only online players can
    /// be named: their uuid comes from the session, and matching by name
    /// would let anyone take an operator's identity. Grants land at level 4,
    /// Vanilla's default `op-permission-level`.
    fn command_op(
        &mut self,
        _id: mc_network::bridge::ConnectionId,
        parsed: &mc_command::dispatch::ParsedCommand,
    ) -> CommandResult {
        let Some(name) = parsed.string(0) else {
            return CommandResult::message("Usage: /op <player>");
        };
        let Some(target) = self.session_id_by_name(name) else {
            return CommandResult::message(format!(
                "Cannot op {name:?}: only online players can be granted (name matching cannot identify anyone else)"
            ));
        };
        match self.grant_operator(target, mc_command::PermissionLevel::Console) {
            Err(error) => CommandResult::message(format!(
                "Could not persist the grant, and nothing was changed: {error}"
            )),
            Ok(None) => CommandResult::message(
                "/op cannot persist: no ops directory was ever set, so nothing was changed.",
            ),
            Ok(Some((granted, true))) => {
                CommandResult::message(format!("Made {granted} a server operator"))
            }
            Ok(Some((granted, false))) => {
                CommandResult::message(format!("Nothing changed: {granted} is already an operator"))
            }
        }
    }

    /// `/deop <player>` (P14-02): the reverse grant, same ladder, same file.
    fn command_deop(
        &mut self,
        _id: mc_network::bridge::ConnectionId,
        parsed: &mc_command::dispatch::ParsedCommand,
    ) -> CommandResult {
        let Some(name) = parsed.string(0) else {
            return CommandResult::message("Usage: /deop <player>");
        };
        let Some(target) = self.session_id_by_name(name) else {
            return CommandResult::message(format!(
                "Cannot deop {name:?}: only online players can be revoked"
            ));
        };
        match self.revoke_operator(target) {
            Err(error) => CommandResult::message(format!(
                "Could not persist the revocation, and nothing was changed: {error}"
            )),
            Ok(None) => {
                CommandResult::message(format!("Nothing changed: {name} is not an operator"))
            }
            Ok(Some(revoked)) => {
                CommandResult::message(format!("Made {revoked} no longer a server operator"))
            }
        }
    }

    /// `/whitelist <on|off|list|add|remove|reload> [player]` (P19-01).
    ///
    /// Administrator like Vanilla's level 3. `on|off` toggles enforcement
    /// live (a restart restores the config value — stated, not hidden);
    /// `add|remove` name online or offline players (an online player's own
    /// uuid is used, A-06) and persist `whitelist.json` with the same
    /// rollback contract as `/op`; `list` names the listed profiles;
    /// `reload` re-reads the file, replacing the live list.
    ///
    /// **A revocation takes effect now** (AUDIT-19 A-12): enforcement turning
    /// on disconnects every online player the gate would now refuse, and
    /// `remove` disconnects the player it just unlisted. Vanilla's gate is
    /// checked at login only, so without this a player kept full access until
    /// they happened to reconnect. Operators keep their bypass at both points,
    /// exactly as they do at the gate.
    fn command_whitelist(
        &mut self,
        _id: mc_network::bridge::ConnectionId,
        parsed: &mc_command::dispatch::ParsedCommand,
    ) -> CommandResult {
        const USAGE: &str = "Usage: /whitelist <on|off|list|add|remove|reload> [player]";
        /// Vanilla's white-list kick, word for word the join gate's refusal.
        const UNLISTED_KICK: &str = "You are not white-listed on this server!";
        let Some(action) = parsed.string(0) else {
            return CommandResult::message(USAGE);
        };
        match action {
            "on" => {
                self.set_whitelist_enforced(true);
                let kicked = self.kick_unlisted_players(UNLISTED_KICK);
                if kicked == 0 {
                    CommandResult::message("Whitelist is now enforced")
                } else {
                    CommandResult::message(format!(
                        "Whitelist is now enforced; disconnected {kicked} player(s) not on it"
                    ))
                }
            }
            "off" => {
                self.set_whitelist_enforced(false);
                CommandResult::message("Whitelist is no longer enforced")
            }
            "list" => {
                let names = self.whitelist_names();
                if names.is_empty() {
                    CommandResult::message("Nobody is whitelisted")
                } else {
                    CommandResult::message(format!(
                        "Whitelisted ({}): {}",
                        names.len(),
                        names.join(", ")
                    ))
                }
            }
            "add" => {
                let Some(name) = parsed.string(1) else {
                    return CommandResult::message("Usage: /whitelist add <player>");
                };
                // Offline names resolve by derivation, like bans: only the
                // file write can refuse now.
                match self.add_to_whitelist(name) {
                    Err(error) => CommandResult::message(format!(
                        "Could not persist the listing, and nothing was changed: {error}"
                    )),
                    Ok(None) => CommandResult::message(
                        "/whitelist cannot persist: no ops directory was ever set, \
                         so nothing was changed.",
                    ),
                    Ok(Some((listed, true))) => {
                        CommandResult::message(format!("Added {listed} to the whitelist"))
                    }
                    Ok(Some((listed, false))) => CommandResult::message(format!(
                        "Nothing changed: {listed} is already whitelisted"
                    )),
                }
            }
            "remove" => {
                let Some(name) = parsed.string(1) else {
                    return CommandResult::message("Usage: /whitelist remove <player>");
                };
                match self.remove_from_whitelist(name) {
                    Err(error) => CommandResult::message(format!(
                        "Could not persist the removal, and nothing was changed: {error}"
                    )),
                    Ok(None) => CommandResult::message(format!(
                        "Nothing changed: {name} is not whitelisted"
                    )),
                    Ok(Some(unlisted)) => {
                        // The player who just lost access goes now, not at
                        // their next login — but only when they *did* lose
                        // it: with enforcement off nobody is refused, so
                        // kicking would punish a player the file no longer
                        // mentions while the gate is open (AUDIT-19 A-12).
                        if let Some(target) = self.session_id_by_name(&unlisted) {
                            let denied = self
                                .sessions
                                .get(&target)
                                .is_some_and(|session| self.whitelist_denies(&session.uuid));
                            if denied {
                                self.kick_player(target, UNLISTED_KICK.to_owned());
                            }
                        }
                        CommandResult::message(format!("Removed {unlisted} from the whitelist"))
                    }
                }
            }
            "reload" => match self.reload_whitelist() {
                Err(error) => CommandResult::message(format!(
                    "Could not reload the whitelist, and nothing was changed: {error}"
                )),
                Ok(None) => CommandResult::message(
                    "/whitelist cannot reload: no ops directory was ever set.",
                ),
                Ok(Some(count)) => {
                    CommandResult::message(format!("Reloaded the whitelist ({count} listed)"))
                }
            },
            other => CommandResult::message(format!("Unknown whitelist action {other:?}. {USAGE}")),
        }
    }

    /// Resolve a ban/pardon/whitelist name to the uuid the server would see.
    ///
    /// The **live session's** uuid when the named player is online, and
    /// offline-mode derivation (`OfflinePlayer:<name>`) otherwise.
    /// `OfflinePlayer:<name>` is exactly the identity an **offline** server
    /// joins with, so the fallback is right there. Under online-mode auth
    /// (P19-05) a session carries the **Mojang** uuid from `hasJoined`, which
    /// the join gate compares and which is what a ban or whitelist row must
    /// carry to match (AUDIT-19 A-06: deriving offline for a live player wrote
    /// a row nothing would ever compare against, so `/ban <online>` reported
    /// `Banned X` while neither kicking nor banning).
    ///
    /// A name that is **not** online still derives offline, and under
    /// online-mode that is a named gap: the Mojang uuid of an absent player is
    /// only knowable from a usercache, which this build does not keep.
    pub(crate) fn ban_uuid_for(name: &str) -> String {
        mc_network::auth::offline_profile(name).id.to_string()
    }

    /// The uuid and canonical name a ban/whitelist entry should carry for
    /// `name`, taking the live session when there is one.
    ///
    /// Split out of [`Self::ban_uuid_for`] so the *command* paths that name a
    /// player can prefer the authenticated identity, while the pure offline
    /// derivation stays available for callers with no session map (and for
    /// tests of the offline shape).
    pub(crate) fn ban_identity_for(&self, name: &str) -> (String, String) {
        match self.session_id_by_name(name) {
            Some(target) => match self.sessions.get(&target) {
                Some(session) => (session.uuid.clone(), session.name.clone()),
                None => (Self::ban_uuid_for(name), name.to_owned()),
            },
            None => (Self::ban_uuid_for(name), name.to_owned()),
        }
    }

    /// Parse a `/ban-ip` address: a bare IP or an `ip:port` pair.
    fn parse_ban_address(text: &str) -> Option<std::net::IpAddr> {
        let text = text.trim();
        if let Ok(ip) = text.parse() {
            return Some(ip);
        }
        text.parse::<std::net::SocketAddr>()
            .map(|addr| addr.ip())
            .ok()
    }

    /// `/ban <player> [reason]` (P19-02): file the ban, then disconnect
    /// the live session with the ban screen.
    ///
    /// The write lands before the kick: a failed save never kicks without
    /// recording. Banning can target an offline profile (the uuid derives
    /// from the name); only the disconnect needs a live session. **An online
    /// player's own uuid is used** — the join gate compares that identity, so
    /// deriving one offline would file a row that never matches (AUDIT-19
    /// A-06, where `/ban <live player>` neither kicked nor banned).
    ///
    /// The reason is **optional**, Vanilla's `/ban <player> [reason]`: with
    /// none, both the filed row and the ban screen carry
    /// [`crate::bans::DEFAULT_BAN_REASON`]. The tree declares it optional
    /// greedy (AUDIT-19 fix round) — while it was required greedy, a bare
    /// `/ban <player>` died in the parser, so this default was unreachable
    /// from chat. The pin is
    /// `ban_without_a_reason_files_the_default_reason`, which runs the chat
    /// path and not this function.
    fn command_ban(
        &mut self,
        _id: mc_network::bridge::ConnectionId,
        parsed: &mc_command::dispatch::ParsedCommand,
    ) -> CommandResult {
        let Some(name) = parsed.string(0) else {
            return CommandResult::message("Usage: /ban <player> [reason]");
        };
        // One binding, so the row and the ban screen cannot disagree about the
        // reason they carry.
        let reason = parsed.string(1).unwrap_or("");
        let reason = if reason.is_empty() {
            crate::bans::DEFAULT_BAN_REASON
        } else {
            reason
        };
        // The live session's uuid when the name is online (AUDIT-19 A-06):
        // the file row and the kick must name the identity the join gate
        // compares, which under online-mode is the Mojang uuid.
        let (uuid, listed) = self.ban_identity_for(name);
        let source = parsed.source.name.clone();
        let target = self.session_id_by_name(name);
        let ban = crate::bans::PlayerBan {
            uuid: uuid.clone(),
            name: listed.clone(),
            created: std::time::SystemTime::now(),
            source,
            expires: None,
            reason: reason.to_owned(),
        };
        match self.file_player_ban(ban) {
            Err(error) => CommandResult::message(format!(
                "Could not persist the ban, and nothing was changed: {error}"
            )),
            Ok(false) => CommandResult::message(
                "/ban cannot persist: no ops directory was ever set, so nothing was changed.",
            ),
            Ok(true) => {
                if let Some(target) = target {
                    self.kick_player(
                        target,
                        format!("You are banned from this server.\nReason: {reason}"),
                    );
                }
                CommandResult::message(format!("Banned {name}"))
            }
        }
    }

    /// `/ban-ip <address> [reason]` (P19-02): file the address ban, then
    /// disconnect every live session holding it.
    fn command_ban_ip(
        &mut self,
        _id: mc_network::bridge::ConnectionId,
        parsed: &mc_command::dispatch::ParsedCommand,
    ) -> CommandResult {
        let Some(text) = parsed.string(0) else {
            return CommandResult::message("Usage: /ban-ip <address> [reason]");
        };
        let Some(ip) = Self::parse_ban_address(text) else {
            return CommandResult::message(format!(
                "{text:?} is not an IP address (bare `1.2.3.4` or `ip:port`)"
            ));
        };
        let reason = parsed.string(1).unwrap_or("").to_owned();
        let source = parsed.source.name.clone();
        match self.file_ip_ban(crate::bans::IpBan {
            ip,
            created: std::time::SystemTime::now(),
            source,
            expires: None,
            reason: if reason.is_empty() {
                crate::bans::DEFAULT_BAN_REASON.to_owned()
            } else {
                reason.clone()
            },
        }) {
            Err(error) => CommandResult::message(format!(
                "Could not persist the ban, and nothing was changed: {error}"
            )),
            Ok(None) => CommandResult::message(
                "/ban-ip cannot persist: no ops directory was ever set, so nothing was changed.",
            ),
            Ok(Some(holders)) => {
                for holder in holders {
                    self.kick_player(
                        holder,
                        format!(
                            "You are banned from this server.\nReason: {}",
                            if reason.is_empty() {
                                crate::bans::DEFAULT_BAN_REASON
                            } else {
                                reason.as_str()
                            }
                        ),
                    );
                }
                CommandResult::message(format!("Banned {ip}"))
            }
        }
    }

    /// `/pardon <player>` (P19-02): remove the profile row.
    ///
    /// Works offline (uuid derives from the name); a missing row is a
    /// no-op report, not an error.
    fn command_pardon(
        &mut self,
        _id: mc_network::bridge::ConnectionId,
        parsed: &mc_command::dispatch::ParsedCommand,
    ) -> CommandResult {
        let Some(name) = parsed.string(0) else {
            return CommandResult::message("Usage: /pardon <player>");
        };
        match self.pardon_player(&Self::ban_uuid_for(name)) {
            Err(error) => CommandResult::message(format!(
                "Could not persist the pardon, and nothing was changed: {error}"
            )),
            Ok(true) => CommandResult::message(format!("Pardoned {name}")),
            Ok(false) => CommandResult::message(format!("Nothing changed: {name} is not banned")),
        }
    }

    /// `/pardon-ip <address>` (P19-02): remove the address row.
    fn command_pardon_ip(
        &mut self,
        _id: mc_network::bridge::ConnectionId,
        parsed: &mc_command::dispatch::ParsedCommand,
    ) -> CommandResult {
        let Some(text) = parsed.string(0) else {
            return CommandResult::message("Usage: /pardon-ip <address>");
        };
        let Some(ip) = Self::parse_ban_address(text) else {
            return CommandResult::message(format!("{text:?} is not an IP address"));
        };
        match self.pardon_address(&ip) {
            Err(error) => CommandResult::message(format!(
                "Could not persist the pardon, and nothing was changed: {error}"
            )),
            Ok(true) => CommandResult::message(format!("Pardoned {ip}")),
            Ok(false) => CommandResult::message(format!("Nothing changed: {ip} is not banned")),
        }
    }

    /// `/banlist [players|ips]` (P19-02): name the rows.
    fn command_banlist(
        &mut self,
        _id: mc_network::bridge::ConnectionId,
        parsed: &mc_command::dispatch::ParsedCommand,
    ) -> CommandResult {
        match parsed.string(0).unwrap_or("players") {
            "players" => {
                let names = self.ban_names();
                if names.is_empty() {
                    CommandResult::message("Nobody is banned")
                } else {
                    CommandResult::message(format!(
                        "Banned players ({}): {}",
                        names.len(),
                        names.join(", ")
                    ))
                }
            }
            "ips" => {
                let ips = self.banned_ips();
                if ips.is_empty() {
                    CommandResult::message("No addresses are banned")
                } else {
                    CommandResult::message(format!(
                        "Banned addresses ({}): {}",
                        ips.len(),
                        ips.iter()
                            .map(std::net::IpAddr::to_string)
                            .collect::<Vec<_>>()
                            .join(", ")
                    ))
                }
            }
            other => CommandResult::message(format!(
                "Unknown ban list {other:?}: ask for `players` or `ips`"
            )),
        }
    }

    /// `/kick <player> [reason]` (P19-02): disconnect a live session with
    /// a reason. No file changes — a kick is a moment, not a record.
    fn command_kick(
        &mut self,
        _id: mc_network::bridge::ConnectionId,
        parsed: &mc_command::dispatch::ParsedCommand,
    ) -> CommandResult {
        let Some(name) = parsed.string(0) else {
            return CommandResult::message("Usage: /kick <player> [reason]");
        };
        let Some(target) = self.session_id_by_name(name) else {
            return CommandResult::message(format!(
                "Cannot kick {name:?}: only online players can be kicked"
            ));
        };
        let reason = parsed.string(1).unwrap_or("").to_owned();
        self.kick_player(
            target,
            if reason.is_empty() {
                "Kicked by an operator.".to_owned()
            } else {
                reason
            },
        );
        CommandResult::message(format!("Kicked {name}"))
    }

    /// Parse a game-mode word: Vanilla's full names plus the `s`/`c`/`a`/`sp`
    /// shortcuts Vanilla accepts.
    fn parse_game_mode(word: &str) -> Option<mc_entity::GameMode> {
        use mc_entity::GameMode;
        Some(match word {
            "survival" | "s" => GameMode::Survival,
            "creative" | "c" => GameMode::Creative,
            "adventure" | "a" => GameMode::Adventure,
            "spectator" | "sp" => GameMode::Spectator,
            _ => return None,
        })
    }

    /// `/gamemode <mode> [target]`
    fn command_gamemode(
        &mut self,
        id: mc_network::bridge::ConnectionId,
        parsed: &mc_command::dispatch::ParsedCommand,
        report: &mut TickReport,
    ) -> CommandResult {
        let Some(word) = parsed.string(0) else {
            return CommandResult::message("Usage: /gamemode <mode> [target]");
        };
        let Some(mode) = Self::parse_game_mode(word) else {
            return CommandResult::message(
                "Usage: /gamemode <survival|creative|adventure|spectator> [target]",
            );
        };
        let target = parsed.string(1).unwrap_or(&parsed.source.name);
        if !Self::targets_self(target, &parsed.source.name) {
            return CommandResult::message(format!(
                "Cannot change {target:?}'s game mode: this build only changes the invoking player"
            ));
        }
        if !self.set_player_game_mode(id, mode, report) {
            return CommandResult::message("You are not online.");
        }
        let name = match mode {
            mc_entity::GameMode::Survival => "Survival Mode",
            mc_entity::GameMode::Creative => "Creative Mode",
            mc_entity::GameMode::Adventure => "Adventure Mode",
            mc_entity::GameMode::Spectator => "Spectator Mode",
        };
        CommandResult::message(format!("Set own game mode to {name}"))
    }

    /// `/give <target> <item> [count]`
    fn command_give(
        &mut self,
        id: mc_network::bridge::ConnectionId,
        parsed: &mc_command::dispatch::ParsedCommand,
        report: &mut TickReport,
    ) -> CommandResult {
        let target = parsed.string(0).unwrap_or("");
        if !Self::targets_self(target, &parsed.source.name) {
            return CommandResult::message(format!(
                "Cannot give to {target:?}: this build only gives to the invoking player"
            ));
        }
        let Some(item) = parsed.argument(1).and_then(|value| value.as_resource()) else {
            return CommandResult::message("Usage: /give <target> <item> [count]");
        };
        let name = item.to_string();
        let Ok(item_id) = self.registries().items.id(&name) else {
            return CommandResult::message(format!("Unknown item {name:?}"));
        };
        // The grammar caps the count at one stack (1..=64), so the conversion
        // below cannot truncate; the fallback is unreachable caution, not a
        // second rule.
        let count = parsed
            .integer(2)
            .and_then(|count| i32::try_from(count).ok())
            .unwrap_or(1);
        match self.give_player_item(id, item_id, count, report) {
            None => CommandResult::message("You are not online."),
            Some((placed, 0)) => {
                CommandResult::message(format!("Gave {placed} [{name}] to {}", parsed.source.name))
            }
            Some((placed, dropped)) => CommandResult::message(format!(
                "Gave {placed} [{name}] to {} ({dropped} dropped: inventory full)",
                parsed.source.name
            )),
        }
    }

    /// `/kill [target]`
    fn command_kill(
        &mut self,
        id: mc_network::bridge::ConnectionId,
        parsed: &mc_command::dispatch::ParsedCommand,
    ) -> CommandResult {
        let target = parsed.string(0).unwrap_or(&parsed.source.name);
        if !Self::targets_self(target, &parsed.source.name) {
            return CommandResult::message(format!(
                "Cannot kill {target:?}: this build only kills the invoking player"
            ));
        }
        match self.kill_player(id) {
            None => CommandResult::message("You are not online."),
            Some(outcome) if outcome.died => {
                CommandResult::message(format!("Killed {}", parsed.source.name))
            }
            Some(_) => CommandResult::message(format!("{} is already dead", parsed.source.name)),
        }
    }

    /// `/effect give <target> <effect> [seconds] [amplifier]` /
    /// `/effect clear [<target> [<effect>]]` (P16-03).
    ///
    /// The operator-visible source for status effects: no natural source
    /// exists in the tree yet (no eating, no witch, no beacon), so a command
    /// is what puts an effect on a player — the same way vanilla's command
    /// does. Like `give`/`kill`, only the invoking player may be targeted.
    /// Unmodelled effect names are refused with the modelled list rather
    /// than granted as dead icons.
    fn command_effect(
        &mut self,
        id: mc_network::bridge::ConnectionId,
        parsed: &mc_command::dispatch::ParsedCommand,
        report: &mut TickReport,
    ) -> CommandResult {
        let action = parsed.string(0).unwrap_or("");
        let target = parsed.string(1).unwrap_or(&parsed.source.name);
        if !Self::targets_self(target, &parsed.source.name) {
            return CommandResult::message(format!(
                "Cannot affect {target:?}: this build only targets the invoking player"
            ));
        }
        match action {
            "give" => self.command_effect_give(id, parsed, report),
            "clear" => self.command_effect_clear(id, parsed, report),
            _ => CommandResult::message(
                "Usage: /effect <give|clear> <target> [<effect> [seconds] [amplifier]]",
            ),
        }
    }

    /// `/effect give` arm: resolve, store, announce, confirm.
    fn command_effect_give(
        &mut self,
        id: mc_network::bridge::ConnectionId,
        parsed: &mc_command::dispatch::ParsedCommand,
        report: &mut TickReport,
    ) -> CommandResult {
        let Some(name) = parsed.string(2) else {
            return CommandResult::message(
                "Usage: /effect give <target> <effect> [seconds] [amplifier]",
            );
        };
        let Some(kind) = mc_entity::effect::EffectKind::from_name(name) else {
            return CommandResult::message(format!(
                "Unknown effect {name:?}: this build models speed, slowness, strength, weakness, resistance, poison, wither and regeneration"
            ));
        };
        let seconds = parsed.integer(3).unwrap_or(30).clamp(1, 1_000_000);
        let amplifier = i32::try_from(parsed.integer(4).unwrap_or(0).clamp(0, 255)).unwrap_or(0);
        let duration = seconds.saturating_mul(20).min(i64::from(i32::MAX));
        let duration = i32::try_from(duration).unwrap_or(i32::MAX);
        let Some(session) = self.sessions.get_mut(&id) else {
            return CommandResult::message("You are not online.");
        };
        session.player.give_effect(kind.id(), amplifier, duration);
        let packet = mc_protocol::packets::play::UpdateMobEffect {
            entity_id: session.entity.get(),
            effect_id: kind.wire_id(),
            amplifier,
            duration,
            flags: mc_protocol::packets::play::UpdateMobEffect::flags_for(false, true),
        };
        let _ = self.send(id, &packet, report);
        CommandResult::message(format!(
            "Gave {} {} ({}s) to {}",
            name, amplifier, seconds, parsed.source.name
        ))
    }

    /// `/effect clear` arm: one id or everything, each with its removal packet.
    fn command_effect_clear(
        &mut self,
        id: mc_network::bridge::ConnectionId,
        parsed: &mc_command::dispatch::ParsedCommand,
        report: &mut TickReport,
    ) -> CommandResult {
        let Some(session) = self.sessions.get_mut(&id) else {
            return CommandResult::message("You are not online.");
        };
        let entity_id = session.entity.get();
        let removed: Vec<i32> = if let Some(name) = parsed.string(2) {
            let Some(kind) = mc_entity::effect::EffectKind::from_name(name) else {
                return CommandResult::message(format!("Unknown effect {name:?}"));
            };
            session
                .player
                .clear_effect(kind.id())
                .then_some(kind.wire_id())
                .into_iter()
                .collect()
        } else {
            let ids: Vec<i32> = session.player.effects.keys().copied().collect();
            for effect_id in &ids {
                session.player.clear_effect(*effect_id);
            }
            // Stored ids ride the legacy table; only modelled kinds have a
            // known wire id, and an unmodelled icon is skipped rather than
            // mislabelled (see `wire_id_of`). Logged so the skip is visible
            // instead of silent (L-5).
            ids.into_iter()
                .filter_map(|effect_id| {
                    mc_entity::effect::wire_id_of(effect_id).or_else(|| {
                        debug!(
                            name = %parsed.source.name,
                            effect_id, "clear skips an effect with no wire id"
                        );
                        None
                    })
                })
                .collect()
        };
        for effect_id in &removed {
            let packet = mc_protocol::packets::play::RemoveMobEffect {
                entity_id,
                effect_id: *effect_id,
            };
            let _ = self.send(id, &packet, report);
        }
        CommandResult::message(format!(
            "Cleared {} effect(s) from {}",
            removed.len(),
            parsed.source.name
        ))
    }

    /// `/seed`
    fn command_seed(&self) -> CommandResult {
        CommandResult::message(format!("Seed: {}", self.random_seed()))
    }

    /// `/save-all [flush]` (P19-03): save now, unconditionally.
    ///
    /// Runs even with automatic saving held: it is an explicit operator
    /// action, and `flush` (the only accepted mode) names what the call
    /// already does — `save_all_owned` pushes dirty chunks and syncs, so
    /// the reply is sent only after data is on disk. Anything else as a
    /// mode is refused by name.
    fn command_save_all(&mut self, parsed: &mc_command::dispatch::ParsedCommand) -> CommandResult {
        if let Some(mode) = parsed.string(0)
            && mode != "flush"
        {
            return CommandResult::message(
                "Usage: /save-all [flush]: `flush` is the only mode".to_owned(),
            );
        }
        match self.save_all_owned() {
            Err(error) => CommandResult::message(format!("Save failed: {error}")),
            Ok(()) => CommandResult::message("Saved the world".to_owned()),
        }
    }

    /// `/save-off` (P19-03): hold automatic (autosave-tick) writes.
    ///
    /// Dirty flags are kept, so nothing is lost — the next `save-all`,
    /// `save-on` or shutdown save writes it all. Explicit saves and the
    /// shutdown save still write; only the timer holds.
    fn command_save_off(&mut self) -> CommandResult {
        self.set_saving_enabled(false);
        CommandResult::message("Automatic saving is now held".to_owned())
    }

    /// `/save-on` (P19-03): resume automatic writes.
    fn command_save_on(&mut self) -> CommandResult {
        self.set_saving_enabled(true);
        CommandResult::message("Automatic saving is resumed".to_owned())
    }
    /// `/difficulty [difficulty]`
    fn command_difficulty(
        &mut self,
        parsed: &mc_command::dispatch::ParsedCommand,
    ) -> CommandResult {
        use mc_persistence::level::Difficulty;
        fn display(difficulty: Difficulty) -> &'static str {
            match difficulty {
                Difficulty::Peaceful => "Peaceful",
                Difficulty::Easy => "Easy",
                Difficulty::Normal => "Normal",
                Difficulty::Hard => "Hard",
            }
        }
        let Some(word) = parsed.string(0) else {
            return CommandResult::message(format!(
                "The difficulty is {}",
                display(self.difficulty())
            ));
        };
        // Vanilla's four full names, case-sensitive like Vanilla's enum parser.
        let difficulty = match word {
            "peaceful" => Difficulty::Peaceful,
            "easy" => Difficulty::Easy,
            "normal" => Difficulty::Normal,
            "hard" => Difficulty::Hard,
            _ => {
                return CommandResult::message("Usage: /difficulty [peaceful|easy|normal|hard]");
            }
        };
        if self.difficulty_locked() {
            return CommandResult::message("The difficulty is locked and cannot be changed.");
        }
        if let Err(error) = self.set_difficulty(difficulty) {
            return CommandResult::message(format!("Could not save level.dat: {error}"));
        }
        CommandResult::message(format!("Set the difficulty to {}", display(difficulty)))
    }

    /// `/clear [<target> [<item> [<maxCount>]]]` (P18-02).
    ///
    /// Targets the invoking player only (same authority reason as `/give`).
    /// `maxCount == 0` is Vanilla's count-only form: it reports how many match
    /// and removes nothing.
    fn command_clear(
        &mut self,
        id: mc_network::bridge::ConnectionId,
        parsed: &mc_command::dispatch::ParsedCommand,
        report: &mut TickReport,
    ) -> CommandResult {
        let target = parsed.string(0).unwrap_or(&parsed.source.name);
        if !Self::targets_self(target, &parsed.source.name) {
            return CommandResult::message(format!(
                "Cannot clear {target:?}: this build only clears the invoking player"
            ));
        }
        let item_filter = parsed
            .argument(1)
            .and_then(|value| value.as_resource())
            .map(|id| format!("{id}"));
        let max_count = parsed.integer(2).and_then(|n| i32::try_from(n).ok());
        let count_only = max_count == Some(0);
        // Resolve names before the mutable session borrow: the registry borrow
        // and the inventory write cannot overlap.
        let mut matching: Vec<(usize, i32)> = Vec::new();
        {
            let Some(session) = self.sessions.get(&id) else {
                return CommandResult::message("You are not online.");
            };
            for slot in 0..session.player.inventory.stored_slots() {
                let stack = session.player.inventory.slot(slot);
                let Some(item_id) = stack.item_id() else {
                    continue;
                };
                let name = self
                    .registries()
                    .items
                    .name(item_id)
                    .map(str::to_owned)
                    .unwrap_or_default();
                if let Some(want) = &item_filter {
                    let want_bare = want.strip_prefix("minecraft:").unwrap_or(want);
                    let name_bare = name.strip_prefix("minecraft:").unwrap_or(&name);
                    if name_bare != want_bare && name != *want {
                        continue;
                    }
                }
                matching.push((slot, stack.count()));
            }
        }
        let mut removed = 0i32;
        // `maxCount == 0` is Vanilla's count-only form: it reports the total
        // match and removes nothing, so the budget is unlimited for counting.
        let mut budget = if count_only {
            i32::MAX
        } else {
            max_count.unwrap_or(i32::MAX)
        };
        for (slot, available) in matching {
            if budget <= 0 {
                break;
            }
            let take = available.min(budget);
            if count_only {
                removed += take;
                budget -= take;
                continue;
            }
            let Some(session) = self.sessions.get_mut(&id) else {
                break;
            };
            if let Ok(taken) = session.player.inventory.remove_from_slot(slot, take) {
                removed += taken.count();
                budget -= taken.count();
            }
        }
        if !count_only {
            self.sync_menu_from_inventory(id, report);
        }
        if removed == 0 {
            CommandResult::message(format!("No items were found on {}", parsed.source.name))
        } else if count_only {
            CommandResult::message(format!(
                "Found {removed} matching item(s) on {}",
                parsed.source.name
            ))
        } else {
            CommandResult::message(format!(
                "Removed {removed} item(s) from {}",
                parsed.source.name
            ))
        }
    }

    /// `/xp` / `/experience <action> <amount> [levels|points]` (P18-02).
    ///
    /// Actions: `add`, `set`, `query`. Unit defaults to `levels`. Only the
    /// invoking player is affected (same authority reason as `/give`).
    fn command_xp(
        &mut self,
        id: mc_network::bridge::ConnectionId,
        parsed: &mc_command::dispatch::ParsedCommand,
        report: &mut TickReport,
    ) -> CommandResult {
        let Some(action) = parsed.string(0) else {
            return CommandResult::message("Usage: /xp <add|set|query> [<amount>] [levels|points]");
        };
        let value = parsed.string(1);
        let unit_word = match action {
            // `query` takes the unit in the value slot: `/xp query levels`.
            "query" => value.or_else(|| parsed.string(2)),
            _ => parsed.string(2).or(Some("levels")),
        };
        let as_levels = match unit_word.unwrap_or("levels") {
            "levels" | "level" | "l" => true,
            "points" | "point" | "p" => false,
            other => {
                return CommandResult::message(format!(
                    "Unknown experience unit {other:?}: this build accepts levels or points"
                ));
            }
        };
        let Some(session) = self.sessions.get_mut(&id) else {
            return CommandResult::message("You are not online.");
        };
        match action {
            "query" => {
                let (value, label) = if as_levels {
                    (session.player.level, "levels")
                } else {
                    (session.player.total_experience, "points")
                };
                CommandResult::message(format!("{} has {value} {label}", parsed.source.name))
            }
            "add" | "set" => {
                let Some(amount_text) = value else {
                    return CommandResult::message("Usage: /xp <add|set> <amount> [levels|points]");
                };
                let Ok(amount) = amount_text.parse::<i32>() else {
                    return CommandResult::message(format!(
                        "{amount_text:?} is not an experience amount"
                    ));
                };
                if action == "add" {
                    if as_levels {
                        for _ in 0..amount.max(0) {
                            let needed = mc_entity::Player::experience_needed_for_level(
                                session.player.level,
                            );
                            session.player.add_experience(needed.max(1));
                        }
                        if amount < 0 {
                            session.player.level = (session.player.level + amount).max(0);
                        }
                    } else {
                        session.player.add_experience(amount);
                    }
                } else if as_levels {
                    session.player.level = amount.max(0);
                    session.player.experience = 0.0;
                } else {
                    session.player.reset_experience();
                    session.player.add_experience(amount.max(0));
                }
                let (level, total, progress) = (
                    session.player.level,
                    session.player.total_experience,
                    session.player.experience_progress(),
                );
                let packet = mc_protocol::packets::play::SetExperience {
                    progress,
                    level,
                    total,
                };
                let _ = self.send(id, &packet, report);
                CommandResult::message(format!(
                    "Set {}'s experience to {level} levels / {total} points",
                    parsed.source.name
                ))
            }
            other => CommandResult::message(format!(
                "Unknown experience action {other:?}: this build accepts add, set or query"
            )),
        }
    }

    /// `/enchant <enchantment> [<level>]` (P18-02 + P18-01b effects).
    ///
    /// Writes a `minecraft:enchantments` component onto the held stack.
    /// Efficiency/Sharpness/Protection/Unbreaking change behaviour; every
    /// other modelled name is stored and inert (named gap). Unmodelled names
    /// are refused with the modelled list.
    fn command_enchant(
        &mut self,
        id: mc_network::bridge::ConnectionId,
        parsed: &mc_command::dispatch::ParsedCommand,
        report: &mut TickReport,
    ) -> CommandResult {
        let Some(name) = parsed.string(0) else {
            return CommandResult::message("Usage: /enchant <enchantment> [level]");
        };
        let bare = name.strip_prefix("minecraft:").unwrap_or(name);
        let Some((_, registry_id, max_level)) =
            MODELLED_ENCHANTMENTS.iter().find(|(n, _, _)| *n == bare)
        else {
            return CommandResult::message(format!(
                "Unknown enchantment {name:?}: this build stores the vanilla set \
                 (sharpness, efficiency, unbreaking, protection, …); the four \
                 named effects apply and the rest are inert"
            ));
        };
        let level = parsed.integer(1).unwrap_or(1);
        let level = i32::try_from(level).unwrap_or(1);
        if level < 1 || level > *max_level {
            return CommandResult::message(format!(
                "Level {level} is out of range for {bare} (1..={max_level})"
            ));
        }
        let Some(session) = self.sessions.get_mut(&id) else {
            return CommandResult::message("You are not online.");
        };
        let slot = usize::from(session.player.inventory.selected_hotbar());
        let mut stack = session.player.inventory.slot(slot);
        if stack.is_empty() {
            return CommandResult::message("You must hold an item to enchant it.");
        }
        let mut entries: Vec<(i32, i32)> = stack
            .enchantments()
            .unwrap_or(&[])
            .iter()
            .copied()
            .filter(|(id, _)| *id != *registry_id)
            .collect();
        entries.push((*registry_id, level));
        entries.sort_unstable();
        stack.set_component(mc_entity::DataComponent::Enchantments(entries));
        if session
            .player
            .inventory
            .set_slot(slot, stack.clone())
            .is_err()
        {
            return CommandResult::message("Could not write the enchantment onto the held item.");
        }
        self.sync_menu_from_inventory(id, report);
        let active = matches!(
            bare,
            "efficiency" | "sharpness" | "protection" | "unbreaking"
        );
        let note = if active {
            "applied"
        } else {
            "stored and inert — named gap in the parity matrix"
        };
        CommandResult::message(format!(
            "Enchanted the held item with {bare} {level} ({note})"
        ))
    }

    /// `/setblock <pos> <block> [replace|destroy|keep]` (P18-02).
    fn command_setblock(&mut self, parsed: &mc_command::dispatch::ParsedCommand) -> CommandResult {
        let Some(mc_command::ArgumentValue::BlockPos { x, y, z }) = parsed.argument(0) else {
            return CommandResult::message("Usage: /setblock <pos> <block> [mode]");
        };
        let Some(block) = parsed.argument(1).and_then(|value| value.as_resource()) else {
            return CommandResult::message("Usage: /setblock <pos> <block> [mode]");
        };
        let mode = parsed.string(2).unwrap_or("replace");
        let Some(mode) = BlockWriteMode::parse(mode) else {
            return CommandResult::message(
                "Unknown setblock mode: this build accepts replace, destroy or keep \
                 (hollow/outline/filtered are refused)",
            );
        };
        let base = parsed.source.block_position().unwrap_or((0, 0, 0));
        let (bx, by, bz) = (x.resolve(base.0), y.resolve(base.1), z.resolve(base.2));
        let name = format!("{block}");
        let Ok(state) = self.registries().blocks.default_state(&name) else {
            return CommandResult::message(format!("Unknown block {name}"));
        };
        if !self.in_build_range(by) {
            return CommandResult::message(format!(
                "Cannot set a block at y={by}: outside the world's build range."
            ));
        }
        match mode.apply(self, bx, by, bz, state) {
            Ok(true) => CommandResult::message(format!("Set the block at {bx} {by} {bz}")),
            Ok(false) => {
                CommandResult::message(format!("No change at {bx} {by} {bz} (mode {mode})"))
            }
            Err(reason) => CommandResult::message(reason),
        }
    }

    /// The first chunk a `/fill` region spans that is **stored but
    /// unreadable**, in the order the write loop below would reach it
    /// (z-major, then x), or `None` when every chunk in the span reads.
    ///
    /// AUDIT-19's fix round (A-11): A-10 taught the per-cell write to refuse
    /// such a chunk, but that refusal fired **inside** the loop, so the cells
    /// before the bad chunk were already applied while the reply read as a
    /// refusal of the whole fill. Checking the region first is what makes the
    /// reply true — either every cell is written or none is. The load is the
    /// load-before-write ordering P19-08 installed (only a read can learn that
    /// a stored chunk is unreadable), pulled ahead of the first write; the
    /// loop's own [`Game::load_or_create_chunk`] calls then find each chunk
    /// loaded, and the span is already bounded by [`MAX_FILL_CHUNKS`]. The pin
    /// is `fill_into_a_region_with_an_unreadable_chunk_writes_nothing`.
    fn first_unreadable_chunk_in(
        &mut self,
        min_x: i32,
        max_x: i32,
        min_z: i32,
        max_z: i32,
    ) -> Option<ChunkPos> {
        for cz in (min_z >> 4)..=(max_z >> 4) {
            for cx in (min_x >> 4)..=(max_x >> 4) {
                let chunk = ChunkPos::new(cx, cz);
                self.load_or_create_chunk(chunk);
                if self.chunk_is_unreadable(chunk) {
                    return Some(chunk);
                }
            }
        }
        None
    }

    /// `/fill <from> <to> <block> [replace|destroy|keep]` (P18-02).
    ///
    /// Two named reds bound the work, in this order: the volume cap
    /// ([`MAX_FILL_VOLUME`]) and the chunk budget ([`MAX_FILL_CHUNKS`],
    /// AUDIT-19 A-09). The first alone let a `32768×1×1` bar load 2048 chunks
    /// on the tick thread while `persist.rs` promised reads stop at
    /// [`CHUNKS_PER_TICK`]. Both refusals name their limit.
    ///
    /// A third refusal — a chunk in the region that is **stored but
    /// unreadable** (AUDIT-19 A-10) — is checked for the whole region
    /// **before the first write** (A-11, [`Self::first_unreadable_chunk_in`]),
    /// so its "nothing was written" is a fact rather than an assumption.
    fn command_fill(&mut self, parsed: &mc_command::dispatch::ParsedCommand) -> CommandResult {
        let Some(mc_command::ArgumentValue::BlockPos {
            x: x0,
            y: y0,
            z: z0,
        }) = parsed.argument(0)
        else {
            return CommandResult::message("Usage: /fill <from> <to> <block> [mode]");
        };
        let Some(mc_command::ArgumentValue::BlockPos {
            x: x1,
            y: y1,
            z: z1,
        }) = parsed.argument(1)
        else {
            return CommandResult::message("Usage: /fill <from> <to> <block> [mode]");
        };
        let Some(block) = parsed.argument(2).and_then(|value| value.as_resource()) else {
            return CommandResult::message("Usage: /fill <from> <to> <block> [mode]");
        };
        let mode = parsed.string(3).unwrap_or("replace");
        let Some(mode) = BlockWriteMode::parse(mode) else {
            return CommandResult::message(
                "Unknown fill mode: this build accepts replace, destroy or keep \
                 (hollow/outline/filtered are refused)",
            );
        };
        let base = parsed.source.block_position().unwrap_or((0, 0, 0));
        let (ax, ay, az) = (x0.resolve(base.0), y0.resolve(base.1), z0.resolve(base.2));
        let (bx, by, bz) = (x1.resolve(base.0), y1.resolve(base.1), z1.resolve(base.2));
        let (min_x, max_x) = if ax <= bx { (ax, bx) } else { (bx, ax) };
        let (min_y, max_y) = if ay <= by { (ay, by) } else { (by, ay) };
        let (min_z, max_z) = if az <= bz { (az, bz) } else { (bz, az) };
        let volume = fill_volume(min_x, max_x, min_y, max_y, min_z, max_z);
        if !fill_volume_accepted(volume) {
            return CommandResult::message(format!(
                "The region is {volume} blocks, above the {MAX_FILL_VOLUME}-block limit"
            ));
        }
        // The block cap does not bound the chunk reads beneath it (A-09): a
        // `32768 0 0 → 32768 0 0` bar is one block tall and 2048 chunks wide.
        let chunks = fill_chunk_span(min_x, max_x, min_z, max_z);
        if !fill_chunk_span_accepted(chunks) {
            return CommandResult::message(format!(
                "The region spans {chunks} chunks, above the {MAX_FILL_CHUNKS}-chunk limit \
                 for a single fill (every cell's chunk is read before it is written)"
            ));
        }
        let name = format!("{block}");
        let Ok(state) = self.registries().blocks.default_state(&name) else {
            return CommandResult::message(format!("Unknown block {name}"));
        };
        if !self.in_build_range(min_y) || !self.in_build_range(max_y) {
            return CommandResult::message(
                "Cannot fill outside the world's build range.".to_owned(),
            );
        }
        // A-11: the region is checked **before the first write**, so the
        // refusal is true of the whole fill. The A-10 check inside
        // [`BlockWriteMode::apply`] stays — `setblock` needs it, and it is the
        // second line here for a chunk that somehow turns unreadable after
        // this scan — but a fill that reached it used to have applied the
        // cells before the bad chunk while the reply spoke as if nothing had.
        if let Some(chunk) = self.first_unreadable_chunk_in(min_x, max_x, min_z, max_z) {
            return CommandResult::message(format!(
                "Cannot fill the region: chunk {} {} is stored but unreadable, so nothing was \
                 written",
                chunk.x, chunk.z
            ));
        }
        let mut changed = 0i32;
        for y in min_y..=max_y {
            for z in min_z..=max_z {
                for x in min_x..=max_x {
                    match mode.apply(self, x, y, z, state) {
                        Ok(true) => changed += 1,
                        Ok(false) => {}
                        // Unreachable while the scan above holds — every chunk
                        // in the span is loaded and readable before the first
                        // write, with nothing running in between — and phrased
                        // so that it stays truthful if it ever is reached: the
                        // count is what was actually applied.
                        Err(reason) => {
                            return CommandResult::message(format!(
                                "Wrote {changed} block(s) before the fill was refused: {reason}"
                            ));
                        }
                    }
                }
            }
        }
        CommandResult::message(format!(
            "Filled {changed} block(s) out of {volume} in the region"
        ))
    }

    /// `/summon <type> [pos]` (P18-02).
    ///
    /// Only the eight modelled [`mc_entity::mob::MobKind`] names. Anything else
    /// is refused **by name** rather than spawned as a generic mob.
    fn command_summon(&mut self, parsed: &mc_command::dispatch::ParsedCommand) -> CommandResult {
        let Some(kind_text) = parsed.string(0) else {
            return CommandResult::message("Usage: /summon <type> [pos]");
        };
        let bare = kind_text.strip_prefix("minecraft:").unwrap_or(kind_text);
        let Some(kind) = mc_entity::mob::MobKind::from_name(bare) else {
            let modelled: Vec<&str> = mc_entity::mob::MobKind::ALL
                .iter()
                .map(|kind| kind.name())
                .collect();
            return CommandResult::message(format!(
                "Unknown entity {kind_text:?}: this build summons only {}",
                modelled.join(", ")
            ));
        };
        let origin = parsed.source.block_position().unwrap_or((0, 64, 0));
        let position =
            if let Some(mc_command::ArgumentValue::BlockPos { x, y, z }) = parsed.argument(1) {
                let (px, py, pz) = (
                    x.resolve(origin.0),
                    y.resolve(origin.1),
                    z.resolve(origin.2),
                );
                if !self.in_build_range(py) {
                    return CommandResult::message(format!(
                        "Cannot summon at y={py}: outside the world's build range."
                    ));
                }
                mc_world::Vec3::new(f64::from(px) + 0.5, f64::from(py), f64::from(pz) + 0.5)
            } else {
                let (px, py, pz) = origin;
                mc_world::Vec3::new(f64::from(px) + 0.5, f64::from(py), f64::from(pz) + 0.5)
            };
        match self.spawn_mob(kind, position) {
            Ok(_) => CommandResult::message(format!(
                "Summoned {} at {} {} {}",
                kind.name(),
                position.x.floor(),
                position.y.floor(),
                position.z.floor()
            )),
            Err(error) => CommandResult::message(format!("Could not summon: {error}")),
        }
    }

    /// `/setworldspawn [pos]` (P18-02).
    fn command_setworldspawn(
        &mut self,
        parsed: &mc_command::dispatch::ParsedCommand,
    ) -> CommandResult {
        let base = parsed.source.block_position().unwrap_or((0, 64, 0));
        let (spawn_x, spawn_y, spawn_z) =
            if let Some(mc_command::ArgumentValue::BlockPos { x, y, z }) = parsed.argument(0) {
                (x.resolve(base.0), y.resolve(base.1), z.resolve(base.2))
            } else {
                base
            };
        if !self.in_build_range(spawn_y) {
            return CommandResult::message(format!(
                "Cannot set the world spawn at y={spawn_y}: outside the world's build range."
            ));
        }
        self.world_mut().set_spawn(spawn_x, spawn_y, spawn_z);
        CommandResult::message(format!(
            "Set the world spawn to {spawn_x} {spawn_y} {spawn_z}"
        ))
    }

    /// `/msg` / `/tell` / `/w <target> <message>` (P18-02).
    fn command_msg(
        &mut self,
        id: mc_network::bridge::ConnectionId,
        parsed: &mc_command::dispatch::ParsedCommand,
        report: &mut TickReport,
    ) -> CommandResult {
        let Some(target) = parsed.string(0) else {
            return CommandResult::message("Usage: /msg <target> <message>");
        };
        let Some(message) = parsed.string(1) else {
            return CommandResult::message("Usage: /msg <target> <message>");
        };
        let Some(target_id) = self.session_id_by_name(target) else {
            return CommandResult::message(format!("No player was found matching {target:?}"));
        };
        let line = format!("{} whispers to you: {message}", parsed.source.name);
        if self
            .send(
                target_id,
                &mc_protocol::packets::play::DisguisedChat {
                    message: TextComponent::literal(line),
                    chat_type: crate::game::CHAT_TYPE_CHAT,
                    sender_name: TextComponent::literal("Server"),
                    target_name: None,
                },
                report,
            )
            .is_err()
        {
            return CommandResult::message("The whisper could not be delivered.");
        }
        let _ = id;
        CommandResult::message(format!("You whisper to {target}: {message}"))
    }

    /// `/me <action>` (P18-02).
    fn command_me(
        &mut self,
        parsed: &mc_command::dispatch::ParsedCommand,
        report: &mut TickReport,
    ) -> CommandResult {
        let Some(action) = parsed.string(0) else {
            return CommandResult::message("Usage: /me <action>");
        };
        let line = format!("* {} {action}", parsed.source.name);
        let ids: Vec<mc_network::bridge::ConnectionId> = self.sessions.keys().copied().collect();
        for target in ids {
            let _ = self.send(
                target,
                &mc_protocol::packets::play::DisguisedChat {
                    message: TextComponent::literal(line.clone()),
                    chat_type: crate::game::CHAT_TYPE_CHAT,
                    sender_name: TextComponent::literal("Server"),
                    target_name: None,
                },
                report,
            );
        }
        info!(from = %parsed.source.name, %action, "me");
        CommandResult::silent()
    }
}

/// How `setblock`/`fill` write a cell (P18-02).
///
/// Only the three Vanilla modes the closed list names. `hollow`, `outline` and
/// `filtered` are refused **by name** at parse time rather than mapped onto
/// one of these.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BlockWriteMode {
    /// Always write.
    Replace,
    /// Write only when the cell is air.
    Keep,
    /// Always write (drops are not modelled here; the tick's break path owns them).
    Destroy,
}

impl BlockWriteMode {
    fn parse(word: &str) -> Option<Self> {
        match word {
            "replace" => Some(Self::Replace),
            "keep" => Some(Self::Keep),
            "destroy" => Some(Self::Destroy),
            _ => None,
        }
    }

    /// Apply one cell. Returns whether the world changed.
    ///
    /// **The load comes first, and it is load-bearing** (P19-08, finding F3). A
    /// command names arbitrary coordinates, so the target chunk is often one no
    /// player has loaded. The world's answer to `set_block` in that case is the
    /// all-air placeholder `World::ensure_chunk` builds — and that placeholder is
    /// dirty by construction, so the next save writes it over the stored chunk:
    /// the terrain and every player edit in that chunk are gone. The ordering
    /// that prevents it lives in [`Game::load_or_create_chunk`] (already loaded →
    /// nothing; stored → read from disk; absent → generate; unreadable →
    /// placeholder, marked clean and never written back). Bypassing it here
    /// re-opens the AUDIT-09 B-01 data-loss class through the command path. The
    /// pin is `setblock_into_an_unloaded_chunk_keeps_the_stored_blocks`.
    ///
    /// It is also what makes `keep` mean anything for an unloaded chunk: with the
    /// real chunk read first, `get_block_loaded` answers with what is stored,
    /// where it used to answer `None` and the air test read that as "empty, so
    /// write".
    ///
    /// Cost: a wide `/fill` loads every chunk it spans, one read or generation
    /// each, on the tick thread — the same on-demand load Vanilla's command
    /// performs. **The span is budgeted by the caller** ([`MAX_FILL_CHUNKS`],
    /// AUDIT-19 A-09), so this per-cell load can no longer exceed what one
    /// tick's chunk reads are allowed to cost; the block cap alone would have
    /// allowed 2048 chunk reads from a 32 768-block bar.
    ///
    /// **A chunk that is stored but unreadable refuses the write** (AUDIT-19
    /// A-10). `load_or_create_chunk` answers that case with an all-air
    /// placeholder it deliberately never saves, so the write below would land,
    /// report success and be gone at the next restart — the same silent
    /// divergence from disk this doc's first paragraph is about, one step
    /// further out. The refusal names the chunk, because "which chunk" is the
    /// only part an operator can act on. The pin is
    /// `setblock_into_an_unreadable_chunk_is_refused_by_name`.
    fn apply(self, game: &mut Game, x: i32, y: i32, z: i32, state: i32) -> Result<bool, String> {
        let chunk = ChunkPos::new(x >> 4, z >> 4);
        game.load_or_create_chunk(chunk);
        if game.chunk_is_unreadable(chunk) {
            return Err(format!(
                "Cannot set the block at {x} {y} {z}: chunk {} {} is stored but unreadable, so \
                 nothing written there would survive a restart",
                chunk.x, chunk.z
            ));
        }
        let current = game.world().get_block_loaded(x, y, z);
        match self {
            Self::Keep => {
                if current.is_some_and(|id| id != 0) {
                    return Ok(false);
                }
            }
            Self::Replace | Self::Destroy => {}
        }
        game.world_mut()
            .set_block(x, y, z, state)
            .map(|change| change.is_some())
            .map_err(|error| format!("Could not set the block: {error}"))
    }
}

impl std::fmt::Display for BlockWriteMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Replace => "replace",
            Self::Keep => "keep",
            Self::Destroy => "destroy",
        })
    }
}

/// Whether a source kind may run commands at all.
///
/// A command block is a source kind the enum carries but nothing constructs, so this
/// exists to make the intent explicit rather than leaving `SourceKind` unread.
#[must_use]
pub const fn may_run_commands(kind: SourceKind) -> bool {
    match kind {
        SourceKind::Player | SourceKind::Console => true,
        // Recorded as false rather than true: a command block needs the block-entity
        // machinery that P06-07 does not yet run on a schedule.
        SourceKind::CommandBlock => false,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        MAX_FILL_CHUNKS, MAX_FILL_VOLUME, fill_chunk_span, fill_chunk_span_accepted, fill_volume,
        fill_volume_accepted,
    };

    #[test]
    fn fill_boundary_accepts_at_cap_and_refuses_above() {
        // E-15: the boundary the `>` comparison draws, without executing a
        // 32 768-block fill. Zeroing the cap fails the at-cap arm; flipping
        // to `>=` fails it too; removing the check fails the over-cap arm.
        assert_eq!(fill_volume(0, 31, 0, 31, 0, 31), MAX_FILL_VOLUME);
        assert!(fill_volume_accepted(MAX_FILL_VOLUME));
        assert!(fill_volume_accepted(0));
        assert!(!fill_volume_accepted(MAX_FILL_VOLUME + 1));
    }

    #[test]
    fn fill_chunk_span_counts_the_chunks_the_volume_hides() {
        // AUDIT-19 A-09: the volume cap and the chunk cost are different
        // questions. A one-block-tall bar across 4096 blocks is 4097 blocks
        // (well inside the volume cap) and 257 chunks (well outside the
        // per-tick budget this server promises).
        assert_eq!(fill_chunk_span(0, 0, 0, 0), 1);
        assert_eq!(fill_chunk_span(0, 15, 0, 15), 1);
        assert_eq!(fill_chunk_span(0, 16, 0, 0), 2);
        assert_eq!(fill_chunk_span(0, 4096, 0, 0), 257);
        assert!(fill_volume_accepted(fill_volume(0, 4096, 0, 0, 0, 0)));
        assert!(!fill_chunk_span_accepted(fill_chunk_span(0, 4096, 0, 0)));

        // The boundary itself, like the volume cap's: at the budget passes,
        // one chunk more is refused. Zeroing the budget fails the first arm;
        // flipping to `>=` fails it too; removing the check fails the second.
        assert!(fill_chunk_span_accepted(MAX_FILL_CHUNKS));
        assert!(!fill_chunk_span_accepted(MAX_FILL_CHUNKS + 1));
    }
}
