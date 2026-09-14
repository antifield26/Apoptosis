//! The entity-type registry, extracted from the jar rather than retyped.
//!
//! # Why this table exists, and why it is extracted
//!
//! `add_entity` carries an **entity type id**, and the client resolves that number against the registry this
//! server sends it. It is the same shape as the two defects this project has already paid for:
//!
//! * a block's default state taken to be its lowest id, wrong for **642 of 1168** blocks, which put every log on
//!   its side and water inside every leaf (KD-56);
//! * `PLAINS_BIOME_ID = 0` where id 0 in the client's registry is `minecraft:badlands`, which painted every chunk
//!   of every world red sand (KD-65).
//!
//! The rule those produced: **a number sent to a client is a claim about a registry the client owns**, and it
//! needs a jar extraction, a capture, or an assertion naming the registry — never a recollection.
//!
//! So the table comes from `tools/vanilla-probe/EntityTypeProbe.java`, in the pipeline `blocks.tsv` and
//! `items.tsv` went through, and it is checked in as `entity_types.tsv`.
//!
//! # What it says, which is not what anyone would guess
//!
//! ```text
//! 0    minecraft:acacia_boat
//! 30   minecraft:cow
//! 71   minecraft:item          <- the drop P10-08 has to make visible
//! 150  minecraft:zombie
//! 155  minecraft:player
//! ```
//!
//! **157 registrations, and id 0 is a boat.** The registry is alphabetical, exactly as the biome registry is,
//! which is why the assumption "the first entry is the ordinary one" survived so long: it is true often enough
//! elsewhere to be believed.

use mc_core::error::{ServerError, ServerResult};
use std::collections::HashMap;
use std::path::Path;

/// `minecraft:player`, the type the join path names.
pub const PLAYER: &str = "minecraft:player";

/// `minecraft:item`, the type a dropped stack is carried by.
pub const ITEM: &str = "minecraft:item";

/// Entity-type lookup table.
#[derive(Debug, Clone)]
pub struct EntityTypeRegistry {
    by_name: HashMap<String, i32>,
    names: Vec<String>,
}

impl EntityTypeRegistry {
    /// Parse an `entity_types.tsv` table.
    ///
    /// # Errors
    ///
    /// [`ServerError::Operational`] when the file cannot be read, [`ServerError::CorruptData`] when a row is
    /// malformed, the table is empty, or ids are not contiguous from 0.
    pub fn load(path: &Path) -> ServerResult<Self> {
        let text = std::fs::read_to_string(path).map_err(|e| {
            ServerError::Operational(format!("cannot read {}: {e}", path.display()))
        })?;
        Self::parse(&text)
    }

    /// Parse a table from memory.
    ///
    /// # Errors
    ///
    /// As for [`EntityTypeRegistry::load`].
    pub fn parse(text: &str) -> ServerResult<Self> {
        let mut names: Vec<String> = Vec::new();
        for (number, line) in text.lines().enumerate() {
            if line.starts_with('#') || line.trim().is_empty() {
                continue;
            }
            let row = number + 1;
            let mut fields = line.split('\t');
            let (Some(id), Some(name)) = (fields.next(), fields.next()) else {
                return Err(ServerError::CorruptData(format!(
                    "entity_types.tsv line {row}: expected 2 tab-separated fields"
                )));
            };
            let id: i32 = id.parse().map_err(|_| {
                ServerError::CorruptData(format!("line {row}: bad entity type id {id:?}"))
            })?;
            // Contiguity is the check that makes the table usable as a lookup at all: the id *is* the index, so a
            // gap or a reorder would silently return a neighbouring entity's name.
            if id as usize != names.len() {
                return Err(ServerError::CorruptData(format!(
                    "line {row}: entity type id {id} is out of order (expected {})",
                    names.len()
                )));
            }
            names.push(name.to_owned());
        }
        if names.is_empty() {
            return Err(ServerError::CorruptData(
                "entity type table is empty".to_owned(),
            ));
        }
        let by_name = names
            .iter()
            .enumerate()
            .map(|(id, name)| (name.clone(), id as i32))
            .collect();
        Ok(Self { by_name, names })
    }

    /// Number of registered entity types.
    #[must_use]
    pub fn len(&self) -> usize {
        self.names.len()
    }

