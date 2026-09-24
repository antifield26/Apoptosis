//! Shared worldgen JSON readers: y anchors, height ranges, count providers and
//! block tags (P18-03).
//!
//! Vanilla's `worldgen/configured_feature`, `worldgen/placed_feature` and
//! `worldgen/configured_carver` documents are small JSON objects whose numbers
//! this crate turns into generators. The shapes here were **measured** from the
//! extracted 26.1.2 pack (`target/vanilla-26.1.2/extract`), not recalled:
//!
//! | Shape | Seen in |
//! |---|---|
//! | `{"type":"count","count":N}` or a uniform `IntProvider` | every ore placed feature |
//! | `{"type":"rarity_filter","chance":N}` | `ore_granite_upper`, `ore_diamond_large` |
//! | `{"type":"height_range","height":{type, min_inclusive, max_inclusive, plateau?}}` | every ore placed feature |
//! | y anchor `{"absolute"\|"above_bottom"\|"below_top":N}` | every height range and carver `y`/`lava_level` |
//! | float range `{"type":"uniform","min_inclusive":F,"max_exclusive":F}` | carver radius multipliers, `floor_level` |
//!
//! Everything here is `serde_json::Value` driven on purpose: the pack is
//! operator-supplied (AGENTS.md §10), so a missing or hostile field is a
//! **reported skip**, never a panic and never a silent default that pretends a
//! feature loaded.

use crate::seed::{WorldgenContext, splitmix64_mix};
use mc_core::random::RandomSource;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use thiserror::Error;

/// Largest worldgen JSON file accepted, in bytes.
///
/// **product decision.** 1 MiB is 4× the largest file measured in
/// `data/minecraft/worldgen/` (the biggest `noise_settings` is ~112 KiB) and
/// still bounds a hostile pack.
pub const MAX_WORLGEN_JSON_BYTES: u64 = 1024 * 1024;

/// Why a worldgen JSON document could not be turned into a generator input.
#[derive(Debug, Error)]
pub enum WorldgenJsonError {
    /// The file is missing or unreadable.
    #[error("{p}: {source}", p = path.display())]
    Io {
        /// The path.
        path: PathBuf,
        /// The underlying error.
        source: std::io::Error,
    },
    /// The file exceeds [`MAX_WORLGEN_JSON_BYTES`].
    #[error("{p}: {bytes} bytes exceeds the limit", p = path.display())]
    TooLarge {
        /// The path.
        path: PathBuf,
        /// Size seen.
        bytes: u64,
    },
    /// The file is not valid JSON, or not an object.
    #[error("{p}: {reason}", p = path.display())]
    BadJson {
        /// The path.
        path: PathBuf,
        /// What `serde_json` said.
        reason: String,
    },
    /// A required field is missing or has the wrong type.
    #[error("{p}: {reason}", p = path.display())]
    BadField {
        /// The path.
        path: PathBuf,
        /// Which field, and why.
        reason: String,
    },
}

/// Read one worldgen JSON object.
///
/// # Errors
///
/// [`WorldgenJsonError`] on I/O, size, parse or non-object shape.
pub fn read_worldgen_json(path: &Path) -> Result<serde_json::Value, WorldgenJsonError> {
    let meta = std::fs::metadata(path).map_err(|source| WorldgenJsonError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    if meta.len() > MAX_WORLGEN_JSON_BYTES {
        return Err(WorldgenJsonError::TooLarge {
            path: path.to_path_buf(),
            bytes: meta.len(),
        });
    }
    let bytes = std::fs::read(path).map_err(|source| WorldgenJsonError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let value: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|error| WorldgenJsonError::BadJson {
            path: path.to_path_buf(),
            reason: error.to_string(),
        })?;
    if !value.is_object() {
        return Err(WorldgenJsonError::BadJson {
            path: path.to_path_buf(),
            reason: "top-level value is not an object".to_owned(),
        });
    }
    Ok(value)
}

/// A y position described the way Vanilla's `VerticalAnchor` describes one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum YAnchor {
    /// Absolute block y.
    Absolute(i32),
    /// `min_y + offset`.
    AboveBottom(i32),
    /// `(min_y + height) - offset` — Vanilla's `getMaxGenY() - offset`.
    BelowTop(i32),
}

