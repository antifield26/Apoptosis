//! Recipes loaded from the pack format (P07-09).
//!
//! ## The formats, surveyed from the 26.1.2 jar
//!
//! `data/minecraft/recipe/` holds **1 516 files** across **21 recipe types**. This
//! module models the seven that are actual crafting/cooking recipes and that the
//! container layer can execute:
//!
//! | Type | Count | Modelled |
//! |---|---|---|
//! | `crafting_shaped` | 708 | yes |
//! | `crafting_shapeless` | 322 | yes |
//! | `stonecutting` | 275 | yes |
//! | `smelting` | 73 | yes |
//! | `blasting` | 25 | yes |
//! | `campfire_cooking` | 9 | yes |
//! | `smoking` | 9 | yes |
//! | `crafting_transmute` | 33 | **no** — new in 26.1, semantics unverified |
//! | `smithing_trim` / `smithing_transform` | 30 | **no** — needs a smithing menu |
//! | `crafting_dye`, `crafting_imbue`, `crafting_decorated_pot` | 8 | **no** — new in 26.1 |
//! | `crafting_special_*` | 25 | **no** — hard-coded behaviours, not data |
//!
//! The unmodelled types are **skipped and counted**, never silently dropped: a
//! [`RecipeLoadReport`] names how many of each were ignored and why, because a pack
//! whose recipe did nothing is otherwise indistinguishable from a pack that failed to
//! load.
//!
//! ## Why the cooking types share one struct
//!
//! `smelting`, `blasting`, `smoking` and `campfire_cooking` have identical JSON —
//! `ingredient`, `result`, `cookingtime`, `experience` — and differ only in which
//! block performs them and in their default timings. One [`CookingRecipe`] with a
//! [`SmeltingKind`] discriminator means one parser and one set of tests rather than
//! four copies (AGENTS.md §3.4).
//!
//! ## Real values this replaced guessed ones with
//!
//! The Phase 06 review recorded that the hand-written furnace table was
//! "community knowledge, not a jar dump". The jar confirms the two values that were
//! guessed: iron from raw iron is `cookingtime: 200, experience: 0.7`. Loading it
//! means the numbers now come from the data, and the guessed table can go.

use mc_core::ids::ResourceId;
use std::collections::BTreeMap;
use std::path::Path;

use crate::json::{JsonError, Limits, optional_f64, optional_i64, optional_str, read_json_object};

/// An ingredient: either an id or a tag reference.
///
/// The jar writes this two ways — a bare string (`"ingredient": "minecraft:raw_iron"`)
/// or an array of alternatives — and a `#`-prefixed string is a tag.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ingredient {
    /// A single item, or one alternative of several.
    Item(ResourceId),
    /// A tag reference: any item in the tag satisfies the slot.
    Tag(ResourceId),
}

impl Ingredient {
    /// Whether a concrete item satisfies this ingredient.
    ///
    /// Tag membership needs a [`crate::TagSet`], which the caller supplies as a
    /// predicate so this type stays independent of the loader.
    #[must_use]
    pub fn accepts(
        &self,
        item: &ResourceId,
        in_tag: &dyn Fn(&ResourceId, &ResourceId) -> bool,
    ) -> bool {
        match self {
            Self::Item(id) => id == item,
            Self::Tag(tag) => in_tag(tag, item),
        }
    }
}

/// A shaped crafting recipe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShapedRecipe {
    /// The recipe's name, as `namespace:path`.
    pub name: ResourceId,
    /// The pattern rows, top to bottom. A space is an empty slot.
    pub pattern: Vec<String>,
    /// Ingredient per pattern character. A space is never a key.
    pub key: BTreeMap<char, Vec<Ingredient>>,
    /// What it produces.
    pub result: ResourceId,
    /// How many.
    pub result_count: i32,
    /// The `group` string, which only affects recipe-book grouping.
    pub group: Option<String>,
}

impl ShapedRecipe {
    /// Pattern width, in slots.
    #[must_use]
    pub fn width(&self) -> usize {
        self.pattern.first().map_or(0, String::len)
    }

    /// Pattern height, in rows.
    #[must_use]
    pub fn height(&self) -> usize {
        self.pattern.len()
    }

