//! Bounded A* pathfinding for walking mobs, against a caller-supplied block view.
//!
//! ## Why the world arrives as a trait (AGENTS.md §3.4)
//!
//! `mc-entity` owns mob state, not block storage. The pathfinder therefore asks
//! one question — "is this block solid?" — through [`BlockView`], and the game
//! loop answers it from whatever it holds. The same boundary is used by
//! [`crate::mob::Rng`] for randomness, for the same reason: a crate that reaches
//! into the world layer cannot be unit-tested without one.
//!
//! ```
//! use mc_entity::pathfind::{BlockView, SearchLimits, find_path};
//!
//! struct Flat;
//! impl BlockView for Flat {
//!     fn is_solid(&self, _x: i32, y: i32, _z: i32) -> bool {
//!         y <= 63
//!     }
//! }
//!
//! let path = find_path(&Flat, (0, 64, 0), (5, 64, 0), SearchLimits::default());
//! assert_eq!(path.map(|path| path.len()), Some(5));
//! ```
//!
//! ## What this is, and what it is not
//!
//! It is a **grid A\*** over block cells, with the moves a walking mob can make:
//!
//! - one step to each of the four horizontal neighbours, at the same level;
//! - one step up, when the destination is standable and the mob's current cell has
//!   the head room to jump;
//! - a fall of up to [`MAX_FALL`] blocks, when the whole column between is clear
//!   and the landing is standable.
//!
//! Diagonals are refused, not overlooked. A diagonal step from `(0, 0)` to
//! `(1, 1)` is a move through the corner shared by `(1, 0)` and `(0, 1)`; if either
//! is solid the mob would clip it, and Vanilla mobs do not cut corners through
//! solid blocks.
//!
//! It is **not** Vanilla's pathfinder. Vanilla runs a specialised
//! `PathNavigation` with per-mob node evaluators (swim, fly, amphibious, climb),
//! door and fence-post handling, `PathFinder`'s own tie-breaking and a
//! `pathfinding` budget shared across mobs. None of that is claimed here.
//!
//! ## Deliberate simplifications (AGENTS.md §3.3)
//!
//! - **No mob width.** A cell is judged by [`SearchLimits::clearance`] *height*
//!   only, so a spider (1.4 wide) fits through a one-block gap in this model.
//! - **A `clearance` of 2 works on flat ground, but a *step up* onto ground the
//!   mob could not stand on is not modelled** (Vanilla's `PathNodeEvaluator`
//!   checks the space above the step). The step-up branch requires the mob's own
//!   head room at its current cell, which is the same thing for a mob standing on
//!   a floor; it does not simulate the jump arc.
//! - **No cost model beyond distance.** Vanilla weights water, doors, dangerous
//!   blocks and `Malus` penalties; every step here costs 1 and a fall costs
//!   `1 + blocks fallen`.
//! - **No path smoothing, no partial paths.** A* either reaches the goal or
//!   returns `None`; Vanilla's navigation keeps the best node found so far and
//!   re-plans, which is the caller's business here.
//! - **No water, lava, ladders, doors or climbable blocks.** Only solid/not
//!   solid is asked.
//! - **The start cell is not validated.** A mob already inside a block can still
//!   path out, because refusing would strand it.
//!
//! ## Cost and determinism
//!
//! The search is bounded by [`SearchLimits`]: at most [`SearchLimits::max_nodes`]
//! nodes are expanded and the returned path is at most
//! [`SearchLimits::max_path_len`] steps. Both bounds are enforced on every input,
//! including hostile ones, because a mob whose goal came from a corrupted chunk
//! must not be able to stall the tick thread (AGENTS.md §9, §10).
//!
//! Ties in the frontier are broken by `(f, x, z, y)`, ascending, through a total
//! [`Ord`] on the heap node — so two runs on the same inputs return the same path
//! (AGENTS.md §3.6). Every collection in the search is a `BTreeMap`/`BTreeSet`
//! (never a `HashMap`) for the same reason.

use crate::mob::MobKind;
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet, BinaryHeap};

/// A block position, used locally to keep the signatures readable.
type Pos = (i32, i32, i32);

/// The four horizontal moves a walking mob can make, in a fixed order.
///
/// The order is part of the deterministic contract: neighbours are generated in
/// it, and two equal-cost paths are resolved by the frontier's tie-break, not by
/// accident of enumeration.
const DIRECTIONS: [(i32, i32); 4] = [(-1, 0), (1, 0), (0, -1), (0, 1)];

/// How far up a walking mob can step in one move.
pub const MAX_STEP_UP: i32 = 1;