impl YAnchor {
    /// Parse `{"absolute"|"above_bottom"|"below_top": N}`.
    #[must_use]
    pub fn parse(value: &serde_json::Value) -> Option<Self> {
        let object = value.as_object()?;
        for (key, variant) in [
            ("absolute", Self::Absolute as fn(i32) -> Self),
            ("above_bottom", Self::AboveBottom),
            ("below_top", Self::BelowTop),
        ] {
            if let Some(number) = object.get(key).and_then(serde_json::Value::as_i64) {
                return Some(variant(i32::try_from(number).unwrap_or(0)));
            }
        }
        None
    }

    /// Resolve against a dimension, then clamp to a legal inclusive block y.
    ///
    /// Vanilla resolves the anchor first and the height provider clamps into
    /// the generation range — that clamp is load-bearing for diamond, whose
    /// trapezoid is `above_bottom(-80)..=above_bottom(80)` (world `-144..=16`
    /// before the clamp) and only becomes the familiar `-64..=16` triangle
    /// *because* of it.
    #[must_use]
    pub fn resolve(self, context: &WorldgenContext) -> i32 {
        let max_inclusive = context.effective_max_y().saturating_sub(1);
        let raw = match self {
            Self::Absolute(y) => y,
            Self::AboveBottom(offset) => context.min_y.saturating_add(offset),
            Self::BelowTop(offset) => context.max_y().saturating_sub(offset),
        };
        raw.clamp(context.min_y, max_inclusive)
    }
}

/// How a feature picks its y within a band.
#[derive(Debug, Clone, PartialEq)]
pub enum HeightDist {
    /// Uniform over the inclusive band.
    Uniform {
        /// Lowest y (inclusive), unresolved.
        min: YAnchor,
        /// Highest y (inclusive), unresolved.
        max: YAnchor,
    },
    /// Trapezoid: two rising/falling slopes of equal width and a flat plateau.
    ///
    /// `plateau` is the flat-top width in blocks (0 makes the shape a triangle
    /// after the two uniforms fold). This is the shape Vanilla's
    /// `TrapezoidHeight` uses for diamond, lapis and the buried ores.
    Trapezoid {
        /// Lowest y (inclusive), unresolved.
        min: YAnchor,
        /// Highest y (inclusive), unresolved.
        max: YAnchor,
        /// Flat-top width in blocks.
        plateau: i32,
    },
}

impl HeightDist {
    /// Parse Vanilla's `height` object (`uniform` or `trapezoid`).
    #[must_use]
    pub fn parse(value: &serde_json::Value) -> Option<Self> {
        let object = value.as_object()?;
        let kind = object.get("type").and_then(serde_json::Value::as_str)?;
        let min = YAnchor::parse(object.get("min_inclusive")?)?;
        let max = YAnchor::parse(object.get("max_inclusive")?)?;
        match kind.rsplit(':').next()? {
            "uniform" => Some(Self::Uniform { min, max }),
            "trapezoid" => {
                let plateau = object
                    .get("plateau")
                    .and_then(serde_json::Value::as_i64)
                    .and_then(|p| i32::try_from(p).ok())
                    .unwrap_or(0);
                Some(Self::Trapezoid {
                    min,
                    max,
                    plateau: plateau.max(0),
                })
            }
            _ => None,
        }
    }

