//! Regression pins for the P17 owner-session fixes (AUDIT-17 Lane E).
//!
//! Each test is the named detector for one historical fix: creative slot
//! writes, double-door pairing from the lower half, button pulse (no sticky
//! hold), and sneak-place via `player_input` bit 5. Falsification: neutralise
//! the matching arm in `session.rs` / `mod.rs` and exactly one of these goes
//! red.

#![allow(clippy::float_cmp)]
#![allow(clippy::cast_possible_truncation)]

use mc_entity::GameMode;
use mc_entity::stack::ItemStack as EntityStack;
use mc_network::bridge::{
    ClientEvent, ClientEventKind, ConnectionId, InboundReceiver, OutboundSender, game_channel,
};
use mc_protocol::ids::clientbound;
use mc_protocol::packets::Packet;
use mc_protocol::packets::play::ItemStack as WireStack;
use mc_protocol::packets::play::{PlayIntent, block_position};
use mc_server::game::Game;
use mc_server::storage::WorldService;
use mc_test_support::fixtures::TempDir;
use mc_world::ChunkPos;

struct Harness {
    game: Game,
    events: tokio::sync::mpsc::Sender<ClientEvent>,
    id: ConnectionId,
    _dir: TempDir,
}

impl Harness {
    fn new(tag: &str) -> Self {
        let dir = TempDir::new(tag);
        let config = mc_server::config::StorageConfig {
            world_dir: dir.path().join("world"),
            autosave_ticks: 0,
            seed: None,
        };
        let storage = WorldService::open(&config).expect("world opens");
        let (tx, rx) = game_channel(256);
        let game =
            Game::with_seed_and_storage(storage, 4, rx, mc_server::game::DEFAULT_RANDOM_SEED)
                .expect("game builds");
        Self {
            game,
            events: tx,
            id: ConnectionId(1),
            _dir: dir,
        }
    }

    fn join(&mut self, name: &str) -> InboundReceiver {
        let (outbound, out) = OutboundSender::pair(self.id, 8192);
        self.events
            .try_send(ClientEvent {
                id: self.id,
                kind: ClientEventKind::Joined {
                    profile: mc_network::auth::offline_profile(name),
                    outbound,
                },
            })
            .expect("join event queued");
        self.game.tick().expect("tick");
        out
    }

    fn intent(&mut self, intent: PlayIntent) {
        self.events
            .try_send(ClientEvent {
                id: self.id,
                kind: ClientEventKind::Intent(intent),
            })
            .expect("intent queued");
        self.game.tick().expect("tick");
    }

    fn run(&mut self, ticks: usize) {
        for _ in 0..ticks {
            self.game.tick().expect("tick");
        }
    }

    fn item_id(&self, name: &str) -> i32 {
        self.game
            .registries()
            .items
            .id(name)
            .unwrap_or_else(|_| panic!("{name} is a known item"))
    }

    fn give(&mut self, item: &str) {
        let id = self.item_id(item);
        self.game
            .player_mut(self.id)
            .expect("player")
            .inventory
            .set_slot(0, EntityStack::new(id, 1).expect("stack"))
            .expect("hotbar 0 takes it");
    }

    fn empty_hand(&mut self) {
        self.game
            .player_mut(self.id)
            .expect("player")
            .inventory
            .set_slot(0, EntityStack::EMPTY)
            .expect("hand emptied");
    }

    fn look(&mut self, yaw: f32) {
        self.game.player_mut(self.id).expect("player").yaw = yaw;
    }

    fn set_game_mode(&mut self, mode: GameMode) {
        self.game.player_mut(self.id).expect("player").game_mode = mode;
    }

    fn inv_count(&self, name: &str) -> i32 {
        let id = self.item_id(name);
        let inv = &self.game.player(self.id).expect("player").inventory;
        (0..41)
            .map(|slot| {
                let stack = inv.slot(slot);
                if stack.item_id() == Some(id) {
                    stack.count()
                } else {
                    0
                }
            })
            .sum()
    }

    fn click(&mut self, x: i32, y: i32, z: i32, face: i32) {
        self.intent(PlayIntent::UseItemOn {
            hand: 0,
            position: block_position(x, y, z),
            face,
            cursor_x: 0.5,
            cursor_y: 0.5,
            cursor_z: 0.5,
            inside_block: false,
            world_border_hit: false,
            sequence: 0,
        });
    }