/// How far down a walking mob will drop in one move.
///
/// Vanilla mobs path down cliffs of up to three blocks without hesitation and
/// take fall damage beyond that. Refusing taller drops here keeps the mob on
/// walkable ground; it is not a fall-damage model.
pub const MAX_FALL: i32 = 3;

/// The most vertical space the search will ever require, whatever a caller asks
/// for: [`SearchLimits::for_mob`] clamps to it so a hostile value cannot make the
/// search read a tower of blocks.
pub const MAX_CLEARANCE: u8 = 4;

/// The clamp in [`MAX_CLEARANCE`] is only a clamp if it is below the type's
/// maximum; checked at compile time so a later edit cannot quietly disable it.
const _: () = assert!(MAX_CLEARANCE < u8::MAX);

/// Minimum nodes the search will expand, after clamping.
pub const DEFAULT_MAX_NODES: usize = 512;

/// Default path-length cap, in steps (the returned path excludes the start, so
/// this is also its maximum `len()`).
pub const DEFAULT_MAX_PATH_LEN: i32 = 64;

/// Default clearance for a mob two blocks tall.
pub const DEFAULT_CLEARANCE: u8 = 2;

/// The blocks the pathfinder is allowed to ask about.
///
/// One method, one question: solid means "blocks a walking mob". Air, water,
/// torches and every non-full-cube shape must answer `false`, and an unknown
/// block should answer `true` — failing safe is what keeps a mob out of a block
/// the caller cannot classify.
///
/// The game loop implements this for its world type; tests implement it over a
/// flat grid or a set of solid cells. Nothing in this crate implements it,
/// because nothing in this crate owns blocks.
pub trait BlockView {
    /// Whether the block at `(x, y, z)` blocks movement.
    fn is_solid(&self, x: i32, y: i32, z: i32) -> bool;
}

/// How much work [`find_path`] is allowed to do.
///
/// Every field is clamped into a sane range before use, so a caller may pass a
/// value straight from configuration or from a corrupted save without risking a
/// panic or an unbounded loop (AGENTS.md §9).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SearchLimits {
    /// Maximum number of nodes expanded before giving up. Clamped to at least 1.
    pub max_nodes: usize,
    /// Maximum number of steps in the returned path. Clamped to at least 1.
    pub max_path_len: i32,
    /// Vertical space a mob needs, in blocks. Clamped into `1..=`
    /// [`MAX_CLEARANCE`], so a hostile value cannot make the search read a tower.
    pub clearance: u8,
}

impl SearchLimits {
    /// The limits a two-block-tall mob uses on flat ground.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            max_nodes: DEFAULT_MAX_NODES,
            max_path_len: DEFAULT_MAX_PATH_LEN,
            clearance: DEFAULT_CLEARANCE,
        }
    }

    /// Limits sized for one mob kind, with `max_path_len` in steps.
    ///
    /// The clearance is [`MobKind::clearance`] (a chicken needs one block, a
    /// zombie two). Every other field takes its default; a caller that wants
    /// tighter bounds sets them afterwards.
    #[must_use]
    pub const fn for_mob(kind: MobKind, max_path_len: i32) -> Self {
        Self {
            max_nodes: DEFAULT_MAX_NODES,
            max_path_len,
            clearance: kind.clearance(),
        }
    }

    /// These limits with every field inside its clamped range.
    ///
    /// Private: clamping is part of the search's contract, not a value the caller
    /// has to reason about.
    fn clamped(self) -> Self {
        Self {
            max_nodes: self.max_nodes.max(1),
            max_path_len: self.max_path_len.max(1),
            clearance: self.clearance.clamp(1, MAX_CLEARANCE),
        }
    }
}

impl Default for SearchLimits {
    /// Equivalent to [`SearchLimits::new`].
    fn default() -> Self {
        Self::new()
    }
}

/// What one search actually did, for tests and for tick-budget diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SearchStats {
    /// Nodes expanded (popped and not already closed), excluding the goal test.
    pub expanded: u64,
    /// Distinct cells that entered the frontier.
    pub visited: u64,
    /// Blocks read through the [`BlockView`].
    pub block_reads: u64,
}

/// Find a walking path from `start` to `goal`, excluding the start.
///
/// Returns `None` when the goal is unreachable within `limits`, when the goal is
/// not a cell a mob could stand in, or when the straight-line distance alone
/// makes the search hopeless — a goal 10 000 blocks away is refused after reading
/// **zero** blocks rather than expanded to the node cap (AGENTS.md §9, §10).
///
/// A `start` equal to `goal` yields an empty path. The start cell itself is not
/// validated; see the module note.
#[must_use]
pub fn find_path(
    view: &impl BlockView,
    start: Pos,
    goal: Pos,
    limits: SearchLimits,
) -> Option<Vec<Pos>> {
    find_path_with_stats(view, start, goal, limits).0
}