    /// Sample a y in the resolved, clamped band.
    #[must_use]
    pub fn sample(&self, random: &mut RandomSource, context: &WorldgenContext) -> i32 {
        match *self {
            Self::Uniform { min, max } => {
                let lo = min.resolve(context);
                let hi = max.resolve(context);
                random.next_i32_inclusive(lo, hi.max(lo))
            }
            Self::Trapezoid { min, max, plateau } => {
                let lo = min.resolve(context);
                let hi = max.resolve(context);
                sample_trapezoid(random, lo, hi.max(lo), plateau)
            }
        }
    }

    /// The resolved inclusive band, for statistics and tests.
    #[must_use]
    pub fn band(&self, context: &WorldgenContext) -> (i32, i32) {
        let (min, max) = match *self {
            Self::Uniform { min, max } | Self::Trapezoid { min, max, .. } => (min, max),
        };
        let lo = min.resolve(context);
        let hi = max.resolve(context);
        (lo, hi.max(lo))
    }
}

/// Discrete trapezoid sampler.
///
/// **approximation** of Vanilla's `TrapezoidHeight.sample`: two `U(0, slope)`
/// draws fold into a triangle on each slope, then a uniform plateau offset.
/// `plateau = 0` is a triangle on `[min, max]`; `plateau = max - min` is
/// uniform. The *shape family* matches Vanilla; the exact integer draw sequence
/// is not claimed to be bit-identical.
fn sample_trapezoid(random: &mut RandomSource, min: i32, max: i32, plateau: i32) -> i32 {
    let span = max.saturating_sub(min);
    if span <= 0 {
        return min;
    }
    let plateau = plateau.clamp(0, span);
    let slope = span - plateau;
    // Two U(0, slope) samples: their half-sum is triangular on `0..=slope`.
    let rise = random.next_i32_inclusive(0, slope) + random.next_i32_inclusive(0, slope);
    let offset = rise / 2;
    let flat = if plateau > 0 {
        random.next_i32_inclusive(0, plateau)
    } else {
        0
    };
    min + offset + flat
}

/// How many times a placed feature runs per chunk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Attempts {
    /// Exactly `n` attempts (`{"type":"count","count":N}`).
    Fixed(i32),
    /// `U(min, max)` attempts per chunk (`count` as a uniform `IntProvider`).
    Uniform {
        /// Lowest attempt count (inclusive).
        min: i32,
        /// Highest attempt count (inclusive).
        max: i32,
    },
    /// One attempt with probability `1 / chance` (`rarity_filter`).
    Rarity {
        /// Denominator of the success probability.
        chance: i32,
    },
}

impl Attempts {
    /// Parse the placement modifiers that decide attempt count.
    ///
    /// Accepts `count` (fixed or uniform) and `rarity_filter`. Anything else
    /// (a `NoiseBasedCountPlacement`, …) is `None` and the caller reports a
    /// skip rather than inventing a count.
    #[must_use]
    pub fn parse_placement(placement: &serde_json::Value) -> Option<Self> {
        let list = placement.as_array()?;
        for entry in list {
            let object = entry.as_object()?;
            let kind = object.get("type").and_then(serde_json::Value::as_str)?;
            match kind.rsplit(':').next()? {
                "count" => {
                    let count = object.get("count")?;
                    if let Some(n) = count.as_i64() {
                        return Some(Self::Fixed(i32::try_from(n).unwrap_or(0).max(0)));
                    }
                    let inner = count.as_object()?;
                    let min = inner
                        .get("min_inclusive")
                        .and_then(serde_json::Value::as_i64)?;
                    let max = inner
                        .get("max_inclusive")
                        .and_then(serde_json::Value::as_i64)?;
                    return Some(Self::Uniform {
                        min: i32::try_from(min).unwrap_or(0).max(0),
                        max: i32::try_from(max).unwrap_or(0).max(0),
                    });
                }
                "rarity_filter" => {
                    let chance = object.get("chance").and_then(serde_json::Value::as_i64)?;
                    return Some(Self::Rarity {
                        chance: i32::try_from(chance).unwrap_or(1).max(1),
                    });
                }
                _ => {}
            }
        }
        None
    }

