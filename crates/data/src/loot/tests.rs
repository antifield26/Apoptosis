//! Unit tests for [`super`] (P07-10).
//!
//! The fixtures here are written to look like the real files — see the module documentation
//! for the figures they are shaped after — and the deterministic [`SeqRng`] makes every roll
//! reproducible, which is the whole point of injecting the randomness.

use super::{
    BINOMIAL_ROUND_CAP_CHECK, CountProvider, LootCondition, LootContext, LootEntry, LootFunction,
    LootLoadReport, LootPool, LootTable, LootTables, MAX_TABLE_NESTING, Refusal, RollError, Rng,
    roll,
};
use mc_core::ids::ResourceId;
use serde_json::json;
use std::collections::BTreeMap;

/// A deterministic stand-in for `java.util.Random`, for tests only.
///
/// It is **not** claimed to be `java.util.Random`: the real one is `mc-simulation`'s
/// `RandomSource`, which is verified against the JDK, and the trait boundary in the module
/// documentation is what lets that one be wired up in production. What matters here is that
/// the sequence is fixed, so a test can say "this seed produces this loot" and mean it.
struct SeqRng(u64);

impl SeqRng {
    fn new(seed: u64) -> Self {
        Self(seed)
    }
}

impl Rng for SeqRng {
    fn next_u32(&mut self) -> u32 {
        // xorshift64*, which is deterministic and has no state beyond the seed.
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        (x.wrapping_mul(0x2545_f491_4f6c_dd1d) >> 32) as u32
    }
}

fn id(text: &str) -> ResourceId {
    ResourceId::parse(text).expect("a valid id")
}

fn item(name: &str, weight: i32) -> LootEntry {
    LootEntry::Item {
        name: id(name),
        weight,
        quality: 0,
        expand: false,
        functions: Vec::new(),
        conditions: Vec::new(),
    }
}

fn table_with(entries: Vec<LootEntry>, rolls: f64) -> LootTable {
    LootTable {
        name: Some(id("minecraft:test")),
        kind: "minecraft:block".to_owned(),
        random_sequence: None,
        pools: vec![LootPool {
            rolls: CountProvider::Constant(rolls),
            bonus_rolls: CountProvider::Constant(0.0),
            entries,
            conditions: Vec::new(),
            functions: Vec::new(),
        }],
        functions: Vec::new(),
    }
}

fn context() -> LootContext {
    LootContext::default()
}

fn tool(levels: &[(&str, i32)]) -> LootContext {
    let mut map = BTreeMap::new();
    for (name, level) in levels {
        map.insert(id(name), *level);
    }
    LootContext::with_tool(map)
}

#[test]
fn a_single_item_pool_rolls_that_item() {
    let table = table_with(vec![item("minecraft:stone", 1)], 1.0);
    let mut rng = SeqRng::new(1);
    let out = roll(&table, &LootTables::new(), &mut rng, &context()).expect("rolls");
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].item, id("minecraft:stone"));
    assert_eq!(out[0].count, 1);
}

#[test]
fn the_same_seed_rolls_the_same_loot() {
    // AGENTS.md section 3.6: the same state plus the same inputs must give the same output.
    let table = table_with(
        vec![
            item("minecraft:cod", 60),
            item("minecraft:salmon", 25),
            item("minecraft:tropical_fish", 2),
            item("minecraft:pufferfish", 13),
        ],
        3.0,
    );
    let first = roll(
        &table,
        &LootTables::new(),
        &mut SeqRng::new(42),
        &context(),
    )
    .expect("rolls");
    let second = roll(
        &table,
        &LootTables::new(),
        &mut SeqRng::new(42),
        &context(),
    )
    .expect("rolls");
    assert_eq!(first, second);
    assert_eq!(first.len(), 3, "three rolls, from the pool's rolls value");
}

#[test]
fn weights_are_respected_across_many_draws() {
    // A 60:25:2:13 pool, drawn 10 000 times: the observed shares must track the weights. This
    // is a distribution test, so the bound is loose enough not to flake and tight enough to
    // catch a selection bug (an unweighted pick would give 25% each).
    let table = table_with(
        vec![
            item("minecraft:cod", 60),
            item("minecraft:salmon", 25),
            item("minecraft:tropical_fish", 2),
            item("minecraft:pufferfish", 13),
        ],
        1.0,
    );
    let mut rng = SeqRng::new(7);
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for _ in 0..10_000 {
        let out = roll(&table, &LootTables::new(), &mut rng, &context()).expect("rolls");
        for stack in out {
            *counts.entry(stack.item.to_string()).or_insert(0) += 1;
        }
    }
    let cod = counts.get("minecraft:cod").copied().unwrap_or(0);
    let tropical = counts
        .get("minecraft:tropical_fish")
        .copied()
        .unwrap_or(0);
    assert!(
        (5_400..6_600).contains(&cod),
        "cod should be about 60%, got {cod}"
    );
    assert!(
        (100..350).contains(&tropical),
        "tropical_fish should be about 2%, got {tropical}"
    );
}