/// [`find_path`], plus what the search cost.
///
/// Same search, same result; the statistics exist so a caller (or a test) can see
/// how close a query came to its limits without timing it.
#[must_use]
pub fn find_path_with_stats(
    view: &impl BlockView,
    start: Pos,
    goal: Pos,
    limits: SearchLimits,
) -> (Option<Vec<Pos>>, SearchStats) {
    let limits = limits.clamped();
    let mut stats = SearchStats::default();

    // Already there.
    if start == goal {
        return (Some(Vec::new()), stats);
    }
    // Vertical refusals, before anything else: a goal far below is a drop no
    // walking mob will take.
    if i64::from(start.1) - i64::from(goal.1) > i64::from(MAX_FALL) {
        return (None, stats);
    }
    // Straight-line refusals. Each move changes the Manhattan distance by
    // exactly one, so a path shorter than it cannot exist, and a goal further
    // away than `max_path_len` is unreachable by definition. Checked in `i64`:
    // `goal - start` overflows `i32` for hostile coordinates.
    let manhattan = manhattan(start, goal);
    if manhattan > i64::from(limits.max_path_len) {
        return (None, stats);
    }
    // The goal must be somewhere a mob could stand, or no amount of searching
    // will help.
    if !is_standable(view, goal, limits.clearance) {
        stats.block_reads += u64::from(limits.clearance) + 1;
        return (None, stats);
    }

    match astar(view, start, goal, limits, &mut stats) {
        // A path can be longer than the straight line when it has to go around
        // something; the cap applies to the path, not just to the estimate.
        Some(path) if path.len() > limits.max_path_len as usize => (None, stats),
        other => (other, stats),
    }
}

/// The search itself. `start != goal`, the goal is standable, and the
/// straight-line distance is within `max_path_len` by the time this runs.
fn astar(
    view: &impl BlockView,
    start: Pos,
    goal: Pos,
    limits: SearchLimits,
    stats: &mut SearchStats,
) -> Option<Vec<Pos>> {
    let mut frontier: BinaryHeap<std::cmp::Reverse<Node>> = BinaryHeap::new();
    let mut came_from: BTreeMap<Pos, Pos> = BTreeMap::new();
    let mut best_cost: BTreeMap<Pos, i64> = BTreeMap::new();
    let mut closed: BTreeSet<Pos> = BTreeSet::new();

    best_cost.insert(start, 0);
    frontier.push(std::cmp::Reverse(Node::new(0, start, goal)));

    while let Some(std::cmp::Reverse(node)) = frontier.pop() {
        let current = (node.x, node.y, node.z);
        if current == goal {
            return reconstruct(&came_from, start, goal);
        }
        if !closed.insert(current) {
            // A stale frontier entry: the cell was expanded at a lower cost.
            continue;
        }
        if stats.expanded >= limits.max_nodes as u64 {
            // The budget is spent; the mob simply does not get a path this tick.
            return None;
        }
        stats.expanded += 1;
        let cost_here = best_cost.get(&current).copied().unwrap_or(0);

        for neighbour in neighbours(view, current, limits.clearance, stats) {
            let (next, step_cost) = neighbour;
            if closed.contains(&next) {
                continue;
            }
            let tentative = cost_here + step_cost;
            let slot = best_cost.entry(next).or_insert(i64::MAX);
            if tentative < *slot {
                *slot = tentative;
                stats.visited += 1;
                came_from.insert(next, current);
                frontier.push(std::cmp::Reverse(Node::new(tentative, next, goal)));
            }
        }
    }
    // The frontier ran dry: the goal is walled off.
    None
}

/// Walk the parent map back from `goal` to `start`, reversed and without the
/// start (the documented shape of a path).
fn reconstruct(came_from: &BTreeMap<Pos, Pos>, start: Pos, goal: Pos) -> Option<Vec<Pos>> {
    let mut reversed = Vec::new();
    let mut current = goal;
    while current != start {
        reversed.push(current);
        current = *came_from.get(&current)?;
    }
    reversed.reverse();
    Some(reversed)
}

/// The moves out of `cell`, in a fixed order, each with its cost.
///
/// Per direction the branches are tried in a fixed order — level, step up, fall —
/// and the first that works wins. A level step is preferred over a step up even
/// when both land on the same y (they cannot: a step up is only tried when the
/// level cell is not standable), and both are preferred over a drop, which is
/// what keeps a mob from hopping off a ledge it could have walked past.
fn neighbours(
    view: &impl BlockView,
    cell: Pos,
    clearance: u8,
    stats: &mut SearchStats,
) -> Vec<(Pos, i64)> {
    let (x, _, z) = cell;
    let mut moves = Vec::with_capacity(4);
    for (dx, dz) in DIRECTIONS {
        let (Some(nx), Some(nz)) = (x.checked_add(dx), z.checked_add(dz)) else {
            // A neighbour off the edge of the coordinate space: not a move.
            continue;
        };
        if let Some(landing) = step_to(view, cell, nx, nz, clearance, stats) {
            moves.push(landing);
        }
    }
    moves
}

