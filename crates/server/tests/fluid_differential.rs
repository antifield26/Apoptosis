//! Differential fluid test (P20-01): **our** engine against the **real 26.1.2 server**.
//!
//! The five scenarios are frozen in `docs/adr/ADR-0009-world-ticking.md` section 2.6 —
//! `spring_flow`, `falling_column`, `lava_meets_water`, `waterlogged_stairs`,
//! `bucket_place` — with `N = 40` ticks each. Both sides read one spec,
//! `crates/test-support/fixtures/fluids/scenarios.toml`: the Rust side builds the world
//! it describes, the capture tool (`tools/fluid-differential/capture.py`) turns it into
//! console commands for the official server. There is no second copy of the scenario to
//! drift from.
//!
//! ## What is compared, and why it is complete
//!
//! The vanilla baseline is not hand-written and not synthesised: the capture script
//! starts the real server jar, freezes it, applies the setup, saves (that is `initial.txt`,
//! the state at tick 0), advances exactly `N` ticks with `/tick step N`, saves again (that
//! is `baseline.txt`) and reads the cells back out of the region file the server wrote. The
//! game time is checked to have moved by exactly `N`, so "after 40 ticks" is measured, not
//! assumed. `MANIFEST.txt` records the jar hash, the Java version, the command script and
//! the region-file hashes for every row.
//!
//! Everything is compared inside one box — `x 2..13, y 100..107, z 4..12` — and the box is
//! **sealed**: its floor, four walls and lid are bedrock, built by the shared frame, and
//! `assert_sealed` refuses to compare a dump whose shell is not bedrock. Fluid cannot leave
//! a sealed box, so comparing the box is comparing everything the scenario can touch, cell
//! by cell, with no tolerance and no sampling.
//!
//! ## The comparison gate: tick 0 first
//!
//! Both sides must start from the same world before a divergence means anything, so the
//! test first compares our freshly built world against `initial.txt`. If that fails, the
//! scenario is not a fluid finding and the report says so: it prints the tick-0 divergence
//! **before** the post-tick ones, because every later divergence would be an artifact of it.
//! (The check also validates this file's two decoders — chunk-cell order and the dump
//! parser — against the server's own output, which is why a wrong index order cannot be
//! mistaken for a fluid bug.)
//!
//! ## Three comparisons per scenario
//!
//! 1. **tick 0** — our built world against `initial.txt`.
//! 2. **after N ticks, in memory** — where a flow bug shows up, untouched by persistence.
//! 3. **after a save and a reopen from disk** — the same cells read back through
//!    [`WorldStorage`], so a fluid state that survives the tick but not the encoder is
//!    reported as its own failure rather than hidden behind a matching simulation.
//!
//! ## Named boundaries (what this rig does **not** cover)
//!
//! * **`bucket_place`'s action path.** A server console cannot deliver `UseItemOn`, and no
//!   real client is attached, so that scenario's vanilla side writes the block state a
//!   water bucket leaves behind (`minecraft:water[level=0]` on top of the pillar) rather
//!   than performing the bucket use. What is compared is the flow that follows the
//!   placement. Whether *our* `UseItemOn`/bucket path produces that source cell is **not**
//!   verified here and needs its own differential.
//! * **Random ticks, time and weather.** The vanilla side runs with `random_tick_speed 0`,
//!   `spawn_mobs false`, `advance_time false`, `advance_weather false` and
//!   `fire_spread_radius_around_player 0` (the 26.1.2 rule names — the capture fails a run
//!   the server rejects a command in, so a stale name cannot silently leave a default in
//!   place). Anything those rules would have done is P20-02's and P20-03's subject, not this
//!   rig's.
//! * **Entities.** No player, mob or item is in the world; float/sink and lava damage are
//!   not covered.
//! * **A divergence is a finding, not a failing fixture.** When this test goes red the
//!   first divergence names a cell, what vanilla has there and what we have. It is not
//!   patched by editing `crates/test-support/fixtures/fluids/**`: those files are the
//!   server's output. Re-generate them with the capture tool or fix the engine.
//!
//! ## Running it
//!
//! ```text
//! cargo test -p mc-server --test fluid_differential -- --nocapture
//! ```
//!
//! Re-generating the baselines (needs `target/vanilla-26.1.2/server-26.1.2.jar` and Java):
//!
//! ```text
//! python tools/fluid-differential/capture.py
//! ```