#[test]
fn rolls_equal_to_zero_produce_nothing() {
    let table = table_with(vec![item("minecraft:stone", 1)], 0.0);
    let out = roll(
        &table,
        &LootTables::new(),
        &mut SeqRng::new(1),
        &context(),
    )
    .expect("rolls");
    assert!(out.is_empty());
}

#[test]
fn a_uniform_roll_count_is_drawn_from_the_provider() {
    let mut table = table_with(vec![item("minecraft:stone", 1)], 0.0);
    table.pools[0].rolls = CountProvider::Uniform { min: 2.0, max: 5.0 };
    for seed in 0..32 {
        let out = roll(
            &table,
            &LootTables::new(),
            &mut SeqRng::new(seed),
            &context(),
        )
        .expect("rolls");
        assert!(
            (2..=5).contains(&out.len()),
            "uniform 2..5 gave {} rolls at seed {seed}",
            out.len()
        );
    }
}

#[test]
fn set_count_sets_and_adds() {
    let mut table = table_with(vec![item("minecraft:stick", 1)], 1.0);
    table.pools[0].entries[0] = LootEntry::Item {
        name: id("minecraft:stick"),
        weight: 1,
        quality: 0,
        expand: false,
        functions: vec![LootFunction::SetCount {
            count: CountProvider::Constant(4.0),
            add: false,
        }],
        conditions: Vec::new(),
    };
    let out = roll(
        &table,
        &LootTables::new(),
        &mut SeqRng::new(1),
        &context(),
    )
    .expect("rolls");
    assert_eq!(out[0].count, 4, "set_count replaces");

    table.pools[0].entries[0] = LootEntry::Item {
        name: id("minecraft:stick"),
        weight: 1,
        quality: 0,
        expand: false,
        functions: vec![LootFunction::SetCount {
            count: CountProvider::Constant(3.0),
            add: true,
        }],
        conditions: Vec::new(),
    };
    let out = roll(
        &table,
        &LootTables::new(),
        &mut SeqRng::new(1),
        &context(),
    )
    .expect("rolls");
    assert_eq!(out[0].count, 4, "add starts from the 1 the entry produced");
}

#[test]
fn limit_count_clamps_and_discards() {
    let mut table = table_with(vec![item("minecraft:stick", 1)], 1.0);
    let entry = LootEntry::Item {
        name: id("minecraft:stick"),
        weight: 1,
        quality: 0,
        expand: false,
        functions: vec![
            LootFunction::SetCount {
                count: CountProvider::Constant(10.0),
                add: false,
            },
            LootFunction::LimitCount { min: 1, max: 8 },
        ],
        conditions: Vec::new(),
    };
    table.pools[0].entries[0] = entry.clone();
    let out = roll(
        &table,
        &LootTables::new(),
        &mut SeqRng::new(1),
        &context(),
    )
    .expect("rolls");
    assert_eq!(out[0].count, 8, "clamped to max");

    table.pools[0].entries[0] = LootEntry::Item {
        name: id("minecraft:stick"),
        weight: 1,
        quality: 0,
        expand: false,
        functions: vec![
            LootFunction::SetCount {
                count: CountProvider::Constant(0.0),
                add: false,
            },
            LootFunction::LimitCount { min: 1, max: 8 },
        ],
        conditions: Vec::new(),
    };
    let out = roll(
        &table,
        &LootTables::new(),
        &mut SeqRng::new(1),
        &context(),
    )
    .expect("rolls");
    assert_eq!(out[0].count, 0, "below min means an empty stack");
}