/// The destination a mob at `cell` reaches by moving to `(nx, nz)`, if any.
///
/// Also enforces the step-up head-room rule: a mob whose own cell is blocked
/// above cannot jump.
fn step_to(
    view: &impl BlockView,
    cell: Pos,
    nx: i32,
    nz: i32,
    clearance: u8,
    stats: &mut SearchStats,
) -> Option<(Pos, i64)> {
    let (_, y, _) = cell;
    // Level.
    if is_standable_counted(view, (nx, y, nz), clearance, stats) {
        return Some(((nx, y, nz), 1));
    }
    // Step up, if the mob has the room to jump in its current cell.
    if let (Some(up), Some(head)) = (
        y.checked_add(MAX_STEP_UP),
        y.checked_add(i32::from(clearance)),
    ) && !view.is_solid(cell.0, head, cell.2)
    {
        stats.block_reads += 1;
        if is_standable_counted(view, (nx, up, nz), clearance, stats) {
            return Some(((nx, up, nz), 1));
        }
    }
    // Fall.
    for fall in 1..=MAX_FALL {
        let Some(landing_y) = y.checked_sub(fall) else {
            break;
        };
        if !fall_is_clear(view, nx, nz, y, landing_y, clearance, stats) {
            continue;
        }
        if is_standable_counted(view, (nx, landing_y, nz), clearance, stats) {
            return Some(((nx, landing_y, nz), 1 + i64::from(fall)));
        }
    }
    None
}

/// Whether a mob with `clearance` blocks of height could stand in `cell`:
/// every cell of its body is free and the block under its feet is solid.
///
/// Refuses coordinates at the edge of the `i32` range instead of wrapping.
fn is_standable(view: &impl BlockView, cell: Pos, clearance: u8) -> bool {
    is_standable_counted(view, cell, clearance, &mut SearchStats::default())
}

/// [`is_standable`], counting the reads.
fn is_standable_counted(
    view: &impl BlockView,
    cell: Pos,
    clearance: u8,
    stats: &mut SearchStats,
) -> bool {
    let (x, y, z) = cell;
    for step in 0..i32::from(clearance) {
        let Some(level) = y.checked_add(step) else {
            return false;
        };
        // Counted before the call so the tally is exactly the number of
        // `is_solid` calls this search made.
        stats.block_reads += 1;
        if view.is_solid(x, level, z) {
            return false;
        }
    }
    let Some(below) = y.checked_sub(1) else {
        return false;
    };
    stats.block_reads += 1;
    view.is_solid(x, below, z)
}

/// Whether the column a mob falls through is clear, above the landing cell.
///
/// The landing cell's own body space is checked by [`is_standable_counted`]; this
/// covers the blocks it passes on the way down, up to its current head height.
fn fall_is_clear(
    view: &impl BlockView,
    x: i32,
    z: i32,
    from_y: i32,
    landing_y: i32,
    clearance: u8,
    stats: &mut SearchStats,
) -> bool {
    let Some(top) = from_y.checked_add(i32::from(clearance)) else {
        return false;
    };
    let mut level = landing_y;
    while level < top {
        if view.is_solid(x, level, z) {
            stats.block_reads += 1;
            return false;
        }
        stats.block_reads += 1;
        let Some(next) = level.checked_add(1) else {
            return false;
        };
        level = next;
    }
    true
}

/// Manhattan distance, saturating in `i64` so hostile coordinates cannot wrap.
fn manhattan(from: Pos, to: Pos) -> i64 {
    let dx = (i64::from(to.0) - i64::from(from.0)).abs();
    let dy = (i64::from(to.1) - i64::from(from.1)).abs();
    let dz = (i64::from(to.2) - i64::from(from.2)).abs();
    dx.saturating_add(dy).saturating_add(dz)
}

/// One entry in the A* frontier.
///
/// The [`Ord`] implementation is the determinism contract: entries compare by
/// `f`, then by `x`, `z`, `y` ascending. `BinaryHeap` is a max-heap, so a
/// [`std::cmp::Reverse`] wrapper turns this into "lowest `f` first, ties broken
/// by the lowest `(x, z, y)` lexicographically". Because the order is total (no
/// two distinct cells can compare equal), the pop order is a function of the
/// input alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Node {
    f: i64,
    x: i32,
    y: i32,
    z: i32,
}

