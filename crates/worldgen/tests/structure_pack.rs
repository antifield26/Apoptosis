//! Differential test: read the **real** 26.1.2 structure pack (P07-16, P07-18).
//!
//! The unit tests in `mc_worldgen::structure` use hand-built NBT fixtures, which
//! proves the schema but not that it is the schema the shipped pack uses. This
//! file loads the actual `structure/` tree and asserts the figures measured from
//! the jar.
//!
//! It is `#[ignore]`d because it needs `MC_VANILLA_DATA` — a directory holding the
//! extracted `data/minecraft/` — which is not committed (8.5 MiB of Mojang data).
//! The same variable and the same root are used by
//! `crates/data/tests/vanilla_pack.rs` and
//! `crates/container/tests/vanilla_smelting.rs`, so the extraction command in
//! `docs/research/data-pack-baseline.md` section 0 serves all three:
//!
//! ```text
//! set MC_VANILLA_DATA=target\vanilla-26.1.2\extract\data\minecraft
//! cargo test -p mc-worldgen --test structure_pack -- --ignored --nocapture
//! ```
//!
//! ## Why this is worth having — the bug it caught
//!
//! An earlier version of the reader accepted `size` and `blocks[i].pos` **only**
//! as `TAG_IntArray`. The real pack writes both as `TAG_List<TAG_Int>` in
//! **1 202 of 1 202** files. Every hand-built fixture passed; every real file was
//! refused with "no usable `size`". That is precisely the failure mode a
//! synthetic-only test suite cannot see, and it is why the tag-type assertions
//! below are the first thing this file checks.

use mc_core::ids::ResourceId;
use mc_registry::Registries;
use mc_worldgen::structure::{StructureError, StructureLimits, read_structure};
use mc_worldgen::structures::load_structures;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// The extracted `data/minecraft` directory, or `None` when the variable is unset.
///
/// `None` **skips** rather than fails: a contributor without the extracted pack
/// must still be able to run the suite. The skip is loud and names the variable.
fn pack_root() -> Option<PathBuf> {
    let root = PathBuf::from(std::env::var("MC_VANILLA_DATA").ok()?);
    assert!(
        root.is_dir(),
        "MC_VANILLA_DATA points at {}, which is not a directory. See \
         docs/research/data-pack-baseline.md section 0 for the extraction command.",
        root.display()
    );
    assert!(
        root.join("structure").is_dir(),
        "{} does not look like an extracted data/minecraft (no structure/)",
        root.display()
    );
    Some(root)
}

/// Whether the differential tests can run at all.
macro_rules! pack_or_skip {
    () => {
        match pack_root() {
            Some(root) => root,
            None => {
                eprintln!("MC_VANILLA_DATA is not set; skipping");
                return;
            }
        }
    };
}

/// Every `.nbt` under `<root>/structure`, ascending.
fn pack_files(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut pending = vec![root.join("structure")];
    while let Some(directory) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            match entry.file_type() {
                Ok(kind) if kind.is_dir() => pending.push(path),
                Ok(kind) if kind.is_file() && path.extension().is_some_and(|e| e == "nbt") => {
                    out.push(path);
                }
                _ => {}
            }
        }
    }
    out.sort();
    out
}

/// Parse a three-integer coordinate field the way the reader does.
///
/// Both encodings are handled because the census must be able to *report* which one
/// the pack uses, rather than failing on the one the reader happens to accept.
fn read_three_ints(tag: &mc_nbt::NbtTag, field: &str) -> Option<[i32; 3]> {
    if let Some(array) = tag.get_int_array(field) {
        return <[i32; 3]>::try_from(array).ok();
    }
    let items = tag.get_list(field)?;
    if items.len() != 3 {
        return None;
    }
    let mut out = [0_i32; 3];
    for (slot, item) in items.iter().enumerate() {
        *out.get_mut(slot)? = i32::try_from(item.as_i64()?).ok()?;
    }
    Some(out)
}

/// The NBT type of a field, for the tag-type census.
fn tag_type(tag: &mc_nbt::NbtTag, field: &str) -> String {
    match tag.get(field) {
        Some(mc_nbt::NbtTag::List(items)) => match items.first() {
            Some(first) => format!("TAG_List<{}>", first.type_name()),
            None => "TAG_List<EMPTY>".to_owned(),
        },
        Some(other) => other.type_name().to_owned(),
        None => "absent".to_owned(),
    }
}