    /// The ingredient for a pattern cell, or `None` for a blank.
    ///
    /// # Errors
    ///
    /// [`JsonError::Invalid`] when the pattern names a character absent from `key`,
    /// which is a malformed recipe rather than a blank.
    pub fn ingredient_at(
        &self,
        row: usize,
        column: usize,
    ) -> Result<Option<&[Ingredient]>, JsonError> {
        let Some(line) = self.pattern.get(row) else {
            return Ok(None);
        };
        let Some(character) = line.chars().nth(column) else {
            return Ok(None);
        };
        if character == ' ' {
            return Ok(None);
        }
        self.key
            .get(&character)
            .map(|alternatives| Some(alternatives.as_slice()))
            .ok_or_else(|| JsonError::Invalid {
                path: Path::new(&self.name.to_string()).to_path_buf(),
                reason: format!("pattern uses {character:?}, which the key does not define"),
            })
    }
}

/// A shapeless crafting recipe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShapelessRecipe {
    /// The recipe's name.
    pub name: ResourceId,
    /// One entry per required item; each entry is a set of alternatives.
    pub ingredients: Vec<Vec<Ingredient>>,
    /// What it produces.
    pub result: ResourceId,
    /// How many.
    pub result_count: i32,
    /// The `group` string.
    pub group: Option<String>,
}

/// Which cooking block a [`CookingRecipe`] belongs to.
///
/// The four types share one JSON shape, so they share one struct and differ by this.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SmeltingKind {
    /// A furnace.
    Smelting,
    /// A blast furnace.
    Blasting,
    /// A smoker.
    Smoking,
    /// A campfire.
    CampfireCooking,
}

impl SmeltingKind {
    /// The recipe type string in the pack format.
    #[must_use]
    pub const fn type_name(self) -> &'static str {
        match self {
            Self::Smelting => "minecraft:smelting",
            Self::Blasting => "minecraft:blasting",
            Self::Smoking => "minecraft:smoking",
            Self::CampfireCooking => "minecraft:campfire_cooking",
        }
    }

    /// Recognise a type string.
    #[must_use]
    pub fn from_type_name(text: &str) -> Option<Self> {
        match text {
            "minecraft:smelting" => Some(Self::Smelting),
            "minecraft:blasting" => Some(Self::Blasting),
            "minecraft:smoking" => Some(Self::Smoking),
            "minecraft:campfire_cooking" => Some(Self::CampfireCooking),
            _ => None,
        }
    }

    /// The default cooking time when a recipe omits `cookingtime`.
    ///
    /// **Measured from the jar's recipe data**, by counting every cooking recipe's own `cookingtime`:
    ///
    /// ```text
    /// minecraft:smelting:          cookingtime=200  x73
    /// minecraft:blasting:          cookingtime=100  x25
    /// minecraft:smoking:           cookingtime=100  x9
    /// minecraft:campfire_cooking:  cookingtime=600  x9
    /// ```
    ///
    /// **Campfire cooking is the slow one at 600 ticks**, not a fast one: a campfire cooks four items at once and
    /// takes thirty seconds over them. Grouping it with blasting and smoking at 100 was wrong, and the doc was
    /// once rewritten to agree with that wrong constant — see KD-73. `container/furnace.rs` says 600 and always
    /// has.
    ///
    /// These defaults matter only for a pack that omits `cookingtime`, which vanilla never does — so they are
    /// recorded as *not exercised by vanilla* rather than presented as verified.
    #[must_use]
    pub const fn default_cooking_time(self) -> i32 {
        match self {
            Self::Smelting => 200,
            // Blasting and smoking are the fast pair at half smelting's time.
            Self::Blasting | Self::Smoking => 100,
            // **And a campfire is the slow one.** It cooks four items at once and takes 600 ticks over them;
            // grouping it with the fast pair was wrong, and the doc beside it was once rewritten to agree.
            Self::CampfireCooking => 600,
        }
    }
}