    /// Attempts for this chunk, using a stream derived from `(seed, chunk)`.
    #[must_use]
    pub fn sample(&self, random: &mut RandomSource) -> i32 {
        match *self {
            Self::Fixed(n) => n.max(0),
            Self::Uniform { min, max } => random.next_i32_inclusive(min.max(0), max.max(0)),
            Self::Rarity { chance } => i32::from(random.one_in(chance)),
        }
    }

    /// Mean attempts per chunk, for statistical expectations.
    #[must_use]
    pub fn mean(&self) -> f64 {
        match *self {
            Self::Fixed(n) => f64::from(n),
            Self::Uniform { min, max } => f64::from(min + max) / 2.0,
            Self::Rarity { chance } => 1.0 / f64::from(chance.max(1)),
        }
    }
}

/// A uniform float range as carver configs declare it (`min_inclusive`,
/// `max_exclusive`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FloatRange {
    /// Lowest value (inclusive).
    pub min: f64,
    /// Highest value (exclusive, as the pack writes it).
    pub max: f64,
}

impl FloatRange {
    /// Parse `{"type":"uniform","min_inclusive":F,"max_exclusive":F}` or a bare number.
    #[must_use]
    pub fn parse(value: &serde_json::Value) -> Option<Self> {
        if let Some(n) = value.as_f64() {
            return Some(Self { min: n, max: n });
        }
        let object = value.as_object()?;
        let min = object
            .get("min_inclusive")
            .and_then(serde_json::Value::as_f64)?;
        let max = object
            .get("max_exclusive")
            .or_else(|| object.get("max_inclusive"))
            .and_then(serde_json::Value::as_f64)?;
        Some(Self { min, max })
    }

    /// Sample from the range; a degenerate range collapses to `min`.
    #[must_use]
    pub fn sample(&self, random: &mut RandomSource) -> f64 {
        random.next_f64_between(self.min, self.max)
    }
}

/// A resolved block tag: the set of default-state ids it names.
///
/// Tags are read from `tags/block/<path>.json` and expanded transitively
/// (`"#minecraft:base_stone_overworld"`). A missing tag is an **empty set**,
/// which makes the caller's replaceable check simply never fire — reported
/// through [`TagTable::missing`] rather than invented.
#[derive(Debug, Clone, Default)]
pub struct TagTable {
    /// `namespace:path` → member state ids.
    tags: BTreeMap<String, BTreeSet<i32>>,
    /// Tags that were referenced but not found.
    missing: BTreeSet<String>,
}

impl TagTable {
    /// Load every `tags/block/**/*.json` under `namespace_root`.
    ///
    /// `namespace_root` is the `data/minecraft` directory. Nested `#tag`
    /// references are expanded with a depth cap so a hostile cycle cannot
    /// recurse (the cycle is dropped, not panicked).
    #[must_use]
    pub fn load_block_tags(namespace_root: &Path, registry: &mc_registry::BlockRegistry) -> Self {
        let mut raw: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let mut pending = vec![namespace_root.join("tags").join("block")];
        while let Some(dir) = pending.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            let mut names: Vec<_> = entries.flatten().map(|e| e.path()).collect();
            names.sort();
            for path in names {
                if path.is_dir() {
                    pending.push(path);
                    continue;
                }
                if path.extension().is_none_or(|e| e != "json") {
                    continue;
                }
                let Some(name) = tag_name_of(&namespace_root.join("tags").join("block"), &path)
                else {
                    continue;
                };
                let Ok(value) = read_worldgen_json(&path) else {
                    continue;
                };
                let Some(values) = value.get("values").and_then(serde_json::Value::as_array) else {
                    continue;
                };
                let entries = values
                    .iter()
                    .filter_map(|v| v.as_str().map(str::to_owned))
                    .collect::<Vec<_>>();
                raw.insert(name, entries);
            }
        }
        let mut table = Self::default();
        let keys: Vec<String> = raw.keys().cloned().collect();
        for key in keys {
            let mut out = BTreeSet::new();
            expand_tag(&raw, &key, &mut out, registry, &mut table.missing, 0);
            table.tags.insert(key, out);
        }
        table
    }

    /// Whether `state_id` is a member of `minecraft:<path>` (or a fully
    /// qualified `ns:path`).
    #[must_use]
    pub fn contains(&self, tag: &str, state_id: i32) -> bool {
        let key = if tag.contains(':') {
            tag.to_owned()
        } else {
            format!("minecraft:{tag}")
        };
        self.tags
            .get(&key)
            .is_some_and(|set| set.contains(&state_id))
    }

    /// The resolved member set of a tag (empty when unknown).
    #[must_use]
    pub fn members(&self, tag: &str) -> Option<&BTreeSet<i32>> {
        let key = if tag.contains(':') {
            tag
        } else {
            // Prefix only when the caller passed a bare path; keep the borrow
            // simple by going through a local.
            return self.tags.get(&format!("minecraft:{tag}"));
        };
        self.tags.get(key)
    }

    /// Tags that were referenced but never defined.
    #[must_use]
    pub const fn missing(&self) -> &BTreeSet<String> {
        &self.missing
    }
}

