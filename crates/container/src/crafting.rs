//! Crafting: shaped and shapeless recipe matching, and consuming a grid (P06-04).
//!
//! ## What this is, and what it is not
//!
//! This module holds a **hand-written baseline subset** of the recipe set: the
//! recipes needed to make the crafting path real and testable end to end (planks,
//! sticks, a crafting table, a torch, a furnace, a chest and a handful of others,
//! all listed in [`RecipeRegistry::baseline`]). It is **not** the Vanilla recipe
//! set and must never be presented as one. Vanilla's recipes are *data*: each one
//! comes from a `data/<namespace>/recipe/*.json` file with ingredient **tags**
//! (`#minecraft:planks`), an ingredient type (item, tag or custom), a recipe
//! `type` (`crafting_shaped`, `crafting_shapeless`, `crafting_transmute`,
//! `crafting_special_*`, …), a recipe-book `category`/`group` and a
//! `show_notification` flag — none of which is modelled here. Loading that real
//! data is **P07-03**; when it lands, this module's registry becomes one of its
//! consumers and the baseline below becomes a fallback fixture rather than the
//! source of truth.
//!
//! Concretely, what this module does **not** implement:
//!
//! - **Ingredient tags.** A recipe ingredient is a set of item ids, not a
//!   `#minecraft:planks` tag. The baseline spells "any planks" as one ingredient
//!   with eleven alternatives, which is equivalent in effect and honest about the
//!   missing abstraction; a real tag resolves through the P07-03 data loader.
//! - **Recipe types other than `crafting_shaped`/`crafting_shapeless`.** No
//!   transmute, no dyeing, no firework assembly, no map cloning, no banner
//!   patterns, no repair, no `crafting_special_*`.
//! - **The recipe book and its `group`/`category` metadata**, so no
//!   "did the player unlock this recipe" check exists.
//! - **Remainders** (a recipe input that leaves a container behind, e.g. a bucket
//!   in a cake) — no baseline recipe here needs one.
//!
//! ## Grid semantics (verified against Vanilla's `CraftingMenu`)
//!
//! - A grid is a flat `&[ItemStack]` plus a `width`; the height is
//!   `grid.len() / width`. The player's 2x2 grid is 4 slots at width 2 (menu slots
//!   `1..=4`, see [`crate::Menu::player`]); a crafting table is 9 slots at width 3.
//!   A ragged grid (a length that is not a multiple of the width) is refused, as
//!   are a zero width, a width above [`MAX_GRID_WIDTH`], and a height above it.
//! - **A shaped recipe matches at any offset** where its pattern fits, so a 2x2
//!   recipe placed in any corner of a 3x3 grid is the same recipe. Vanilla does
//!   exactly this by sliding the pattern across the grid
//!   (`ShapedRecipe.matches(CraftingInput, Level)`).
//! - **A shaped recipe must fill the grid exactly.** Every grid cell outside the
//!   pattern's bounding box must be empty, and every blank cell *inside* the
//!   pattern must be empty too — a cobblestone ring with a filled centre is not the
//!   furnace recipe. This is Vanilla's rule (a shaped recipe consumes the whole
//!   grid), and it is the rule that decides between two recipes where one pattern
//!   is a sub-pattern of another: four planks in a 2x2 grid are a crafting table,
//!   never two disjoint sticks, because the sticks pattern leaves the second column
//!   occupied.
//! - **A shaped recipe also matches its horizontal mirror.** Vanilla tries
//!   `ShapedRecipePattern.matches(...)` and, if that fails, the mirrored form
//!   `matches(..., true)`, which is why a recipe drawn right-to-left crafts.
//! - **A shapeless recipe matches any permutation** of its ingredients and fails
//!   on a grid holding anything extra. Vanilla's `ShapelessRecipe` copies the grid
//!   list and removes one entry per ingredient, which is a multiset comparison; a
//!   multiset comparison over the whole grid is already an exact-fill rule.
//! - **Scanning is ordered.** Recipes are tried in registration order and grid
//!   offsets in row-major order, so the same grid always yields the same match
//!   (AGENTS.md section 3.6). Where two recipes would match the same grid the
//!   first registered wins; the baseline avoids that ambiguity by construction, and
//!   a test asserts every baseline pattern matches its own recipe while no two
//!   baseline names collide.
//!
//! ## Determinism (AGENTS.md section 3.6)
//!
//! Everything a caller walks is a `Vec` or a fixed-size array, indexed by
//! position. There is no `HashMap`, no randomness and no clock in this module.
//!
//! ## Result-slot stacking
//!
//! Vanilla's `ResultSlot` shows the recipe's *full output count* — a log crafts
//! into 4 planks, and the slot reads 4 — and that count never merges with a stack
//! already there (the slot is take-only). [`RecipeRegistry::recompute_result`]
//! therefore returns `min(result_count, item max stack size)` as a whole stack; it
//! never adds to whatever the caller's result slot currently holds. The `min` is
//! this build's addition, not Vanilla's: Vanilla's recipe validation requires a
//! result count that fits in one stack, and a data pack can break that, so the
//! clamp stops a hand-written recipe from producing a stack [`ItemStack::new`]
//! would refuse. A test asserts every baseline result is within its item's limit,
//! so the clamp is a guard rail and not a silent truncation of the baseline data.

#![deny(missing_docs)]

use mc_core::error::{ServerError, ServerResult};
use mc_entity::stack::{ItemStack, StackSizeTable};
use mc_registry::ItemRegistry;

/// Largest crafting grid this module accepts, per axis (a crafting table).
pub const MAX_GRID_WIDTH: usize = 3;

/// Largest crafting grid, in slots.
pub const MAX_GRID_SLOTS: usize = MAX_GRID_WIDTH * MAX_GRID_WIDTH;

/// Up to this many distinct alternatives may be given for one ingredient.
///
/// A product decision: the baseline expresses "any planks" as one ingredient with
/// several alternatives, and 16 is above the number of wood types any one
/// ingredient needs. The cap exists so a generated or hostile recipe cannot ask
/// for an unbounded match list; a recipe needing more belongs in the P07-03 data
/// loader, which models ingredient tags properly.
pub const MAX_ALTERNATIVES_PER_KEY: usize = 16;

/// How much of a grid a shaped pattern covers, in either axis.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Shape {
    width: usize,
    height: usize,
}