/// A cooking recipe: smelting, blasting, smoking or campfire cooking.
#[derive(Debug, Clone, PartialEq)]
pub struct CookingRecipe {
    /// The recipe's name.
    pub name: ResourceId,
    /// Which cooking block performs it.
    pub kind: SmeltingKind,
    /// What goes in. Always present for the four cooking types.
    pub ingredient: Vec<Ingredient>,
    /// What comes out.
    pub result: ResourceId,
    /// How many.
    pub result_count: i32,
    /// Ticks to cook. Vanilla always states it; the default is a fallback.
    pub cooking_time: i32,
    /// Experience granted. **Absent means zero**, which is what the format says.
    pub experience: f64,
}

/// A stonecutting recipe: one ingredient in, `count` of a result out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StonecuttingRecipe {
    /// The recipe's name.
    pub name: ResourceId,
    /// The input, which may be a tag.
    pub ingredient: Vec<Ingredient>,
    /// What comes out.
    pub result: ResourceId,
    /// How many.
    pub result_count: i32,
}

/// Any recipe this build understands.
#[derive(Debug, Clone, PartialEq)]
pub enum Recipe {
    /// A shaped crafting recipe.
    Shaped(ShapedRecipe),
    /// A shapeless crafting recipe.
    Shapeless(ShapelessRecipe),
    /// A cooking recipe.
    Cooking(CookingRecipe),
    /// A stonecutting recipe.
    Stonecutting(StonecuttingRecipe),
}

impl Recipe {
    /// The recipe's name.
    #[must_use]
    pub fn name(&self) -> &ResourceId {
        match self {
            Self::Shaped(recipe) => &recipe.name,
            Self::Shapeless(recipe) => &recipe.name,
            Self::Cooking(recipe) => &recipe.name,
            Self::Stonecutting(recipe) => &recipe.name,
        }
    }

    /// What it produces.
    #[must_use]
    pub fn result(&self) -> &ResourceId {
        match self {
            Self::Shaped(recipe) => &recipe.result,
            Self::Shapeless(recipe) => &recipe.result,
            Self::Cooking(recipe) => &recipe.result,
            Self::Stonecutting(recipe) => &recipe.result,
        }
    }

    /// How many it produces.
    #[must_use]
    pub fn result_count(&self) -> i32 {
        match self {
            Self::Shaped(recipe) => recipe.result_count,
            Self::Shapeless(recipe) => recipe.result_count,
            Self::Cooking(recipe) => recipe.result_count,
            Self::Stonecutting(recipe) => recipe.result_count,
        }
    }

    /// Which kind this is.
    #[must_use]
    pub const fn kind(&self) -> RecipeKind {
        match self {
            Self::Shaped(_) => RecipeKind::Shaped,
            Self::Shapeless(_) => RecipeKind::Shapeless,
            Self::Cooking(_) => RecipeKind::Cooking,
            Self::Stonecutting(_) => RecipeKind::Stonecutting,
        }
    }
}

/// The broad category of a [`Recipe`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RecipeKind {
    /// Shaped crafting.
    Shaped,
    /// Shapeless crafting.
    Shapeless,
    /// Cooking in a furnace-like block.
    Cooking,
    /// Stonecutting.
    Stonecutting,
}

impl RecipeKind {
    /// Stable name for logs and reports.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Shaped => "shaped",
            Self::Shapeless => "shapeless",
            Self::Cooking => "cooking",
            Self::Stonecutting => "stonecutting",
        }
    }
}

/// What a recipe load did.
///
/// The counts are the point: a pack whose 40 `crafting_transmute` recipes silently
/// did nothing would otherwise look identical to one that loaded cleanly.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RecipeLoadReport {
    /// Files merged.
    pub files: usize,
    /// Recipes loaded, by kind.
    pub loaded: BTreeMap<RecipeKind, usize>,
    /// Types recognised but not modelled, with how many files used them.
    pub unmodelled: BTreeMap<String, usize>,
    /// Files skipped because they could not be read or parsed.
    pub skipped: Vec<String>,
}

impl RecipeLoadReport {
    /// Total recipes loaded.
    #[must_use]
    pub fn total_loaded(&self) -> usize {
        self.loaded.values().sum()
    }

    /// Total files ignored because their type is not modelled.
    #[must_use]
    pub fn total_unmodelled(&self) -> usize {
        self.unmodelled.values().sum()
    }

    /// Whether nothing was skipped.
    #[must_use]
    pub fn is_clean(&self) -> bool {
        self.skipped.is_empty()
    }
}

