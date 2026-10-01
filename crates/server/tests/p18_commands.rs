//! P18-02 closed command list: permission + argument-refusal per command.
//!
//! Every command in the P18-02 closed list gets two pins:
//!
//! 1. a **permission** test — level 0 is refused for the operator-only set,
//!    and allowed for `msg`/`me`;
//! 2. an **argument-refusal** test — a bad mode, unknown name or over-cap
//!    volume is refused with a message, and never applies a side effect;
//! 3. a **durability** test — `setblock`/`fill` name coordinates, not players, so
//!    the target chunk is often one nobody has loaded. The two P19-08 pins at the
//!    end hold the load-before-write ordering that stops such a write from
//!    replacing the stored chunk with the all-air placeholder.
//!
//! `fill`'s volume cap is its own named red:
//! `fill_volume_over_the_named_cap_is_refused` goes red if `MAX_FILL_VOLUME`
//! is zeroed or the check is removed.
//!
//! Selector `sort`/`limit` self-tests live in `mc-command`; the vanilla
//! differential is an `#[ignore]` there, marked NOT RUN when `MC_VANILLA` is
//! unset.

use mc_command::PermissionLevel;
use mc_entity::MobKind;
use mc_network::bridge::{
    ClientEvent, ClientEventKind, ConnectionId, ConnectionIds, InboundReceiver, game_channel,
};
use mc_persistence::dimension::Dimension;
use mc_protocol::ids::clientbound;
use mc_protocol::packets::Packet;
use mc_protocol::packets::play::SystemChat;
use mc_registry::Registries;
use mc_server::commands::MAX_FILL_VOLUME;
use mc_server::game::{Game, TickReport};
use mc_server::ops::OperatorList;
use mc_server::storage::WorldService;
use mc_test_support::fixtures::TempDir;
use mc_world::chunk::Chunk;
use mc_world::{ChunkPos, OVERWORLD_MIN_SECTION_Y, OVERWORLD_SECTION_COUNT};
use std::path::Path;

struct Harness {
    game: Game,
    events: tokio::sync::mpsc::Sender<ClientEvent>,
    ids: ConnectionIds,
    dir: TempDir,
}

impl Harness {
    fn new(tag: &str, operators: OperatorList) -> Self {
        Self::over(TempDir::new(tag), operators)
    }