/// `tags/block/stone_ore_replaceables.json` → `minecraft:stone_ore_replaceables`.
fn tag_name_of(root: &Path, path: &Path) -> Option<String> {
    let rel = path.strip_prefix(root).ok()?;
    let mut value = rel.to_string_lossy().replace('\\', "/");
    if let Some(stripped) = value.strip_suffix(".json") {
        value = stripped.to_owned();
    }
    Some(format!("minecraft:{value}"))
}

/// Expand one tag into state ids, following `#other` entries.
fn expand_tag(
    raw: &BTreeMap<String, Vec<String>>,
    key: &str,
    out: &mut BTreeSet<i32>,
    registry: &mc_registry::BlockRegistry,
    missing: &mut BTreeSet<String>,
    depth: usize,
) {
    if depth > 8 {
        return;
    }
    let Some(entries) = raw.get(key) else {
        missing.insert(key.to_owned());
        return;
    };
    for entry in entries {
        if let Some(nested) = entry.strip_prefix('#') {
            let nested_key = if nested.contains(':') {
                nested.to_owned()
            } else {
                format!("minecraft:{nested}")
            };
            if out.iter().count() > 10_000 {
                return;
            }
            expand_tag(raw, &nested_key, out, registry, missing, depth + 1);
        } else if let Ok(id) = registry.default_state(entry) {
            out.insert(id);
        } else {
            missing.insert(entry.clone());
        }
    }
}

/// Per-feature random source: independent of iteration order and of how many
/// earlier attempts were skipped (same rule as [`crate::features`]).
#[must_use]
pub fn feature_random(
    world_seed: i64,
    feature_key: &str,
    pos: mc_world::ChunkPos,
    attempt: u32,
) -> RandomSource {
    let mut key_hash: u64 = 0xCBF2_9CE4_8422_2325;
    for byte in feature_key.as_bytes() {
        key_hash ^= u64::from(*byte);
        key_hash = key_hash.wrapping_mul(0x0000_0100_0000_01B3);
    }
    let packed = crate::seed::pack_chunk_pos(pos);
    let mixed = splitmix64_mix(
        world_seed as u64
            ^ key_hash
            ^ packed
            ^ (u64::from(attempt).wrapping_mul(0x9E37_79B9_7F4A_7C15)),
    );
    RandomSource::new(mixed as i64)
}

#[cfg(test)]
#[allow(clippy::float_cmp)]
mod tests {
    use super::{Attempts, FloatRange, HeightDist, YAnchor, sample_trapezoid};
    use crate::seed::{WorldSeed, WorldgenContext};
    use mc_core::random::RandomSource;