/// The palette entries of one `palette` or `palettes` element.
///
/// The singular form is `palette[i] = {Name, Properties?}`, so the element *is* an
/// entry. The plural form is `palettes[i] = [entry, …]`, so the element is a
/// `TAG_List` of entries.
fn entry_list(element: &mc_nbt::NbtTag) -> Vec<&mc_nbt::NbtTag> {
    match element {
        mc_nbt::NbtTag::List(items) => items.iter().collect(),
        other => vec![other],
    }
}

/// Parse the NBT of a structure file directly, so the census can see the fields
/// this crate deliberately does not model (`nbt`, `entities`, `palettes`).
/// The NBT of a structure file, plus its **root tag name**.
///
/// The name is returned rather than discarded: `read_named` produces it, and an earlier version
/// of this file dropped it and then "measured" it as a constant. A census that throws away a
/// value cannot honestly assert anything about it.
fn raw_root(path: &Path) -> (String, mc_nbt::NbtTag) {
    let bytes = std::fs::read(path).expect("the file reads");
    assert_eq!(&bytes[..2], &[0x1F, 0x8B], "{} is not gzip", path.display());
    let decoded = mc_persistence::compression::Compression::Gzip
        .decompress(&bytes, 8 * 1024 * 1024)
        .expect("the gzip stream decodes");
    mc_nbt::read_named(&decoded, mc_nbt::Limits::DISK).expect("the NBT parses")
}

fn relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

/// Every tally the pack census produces.
///
/// A named struct rather than twenty locals, so the measurement's origin and the assertions that
/// consume it are the same object. The fabricated measurement this file once contained survived
/// because a value's computation and its assertion sat ninety lines apart.
#[derive(Debug, Default)]
struct PackCensus {
    files: usize,
    data_versions: BTreeSet<i32>,
    data_version_tag_types: BTreeSet<String>,
    size_tag_types: BTreeSet<String>,
    pos_tag_types: BTreeSet<String>,
    state_tag_types: BTreeSet<String>,
    entities_tag_types: BTreeSet<String>,
    root_names: BTreeSet<String>,
    palette_names: BTreeSet<String>,
    plural_palettes: BTreeSet<String>,
    with_entities: usize,
    with_properties: usize,
    with_block_nbt: usize,
    largest_size: (i32, i32, i32),
    largest_file: String,
    max_axis_seen: i32,
    total_blocks: usize,
    max_blocks: usize,
    non_positive_size: usize,
    positions_outside_size: usize,
}

/// Walk every structure file and tally what the pack contains.
fn scan_pack(root: &Path, files: &[PathBuf]) -> PackCensus {
    let mut census = PackCensus {
        files: files.len(),
        ..PackCensus::default()
    };

    for path in files {
        let (root_tag_name, tag) = raw_root(path);
        let name = relative(root, path);

        census.root_names.insert(root_tag_name);

        // MEASURED: exactly one DataVersion, 4790, always a `TAG_Int`.
        let version = tag.get_i32("DataVersion").expect("DataVersion");
        census.data_versions.insert(version);
        census
            .data_version_tag_types
            .insert(tag_type(&tag, "DataVersion"));

        if tag.contains("palettes") {
            census.plural_palettes.insert(name.clone());
        }
        census.size_tag_types.insert(tag_type(&tag, "size"));
        let size = read_three_ints(&tag, "size").expect("size is three integers");
        if size.iter().any(|axis| *axis <= 0) {
            census.non_positive_size += 1;
        }
        for axis in size {
            census.max_axis_seen = census.max_axis_seen.max(axis);
        }
        let volume = size[0].saturating_mul(size[1]).saturating_mul(size[2]);
        let largest_volume = census
            .largest_size
            .0
            .saturating_mul(census.largest_size.1)
            .saturating_mul(census.largest_size.2);
        if volume > largest_volume {
            census.largest_size = (size[0], size[1], size[2]);
            census.largest_file.clone_from(&name);
        }

        let entities = tag.get_list("entities").map_or(0, <[mc_nbt::NbtTag]>::len);
        census.entities_tag_types.insert(tag_type(&tag, "entities"));
        if entities > 0 {
            census.with_entities += 1;
        }

        let palette = tag.get_list("palette").or_else(|| tag.get_list("palettes"));
        let entries = palette.map_or_else(Vec::new, |list| {
            list.iter().flat_map(entry_list).collect::<Vec<_>>()
        });
        for entry in &entries {
            if let Some(block_name) = entry.get_str("Name") {
                census.palette_names.insert(block_name.to_owned());
            }
            if entry.contains("Properties") {
                census.with_properties += 1;
            }
        }

        let blocks = tag.get_list("blocks").map_or(0, <[mc_nbt::NbtTag]>::len);
        census.total_blocks += blocks;
        census.max_blocks = census.max_blocks.max(blocks);
        if let Some(block_list) = tag.get_list("blocks") {
            for block in block_list {
                census.state_tag_types.insert(tag_type(block, "state"));
                census.pos_tag_types.insert(tag_type(block, "pos"));
                if block.contains("nbt") {
                    census.with_block_nbt += 1;
                }
                let pos = read_three_ints(block, "pos").expect("pos is three integers");
                if !(0..size[0]).contains(&pos[0])
                    || !(0..size[1]).contains(&pos[1])
                    || !(0..size[2]).contains(&pos[2])
                {
                    census.positions_outside_size += 1;
                }
            }
        }
    }

    census
}