    /// Whether the table is empty. Never true for a parsed table.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }

    /// The wire id of an entity type by name.
    ///
    /// # Errors
    ///
    /// [`ServerError::CorruptData`] when the name is not registered. Refusing is deliberate: substituting a
    /// default would send the client a different entity than the caller asked for, which is what KD-65 did with
    /// every chunk it sent.
    pub fn id(&self, name: &str) -> ServerResult<i32> {
        self.by_name
            .get(name)
            .copied()
            .ok_or_else(|| ServerError::CorruptData(format!("unknown entity type {name:?}")))
    }

    /// The name of an entity type by wire id.
    ///
    /// # Errors
    ///
    /// [`ServerError::CorruptData`] when no type has that id.
    pub fn name(&self, id: i32) -> ServerResult<&str> {
        usize::try_from(id)
            .ok()
            .and_then(|index| self.names.get(index))
            .map(String::as_str)
            .ok_or_else(|| ServerError::CorruptData(format!("no entity type with id {id}")))
    }

    /// Every registered name, in id order.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.names.iter().map(String::as_str)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> EntityTypeRegistry {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        EntityTypeRegistry::load(&root.join(crate::FIXTURE_DIR).join("entity_types.tsv"))
            .expect("the entity type table loads")
    }

    #[test]
    fn the_table_is_the_shape_the_probe_wrote() {
        let registry = fixture();
        // 157 registrations, the count `EntityTypeProbe` printed. Asserted rather than assumed: a table that
        // silently lost rows would still resolve every name it kept, which is the failure mode a lookup cannot
        // detect from the inside.
        assert_eq!(
            registry.len(),
            157,
            "the entity type table has changed size"
        );
        assert!(!registry.is_empty());
    }

    #[test]
    fn the_named_types_this_phase_needs_are_where_the_jar_puts_them() {
        let registry = fixture();
        // **The numbers are the point.** `EntityTypeProbe` measured these from the 26.1.2 jar, and the registry
        // is alphabetical, so id 0 is a boat and the two types this phase sends are nowhere near it.
        assert_eq!(registry.id(PLAYER).expect("player"), 155);
        assert_eq!(registry.id(ITEM).expect("item"), 71);
        assert_eq!(
            registry.name(0).expect("id 0"),
            "minecraft:acacia_boat",
            "id 0 is a boat, which is why nothing may assume the first entry is the ordinary one"
        );
        assert_eq!(registry.name(150).expect("150"), "minecraft:zombie");
        assert_eq!(registry.name(30).expect("30"), "minecraft:cow");
    }

    #[test]
    fn a_name_and_an_id_round_trip() {
        let registry = fixture();
        for id in 0..i32::try_from(registry.len()).expect("fits") {
            let name = registry.name(id).expect("in range").to_owned();
            assert_eq!(registry.id(&name).expect("the same name"), id);
        }
    }

    #[test]
    fn an_unknown_name_or_id_is_refused_rather_than_defaulted() {
        let registry = fixture();
        assert!(registry.id("minecraft:not_an_entity").is_err());
        assert!(registry.name(157).is_err());
        assert!(registry.name(-1).is_err());
    }

    #[test]
    fn ids_must_be_contiguous_from_zero() {
        // The id *is* the index, so a gap would silently resolve one entity's name for another's id. That is
        // what the parser refuses, and this is the test that says so.
        let gapped = "# header\n0\tminecraft:acacia_boat\n2\tminecraft:allay\n";
        assert!(EntityTypeRegistry::parse(gapped).is_err());
        let reordered = "# header\n1\tminecraft:allay\n0\tminecraft:acacia_boat\n";
        assert!(EntityTypeRegistry::parse(reordered).is_err());
        assert!(EntityTypeRegistry::parse("").is_err());
        // One field and no tab. The parser checks the **shape** of a row, not whether a name looks like an
        // identifier — which is `items.rs`'s contract too. The first version of this test asserted the naming
        // rule and failed on a row that was well formed: a check whose scope was wider than the code's.
        assert!(EntityTypeRegistry::parse("0\n").is_err());
        assert!(
            EntityTypeRegistry::parse("0\t\n").is_ok(),
            "an empty name is a shape the parser allows"
        );
    }
}
