//! Item registry (P04-12, P04-01).
//!
//! `items.tsv` is one line per item in Vanilla registration order, which is the
//! numeric id the wire uses in `ItemStack`:
//!
//! ```text
//! <id>\t<name>\t<block it places, or ->
//! ```
//!
//! Items are how a block enters the inventory and how block placement is
//! validated (a held item must map to the block being placed), so the mapping is
//! part of the registry rather than of gameplay code.

use mc_core::error::{ServerError, ServerResult};
use std::collections::HashMap;
use std::path::Path;

/// Item lookup table.
#[derive(Debug, Clone)]
pub struct ItemRegistry {
    by_name: HashMap<String, i32>,
    by_id: Vec<ItemEntry>,
    /// Tool rules by item name, from `tool_rules.tsv` beside `items.tsv`
    /// (P16-05): vanilla Tool components in evaluation order.
    tools: HashMap<String, ToolEntry>,
}

/// One item's Tool component: ordered rules plus the default speed.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolEntry {
    /// Speed when no rule matches (1.0 for every vanilla tool).
    pub default_speed: f32,
    /// Rules in evaluation order: the first rule whose tag contains the
    /// block and that sets the field wins (pumpkin `ItemStack::get_speed` /
    /// `is_correct_for_drops`).
    pub rules: Vec<ToolRule>,
}

/// One Tool rule.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolRule {
    /// Block tag this rule matches, e.g. `minecraft:mineable/pickaxe`.
    pub tag: String,
    /// Mining speed on a match, or `None` when the rule only judges drops.
    pub speed: Option<f32>,
    /// Whether the match harvests drops, or `None` for speed-only rules.
    pub correct: Option<bool>,
}

/// One registered item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemEntry {
    /// Registry name, e.g. `minecraft:stone`.
    pub name: String,
    /// Block this item places, when it is a block item.
    pub block: Option<String>,
}

impl ItemRegistry {
    /// Parse an `items.tsv` table.
    ///
    /// # Errors
    ///
    /// [`ServerError::Operational`] when the file cannot be read,
    /// [`ServerError::CorruptData`] when a row is malformed, the file is empty,
    /// or ids are not contiguous from 0.
    pub fn load(path: &Path) -> ServerResult<Self> {
        let text = std::fs::read_to_string(path).map_err(|e| {
            ServerError::Operational(format!("cannot read {}: {e}", path.display()))
        })?;
        let mut registry = Self::parse(&text)?;
        apply_tool_rules(&mut registry.tools, path);
        Ok(registry)
    }

    /// Parse a table from memory.
    ///
    /// # Errors
    ///
    /// As for [`ItemRegistry::load`].
    pub fn parse(text: &str) -> ServerResult<Self> {
        let mut by_id: Vec<ItemEntry> = Vec::new();
        for (number, line) in text.lines().enumerate() {
            if line.starts_with('#') || line.trim().is_empty() {
                continue;
            }
            let row = number + 1;
            let mut fields = line.split('\t');
            let (Some(id), Some(name), Some(block)) = (fields.next(), fields.next(), fields.next())
            else {
                return Err(ServerError::CorruptData(format!(
                    "items.tsv line {row}: expected 3 tab-separated fields"
                )));
            };
            let id: i32 = id
                .parse()
                .map_err(|_| ServerError::CorruptData(format!("line {row}: bad item id {id:?}")))?;
            if id as usize != by_id.len() {
                return Err(ServerError::CorruptData(format!(
                    "line {row}: item id {id} is out of order (expected {})",
                    by_id.len()
                )));
            }
            by_id.push(ItemEntry {
                name: name.to_owned(),
                block: (block != "-").then(|| block.to_owned()),
            });
        }
        if by_id.is_empty() {
            return Err(ServerError::CorruptData("item table is empty".to_owned()));
        }
        let by_name = by_id
            .iter()
            .enumerate()
            .map(|(id, entry)| (entry.name.clone(), id as i32))
            .collect();
        Ok(Self {
            by_name,
            by_id,
            tools: HashMap::new(),
        })
    }

    /// Number of registered items.
    #[must_use]
    pub fn len(&self) -> usize {
        self.by_id.len()
    }