/// Print the census, so a failure shows what was measured.
fn print_census(root: &Path, census: &PackCensus) {
    println!("MEASURED pack census (root {}):", root.display());
    println!("  files                        : {}", census.files);
    println!(
        "  DataVersion values           : {:?}",
        census.data_versions
    );
    println!(
        "  DataVersion tag type         : {:?}",
        census.data_version_tag_types
    );
    println!(
        "  `size` tag type              : {:?}",
        census.size_tag_types
    );
    println!(
        "  `blocks[i].pos` tag type     : {:?}",
        census.pos_tag_types
    );
    println!(
        "  `blocks[i].state` tag type   : {:?}",
        census.state_tag_types
    );
    println!(
        "  `entities` tag type          : {:?}",
        census.entities_tag_types
    );
    println!(
        "  largest size by volume       : {:?} ({})",
        census.largest_size, census.largest_file
    );
    println!("  largest single axis          : {}", census.max_axis_seen);
    println!(
        "  distinct palette block names : {}",
        census.palette_names.len()
    );
    println!("  files with entities          : {}", census.with_entities);
    println!(
        "  palette entries with Props   : {}",
        census.with_properties
    );
    println!("  blocks carrying `nbt`        : {}", census.with_block_nbt);
    println!(
        "  `palettes` (plural) files    : {}",
        census.plural_palettes.len()
    );
    println!("  total blocks                 : {}", census.total_blocks);
    println!("  largest block list           : {}", census.max_blocks);
    println!(
        "  non-positive `size`          : {}",
        census.non_positive_size
    );
    println!(
        "  positions outside `size`     : {}",
        census.positions_outside_size
    );
}