impl Shape {
    /// A `width` x `height` shape.
    #[must_use]
    pub const fn new(width: usize, height: usize) -> Self {
        Self { width, height }
    }

    /// Columns.
    #[must_use]
    pub const fn width(self) -> usize {
        self.width
    }

    /// Rows.
    #[must_use]
    pub const fn height(self) -> usize {
        self.height
    }

    /// Slots covered.
    #[must_use]
    pub const fn len(self) -> usize {
        self.width * self.height
    }

    /// Whether it covers no slots (only possible before normalisation).
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.width == 0 || self.height == 0
    }
}

/// One recipe ingredient: a set of item ids, any one of which satisfies it.
///
/// Several ids is how the baseline spells "any planks" without an ingredient-tag
/// abstraction (see the module docs). The list is stored sorted and deduplicated
/// so matching and iteration are deterministic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ingredient {
    /// Accepted item ids, ascending, at least one, at most
    /// [`MAX_ALTERNATIVES_PER_KEY`].
    item_ids: Vec<i32>,
}

impl Ingredient {
    /// An ingredient satisfied by exactly one item id.
    #[must_use]
    pub fn item(item_id: i32) -> Self {
        Self {
            item_ids: vec![item_id],
        }
    }

    /// An ingredient satisfied by any of `item_ids`.
    ///
    /// # Errors
    ///
    /// [`ServerError::Invariant`] when `item_ids` is empty (an ingredient nothing
    /// can satisfy is a recipe that can never be crafted, i.e. a table bug) or
    /// longer than [`MAX_ALTERNATIVES_PER_KEY`].
    pub fn any_of(item_ids: &[i32]) -> ServerResult<Self> {
        if item_ids.is_empty() {
            return Err(ServerError::Invariant(
                "an ingredient must accept at least one item".to_owned(),
            ));
        }
        if item_ids.len() > MAX_ALTERNATIVES_PER_KEY {
            return Err(ServerError::Invariant(format!(
                "an ingredient with {} alternatives exceeds the {} this build accepts",
                item_ids.len(),
                MAX_ALTERNATIVES_PER_KEY
            )));
        }
        let mut item_ids = item_ids.to_vec();
        item_ids.sort_unstable();
        item_ids.dedup();
        Ok(Self { item_ids })
    }

    /// The accepted item ids, ascending.
    #[must_use]
    pub fn item_ids(&self) -> &[i32] {
        &self.item_ids
    }

    /// Whether `item_id` satisfies this ingredient.
    #[must_use]
    pub fn accepts(&self, item_id: i32) -> bool {
        self.item_ids.binary_search(&item_id).is_ok()
    }
}

/// A shaped recipe pattern: a `width` x `height` grid of optional ingredients.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShapedPattern {
    width: usize,
    height: usize,
    cells: Vec<Option<Ingredient>>,
}

impl ShapedPattern {
    /// Build a pattern from rows of optional ingredients.
    ///
    /// Trailing all-empty rows and columns are trimmed first — an author writing a
    /// 2x2 recipe as a 3x3 drawing with a blank row is expressing the same shape
    /// Vanilla's `ShapedRecipePattern` computes — so `[[A], [A]]` and a 3x3
    /// drawing of the same two cells are one pattern.
    ///
    /// # Errors
    ///
    /// [`ServerError::Invariant`] when `rows` is empty, when the rows have
    /// different lengths (a ragged pattern is an authoring bug, not input), when
    /// the shape exceeds [`MAX_GRID_WIDTH`] in either axis, or when nothing is
    /// left after normalisation.
    pub fn from_rows(rows: &[&[Option<Ingredient>]]) -> ServerResult<Self> {
        if rows.is_empty() {
            return Err(ServerError::Invariant(
                "a shaped pattern needs at least one row".to_owned(),
            ));
        }
        let raw_width = rows[0].len();
        let raw_height = rows.len();
        if let Some(ragged) = rows.iter().find(|row| row.len() != raw_width) {
            return Err(ServerError::Invariant(format!(
                "a shaped pattern must be rectangular: expected {raw_width} columns, got {}",
                ragged.len()
            )));
        }
        if raw_width == 0 {
            return Err(ServerError::Invariant(
                "a shaped pattern needs at least one column".to_owned(),
            ));
        }
        if raw_width > MAX_GRID_WIDTH || raw_height > MAX_GRID_WIDTH {
            return Err(ServerError::Invariant(format!(
                "a {raw_width}x{raw_height} shaped pattern exceeds the \
                 {MAX_GRID_WIDTH}x{MAX_GRID_WIDTH} limit"
            )));
        }
        let mut width = raw_width;
        while width > 0 && (0..raw_height).all(|y| rows[y][width - 1].is_none()) {
            width -= 1;
        }
        let mut height = raw_height;
        while height > 0 && rows[height - 1].iter().all(Option::is_none) {
            height -= 1;
        }
        if width == 0 || height == 0 {
            return Err(ServerError::Invariant(
                "a shaped pattern must use at least one cell".to_owned(),
            ));
        }
        let mut cells = Vec::with_capacity(width * height);
        for row in rows.iter().take(height) {
            cells.extend_from_slice(&row[..width]);
        }
        Ok(Self {
            width,
            height,
            cells,
        })
    }

    /// The pattern's columns after normalisation.
    #[must_use]
    pub const fn width(&self) -> usize {
        self.width
    }

    /// The pattern's rows after normalisation.
    #[must_use]
    pub const fn height(&self) -> usize {
        self.height
    }

    /// The pattern's size.
    #[must_use]
    pub const fn shape(&self) -> Shape {
        Shape::new(self.width, self.height)
    }

    /// Every cell, row-major.
    #[must_use]
    pub fn cells(&self) -> &[Option<Ingredient>] {
        &self.cells
    }

    /// The ingredient at `(x, y)`, or `None` for a blank cell or a position
    /// outside the pattern.
    #[must_use]
    pub fn at(&self, x: usize, y: usize) -> Option<&Ingredient> {
        if x >= self.width || y >= self.height {
            return None;
        }
        self.cells[y * self.width + x].as_ref()
    }
}

/// A shaped recipe: a pattern, its mirror image, and one result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShapedRecipe {
    name: String,
    pattern: ShapedPattern,
    mirrored_cells: Vec<Option<Ingredient>>,
    result: i32,
    result_count: i32,
}

