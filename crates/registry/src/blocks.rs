//! Block-state id table (P04-01).
//!
//! The fixture (`crates/test-support/fixtures/registry/blocks.tsv`) stores one
//! line per block:
//!
//! ```text
//! <name>\t<first state id>\t<state count>\t<axis list>
//! ```
//!
//! where `axis list` is `prop=v1|v2|v3;prop2=...` (`-` for a stateless block). A
//! state's id is `first + Σ index_i · stride_i` with the **last axis varying
//! fastest** (mixed radix). `compact_blocks.py` proved that form reproduces all
//! 29 873 ids exactly, and `tests` below re-checks a sample against values taken
//! from the uncompressed dump.

use mc_core::error::{ServerError, ServerResult};
use std::collections::HashMap;
use std::path::Path;

/// One block's state layout.
#[derive(Debug, Clone, PartialEq, Eq)]
struct BlockEntry {
    first_state_id: i32,
    state_count: i32,
    /// `(property, ordered values)`, in the order the id index is built.
    axes: Vec<(String, Vec<String>)>,
}

impl BlockEntry {
    /// The id of this block's first (default) state.
    ///
    /// Public so [`BlockRegistry::block_names`] can order by it without reaching into
    /// a private field.
    #[must_use]
    pub const fn first_state_id(&self) -> i32 {
        self.first_state_id
    }

    /// Mixed-radix index of a property assignment, or `None` if a property is
    /// missing or carries an unknown value.
    fn index_of(&self, properties: &[(String, String)]) -> Option<usize> {
        if self.axes.is_empty() {
            return if properties.is_empty() { Some(0) } else { None };
        }
        let mut index = 0usize;
        let mut stride = 1usize;
        for (name, values) in self.axes.iter().rev() {
            let value = properties
                .iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.as_str())?;
            let position = values.iter().position(|candidate| candidate == value)?;
            index += position * stride;
            stride *= values.len();
        }
        if stride != self.state_count as usize {
            return None;
        }
        // Every supplied property must belong to this block.
        if properties
            .iter()
            .any(|(key, _)| !self.axes.iter().any(|(axis, _)| axis == key))
        {
            return None;
        }
        Some(index)
    }

    /// Property assignment for a mixed-radix index.
    fn properties_of(&self, offset: usize) -> Option<Vec<(String, String)>> {
        if offset >= self.state_count as usize {
            return None;
        }
        let mut remaining = offset;
        let mut out = Vec::with_capacity(self.axes.len());
        // Axes are stored least-significant-first for decoding.
        for (name, values) in self.axes.iter().rev() {
            let position = remaining % values.len();
            remaining /= values.len();
            out.push((name.clone(), values[position].clone()));
        }
        out.sort();
        out.reverse();
        Some(out)
    }
}

/// A block state resolved to its wire id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockStateRef {
    /// Registry name, e.g. `minecraft:oak_log`.
    pub name: String,
    /// Sorted `(property, value)` pairs.
    pub properties: Vec<(String, String)>,
    /// Global block-state id sent on the wire.
    pub id: i32,
}

impl BlockStateRef {
    /// Whether this state has no properties.
    #[must_use]
    pub fn is_default(&self) -> bool {
        self.properties.is_empty()
    }
}

/// Block-state lookup table.
#[derive(Debug, Clone)]
pub struct BlockRegistry {
    by_name: HashMap<String, BlockEntry>,
    by_id: Vec<(String, i32)>,
    air_id: i32,
    /// Ids of every state that is air or a fluid-less "empty" block.
    empty_ids: Vec<i32>,
}

/// `minecraft:air`, the id every empty position uses.
pub const AIR: &str = "minecraft:air";

impl BlockRegistry {
    /// Parse a `blocks.tsv` table.
    ///
    /// # Errors
    ///
    /// [`ServerError::Operational`] when the file cannot be read,
    /// [`ServerError::CorruptData`] when a row is malformed or the file is empty.
    pub fn load(path: &Path) -> ServerResult<Self> {
        let text = std::fs::read_to_string(path).map_err(|e| {
            ServerError::Operational(format!("cannot read {}: {e}", path.display()))
        })?;
        Self::parse(&text)
    }