impl Node {
    /// A frontier entry for `cell`, scored `g + h`.
    fn new(g: i64, cell: Pos, goal: Pos) -> Self {
        Self {
            f: g.saturating_add(manhattan(cell, goal)),
            x: cell.0,
            y: cell.1,
            z: cell.2,
        }
    }
}

impl Ord for Node {
    fn cmp(&self, other: &Self) -> Ordering {
        self.f
            .cmp(&other.f)
            .then_with(|| self.x.cmp(&other.x))
            .then_with(|| self.z.cmp(&other.z))
            .then_with(|| self.y.cmp(&other.y))
    }
}

impl PartialOrd for Node {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[cfg(test)]
mod tests {
    use super::{
        BlockView, DEFAULT_MAX_NODES, DEFAULT_MAX_PATH_LEN, MAX_FALL, MAX_STEP_UP, MobKind,
        SearchLimits, SearchStats, find_path, find_path_with_stats,
    };
    use std::cell::Cell;
    use std::collections::BTreeSet;

    /// The topmost solid layer of the test world. A mob stands at y = 64.
    const GROUND_Y: i32 = 63;
    /// A cell a walking mob stands in.
    const FEET_Y: i32 = GROUND_Y + 1;

    /// A flat world with optional extra solid blocks and optional holes.
    ///
    /// Everything at or below [`GROUND_Y`] is solid unless it is a hole; anything
    /// above is air unless it is an extra solid block. Counts every block read, so
    /// a test can prove the search refused a query *before* touching the world.
    struct FlatWorld {
        extra_solid: BTreeSet<(i32, i32, i32)>,
        holes: BTreeSet<(i32, i32, i32)>,
        reads: Cell<u64>,
    }

    impl FlatWorld {
        fn new() -> Self {
            Self {
                extra_solid: BTreeSet::new(),
                holes: BTreeSet::new(),
                reads: Cell::new(0),
            }
        }

        /// A wall of solid blocks at `x`, from `y0` to `y1` inclusive, spanning
        /// `z0..=z1`.
        fn wall(mut self, x: i32, y0: i32, y1: i32, z0: i32, z1: i32) -> Self {
            for y in y0..=y1 {
                for z in z0..=z1 {
                    self.extra_solid.insert((x, y, z));
                }
            }
            self
        }

        /// A hole in the ground at `(x, z)` covering `y0..=y1`.
        ///
        /// The two bounds are sorted, so a caller may name the top of the hole
        /// first; `63..=61` is an empty range in Rust and would otherwise create
        /// no hole at all, silently turning a test into a no-op.
        fn hole(mut self, x: i32, z: i32, y0: i32, y1: i32) -> Self {
            for y in y0.min(y1)..=y0.max(y1) {
                self.holes.insert((x, y, z));
            }
            self
        }

        fn reads(&self) -> u64 {
            self.reads.get()
        }
    }

    impl BlockView for FlatWorld {
        fn is_solid(&self, x: i32, y: i32, z: i32) -> bool {
            self.reads.set(self.reads.get() + 1);
            let pos = (x, y, z);
            if self.extra_solid.contains(&pos) {
                return true;
            }
            if self.holes.contains(&pos) {
                return false;
            }
            y <= GROUND_Y
        }
    }

    /// Every block cell a path visits must be walkable: it is air, and its
    /// neighbours in the path are never more than one step away.
    fn assert_path_is_walkable(view: &FlatWorld, path: &[(i32, i32, i32)], clearance: u8) {
        for &(x, y, z) in path {
            for step in 0..i32::from(clearance) {
                assert!(
                    !view.is_solid(x, y + step, z),
                    "path enters solid block ({x}, {}, {z})",
                    y + step
                );
            }
            assert!(
                view.is_solid(x, y - 1, z),
                "path cell ({x}, {y}, {z}) has no floor"
            );
        }
    }

    #[test]
    fn limits_are_sane_and_clamp_hostile_values() {
        let limits = SearchLimits::new();
        assert_eq!(limits.max_nodes, DEFAULT_MAX_NODES);
        assert_eq!(limits.max_path_len, DEFAULT_MAX_PATH_LEN);
        assert_eq!(limits.clearance, 2);
        assert_eq!(SearchLimits::default(), limits);
        for kind in MobKind::ALL {
            let sized = SearchLimits::for_mob(kind, 32);
            assert_eq!(sized.clearance, kind.clearance());
            assert_eq!(sized.max_nodes, DEFAULT_MAX_NODES);
            assert_eq!(sized.max_path_len, 32);
        }
        assert_eq!(MAX_STEP_UP, 1);
        assert_eq!(MAX_FALL, 3);
    }