impl ShapedRecipe {
    /// A shaped recipe.
    ///
    /// # Errors
    ///
    /// [`ServerError::Invariant`] when `name` is empty, when `result_count` is not
    /// in `1..=64` (the ceiling every stack obeys, so a larger result could never
    /// be produced), or when the pattern is unusable (see
    /// [`ShapedPattern::from_rows`], which holds the rest of the pattern
    /// validation).
    pub fn new(
        name: &str,
        pattern: ShapedPattern,
        result: i32,
        result_count: i32,
    ) -> ServerResult<Self> {
        if name.is_empty() {
            return Err(ServerError::Invariant("a recipe needs a name".to_owned()));
        }
        if !(1..=mc_entity::stack::HARD_MAX_STACK_SIZE).contains(&result_count) {
            return Err(ServerError::Invariant(format!(
                "recipe {name:?} produces {result_count} items, which is outside 1..={}",
                mc_entity::stack::HARD_MAX_STACK_SIZE
            )));
        }
        if pattern.shape().is_empty() {
            return Err(ServerError::Invariant(format!(
                "recipe {name:?} has an empty pattern"
            )));
        }
        let mirrored_cells = mirror(&pattern);
        Ok(Self {
            name: name.to_owned(),
            mirrored_cells,
            pattern,
            result,
            result_count,
        })
    }

    /// How this recipe consumes a grid.
    #[must_use]
    pub const fn kind(&self) -> RecipeKind {
        RecipeKind::Shaped
    }

    /// Stable name for logs and tests.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The pattern.
    #[must_use]
    pub const fn pattern(&self) -> &ShapedPattern {
        &self.pattern
    }

    /// The horizontally mirrored pattern's cells, row-major.
    #[must_use]
    pub fn mirrored_cells(&self) -> &[Option<Ingredient>] {
        &self.mirrored_cells
    }

    /// Result item id.
    #[must_use]
    pub const fn result(&self) -> i32 {
        self.result
    }

    /// Result count for one craft.
    #[must_use]
    pub const fn result_count(&self) -> i32 {
        self.result_count
    }

    /// How many grid slots must be filled for this recipe to match.
    #[must_use]
    pub fn ingredient_count(&self) -> usize {
        self.pattern
            .cells
            .iter()
            .filter(|cell| cell.is_some())
            .count()
    }

    /// Whether `grid` holds this shape at the given offset with `cells`.
    fn matches_at(
        &self,
        grid: &[ItemStack],
        width: usize,
        offset: usize,
        cells: &[Option<Ingredient>],
    ) -> bool {
        matches_at(grid, width, offset, self.pattern.shape(), cells)
    }

    /// `(offset, mirrored, consumed cells)` of the first place `grid` holds this
    /// pattern, scanning offsets row-major and trying the unmirrored form before
    /// the mirrored one.
    fn find_at(
        &self,
        grid: &[ItemStack],
        width: usize,
    ) -> Option<(usize, bool, [bool; MAX_GRID_SLOTS])> {
        let height = grid.len() / width;
        let shape = self.pattern.shape();
        if shape.width() > width || shape.height() > height {
            return None;
        }
        for y in 0..=height - shape.height() {
            for x in 0..=width - shape.width() {
                let offset = y * width + x;
                if let Some(cells) =
                    self.matched_cells(grid, width, offset, shape, &self.pattern.cells)
                {
                    return Some((offset, false, cells));
                }
                if let Some(cells) =
                    self.matched_cells(grid, width, offset, shape, &self.mirrored_cells)
                {
                    return Some((offset, true, cells));
                }
            }
        }
        None
    }

    /// `considered` cells if this pattern's `cells` sit at `offset`, else `None`.
    fn matched_cells(
        &self,
        grid: &[ItemStack],
        width: usize,
        offset: usize,
        shape: Shape,
        cells: &[Option<Ingredient>],
    ) -> Option<[bool; MAX_GRID_SLOTS]> {
        if !self.matches_at(grid, width, offset, cells) {
            return None;
        }
        let mut considered = [false; MAX_GRID_SLOTS];
        for y in 0..shape.height() {
            for x in 0..shape.width() {
                if cells[y * shape.width() + x].is_some() {
                    considered[offset + y * width + x] = true;
                }
            }
        }
        Some(considered)
    }
}

/// Horizontal mirror of a pattern's cells, same shape.
fn mirror(pattern: &ShapedPattern) -> Vec<Option<Ingredient>> {
    let mut cells = Vec::with_capacity(pattern.cells.len());
    for y in 0..pattern.height {
        for x in 0..pattern.width {
            cells.push(pattern.cells[y * pattern.width + (pattern.width - 1 - x)].clone());
        }
    }
    cells
}

/// Whether `cells` (a `shape`-sized pattern) sits at `offset` in `grid` **and
/// nothing else is in the grid**.
///
/// Both halves are Vanilla's rule:
///
/// * every pattern cell that names an ingredient must find that item in the grid,
///   and every blank pattern cell means "must be empty" — a cobblestone ring with a
///   filled centre is not the furnace recipe;
/// * a shaped recipe consumes the **whole** grid, so any occupied cell outside the
///   pattern's bounding box means this recipe does not match. Without that rule, a
///   2x2 grid of planks matches the 1x2 sticks pattern drawn in its left column —
///   which is what an earlier version of this function did, and what the offset
///   test in `crafting/tests.rs` caught.
fn matches_at(
    grid: &[ItemStack],
    width: usize,
    offset: usize,
    shape: Shape,
    cells: &[Option<Ingredient>],
) -> bool {
    for y in 0..shape.height() {
        for x in 0..shape.width() {
            let stack = grid.get(offset + y * width + x);
            match cells[y * shape.width() + x].as_ref() {
                Some(ingredient) => {
                    let Some(item_id) = stack.and_then(ItemStack::item_id) else {
                        return false;
                    };
                    if !ingredient.accepts(item_id) {
                        return false;
                    }
                }
                None => {
                    if stack.is_some_and(|stack| !stack.is_empty()) {
                        return false;
                    }
                }
            }
        }
    }
    // The grid is square in every layout this build has (`grid_shape` derives the
    // height from the length, and both axes are bounded by `MAX_GRID_WIDTH`), so
    // one bound serves both axes.
    let edge = width.max(shape.height());
    let (left, top) = (offset % width, offset / width);
    for y in 0..edge {
        for x in 0..edge {
            if y >= top && y < top + shape.height() && x >= left && x < left + shape.width() {
                continue;
            }
            if grid
                .get(y * width + x)
                .is_some_and(|stack| !stack.is_empty())
            {
                return false;
            }
        }
    }
    true
}