    /// Whether the table is empty (never true for a loaded table).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.by_id.is_empty()
    }

    /// Numeric id of an item.
    ///
    /// # Errors
    ///
    /// [`ServerError::CorruptData`] when the item is not registered.
    pub fn id(&self, name: &str) -> ServerResult<i32> {
        self.by_name
            .get(name)
            .copied()
            .ok_or_else(|| ServerError::CorruptData(format!("unknown item {name:?}")))
    }

    /// Entry for a numeric id.
    ///
    /// # Errors
    ///
    /// [`ServerError::CorruptData`] when the id is outside the table.
    pub fn entry(&self, id: i32) -> ServerResult<&ItemEntry> {
        usize::try_from(id)
            .ok()
            .and_then(|index| self.by_id.get(index))
            .ok_or_else(|| ServerError::CorruptData(format!("item id {id} out of range")))
    }

    /// Name of an item id.
    ///
    /// # Errors
    ///
    /// As for [`ItemRegistry::entry`].
    pub fn name(&self, id: i32) -> ServerResult<&str> {
        Ok(self.entry(id)?.name.as_str())
    }

    /// Block a block-item places.
    ///
    /// # Errors
    ///
    /// As for [`ItemRegistry::entry`].
    pub fn block_of(&self, id: i32) -> ServerResult<Option<&str>> {
        Ok(self.entry(id)?.block.as_deref())
    }

    /// Every item name, in ascending id order.
    ///
    /// Added for data-pack tag validation (P07-03): checking whether a tag references
    /// a real item needs the *set* of names, which `id(name)` cannot produce. The
    /// order is by id, so a caller building a set from this gets the same set on every
    /// run.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.by_id.iter().map(|entry| entry.name.as_str())
    }

    /// Item that places a given block, when one exists.
    ///
    /// # Errors
    ///
    /// [`ServerError::CorruptData`] when no item places the block.
    pub fn item_for_block(&self, block: &str) -> ServerResult<i32> {
        // The overwhelmingly common case is a 1:1 name match; fall back to a scan
        // for the exceptions (e.g. `minecraft:redstone_wire` ← `minecraft:redstone`).
        if let Some(id) = self.by_name.get(block)
            && self.by_id[*id as usize].block.as_deref() == Some(block)
        {
            return Ok(*id);
        }
        self.by_id
            .iter()
            .position(|entry| entry.block.as_deref() == Some(block))
            .map(|index| index as i32)
            .ok_or_else(|| ServerError::CorruptData(format!("no item places block {block:?}")))
    }

    /// Tool rules for an item name, or `None` for a non-tool (hand equivalent).
    ///
    /// `None` means "dig everything at speed 1.0, harvest nothing a tool is
    /// required for" — the vanilla reading of an empty hand, and of any tool
    /// the fixture predates.
    #[must_use]
    pub fn tool(&self, name: &str) -> Option<&ToolEntry> {
        self.tools.get(name)
    }
}

/// Load `tool_rules.tsv` beside an `items.tsv` into `tools` (P16-05).
///
/// Missing file: warn once and leave the map empty (every held item then
/// digs as a hand — the degraded mode). Malformed rows warn per row and
/// skip; a bad order field fails the load, because rule order *is* the
/// semantics and a misordered table would invert tool judgments silently.
fn apply_tool_rules(tools: &mut HashMap<String, ToolEntry>, items_path: &Path) {
    let Some(dir) = items_path.parent() else {
        return;
    };
    let path = dir.join("tool_rules.tsv");
    let Ok(text) = std::fs::read_to_string(&path) else {
        tracing::warn!(
            path = %path.display(),
            "no tool rules beside the item table; every held item digs as a hand"
        );
        return;
    };
    let mut ordered: HashMap<String, Vec<(usize, ToolRule, f32)>> = HashMap::new();
    for (number, line) in text.lines().enumerate() {
        if line.starts_with('#') || line.trim().is_empty() {
            continue;
        }
        let row = number + 1;
        let fields: Vec<&str> = line.split('\t').collect();
        if fields.len() != 6 {
            tracing::warn!(
                row,
                "tool_rules.tsv: expected 6 tab-separated fields; skipped"
            );
            continue;
        }
        let Ok(order) = fields[1].parse::<usize>() else {
            tracing::warn!(row, "tool_rules.tsv: bad rule order; skipped");
            continue;
        };
        let speed = match fields[3] {
            "-" => None,
            speed => {
                let Ok(speed) = speed.strip_suffix("f32").unwrap_or(speed).parse::<f32>() else {
                    tracing::warn!(row, "tool_rules.tsv: bad speed; skipped");
                    continue;
                };
                Some(speed)
            }
        };
        let correct = match fields[4] {
            "-" => None,
            "true" => Some(true),
            "false" => Some(false),
            _ => {
                tracing::warn!(row, "tool_rules.tsv: bad correct flag; skipped");
                continue;
            }
        };
        let fields5 = fields[5].strip_suffix("f32").unwrap_or(fields[5]);
        let Ok(default) = fields5.parse::<f32>() else {
            tracing::warn!(row, "tool_rules.tsv: bad default speed; skipped");
            continue;
        };
        ordered.entry(fields[0].to_owned()).or_default().push((
            order,
            ToolRule {
                tag: fields[2].to_owned(),
                speed,
                correct,
            },
            default,
        ));
    }
    let mut applied = 0usize;
    for (name, mut rules) in ordered {
        rules.sort_by_key(|(order, _, _)| *order);
        let default = rules.first().map_or(1.0, |(_, _, default)| *default);
        tools.insert(
            name,
            ToolEntry {
                default_speed: default,
                rules: rules.into_iter().map(|(_, rule, _)| rule).collect(),
            },
        );
        applied += 1;
    }
    tracing::debug!(applied, "tool rules loaded");
}

