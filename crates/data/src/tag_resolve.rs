//! Tag resolution: transitive closure with cycle and depth guards.
//!
//! Kept separate from the file loading in [`super::tag`] so the algorithm can be
//! tested without a filesystem, and so its guards are visible in one place.
//!
//! ## Why a bounded recursion rather than an explicit stack machine
//!
//! An earlier draft used a hand-rolled explicit-stack DFS with per-frame
//! accumulators. It was harder to read *and* it was wrong (frames were popped out of
//! step with the stack). The property that actually has to hold is "resolution
//! terminates on any input", and there are two honest ways to get it: an explicit
//! machine, or a recursion with a depth limit. Vanilla's deepest tag nesting is
//! **4**, so a limit of 64 is 16x headroom, and a pack that nests deeper than that is
//! reported rather than followed. That makes the recursion depth a *checked input*
//! rather than a hope about the input, which is the same guarantee with far less
//! mechanism.

use mc_core::ids::ResourceId;
use std::collections::{BTreeMap, BTreeSet};

use super::tag::{RegistryContents, TagEntry, TagKey, TagLoadReport, TagProblem, TagValue};

/// Deepest tag nesting resolution will follow.
///
/// Vanilla's deepest is 4 (`block/supports_crimson_fungus`). 64 is 16x headroom; a
/// deeper chain is a pack that is either generated or hostile, and either way it is
/// reported instead of followed.
pub const MAX_TAG_DEPTH: usize = 64;

/// Resolve every tag, applying pack-override rules.
///
/// `ordered` is the files in load order: a later file with `"replace": true` clears
/// what came before, and without it the entries are **appended**. That is the pack
/// format's merge rule, and it is why overriding a vanilla tag needs `replace`.
pub fn resolve_all(
    ordered: &[(TagKey, TagValue)],
    contents: &RegistryContents,
    report: &mut TagLoadReport,
) -> BTreeMap<TagKey, BTreeSet<ResourceId>> {
    let merged = merge(ordered, report);
    let mut memo: BTreeMap<TagKey, BTreeSet<ResourceId>> = BTreeMap::new();
    let mut resolving: Vec<TagKey> = Vec::new();

    for key in merged.keys() {
        if !memo.contains_key(key) {
            let mut ids = BTreeSet::new();
            resolve_into(
                key,
                &merged,
                contents,
                report,
                &mut memo,
                &mut resolving,
                &mut ids,
                0,
            );
        }
    }
    memo
}

/// Merge files into one value per tag, in order.
fn merge(ordered: &[(TagKey, TagValue)], report: &mut TagLoadReport) -> BTreeMap<TagKey, TagValue> {
    let mut merged: BTreeMap<TagKey, TagValue> = BTreeMap::new();
    for (key, value) in ordered {
        report.files += 1;
        let slot = merged.entry(key.clone()).or_default();
        if value.replace {
            *slot = value.clone();
        } else {
            slot.entries.extend(value.entries.iter().cloned());
        }
    }
    merged
}