    /// Click with an explicit cursor (door hinge tiebreak).
    fn click_cursor(&mut self, x: i32, y: i32, z: i32, face: i32, cursor: (f32, f32, f32)) {
        self.intent(PlayIntent::UseItemOn {
            hand: 0,
            position: block_position(x, y, z),
            face,
            cursor_x: cursor.0,
            cursor_y: cursor.1,
            cursor_z: cursor.2,
            inside_block: false,
            world_border_hit: false,
            sequence: 0,
        });
    }

    fn prop(&self, x: i32, y: i32, z: i32, key: &str) -> String {
        let id = self.game.world().get_block_loaded(x, y, z).expect("loaded");
        self.game
            .registries()
            .blocks
            .properties_of(id)
            .expect("props read")
            .iter()
            .find(|(k, _)| k == key)
            .map_or_else(|| panic!("{key} present"), |(_, v)| v.clone())
    }

    fn name(&self, x: i32, y: i32, z: i32) -> String {
        let id = self.game.world().get_block_loaded(x, y, z).expect("loaded");
        self.game
            .registries()
            .blocks
            .block_name(id)
            .expect("named")
            .to_owned()
    }

    fn set_block(&mut self, x: i32, y: i32, z: i32, name: &str) {
        let id = self
            .game
            .registries()
            .blocks
            .default_state(name)
            .unwrap_or_else(|_| panic!("{name} is a known block"));
        self.game.load_chunk(ChunkPos::new(x >> 4, z >> 4));
        self.game
            .world_mut()
            .set_block(x, y, z, id)
            .expect("block set");
    }
}

fn floor(harness: &mut Harness, sx: i32, sy: i32, sz: i32) {
    let stone = harness
        .game
        .registries()
        .blocks
        .default_state("minecraft:stone")
        .expect("stone");
    let air = harness.game.registries().blocks.air_id();
    for x in (sx - 4)..=(sx + 8) {
        for z in (sz - 3)..=(sz + 3) {
            harness.game.load_chunk(ChunkPos::new(x >> 4, z >> 4));
            harness
                .game
                .world_mut()
                .set_block(x, sy - 1, z, stone)
                .expect("floor");
            for y in [sy, sy + 1, sy + 2] {
                harness
                    .game
                    .world_mut()
                    .set_block(x, y, z, air)
                    .expect("cleared");
            }
        }
    }
}

/// c66f1d2: creative takes land in the inventory; survival writes are refused.
#[test]
fn creative_take_writes_hotbar_and_survival_is_refused() {
    let mut harness = Harness::new("p17-pin-creative");
    harness.join("Architect");
    harness.set_game_mode(GameMode::Creative);
    let stick = harness.item_id("minecraft:stick");
    // Window slot 36 is hotbar 0.
    harness.intent(PlayIntent::SetCreativeModeSlot {
        slot: 36,
        item: WireStack {
            item_id: stick,
            count: 5,
            components: Vec::new(),
        },
    });
    assert_eq!(
        harness.inv_count("minecraft:stick"),
        5,
        "creative take lands"
    );
    assert_eq!(
        harness
            .game
            .player(harness.id)
            .expect("player")
            .inventory
            .slot(0)
            .count(),
        5
    );

    // Survival must refuse the same write.
    harness.set_game_mode(GameMode::Survival);
    harness
        .game
        .player_mut(harness.id)
        .expect("player")
        .inventory
        .set_slot(0, EntityStack::EMPTY)
        .expect("clear");
    harness.intent(PlayIntent::SetCreativeModeSlot {
        slot: 36,
        item: WireStack {
            item_id: stick,
            count: 7,
            components: Vec::new(),
        },
    });
    assert_eq!(
        harness.inv_count("minecraft:stick"),
        0,
        "survival never mints from set_creative_mode_slot"
    );
}

/// B-H2: a creative take keeps the wire patch — a damaged shovel arrives
/// damaged, not plain.
#[test]
fn creative_take_keeps_the_component_patch() {
    let mut harness = Harness::new("p17-pin-creative-patch");
    harness.join("Architect");
    harness.set_game_mode(GameMode::Creative);
    let shovel = harness.item_id("minecraft:wooden_shovel");
    harness.intent(PlayIntent::SetCreativeModeSlot {
        slot: 36,
        item: WireStack {
            item_id: shovel,
            count: 1,
            components: vec![(mc_entity::components::TYPE_DAMAGE, vec![0x01])],
        },
    });
    let stack = harness
        .game
        .player(harness.id)
        .expect("player")
        .inventory
        .slot(0);
    assert_eq!(
        stack.components().damage(),
        Some(1),
        "the creative-taken shovel carries its damage patch"
    );
}