#[test]
fn alternatives_take_the_first_child_whose_conditions_pass() {
    // The shape of `blocks/stone`: silk touch first, cobblestone second. Both are item
    // entries; the first carries a `match_tool` condition on silk touch.
    let silk_touch = LootCondition::MatchToolEnchantments {
        requirements: vec![super::EnchantmentRequirement {
            enchantment: "minecraft:silk_touch".to_owned(),
            min_level: Some(1),
            max_level: None,
        }],
    };
    let mut table = table_with(
        vec![LootEntry::Alternatives {
            children: vec![
                LootEntry::Item {
                    name: id("minecraft:stone"),
                    weight: 1,
                    quality: 0,
                    expand: false,
                    functions: Vec::new(),
                    conditions: vec![silk_touch],
                },
                LootEntry::Item {
                    name: id("minecraft:cobblestone"),
                    weight: 1,
                    quality: 0,
                    expand: false,
                    functions: Vec::new(),
                    conditions: vec![LootCondition::SurvivesExplosion],
                },
            ],
            conditions: Vec::new(),
        }],
        1.0,
    );

    // With silk touch and a survived explosion: stone.
    let mut with_silk = tool(&[("minecraft:silk_touch", 1)]);
    with_silk.survives_explosion = Some(true);
    let out = roll(&table, &LootTables::new(), &mut SeqRng::new(1), &with_silk).expect("rolls");
    assert_eq!(out[0].item, id("minecraft:stone"));

    // Without silk touch: cobblestone.
    let mut without = tool(&[("minecraft:fortune", 3)]);
    without.survives_explosion = Some(true);
    let out = roll(&table, &LootTables::new(), &mut SeqRng::new(1), &without).expect("rolls");
    assert_eq!(out[0].item, id("minecraft:cobblestone"));

    // With an unknown tool: refused, because the same table gives different answers.
    let err = roll(&table, &LootTables::new(), &mut SeqRng::new(1), &context())
        .expect_err("must refuse");
    match err {
        RollError::Unexecutable { refusals } => assert!(
            refusals
                .iter()
                .any(|refusal| refusal.type_name == "minecraft:match_tool"),
            "{refusals:?}"
        ),
        other => panic!("expected a refusal, got {other}"),
    }

    // The table's structure is unchanged by any of that.
    assert_eq!(table.pools[0].entries.len(), 1);
    let _ = &mut table;
}

#[test]
fn an_unmodelled_condition_refuses_rather_than_being_ignored() {
    // The critical property: a condition this build cannot evaluate must not be treated as
    // "false" (which silently deletes a drop) or as "true" (which silently adds one).
    let mut table = table_with(vec![item("minecraft:diamond", 1)], 1.0);
    table.pools[0].entries[0] = LootEntry::Item {
        name: id("minecraft:diamond"),
        weight: 1,
        quality: 0,
        expand: false,
        functions: Vec::new(),
        conditions: vec![LootCondition::Raw(json!({
            "condition": "minecraft:block_state_property",
            "block": "minecraft:wheat",
            "properties": {"age": "7"}
        }))],
    };
    let err = roll(
        &table,
        &LootTables::new(),
        &mut SeqRng::new(1),
        &context(),
    )
    .expect_err("must refuse");
    let text = err.to_string();
    assert!(text.contains("block_state_property"), "{text}");
    assert!(
        !text.is_empty() && matches!(err, RollError::Unexecutable { .. }),
        "{text}"
    );
}

#[test]
fn an_unmodelled_function_refuses_rather_than_being_skipped() {
    let mut table = table_with(vec![item("minecraft:raw_iron", 1)], 1.0);
    table.pools[0].entries[0] = LootEntry::Item {
        name: id("minecraft:raw_iron"),
        weight: 1,
        quality: 0,
        expand: false,
        functions: vec![LootFunction::Raw(json!({
            "function": "minecraft:apply_bonus",
            "enchantment": "minecraft:fortune",
            "formula": "minecraft:ore_drops"
        }))],
        conditions: Vec::new(),
    };
    let err = roll(
        &table,
        &LootTables::new(),
        &mut SeqRng::new(1),
        &context(),
    )
    .expect_err("must refuse");
    assert!(err.to_string().contains("apply_bonus"), "{err}");
}

#[test]
fn set_damage_is_modelled_but_refused() {
    let mut table = table_with(vec![item("minecraft:iron_pickaxe", 1)], 1.0);
    table.pools[0].entries[0] = LootEntry::Item {
        name: id("minecraft:iron_pickaxe"),
        weight: 1,
        quality: 0,
        expand: false,
        functions: vec![LootFunction::SetDamage {
            damage: CountProvider::Constant(0.5),
            add: false,
        }],
        conditions: Vec::new(),
    };
    let err = roll(
        &table,
        &LootTables::new(),
        &mut SeqRng::new(1),
        &context(),
    )
    .expect_err("must refuse");
    assert!(err.to_string().contains("set_damage"), "{err}");
}

#[test]
fn a_refusal_lists_every_reason_not_only_the_first() {
    // A table with two different unexecutable constructs must report both, so one run tells
    // the whole story.
    let mut table = table_with(vec![item("minecraft:stone", 1)], 1.0);
    table.functions = vec![LootFunction::Raw(json!({"function": "minecraft:copy_name"}))];
    table.pools[0].conditions = vec![LootCondition::Raw(json!({"condition": "minecraft:location_check"}))];
    table.pools[0].entries[0] = LootEntry::Item {
        name: id("minecraft:stone"),
        weight: 1,
        quality: 0,
        expand: false,
        functions: vec![LootFunction::Raw(json!({"function": "minecraft:furnace_smelt"}))],
        conditions: Vec::new(),
    };
    let err = roll(
        &table,
        &LootTables::new(),
        &mut SeqRng::new(1),
        &context(),
    )
    .expect_err("must refuse");
    let text = err.to_string();
    assert!(text.contains("copy_name"), "{text}");
    assert!(text.contains("location_check"), "{text}");
}