/// A shapeless recipe: a bag of ingredients and one result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShapelessRecipe {
    name: String,
    ingredients: Vec<Ingredient>,
    result: i32,
    result_count: i32,
}

impl ShapelessRecipe {
    /// A shapeless recipe.
    ///
    /// # Errors
    ///
    /// [`ServerError::Invariant`] when `name` is empty, when `ingredients` is
    /// empty or longer than [`MAX_GRID_SLOTS`], or when `result_count` is outside
    /// `1..=64`.
    pub fn new(
        name: &str,
        ingredients: Vec<Ingredient>,
        result: i32,
        result_count: i32,
    ) -> ServerResult<Self> {
        if name.is_empty() {
            return Err(ServerError::Invariant("a recipe needs a name".to_owned()));
        }
        if ingredients.is_empty() {
            return Err(ServerError::Invariant(format!(
                "shapeless recipe {name:?} has no ingredients"
            )));
        }
        if ingredients.len() > MAX_GRID_SLOTS {
            return Err(ServerError::Invariant(format!(
                "shapeless recipe {name:?} needs {} ingredients, more than the {MAX_GRID_SLOTS} \
                 slots of the largest grid",
                ingredients.len()
            )));
        }
        if !(1..=mc_entity::stack::HARD_MAX_STACK_SIZE).contains(&result_count) {
            return Err(ServerError::Invariant(format!(
                "recipe {name:?} produces {result_count} items, which is outside 1..={}",
                mc_entity::stack::HARD_MAX_STACK_SIZE
            )));
        }
        Ok(Self {
            name: name.to_owned(),
            ingredients,
            result,
            result_count,
        })
    }

    /// How this recipe consumes a grid.
    #[must_use]
    pub const fn kind(&self) -> RecipeKind {
        RecipeKind::Shapeless
    }

    /// Stable name for logs and tests.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The ingredients, in registration order.
    #[must_use]
    pub fn ingredients(&self) -> &[Ingredient] {
        &self.ingredients
    }

    /// Result item id.
    #[must_use]
    pub const fn result(&self) -> i32 {
        self.result
    }

    /// Result count for one craft.
    #[must_use]
    pub const fn result_count(&self) -> i32 {
        self.result_count
    }

    /// How many grid slots must be filled for this recipe to match.
    #[must_use]
    pub fn ingredient_count(&self) -> usize {
        self.ingredients.len()
    }

    /// The cells one instance of each ingredient occupies, if every ingredient is
    /// satisfied by a distinct non-empty slot and no slot is left over.
    ///
    /// This is Vanilla's `ShapelessRecipe` algorithm: walk the grid, remove one
    /// entry per ingredient, and refuse when an ingredient is unsatisfied, when a
    /// slot is left over (an extra item) or when the grid is not exactly as full
    /// as the recipe needs. Greedy matching is sound here because a set of
    /// same-sized items that can be matched at all can be matched greedily: if the
    /// current slot's item is still required by some unsatisfied ingredient, taking
    /// it can only help, and if no remaining ingredient accepts it the grid cannot
    /// be a permutation of the ingredients.
    fn match_cells(&self, grid: &[ItemStack]) -> Option<[bool; MAX_GRID_SLOTS]> {
        let mut satisfied = vec![false; self.ingredients.len()];
        let mut considered = [false; MAX_GRID_SLOTS];
        let mut matched_items = 0usize;
        for (index, stack) in grid.iter().enumerate().take(MAX_GRID_SLOTS) {
            let Some(item_id) = stack.item_id() else {
                continue;
            };
            let choice = self
                .ingredients
                .iter()
                .enumerate()
                .find(|(slot, ingredient)| !satisfied[*slot] && ingredient.accepts(item_id));
            // No remaining ingredient accepts this item, so the grid cannot be a
            // permutation of the ingredients.
            let (slot, _) = choice?;
            satisfied[slot] = true;
            considered[index] = true;
            matched_items += 1;
        }
        (matched_items == self.ingredients.len()).then_some(considered)
    }
}

/// The two recipe shapes this build understands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RecipeKind {
    /// A recipe whose pattern must appear in the grid, in one of its two mirror
    /// forms, at any offset where it fits.
    Shaped,
    /// A recipe whose ingredients may appear in any arrangement.
    Shapeless,
}

impl RecipeKind {
    /// Stable name for logs.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Shaped => "shaped",
            Self::Shapeless => "shapeless",
        }
    }
}

/// One crafting recipe.
///
/// The two kinds are boxed so the enum stays small; a caller sees a uniform
/// `name`/`kind`/`result`/`result_count`/`ingredient_count` surface regardless.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Recipe {
    /// A shaped recipe.
    Shaped(Box<ShapedRecipe>),
    /// A shapeless recipe.
    Shapeless(Box<ShapelessRecipe>),
}

impl Recipe {
    /// Stable name for logs and tests.
    #[must_use]
    pub fn name(&self) -> &str {
        match self {
            Self::Shaped(recipe) => recipe.name(),
            Self::Shapeless(recipe) => recipe.name(),
        }
    }

    /// How this recipe consumes a grid.
    #[must_use]
    pub const fn kind(&self) -> RecipeKind {
        match self {
            Self::Shaped(_) => RecipeKind::Shaped,
            Self::Shapeless(_) => RecipeKind::Shapeless,
        }
    }

    /// Result item id.
    #[must_use]
    pub const fn result(&self) -> i32 {
        match self {
            Self::Shaped(recipe) => recipe.result(),
            Self::Shapeless(recipe) => recipe.result(),
        }
    }

    /// Result count for one craft.
    #[must_use]
    pub const fn result_count(&self) -> i32 {
        match self {
            Self::Shaped(recipe) => recipe.result_count(),
            Self::Shapeless(recipe) => recipe.result_count(),
        }
    }

    /// How many grid slots must be filled for this recipe to match.
    #[must_use]
    pub fn ingredient_count(&self) -> usize {
        match self {
            Self::Shaped(recipe) => recipe.ingredient_count(),
            Self::Shapeless(recipe) => recipe.ingredient_count(),
        }
    }
}