    #[test]
    fn a_straight_line_is_found_on_flat_ground() {
        let world = FlatWorld::new();
        let path = find_path(
            &world,
            (0, FEET_Y, 0),
            (5, FEET_Y, 0),
            SearchLimits::default(),
        )
        .expect("a straight line on flat ground");
        assert_eq!(
            path,
            vec![
                (1, FEET_Y, 0),
                (2, FEET_Y, 0),
                (3, FEET_Y, 0),
                (4, FEET_Y, 0),
                (5, FEET_Y, 0)
            ],
            "the path excludes the start and is exactly the straight line"
        );
        assert_path_is_walkable(&world, &path, 2);

        // A start equal to the goal is an empty path, not `None`.
        assert_eq!(
            find_path(
                &world,
                (2, FEET_Y, 2),
                (2, FEET_Y, 2),
                SearchLimits::default()
            ),
            Some(Vec::new())
        );
    }

    #[test]
    fn a_one_block_step_up_is_taken() {
        // A single block at (3, 64, 0): the mob steps onto it and down the far side.
        let world = FlatWorld::new().wall(3, FEET_Y, FEET_Y, 0, 0);
        let path = find_path(
            &world,
            (0, FEET_Y, 0),
            (5, FEET_Y, 0),
            SearchLimits::default(),
        )
        .expect("a one-block step is a normal move");
        assert_eq!(path.len(), 5, "{path:?}");
        assert!(
            path.contains(&(3, FEET_Y + 1, 0)),
            "the path walks over the block: {path:?}"
        );
        assert_path_is_walkable(&world, &path, 2);
    }

    #[test]
    fn a_two_block_wall_is_routed_around_or_refused() {
        // A wall two blocks tall and five long: too tall to step onto, so the only
        // way past is around the end. The node budget is raised so that this test
        // measures the *path* length and not the search budget.
        let build = || FlatWorld::new().wall(3, FEET_Y, FEET_Y + 1, -2, 2);
        let roomy = SearchLimits {
            max_nodes: 4096,
            ..SearchLimits::default()
        };

        // With room to manoeuvre, the path goes around and stays out of the wall.
        let world = build();
        let path = find_path(&world, (0, FEET_Y, 0), (5, FEET_Y, 0), roomy)
            .expect("a detour within the limits");
        assert!(
            path.len() > 5,
            "going around a wall must be longer than the straight line: {path:?}"
        );
        assert_path_is_walkable(&world, &path, 2);
        assert!(
            path.iter()
                .all(|&(x, _, z)| x != 3 || !(-2..=2).contains(&z)),
            "the path may not pass through the wall: {path:?}"
        );

        // With a path-length cap below the detour, the same query is refused
        // rather than answered with an over-long path. Same node budget, so the
        // cap is what decides.
        let tight = SearchLimits {
            max_path_len: 6,
            ..roomy
        };
        assert_eq!(
            find_path(&world, (0, FEET_Y, 0), (5, FEET_Y, 0), tight),
            None
        );
    }

    #[test]
    fn falls_of_up_to_three_blocks_are_allowed_and_deeper_ones_are_not() {
        // Landing at y = 61 is a three-block drop; landing at y = 60 is four.
        let three = FlatWorld::new().hole(3, 0, FEET_Y - 1, FEET_Y - 3);
        let four = FlatWorld::new().hole(3, 0, FEET_Y - 1, FEET_Y - 4);
        let start = (0, FEET_Y, 0);
        let landing = (3, FEET_Y - 3, 0);

        assert!(
            find_path(&three, start, landing, SearchLimits::default()).is_some(),
            "a three-block drop is a legal move"
        );
        let deep = landing;
        let deep = (deep.0, deep.1 - 1, deep.2);
        assert_eq!(
            find_path(&four, start, deep, SearchLimits::default()),
            None,
            "a four-block drop is refused"
        );
        // A goal directly below by more than MAX_FALL is refused up front.
        assert_eq!(
            find_path(
                &FlatWorld::new(),
                (0, FEET_Y, 0),
                (0, FEET_Y - MAX_FALL - 1, 0),
                SearchLimits::default()
            ),
            None
        );
        // …and the landing cell of the deep hole is not standable at all.
        assert_eq!(
            find_path(&four, start, (3, FEET_Y - 2, 0), SearchLimits::default()),
            None,
            "the cell in the middle of a four-deep shaft has no floor"
        );
    }