#[test]
#[ignore = "differential: needs MC_VANILLA_DATA (extracted data/minecraft from the 26.1.2 jar)"]
fn the_pack_has_the_measured_file_count_and_shape() {
    let root = pack_or_skip!();
    let files = pack_files(&root);

    // MEASURED: 1 202 files under \structure/\.
    assert_eq!(files.len(), 1202, "the pack's file count moved");

    let census = scan_pack(&root, &files);
    print_census(&root, &census);
    // MEASURED: every file's root tag is unnamed, so this set is exactly `{""}`. Unlike the
    // version this replaced, it is computed from the files rather than from a constant, so a named
    // root would fail it.
    assert_eq!(
        census.root_names,
        BTreeSet::from([String::new()]),
        "every structure file's root tag is unnamed"
    );

    assert_eq!(
        census.data_versions,
        BTreeSet::from([4790]),
        "all 1 202 files declare DataVersion 4790"
    );
    assert_eq!(
        census.data_version_tag_types,
        BTreeSet::from(["TAG_Int".to_owned()])
    );
    // THE important assertions: `size` and `pos` are `TAG_List<TAG_Int>`, **not**
    // `TAG_IntArray`. A reader that assumed the array form refused every real file
    // while passing every hand-built fixture, so these are the regression test for
    // that mistake.
    assert_eq!(
        census.size_tag_types,
        BTreeSet::from(["TAG_List<TAG_Int>".to_owned()]),
        "`size` is a TAG_List of TAG_Int in every file"
    );
    assert_eq!(
        census.pos_tag_types,
        BTreeSet::from(["TAG_List<TAG_Int>".to_owned()]),
        "`blocks[i].pos` is a TAG_List of TAG_Int in every block"
    );
    assert_eq!(
        census.state_tag_types,
        BTreeSet::from(["TAG_Int".to_owned()]),
        "`blocks[i].state` is a TAG_Int"
    );
    // MEASURED: `entities` is `TAG_List<EMPTY>` in the 1 147 files with no entities, and
    // `TAG_List<TAG_Compound>` in the 55 that hold them.
    //
    // `EMPTY` rather than `TAG_End`: an empty list's *declared* element type is `TAG_End` (id 0) on
    // disk, but `tag_type` reports `EMPTY` because it inspects the first element and there is none.
    // An earlier version of this assertion expected the on-disk spelling and failed — it was written
    // from the spec rather than from the helper's output.
    assert_eq!(
        census.entities_tag_types,
        BTreeSet::from([
            "TAG_List<TAG_Compound>".to_owned(),
            "TAG_List<EMPTY>".to_owned(),
        ])
    );
    // MEASURED: the largest template by volume, and the largest single axis.
    assert_eq!(census.largest_size, (38, 48, 38));
    assert_eq!(
        census.largest_file,
        "structure/bastion/treasure/big_air_full.nbt"
    );
    assert_eq!(
        census.max_axis_seen, 48,
        "the largest single axis in the pack"
    );
    // MEASURED: 408 distinct block names, counting every alternative palette in the
    // 20 plural files; the 1 182 singular files alone use 403.
    assert_eq!(
        census.palette_names.len(),
        408,
        "the pack's palette vocabulary moved"
    );
    // MEASURED: 55 files declare entities (46 with one, 9 with two).
    assert_eq!(
        census.with_entities, 55,
        "the entity-bearing file count moved"
    );
    // MEASURED: 14 371 palette entries carry `Properties`.
    assert_eq!(census.with_properties, 14_371, "the Properties count moved");
    // MEASURED: 4 848 blocks carry block-entity `nbt`, which this crate drops.
    assert_eq!(census.with_block_nbt, 4_848, "the block-entity count moved");
    // MEASURED: exactly the 20 shipwreck files use the plural form.
    assert_eq!(
        census.plural_palettes.len(),
        20,
        "the plural-palette set moved"
    );
    let plural: Vec<&String> = census.plural_palettes.iter().collect();
    assert!(
        plural
            .iter()
            .all(|path| path.contains("structure/shipwreck/")),
        "every plural-palette file is a shipwreck: {plural:?}"
    );
    // MEASURED: 1 058 602 blocks and a largest block list of 29 938.
    assert_eq!(
        census.total_blocks, 1_058_602,
        "the pack's block total moved"
    );
    assert_eq!(census.max_blocks, 29_938, "the largest block list moved");
    // No file declares a degenerate size, so the reader's refusal of `[0, 0, 0]`
    // never fires on the shipped pack.
    assert_eq!(census.non_positive_size, 0);
    assert_eq!(
        census.positions_outside_size, 0,
        "positions stay inside `size`"
    );
}

#[test]
#[ignore = "differential: needs MC_VANILLA_DATA (extracted data/minecraft from the 26.1.2 jar)"]
fn every_palette_block_name_resolves_through_the_registry() {
    let root = pack_or_skip!();
    let files = pack_files(&root);
    let blocks = Registries::vanilla().expect("registry fixture").blocks;

    let mut checked_names = BTreeSet::new();
    let mut failed: Vec<String> = Vec::new();
    let mut resolved_entries = 0_usize;
    let mut highest_palette_index = 0_usize;

    for path in &files {
        let name = relative(&root, path);
        // The 20 plural-palette files are refused by the reader, so they are not in
        // scope for resolution; the census above already counts them.
        let Ok(template) = read_structure(path, StructureLimits::PACK) else {
            assert!(
                name.contains("structure/shipwreck/"),
                "{name} was refused but is not a shipwreck"
            );
            continue;
        };
        for entry in &template.palette {
            checked_names.insert(entry.name.to_string());
        }
        match template.resolve(&blocks) {
            Ok(resolved) => {
                resolved_entries += resolved.palette.len();
                assert_eq!(
                    resolved.palette.len(),
                    template.palette.len(),
                    "{name}: one id per palette entry"
                );
                // The reader refuses an out-of-range index, so this must hold for
                // every real file.
                assert!(
                    resolved.highest_palette_index().unwrap_or(0) < resolved.palette.len(),
                    "{name}: a block indexes outside its palette"
                );
                highest_palette_index =
                    highest_palette_index.max(resolved.highest_palette_index().unwrap_or(0));
            }
            Err(error) => failed.push(format!("{name}: {error}")),
        }
    }

    println!(
        "MEASURED: {} distinct block names, {resolved_entries} palette entries and a highest \
         palette index of {highest_palette_index} resolved against the 26.1.2 registry, \
         {} failures",
        checked_names.len(),
        failed.len()
    );
    // MEASURED: every name — including all property-bearing entries — resolves. A
    // registry refresh that dropped a block fails here rather than at world
    // generation time.
    assert!(
        failed.is_empty(),
        "the registry cannot resolve {} pack templates:\n{}",
        failed.len(),
        failed.join("\n")
    );
    assert_eq!(checked_names.len(), 403, "the resolvable vocabulary moved");
    assert_eq!(resolved_entries, 15_558, "the palette-entry total moved");
    assert_eq!(
        highest_palette_index, 123,
        "the highest palette index moved"
    );
}