#[test]
fn a_refusal_is_deterministic() {
    let mut table = table_with(vec![item("minecraft:stone", 1)], 1.0);
    table.functions = vec![
        LootFunction::Raw(json!({"function": "minecraft:zeta"})),
        LootFunction::Raw(json!({"function": "minecraft:alpha"})),
    ];
    let first = roll(
        &table,
        &LootTables::new(),
        &mut SeqRng::new(1),
        &context(),
    )
    .expect_err("must refuse");
    let second = roll(
        &table,
        &LootTables::new(),
        &mut SeqRng::new(9),
        &context(),
    )
    .expect_err("must refuse");
    assert_eq!(first.to_string(), second.to_string());
}

#[test]
fn a_named_reference_resolves_through_the_registry() {
    let mut tables = LootTables::new();
    tables.insert(table_with(vec![item("minecraft:creeper_head", 1)], 1.0));
    let referencing = LootTable {
        name: Some(id("minecraft:entities/zombie")),
        kind: "minecraft:entity".to_owned(),
        random_sequence: None,
        pools: vec![LootPool {
            rolls: CountProvider::Constant(1.0),
            bonus_rolls: CountProvider::Constant(0.0),
            entries: vec![LootEntry::LootTable {
                name: Some(id("minecraft:test")),
                inline: None,
                weight: 1,
                quality: 0,
                conditions: Vec::new(),
            }],
            conditions: Vec::new(),
            functions: Vec::new(),
        }],
        functions: Vec::new(),
    };
    tables.insert(referencing.clone());
    let out = roll(&referencing, &tables, &mut SeqRng::new(1), &context()).expect("rolls");
    assert_eq!(out[0].item, id("minecraft:creeper_head"));
}

#[test]
fn a_missing_reference_is_an_error_naming_it() {
    let referencing = LootTable {
        name: Some(id("minecraft:entities/zombie")),
        kind: "minecraft:entity".to_owned(),
        random_sequence: None,
        pools: vec![LootPool {
            rolls: CountProvider::Constant(1.0),
            bonus_rolls: CountProvider::Constant(0.0),
            entries: vec![LootEntry::LootTable {
                name: Some(id("minecraft:charged_creeper/zombie")),
                inline: None,
                weight: 1,
                quality: 0,
                conditions: Vec::new(),
            }],
            conditions: Vec::new(),
            functions: Vec::new(),
        }],
        functions: Vec::new(),
    };
    let err = roll(
        &referencing,
        &LootTables::new(),
        &mut SeqRng::new(1),
        &context(),
    )
    .expect_err("must refuse");
    assert!(matches!(err, RollError::UnknownTable { .. }), "{err}");
    assert!(err.to_string().contains("charged_creeper/zombie"), "{err}");
}

#[test]
fn a_cycle_between_two_tables_is_reported_and_terminates() {
    // A cycle cannot come from vanilla (its deepest chain is 2), so it can only come from a
    // pack — and a roller that recursed would hang.
    let mut tables = LootTables::new();
    let make = |name: &str, target: &str| LootTable {
        name: Some(id(name)),
        kind: "minecraft:chest".to_owned(),
        random_sequence: None,
        pools: vec![LootPool {
            rolls: CountProvider::Constant(1.0),
            bonus_rolls: CountProvider::Constant(0.0),
            entries: vec![LootEntry::LootTable {
                name: Some(id(target)),
                inline: None,
                weight: 1,
                quality: 0,
                conditions: Vec::new(),
            }],
            conditions: Vec::new(),
            functions: Vec::new(),
        }],
        functions: Vec::new(),
    };
    let a = make("minecraft:a", "minecraft:b");
    tables.insert(a.clone());
    tables.insert(make("minecraft:b", "minecraft:a"));
    let err = roll(&a, &tables, &mut SeqRng::new(1), &context()).expect_err("must refuse");
    assert!(matches!(err, RollError::Cyclic { .. }), "{err}");
    assert!(err.to_string().contains("minecraft:a"), "{err}");
}

#[test]
fn a_self_reference_is_a_cycle() {
    let mut tables = LootTables::new();
    let a = LootTable {
        name: Some(id("minecraft:a")),
        kind: "minecraft:chest".to_owned(),
        random_sequence: None,
        pools: vec![LootPool {
            rolls: CountProvider::Constant(1.0),
            bonus_rolls: CountProvider::Constant(0.0),
            entries: vec![LootEntry::LootTable {
                name: Some(id("minecraft:a")),
                inline: None,
                weight: 1,
                quality: 0,
                conditions: Vec::new(),
            }],
            conditions: Vec::new(),
            functions: Vec::new(),
        }],
        functions: Vec::new(),
    };
    tables.insert(a.clone());
    let err = roll(&a, &tables, &mut SeqRng::new(1), &context()).expect_err("must refuse");
    assert!(matches!(err, RollError::Cyclic { .. }), "{err}");
}

