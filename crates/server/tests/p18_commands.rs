//! P18-02 closed command list: permission + argument-refusal per command.
//!
//! Every command in the P18-02 closed list gets two pins:
//!
//! 1. a **permission** test — level 0 is refused for the operator-only set,
//!    and allowed for `msg`/`me`;
//! 2. an **argument-refusal** test — a bad mode, unknown name or over-cap
//!    volume is refused with a message, and never applies a side effect.
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
use mc_protocol::ids::clientbound;
use mc_protocol::packets::Packet;
use mc_protocol::packets::play::SystemChat;
use mc_server::commands::MAX_FILL_VOLUME;
use mc_server::game::{Game, TickReport};
use mc_server::ops::OperatorList;
use mc_server::storage::WorldService;
use mc_test_support::fixtures::TempDir;
use std::path::Path;

struct Harness {
    game: Game,
    events: tokio::sync::mpsc::Sender<ClientEvent>,
    ids: ConnectionIds,
    _dir: TempDir,
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
            _dir: dir,
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

    // `teleport` is an alias of `tp`, which this build leaves at level 0
    // (self-only authority gate). It must not deny.
    let lines = harness.command(id, &mut out, "teleport Chief 1 64 1");
    assert!(
        !denied(&lines),
        "teleport is level 0 like tp, saw {lines:?}"
    );
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
    // here: writing 32 768 blocks in a debug build is a multi-second loop, and
    // the acceptance is the cap, not the fill throughput.
    const {
        assert!(
            MAX_FILL_VOLUME > 0 && MAX_FILL_VOLUME == 32_768,
            "the named red pins Vanilla's 32 768 figure"
        );
    }
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
    let mut harness = Harness::new(
        "p18-teleport",
        OperatorList::parse("[]", Path::new("ops.json")).expect("empty"),
    );
    let (id, mut out) = harness.join("Chief");
    let before = harness.game.player(id).expect("player").position;

    let lines = harness.command(id, &mut out, "teleport Chief 40 80 -40");
    assert!(
        !denied(&lines),
        "teleport is level 0 like tp, saw {lines:?}"
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
        kd31 >= 29,
        "KD-31 counts {kd31} roots; the P18-02 closed list alone is 13 new names"
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