/// d95ce2a / 80d4558: an opposite-hinge pair flips together, including from
/// the upper half (the `lower_y` remap).
#[test]
fn opposite_hinge_double_doors_open_together_from_either_half() {
    let mut harness = Harness::new("p17-pin-double-door");
    harness.join("Carpenter");
    let (sx, sy, sz) = harness.game.spawn();
    floor(&mut harness, sx, sy, sz);
    harness.look(0.0); // look south → facing north
    let (x0, y0, z0) = (sx + 1, sy, sz);
    let (x1, _, z1) = (sx + 2, sy, sz);

    // Left leaf (cursor west → hinge left) then right leaf (cursor east).
    harness.give("minecraft:oak_door");
    harness.click_cursor(x0, y0 - 1, z0, 1, (0.2, 0.5, 0.5));
    harness.give("minecraft:oak_door");
    harness.click_cursor(x1, y1_floor(y0), z1, 1, (0.8, 0.5, 0.5));
    assert_eq!(harness.name(x0, y0, z0), "minecraft:oak_door");
    assert_eq!(harness.name(x1, y0, z1), "minecraft:oak_door");
    let h0 = harness.prop(x0, y0, z0, "hinge");
    let h1 = harness.prop(x1, y0, z1, "hinge");
    assert_ne!(h0, h1, "opposite hinges form a pair, got {h0}/{h1}");

    // Click the **upper** half of the first leaf: both leaves must open.
    harness.empty_hand();
    harness.click(x0, y0 + 1, z0, 1);
    assert_eq!(
        harness.prop(x0, y0, z0, "open"),
        "true",
        "clicked leaf opens"
    );
    assert_eq!(
        harness.prop(x1, y0, z1, "open"),
        "true",
        "paired leaf follows"
    );
    assert_eq!(harness.prop(x0, y0 + 1, z0, "open"), "true");
    assert_eq!(harness.prop(x1, y0 + 1, z1, "open"), "true");

    // Close from the lower half of the second leaf: both close again.
    harness.click(x1, y0, z1, 1);
    assert_eq!(harness.prop(x0, y0, z0, "open"), "false");
    assert_eq!(harness.prop(x1, y0, z1, "open"), "false");
}

fn y1_floor(y0: i32) -> i32 {
    y0 - 1
}

/// ae94f4b / f16b25a: a button is a pulse — press schedules unpress and a
/// second press inside the hold does not stick it on.
#[test]
fn stone_button_unpresses_after_the_hold_and_never_sticks() {
    let mut harness = Harness::new("p17-pin-button");
    harness.join("Stoner");
    let (sx, sy, sz) = harness.game.spawn();
    floor(&mut harness, sx, sy, sz);
    harness.look(0.0);
    let (x, y, z) = (sx + 1, sy, sz);
    // Place the button on the floor's top face.
    harness.give("minecraft:stone_button");
    harness.click(x, y - 1, z, 1);
    assert_eq!(harness.name(x, y, z), "minecraft:stone_button");
    assert_eq!(
        harness.prop(x, y, z, "powered"),
        "false",
        "placement must not feed a pulse (owner A2)"
    );

    harness.empty_hand();
    harness.click(x, y, z, 1);
    assert_eq!(harness.prop(x, y, z, "powered"), "true", "press powers");
    // Two `intent` ticks already ran (press + re-press). The hold is 20 from
    // the first press, so 17 more ticks sit inside it and 20 clear it.
    harness.click(x, y, z, 1);
    harness.run(17);
    assert_eq!(harness.prop(x, y, z, "powered"), "true", "still held at 19");
    harness.run(4);
    assert_eq!(
        harness.prop(x, y, z, "powered"),
        "false",
        "stone button releases by tick 21"
    );
}