#[test]
fn an_inline_table_is_rolled_in_place() {
    // The shape of `equipment/trial_chamber`, which inlines three tables.
    let inline = LootTable {
        name: None,
        kind: "minecraft:inline".to_owned(),
        random_sequence: None,
        pools: vec![LootPool {
            rolls: CountProvider::Constant(1.0),
            bonus_rolls: CountProvider::Constant(0.0),
            entries: vec![item("minecraft:chainmail_helmet", 1)],
            conditions: Vec::new(),
            functions: Vec::new(),
        }],
        functions: Vec::new(),
    };
    let table = LootTable {
        name: Some(id("minecraft:equipment/trial_chamber")),
        kind: "minecraft:equipment".to_owned(),
        random_sequence: None,
        pools: vec![LootPool {
            rolls: CountProvider::Constant(1.0),
            bonus_rolls: CountProvider::Constant(0.0),
            entries: vec![LootEntry::LootTable {
                name: None,
                inline: Some(Box::new(inline)),
                weight: 1,
                quality: 0,
                conditions: Vec::new(),
            }],
            conditions: Vec::new(),
            functions: Vec::new(),
        }],
        functions: Vec::new(),
    };
    let out = roll(&table, &LootTables::new(), &mut SeqRng::new(1), &context()).expect("rolls");
    assert_eq!(out[0].item, id("minecraft:chainmail_helmet"));
    assert_eq!(table.nested_tables().len(), 1);
}

#[test]
fn a_tag_entry_is_refused() {
    let mut table = table_with(vec![item("minecraft:stone", 1)], 1.0);
    table.pools[0].entries[0] = LootEntry::Tag {
        name: id("minecraft:creeper_drop_music_discs"),
        weight: 1,
        quality: 0,
        conditions: Vec::new(),
    };
    let err = roll(
        &table,
        &LootTables::new(),
        &mut SeqRng::new(1),
        &context(),
    )
    .expect_err("must refuse");
    assert!(err.to_string().contains("creeper_drop_music_discs"), "{err}");
}

#[test]
fn a_dynamic_entry_is_refused() {
    let mut table = table_with(vec![item("minecraft:stone", 1)], 1.0);
    table.pools[0].entries[0] = LootEntry::Dynamic {
        name: "minecraft:sherds".to_owned(),
        weight: 1,
        quality: 0,
        conditions: Vec::new(),
    };
    let err = roll(
        &table,
        &LootTables::new(),
        &mut SeqRng::new(1),
        &context(),
    )
    .expect_err("must refuse");
    assert!(err.to_string().contains("minecraft:sherds"), "{err}");
}

#[test]
fn survives_explosion_needs_the_context_field() {
    let mut table = table_with(vec![item("minecraft:cobblestone", 1)], 1.0);
    table.pools[0].conditions = vec![LootCondition::SurvivesExplosion];

    let err = roll(
        &table,
        &LootTables::new(),
        &mut SeqRng::new(1),
        &context(),
    )
    .expect_err("must refuse without the field");
    assert!(err.to_string().contains("survives_explosion"), "{err}");

    let mut survived = context();
    survived.survives_explosion = Some(true);
    let out = roll(
        &table,
        &LootTables::new(),
        &mut SeqRng::new(1),
        &survived,
    )
    .expect("rolls");
    assert_eq!(out.len(), 1);

    let mut destroyed = context();
    destroyed.survives_explosion = Some(false);
    let out = roll(
        &table,
        &LootTables::new(),
        &mut SeqRng::new(1),
        &destroyed,
    )
    .expect("rolls");
    assert!(out.is_empty(), "the condition said no");
}

#[test]
fn random_chance_zero_never_fires_and_one_always_does() {
    for chance in [0.0f32, 1.0] {
        let mut table = table_with(vec![item("minecraft:stone", 1)], 1.0);
        table.pools[0].conditions = vec![LootCondition::RandomChance { chance }];
        for seed in 0..40 {
            let out = roll(
                &table,
                &LootTables::new(),
                &mut SeqRng::new(seed),
                &context(),
            )
            .expect("rolls");
            if chance == 0.0 {
                assert!(out.is_empty(), "a 0% chance fired at seed {seed}");
            } else {
                assert_eq!(out.len(), 1, "a 100% chance failed at seed {seed}");
            }
        }
    }
}

#[test]
fn any_of_and_inverted_compose() {
    let mut table = table_with(vec![item("minecraft:stone", 1)], 1.0);
    // `any_of [inverted(survives_explosion)]`: passes when the explosion destroyed the block.
    table.pools[0].conditions = vec![LootCondition::AnyOf {
        terms: vec![LootCondition::Inverted {
            term: Box::new(LootCondition::SurvivesExplosion),
        }],
    }];
    let mut destroyed = context();
    destroyed.survives_explosion = Some(false);
    let out = roll(
        &table,
        &LootTables::new(),
        &mut SeqRng::new(1),
        &destroyed,
    )
    .expect("rolls");
    assert_eq!(out.len(), 1);

    let mut survived = context();
    survived.survives_explosion = Some(true);
    let out = roll(
        &table,
        &LootTables::new(),
        &mut SeqRng::new(1),
        &survived,
    )
    .expect("rolls");
    assert!(out.is_empty());
}