/// Every loaded recipe, indexed for lookup.
#[derive(Debug, Clone, Default)]
pub struct RecipeBook {
    recipes: Vec<Recipe>,
    by_name: BTreeMap<ResourceId, usize>,
}

impl RecipeBook {
    /// An empty book.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a recipe. A later recipe with the same name **replaces** an earlier one,
    /// which is the pack-override rule.
    pub fn insert(&mut self, recipe: Recipe) {
        if let Some(index) = self.by_name.get(recipe.name()).copied() {
            self.recipes[index] = recipe;
            return;
        }
        self.by_name
            .insert(recipe.name().clone(), self.recipes.len());
        self.recipes.push(recipe);
    }

    /// Every recipe, in load order.
    #[must_use]
    pub fn recipes(&self) -> &[Recipe] {
        &self.recipes
    }

    /// How many recipes.
    #[must_use]
    pub fn len(&self) -> usize {
        self.recipes.len()
    }

    /// Whether there are none.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.recipes.is_empty()
    }

    /// A recipe by name.
    #[must_use]
    pub fn by_name(&self, name: &ResourceId) -> Option<&Recipe> {
        self.by_name.get(name).map(|index| &self.recipes[*index])
    }

    /// Recipes producing an item, in load order.
    #[must_use]
    pub fn producing(&self, item: &ResourceId) -> Vec<&Recipe> {
        self.recipes
            .iter()
            .filter(|recipe| recipe.result() == item)
            .collect()
    }

    /// Recipes of one kind.
    #[must_use]
    pub fn of_kind(&self, kind: RecipeKind) -> Vec<&Recipe> {
        self.recipes
            .iter()
            .filter(|recipe| recipe.kind() == kind)
            .collect()
    }
}

/// Load every recipe file under `<namespace_dir>/recipe/`.
///
/// # Errors
///
/// Never fails as a whole: a malformed file is recorded in the report and skipped.
pub fn load_directory(
    namespace_dir: &Path,
    namespace: &str,
    limits: Limits,
    report: &mut RecipeLoadReport,
) -> Vec<Recipe> {
    let base = namespace_dir.join("recipe");
    let mut out = Vec::new();
    for path in crate::tag::json_files(&base) {
        let Ok(relative) = path.strip_prefix(&base) else {
            continue;
        };
        // `with_extension` returns an owned PathBuf, so bind it before borrowing.
        let stem_path = relative.with_extension("");
        let Some(stem) = stem_path.to_str() else {
            continue;
        };
        let stem = stem.replace('\\', "/");
        let Ok(name) = ResourceId::parse(&format!("{namespace}:{stem}")) else {
            report
                .skipped
                .push(format!("{}: unusable recipe name", path.display()));
            continue;
        };
        match read_recipe_file(&path, name, limits) {
            Ok(Some(recipe)) => {
                *report.loaded.entry(recipe.kind()).or_insert(0) += 1;
                out.push(recipe);
            }
            // A recognised-but-unmodelled type: counted, not dropped silently.
            Ok(None) => {}
            Err(Skipped::Unmodelled(type_name)) => {
                *report.unmodelled.entry(type_name).or_insert(0) += 1;
            }
            Err(Skipped::Parse(error)) => report.skipped.push(error.to_string()),
        }
    }
    report.files += out.len();
    out
}

/// Why a recipe file produced no recipe.
enum Skipped {
    /// The type is not modelled by this build.
    Unmodelled(String),
    /// The file is malformed.
    Parse(JsonError),
}