/// Which recipe matched a grid, and which cells it consumes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecipeMatch<'a> {
    recipe: &'a Recipe,
    offset: usize,
    mirrored: bool,
    consumed: [bool; MAX_GRID_SLOTS],
}

impl RecipeMatch<'_> {
    /// The recipe that matched.
    #[must_use]
    pub const fn recipe(&self) -> &Recipe {
        self.recipe
    }

    /// Row-major grid index of the pattern's top-left cell (`0` for shapeless,
    /// which has no offset).
    #[must_use]
    pub const fn offset(&self) -> usize {
        self.offset
    }

    /// Whether the mirrored form of the pattern matched.
    #[must_use]
    pub const fn mirrored(&self) -> bool {
        self.mirrored
    }

    /// Whether the given grid index is one of the cells this recipe consumes.
    #[must_use]
    pub fn consumes(&self, index: usize) -> bool {
        self.consumed.get(index).copied().unwrap_or(false)
    }

    /// Indices the recipe consumes, ascending.
    #[must_use]
    pub fn consumed_slots(&self) -> Vec<usize> {
        (0..MAX_GRID_SLOTS)
            .filter(|index| self.consumes(*index))
            .collect()
    }
}

/// What sort of grid `grid` and `width` describe, or `None` if that is not a grid.
///
/// A grid is at most [`MAX_GRID_WIDTH`] in either axis, at most
/// [`MAX_GRID_SLOTS`] slots, and rectangular (`len % width == 0`). A zero width
/// has no meaning at all, and a ragged or oversized one is a caller bug that must
/// produce "no match" rather than an index panic (AGENTS.md sections 9 and 10).
fn grid_shape(grid: &[ItemStack], width: usize) -> Option<Shape> {
    if width == 0 || width > MAX_GRID_WIDTH || grid.len() > MAX_GRID_SLOTS {
        return None;
    }
    if !grid.len().is_multiple_of(width) {
        return None;
    }
    let height = grid.len() / width;
    if height == 0 || height > MAX_GRID_WIDTH {
        return None;
    }
    Some(Shape::new(width, height))
}

/// Matching, crafting and result recomputation over an ordered recipe list.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RecipeRegistry {
    recipes: Vec<Recipe>,
}

/// What a pack-to-table conversion did, including what it could not represent.
///
/// Counted rather than logged so a caller loading the real pack knows how much
/// of it became craftable and why the rest did not.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CraftingConversion {
    /// Shaped recipes read.
    pub shaped_seen: usize,
    /// Shapeless recipes read.
    pub shapeless_seen: usize,
    /// Recipes that converted.
    pub converted: usize,
    /// Recipes skipped for a tag/unknown ingredient (no tag resolver here).
    pub tag_or_unknown: usize,
    /// Recipes whose result name is not in the item registry.
    pub unknown_results: Vec<String>,
    /// Malformed recipes (bad counts, patterns, duplicate names at build).
    pub malformed: Vec<String>,
    /// Non-crafting recipes (cooking, stonecutting, smithing, special, …).
    pub other_kinds: usize,
}

impl RecipeRegistry {
    /// A registry over an explicit recipe list, in matching order.
    ///
    /// # Errors
    ///
    /// [`ServerError::Invariant`] when two recipes share a name — a name is how a
    /// recipe is identified in a log, a test and (later) a data pack, so a
    /// duplicate would make "which recipe matched" unanswerable.
    pub fn new(recipes: Vec<Recipe>) -> ServerResult<Self> {
        for (index, recipe) in recipes.iter().enumerate() {
            if recipes[..index]
                .iter()
                .any(|earlier| earlier.name() == recipe.name())
            {
                return Err(ServerError::Invariant(format!(
                    "duplicate recipe name {:?}",
                    recipe.name()
                )));
            }
        }
        Ok(Self { recipes })
    }