#[test]
fn an_unknown_member_of_any_of_refuses_even_when_another_term_passes() {
    // "I do not know" is not "false": a term that cannot be evaluated makes the whole
    // disjunction unknown, because the outcome could differ.
    let mut table = table_with(vec![item("minecraft:stone", 1)], 1.0);
    table.pools[0].conditions = vec![LootCondition::AnyOf {
        terms: vec![
            LootCondition::Raw(json!({"condition": "minecraft:location_check"})),
            LootCondition::RandomChance { chance: 1.0 },
        ],
    }];
    let err = roll(
        &table,
        &LootTables::new(),
        &mut SeqRng::new(1),
        &context(),
    )
    .expect_err("must refuse");
    assert!(err.to_string().contains("location_check"), "{err}");
}

#[test]
fn a_table_bonus_is_indexed_by_enchantment_level() {
    let mut table = table_with(vec![item("minecraft:gravel", 1)], 1.0);
    table.pools[0].conditions = vec![LootCondition::TableBonus {
        enchantment: id("minecraft:fortune"),
        // Level 0 never fires, level 1 always does.
        chances: vec![0.0, 1.0],
    }];
    let out = roll(
        &table,
        &LootTables::new(),
        &mut SeqRng::new(1),
        &tool(&[("minecraft:fortune", 0)]),
    )
    .expect("rolls");
    assert!(out.is_empty(), "level 0 should never fire");
    let out = roll(
        &table,
        &LootTables::new(),
        &mut SeqRng::new(1),
        &tool(&[("minecraft:fortune", 1)]),
    )
    .expect("rolls");
    assert_eq!(out.len(), 1, "level 1 should always fire");
    // A level past the end saturates at the last chance rather than reading out of bounds.
    let out = roll(
        &table,
        &LootTables::new(),
        &mut SeqRng::new(1),
        &tool(&[("minecraft:fortune", 99)]),
    )
    .expect("rolls");
    assert_eq!(out.len(), 1);
}

#[test]
fn an_empty_table_bonus_is_refused_not_a_panic() {
    let mut table = table_with(vec![item("minecraft:gravel", 1)], 1.0);
    table.pools[0].conditions = vec![LootCondition::TableBonus {
        enchantment: id("minecraft:fortune"),
        chances: Vec::new(),
    }];
    let err = roll(
        &table,
        &LootTables::new(),
        &mut SeqRng::new(1),
        &tool(&[("minecraft:fortune", 1)]),
    )
    .expect_err("must refuse");
    assert!(err.to_string().contains("table_bonus"), "{err}");
}

#[test]
fn a_binomial_count_stays_within_its_rounds() {
    let mut table = table_with(vec![item("minecraft:wheat_seeds", 1)], 1.0);
    table.pools[0].entries[0] = LootEntry::Item {
        name: id("minecraft:wheat_seeds"),
        weight: 1,
        quality: 0,
        expand: false,
        functions: vec![LootFunction::SetCount {
            count: CountProvider::Binomial {
                extra: 3,
                probability: 1.0,
            },
            add: false,
        }],
        conditions: Vec::new(),
    };
    for seed in 0..16 {
        let out = roll(
            &table,
            &LootTables::new(),
            &mut SeqRng::new(seed),
            &context(),
        )
        .expect("rolls");
        assert_eq!(out[0].count, 4, "3 rounds at p=1 plus the base 1");
    }
}

#[test]
fn an_absurd_binomial_is_refused_rather_than_burning_draws() {
    // AGENTS.md section 10: allocation/CPU amplification. One billion rounds would not
    // crash, it would stall — which on a 20 TPS server is the same outage.
    let mut table = table_with(vec![item("minecraft:wheat_seeds", 1)], 1.0);
    table.pools[0].entries[0] = LootEntry::Item {
        name: id("minecraft:wheat_seeds"),
        weight: 1,
        quality: 0,
        expand: false,
        functions: vec![LootFunction::SetCount {
            count: CountProvider::Binomial {
                extra: 1_000_000_000,
                probability: 0.5,
            },
            add: false,
        }],
        conditions: Vec::new(),
    };
    let err = roll(
        &table,
        &LootTables::new(),
        &mut SeqRng::new(1),
        &context(),
    )
    .expect_err("must refuse");
    assert!(err.to_string().contains("binomial"), "{err}");
}

