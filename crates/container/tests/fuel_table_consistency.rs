//! The fuel table's doc and its rows must agree, because three times in this review they did not.
//!
//! ## Why a test rather than a comment
//!
//! `container/furnace.rs` documents its fuel table as a Markdown table with an evidence label per row, and puts
//! the same label in each row's `evidence:` field. The design note says why the label is part of the row: "so
//! reordering or adding a row cannot silently attach the wrong evidence to a fuel".
//!
//! **And then the two disagreed.** KD-76: the doc said `derived (9 x coal)` for the coal block, which is not a
//! derivation — nine coals are 14400 and a block is 16000 — and while correcting the table, `evidence:
//! Evidence::Derived` was left in the row it describes. **A fresh doc-versus-code disagreement, created by the
//! commit that was fixing one.**
//!
//! That is the third variation of one mistake in four rounds (KD-70 changed prose to match a wrong constant,
//! KD-71 changed a body and left its doc, KD-76 changed a table and left its row), and **every gate was green each
//! time**, because no gate reads a doc and compares it with the code beneath it.
//!
//! So this one does. It reads the source as text — `include_str!` rather than a hand-copied list, because a test
//! written from the same belief asserts the same wrong thing, which is the whole subject of this review.

/// One row of the doc's Markdown table: the item name and the evidence label.
#[derive(Debug, PartialEq, Eq)]
struct DocRow {
    item: String,
    label: String,
}

/// The rows of the doc table, in order.
fn doc_rows() -> Vec<DocRow> {
    let mut rows = Vec::new();
    let mut started = false;
    for line in include_str!("../src/furnace.rs").lines() {
        let trimmed = line.trim();
        // The file documents a second table (smelting recipes) whose fourth column is an experience value, so
        // this stops at the first non-table line after the fuel table has started.
        if started && !trimmed.starts_with("/// |") {
            break;
        }
        let Some(rest) = trimmed.strip_prefix("/// | `minecraft:") else {
            continue;
        };
        started = true;
        // `coal` | 1600 | 80 | verified |
        let mut cells = rest.split('|').map(str::trim);
        let item = cells
            .next()
            .unwrap_or_default()
            .trim_end_matches(char::from(96))
            .to_owned();
        let _ticks = cells.next();
        let _seconds = cells.next();
        let label = cells.next().unwrap_or_default().to_owned();
        if item.is_empty() || label.is_empty() {
            continue;
        }
        rows.push(DocRow {
            item: format!("minecraft:{item}"),
            label,
        });
    }
    rows
}

/// The item and evidence of each row in the code, in order.
fn code_rows() -> Vec<DocRow> {
    let mut rows = Vec::new();
    let mut item: Option<String> = None;
    for line in include_str!("../src/furnace.rs").lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("item: \"") {
            item = rest.strip_suffix("\",").map(str::to_owned);
        }
        if let Some(rest) = trimmed.strip_prefix("evidence: Evidence::") {
            let variant = rest.trim_end_matches(',').trim();
            if let (Some(name), Some(_)) = (item.take(), Some(variant)) {
                rows.push(DocRow {
                    item: name,
                    label: variant.to_lowercase(),
                });
            }
        }
    }
    rows
}

#[test]
fn the_fuel_tables_doc_and_its_rows_say_the_same_thing() {
    let doc = doc_rows();
    let code = code_rows();
    assert_eq!(
        doc.len(),
        code.len(),
        "the doc table has {} rows and the code has {}: {} vs {}",
        doc.len(),
        code.len(),
        doc.iter()
            .map(|r| r.item.as_str())
            .collect::<Vec<_>>()
            .join(", "),
        code.iter()
            .map(|r| r.item.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    );
    assert!(
        !doc.is_empty(),
        "the doc table parsed as empty, so this test proves nothing"
    );

    for (doc_row, code_row) in doc.iter().zip(&code) {
        assert_eq!(
            doc_row.item, code_row.item,
            "the doc table and the code list different fuels in the same position"
        );
        // The doc's label may carry a parenthetical; the code's field is the variant alone.
        let label = doc_row.label.split_whitespace().next().unwrap_or_default();
        let expected = match label {
            "derived" => "derived",
            "approximation" => "approximation",
            _ => "verified",
        };
        assert_eq!(
            code_row.label, expected,
            "{}: the doc says {:?} and the row says {:?}. This is KD-76: the two are edited as one artifact and \
             treated as two.",
            doc_row.item, doc_row.label, code_row.label
        );
    }
}

#[test]
fn no_row_claims_the_coal_block_is_derived_from_nine_coal() {
    // The instance that was wrong, kept as its own assertion because it is the one a reader would check by
    // arithmetic and reach a different answer.
    // **Inside a table row**, not anywhere in the file: the comment explaining this defect names the phrase in
    // order to say it is wrong, and a check that forbids the string outright fails on the corrected code.
    let offenders: Vec<usize> = include_str!("../src/furnace.rs")
        .lines()
        .enumerate()
        .filter(|(_, line)| {
            let trimmed = line.trim();
            trimmed.starts_with("/// |") && trimmed.contains("derived (9 x coal)")
        })
        .map(|(index, _)| index + 1)
        .collect();
    assert!(
        offenders.is_empty(),
        "table rows calling the coal block a derivation of nine coal, at lines {offenders:?}: nine coals are \
         14400 and a block is 16000"
    );
}