/// Accumulate `key`'s ids into `out`, resolving references depth-first.
#[allow(clippy::too_many_arguments)]
fn resolve_into(
    key: &TagKey,
    merged: &BTreeMap<TagKey, TagValue>,
    contents: &RegistryContents,
    report: &mut TagLoadReport,
    memo: &mut BTreeMap<TagKey, BTreeSet<ResourceId>>,
    resolving: &mut Vec<TagKey>,
    out: &mut BTreeSet<ResourceId>,
    depth: usize,
) {
    if let Some(done) = memo.get(key) {
        out.extend(done.iter().cloned());
        return;
    }
    if depth > MAX_TAG_DEPTH {
        report.problems.push(TagProblem::TooDeep {
            at: key.clone(),
            limit: MAX_TAG_DEPTH,
        });
        return;
    }
    // A tag already on the resolution path is a cycle. Report it once, contribute
    // nothing, and do not recurse: that is what makes termination unconditional.
    if resolving.contains(key) {
        let mut path = resolving.clone();
        path.push(key.clone());
        report.problems.push(TagProblem::Cycle { path });
        return;
    }
    resolving.push(key.clone());

    let mut own: BTreeSet<ResourceId> = BTreeSet::new();
    if let Some(value) = merged.get(key) {
        for entry in &value.entries {
            match entry {
                TagEntry::Id(id) | TagEntry::OptionalId(id) => {
                    let optional = entry.is_optional();
                    match contents.contains(&key.registry, id) {
                        // `None` means the registry is not modelled, so the id is
                        // accepted: this loader must not claim an id is missing when
                        // it simply does not know the registry.
                        Some(true) | None => {
                            own.insert(id.clone());
                        }
                        Some(false) if optional => {
                            report.problems.push(TagProblem::OptionalMissingId {
                                from: key.clone(),
                                missing: id.clone(),
                                registry: key.registry.clone(),
                            });
                        }
                        Some(false) => {
                            report.problems.push(TagProblem::MissingId {
                                from: key.clone(),
                                missing: id.clone(),
                                registry: key.registry.clone(),
                            });
                        }
                    }
                }
                TagEntry::Tag(target) | TagEntry::OptionalTag(target) => {
                    let optional = entry.is_optional();
                    if merged.contains_key(target) {
                        resolve_into(
                            target,
                            merged,
                            contents,
                            report,
                            memo,
                            resolving,
                            &mut own,
                            depth + 1,
                        );
                    } else if optional {
                        report.problems.push(TagProblem::OptionalMissingTag {
                            from: key.clone(),
                            missing: target.clone(),
                        });
                    } else {
                        report.problems.push(TagProblem::MissingTag {
                            from: key.clone(),
                            missing: target.clone(),
                        });
                    }
                }
            }
        }
    }

    // `resolving` is a path, so the key must be the entry we pushed.
    debug_assert_eq!(resolving.last(), Some(key));
    resolving.pop();
    memo.insert(key.clone(), own.clone());
    out.extend(own);
}

#[cfg(test)]
mod tests {
    use super::{MAX_TAG_DEPTH, resolve_all};
    use crate::tag::{RegistryContents, TagEntry, TagKey, TagLoadReport, TagProblem, TagValue};
    use mc_core::ids::ResourceId;
    use std::collections::BTreeSet;

    fn id(text: &str) -> ResourceId {
        ResourceId::parse(text).expect("a valid id")
    }

    fn tag(registry: &str, name: &str) -> TagKey {
        TagKey::new(registry, id(name))
    }

    /// `contents` knows the registry `item` and the four ids used below.
    fn contents() -> RegistryContents {
        let mut contents = RegistryContents::new();
        let ids: BTreeSet<ResourceId> = ["minecraft:oak_planks", "minecraft:stick"]
            .iter()
            .map(|t| id(t))
            .collect();
        contents.insert("item", ids);
        contents
    }

    fn value(entries: Vec<TagEntry>) -> TagValue {
        TagValue {
            replace: false,
            entries,
        }
    }

    #[test]
    fn a_flat_tag_resolves_to_its_ids() {
        let ordered = vec![(
            tag("item", "minecraft:a"),
            value(vec![TagEntry::Id(id("minecraft:oak_planks"))]),
        )];
        let mut report = TagLoadReport::default();
        let resolved = resolve_all(&ordered, &contents(), &mut report);
        assert!(report.is_clean(), "{:?}", report.problems);
        let ids = resolved.get(&tag("item", "minecraft:a")).expect("resolved");
        assert_eq!(ids.len(), 1);
        assert!(ids.contains(&id("minecraft:oak_planks")));
    }

    #[test]
    fn nesting_is_transitive() {
        // a -> b -> id, the shape vanilla uses (deepest observed: 4).
        let ordered = vec![
            (
                tag("item", "minecraft:a"),
                value(vec![TagEntry::Tag(tag("item", "minecraft:b"))]),
            ),
            (
                tag("item", "minecraft:b"),
                value(vec![TagEntry::Tag(tag("item", "minecraft:c"))]),
            ),
            (
                tag("item", "minecraft:c"),
                value(vec![TagEntry::Id(id("minecraft:stick"))]),
            ),
        ];
        let mut report = TagLoadReport::default();
        let resolved = resolve_all(&ordered, &contents(), &mut report);
        assert!(report.is_clean(), "{:?}", report.problems);
        assert!(resolved[&tag("item", "minecraft:a")].contains(&id("minecraft:stick")));
        assert!(resolved[&tag("item", "minecraft:b")].contains(&id("minecraft:stick")));
    }