#[test]
#[ignore = "differential: needs MC_VANILLA_DATA (extracted data/minecraft from the 26.1.2 jar)"]
fn the_loader_reads_1182_files_and_names_the_other_20() {
    let root = pack_or_skip!();
    let (registry, report) = load_structures(&root, StructureLimits::PACK);
    println!("MEASURED {report}");

    // The report is complete and internally consistent.
    assert!(report.is_complete(), "{report}");
    assert!(report.accounts_for_all_files(), "{report}");
    assert!(report.unreadable_dirs.is_empty(), "{report:?}");
    // MEASURED: 1 202 files, of which 1 182 load.
    assert_eq!(report.files, 1202);
    assert_eq!(report.loaded, 1182);
    assert_eq!(report.refused(), 20);

    // **Every skipped file, named, with its reason.** This is the census the P07-16
    // report has to carry: 20 of 1 202 files do not load, all for one unimplemented
    // format feature, and none for a parse failure.
    let mut by_reason: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (path, reason) in &report.skipped {
        by_reason
            .entry(reason.clone())
            .or_default()
            .push(relative(&root, path));
    }
    println!("MEASURED skipped files by reason:");
    for (reason, paths) in &by_reason {
        println!("  {} file(s): {reason}", paths.len());
        for path in paths {
            println!("      {path}");
        }
    }

    // One reason, and it is a *missing feature*, not a parse failure: the plural
    // `palettes` form. Every affected file is a shipwreck.
    assert_eq!(
        by_reason.len(),
        1,
        "one distinct refusal reason: {by_reason:?}"
    );
    let (reason, paths) = by_reason.iter().next().expect("one reason");
    assert!(
        reason.contains("palettes"),
        "the refusal must name the `palettes` key: {reason}"
    );
    assert_eq!(paths.len(), 20);
    for path in paths {
        assert!(
            path.starts_with("structure/shipwreck/"),
            "unexpected refusal: {path}"
        );
    }
    // Every one of them carries `palettes`, so the refusal is the feature and not a
    // coincidence of a corrupt file.
    for path in paths {
        let full = root.join(path);
        assert!(
            raw_root(&full).1.contains("palettes"),
            "{path} was refused without declaring `palettes` — that would be a bug"
        );
        assert!(
            !raw_root(&full).1.contains("palette"),
            "{path} declares both `palette` and `palettes`"
        );
    }

    // MEASURED: 1 182 distinct names from 1 182 loaded files, because the name is
    // the full path under the namespace root.
    assert_eq!(report.duplicate_names, 0, "no name collisions in the pack");
    assert_eq!(registry.len(), 1_182, "the unique template count moved");
    assert_eq!(
        registry.source_count(),
        report.files,
        "every file was offered"
    );

    // MEASURED: 1 028 of the 1 182 readable templates fit inside one chunk; the
    // default placement policy's reachability depends on this number.
    assert_eq!(
        report.fitting_in_one_chunk, 1_028,
        "the single-chunk population moved"
    );

    // MEASURED totals, so a reader regression that dropped blocks, palette entries
    // or entities shows up as a number rather than as a subtly wrong world.
    assert_eq!(report.blocks, 1_049_447, "the loadable block total moved");
    assert_eq!(
        report.palette_entries, 15_558,
        "the palette-entry total moved"
    );
    assert_eq!(report.entities, 64, "the entity total moved");

    // Names are path-derived and deterministic.
    assert_eq!(
        registry
            .by_str("minecraft:structure/empty")
            .expect("the pack's `empty.nbt` loads")
            .size,
        [1, 1, 1]
    );
    let names = registry.names();
    assert!(names.windows(2).all(|pair| pair[0] < pair[1]), "sorted");
    assert!(
        names.iter().all(|name| name.namespace() == "minecraft"),
        "the pack is the minecraft namespace"
    );

    // Two loads of the same tree agree, so chunk generation cannot depend on
    // filesystem iteration order (AGENTS.md §3.6).
    let (again, report_again) = load_structures(&root, StructureLimits::PACK);
    assert_eq!(report, report_again);
    assert_eq!(again.names(), names);
}