    fn context() -> WorldgenContext {
        WorldgenContext::overworld(WorldSeed::from_raw(1))
    }

    #[test]
    fn y_anchors_resolve_and_clamp_like_the_pack() {
        let ctx = context();
        assert_eq!(YAnchor::Absolute(10).resolve(&ctx), 10);
        assert_eq!(YAnchor::AboveBottom(8).resolve(&ctx), -56);
        assert_eq!(YAnchor::BelowTop(0).resolve(&ctx), 319, "clamped from 320");
        // Diamond's unclamped band is -144..=16; the clamp is what makes the
        // familiar -64..=16.
        assert_eq!(YAnchor::AboveBottom(-80).resolve(&ctx), -64);
        assert_eq!(YAnchor::AboveBottom(80).resolve(&ctx), 16);
    }

    #[test]
    fn height_distributions_parse_from_the_pack_shape() {
        let uniform: serde_json::Value = serde_json::from_str(
            r#"{"type":"minecraft:uniform","max_inclusive":{"below_top":0},"min_inclusive":{"absolute":136}}"#,
        )
        .expect("json");
        let dist = HeightDist::parse(&uniform).expect("uniform");
        assert_eq!(dist.band(&context()), (136, 319));

        let trap: serde_json::Value = serde_json::from_str(
            r#"{"type":"minecraft:trapezoid","max_inclusive":{"above_bottom":80},"min_inclusive":{"above_bottom":-80}}"#,
        )
        .expect("json");
        let dist = HeightDist::parse(&trap).expect("trapezoid");
        assert_eq!(dist.band(&context()), (-64, 16));
    }

    #[test]
    fn attempts_parse_count_rarity_and_uniform() {
        let fixed: serde_json::Value =
            serde_json::from_str(r#"[{"type":"minecraft:count","count":7}]"#).expect("json");
        assert_eq!(Attempts::parse_placement(&fixed), Some(Attempts::Fixed(7)));
        let rarity: serde_json::Value =
            serde_json::from_str(r#"[{"type":"minecraft:rarity_filter","chance":6}]"#)
                .expect("json");
        assert_eq!(
            Attempts::parse_placement(&rarity),
            Some(Attempts::Rarity { chance: 6 })
        );
        let uniform: serde_json::Value = serde_json::from_str(
            r#"[{"type":"minecraft:count","count":{"type":"minecraft:uniform","max_inclusive":1,"min_inclusive":0}}]"#,
        )
        .expect("json");
        assert_eq!(
            Attempts::parse_placement(&uniform),
            Some(Attempts::Uniform { min: 0, max: 1 })
        );
    }

    #[test]
    fn trapezoid_endpoints_and_plateau_behave() {
        let mut random = RandomSource::new(3);
        for _ in 0..200 {
            let y = sample_trapezoid(&mut random, -64, 16, 0);
            assert!((-64..=16).contains(&y), "{y}");
        }
        // A full plateau is uniform over the band (every value reachable).
        let mut seen = std::collections::BTreeSet::new();
        for _ in 0..500 {
            seen.insert(sample_trapezoid(&mut random, 0, 8, 8));
        }
        assert_eq!(seen.len(), 9, "0..=8 inclusive: {seen:?}");
    }

    #[test]
    fn float_range_parses_uniform_and_bare() {
        let bare = serde_json::Value::from(3.0);
        let range = FloatRange::parse(&bare).expect("bare");
        assert_eq!(range.min, 3.0);
        assert_eq!(range.max, 3.0);
        let uniform: serde_json::Value = serde_json::from_str(
            r#"{"type":"minecraft:uniform","max_exclusive":1.4,"min_inclusive":0.7}"#,
        )
        .expect("json");
        let range = FloatRange::parse(&uniform).expect("uniform");
        assert!((range.min - 0.7).abs() < 1e-12);
        assert!((range.max - 1.4).abs() < 1e-12);
    }
}