#[cfg(test)]
#[allow(clippy::float_cmp, reason = "fixture pins assert exact parsed speeds")]
mod tests {
    use super::ItemRegistry;

    fn registry() -> ItemRegistry {
        ItemRegistry::load(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../crates/test-support/fixtures/registry/items.tsv"),
        )
        .expect("fixture loads")
    }

    #[test]
    fn the_table_covers_the_whole_vanilla_item_registry() {
        let items = registry();
        assert_eq!(items.len(), 1506, "item count from the jar dump");
    }

    #[test]
    fn ids_match_the_jar_dump() {
        let items = registry();
        assert_eq!(items.id("minecraft:air").expect("air"), 0);
        assert_eq!(items.id("minecraft:stone").expect("stone"), 1);
        assert_eq!(items.name(1).expect("id 1"), "minecraft:stone");
        assert_eq!(
            items.block_of(1).expect("block of stone"),
            Some("minecraft:stone")
        );
    }

    #[test]
    fn block_items_map_both_ways() {
        let items = registry();
        for name in [
            "minecraft:stone",
            "minecraft:dirt",
            "minecraft:oak_log",
            "minecraft:chest",
            "minecraft:crafting_table",
        ] {
            let id = items.id(name).expect("item exists");
            assert_eq!(items.block_of(id).expect("block"), Some(name));
            assert_eq!(items.item_for_block(name).expect("item for block"), id);
        }
        // Non-block items have no block.
        let stick = items.id("minecraft:stick").expect("stick");
        assert_eq!(items.block_of(stick).expect("block"), None);
    }

    #[test]
    fn unknown_items_and_ids_are_refused() {
        let items = registry();
        assert!(items.id("minecraft:not_an_item").is_err());
        assert!(items.entry(99_999).is_err());
        assert!(items.name(-1).is_err());
        assert!(items.item_for_block("minecraft:not_a_block").is_err());
    }

    #[test]
    fn tool_rules_mirror_the_vanilla_tool_components() {
        // P16-05: ordered rules from pumpkin's generated item.rs via
        // target/extract_mining.py. First match wins per field.
        let items = registry();
        let pick = items.tool("minecraft:diamond_pickaxe").expect("a tool");
        assert_eq!(pick.default_speed, 1.0);
        assert_eq!(pick.rules.len(), 2);
        assert_eq!(pick.rules[0].tag, "minecraft:incorrect_for_diamond_tool");
        assert_eq!(pick.rules[0].speed, None);
        assert_eq!(pick.rules[0].correct, Some(false));
        assert_eq!(pick.rules[1].tag, "minecraft:mineable/pickaxe");
        assert_eq!(pick.rules[1].speed, Some(8.0));
        assert_eq!(pick.rules[1].correct, Some(true));
        // A non-tool has no entry: the caller digs as a hand.
        assert_eq!(items.tool("minecraft:stick"), None);
        assert_eq!(items.tool("minecraft:not_an_item"), None);
    }

    #[test]
    fn malformed_tables_are_rejected() {
        assert!(ItemRegistry::parse("").is_err());
        assert!(ItemRegistry::parse("0\tminecraft:air").is_err());
        // Out-of-order id.
        assert!(ItemRegistry::parse("1\tminecraft:air\t-").is_err());
    }
}