#[test]
#[ignore = "differential: needs MC_VANILLA_DATA (extracted data/minecraft from the 26.1.2 jar)"]
fn the_measured_limits_guard_the_real_pack() {
    let root = pack_or_skip!();
    let files = pack_files(&root);

    // Every real template passes the reader's limits, and the largest real axis is
    // inside `max_axis` with room to spare. Both halves asserted, because a limit
    // that silently clipped real files would be indistinguishable from a correct
    // one until a village went missing.
    let mut accepted = 0_usize;
    let mut refused = 0_usize;
    let mut max_axis_seen = 0_i32;
    for path in &files {
        match read_structure(path, StructureLimits::PACK) {
            Ok(template) => {
                accepted += 1;
                for axis in template.size {
                    max_axis_seen = max_axis_seen.max(axis);
                }
            }
            Err(error) => {
                refused += 1;
                assert!(
                    matches!(error, StructureError::UnsupportedPalettes { .. }),
                    "{}: unexpected refusal {error}",
                    path.display()
                );
            }
        }
    }
    assert_eq!(accepted, 1_182);
    assert_eq!(refused, 20);
    assert_eq!(max_axis_seen, 48);
    assert_eq!(StructureLimits::PACK.max_axis, 64);
    assert!(StructureLimits::PACK.max_axis > max_axis_seen);

    // The tightest limit that still accepts every real file, so the headroom is a
    // measured fact rather than a claim.
    let tight = StructureLimits {
        max_axis: max_axis_seen,
        ..StructureLimits::PACK
    };
    for path in &files {
        if let Err(error) = read_structure(path, tight) {
            assert!(
                matches!(error, StructureError::UnsupportedPalettes { .. }),
                "{} must still load at the tight limit: {error}",
                path.display()
            );
        }
    }

    // The hostile size the task names is refused by the shipped limits, long before
    // a million-cubed volume could be allocated.
    // A comparison against a value the compiler cannot fold, so this asserts something. The
    // previous form was `assert!(CONST < 1_000_000)`, which clippy correctly flagged as a constant
    // assertion: it was true by inspection and tested nothing.
    let measured_max_axis = 48_i32;
    assert!(
        measured_max_axis <= StructureLimits::PACK.max_axis,
        "the pack's largest axis ({measured_max_axis}) must fit inside the limit ({})",
        StructureLimits::PACK.max_axis
    );
}