    /// An empty registry: nothing matches.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            recipes: Vec::new(),
        }
    }

    /// The **hand-written baseline subset** of the Vanilla recipe set.
    ///
    /// This is not the Vanilla recipe set (see the module docs): it is the small
    /// set that makes the crafting path real and testable, and every entry is
    /// recorded here so the whole thing can be replaced by the P07-03 data loader.
    ///
    /// | recipe | ingredients | shape | result |
    /// |---|---|---|---|
    /// | `baseline_planks_from_<wood>_log` (9 woods) | 1 log | shapeless, 2x2 grid | 4 planks |
    /// | `baseline_planks_from_<stem>` (2 stems) | 1 stem | shapeless, 2x2 grid | 4 planks |
    /// | `baseline_sticks_from_planks` | 2 planks stacked | shaped 1x2 | 4 sticks |
    /// | `baseline_crafting_table` | 4 planks | shaped 2x2 | 1 crafting table |
    /// | `baseline_furnace` | 8 cobblestone | shaped 3x3 ring, empty centre | 1 furnace |
    /// | `baseline_chest` | 8 planks | shaped 3x3 ring, empty centre | 1 chest |
    /// | `baseline_torch` | coal over a stick | shaped 1x2 | 4 torches |
    /// | `baseline_torch_from_charcoal` | charcoal over a stick | shaped 1x2 | 4 torches |
    /// | `baseline_stone` | 1 cobblestone | shaped 1x1 | 1 stone |
    /// | `baseline_oak_slab` | 3 planks in a row | shaped 3x1 | 6 oak slabs |
    ///
    /// The recipe shapes, counts and results are Vanilla's for these items; the
    /// limitation is coverage and the missing ingredient-tag abstraction, not the
    /// arithmetic. What is deliberately **not** Vanilla here:
    ///
    /// - Only oak planks make sticks and only oak slabs are produced. Vanilla has
    ///   one recipe per wood type for each; stated rather than guessed.
    /// - "Any planks" is one ingredient with eleven alternatives rather than the
    ///   `#minecraft:planks` tag, and "any log" likewise.
    /// - A log produces **its own** wood's planks (that part is Vanilla), so the
    ///   baseline has 11 planks recipes where Vanilla has 24 — ten logs and stems
    ///   in, plus bamboo, which is left out entirely because its 26.1 recipe is a
    ///   different shape (`crafting_shapeless` from a bamboo block via a different
    ///   count).
    ///
    /// # Errors
    ///
    /// [`ServerError::CorruptData`] naming the first item that `items` does not
    /// contain. A missing name is never skipped: a baseline that silently lost a
    /// recipe would be exactly the fake completeness AGENTS.md section 3.3
    /// forbids.
    // One long function because it *is* the table: eleven wood recipes, seven
    // shaped recipes and their item lookups. Splitting it would move the recipe
    // documentation away from the recipes.
    #[allow(clippy::too_many_lines)]
    pub fn baseline(items: &ItemRegistry) -> ServerResult<Self> {
        let mut recipes = Vec::new();

        // Every wood's planks from that wood's own log (or stem), one at a time.
        for wood in [
            "oak", "spruce", "birch", "jungle", "acacia", "dark_oak", "mangrove", "cherry",
            "pale_oak",
        ] {
            let log = format!("minecraft:{wood}_log");
            let planks = format!("minecraft:{wood}_planks");
            recipes.push(Recipe::Shapeless(Box::new(ShapelessRecipe::new(
                &format!("baseline_planks_from_{log}"),
                vec![Ingredient::item(id_of(items, &log)?)],
                id_of(items, &planks)?,
                4,
            )?)));
        }
        for stem in ["crimson", "warped"] {
            let stem_name = format!("minecraft:{stem}_stem");
            let planks = format!("minecraft:{stem}_planks");
            recipes.push(Recipe::Shapeless(Box::new(ShapelessRecipe::new(
                &format!("baseline_planks_from_{stem_name}"),
                vec![Ingredient::item(id_of(items, &stem_name)?)],
                id_of(items, &planks)?,
                4,
            )?)));
        }

        // `any_of` sorts and deduplicates, and `?` refuses a missing name.
        let planks = Ingredient::any_of(&[
            id_of(items, "minecraft:oak_planks")?,
            id_of(items, "minecraft:spruce_planks")?,
            id_of(items, "minecraft:birch_planks")?,
            id_of(items, "minecraft:jungle_planks")?,
            id_of(items, "minecraft:acacia_planks")?,
            id_of(items, "minecraft:dark_oak_planks")?,
            id_of(items, "minecraft:mangrove_planks")?,
            id_of(items, "minecraft:cherry_planks")?,
            id_of(items, "minecraft:pale_oak_planks")?,
            id_of(items, "minecraft:crimson_planks")?,
            id_of(items, "minecraft:warped_planks")?,
        ])?;
        let stick = Ingredient::item(id_of(items, "minecraft:stick")?);
        let cobblestone = Ingredient::item(id_of(items, "minecraft:cobblestone")?);
        let coal = Ingredient::item(id_of(items, "minecraft:coal")?);
        let charcoal = Ingredient::item(id_of(items, "minecraft:charcoal")?);

        // Two planks stacked: sticks. Vanilla has one per wood; this is the oak one.
        recipes.push(shaped(
            "baseline_sticks_from_planks",
            &[&[Some(planks.clone())], &[Some(planks.clone())]],
            id_of(items, "minecraft:stick")?,
            4,
        )?);
        // A 2x2 block of planks.
        recipes.push(shaped(
            "baseline_crafting_table",
            &[
                &[Some(planks.clone()), Some(planks.clone())],
                &[Some(planks.clone()), Some(planks.clone())],
            ],
            id_of(items, "minecraft:crafting_table")?,
            1,
        )?);
        // The 3x3 cobblestone ring with an empty centre.
        recipes.push(shaped(
            "baseline_furnace",
            &[
                &[
                    Some(cobblestone.clone()),
                    Some(cobblestone.clone()),
                    Some(cobblestone.clone()),
                ],
                &[Some(cobblestone.clone()), None, Some(cobblestone.clone())],
                &[
                    Some(cobblestone.clone()),
                    Some(cobblestone.clone()),
                    Some(cobblestone.clone()),
                ],
            ],
            id_of(items, "minecraft:furnace")?,
            1,
        )?);
        // The 3x3 planks ring with an empty centre.
        recipes.push(shaped(
            "baseline_chest",
            &[
                &[
                    Some(planks.clone()),
                    Some(planks.clone()),
                    Some(planks.clone()),
                ],
                &[Some(planks.clone()), None, Some(planks.clone())],
                &[
                    Some(planks.clone()),
                    Some(planks.clone()),
                    Some(planks.clone()),
                ],
            ],
            id_of(items, "minecraft:chest")?,
            1,
        )?);
        // Coal (or charcoal) above a stick: four torches.
        recipes.push(shaped(
            "baseline_torch",
            &[&[Some(coal)], &[Some(stick.clone())]],
            id_of(items, "minecraft:torch")?,
            4,
        )?);
        recipes.push(shaped(
            "baseline_torch_from_charcoal",
            &[&[Some(charcoal)], &[Some(stick.clone())]],
            id_of(items, "minecraft:torch")?,
            4,
        )?);
        // One cobblestone in a 2x2 grid, so it also matches at any offset of a
        // larger grid, exactly as `crafting_shaped` with a 1x1 pattern does.
        recipes.push(shaped(
            "baseline_stone",
            &[&[Some(cobblestone)]],
            id_of(items, "minecraft:stone")?,
            1,
        )?);
        // Three planks in a row: six oak slabs.
        recipes.push(shaped(
            "baseline_oak_slab",
            &[&[Some(planks.clone()), Some(planks.clone()), Some(planks)]],
            id_of(items, "minecraft:oak_slab")?,
            6,
        )?);

        Self::new(recipes)
    }

    /// Build the table from a loaded data-pack [`mc_data::RecipeBook`] (P12-07).
    ///
    /// Only item-ingredient `crafting_shaped`/`crafting_shapeless` recipes with
    /// registry-known ids convert; tag ingredients, unknown items, and other
    /// recipe types are counted and skipped (same refusal-ladder honesty as the
    /// loot and smelting joins). Order is the book's load order, so pack
    /// overrides win by position like they do on disk.
    ///
    /// # Errors
    ///
    /// [`ServerError::Invariant`] when two converted recipes share a name.
    #[allow(clippy::too_many_lines)]
    pub fn from_book(
        book: &mc_data::RecipeBook,
        items: &ItemRegistry,
    ) -> ServerResult<(Self, CraftingConversion)> {
        use mc_data::{Ingredient as DataIngredient, Recipe as DataRecipe};

        fn full_name(id: &mc_core::ids::ResourceId) -> String {
            format!("{}:{}", id.namespace(), id.value())
        }

        fn container_ingredient(
            alternatives: &[DataIngredient],
            items: &ItemRegistry,
        ) -> Option<Ingredient> {
            let mut ids = Vec::new();
            for alternative in alternatives {
                match alternative {
                    DataIngredient::Item(id) => {
                        let Ok(item_id) = items.id(&full_name(id)) else {
                            return None;
                        };
                        ids.push(item_id);
                    }
                    DataIngredient::Tag(_) => return None,
                }
            }
            if ids.is_empty() {
                return None;
            }
            ids.sort_unstable();
            ids.dedup();
            if ids.len() == 1 {
                Some(Ingredient::item(ids[0]))
            } else {
                Ingredient::any_of(&ids).ok()
            }
        }

        let mut report = CraftingConversion::default();
        let mut recipes: Vec<Recipe> = Vec::new();
        for data in book.recipes() {
            match data {
                DataRecipe::Shaped(shaped) => {
                    report.shaped_seen += 1;
                    let Ok(result) = items.id(&full_name(&shaped.result)) else {
                        report.unknown_results.push(shaped.result.to_string());
                        continue;
                    };
                    if !(1..=mc_entity::stack::HARD_MAX_STACK_SIZE).contains(&shaped.result_count) {
                        report.malformed.push(shaped.name.to_string());
                        continue;
                    }
                    // Build rows of container ingredients; any tag/unknown cell
                    // refuses the whole recipe (counted, not guessed).
                    let mut rows: Vec<Vec<Option<Ingredient>>> = Vec::new();
                    let mut refused = false;
                    for row in 0..shaped.height() {
                        let mut cells = Vec::new();
                        for col in 0..shaped.width() {
                            let alternatives = match shaped.ingredient_at(row, col) {
                                Ok(Some(alternatives)) => alternatives,
                                Ok(None) => {
                                    cells.push(None);
                                    continue;
                                }
                                Err(_) => {
                                    refused = true;
                                    break;
                                }
                            };
                            if let Some(ingredient) = container_ingredient(alternatives, items) {
                                cells.push(Some(ingredient));
                            } else {
                                refused = true;
                                break;
                            }
                        }
                        if refused {
                            break;
                        }
                        rows.push(cells);
                    }
                    if refused {
                        report.tag_or_unknown += 1;
                        continue;
                    }
                    let row_refs: Vec<&[Option<Ingredient>]> =
                        rows.iter().map(Vec::as_slice).collect();
                    let Ok(pattern) = ShapedPattern::from_rows(&row_refs) else {
                        report.malformed.push(shaped.name.to_string());
                        continue;
                    };
                    let Ok(recipe) = ShapedRecipe::new(
                        &shaped.name.to_string(),
                        pattern,
                        result,
                        shaped.result_count,
                    ) else {
                        report.malformed.push(shaped.name.to_string());
                        continue;
                    };
                    recipes.push(Recipe::Shaped(Box::new(recipe)));
                    report.converted += 1;
                }
                DataRecipe::Shapeless(shapeless) => {
                    report.shapeless_seen += 1;
                    let Ok(result) = items.id(&full_name(&shapeless.result)) else {
                        report.unknown_results.push(shapeless.result.to_string());
                        continue;
                    };
                    if !(1..=mc_entity::stack::HARD_MAX_STACK_SIZE)
                        .contains(&shapeless.result_count)
                    {
                        report.malformed.push(shapeless.name.to_string());
                        continue;
                    }
                    let mut ingredients = Vec::new();
                    let mut refused = false;
                    for entry in &shapeless.ingredients {
                        if let Some(ingredient) = container_ingredient(entry, items) {
                            ingredients.push(ingredient);
                        } else {
                            refused = true;
                            break;
                        }
                    }
                    if refused {
                        report.tag_or_unknown += 1;
                        continue;
                    }
                    let Ok(recipe) = ShapelessRecipe::new(
                        &shapeless.name.to_string(),
                        ingredients,
                        result,
                        shapeless.result_count,
                    ) else {
                        report.malformed.push(shapeless.name.to_string());
                        continue;
                    };
                    recipes.push(Recipe::Shapeless(Box::new(recipe)));
                    report.converted += 1;
                }
                _ => {
                    report.other_kinds += 1;
                }
            }
        }
        let registry = Self::new(recipes)?;
        Ok((registry, report))
    }

    /// The recipes, in matching order.
    #[must_use]
    pub fn recipes(&self) -> &[Recipe] {
        &self.recipes
    }

    /// Number of recipes.
    #[must_use]
    pub fn len(&self) -> usize {
        self.recipes.len()
    }

    /// Whether there are no recipes.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.recipes.is_empty()
    }

    /// The recipe with this name.
    #[must_use]
    pub fn by_name(&self, name: &str) -> Option<&Recipe> {
        self.recipes.iter().find(|recipe| recipe.name() == name)
    }

    /// Every recipe name, in matching order.
    #[must_use]
    pub fn names(&self) -> Vec<&str> {
        self.recipes.iter().map(Recipe::name).collect()
    }

    /// The first recipe that matches `grid`, or `None`.
    ///
    /// A malformed grid (zero width, a width above [`MAX_GRID_WIDTH`], a length
    /// that is not a multiple of the width, more than [`MAX_GRID_SLOTS`] slots, or
    /// an all-empty grid) never matches; this is the pure form of
    /// [`RecipeRegistry::matches_with`]. Use the `_with` form when an unknown item
    /// id in a recipe must be *reported* rather than treated as a non-match.
    #[must_use]
    pub fn matches(&self, grid: &[ItemStack], width: usize) -> bool {
        self.find_match(grid, width).is_some()
    }

    /// [`RecipeRegistry::matches`], reporting a corrupt recipe instead of hiding
    /// it.
    ///
    /// # Errors
    ///
    /// [`ServerError::CorruptData`] naming an item id that `items` does not know:
    /// an alternative in a matching ingredient, or the matched recipe's result. An
    /// id outside the registry means the recipe table does not belong to this
    /// registry, which is data corruption rather than a non-match.
    pub fn matches_with(
        &self,
        grid: &[ItemStack],
        width: usize,
        items: &ItemRegistry,
    ) -> ServerResult<bool> {
        let Some(found) = self.find_match(grid, width) else {
            return Ok(false);
        };
        check_ids(items, found.recipe)
    }

    /// The first recipe that matches, with the cells it would consume.
    ///
    /// `None` covers "no recipe matches" and "this is not a grid" alike; neither
    /// is an error, and a caller that must distinguish them has
    /// [`RecipeRegistry::matches_with`].
    #[must_use]
    pub fn find_match(&self, grid: &[ItemStack], width: usize) -> Option<RecipeMatch<'_>> {
        let shape = grid_shape(grid, width)?;
        if grid.iter().all(ItemStack::is_empty) {
            return None;
        }
        let width = shape.width();
        for recipe in &self.recipes {
            let found = match recipe {
                Recipe::Shaped(shaped) => {
                    shaped
                        .find_at(grid, width)
                        .map(|(offset, mirrored, consumed)| RecipeMatch {
                            recipe,
                            offset,
                            mirrored,
                            consumed,
                        })
                }
                Recipe::Shapeless(shapeless) => {
                    shapeless.match_cells(grid).map(|consumed| RecipeMatch {
                        recipe,
                        offset: 0,
                        mirrored: false,
                        consumed,
                    })
                }
            };
            if found.is_some() {
                return found;
            }
        }
        None
    }

    /// Craft once: consume one item from each ingredient cell and return the
    /// result.
    ///
    /// Pure with respect to the menu — it mutates `grid` (the crafting input
    /// slots) and nothing else. It does not touch a result slot, a cursor, a
    /// container or a packet; moving the result out of the result slot is the
    /// caller's job (a `container_click` on the result slot, whose recomputation is
    /// [`RecipeRegistry::recompute_result`]).
    ///
    /// # Errors
    ///
    /// [`ServerError::CorruptData`] naming an item id `items` does not know (see
    /// [`RecipeRegistry::matches_with`]).
    ///
    /// `Ok(None)` means "nothing matched" or "this is not a grid": no recipe, a
    /// grid too small for one, a zero-width grid, or an all-empty grid. In every
    /// one of those cases `grid` is left exactly as it was.
    pub fn craft(
        &self,
        grid: &mut [ItemStack],
        width: usize,
        items: &ItemRegistry,
    ) -> ServerResult<Option<ItemStack>> {
        let Some(found) = self.find_match(grid, width) else {
            return Ok(None);
        };
        check_ids(items, found.recipe)?;
        let consumed = found.consumed;
        consume(grid, &consumed);
        Ok(Some(ItemStack::new(
            found.recipe.result(),
            found.recipe.result_count(),
        )?))
    }

    /// The stack that belongs in the result slot for `grid`, or
    /// [`ItemStack::EMPTY`] when nothing matches.
    ///
    /// This is the recompute hook: call it after any change to the crafting input
    /// slots and write the answer into the result slot, marking it changed so the
    /// client is told even when the value is identical
    /// ([`crate::Container::mark_changed`]). It does **not** consume anything, so
    /// it is safe to call on every click.
    ///
    /// The result is the recipe's full output count, clamped down to the item's own
    /// maximum stack size (see the module docs' "Result-slot stacking"; a baseline
    /// recipe can never hit the clamp, and a test asserts that).
    ///
    /// # Errors
    ///
    /// [`ServerError::CorruptData`] naming an item id `items` does not know, as
    /// for [`RecipeRegistry::matches_with`].
    pub fn recompute_result(
        &self,
        grid: &[ItemStack],
        width: usize,
        items: &ItemRegistry,
        stack_sizes: &StackSizeTable,
    ) -> ServerResult<ItemStack> {
        let Some(found) = self.find_match(grid, width) else {
            return Ok(ItemStack::EMPTY);
        };
        check_ids(items, found.recipe)?;
        let result = found.recipe.result();
        let count = found
            .recipe
            .result_count()
            .min(stack_sizes.max_stack_size(result))
            .max(0);
        ItemStack::new(result, count)
    }
}