/// a9b1c63: sneak (`player_input` bit 5) places onto a container instead of
/// opening it — the real client's path (not `player_command` 0/1).
#[test]
fn player_input_bit5_sneak_places_onto_a_chest_instead_of_opening() {
    let mut harness = Harness::new("p17-pin-sneak");
    harness.join("Builder");
    let (sx, sy, sz) = harness.game.spawn();
    floor(&mut harness, sx, sy, sz);
    harness.look(0.0);
    let (x, y, z) = (sx + 1, sy, sz);
    harness.set_block(x, y, z, "minecraft:chest");

    // Sneak on via player_input bit 5 (32).
    harness.intent(PlayIntent::PlayerInput { input: 32 });
    harness.give("minecraft:stone");
    harness.click(x, y, z, 1); // top face → stone lands above
    assert_eq!(
        harness.name(x, y + 1, z),
        "minecraft:stone",
        "sneak places against the chest"
    );
    assert_eq!(harness.name(x, y, z), "minecraft:chest", "chest stays");

    // Clear sneak and click again: the chest opens (no second stone).
    harness.intent(PlayIntent::PlayerInput { input: 0 });
    harness.empty_hand();
    harness.click(x, y, z, 1);
    assert_eq!(
        harness.name(x, y + 1, z),
        "minecraft:stone",
        "without sneak nothing new lands on the chest"
    );
}

// ----------------------------------------------------- AUDIT-19 B19-2 pins
//
// AUDIT-18 B-M2 called the two silent downgrades "loud": `wire_stack`'s
// component-free fallback logged at `debug!` (invisible at the default `info`
// filter an operator actually runs) and the creative fallback logged nothing
// at all. Both are `warn!` now, and the two tests below are what keeps them
// there — each drives the real code path on the real stack shape and reads the
// `tracing` event back, so dropping either message below `warn!` (or deleting
// it) goes red instead of quiet.

/// One captured `tracing` event: its level and its `message` field.
type Captured = Vec<(tracing::Level, String)>;

/// A `tracing` subscriber that records every event raised on this thread.
///
/// Hand-rolled rather than a `tracing_subscriber` fmt layer: the assertion is
/// about the **level** of one message, and a subscriber that keeps events as
/// data says that directly instead of parsing formatted text back out.
struct LevelSpy {
    events: std::sync::Mutex<Captured>,
}

/// Pulls the `message` field out of a captured event.
struct MessageVisitor(String);

impl tracing::field::Visit for MessageVisitor {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.0 = format!("{value:?}");
        }
    }
}

impl tracing::Subscriber for LevelSpy {
    fn enabled(&self, _metadata: &tracing::Metadata<'_>) -> bool {
        true
    }

    fn new_span(&self, _span: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }

    fn record(&self, _span: &tracing::span::Id, _values: &tracing::span::Record<'_>) {}

    fn record_follows_from(&self, _span: &tracing::span::Id, _follows: &tracing::span::Id) {}

    fn event(&self, event: &tracing::Event<'_>) {
        let mut message = MessageVisitor(String::new());
        event.record(&mut message);
        self.events
            .lock()
            .expect("the spy is not poisoned")
            .push((*event.metadata().level(), message.0));
    }

    fn enter(&self, _span: &tracing::span::Id) {}

    fn exit(&self, _span: &tracing::span::Id) {}
}

/// Run `body` with the spy installed as this thread's subscriber and return
/// both its value and what it saw.
///
/// A thread-local default, which is why it works here: `Game::tick` and every
/// `wire_stack` call a test makes run on the test's own thread.
fn capture_events<T>(body: impl FnOnce() -> T) -> (T, Captured) {
    let spy = std::sync::Arc::new(LevelSpy {
        events: std::sync::Mutex::new(Vec::new()),
    });
    let seen = std::sync::Arc::clone(&spy);
    let value = tracing::subscriber::with_default(spy, body);
    let events = seen.events.lock().expect("the spy is not poisoned").clone();
    (value, events)
}

/// Assert some event carries `needle` at `warn!` or above.
///
/// `Level`'s order runs ERROR < WARN < INFO < DEBUG < TRACE, so "at least as
/// severe as warn" is `<= WARN` — the comparison an operator's filter makes.
fn assert_warned(events: &Captured, needle: &str) {
    assert!(
        events
            .iter()
            .any(|(level, message)| *level <= tracing::Level::WARN && message.contains(needle)),
        "no warn-or-above event contained {needle:?}; the downgrade is invisible at the default \
         `info` filter. Saw {events:?}"
    );
}

/// AUDIT-19 B19-2 (arm 1): `wire_stack`'s fallback is a `warn!`.
///
/// The trigger is the one shape that cannot be reframed on the wire — a
/// `Consumable` carrying `on_consume_effects` — sitting in a chest the player
/// then opens, so the stack really does go through `wire_stack` on its way to
/// `container_set_content`.
#[test]
fn a_stack_that_cannot_be_framed_warns_at_warn_level() {
    use mc_entity::components::{
        Consumable, ConsumeAnimation, DataComponent, ItemComponents, SoundRef,
    };

    let mut harness = Harness::new("p19-b19-2-wire-stack");
    let mut out = harness.join("Packer");
    let (sx, sy, sz) = harness.game.spawn();
    floor(&mut harness, sx, sy, sz);
    harness.look(0.0);
    let (cx, cy, cz) = (sx + 1, sy, sz);
    harness.set_block(cx, cy, cz, "minecraft:chest");
    let pos = mc_container::BlockPos::new(cx, cy, cz);
    harness
        .game
        .block_entities_mut()
        .insert(mc_container::BlockEntity::new(
            pos,
            mc_container::BlockEntityKind::Container,
        ));

    // The stack `mc-protocol`'s own `consumable_effects_refuse_wire_framing`
    // test pins as unframeable, placed where a client would have to be told
    // about it.
    let mut components = ItemComponents::new();
    components.set(DataComponent::Consumable(Consumable {
        consume_seconds: 1.6,
        animation: ConsumeAnimation::Eat,
        sound: SoundRef::Named {
            name: "minecraft:entity.generic.eat".to_owned(),
            range: None,
        },
        consume_particles: true,
        on_consume_effects: vec![mc_nbt::NbtTag::Int(1)],
    }));
    let stick = harness.item_id("minecraft:stick");
    let poisoned = EntityStack::with_components(stick, 3, components).expect("a legal stack");
    harness
        .game
        .block_entities_mut()
        .get_mut(pos)
        .expect("the chest entity exists")
        .data
        .items_mut()
        .expect("a chest has slots")[0] = poisoned;

    harness.empty_hand();
    // Drop the join burst so the packet below can only be the open's.
    while out.try_recv().is_some() {}
    let ((), events) = capture_events(|| harness.click(cx, cy, cz, 1));

    assert_warned(&events, "a stack fell back to a component-free wire form");
    // And the fallback still does its job: the window the client is sent has
    // the item, minus the components the wire cannot frame. Read from the
    // packet rather than the menu, because the menu keeps the typed patch —
    // the downgrade only exists on the wire, which is exactly why it needs a
    // trace.
    let shown = std::iter::from_fn(|| out.try_recv())
        .filter(|raw| raw.id == clientbound::play::CONTAINER_SET_CONTENT)
        .find_map(|raw| mc_protocol::packets::play::ContainerSetContent::decode(&raw.payload).ok())
        .expect("the open sends the window contents");
    assert_eq!(
        shown.slots.len(),
        63,
        "a single chest is 27 slots plus the 36-slot player half"
    );
    let shown = shown.slots.into_iter().next().expect("slot 0");
    assert_eq!(shown.item_id, stick, "the stack is still shown");
    assert_eq!(shown.count, 3, "with its count");
    assert!(
        shown.components.is_empty(),
        "and in the component-free form the fallback chose"
    );
}

/// AUDIT-19 B19-2 (arm 2): the creative take's fallback is a `warn!`.
///
/// A creative client sends the full component patch, so a patch this build
/// cannot decode used to become a plain stack with no trace anywhere. The
/// payload here is a known type (damage, 3) with an empty body: the decoder
/// refuses the truncation, which is the fallback trigger.
#[test]
fn a_creative_take_whose_patch_cannot_be_decoded_warns_at_warn_level() {
    let mut harness = Harness::new("p19-b19-2-creative-fallback");
    // The receiver is kept alive so the take's own window sync can be sent;
    // a dropped one turns the join's packets into "outbound queue full"
    // warnings that have nothing to do with what this test pins.
    let _out = harness.join("Architect");
    harness.set_game_mode(GameMode::Creative);
    let stick = harness.item_id("minecraft:stick");

    let ((), events) = capture_events(|| {
        harness.intent(PlayIntent::SetCreativeModeSlot {
            slot: 36, // window slot 36 is hotbar 0
            item: WireStack {
                item_id: stick,
                count: 5,
                components: vec![(mc_entity::components::TYPE_DAMAGE, Vec::new())],
            },
        });
    });

    assert_warned(
        &events,
        "a creative take fell back to a component-free stack",
    );
    assert_eq!(
        harness.inv_count("minecraft:stick"),
        5,
        "the take still lands, so the warn is the only trace of the lost patch"
    );
    let landed = harness
        .game
        .player(harness.id)
        .expect("player")
        .inventory
        .slot(0);
    assert!(
        landed.components().is_empty(),
        "the fallback is the component-free stack the warn names"
    );
}