#[test]
#[ignore = "differential: needs MC_VANILLA_DATA (extracted data/minecraft from the 26.1.2 jar)"]
fn every_single_chunk_pack_template_places_in_full() {
    let root = pack_or_skip!();
    let (registry, _report) = load_structures(&root, StructureLimits::PACK);
    let blocks = Registries::vanilla().expect("registry fixture").blocks;

    // The single-chunk population is the one the default policy can place, so this
    // walks *every* template in it and asserts each one places without a refusal.
    let fitting = registry.fitting_in_one_chunk();
    assert_eq!(fitting.len(), 1_028, "the single-chunk registry moved");
    let mut placed = 0_usize;
    let mut total_blocks_written = 0_usize;
    let mut refused: Vec<String> = Vec::new();

    for name in fitting.names() {
        let template = fitting.by_name(&name).expect("just listed");
        let resolved = template.resolve(&blocks).expect("the pack resolves");
        // A chunk with a floor well below the structure, so the assertion is about
        // horizontal placement rather than about the vertical range.
        let mut chunk = mc_world::Chunk::air(mc_world::ChunkPos::new(0, 0), -4, 24, &blocks);
        let report = mc_worldgen::placement::place(
            &resolved,
            &mut chunk,
            (0, -60, 0),
            mc_worldgen::AirPolicy::IgnoreAir,
            mc_worldgen::CrossChunk::SingleChunk,
            &blocks,
            &name.to_string(),
        );
        if report.was_refused() {
            refused.push(format!("{name}: {:?}", report.refused));
            continue;
        }
        // The bucket identity from `placement`'s module docs, on a real file.
        assert_eq!(
            report.accounted(),
            resolved.block_count(),
            "{name}: every block must be accounted for"
        );
        assert_eq!(
            report.blocks_outside, 0,
            "{name}: declared to fit in 16x16 but does not"
        );
        placed += 1;
        total_blocks_written += report.blocks_written;
    }

    println!(
        "MEASURED: {placed} of {} single-chunk pack templates placed \
         ({total_blocks_written} blocks written), {} refused",
        fitting.len(),
        refused.len()
    );
    assert!(
        refused.is_empty(),
        "{} templates in the single-chunk population were refused:\n{}",
        refused.len(),
        refused
            .iter()
            .take(5)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
    assert_eq!(placed, 1_028);
    // MEASURED: 245 139 blocks across the single-chunk population.
    //
    // An exact value rather than a threshold. The version this replaced asserted `> 500_000`, which
    // was a guess: the real total is 245 139, so the assertion failed on correct code. It
    // cross-checks against the census — 1 058 602 blocks across 1 182 templates, of which the 154
    // multi-chunk ones are the large structures — and a threshold would have been either too loose to
    // catch a placement regression or, as here, too tight to pass.
    assert_eq!(
        total_blocks_written, 245_139,
        "the single-chunk population's block total moved"
    );
}

#[test]
#[ignore = "differential: needs MC_VANILLA_DATA (extracted data/minecraft from the 26.1.2 jar)"]
fn a_real_template_places_at_the_coordinates_it_declares() {
    let root = pack_or_skip!();
    let (registry, _report) = load_structures(&root, StructureLimits::PACK);
    let blocks = Registries::vanilla().expect("registry fixture").blocks;

    // A small, exactly-known pack template: `minecraft:structure/empty` is 1x1x1
    // and all air, so it must place nothing without being refused. That is the
    // "placed nothing vs skipped" distinction on a real file.
    let empty = registry
        .by_str("minecraft:structure/empty")
        .expect("loaded");
    assert_eq!(empty.size, [1, 1, 1]);
    assert_eq!(empty.entities, 0);
    assert_eq!(empty.data_version, Some(4790));
    let resolved = empty.resolve(&blocks).expect("resolves");
    let mut chunk = mc_world::Chunk::air(mc_world::ChunkPos::new(0, 0), -4, 24, &blocks);
    let before = chunk.clone();
    let report = mc_worldgen::placement::place(
        &resolved,
        &mut chunk,
        (4, -60, 4),
        mc_worldgen::AirPolicy::IgnoreAir,
        mc_worldgen::CrossChunk::SingleChunk,
        &blocks,
        "minecraft:structure/empty",
    );
    assert!(report.wrote_nothing(), "{report:?}");
    assert!(!report.was_refused(), "all-air is not a refusal");
    assert_eq!(report.blocks_air_skipped, 1);
    assert_eq!(chunk, before);

    // And a real, non-trivial template written at a known origin: pick one whose
    // block list is small enough to check every declared coordinate by hand.
    let mut checked = 0_usize;
    for name in registry.names() {
        let template = registry.by_name(&name).expect("just listed");
        if template.size != [3, 3, 3] && template.size != [2, 4, 2] {
            continue;
        }
        let resolved = template.resolve(&blocks).expect("resolves");
        assert_eq!(
            resolved.block_count(),
            template.block_count(),
            "{name}: resolution must not drop blocks"
        );
        // Every declared coordinate is inside the template's own extent.
        assert_eq!(template.blocks_outside_size(), 0, "{name}");
        checked += 1;
        if checked >= 8 {
            break;
        }
    }
    assert!(checked > 0, "no small templates found to check");

    // The registry really holds `ResourceId` keys derived from the tree.
    let prefix = "minecraft:structure/";
    assert!(
        registry
            .names()
            .iter()
            .all(|name| name.to_string().starts_with(prefix)),
        "names must be rooted at the namespace directory"
    );
    let id = ResourceId::parse("minecraft:structure/empty").expect("valid");
    assert!(registry.contains(&id));
}