    #[test]
    fn a_cycle_is_reported_and_terminates() {
        // a -> b -> a. Vanilla has no cycle, so this can only come from a pack, and a
        // resolver that recursed would hang.
        let ordered = vec![
            (
                tag("item", "minecraft:a"),
                value(vec![TagEntry::Tag(tag("item", "minecraft:b"))]),
            ),
            (
                tag("item", "minecraft:b"),
                value(vec![TagEntry::Tag(tag("item", "minecraft:a"))]),
            ),
        ];
        let mut report = TagLoadReport::default();
        let resolved = resolve_all(&ordered, &contents(), &mut report);
        // It terminated, produced entries for both, and said why.
        assert_eq!(resolved.len(), 2);
        let cycles = report.cycles();
        assert!(!cycles.is_empty(), "the cycle must be reported");
        if let TagProblem::Cycle { path } = cycles[0] {
            assert!(path.len() >= 2, "{path:?}");
            assert_eq!(path.first(), path.last(), "the path closes on itself");
        } else {
            panic!("expected a cycle problem");
        }
    }

    #[test]
    fn a_self_reference_is_a_cycle() {
        let ordered = vec![(
            tag("item", "minecraft:a"),
            value(vec![TagEntry::Tag(tag("item", "minecraft:a"))]),
        )];
        let mut report = TagLoadReport::default();
        let resolved = resolve_all(&ordered, &contents(), &mut report);
        assert_eq!(resolved.len(), 1);
        assert_eq!(report.cycles().len(), 1);
    }

    #[test]
    fn a_nesting_chain_longer_than_the_limit_is_reported_not_followed() {
        // `t0 -> t1 -> … -> tN`, one level per tag, deeper than MAX_TAG_DEPTH.
        let mut ordered = Vec::new();
        for level in 0..(MAX_TAG_DEPTH + 8) {
            ordered.push((
                tag("item", &format!("minecraft:t{level}")),
                value(vec![TagEntry::Tag(tag(
                    "item",
                    &format!("minecraft:t{}", level + 1),
                ))]),
            ));
        }
        let mut report = TagLoadReport::default();
        let resolved = resolve_all(&ordered, &contents(), &mut report);
        assert_eq!(resolved.len(), MAX_TAG_DEPTH + 8);
        assert!(
            report
                .problems
                .iter()
                .any(|p| matches!(p, TagProblem::TooDeep { .. })),
            "the depth guard must fire: {:?}",
            report.problems.len()
        );
    }

    #[test]
    fn a_missing_tag_reference_is_reported_not_dropped() {
        let ordered = vec![(
            tag("item", "minecraft:a"),
            value(vec![TagEntry::Tag(tag("item", "minecraft:ghost"))]),
        )];
        let mut report = TagLoadReport::default();
        let _ = resolve_all(&ordered, &contents(), &mut report);
        assert!(matches!(
            report.problems.as_slice(),
            [TagProblem::MissingTag { .. }]
        ));
    }

    #[test]
    fn an_optional_missing_tag_is_distinguishable_from_a_real_error() {
        let ordered = vec![(
            tag("item", "minecraft:a"),
            value(vec![TagEntry::OptionalTag(tag("item", "minecraft:ghost"))]),
        )];
        let mut report = TagLoadReport::default();
        let _ = resolve_all(&ordered, &contents(), &mut report);
        assert!(
            matches!(
                report.problems.as_slice(),
                [TagProblem::OptionalMissingTag { .. }]
            ),
            "{:?}",
            report.problems
        );
    }

    #[test]
    fn a_missing_id_is_reported_only_when_the_registry_is_known() {
        // Known registry, absent id: reported.
        let ordered = vec![(
            tag("item", "minecraft:a"),
            value(vec![TagEntry::Id(id("minecraft:not_a_real_item"))]),
        )];
        let mut report = TagLoadReport::default();
        let _ = resolve_all(&ordered, &contents(), &mut report);
        assert!(matches!(
            report.problems.as_slice(),
            [TagProblem::MissingId { .. }]
        ));

        // Unknown registry: accepted, because the loader cannot know.
        let ordered = vec![(
            tag("unknown_registry", "minecraft:a"),
            value(vec![TagEntry::Id(id("minecraft:whatever"))]),
        )];
        let mut report = TagLoadReport::default();
        let resolved = resolve_all(&ordered, &contents(), &mut report);
        assert!(report.is_clean(), "{:?}", report.problems);
        assert_eq!(resolved[&tag("unknown_registry", "minecraft:a")].len(), 1);
    }