    /// Parse a table from memory (tests, embedded tables).
    ///
    /// One function on purpose: it is a flat row-by-row validation pass, and
    /// splitting it would scatter the line-numbered error messages.
    ///
    /// # Errors
    ///
    /// As for [`BlockRegistry::load`].
    #[allow(clippy::too_many_lines)]
    pub fn parse(text: &str) -> ServerResult<Self> {
        let mut by_name: HashMap<String, BlockEntry> = HashMap::new();
        let mut by_id: Vec<(String, i32)> = Vec::new();

        for (number, line) in text.lines().enumerate() {
            if line.starts_with('#') || line.trim().is_empty() {
                continue;
            }
            let row = number + 1;
            let mut fields = line.split('\t');
            let (Some(name), Some(first), Some(count), Some(axis_text)) =
                (fields.next(), fields.next(), fields.next(), fields.next())
            else {
                return Err(ServerError::CorruptData(format!(
                    "{} line {row}: expected 4 tab-separated fields",
                    "blocks.tsv"
                )));
            };
            let first_state_id: i32 = first.parse().map_err(|_| {
                ServerError::CorruptData(format!("line {row}: bad first state id {first:?}"))
            })?;
            let state_count: i32 = count.parse().map_err(|_| {
                ServerError::CorruptData(format!("line {row}: bad state count {count:?}"))
            })?;
            if state_count < 1 {
                return Err(ServerError::CorruptData(format!(
                    "line {row}: {name} declares {state_count} states"
                )));
            }
            let axes = if axis_text == "-" {
                Vec::new()
            } else {
                axis_text
                    .split(';')
                    .map(|axis| {
                        let (property, values) = axis.split_once('=').ok_or_else(|| {
                            ServerError::CorruptData(format!(
                                "line {row}: axis {axis:?} has no '='"
                            ))
                        })?;
                        let values: Vec<String> = values.split('|').map(str::to_owned).collect();
                        if property.is_empty() || values.iter().any(String::is_empty) {
                            return Err(ServerError::CorruptData(format!(
                                "line {row}: axis {axis:?} has an empty property name or value"
                            )));
                        }
                        Ok((property.to_owned(), values))
                    })
                    .collect::<ServerResult<Vec<_>>>()?
            };
            let product: usize = axes
                .iter()
                .map(|(_, values)| values.len())
                .product::<usize>()
                .max(1);
            if product != state_count as usize {
                return Err(ServerError::CorruptData(format!(
                    "line {row}: {name} declares {state_count} states but its axes multiply to {product}"
                )));
            }
            for offset in 0..state_count {
                by_id.push((name.to_owned(), first_state_id + offset));
            }
            by_name.insert(
                name.to_owned(),
                BlockEntry {
                    first_state_id,
                    state_count,
                    axes,
                },
            );
        }

        if by_name.is_empty() {
            return Err(ServerError::CorruptData("block table is empty".to_owned()));
        }
        by_id.sort_by_key(|(_, id)| *id);
        // Every id must be present exactly once; the fixture is generated that
        // way and a gap would mean a silently wrong lookup.
        for (expected, (_, id)) in by_id.iter().enumerate() {
            if *id != expected as i32 {
                return Err(ServerError::CorruptData(format!(
                    "block table has a gap: position {expected} holds id {id}"
                )));
            }
        }

        let air_id = by_name
            .get(AIR)
            .map(|entry| entry.first_state_id)
            .ok_or_else(|| {
                ServerError::CorruptData("block table has no minecraft:air".to_owned())
            })?;
        // Air plus the other "invisible" states Vanilla treats as empty. Only air
        // is used for emptiness checks in Phase 04; the list exists so the world
        // code has one place to extend (cave_air, void_air).
        let mut empty_ids = Vec::new();
        for name in [AIR, "minecraft:cave_air", "minecraft:void_air"] {
            if let Some(entry) = by_name.get(name) {
                empty_ids.push(entry.first_state_id);
            }
        }

        let registry = Self {
            by_name,
            by_id,
            air_id,
            empty_ids,
        };
        tracing::debug!(
            blocks = registry.by_name.len(),
            states = registry.by_id.len(),
            "block registry loaded"
        );
        Ok(registry)
    }

    /// Number of registered blocks.
    #[must_use]
    pub fn block_count(&self) -> usize {
        self.by_name.len()
    }

    /// Number of registered block states.
    #[must_use]
    pub fn state_count(&self) -> usize {
        self.by_id.len()
    }

    /// Id of `minecraft:air`'s only state.
    #[must_use]
    pub const fn air_id(&self) -> i32 {
        self.air_id
    }

    /// Whether an id is one of the empty (air-like) states.
    #[must_use]
    pub fn is_empty(&self, id: i32) -> bool {
        self.empty_ids.contains(&id)
    }

    /// Whether a block name is registered.
    #[must_use]
    pub fn contains(&self, name: &str) -> bool {
        self.by_name.contains_key(name)
    }

    /// Every **block** name, ascending by the block's first state id.
    ///
    /// A `BlockRegistry` is keyed by *state*, so the same block appears once per
    /// state; this yields each name once, in the order the blocks first appear. Added
    /// for data-pack tag validation (P07-03), which asks about block names, not
    /// states.
    pub fn block_names(&self) -> impl Iterator<Item = &str> {
        // Sorted by the block's first state id, which is the order the fixture
        // defines, so the iteration is reproducible.
        let mut entries: Vec<(&str, i32)> = self
            .by_name
            .iter()
            .map(|(name, entry)| (name.as_str(), entry.first_state_id()))
            .collect();
        entries.sort_unstable_by_key(|(_, first_state)| *first_state);
        entries.into_iter().map(|(name, _)| name)
    }