    #[test]
    fn an_unreachable_goal_returns_none_within_the_node_cap() {
        // A wall 41 blocks long and two tall: the way round is far longer than the
        // path cap, so the search must exhaust its budget and give up.
        let world = FlatWorld::new().wall(3, FEET_Y, FEET_Y + 1, -20, 20);
        let limits = SearchLimits {
            max_nodes: 64,
            max_path_len: 8,
            clearance: 2,
        };
        let (path, stats) = find_path_with_stats(&world, (0, FEET_Y, 0), (5, FEET_Y, 0), limits);
        assert_eq!(path, None);
        assert!(
            stats.expanded <= 64,
            "the node cap must bound the search: {}",
            stats.expanded
        );
        assert!(stats.block_reads > 0, "the search did look at the world");
    }

    #[test]
    fn a_goal_ten_thousand_blocks_away_is_refused_without_reading_a_block() {
        let world = FlatWorld::new();
        let (path, stats) = find_path_with_stats(
            &world,
            (0, FEET_Y, 0),
            (10_000, FEET_Y, 0),
            SearchLimits::default(),
        );
        assert_eq!(path, None);
        assert_eq!(stats.expanded, 0);
        assert_eq!(stats.visited, 0);
        assert_eq!(
            world.reads(),
            0,
            "the distance check must run before any block read"
        );
        // The same for a goal above and below.
        assert_eq!(
            find_path(
                &world,
                (0, FEET_Y, 0),
                (0, 40_000, 0),
                SearchLimits::default()
            ),
            None
        );
        assert_eq!(world.reads(), 0);
    }

    #[test]
    fn the_same_query_returns_the_same_path_twice() {
        let world = FlatWorld::new().wall(3, FEET_Y, FEET_Y + 1, -2, 2);
        // A budget with room to find the detour, so this test is about the path
        // and not about the search running out of nodes.
        let roomy = SearchLimits {
            max_nodes: 4096,
            ..SearchLimits::default()
        };
        let query = || find_path(&world, (0, FEET_Y, 0), (5, FEET_Y, 2), roomy);
        let first = query().expect("a path around the wall");
        assert_eq!(
            Some(first.clone()),
            query(),
            "A* ties must break the same way"
        );
        // Different inputs are allowed to differ, obviously — this guards against
        // a search that ignores its arguments.
        assert_ne!(
            Some(first),
            find_path(&world, (0, FEET_Y, 0), (0, FEET_Y, 5), roomy)
        );
    }

    #[test]
    fn paths_never_enter_solid_blocks_in_a_maze() {
        // A zig-zag corridor: two walls with offset gaps, plus a raised section.
        let world = FlatWorld::new()
            .wall(3, FEET_Y, FEET_Y, -10, 1)
            .wall(7, FEET_Y, FEET_Y, -1, 10)
            .wall(10, FEET_Y, FEET_Y, -10, 10);
        let path = find_path(
            &world,
            (0, FEET_Y, 0),
            (12, FEET_Y, 0),
            SearchLimits::default(),
        )
        .expect("the corridor is walkable");
        assert_path_is_walkable(&world, &path, 2);
        assert_eq!(path.last(), Some(&(12, FEET_Y, 0)));
        assert!(
            path.iter().any(|&(_, y, _)| y > FEET_Y),
            "at least one of the single-block walls must be stepped over"
        );
    }

    #[test]
    fn hostile_limits_and_coordinates_do_not_panic() {
        let world = FlatWorld::new();
        let start = (0, FEET_Y, 0);
        let goal = (5, FEET_Y, 0);
        let nodes = [0usize, 1, usize::MAX];
        let lengths = [i32::MIN, -1, 0, 1, i32::MAX];
        let clearances = [0u8, 1, 2, 255];
        for max_nodes in nodes {
            for max_path_len in lengths {
                for clearance in clearances {
                    let limits = SearchLimits {
                        max_nodes,
                        max_path_len,
                        clearance,
                    };
                    // No panic, and whatever comes back must satisfy the caps the
                    // search documents (after clamping).
                    if let Some(path) = find_path(&world, start, goal, limits) {
                        assert!(path.len() <= max_path_len.max(1) as usize);
                        assert!(!path.is_empty());
                    }
                }
            }
        }
        // A zero node budget is clamped to one, so the search expands exactly one
        // node and then gives up rather than looping or panicking.
        let (path, stats) = find_path_with_stats(
            &world,
            start,
            goal,
            SearchLimits {
                max_nodes: 0,
                ..SearchLimits::default()
            },
        );
        assert_eq!(path, None);
        assert_eq!(stats.expanded, 1, "max_nodes 0 clamps to 1");

        // Clearance is clamped (the clamp itself is asserted at compile time next
        // to MAX_CLEARANCE), so a huge value stays cheap and correct.
        let huge = SearchLimits {
            clearance: u8::MAX,
            ..SearchLimits::default()
        };
        assert!(find_path(&world, start, goal, huge).is_some());

        // Coordinates at the edge of the `i32` range: neighbours that would wrap
        // are skipped, not wrapped.
        let edge = (i32::MAX, FEET_Y, i32::MAX);
        let next_to_edge = (i32::MAX - 1, FEET_Y, i32::MAX);
        assert_eq!(
            find_path(&world, edge, next_to_edge, SearchLimits::default()),
            Some(vec![next_to_edge])
        );
        // An absurd pair of corners is refused on distance alone.
        assert_eq!(
            find_path(
                &world,
                (i32::MIN, i32::MIN, i32::MIN),
                (i32::MAX, i32::MAX, i32::MAX),
                SearchLimits::default()
            ),
            None
        );
        // Extrema on one axis only: still refused, still no panic.
        assert_eq!(
            find_path(
                &world,
                (0, FEET_Y, i32::MIN),
                (0, FEET_Y, i32::MAX),
                SearchLimits::default()
            ),
            None
        );
    }