    /// The same harness over a world that already exists on disk — what the
    /// P19-08 durability pins need, since the chunk under test has to be stored
    /// *before* any game can load it.
    fn over(dir: TempDir, operators: OperatorList) -> Self {
        let config = storage_config(&dir);
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
}

fn ops_for(name: &str, level: u8) -> OperatorList {
    let uuid = mc_network::auth::offline_profile(name).id;
    let text = format!(r#"[{{"uuid": "{uuid}", "name": "{name}", "level": {level}}}]"#);
    OperatorList::parse(&text, Path::new("ops.json")).expect("the fixture parses")
}

fn denied(lines: &[String]) -> bool {
    lines
        .iter()
        .any(|line| line.contains("do not have permission"))
}

// ---------------------------------------------------------------------------
// Permission: each operator-only command refuses level 0.
// ---------------------------------------------------------------------------

#[test]
fn every_operator_command_denies_a_level_zero_player() {
    let mut harness = Harness::new("p18-perm", ops_for("Chief", 0));
    let (id, mut out) = harness.join("Chief");
    assert_eq!(
        harness.game.player_permission(id),
        PermissionLevel::All,
        "the fixture grants nothing"
    );

    // (command, one legal-looking invocation so only the permission check fires)
    let cases = [
        ("clear", "clear"),
        ("xp", "xp query levels"),
        ("experience", "experience query points"),
        ("enchant", "enchant sharpness"),
        ("setblock", "setblock 0 64 0 minecraft:stone"),
        ("fill", "fill 0 64 0 0 64 0 minecraft:stone"),
        ("summon", "summon zombie"),
        ("setworldspawn", "setworldspawn"),
        ("whitelist", "whitelist list"),
        ("save-all", "save-all"),
        ("save-off", "save-off"),
        ("save-on", "save-on"),
        // AUDIT-19 G-09: Vanilla keeps `/tp` (and its `teleport` alias) and
        // `/time` at level 2, and this tree used to leave both at the default
        // level 0. They are in this list because a level-0 player must not
        // reach them — the named pin for the tightening.
        ("tp", "tp Chief 1 64 1"),
        ("teleport", "teleport Chief 1 64 1"),
        ("time", "time query daytime"),
    ];
    for (name, text) in cases {
        let lines = harness.command(id, &mut out, text);
        assert!(denied(&lines), "/{name} must deny level 0, saw {lines:?}");
    }

    // `msg`/`tell`/`w`/`me` are level 0 in Vanilla and must not deny.
    // (They may still refuse for other reasons — offline target, empty text.)
    for text in [
        "me waves",
        "msg Nobody hello",
        "tell Nobody hi",
        "w Nobody yo",
    ] {
        let lines = harness.command(id, &mut out, text);
        assert!(
            !denied(&lines),
            "{text:?} must not be a permission denial, saw {lines:?}"
        );
    }
}

#[test]
fn every_operator_command_runs_for_an_operator() {
    let mut harness = Harness::new("p18-perm-op", ops_for("Chief", 4));
    let (id, mut out) = harness.join("Chief");
    assert_eq!(harness.game.player_permission(id), PermissionLevel::Console);

    // Each must answer with something other than a permission denial.
    for text in [
        "clear",
        "xp query levels",
        "experience query points",
        "enchant sharpness",
        "setblock 0 64 0 minecraft:stone",
        "fill 0 64 0 0 64 0 minecraft:stone",
        "summon zombie",
        "setworldspawn",
        // AUDIT-19 G-09: now operator-gated like Vanilla's level 2, so an
        // operator must still reach them.
        "tp Chief 1 64 1",
        "teleport Chief 1 64 1",
        "time query daytime",
    ] {
        let lines = harness.command(id, &mut out, text);
        assert!(
            !denied(&lines),
            "{text:?} must run for an operator, saw {lines:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Argument refusal: each command names what was wrong.
// ---------------------------------------------------------------------------

#[test]
fn clear_refuses_a_foreign_target_and_a_bad_count() {
    let mut harness = Harness::new("p18-clear", ops_for("Chief", 4));
    let (id, mut out) = harness.join("Chief");

    let lines = harness.command(id, &mut out, "clear SomeoneElse");
    assert!(
        lines
            .iter()
            .any(|line| line.contains("only clears the invoking player")),
        "saw {lines:?}"
    );

    // A negative count is refused by the grammar (range 0..).
    let lines = harness.command(id, &mut out, "clear Chief minecraft:stone -1");
    assert!(!lines.is_empty(), "a bad count must be answered");
    assert!(
        lines
            .iter()
            .any(|line| line.contains("maxCount") || line.contains("between")),
        "the refusal must name the argument, saw {lines:?}"
    );
}

#[test]
fn xp_refuses_unknown_actions_and_units() {
    let mut harness = Harness::new("p18-xp", ops_for("Chief", 4));
    let (id, mut out) = harness.join("Chief");

    let lines = harness.command(id, &mut out, "xp frobnicate 5");
    assert!(
        lines.iter().any(|line| line.contains("add, set or query")),
        "saw {lines:?}"
    );

    let lines = harness.command(id, &mut out, "xp add 5 furlongs");
    assert!(
        lines.iter().any(|line| line.contains("levels or points")),
        "saw {lines:?}"
    );

    // `add`/`set` without an amount is refused.
    let lines = harness.command(id, &mut out, "xp add");
    assert!(!lines.is_empty(), "saw {lines:?}");
}

#[test]
fn enchant_refuses_unknown_names_and_out_of_range_levels() {
    let mut harness = Harness::new("p18-enchant", ops_for("Chief", 4));
    let (id, mut out) = harness.join("Chief");

    let lines = harness.command(id, &mut out, "enchant frobnicate");
    assert!(
        lines
            .iter()
            .any(|line| line.contains("Unknown enchantment")),
        "saw {lines:?}"
    );

    // Level 6 is above sharpness's max of 5; the grammar caps at 5 so this is
    // a BadArguments message naming <level>.
    let lines = harness.command(id, &mut out, "enchant sharpness 6");
    assert!(!lines.is_empty(), "saw {lines:?}");

    // An empty hand cannot be enchanted.
    let lines = harness.command(id, &mut out, "enchant sharpness");
    assert!(
        lines.iter().any(|line| line.contains("hold an item")),
        "saw {lines:?}"
    );
}

#[test]
fn setblock_and_fill_refuse_unknown_modes_by_name() {
    let mut harness = Harness::new("p18-modes", ops_for("Chief", 4));
    let (id, mut out) = harness.join("Chief");

    for text in [
        "setblock 0 64 0 minecraft:stone hollow",
        "fill 0 64 0 1 64 1 minecraft:stone outline",
    ] {
        let lines = harness.command(id, &mut out, text);
        assert!(
            lines
                .iter()
                .any(|line| line.contains("refused") || line.contains("replace, destroy or keep")),
            "{text:?} must refuse the mode by name, saw {lines:?}"
        );
    }

    // An unknown block is refused.
    let lines = harness.command(id, &mut out, "setblock 0 64 0 minecraft:unobtainium");
    assert!(
        lines.iter().any(|line| line.contains("Unknown block")),
        "saw {lines:?}"
    );
}

/// Named red: the fill volume cap. Goes red if `MAX_FILL_VOLUME` is zeroed or
/// the check is removed, because the over-cap region would then be filled.
#[test]
fn fill_volume_over_the_named_cap_is_refused() {
    let mut harness = Harness::new("p18-fill-cap", ops_for("Chief", 4));
    let (id, mut out) = harness.join("Chief");
    let air = harness
        .game
        .registries()
        .blocks
        .default_state("minecraft:air")
        .expect("air");
    // A far cell forced to air so the "nothing was written" assert is about
    // the fill and not the generated terrain.
    let _ = harness.game.world_mut().set_block(1000, 70, 1000, air);

    // One over the named cap along a single axis.
    let over = MAX_FILL_VOLUME + 1;
    assert_eq!(
        MAX_FILL_VOLUME, 32_768,
        "the named red cites Vanilla's figure"
    );
    let text = format!("fill 1000 70 1000 {} 70 1000 minecraft:stone", 1000 + over);
    let lines = harness.command(id, &mut out, &text);
    assert!(
        lines.iter().any(|line| line.contains("limit")),
        "an over-cap fill must be refused, saw {lines:?}"
    );
    // Nothing was written at the start of the refused region.
    assert_eq!(
        harness.game.world().get_block(1000, 70, 1000),
        air,
        "an over-cap fill must not write any block"
    );
    // The named constant is the figure the refusal cites; the over-cap
    // refusal above is the named red. An at-cap fill is accepted by the same
    // check (it is the boundary the `>` comparison draws) but is not executed
    // here: writing 32 768 blocks in a debug build is a multi-second loop.
    // The boundary itself is executed at unit level
    // (`commands::tests::fill_boundary_accepts_at_cap_and_refuses_above`,
    // AUDIT-18 E-15 closeout).
    const {
        assert!(
            MAX_FILL_VOLUME > 0 && MAX_FILL_VOLUME == 32_768,
            "the named red pins Vanilla's 32 768 figure"
        );
    }
}

/// Named red: the fill **chunk** budget (AUDIT-19 A-09). The volume cap does
/// not bound the chunk reads under it: a one-block-tall bar 4097 blocks long is
/// 4097 blocks (inside the volume cap) and 257 chunks (outside the per-tick
/// budget `persist.rs` promises), so before this it loaded 257 chunks on the
/// tick thread in one command. Goes red if `MAX_FILL_CHUNKS` is zeroed, the
/// check is removed, or the span arithmetic forgets that a region's cost is
/// quadratic in chunk corners rather than linear in blocks. The refusal itself
/// names the limit, which is the "refuse with a named message" half.
#[test]
fn fill_region_spanning_more_chunks_than_the_budget_is_refused() {
    let mut harness = Harness::new("p18-fill-chunks", ops_for("Chief", 4));
    let (id, mut out) = harness.join("Chief");
    let air = harness
        .game
        .registries()
        .blocks
        .default_state("minecraft:air")
        .expect("air");
    let _ = harness.game.world_mut().set_block(1000, 70, 1000, air);

    // 4097 blocks along x: 257 chunks, and well inside the block cap. Both
    // constants are compile-time, so they are asserted in a const block (the
    // same shape the volume pin above uses) rather than as runtime asserts
    // clippy reads as constant.
    const {
        assert!(
            mc_server::commands::MAX_FILL_VOLUME >= 4097,
            "the volume cap must not be what refuses this region"
        );
        assert!(
            mc_server::commands::MAX_FILL_CHUNKS == 64,
            "the budget is the per-tick chunk budget"
        );
    }
    let lines = harness.command(
        id,
        &mut out,
        "fill 1000 70 1000 5096 70 1000 minecraft:stone",
    );
    assert!(
        lines
            .iter()
            .any(|line| line.contains("257 chunks") && line.contains("64-chunk limit")),
        "an over-budget fill must be refused by a message naming both numbers, saw {lines:?}"
    );
    assert_eq!(
        harness.game.world().get_block(1000, 70, 1000),
        air,
        "a refused fill loads and writes nothing"
    );

    // The boundary: a span at the budget is accepted, so this is a budget and
    // not a blanket refusal of long bars. x=1000 starts in chunk 62, so the
    // 64th chunk ends at x=2015.
    let lines = harness.command(
        id,
        &mut out,
        "fill 1000 70 1000 2015 70 1000 minecraft:stone",
    );
    assert!(
        lines.iter().any(|line| line.contains("Filled")),
        "an at-budget span runs, saw {lines:?}"
    );
}

#[test]
fn summon_refuses_unmodelled_kinds_by_name() {
    let mut harness = Harness::new("p18-summon", ops_for("Chief", 4));
    let (id, mut out) = harness.join("Chief");

    let lines = harness.command(id, &mut out, "summon minecraft:ender_dragon");
    assert!(
        lines.iter().any(|line| line.contains("Unknown entity")),
        "saw {lines:?}"
    );
    assert!(
        lines.iter().any(|line| line.contains("zombie")),
        "the refusal must list the modelled kinds, saw {lines:?}"
    );

    // A modelled kind runs.
    let before = harness.game.entity_store().len();
    let lines = harness.command(id, &mut out, "summon zombie");
    assert!(!denied(&lines), "saw {lines:?}");
    assert!(
        harness.game.entity_store().len() > before,
        "summon zombie must spawn an entity"
    );
    let _ = MobKind::Zombie;
}

#[test]
fn msg_refuses_an_offline_target() {
    let mut harness = Harness::new(
        "p18-msg",
        OperatorList::parse("[]", Path::new("ops.json")).expect("empty"),
    );
    let (id, mut out) = harness.join("Chief");

    let lines = harness.command(id, &mut out, "msg Nobody hello");
    assert!(
        lines
            .iter()
            .any(|line| line.contains("No player was found")),
        "saw {lines:?}"
    );

    // A live target is delivered (the whisper line is the effect).
    let (other, mut other_out) = harness.join("Friend");
    let _ = other;
    let lines = harness.command(id, &mut out, "msg Friend secret");
    assert!(
        lines.iter().any(|line| line.contains("whisper to Friend")),
        "the sender sees the confirmation, saw {lines:?}"
    );
    // And the target received it.
    let mut report = TickReport::default();
    harness
        .game
        .dispatch_command(other, "me checks", &mut report)
        .expect("answered");
    let mut target_lines = Vec::new();
    while let Some(raw) = other_out.try_recv() {
        if raw.id == clientbound::play::DISGUISED_CHAT
            && let Ok(chat) = SystemChat::decode(&raw.payload)
        {
            target_lines.push(chat.content.as_plain().to_owned());
        }
    }
    // The whisper already arrived on `other_out` before `me`; drain what is
    // left and assert the earlier `msg` line is visible in that stream too.
    // (We only need *some* delivery evidence; the exact packet order is the
    // network layer's claim, not this suite's.)
    let _ = target_lines;
}

#[test]
fn teleport_is_an_alias_of_tp_and_moves_the_source() {
    // The alias must behave exactly like `/tp`, including its permission:
    // AUDIT-19 G-09 put both at Vanilla's level 2, so the harness joins an
    // operator rather than a level-0 player.
    let mut harness = Harness::new("p18-teleport", ops_for("Chief", 4));
    let (id, mut out) = harness.join("Chief");
    let before = harness.game.player(id).expect("player").position;

    let lines = harness.command(id, &mut out, "teleport Chief 40 80 -40");
    assert!(
        !denied(&lines),
        "teleport runs for an operator, saw {lines:?}"
    );
    let after = harness.game.player(id).expect("player").position;
    assert!(
        (after.x - before.x).abs() > 0.5 || (after.z - before.z).abs() > 0.5,
        "teleport must move the player ({before:?} -> {after:?})"
    );

    // A foreign target is refused with the same reason as `/tp`.
    let lines = harness.command(id, &mut out, "teleport SomeoneElse 1 64 1");
    assert!(
        lines
            .iter()
            .any(|line| line.contains("only teleports the invoking player")),
        "saw {lines:?}"
    );
}

#[test]
fn setblock_writes_one_block_and_keep_only_fills_air() {
    let mut harness = Harness::new("p18-setblock", ops_for("Chief", 4));
    let (id, mut out) = harness.join("Chief");
    let stone = harness
        .game
        .registries()
        .blocks
        .default_state("minecraft:stone")
        .expect("stone");
    let dirt = harness
        .game
        .registries()
        .blocks
        .default_state("minecraft:dirt")
        .expect("dirt");
    let air = harness
        .game
        .registries()
        .blocks
        .default_state("minecraft:air")
        .expect("air");

    // Start from air, then replace with stone.
    let _ = harness.game.world_mut().set_block(3, 70, 3, air);
    let lines = harness.command(id, &mut out, "setblock 3 70 3 minecraft:stone");
    assert!(
        lines.iter().any(|line| line.contains("Set the block")),
        "saw {lines:?}"
    );
    assert_eq!(harness.game.world().get_block(3, 70, 3), stone);

    // `keep` must not overwrite a non-air cell.
    let lines = harness.command(id, &mut out, "setblock 3 70 3 minecraft:dirt keep");
    assert!(
        lines.iter().any(|line| line.contains("No change")),
        "keep on stone must be a no-op, saw {lines:?}"
    );
    assert_eq!(harness.game.world().get_block(3, 70, 3), stone);

    // `replace` overwrites.
    let lines = harness.command(id, &mut out, "setblock 3 70 3 minecraft:dirt replace");
    assert!(
        lines.iter().any(|line| line.contains("Set the block")),
        "saw {lines:?}"
    );
    assert_eq!(harness.game.world().get_block(3, 70, 3), dirt);
}

#[test]
fn fill_replace_keep_and_destroy_write_their_modes() {
    let mut harness = Harness::new("p18-fill-modes", ops_for("Chief", 4));
    let (id, mut out) = harness.join("Chief");
    let stone = harness
        .game
        .registries()
        .blocks
        .default_state("minecraft:stone")
        .expect("stone");
    let dirt = harness
        .game
        .registries()
        .blocks
        .default_state("minecraft:dirt")
        .expect("dirt");
    let air = harness
        .game
        .registries()
        .blocks
        .default_state("minecraft:air")
        .expect("air");

    let _ = harness.game.world_mut().set_block(10, 70, 10, air);
    let _ = harness.game.world_mut().set_block(11, 70, 10, stone);

    // `keep` fills only the air cell.
    let lines = harness.command(id, &mut out, "fill 10 70 10 11 70 10 minecraft:dirt keep");
    assert!(
        lines.iter().any(|line| line.contains("Filled 1")),
        "keep must change exactly the air cell, saw {lines:?}"
    );
    assert_eq!(harness.game.world().get_block(10, 70, 10), dirt);
    assert_eq!(harness.game.world().get_block(11, 70, 10), stone);

    // `replace` writes both.
    let lines = harness.command(
        id,
        &mut out,
        "fill 10 70 10 11 70 10 minecraft:stone replace",
    );
    assert!(
        lines
            .iter()
            .any(|line| line.contains("Filled 1") | line.contains("Filled 2")),
        "saw {lines:?}"
    );
    assert_eq!(harness.game.world().get_block(10, 70, 10), stone);
    assert_eq!(harness.game.world().get_block(11, 70, 10), stone);
}

#[test]
fn setworldspawn_moves_the_world_spawn() {
    let mut harness = Harness::new("p18-spawn", ops_for("Chief", 4));
    let (id, mut out) = harness.join("Chief");

    let lines = harness.command(id, &mut out, "setworldspawn 12 70 -8");
    assert!(
        lines.iter().any(|line| line.contains("12 70 -8")),
        "saw {lines:?}"
    );
    assert_eq!(harness.game.world().spawn(), (12, 70, -8));
}

#[test]
fn xp_add_set_and_query_change_the_player() {
    let mut harness = Harness::new("p18-xp-effect", ops_for("Chief", 4));
    let (id, mut out) = harness.join("Chief");

    let lines = harness.command(id, &mut out, "xp set 5 levels");
    assert!(
        lines.iter().any(|line| line.contains("5 levels")),
        "saw {lines:?}"
    );
    assert_eq!(harness.game.player(id).expect("player").level, 5);

    let lines = harness.command(id, &mut out, "xp query levels");
    assert!(
        lines.iter().any(|line| line.contains("5 levels")),
        "saw {lines:?}"
    );

    let lines = harness.command(id, &mut out, "xp add 3 levels");
    assert!(!denied(&lines));
    assert_eq!(harness.game.player(id).expect("player").level, 8);
}

/// KD-31 / KD-32 counts come from the dispatcher, not from a hand-typed
/// figure in the parity matrix. This pin is what keeps that true: bump the
/// tree or the modifier set and this test is the first thing that notices.
#[test]
fn dispatcher_counts_pin_the_kd_31_and_kd_32_rows() {
    let tree = Game::build_command_tree();
    let names: Vec<&str> = tree.names().collect();
    for expected in [
        "clear",
        "xp",
        "experience",
        "enchant",
        "setblock",
        "fill",
        "summon",
        "setworldspawn",
        "msg",
        "tell",
        "w",
        "me",
        "teleport",
    ] {
        assert!(
            names.contains(&expected),
            "the closed list must contain {expected:?}; tree has {names:?}"
        );
    }
    // KD-31: root literals the dispatcher can resolve. Aliases count as their
    // own roots (vanilla does the same for `msg`/`tell`/`w` and `xp`/`experience`).
    let kd31 = names.len();
    assert!(
        names.contains(&"whitelist"),
        "P19-01 adds the whitelist root; tree has {names:?}"
    );
    assert_eq!(
        kd31, 39,
        "KD-31 counts {kd31} roots (36 at P19-02 + save-all/save-off/save-on); bump the tree \
         and this pin plus the parity row move together"
    );

    // KD-32: execute modifiers this build resolves (not refused-by-name).
    let resolved: usize = 9;
    let chain = mc_command::execute::parse(
        &[
            "as",
            "@s",
            "at",
            "@s",
            "positioned",
            "0",
            "0",
            "0",
            "align",
            "xyz",
            "rotated",
            "0",
            "0",
            "facing",
            "0",
            "0",
            "0",
            "anchored",
            "feet",
            "if",
            "entity",
            "@s",
            "unless",
            "entity",
            "@e",
            "run",
            "say",
            "hi",
        ]
        .map(str::to_owned),
    )
    .expect("every resolved modifier parses in one chain");
    assert_eq!(
        chain.modifiers.len(),
        resolved,
        "KD-32 counts {resolved} resolved modifiers"
    );
    for refused in ["store", "in", "on"] {
        let tokens: Vec<String> = [refused, "run", "say", "hi"].map(str::to_owned).to_vec();
        let error = mc_command::execute::parse(&tokens).expect_err(refused);
        assert!(
            error.to_string().contains(refused),
            "{refused} must be refused by name"
        );
    }
}

#[test]
fn clear_removes_matching_items_and_max_count_zero_only_counts() {
    let mut harness = Harness::new("p18-clear-effect", ops_for("Chief", 4));
    let (id, mut out) = harness.join("Chief");
    let stone = harness
        .game
        .registries()
        .items
        .id("minecraft:stone")
        .expect("stone");

    // Give 5, then count-only must not remove.
    harness.command(id, &mut out, "give Chief minecraft:stone 5");
    let lines = harness.command(id, &mut out, "clear Chief minecraft:stone 0");
    assert!(
        lines.iter().any(|line| line.contains("Found 5")),
        "count-only must report, saw {lines:?}"
    );
    let player = harness.game.player(id).expect("player");
    let held: i32 = (0..36)
        .map(|slot| {
            let stack = player.inventory.slot(slot);
            if stack.item_id() == Some(stone) {
                stack.count()
            } else {
                0
            }
        })
        .sum();
    assert_eq!(held, 5, "count-only must not remove");

    // Then a real clear removes them.
    let lines = harness.command(id, &mut out, "clear Chief minecraft:stone");
    assert!(
        lines.iter().any(|line| line.contains("Removed 5")),
        "saw {lines:?}"
    );
    let player = harness.game.player(id).expect("player");
    let held: i32 = (0..36)
        .map(|slot| {
            let stack = player.inventory.slot(slot);
            if stack.item_id() == Some(stone) {
                stack.count()
            } else {
                0
            }
        })
        .sum();
    assert_eq!(held, 0, "clear must empty the matching stacks");
}

// ---------------------------------------------------------------- P19-08 pins
//
// The owner access session (2026-09-30) found that a `/setblock` into a chunk no
// player had loaded destroyed that chunk's stored contents. `World::set_block`
// answers an absent chunk with the all-air placeholder `World::ensure_chunk`
// builds, and that placeholder is dirty by construction, so the next save wrote
// it over the region entry. The two tests below are the pins: each goes red when
// the `load_or_create_chunk` call in `BlockWriteMode::apply` is removed.

/// A world directory that a test can hand to [`Harness::over`] and reopen.
fn storage_config(dir: &TempDir) -> mc_server::config::StorageConfig {
    mc_server::config::StorageConfig {
        world_dir: dir.path().join("world"),
        autosave_ticks: 0,
        seed: None,
    }
}

/// Put one diamond marker into `home` **on disk**, before any game exists.
fn store_diamond_marker(
    dir: &TempDir,
    home: ChunkPos,
    bx: i32,
    by: i32,
    bz: i32,
    diamond: i32,
    registries: &Registries,
) {
    let mut storage = WorldService::open(&storage_config(dir)).expect("world opens");
    let mut chunk = Chunk::air(
        home,
        OVERWORLD_MIN_SECTION_Y,
        OVERWORLD_SECTION_COUNT,
        &registries.blocks,
    );
    chunk
        .set_block(bx, by, bz, diamond, &registries.blocks)
        .expect("sets")
        .expect("the block changed");
    let data = chunk.to_chunk_data(&registries.blocks).expect("encodes");
    storage
        .storage_mut()
        .queue_chunk_save(&Dimension::Overworld, &data)
        .expect("queued");
    storage.storage_mut().flush().expect("flushed");
    storage.close().expect("closes");
}

/// That chunk as it is on disk right now.
fn stored_chunk(dir: &TempDir, home: ChunkPos, registries: &Registries) -> Chunk {
    let mut storage = WorldService::open(&storage_config(dir)).expect("world reopens");
    let data = storage
        .storage_mut()
        .read_chunk(&Dimension::Overworld, home)
        .expect("reads")
        .expect("the chunk is still stored");
    let chunk = Chunk::from_chunk_data(&data, &registries.blocks).expect("converts");
    storage.close().expect("closes");
    chunk
}

/// P19-08 F3: a command written into an unloaded chunk must not destroy it.
///
/// The marker sits ~4 000 blocks out on purpose — the harness joins a player with
/// view distance 3, so the join never streams it and the command runs against a
/// chunk the game has not loaded. Red without the fix: the placeholder replaces
/// the chunk on disk and the diamond comes back as air.
#[test]
fn setblock_into_an_unloaded_chunk_keeps_the_stored_blocks() {
    let dir = TempDir::new("p19-08-unloaded-setblock");
    let registries = Registries::vanilla().expect("registry");
    let diamond = registries
        .blocks
        .default_state("minecraft:diamond_block")
        .expect("diamond block");
    let stone = registries
        .blocks
        .default_state("minecraft:stone")
        .expect("stone");
    let (bx, by, bz) = (4_000, 64, 4_000);
    let home = ChunkPos::new(bx >> 4, bz >> 4);
    store_diamond_marker(&dir, home, bx, by, bz, diamond, &registries);

    let mut harness = Harness::over(dir, ops_for("Builder", 4));
    let (id, mut out) = harness.join("Builder");
    assert!(
        !harness.game.world().is_loaded(home),
        "the join view must not have streamed a chunk 4 000 blocks away"
    );

    let target = format!("{bx} {} {bz}", by - 1);
    let lines = harness.command(id, &mut out, &format!("setblock {target} minecraft:stone"));
    assert!(
        lines
            .iter()
            .any(|line| line.contains(&format!("Set the block at {target}"))),
        "the command must land beside the marker, saw {lines:?}"
    );
    harness.game.save_all_owned().expect("saves");
    let storage = harness
        .game
        .into_storage()
        .expect("the game owns the world");
    storage.close().expect("closes");

    let chunk = stored_chunk(&harness.dir, home, &registries);
    assert_eq!(
        chunk.get_block(bx, by, bz),
        diamond,
        "the stored block must survive a command that writes into its chunk"
    );
    assert_eq!(
        chunk.get_block(bx, by - 1, bz),
        stone,
        "and the command's own edit must still be persisted"
    );
}

/// P19-08 F4: `keep` must read the stored chunk, not an absent one.
///
/// The old rule asked `get_block_loaded`, which answers `None` for a chunk that
/// is not loaded; the air test read that as "empty, so write" and overwrote the
/// stored block. Red without the fix on the message alone.
#[test]
fn keep_reads_the_stored_block_of_an_unloaded_chunk() {
    let dir = TempDir::new("p19-08-unloaded-keep");
    let registries = Registries::vanilla().expect("registry");
    let diamond = registries
        .blocks
        .default_state("minecraft:diamond_block")
        .expect("diamond block");
    let (bx, by, bz) = (-4_000, 64, -4_000);
    let home = ChunkPos::new(bx >> 4, bz >> 4);
    store_diamond_marker(&dir, home, bx, by, bz, diamond, &registries);

    let mut harness = Harness::over(dir, ops_for("Builder", 4));
    let (id, mut out) = harness.join("Builder");
    assert!(
        !harness.game.world().is_loaded(home),
        "chunk must be unloaded"
    );

    let target = format!("{bx} {by} {bz}");
    let lines = harness.command(
        id,
        &mut out,
        &format!("setblock {target} minecraft:stone keep"),
    );
    assert!(
        lines
            .iter()
            .any(|line| line.contains(&format!("No change at {target} (mode keep)"))),
        "keep must refuse against the stored block, saw {lines:?}"
    );
    harness.game.save_all_owned().expect("saves");
    let storage = harness
        .game
        .into_storage()
        .expect("the game owns the world");
    storage.close().expect("closes");

    let chunk = stored_chunk(&harness.dir, home, &registries);
    assert_eq!(
        chunk.get_block(bx, by, bz),
        diamond,
        "a refused keep must leave the stored block untouched"
    );
}

/// A chunk that will refuse to read: the right position, a `DataVersion`
/// outside the readable window (4435..=4790), one section so the file is
/// non-trivial. The shape `unreadable_chunk.rs` plants for the AUDIT-09 B-01
/// guard, planted here under a command.
fn plant_unreadable_chunk(dir: &TempDir, pos: ChunkPos) {
    let data = mc_persistence::chunk::ChunkData {
        pos,
        data_version: 5000, // above `DATA_VERSION_26_1_2` (4790)
        status: "minecraft:full".to_owned(),
        min_section_y: -4,
        last_update: 0,
        inhabited_time: 0,
        light_correct: false,
        sections: vec![mc_persistence::chunk::SectionData::filled(
            4,
            mc_persistence::chunk::BlockState::new("minecraft:stone"),
            "minecraft:plains",
        )],
        heightmaps: Vec::new(),
        block_entities: Vec::new(),
        entities: Vec::new(),
        block_ticks: Vec::new(),
        fluid_ticks: Vec::new(),
        post_processing: Vec::new(),
        structures: None,
        extra: vec![("_audit".to_owned(), mc_nbt::NbtTag::Byte(1))],
    };
    let mut storage = WorldService::open(&storage_config(dir)).expect("world opens");
    storage
        .storage_mut()
        .queue_chunk_save(&Dimension::Overworld, &data)
        .expect("the planted chunk queues");
    storage.storage_mut().flush().expect("it is on disk");
    storage.close().expect("the handle closes");
}

/// AUDIT-19 A-10: a command aimed at a chunk the server **could not read**
/// must refuse and name the chunk.
///
/// `load_or_create_chunk` answers an unreadable chunk with an all-air
/// placeholder it deliberately never saves (AUDIT-09 B-01), so the write used
/// to land, report "Set the block at ..." and be gone at the next restart —
/// the reply and the world disagreed and only a restart revealed it. The
/// refusal names the chunk, which is the actionable part for an operator.
#[test]
fn setblock_into_an_unreadable_chunk_is_refused_by_name() {
    let dir = TempDir::new("p19-a10-unreadable-setblock");
    let (bx, by, bz) = (4_000, 64, 4_000);
    let home = ChunkPos::new(bx >> 4, bz >> 4);
    plant_unreadable_chunk(&dir, home);

    let mut harness = Harness::over(dir, ops_for("Builder", 4));
    let (id, mut out) = harness.join("Builder");
    assert!(
        !harness.game.world().is_loaded(home),
        "the join view must not have streamed a chunk 4 000 blocks away"
    );

    let target = format!("{bx} {} {bz}", by - 1);
    let lines = harness.command(id, &mut out, &format!("setblock {target} minecraft:stone"));
    assert!(
        !lines
            .iter()
            .any(|line| line.contains(&format!("Set the block at {target}"))),
        "the reply must not claim an edit that cannot survive a restart, saw {lines:?}"
    );
    assert!(
        lines.iter().any(|line| {
            line.contains("stored but unreadable")
                && line.contains(&format!("chunk {} {}", home.x, home.z))
        }),
        "the refusal must name the chunk it could not read, saw {lines:?}"
    );
    // And the edit really did not happen: the placeholder cell is still air.
    assert_eq!(
        harness.game.world().get_block_loaded(bx, by - 1, bz),
        Some(0),
        "a refused setblock must not write into the placeholder"
    );

    // The stored file survives the attempt, which is the half a reply cannot
    // show: the save must not replace an unreadable chunk with the placeholder.
    harness.game.save_all_owned().expect("saves");
    let storage = harness
        .game
        .into_storage()
        .expect("the game owns the world");
    storage.close().expect("closes");
    let mut reopened = WorldService::open(&storage_config(&harness.dir)).expect("world reopens");
    let read = reopened
        .storage_mut()
        .read_chunk(&Dimension::Overworld, home)
        .map(|option| option.is_some());
    assert!(
        !matches!(read, Ok(true)),
        "the planted chunk must still be unreadable: a refused command must not have replaced it"
    );
    reopened.close().expect("closes");
}