    /// Default (property-less) state id of a block.
    ///
    /// For a block with properties this is its first state, matching Vanilla's
    /// `defaultBlockState` only when the first property values are the defaults —
    /// callers that need the true default must supply properties.
    ///
    /// # Errors
    ///
    /// [`ServerError::CorruptData`] when the block is not registered. Refusing is
    /// deliberate: substituting air would silently corrupt a world.
    pub fn default_state(&self, name: &str) -> ServerResult<i32> {
        self.by_name
            .get(name)
            .map(|entry| entry.first_state_id)
            .ok_or_else(|| ServerError::CorruptData(format!("unknown block {name:?}")))
    }

    /// Resolve a name plus properties to a state id.
    ///
    /// An empty property list resolves to the block's first state. Unknown
    /// properties, unknown values and unknown blocks are all errors.
    ///
    /// # Errors
    ///
    /// [`ServerError::CorruptData`] as described above.
    pub fn state_id(&self, name: &str, properties: &[(String, String)]) -> ServerResult<i32> {
        let entry = self
            .by_name
            .get(name)
            .ok_or_else(|| ServerError::CorruptData(format!("unknown block {name:?}")))?;
        if properties.is_empty() {
            return Ok(entry.first_state_id);
        }
        let index = entry.index_of(properties).ok_or_else(|| {
            ServerError::CorruptData(format!("block {name:?} has no state for {properties:?}"))
        })?;
        Ok(entry.first_state_id + index as i32)
    }

    /// Resolve a [`BlockStateRef`].
    ///
    /// # Errors
    ///
    /// As for [`BlockRegistry::state_id`].
    pub fn state_ref(
        &self,
        name: &str,
        properties: &[(String, String)],
    ) -> ServerResult<BlockStateRef> {
        let mut properties = properties.to_vec();
        properties.sort();
        Ok(BlockStateRef {
            name: name.to_owned(),
            id: self.state_id(name, &properties)?,
            properties,
        })
    }

    /// Block name owning a state id.
    ///
    /// # Errors
    ///
    /// [`ServerError::CorruptData`] when the id is outside the table.
    pub fn block_name(&self, id: i32) -> ServerResult<&str> {
        usize::try_from(id)
            .ok()
            .and_then(|index| self.by_id.get(index))
            .map(|(name, _)| name.as_str())
            .ok_or_else(|| ServerError::CorruptData(format!("block state id {id} out of range")))
    }

    /// Full property assignment of a state id.
    ///
    /// # Errors
    ///
    /// [`ServerError::CorruptData`] when the id is outside the table or the
    /// owning block has no entry (table inconsistency).
    pub fn properties_of(&self, id: i32) -> ServerResult<Vec<(String, String)>> {
        let name = self.block_name(id)?.to_owned();
        let entry = self.by_name.get(&name).ok_or_else(|| {
            ServerError::CorruptData(format!("block {name:?} missing from the table"))
        })?;
        let offset = usize::try_from(id - entry.first_state_id).unwrap_or(usize::MAX);
        entry.properties_of(offset).ok_or_else(|| {
            ServerError::CorruptData(format!("state id {id} is not inside {name:?}'s range"))
        })
    }