    #[test]
    fn a_blocked_cell_is_not_a_goal() {
        let world = FlatWorld::new().wall(2, FEET_Y, FEET_Y + 1, 0, 0);
        let (path, stats) = find_path_with_stats(
            &world,
            (0, FEET_Y, 0),
            (2, FEET_Y, 0),
            SearchLimits::default(),
        );
        assert_eq!(path, None, "a goal inside a wall is refused");
        assert_eq!(
            stats.expanded, 0,
            "the goal is checked before the search starts"
        );
        // A goal with no floor is refused too (the middle of a shaft).
        assert_eq!(
            find_path(
                &FlatWorld::new(),
                (0, FEET_Y, 0),
                (0, FEET_Y + 10, 0),
                SearchLimits::default()
            ),
            None
        );
    }

    #[test]
    fn blocking_a_corridor_makes_the_goal_unreachable() {
        // A closed box: the goal is inside a sealed ring of blocks.
        let world = FlatWorld::new()
            .wall(2, FEET_Y, FEET_Y + 2, -1, 1)
            .wall(4, FEET_Y, FEET_Y + 2, -1, 1)
            .wall(2, FEET_Y, FEET_Y + 2, 1, 1)
            .wall(3, FEET_Y, FEET_Y + 2, 1, 1)
            .wall(4, FEET_Y, FEET_Y + 2, 1, 1)
            .wall(2, FEET_Y, FEET_Y + 2, -1, -1)
            .wall(3, FEET_Y, FEET_Y + 2, -1, -1)
            .wall(4, FEET_Y, FEET_Y + 2, -1, -1)
            .wall(3, FEET_Y + 3, FEET_Y + 3, -1, 1);
        let (path, stats) = find_path_with_stats(
            &world,
            (0, FEET_Y, 0),
            (3, FEET_Y, 0),
            SearchLimits::default(),
        );
        assert_eq!(path, None, "a sealed cell cannot be reached");
        assert_eq!(
            stats.expanded, DEFAULT_MAX_NODES as u64,
            "the search uses its whole budget before giving up"
        );
        assert_ne!(stats, SearchStats::default());
    }

    #[test]
    fn two_block_clearance_is_required_overhead() {
        let start = (0, FEET_Y, 0);
        let goal = (4, FEET_Y, 0);
        let line = vec![
            (1, FEET_Y, 0),
            (2, FEET_Y, 0),
            (3, FEET_Y, 0),
            (4, FEET_Y, 0),
        ];

        // A structure two blocks above the floor blocks nothing: a two-block mob
        // needs y = 64 and y = 65 free, and both are.
        let headroom = FlatWorld::new().wall(2, FEET_Y + 2, FEET_Y + 2, -100, 100);
        assert_eq!(
            find_path(&headroom, start, goal, SearchLimits::default()),
            Some(line.clone())
        );

        // A solid layer at y = 65 leaves a one-block tunnel at y = 64 through a
        // wall that runs further than the path cap allows going around: only a mob
        // that fits in one block can use it.
        let tunnel = FlatWorld::new().wall(2, FEET_Y + 1, FEET_Y + 4, -100, 100);
        assert_eq!(
            find_path(&tunnel, start, goal, SearchLimits::default()),
            None,
            "a two-block mob cannot fit through a one-block tunnel"
        );
        let small = SearchLimits::for_mob(MobKind::Chicken, 16);
        assert_eq!(small.clearance, 1);
        assert_eq!(
            find_path(&tunnel, start, goal, small),
            Some(line),
            "the same tunnel is a corridor for a one-block mob"
        );
        // A taller wall is impassable for the small mob too, so the result above
        // is the clearance and not a hole in the search.
        let sealed = FlatWorld::new().wall(2, FEET_Y, FEET_Y + 4, -100, 100);
        assert_eq!(find_path(&sealed, start, goal, small), None);
    }
}