#[test]
fn quality_without_luck_is_refused() {
    let mut table = table_with(vec![item("minecraft:diamond", 1)], 1.0);
    table.pools[0].entries[0] = LootEntry::Item {
        name: id("minecraft:diamond"),
        weight: 1,
        quality: 2,
        expand: false,
        functions: Vec::new(),
        conditions: Vec::new(),
    };
    let err = roll(
        &table,
        &LootTables::new(),
        &mut SeqRng::new(1),
        &context(),
    )
    .expect_err("must refuse");
    assert!(err.to_string().contains("quality"), "{err}");

    let mut with_luck = context();
    with_luck.luck = Some(3.0);
    let out = roll(&table, &LootTables::new(), &mut SeqRng::new(1), &with_luck).expect("rolls");
    assert_eq!(out.len(), 1);
}

#[test]
fn a_hostile_rng_returning_one_does_not_index_past_the_end() {
    // `next_f64` is documented to return `[0, 1)`, but the trait is public, so an
    // implementation can return 1.0. The selection must clamp rather than panic (AGENTS.md
    // section 9: no panic on hostile input, including a hostile trait impl).
    struct AlwaysOne;
    impl Rng for AlwaysOne {
        fn next_u32(&mut self) -> u32 {
            u32::MAX
        }
        fn next_f64(&mut self) -> f64 {
            1.0
        }
    }
    let table = table_with(
        vec![item("minecraft:first", 1), item("minecraft:second", 1)],
        1.0,
    );
    let out = roll(&table, &LootTables::new(), &mut AlwaysOne, &context()).expect("rolls");
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].item, id("minecraft:second"), "the last entry");
}

#[test]
fn an_empty_pool_yields_nothing_rather_than_panicking() {
    let table = table_with(Vec::new(), 4.0);
    let out = roll(
        &table,
        &LootTables::new(),
        &mut SeqRng::new(1),
        &context(),
    )
    .expect("rolls");
    assert!(out.is_empty());
}

#[test]
fn mentioned_types_counts_the_whole_tree() {
    let mut table = table_with(vec![item("minecraft:stone", 1)], 1.0);
    table.pools[0].entries[0] = LootEntry::Item {
        name: id("minecraft:stone"),
        weight: 1,
        quality: 0,
        expand: false,
        functions: vec![LootFunction::Raw(json!({"function": "minecraft:apply_bonus"}))],
        conditions: vec![LootCondition::SurvivesExplosion],
    };
    let types = table.mentioned_types();
    assert_eq!(types.get("minecraft:block").copied(), Some(1));
    assert_eq!(types.get("minecraft:item").copied(), Some(1));
    assert_eq!(types.get("minecraft:apply_bonus").copied(), Some(1));
    assert_eq!(types.get("minecraft:survives_explosion").copied(), Some(1));
    let unmodelled = table.unmodelled_types();
    assert_eq!(unmodelled.get("minecraft:apply_bonus").copied(), Some(1));
    assert!(
        !unmodelled.contains_key("minecraft:item"),
        "a modelled type must not appear as unmodelled"
    );
}

#[test]
fn mentioned_block_names_finds_blocks_inside_raw_conditions() {
    let mut table = table_with(vec![item("minecraft:wheat", 1)], 1.0);
    table.pools[0].conditions = vec![LootCondition::Raw(json!({
        "condition": "minecraft:block_state_property",
        "block": "minecraft:wheat",
        "properties": {"age": "7"}
    }))];
    let names = table.mentioned_block_names();
    assert!(names.contains("minecraft:wheat"), "{names:?}");
}

#[test]
fn a_report_that_lost_a_file_fails_the_accounting_check() {
    // The invariant the loader asserts, exercised directly so a regression in the loader is
    // caught by a fast unit test as well as by the differential one.
    let mut report = LootLoadReport {
        files: 3,
        loaded: 2,
        ..LootLoadReport::default()
    };
    assert!(!report.is_fully_accounted());
    report.skipped.push("one bad file".to_owned());
    assert!(report.is_fully_accounted());
    assert_eq!(report.total_unmodelled_files(), 0);
    report.unmodelled.insert("minecraft:unknown".to_owned(), 1);
    report.loaded -= 1;
    assert!(report.is_fully_accounted());
    assert!(!report.is_clean());
    assert_eq!(report.occurrences("minecraft:unknown"), 1);
}

#[test]
fn refusals_are_ordered_and_deduplicated() {
    let mut refusals = vec![
        Refusal {
            kind: "function",
            type_name: "minecraft:zeta".to_owned(),
            reason: "x",
        },
        Refusal {
            kind: "function",
            type_name: "minecraft:alpha".to_owned(),
            reason: "x",
        },
        Refusal {
            kind: "function",
            type_name: "minecraft:zeta".to_owned(),
            reason: "x",
        },
    ];
    refusals.sort();
    refusals.dedup();
    assert_eq!(refusals.len(), 2);
    assert_eq!(refusals[0].type_name, "minecraft:alpha");
}