use mc_persistence::chunk::{BlockState, ChunkPos as StoredChunkPos};
use mc_persistence::dimension::Dimension;
use mc_persistence::world::WorldStorage;
use mc_registry::{BlockRegistry, Registries};
use mc_server::config::StorageConfig;
use mc_server::game::Game;
use mc_server::storage::WorldService;
use mc_test_support::fixtures::TempDir;
use mc_world::ChunkPos;
use serde::Deserialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// The five names ADR-0009 section 2.6 freezes, in the frozen order. Renaming, swapping or
/// adding one here would make the ADR's "compared 5" unfalsifiable, which is why the names
/// live in the ADR and are checked against it rather than remembered.
const FROZEN: [&str; 5] = [
    "spring_flow",
    "falling_column",
    "lava_meets_water",
    "waterlogged_stairs",
    "bucket_place",
];

/// The box every scenario is built inside and compared over, from the spec.
type Bounds = [i32; 6];

#[derive(Debug, Deserialize)]
struct Spec {
    version: u32,
    /// `x0 y0 z0 x1 y1 z1`, inclusive. `box` is a Rust keyword, hence the rename.
    #[serde(rename = "box")]
    bounds: Bounds,
    world: WorldSpec,
    frame: Vec<Op>,
    scenario: Vec<Scenario>,
}

#[derive(Debug, Deserialize)]
struct WorldSpec {
    chunk: [i32; 2],
    seed: i64,
}

#[derive(Debug, Deserialize)]
struct Op {
    block: String,
    fill: Option<[i32; 6]>,
    set: Option<[i32; 3]>,
}

#[derive(Debug, Deserialize)]
struct Scenario {
    name: String,
    ticks: u32,
    /// Required when `ticks` is not the ADR's 40: the ADR allows a row to need a
    /// jar-stated spread delay, and a row that takes the longer window must say which.
    #[serde(default)]
    ticks_reason: Option<String>,
    /// Jar-stated spread delays, recorded per row (ADR-0009 section 4, P20-01).
    water_delay: u32,
    lava_delay: u32,
    exercises: String,
    setup: Vec<Op>,
}

fn fixtures_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../test-support/fixtures/fluids")
}

fn load_spec() -> Spec {
    let path = fixtures_root().join("scenarios.toml");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()));
    toml::from_str(&text)
        .unwrap_or_else(|error| panic!("{} does not parse: {error}", path.display()))
}

// ------------------------------------------------------------------ block states

/// Split `minecraft:oak_stairs[facing=north,waterlogged=true]` into its name and sorted
/// properties. The spec's own syntax, so a row reads the same in the TOML, in the dump
/// files and in the divergence report.
fn parse_state(text: &str) -> (&str, Vec<(String, String)>) {
    let Some(open) = text.find('[') else {
        return (text, Vec::new());
    };
    let name = &text[..open];
    let inner = text[open + 1..]
        .strip_suffix(']')
        .unwrap_or_else(|| panic!("block state {text:?} has an unclosed property list"));
    let mut properties: Vec<(String, String)> = inner
        .split(',')
        .filter(|part| !part.trim().is_empty())
        .map(|part| {
            let (key, value) = part
                .split_once('=')
                .unwrap_or_else(|| panic!("block state {text:?} has a property without a value"));
            (key.trim().to_owned(), value.trim().to_owned())
        })
        .collect();
    properties.sort();
    (name, properties)
}

