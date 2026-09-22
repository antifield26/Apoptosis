//! P17-04: observer facing/output and dispenser facing, replayed against a
//! machine-written oracle from a live 26.1.2 server.
//!
//! The oracle is `fixtures/vanilla_observer.tsv`, written by
//! `target/p17_observer_run.py` + `target/p17_observer_read.py`. A dropper
//! preloaded with one cobblestone that is empty after the flip FIRED; one that
//! still holds cobble did NOT. That latch is what makes a 2-tick observer pulse
//! visible in a save file.
//!
//! Contract settled here (the P17-01 named gap "facing/output question"):
//! - observer `facing` names the **watch** side;
//! - the pulse leaves only the **back** (opposite) face;
//! - side faces stay dark;
//! - a block update in front of the face pulses the back;
//! - dispenser `facing` names the output face.

use std::collections::BTreeMap;
use std::path::PathBuf;

/// `(label, x, y, z, facing, fired)` from the oracle.
type Row = (String, i32, i32, i32, String, bool);

fn oracle() -> Vec<Row> {
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/vanilla_observer.tsv");
    let text = std::fs::read_to_string(&path).expect("the oracle fixture reads");
    let mut rows = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let parts: Vec<&str> = line.split('\t').collect();
        assert_eq!(parts.len(), 6, "oracle row must be 6 columns: {line}");
        rows.push((
            parts[0].to_owned(),
            parts[1].parse().expect("x"),
            parts[2].parse().expect("y"),
            parts[3].parse().expect("z"),
            parts[4].to_owned(),
            parts[5] == "1",
        ));
    }
    assert!(!rows.is_empty(), "the oracle must not be empty");
    rows
}

/// Unit offset of a `facing` property value (the watch direction).
fn facing_offset(facing: &str) -> (i32, i32, i32) {
    match facing {
        "north" => (0, 0, -1),
        "south" => (0, 0, 1),
        "west" => (-1, 0, 0),
        "east" => (1, 0, 0),
        "up" => (0, 1, 0),
        "down" => (0, -1, 0),
        other => panic!("unknown facing {other:?}"),
    }
}

/// Whether a dropper at `pos` sits on the back face of an observer at
/// `observer` whose `facing` names its watch side.
fn on_observer_back(observer: (i32, i32, i32), facing: &str, pos: (i32, i32, i32)) -> bool {
    let (fx, fy, fz) = facing_offset(facing);
    let back = (observer.0 - fx, observer.1 - fy, observer.2 - fz);
    pos == back
}

/// Whether a dropper sits on a side face of the observer (neither face nor back).
fn on_observer_side(observer: (i32, i32, i32), facing: &str, pos: (i32, i32, i32)) -> bool {
    let (dx, dy, dz) = (pos.0 - observer.0, pos.1 - observer.1, pos.2 - observer.2);
    if (dx, dy, dz) == (0, 0, 0) {
        return false;
    }
    let (fx, fy, fz) = facing_offset(facing);
    // Face or back: along the facing axis exactly one step.
    if (dx, dy, dz) == (fx, fy, fz) || (dx, dy, dz) == (-fx, -fy, -fz) {
        return false;
    }
    dx.abs() + dy.abs() + dz.abs() == 1
}

/// Observer positions in the oracle topologies (from the run script).
fn observers() -> BTreeMap<&'static str, ((i32, i32, i32), &'static str)> {
    BTreeMap::from([
        ("O-N", ((0, 101, 0), "north")),
        ("O-N-FACE", ((4, 101, 0), "north")),
        ("O-E", ((8, 101, 0), "east")),
    ])
}

/// The measured contract, as assertions over the oracle itself.
///
/// Every row that fired must sit on some observer's back face. Every row that
/// did not fire must sit on a side face (or, for the two dispensers, be a
/// dispenser whose `facing` is its output — those fire when their lever
/// powers them).
#[test]
fn the_oracle_shows_facing_is_the_watch_side_and_output_is_the_back() {
    let rows = oracle();
    let obs = observers();
    let mut back_fired = 0;
    let mut side_quiet = 0;
    let mut dispenser_fired = 0;

    for (label, x, y, z, _facing, fired) in &rows {
        let pos = (*x, *y, *z);
        if label.starts_with("D-") {
            // Dispenser under test: `facing` is the output face. Both were
            // lever-powered and both emptied — the latch fired once each.
            assert!(
                *fired,
                "{label} must have emptied (dispenser facing is output)"
            );
            dispenser_fired += 1;
            continue;
        }
        // Find which observer this dropper belongs to by prefix.
        let prefix = label.split('.').next().expect("label.prefix");
        let ((ox, oy, oz), ofacing) = obs
            .get(prefix)
            .unwrap_or_else(|| panic!("unknown observer prefix {prefix}"));
        if on_observer_back((*ox, *oy, *oz), ofacing, pos) {
            assert!(
                *fired,
                "{label} sits on the back of {prefix} facing {ofacing} and must have fired"
            );
            back_fired += 1;
        } else if on_observer_side((*ox, *oy, *oz), ofacing, pos) {
            assert!(
                !*fired,
                "{label} sits on a side of {prefix} facing {ofacing} and must not have fired"
            );
            side_quiet += 1;
        } else if label.contains("dropper_back") {
            // Named back but not on the geometric back — the facing reading is
            // wrong. That is exactly the P17-04 question, so fail loudly.
            panic!(
                "{label} is named back but is not on the back of {prefix} facing {ofacing}: \
                 this would mean `facing` is the output side, not the watch side"
            );
        }
    }

    assert_eq!(back_fired, 3, "three back-face droppers must have fired");
    assert_eq!(
        side_quiet, 3,
        "three side-face droppers must have stayed full"
    );
    assert_eq!(dispenser_fired, 2, "both dispensers must have emptied");
}

/// A face-side block update pulses the back: O-N-FACE's back dropper fired
/// even though its watched source was the dropper *in front* of the face,
/// not a lever.
#[test]
fn a_face_side_update_pulses_the_back() {
    let rows: BTreeMap<String, bool> = oracle()
        .into_iter()
        .map(|(label, .., fired)| (label, fired))
        .collect();
    assert!(
        rows.get("O-N-FACE.dropper_back").copied().unwrap_or(false),
        "the back dropper must fire when the face sees an update"
    );
}

/// Facing east: the west dropper (back) fires and the south dropper (side)
/// does not. Facing is the watch side, not the output side.
#[test]
fn facing_east_watches_east_and_outputs_west() {
    let rows: BTreeMap<String, bool> = oracle()
        .into_iter()
        .map(|(label, .., fired)| (label, fired))
        .collect();
    assert_eq!(
        rows.get("O-E.dropper_back"),
        Some(&true),
        "west = back = output"
    );
    assert_eq!(
        rows.get("O-E.dropper_side"),
        Some(&false),
        "south = side = dark"
    );
}