#[test]
fn lookup_helpers_answer_deterministically() {
    let mut tables = LootTables::new();
    tables.insert(table_with(vec![item("minecraft:stone", 1)], 1.0));
    let mut chest = table_with(vec![item("minecraft:gold_ingot", 1)], 1.0);
    chest.name = Some(id("minecraft:chests/simple_dungeon"));
    chest.kind = "minecraft:chest".to_owned();
    tables.insert(chest);

    assert_eq!(tables.len(), 2);
    assert!(!tables.is_empty());
    let names: Vec<String> = tables.names().map(ToString::to_string).collect();
    assert_eq!(
        names,
        vec![
            "minecraft:chests/simple_dungeon".to_owned(),
            "minecraft:test".to_owned()
        ],
        "names are ascending"
    );
    assert_eq!(tables.of_kind("minecraft:chest").len(), 1);
    assert!(tables.by_name(&id("minecraft:chests/simple_dungeon")).is_some());
    assert!(tables.by_name(&id("minecraft:nope")).is_none());
    assert!(tables
        .referencing(&id("minecraft:test"))
        .is_empty());
}

#[test]
fn an_override_replaces_rather_than_appends() {
    let mut tables = LootTables::new();
    tables.insert(table_with(vec![item("minecraft:stone", 1)], 1.0));
    tables.insert(table_with(vec![item("minecraft:dirt", 1)], 1.0));
    assert_eq!(tables.len(), 1, "the second file overrides the first");
    let out = tables
        .roll_named(&id("minecraft:test"), &mut SeqRng::new(1), &context())
        .expect("rolls");
    assert_eq!(out[0].item, id("minecraft:dirt"));
}

#[test]
fn an_unnamed_table_is_not_stored_in_the_registry() {
    // An inline table has no key, so storing it would make it unreachable *and* make `len()`
    // disagree with `names()`.
    let mut tables = LootTables::new();
    let mut unnamed = table_with(vec![item("minecraft:stone", 1)], 1.0);
    unnamed.name = None;
    tables.insert(unnamed);
    assert_eq!(tables.len(), 0);
    assert_eq!(tables.names().count(), 0);
    assert!(tables.is_empty());
}

#[test]
fn rolling_a_name_that_is_not_loaded_is_an_error() {
    let tables = LootTables::new();
    let err = tables
        .roll_named(&id("minecraft:nope"), &mut SeqRng::new(1), &context())
        .expect_err("must fail");
    assert!(matches!(err, RollError::UnknownTable { .. }), "{err}");
    assert!(err.to_string().contains("minecraft:nope"), "{err}");
}

#[test]
fn the_nesting_limit_is_reported_for_a_long_chain() {
    // A chain longer than MAX_TABLE_NESTING, built by wiring N tables in a line.
    let mut tables = LootTables::new();
    let depth = MAX_TABLE_NESTING + 4;
    for level in 0..depth {
        let target = format!("minecraft:t{}", level + 1);
        let table = LootTable {
            name: Some(id(&format!("minecraft:t{level}"))),
            kind: "minecraft:chest".to_owned(),
            random_sequence: None,
            pools: vec![LootPool {
                rolls: CountProvider::Constant(1.0),
                bonus_rolls: CountProvider::Constant(0.0),
                entries: vec![LootEntry::LootTable {
                    name: Some(id(&target)),
                    inline: None,
                    weight: 1,
                    quality: 0,
                    conditions: Vec::new(),
                }],
                conditions: Vec::new(),
                functions: Vec::new(),
            }],
            functions: Vec::new(),
        };
        tables.insert(table);
    }
    tables.insert(table_with(vec![item("minecraft:stone", 1)], 1.0));
    let start = tables
        .by_name(&id("minecraft:t0"))
        .expect("the chain start");
    let err = roll(start, &tables, &mut SeqRng::new(1), &context()).expect_err("must refuse");
    assert!(
        matches!(err, RollError::TooDeep { .. } | RollError::UnknownTable { .. }),
        "{err}"
    );
}

#[test]
fn the_rng_trait_default_methods_stay_in_range() {
    let mut rng = SeqRng::new(99);
    for _ in 0..1_000 {
        let single = rng.next_f32();
        assert!((0.0..1.0).contains(&single), "{single}");
        let double = rng.next_f64();
        assert!((0.0..1.0).contains(&double), "{double}");
    }
}

#[test]
fn chance_is_strict_at_the_boundaries() {
    // `next_f32() < chance`, which is what vanilla does. A generator that can return exactly
    // 0.0 must therefore fail a 0.0 chance.
    struct Zero;
    impl Rng for Zero {
        fn next_u32(&mut self) -> u32 {
            0
        }
    }
    let mut rng = Zero;
    assert!(!rng.chance(0.0));
    assert!(rng.chance(1.0));
    assert!(rng.chance(f32::MIN_POSITIVE));
}