    #[test]
    fn an_optional_missing_id_is_recorded_separately() {
        let ordered = vec![(
            tag("item", "minecraft:a"),
            value(vec![TagEntry::OptionalId(id("minecraft:not_real"))]),
        )];
        let mut report = TagLoadReport::default();
        let _ = resolve_all(&ordered, &contents(), &mut report);
        assert!(matches!(
            report.problems.as_slice(),
            [TagProblem::OptionalMissingId { .. }]
        ));
    }

    #[test]
    fn merge_appends_unless_replace_is_set() {
        // Two files for the same tag: the second appends by default…
        let ordered = vec![
            (
                tag("item", "minecraft:a"),
                value(vec![TagEntry::Id(id("minecraft:oak_planks"))]),
            ),
            (
                tag("item", "minecraft:a"),
                value(vec![TagEntry::Id(id("minecraft:stick"))]),
            ),
        ];
        let mut report = TagLoadReport::default();
        let resolved = resolve_all(&ordered, &contents(), &mut report);
        assert_eq!(resolved[&tag("item", "minecraft:a")].len(), 2);

        // …and replaces when it says so.
        let ordered = vec![
            (
                tag("item", "minecraft:a"),
                value(vec![TagEntry::Id(id("minecraft:oak_planks"))]),
            ),
            (
                tag("item", "minecraft:a"),
                TagValue {
                    replace: true,
                    entries: vec![TagEntry::Id(id("minecraft:stick"))],
                },
            ),
        ];
        let mut report = TagLoadReport::default();
        let resolved = resolve_all(&ordered, &contents(), &mut report);
        let ids = &resolved[&tag("item", "minecraft:a")];
        assert_eq!(ids.len(), 1, "replace discards the earlier file");
        assert!(ids.contains(&id("minecraft:stick")));
    }

    #[test]
    fn a_tag_declared_but_empty_resolves_to_an_empty_set() {
        let ordered = vec![(tag("item", "minecraft:empty"), value(Vec::new()))];
        let mut report = TagLoadReport::default();
        let resolved = resolve_all(&ordered, &contents(), &mut report);
        assert!(report.is_clean());
        assert_eq!(resolved[&tag("item", "minecraft:empty")].len(), 0);
    }

    #[test]
    fn a_diamond_dependency_is_resolved_once_and_correctly() {
        // a -> {b, c}; b -> d; c -> d. The shared child must be counted once in `a`.
        let ordered = vec![
            (
                tag("item", "minecraft:a"),
                value(vec![
                    TagEntry::Tag(tag("item", "minecraft:b")),
                    TagEntry::Tag(tag("item", "minecraft:c")),
                ]),
            ),
            (
                tag("item", "minecraft:b"),
                value(vec![TagEntry::Tag(tag("item", "minecraft:d"))]),
            ),
            (
                tag("item", "minecraft:c"),
                value(vec![TagEntry::Tag(tag("item", "minecraft:d"))]),
            ),
            (
                tag("item", "minecraft:d"),
                value(vec![TagEntry::Id(id("minecraft:stick"))]),
            ),
        ];
        let mut report = TagLoadReport::default();
        let resolved = resolve_all(&ordered, &contents(), &mut report);
        assert!(report.is_clean(), "{:?}", report.problems);
        assert_eq!(
            resolved[&tag("item", "minecraft:a")].len(),
            1,
            "a set, so the shared id appears once"
        );
    }

    #[test]
    fn reporting_counts_files_exactly_once() {
        let ordered = vec![
            (tag("item", "minecraft:a"), value(Vec::new())),
            (tag("item", "minecraft:b"), value(Vec::new())),
        ];
        let mut report = TagLoadReport::default();
        let _ = resolve_all(&ordered, &contents(), &mut report);
        assert_eq!(report.files, 2);
    }
}