/// The canonical form of a state: `name[prop=value,...]`, properties sorted. The same
/// string the capture tool writes, which is what lets two independently written dumps be
/// compared as text.
fn format_state(name: &str, properties: &[(String, String)]) -> String {
    if properties.is_empty() {
        return name.to_owned();
    }
    let mut sorted: Vec<&(String, String)> = properties.iter().collect();
    sorted.sort();
    let inner: Vec<String> = sorted
        .iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect();
    format!("{name}[{}]", inner.join(","))
}

fn state_id(blocks: &BlockRegistry, text: &str) -> i32 {
    let (name, properties) = parse_state(text);
    blocks.state_id(name, &properties).unwrap_or_else(|error| {
        panic!("the spec names {text:?}, which the block registry does not know: {error}")
    })
}

// ------------------------------------------------------------------- the dump file

/// One recorded dump: a palette of state strings and one row of palette indices per
/// `(y, z)`.
struct Dump {
    game_time: String,
    ticks: u32,
    bounds: Bounds,
    palette: Vec<String>,
    nx: usize,
    rows: usize,
    grid: Vec<Vec<usize>>,
}

fn numbers(text: &str) -> Vec<i32> {
    text.split(|c: char| !c.is_ascii_digit() && c != '-')
        .filter(|part| !part.is_empty())
        .map(|part| part.parse().expect("a number"))
        .collect()
}

impl Dump {
    fn load(path: &Path) -> Self {
        let text = std::fs::read_to_string(path)
            .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()));
        let lines: Vec<&str> = text
            .lines()
            .filter(|line| !line.trim().is_empty())
            .collect();
        let field = |prefix: &str| -> String {
            lines
                .iter()
                .find_map(|line| line.strip_prefix(prefix))
                .unwrap_or_else(|| panic!("{} has no {prefix:?} line", path.display()))
                .trim()
                .to_owned()
        };
        let ticks: u32 = field("# ticks:").parse().expect("a tick count");
        let bounds = {
            let raw = numbers(&field("# box:"));
            assert_eq!(
                raw.len(),
                6,
                "{}: the box line is malformed",
                path.display()
            );
            [raw[0], raw[2], raw[4], raw[1], raw[3], raw[5]]
        };
        // Only the `palette`/`cells` sections are positional; comments are skipped above.
        let start = lines
            .iter()
            .position(|line| line.starts_with("palette "))
            .unwrap_or_else(|| panic!("{} has no palette line", path.display()));
        let palette_count: usize = lines[start][8..].trim().parse().expect("a palette size");
        let palette: Vec<String> = lines[start + 1..start + 1 + palette_count]
            .iter()
            .map(|line| (*line).to_owned())
            .collect();
        let cells_at = start + 1 + palette_count;
        let header = numbers(lines[cells_at]);
        assert_eq!(
            header.len(),
            2,
            "{}: the cells line is malformed",
            path.display()
        );
        let (nx, rows) = (
            usize::try_from(header[0]).expect("a column count"),
            usize::try_from(header[1]).expect("a row count"),
        );
        let grid: Vec<Vec<usize>> = lines[cells_at + 1..]
            .iter()
            .take(rows)
            .map(|line| {
                line.split_whitespace()
                    .map(|value| value.parse().expect("a palette index"))
                    .collect()
            })
            .collect();
        assert_eq!(grid.len(), rows, "{}: truncated cell rows", path.display());
        assert!(
            grid.iter().all(|row| row.len() == nx),
            "{}: a cell row has the wrong width",
            path.display()
        );
        assert!(
            lines.len() == cells_at + 1 + rows,
            "{}: unexpected content after the cell rows",
            path.display()
        );
        Self {
            game_time: field("# game time:"),
            ticks,
            bounds,
            palette,
            nx,
            rows,
            grid,
        }
    }

    /// The state at a world position.
    fn at(&self, x: i32, y: i32, z: i32) -> &str {
        let [x0, y0, z0, x1, y1, z1] = self.bounds;
        assert!(
            (x0..=x1).contains(&x) && (y0..=y1).contains(&y) && (z0..=z1).contains(&z),
            "({x}, {y}, {z}) is outside the compared box {x0}..{x1}, {y0}..{y1}, {z0}..{z1}"
        );
        let nz = usize::try_from(z1 - z0 + 1).expect("a box width");
        let row =
            usize::try_from(y - y0).expect("a row") * nz + usize::try_from(z - z0).expect("a row");
        let column = usize::try_from(x - x0).expect("a column");
        &self.palette[self.grid[row][column]]
    }

    fn totals(&self) -> usize {
        self.nx * self.rows
    }

    /// The box must be sealed by bedrock on all six faces or the comparison region is not
    /// closed and a fluid could have left it.
    fn assert_sealed(&self, path: &Path) {
        assert!(self.totals() > 0, "{}: the box is empty", path.display());
        let [x0, y0, z0, x1, y1, z1] = self.bounds;
        for x in x0..=x1 {
            for z in z0..=z1 {
                for y in y0..=y1 {
                    let shell = x == x0 || x == x1 || y == y0 || y == y1 || z == z0 || z == z1;
                    if shell {
                        assert_eq!(
                            self.at(x, y, z),
                            "minecraft:bedrock",
                            "{}: ({x}, {y}, {z}) is on the box shell and is not bedrock, so the \
                             compared region is not closed",
                            path.display()
                        );
                    }
                }
            }
        }
    }
}