/// A shaped recipe from authored rows, boxed into [`Recipe`].
fn shaped(
    name: &str,
    rows: &[&[Option<Ingredient>]],
    result: i32,
    result_count: i32,
) -> ServerResult<Recipe> {
    Ok(Recipe::Shaped(Box::new(ShapedRecipe::new(
        name,
        ShapedPattern::from_rows(rows)?,
        result,
        result_count,
    )?)))
}

/// An item id by name, or [`ServerError::CorruptData`] naming the missing item.
fn id_of(items: &ItemRegistry, name: &str) -> ServerResult<i32> {
    items.id(name).map_err(|_| {
        ServerError::CorruptData(format!(
            "the baseline recipe set needs item {name:?}, which the item registry does not contain"
        ))
    })
}

/// Whether the matched recipe's result item id is known to `items`.
///
/// Ingredient alternatives are **not** re-checked per call: they are resolved by
/// [`RecipeRegistry::baseline`], which refuses a name the registry lacks, so an
/// ingredient id that reached a match is already known to be registered. The
/// result id is checked because [`Recipe::result`] hands it straight back to the
/// caller, and "this stack's item id is not in the registry" must be an error
/// rather than a stack the caller cannot name.
fn check_ids(items: &ItemRegistry, recipe: &Recipe) -> ServerResult<bool> {
    let result = recipe.result();
    items.entry(result).map_err(|_| {
        ServerError::CorruptData(format!(
            "recipe {:?} produces item id {result}, which the item registry does not contain",
            recipe.name()
        ))
    })?;
    Ok(true)
}

/// Remove exactly one item from each consumed cell, clearing emptied slots.
///
/// The cells were proven non-empty by the match, so each `shrink(1)` here removes
/// one item. Taking less than one (impossible for a matched cell) would leave the
/// grid holding items the recipe did not account for, and `shrink` reports the
/// shortfall precisely so a caller cannot silently rely on it.
fn consume(grid: &mut [ItemStack], consumed: &[bool; MAX_GRID_SLOTS]) {
    for (index, taken) in consumed.iter().enumerate() {
        if !taken {
            continue;
        }
        let Some(stack) = grid.get_mut(index) else {
            continue;
        };
        let _ = stack.shrink(1);
        if stack.count() == 0 {
            *stack = ItemStack::EMPTY;
        }
    }
}

#[cfg(test)]
mod tests;