    /// Inverse of [`BlockRegistry::state_id`]: name plus properties for an id.
    ///
    /// # Errors
    ///
    /// As for [`BlockRegistry::properties_of`].
    pub fn state_ref_of(&self, id: i32) -> ServerResult<BlockStateRef> {
        Ok(BlockStateRef {
            name: self.block_name(id)?.to_owned(),
            properties: self.properties_of(id)?,
            id,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::BlockRegistry;
    use mc_core::error::ServerError;

    fn registry() -> BlockRegistry {
        BlockRegistry::load(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../crates/test-support/fixtures/registry/blocks.tsv"),
        )
        .expect("fixture loads")
    }

    #[test]
    fn the_table_covers_the_whole_vanilla_registry() {
        let blocks = registry();
        assert_eq!(blocks.block_count(), 1168, "block count from the jar dump");
        assert_eq!(
            blocks.state_count(),
            29_873,
            "state count from the jar dump"
        );
        assert_eq!(blocks.air_id(), 0);
    }

    #[test]
    fn ids_match_the_uncompressed_jar_dump() {
        // Values read out of target/vanilla-26.1.2/reports/block_states.tsv.
        let blocks = registry();
        // (block, properties, expected id). The tuple is written out per row
        // rather than aliased: one table row per measured state is the point.
        #[allow(clippy::type_complexity)]
        let cases: &[(&str, &[(&str, &str)], i32)] = &[
            ("minecraft:air", &[], 0),
            ("minecraft:stone", &[], 1),
            ("minecraft:granite", &[], 2),
            ("minecraft:grass_block", &[("snowy", "true")], 8),
            ("minecraft:grass_block", &[("snowy", "false")], 9),
            ("minecraft:oak_log", &[("axis", "x")], 136),
            ("minecraft:oak_log", &[("axis", "y")], 137),
            ("minecraft:oak_log", &[("axis", "z")], 138),
            (
                "minecraft:chest",
                &[
                    ("facing", "north"),
                    ("type", "single"),
                    ("waterlogged", "true"),
                ],
                3987,
            ),
            (
                "minecraft:chest",
                &[
                    ("facing", "north"),
                    ("type", "single"),
                    ("waterlogged", "false"),
                ],
                3988,
            ),
        ];
        for (name, properties, expected) in cases {
            let properties: Vec<(String, String)> = properties
                .iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect();
            assert_eq!(
                blocks.state_id(name, &properties).expect("resolves"),
                *expected,
                "{name} {properties:?}"
            );
            // And the inverse direction agrees.
            let state = blocks.state_ref_of(*expected).expect("resolves");
            assert_eq!(state.name, *name);
            assert_eq!(state.id, *expected);
        }
    }

    #[test]
    fn every_state_round_trips_through_the_table() {
        // The strongest check available without the raw dump: name+properties ->
        // id -> name+properties must be the identity for all 29 873 states.
        let blocks = registry();
        for id in 0..blocks.state_count() as i32 {
            let state = blocks.state_ref_of(id).expect("resolves");
            let again = blocks
                .state_id(&state.name, &state.properties)
                .expect("re-resolves");
            assert_eq!(
                again, id,
                "{} {id} -> {:?} -> {again}",
                state.name, state.properties
            );
        }
    }

    #[test]
    fn property_order_does_not_matter_on_lookup() {
        let blocks = registry();
        let a = blocks
            .state_id(
                "minecraft:chest",
                &[
                    ("facing".to_owned(), "west".to_owned()),
                    ("type".to_owned(), "right".to_owned()),
                    ("waterlogged".to_owned(), "false".to_owned()),
                ],
            )
            .expect("resolves");
        let b = blocks
            .state_id(
                "minecraft:chest",
                &[
                    ("waterlogged".to_owned(), "false".to_owned()),
                    ("facing".to_owned(), "west".to_owned()),
                    ("type".to_owned(), "right".to_owned()),
                ],
            )
            .expect("resolves");
        assert_eq!(a, b);
    }

    #[test]
    fn unknown_blocks_properties_and_values_are_refused() {
        let blocks = registry();
        assert!(matches!(
            blocks.default_state("minecraft:not_a_block"),
            Err(ServerError::CorruptData(_))
        ));
        assert!(
            blocks
                .state_id("minecraft:stone", &[("axis".to_owned(), "y".to_owned())])
                .is_err(),
            "a stateless block must reject properties"
        );
        assert!(
            blocks
                .state_id("minecraft:oak_log", &[("axis".to_owned(), "w".to_owned())])
                .is_err(),
            "an unknown property value must be rejected"
        );
        assert!(
            blocks
                .state_id("minecraft:oak_log", &[("nope".to_owned(), "y".to_owned())])
                .is_err(),
            "an unknown property name must be rejected"
        );
        assert!(blocks.block_name(999_999).is_err());
        assert!(blocks.properties_of(-1).is_err());
        assert!(blocks.state_ref_of(999_999).is_err());
    }

    #[test]
    fn emptiness_is_recognised() {
        let blocks = registry();
        assert!(blocks.is_empty(blocks.air_id()));
        assert!(!blocks.is_empty(blocks.default_state("minecraft:stone").expect("stone")));
    }

    #[test]
    fn malformed_tables_are_rejected() {
        assert!(matches!(
            BlockRegistry::parse(""),
            Err(ServerError::CorruptData(_))
        ));
        // Wrong field count.
        assert!(BlockRegistry::parse("minecraft:air\t0\t1").is_err());
        // Axis product disagrees with the declared state count (2 vs 3).
        assert!(BlockRegistry::parse("minecraft:air\t0\t3\taxis=x|y").is_err());
        // Axis without values.
        assert!(BlockRegistry::parse("minecraft:air\t0\t1\taxis=").is_err());
        // Axis without '='.
        assert!(BlockRegistry::parse("minecraft:air\t0\t1\taxis").is_err());
        // Id gap.
        assert!(BlockRegistry::parse("minecraft:air\t0\t1\t-\nminecraft:stone\t5\t1\t-").is_err());
        // No air.
        assert!(BlockRegistry::parse("minecraft:stone\t0\t1\t-").is_err());
    }
}