/// Read one recipe file.
fn read_recipe_file(
    path: &Path,
    name: ResourceId,
    limits: Limits,
) -> Result<Option<Recipe>, Skipped> {
    let map = read_json_object(path, limits).map_err(Skipped::Parse)?;
    let type_name = crate::json::required_str(&map, "type", path).map_err(Skipped::Parse)?;
    let group = optional_str(&map, "group", path)
        .map_err(Skipped::Parse)?
        .map(str::to_owned);

    match type_name {
        "minecraft:crafting_shaped" => {
            let pattern = read_pattern(&map, path)?;
            let key = read_key(&map, path)?;
            let (result, result_count) = read_result(&map, path)?;
            Ok(Some(Recipe::Shaped(ShapedRecipe {
                name,
                pattern,
                key,
                result,
                result_count,
                group,
            })))
        }
        "minecraft:crafting_shapeless" => {
            let ingredients = read_ingredient_list(&map, "ingredients", path)?;
            let (result, result_count) = read_result(&map, path)?;
            Ok(Some(Recipe::Shapeless(ShapelessRecipe {
                name,
                ingredients,
                result,
                result_count,
                group,
            })))
        }
        "minecraft:stonecutting" => {
            let ingredient = read_ingredient(&map, "ingredient", path)?;
            let (result, result_count) = read_result(&map, path)?;
            Ok(Some(Recipe::Stonecutting(StonecuttingRecipe {
                name,
                ingredient,
                result,
                result_count,
            })))
        }
        other => {
            let Some(kind) = SmeltingKind::from_type_name(other) else {
                return Err(Skipped::Unmodelled(other.to_owned()));
            };
            let ingredient = read_ingredient(&map, "ingredient", path)?;
            let (result, result_count) = read_result(&map, path)?;
            // The jar always states `cookingtime`; the default is a fallback for a
            // pack that omits it, which vanilla never does.
            // A cooking time outside `i32` is a malformed pack; refuse it rather
            // than truncating to a plausible-looking number.
            let cooking_time =
                match optional_i64(&map, "cookingtime", path).map_err(Skipped::Parse)? {
                    None => kind.default_cooking_time(),
                    Some(value) => i32::try_from(value).map_err(|_| {
                        Skipped::Parse(JsonError::Invalid {
                            path: path.to_path_buf(),
                            reason: format!("cookingtime {value} does not fit a tick count"),
                        })
                    })?,
                };
            // Absent experience means zero, which is what the format specifies.
            let experience = optional_f64(&map, "experience", path)
                .map_err(Skipped::Parse)?
                .unwrap_or(0.0);
            Ok(Some(Recipe::Cooking(CookingRecipe {
                name,
                kind,
                ingredient,
                result,
                result_count,
                cooking_time,
                experience,
            })))
        }
    }
}

fn read_pattern(
    map: &serde_json::Map<String, serde_json::Value>,
    path: &Path,
) -> Result<Vec<String>, Skipped> {
    let rows = crate::json::required_array(map, "pattern", path).map_err(Skipped::Parse)?;
    let mut pattern = Vec::with_capacity(rows.len());
    for row in rows {
        let Some(text) = row.as_str() else {
            return Err(Skipped::Parse(JsonError::Invalid {
                path: path.to_path_buf(),
                reason: "every pattern row must be a string".to_owned(),
            }));
        };
        pattern.push(text.to_owned());
    }
    if pattern.is_empty() {
        return Err(Skipped::Parse(JsonError::Invalid {
            path: path.to_path_buf(),
            reason: "a shaped recipe needs at least one pattern row".to_owned(),
        }));
    }
    Ok(pattern)
}

fn read_key(
    map: &serde_json::Map<String, serde_json::Value>,
    path: &Path,
) -> Result<BTreeMap<char, Vec<Ingredient>>, Skipped> {
    let Some(serde_json::Value::Object(key)) = map.get("key") else {
        return Err(Skipped::Parse(JsonError::Invalid {
            path: path.to_path_buf(),
            reason: "a shaped recipe needs a \"key\" object".to_owned(),
        }));
    };
    let mut out = BTreeMap::new();
    for (symbol, value) in key {
        // The format's keys are single characters; a longer one could never appear in
        // a pattern, so it is a malformed recipe.
        let mut chars = symbol.chars();
        let (Some(character), None) = (chars.next(), chars.next()) else {
            return Err(Skipped::Parse(JsonError::Invalid {
                path: path.to_path_buf(),
                reason: format!("key {symbol:?} must be a single character"),
            }));
        };
        out.insert(character, read_ingredients_value(value, path)?);
    }
    Ok(out)
}