// -------------------------------------------------------------------- comparisons

#[derive(Debug)]
struct Divergence {
    x: i32,
    y: i32,
    z: i32,
    expected: String,
    actual: String,
}

impl std::fmt::Display for Divergence {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "({}, {}, {}): vanilla {}, ours {}",
            self.x, self.y, self.z, self.expected, self.actual
        )
    }
}

#[derive(Debug)]
struct Comparison {
    compared: usize,
    matching: usize,
    first: Option<Divergence>,
}

impl Comparison {
    fn exact(&self) -> bool {
        self.matching == self.compared
    }

    fn summary(&self) -> String {
        match &self.first {
            None => format!("{}/{} cells match", self.matching, self.compared),
            Some(first) => format!(
                "{}/{} cells match; first divergence {first}",
                self.matching, self.compared
            ),
        }
    }
}

/// Compare every cell of the box: ours from `read`, vanilla's from the dump. Cell order is
/// the dump's own (`y`, then `z`, then `x`), so the "first" divergence is the first one a
/// reader of the fixture would find.
fn compare(dump: &Dump, mut read: impl FnMut(i32, i32, i32) -> String) -> Comparison {
    let [x0, y0, z0, x1, y1, z1] = dump.bounds;
    let mut comparison = Comparison {
        compared: 0,
        matching: 0,
        first: None,
    };
    for y in y0..=y1 {
        for z in z0..=z1 {
            for x in x0..=x1 {
                let expected = dump.at(x, y, z);
                let actual = read(x, y, z);
                comparison.compared += 1;
                if expected == actual {
                    comparison.matching += 1;
                } else if comparison.first.is_none() {
                    comparison.first = Some(Divergence {
                        x,
                        y,
                        z,
                        expected: expected.to_owned(),
                        actual,
                    });
                }
            }
        }
    }
    comparison
}

// ------------------------------------------------------------------- running a scenario

/// Where each comparison stands for one scenario, ready to be printed or turned into a
/// failure message.
struct Outcome {
    name: String,
    ticks: u32,
    water_delay: u32,
    lava_delay: u32,
    game_time: String,
    box_cells: usize,
    vanilla_changed: usize,
    start: Comparison,
    after: Comparison,
    saved: Comparison,
    timing: String,
}

fn world_config(dir: &TempDir, spec: &Spec) -> StorageConfig {
    StorageConfig {
        world_dir: dir.path().join("world"),
        autosave_ticks: 0,
        seed: Some(spec.world.seed),
    }
}

