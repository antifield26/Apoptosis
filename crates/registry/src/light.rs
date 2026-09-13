//! Per-state light properties, read from the jar (`P10-04`).
//!
//! The three values a light engine needs — emission, dampening and whether sky light falls through
//! undiminished — are read from the jar's **own** accessors by `tools/vanilla-probe/LightProbe.java`, which
//! boots the registry the dedicated server boots and calls `getLightEmission()`, `getLightDampening()` and
//! `propagatesSkylightDown()`.
//!
//! Nothing is transcribed from documentation. That matters more here than for most tables: a wrong dampening
//! produces light that is plausible in every direction and correct in none, and there is no way to notice from
//! the values alone.

use mc_core::error::{ServerError, ServerResult};
use std::path::Path;

/// What an unknown state id is treated as: air.
///
/// A state id outside the table means the table and the block registry disagree, which is a build problem
/// rather than a wire one. Air is the safe reading — it neither emits nor blocks — and the alternative,
/// refusing to compute, would take a whole chunk down for one unknown id.
const UNKNOWN: (u8, u8, bool) = (0, 0, true);

/// Per-state light inputs, indexed by block-state id, loaded from `block_light.tsv`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LightTable {
    emission: Vec<u8>,
    dampening: Vec<u8>,
    propagates: Vec<bool>,
}

/// What an unknown state id is treated as: air.
///
/// A state id outside the table means the table and the block registry disagree, which is a build problem
/// rather than a wire one. Air is the safe reading — it neither emits nor blocks — and the alternative
/// (refusing to compute) would take a whole chunk down for one unknown id.
impl LightTable {
    /// Parse the TSV `LightProbe.java` writes.
    ///
    /// Two row kinds: `block <name> <emission> <dampening> <propagates>` for a block whose states all share
    /// one triple, and `state <state id> <emission> <dampening> <propagates>` for one whose states differ.
    /// The block rows carry no state ids, so the size of the table is only known once a state row or the
    /// caller's block registry says so — this takes the state count explicitly.
    ///
    /// # Errors
    ///
    /// [`ServerError::CorruptData`] for a malformed row or an unparsable number.
    pub fn parse(text: &str, state_count: usize) -> ServerResult<Self> {
        let mut table = Self {
            // Defaults are the unknown-state reading until a row says otherwise.
            emission: vec![UNKNOWN.0; state_count],
            dampening: vec![UNKNOWN.1; state_count],
            propagates: vec![UNKNOWN.2; state_count],
        };
        let mut block_rows: Vec<(String, u8, u8, bool)> = Vec::new();
        for (number, line) in text.lines().enumerate() {
            let line = line.trim_end();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let fields: Vec<&str> = line.split('\t').collect();
            let at = number + 1;
            match fields.as_slice() {
                ["block", name, emission, dampening, propagates] => {
                    block_rows.push((
                        (*name).to_owned(),
                        parse_level(emission, at)?,
                        parse_level(dampening, at)?,
                        parse_flag(propagates, at)?,
                    ));
                }
                ["state", id, emission, dampening, propagates] => {
                    let id: usize = id.parse().map_err(|_| {
                        ServerError::CorruptData(format!(
                            "line {at}: state id {id:?} is not a number"
                        ))
                    })?;
                    if id >= state_count {
                        return Err(ServerError::CorruptData(format!(
                            "line {at}: state id {id} is beyond the {state_count} the registry holds"
                        )));
                    }
                    table.emission[id] = parse_level(emission, at)?;
                    table.dampening[id] = parse_level(dampening, at)?;
                    table.propagates[id] = parse_flag(propagates, at)?;
                }
                _ => {
                    return Err(ServerError::CorruptData(format!(
                        "line {at}: expected 4 or 5 tab-separated fields, found {}",
                        fields.len()
                    )));
                }
            }
        }
        Ok(table)
    }

    /// Read the table from a file.
    ///
    /// # Errors
    ///
    /// [`ServerError::Io`] when the file cannot be read, or whatever [`Self::parse`] reports.
    pub fn load(path: &Path, state_count: usize) -> ServerResult<Self> {
        let text = std::fs::read_to_string(path).map_err(|error| {
            ServerError::Operational(format!("cannot read {}: {error}", path.display()))
        })?;
        Self::parse(&text, state_count)
    }

    /// The light this state emits, `0..=15`.
    #[must_use]
    pub fn emission(&self, state: i32) -> u8 {
        Self::at(state, &self.emission, UNKNOWN.0)
    }

    /// How much light this state absorbs passing through.
    #[must_use]
    pub fn dampening(&self, state: i32) -> u8 {
        Self::at(state, &self.dampening, UNKNOWN.1)
    }

    /// Whether sky light falls through this state undiminished.
    #[must_use]
    pub fn propagates_skylight_down(&self, state: i32) -> bool {
        usize::try_from(state)
            .ok()
            .and_then(|index| self.propagates.get(index).copied())
            .unwrap_or(UNKNOWN.2)
    }

    /// How many state ids the table covers.
    #[must_use]
    pub fn len(&self) -> usize {
        self.emission.len()
    }

    /// Whether the table is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.emission.is_empty()
    }

    fn at(state: i32, column: &[u8], fallback: u8) -> u8 {
        usize::try_from(state)
            .ok()
            .and_then(|index| column.get(index).copied())
            .unwrap_or(fallback)
    }
}

fn parse_level(field: &str, at: usize) -> ServerResult<u8> {
    let value: i32 = field
        .parse()
        .map_err(|_| ServerError::CorruptData(format!("line {at}: {field:?} is not a number")))?;
    u8::try_from(value).map_err(|_| {
        ServerError::CorruptData(format!("line {at}: light level {value} is outside 0..=255"))
    })
}

fn parse_flag(field: &str, at: usize) -> ServerResult<bool> {
    match field {
        "1" => Ok(true),
        "0" => Ok(false),
        other => Err(ServerError::CorruptData(format!(
            "line {at}: expected 0 or 1, found {other:?}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::LightTable;

    #[test]
    fn the_table_parses_block_and_state_rows_and_refuses_nonsense() {
        // Inlined rather than shared with the world crate's tests: this pins the *parser*, and a helper
        // that describes a propagation scenario would tie the two together for no gain.
        let table = LightTable::parse(
            "# synthetic\nstate\t0\t0\t0\t1\nstate\t1\t0\t15\t0\nstate\t2\t14\t0\t1\n",
            3,
        )
        .expect("parses");
        assert_eq!(table.len(), 3);
        assert_eq!(table.emission(2), 14);
        assert_eq!(table.dampening(1), 15);
        assert!(table.propagates_skylight_down(0));
        assert!(!table.propagates_skylight_down(1));

        // States outside the table are air-like rather than a panic or a zero that would mean "opaque".
        assert_eq!(table.emission(999), 0);
        assert_eq!(table.dampening(999), 0);
        assert!(table.propagates_skylight_down(999));

        for bad in [
            "state\t0\t0\t0\n",       // too few fields
            "state\tx\t0\t0\t1\n",    // bad id
            "state\t9\t0\t0\t1\n",    // id beyond the state count
            "state\t0\t0\t0\t2\n",    // flag that is neither 0 nor 1
            "state\t0\tzero\t0\t1\n", // bad emission
            "nonsense\t0\t0\t0\t1\n", // unknown row kind
        ] {
            assert!(
                LightTable::parse(bad, 3).is_err(),
                "{bad:?} should be refused rather than silently defaulted"
            );
        }
    }
}