fn read_result(
    map: &serde_json::Map<String, serde_json::Value>,
    path: &Path,
) -> Result<(ResourceId, i32), Skipped> {
    let Some(serde_json::Value::Object(result)) = map.get("result") else {
        return Err(Skipped::Parse(JsonError::Invalid {
            path: path.to_path_buf(),
            reason: "a recipe needs a \"result\" object".to_owned(),
        }));
    };
    let id = crate::json::required_str(result, "id", path).map_err(Skipped::Parse)?;
    let id = ResourceId::parse(id).map_err(|error| {
        Skipped::Parse(JsonError::Invalid {
            path: path.to_path_buf(),
            reason: format!("result id {id:?} is not valid: {error}"),
        })
    })?;
    // `count` is optional and defaults to 1.
    let count = optional_i64(result, "count", path)
        .map_err(Skipped::Parse)?
        .unwrap_or(1);
    // A count outside 1..=64 is a pack bug; refusing beats producing a stack the
    // inventory cannot hold.
    let count = i32::try_from(count).map_err(|_| {
        Skipped::Parse(JsonError::Invalid {
            path: path.to_path_buf(),
            reason: format!("result count {count} does not fit a stack"),
        })
    })?;
    if count < 1 || count > mc_entity_stack_hard_max() {
        return Err(Skipped::Parse(JsonError::Invalid {
            path: path.to_path_buf(),
            reason: format!("result count {count} is outside 1..=64"),
        }));
    }
    Ok((id, count))
}

/// The hard stack ceiling, re-exported so this crate does not depend on `mc-entity`.
///
/// The value is `64`, which is also `mc_entity::stack::HARD_MAX_STACK_SIZE`; the
/// duplication is deliberate because depending on `mc-entity` from a *data loading*
/// crate would invert the layering (entity depends on registry, data depends on
/// registry, and neither should depend on the other).
const fn mc_entity_stack_hard_max() -> i32 {
    64
}

fn read_ingredient(
    map: &serde_json::Map<String, serde_json::Value>,
    key: &str,
    path: &Path,
) -> Result<Vec<Ingredient>, Skipped> {
    let Some(value) = map.get(key) else {
        return Err(Skipped::Parse(JsonError::Invalid {
            path: path.to_path_buf(),
            reason: format!("missing required field {key:?}"),
        }));
    };
    read_ingredients_value(value, path)
}

fn read_ingredient_list(
    map: &serde_json::Map<String, serde_json::Value>,
    key: &str,
    path: &Path,
) -> Result<Vec<Vec<Ingredient>>, Skipped> {
    let list = crate::json::required_array(map, key, path).map_err(Skipped::Parse)?;
    let mut out = Vec::with_capacity(list.len());
    for value in list {
        out.push(read_ingredients_value(value, path)?);
    }
    Ok(out)
}

/// Read an ingredient, which the format writes as a string or an array of strings.
fn read_ingredients_value(
    value: &serde_json::Value,
    path: &Path,
) -> Result<Vec<Ingredient>, Skipped> {
    match value {
        serde_json::Value::String(text) => Ok(vec![parse_ingredient(text, path)?]),
        serde_json::Value::Array(items) => {
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                let Some(text) = item.as_str() else {
                    return Err(Skipped::Parse(JsonError::Invalid {
                        path: path.to_path_buf(),
                        reason: "an ingredient array may only contain strings".to_owned(),
                    }));
                };
                out.push(parse_ingredient(text, path)?);
            }
            if out.is_empty() {
                return Err(Skipped::Parse(JsonError::Invalid {
                    path: path.to_path_buf(),
                    reason: "an ingredient array must not be empty".to_owned(),
                }));
            }
            Ok(out)
        }
        other => Err(Skipped::Parse(JsonError::Invalid {
            path: path.to_path_buf(),
            reason: format!(
                "an ingredient must be a string or an array, found {}",
                crate::json::type_name(other)
            ),
        })),
    }
}

fn parse_ingredient(text: &str, path: &Path) -> Result<Ingredient, Skipped> {
    let (is_tag, rest) = match text.strip_prefix('#') {
        Some(rest) => (true, rest),
        None => (false, text),
    };
    let id = ResourceId::parse(rest).map_err(|error| {
        Skipped::Parse(JsonError::Invalid {
            path: path.to_path_buf(),
            reason: format!("ingredient {text:?} is not a valid name: {error}"),
        })
    })?;
    Ok(if is_tag {
        Ingredient::Tag(id)
    } else {
        Ingredient::Item(id)
    })
}