/// Copy a world directory, skipping the session lock (which a running server holds).
fn copy_tree(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let name = entry.file_name();
        if name == "session.lock" {
            continue;
        }
        let target = to.join(&name);
        if entry.file_type()?.is_dir() {
            copy_tree(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

/// Build the shared frame **once**: the sealed bedrock box every scenario is set inside.
///
/// The frame is the same for all five rows, and our light engine's cost is per *changed
/// block*, so building it five times was the whole runtime of this test (measured: ~1400
/// `set_block` calls per scenario, 16-50s each, against 0.1s for the 40 ticks). Each
/// scenario still gets its own private copy, so its tick timeline starts from the frame
/// with no scheduled fluid tick left over from the scenario before it — the same thing the
/// vanilla side gets from a freshly generated world per capture run.
fn build_frame_world(spec: &Spec, blocks: &BlockRegistry) -> (TempDir, PathBuf) {
    let dir = TempDir::new("fluid-frame");
    let service = WorldService::open(&world_config(&dir, spec)).expect("the frame world opens");
    let mut game =
        Game::with_seed_and_storage(service, 4, mc_network::bridge::game_channel(64).1, 7)
            .expect("the frame game builds");
    let [chunk_x, chunk_z] = spec.world.chunk;
    assert!(
        game.load_chunk(ChunkPos::new(chunk_x, chunk_z)),
        "chunk ({chunk_x}, {chunk_z}) does not load"
    );
    for op in &spec.frame {
        apply(&mut game, blocks, op);
    }
    game.save_all_owned().expect("the frame world saves");
    game.close_storage().expect("the frame storage closes");
    let world_dir = dir.path().join("world");
    (dir, world_dir)
}

/// Apply one spec operation to our world. A `fill` is every cell of its volume, in the spec's
/// own order.
///
/// The write **and its feed**: in Vanilla the write is what schedules — `Level.setBlock`
/// notifies neighbours (`updateNeighborsAt` when `flags & 1`) and `LiquidBlock.onPlace`
/// schedules the fluid tick. `world_mut().set_block` is the raw setter and does neither, so
/// the rig makes the same `feed_block_change` call the game's own edit paths make. Without it
/// the fluid queue stays empty for all 40 ticks and every cell Vanilla changed shows up as a
/// divergence (P20-01: fed, all five scenarios are 864/864; unfed, all five are red).
fn apply(game: &mut Game, blocks: &mc_registry::BlockRegistry, op: &Op) {
    let id = state_id(blocks, &op.block);
    let cells: Vec<(i32, i32, i32)> = match (op.fill, op.set) {
        (Some([x0, y0, z0, x1, y1, z1]), _) => (x0..=x1)
            .flat_map(|x| (y0..=y1).flat_map(move |y| (z0..=z1).map(move |z| (x, y, z))))
            .collect(),
        (None, Some([x, y, z])) => vec![(x, y, z)],
        (None, None) => panic!("the spec has an operation with neither `fill` nor `set`"),
    };
    for (x, y, z) in cells {
        game.world_mut()
            .set_block(x, y, z, id)
            .unwrap_or_else(|error| {
                panic!("cannot place {} at ({x}, {y}, {z}): {error}", op.block)
            });
        game.feed_block_change(x, y, z);
    }
}

fn cell_map(chunk: &mc_persistence::chunk::ChunkData) -> HashMap<i8, Vec<String>> {
    chunk
        .sections
        .iter()
        .map(|section| {
            let values: Vec<String> = section
                .block_states
                .to_values()
                .unwrap_or_else(|error| panic!("section y={} does not decode: {error}", section.y))
                .iter()
                .map(|state: &BlockState| format_state(&state.name, &state.properties))
                .collect();
            (section.y, values)
        })
        .collect()
}

/// One section cell, in vanilla's order: `y * 256 + z * 16 + x` within the section.
fn section_cell(sections: &HashMap<i8, Vec<String>>, x: i32, y: i32, z: i32) -> String {
    let Some(values) = sections.get(&i8::try_from(y >> 4).expect("a section index in range"))
    else {
        return "minecraft:air".to_owned();
    };
    let index = usize::try_from((y & 15) * 256 + (z & 15) * 16 + (x & 15)).expect("a cell index");
    values
        .get(index)
        .cloned()
        .unwrap_or_else(|| "minecraft:air".to_owned())
}

/// The recorded pair for one scenario, with the cross-checks that stop a fixture and the
/// spec from drifting apart (the dump's own header against the spec's box and tick count,
/// and the sealed-box rule).
fn load_fixtures(spec: &Spec, scenario: &Scenario) -> Result<(Dump, Dump), String> {
    let root = fixtures_root().join(&scenario.name);
    let initial = Dump::load(&root.join("initial.txt"));
    let baseline = Dump::load(&root.join("baseline.txt"));
    for (dump, path) in [
        (&initial, root.join("initial.txt")),
        (&baseline, root.join("baseline.txt")),
    ] {
        if dump.bounds != spec.bounds {
            return Err(format!(
                "{} covers {:?}, the spec says {:?}: the fixture and the spec have drifted",
                path.display(),
                dump.bounds,
                spec.bounds
            ));
        }
        if dump.ticks != scenario.ticks {
            return Err(format!(
                "{} was captured at {} ticks, the spec says {}",
                path.display(),
                dump.ticks,
                scenario.ticks
            ));
        }
        dump.assert_sealed(&path);
    }
    Ok((initial, baseline))
}

/// How many cells the vanilla run changed over the window: a scenario that changes nothing
/// is not exercising anything, and this is the number that says so.
fn vanilla_changed(initial: &Dump, baseline: &Dump) -> usize {
    let [x0, y0, z0, x1, y1, z1] = baseline.bounds;
    let mut changed = 0;
    for y in y0..=y1 {
        for z in z0..=z1 {
            for x in x0..=x1 {
                if initial.at(x, y, z) != baseline.at(x, y, z) {
                    changed += 1;
                }
            }
        }
    }
    changed
}

/// Our saved chunk, read back through the persistence layer and indexed by section.
fn read_saved_cells(world_dir: &Path, chunk: [i32; 2]) -> Result<HashMap<i8, Vec<String>>, String> {
    let [chunk_x, chunk_z] = chunk;
    let mut storage = WorldStorage::open(world_dir, 0)
        .map_err(|error| format!("the saved world does not reopen: {error}"))?;
    let stored = storage
        .read_chunk(&Dimension::Overworld, StoredChunkPos::new(chunk_x, chunk_z))
        .map_err(|error| format!("the saved chunk does not read back: {error}"))?;
    let Some(stored) = stored else {
        return Err(format!(
            "chunk ({chunk_x}, {chunk_z}) is not in the saved world, so the save wrote nothing"
        ));
    };
    Ok(cell_map(&stored))
}

fn run_scenario(
    spec: &Spec,
    blocks: &BlockRegistry,
    scenario: &Scenario,
    frame_world: &Path,
) -> Result<Outcome, String> {
    let (initial, baseline) = load_fixtures(spec, scenario)?;
    let changed = vanilla_changed(&initial, &baseline);

    let started = std::time::Instant::now();
    // A private copy of the frame world per scenario: its own tick timeline, and no
    // scheduled fluid tick carried over from the scenario before it.
    let dir = TempDir::new(&format!("fluid-{}", scenario.name));
    let world_dir = dir.path().join("world");
    copy_tree(frame_world, &world_dir)
        .map_err(|error| format!("the frame world does not copy: {error}"))?;
    let service = WorldService::open(&StorageConfig {
        world_dir: world_dir.clone(),
        autosave_ticks: 0,
        seed: Some(spec.world.seed),
    })
    .map_err(|error| format!("the world does not open: {error}"))?;
    let mut game =
        Game::with_seed_and_storage(service, 4, mc_network::bridge::game_channel(64).1, 7)
            .map_err(|error| format!("the game does not build: {error}"))?;

    let [chunk_x, chunk_z] = spec.world.chunk;
    let pos = ChunkPos::new(chunk_x, chunk_z);
    if !game.load_chunk(pos) {
        return Err(format!("chunk ({chunk_x}, {chunk_z}) does not load"));
    }
    for op in &scenario.setup {
        apply(&mut game, blocks, op);
    }
    // ---- the harness-asymmetry guard (why it exists) -------------------------
    // `apply` writes through `world_mut().set_block`, the raw setter, and then
    // calls `feed_block_change`. The write is the scenario; **the feed is the
    // harness's stand-in for `Level.setBlock`**, and it is the only thing here
    // that queues fluid work. If that call ever disappears, nothing else in the
    // rig notices: the fluid queue stays empty for all forty ticks, our world
    // never changes by itself, and the test goes red naming cell after cell that
    // vanilla changed — a failure that looks like an engine bug and is really a
    // rig that stopped feeding the queue. That asymmetry (vanilla's real
    // `setBlock` on one side, a raw setter plus a feed on the other) is
    // invisible in the comparison, so it is asserted here instead.
    //
    // Every scenario's setup writes at least one water or lava block, and nothing
    // drains the queue before the first `game.tick()` below, so a non-empty queue
    // at this point can only mean those writes were fed. A zero here is the
    // harness failing, not the engine.
    assert!(
        game.fluid_pending() > 0,
        "{}: the fluid queue is empty after the setup writes, so \
         `feed_block_change` did not schedule the fluid blocks the setup placed. \
         This rig writes through the raw `world_mut().set_block`, which — unlike \
         vanilla's `Level.setBlock` — schedules nothing; without the feed the \
         forty ticks below change nothing and every cell vanilla changed is \
         reported as an engine divergence. Fix the harness, not the engine.",
        scenario.name
    );
    let setup_done = started.elapsed();

    // 1. The gate: both sides must start from the same cells.
    let start = compare(&initial, |x, y, z| our_cell(&game, blocks, x, y, z));

    // 2. The simulation, in memory, before persistence can hide anything.
    for _ in 0..scenario.ticks {
        game.tick()
            .map_err(|error| format!("tick {} failed: {error}", game.tick_count()))?;
    }
    let ticks_done = started.elapsed();
    let after = compare(&baseline, |x, y, z| our_cell(&game, blocks, x, y, z));

    // 3. The same cells, saved and read back from disk.
    game.save_all_owned()
        .map_err(|error| format!("the save failed: {error}"))?;
    game.close_storage()
        .map_err(|error| format!("the storage handle does not close: {error}"))?;
    let sections = read_saved_cells(&world_dir, spec.world.chunk)?;
    let saved = compare(&baseline, |x, y, z| section_cell(&sections, x, y, z));
    let total = started.elapsed();

    Ok(Outcome {
        name: scenario.name.clone(),
        ticks: scenario.ticks,
        water_delay: scenario.water_delay,
        lava_delay: scenario.lava_delay,
        game_time: baseline.game_time.clone(),
        box_cells: initial.totals(),
        vanilla_changed: changed,
        start,
        after,
        saved,
        timing: format!(
            "open+setup {:.1}s, ticks {:.1}s, save+reload {:.1}s, total {:.1}s",
            setup_done.as_secs_f64(),
            ticks_done.saturating_sub(setup_done).as_secs_f64(),
            total.saturating_sub(ticks_done).as_secs_f64(),
            total.as_secs_f64()
        ),
    })
}

/// One cell of our live world, in the same form the dump files use.
fn our_cell(game: &Game, blocks: &BlockRegistry, x: i32, y: i32, z: i32) -> String {
    let id = game.world().get_block(x, y, z);
    let state = blocks
        .state_ref_of(id)
        .unwrap_or_else(|error| panic!("our world holds state id {id}: {error}"));
    format_state(&state.name, &state.properties)
}

#[test]
fn the_spec_holds_exactly_the_five_frozen_scenarios() {
    let spec = load_spec();
    let names: Vec<&str> = spec.scenario.iter().map(|row| row.name.as_str()).collect();
    assert_eq!(
        names, FROZEN,
        "ADR-0009 section 2.6 freezes these five names, in this order; renaming, swapping or \
         adding a scenario would make `compared 5 / skipped 0` unfalsifiable"
    );
    assert_eq!(spec.version, 1, "the spec's format version");
    for row in &spec.scenario {
        assert!(
            row.ticks == 40 || row.ticks_reason.is_some(),
            "{} runs {} ticks: the ADR's N is 40, and a row that needs a longer window must say \
             which jar-stated delay it is waiting for",
            row.name,
            row.ticks
        );
        assert!(
            row.water_delay > 0 && row.lava_delay > 0,
            "{} must record the jar-stated spread delays",
            row.name
        );
        assert!(
            !row.exercises.trim().is_empty(),
            "{} must say what it exercises",
            row.name
        );
        assert!(!row.setup.is_empty(), "{} has no setup", row.name);
    }
    println!(
        "spec: {} scenarios, box {:?}, world seed {}",
        spec.scenario.len(),
        spec.bounds,
        spec.world.seed
    );
}

#[test]
fn every_scenario_matches_the_vanilla_server_cell_for_cell() {
    let spec = load_spec();
    let mut outcomes = Vec::new();
    let mut failures = Vec::new();
    let mut skipped = Vec::new();

    let blocks = match Registries::vanilla() {
        Ok(registries) => registries.blocks,
        Err(error) => panic!("the shipped registry tables do not load: {error}"),
    };
    let started = std::time::Instant::now();
    let (_frame_dir, frame_world) = build_frame_world(&spec, &blocks);
    println!(
        "frame built once in {:.1}s: {}",
        started.elapsed().as_secs_f64(),
        frame_world.display()
    );

    for scenario in &spec.scenario {
        match run_scenario(&spec, &blocks, scenario, &frame_world) {
            Ok(outcome) => {
                println!(
                    "\n=== {} ({} ticks; water delay {}t, lava delay {}t) ===\n    {}",
                    outcome.name,
                    outcome.ticks,
                    outcome.water_delay,
                    outcome.lava_delay,
                    scenario.exercises.replace('\n', " ")
                );
                println!(
                    "    vanilla changed {} of {} cells over the window ({})",
                    outcome.vanilla_changed, outcome.box_cells, outcome.game_time
                );
                println!("    tick 0      : {}", outcome.start.summary());
                println!("    after ticks : {}", outcome.after.summary());
                println!("    after save  : {}", outcome.saved.summary());
                println!("    cost        : {}", outcome.timing);
                if !outcome.start.exact() {
                    failures.push(format!(
                        "{}: our world does not start from the vanilla state ({}), so nothing \
                         after it is a fluid finding; the setup path is at fault first",
                        outcome.name,
                        outcome.start.summary()
                    ));
                }
                if !outcome.after.exact() {
                    failures.push(format!(
                        "{}: after {} ticks our engine differs from the vanilla server at {}",
                        outcome.name,
                        outcome.ticks,
                        outcome.after.summary()
                    ));
                }
                if !outcome.saved.exact() {
                    failures.push(format!(
                        "{}: the state read back from our saved chunk differs at {}",
                        outcome.name,
                        outcome.saved.summary()
                    ));
                }
                outcomes.push(outcome);
            }
            Err(error) => skipped.push(format!("{}: {error}", scenario.name)),
        }
    }

    assert!(
        skipped.is_empty(),
        "{} of {} scenarios could not be compared at all:\n  {}",
        skipped.len(),
        spec.scenario.len(),
        skipped.join("\n  ")
    );
    assert_eq!(
        outcomes.len(),
        FROZEN.len(),
        "compared {} / skipped {}; ADR-0009 section 2.6 pins compared 5 / skipped 0",
        outcomes.len(),
        skipped.len()
    );
    assert!(
        failures.is_empty(),
        "the fluid differential is red — this is a finding against our engine, not a fixture \
         to edit:\n  {}",
        failures.join("\n  ")
    );
}
