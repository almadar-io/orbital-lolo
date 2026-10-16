//! Formatter round-trip and snapshot tests.
//!
//! Every non-error fixture is parsed, formatted, and re-parsed. The lowered
//! `.orb` JSON of the re-parsed output must match the lowered JSON of the
//! original, proving the formatter is semantics-preserving.

use orbital_lolo::{format_program, parse_and_lower_str, parse_str};
use serde_json::Value;
use std::fs;
use std::path::PathBuf;

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

// Drop V4 identity fields: opaque per-lowering ids + ledger + schemaVersion.
// These tests assert *semantic* preservation across a reformat; ids are
// minted fresh each (independent) lowering and are not semantic content.
fn is_identity_key(k: &str) -> bool {
    k == "id" || k == "ledger" || k == "schemaVersion" || k.ends_with("Id") || k.ends_with("Ids")
}

fn strip_identity_keys(value: Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut out = serde_json::Map::new();
            for (k, v) in map {
                if is_identity_key(&k) {
                    continue;
                }
                out.insert(k, strip_identity_keys(v));
            }
            Value::Object(out)
        }
        Value::Array(items) => Value::Array(items.into_iter().map(strip_identity_keys).collect()),
        v => v,
    }
}

/// Strips identity fields, then sorts every object's keys via
/// `orbital_core::util::canonical_json` (the one production owner) so two
/// independently-lowered values compare deterministically.
fn canonicalize(value: Value) -> Value {
    orbital_core::util::canonical_json(&strip_identity_keys(value))
}

fn strip_nulls(value: Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut out = serde_json::Map::new();
            for (k, v) in map {
                let stripped = strip_nulls(v);
                if !stripped.is_null() {
                    out.insert(k, stripped);
                }
            }
            Value::Object(out)
        }
        Value::Array(items) => Value::Array(items.into_iter().map(strip_nulls).collect()),
        v => v,
    }
}

fn assert_round_trip(name: &str) {
    let dir = fixtures_dir();
    let lolo_path = dir.join(format!("{name}.lolo"));
    let source = fs::read_to_string(&lolo_path).expect("read lolo");

    let ast = parse_str(&source).expect("parse original");
    let formatted = format_program(&ast);
    let _ast2 = parse_str(&formatted).expect("parse formatted");

    let schema1 = parse_and_lower_str(&source).expect("lower original");
    let schema2 = parse_and_lower_str(&formatted).expect("lower formatted");

    let v1 = canonicalize(strip_nulls(serde_json::to_value(&schema1).unwrap()));
    let v2 = canonicalize(strip_nulls(serde_json::to_value(&schema2).unwrap()));

    if v1 != v2 {
        // Write the formatted output next to the fixture for debugging.
        let debug_path = dir.join(format!("{name}.formatted.debug.lolo"));
        fs::write(&debug_path, &formatted).ok();
        panic!(
            "Formatter round-trip failed for {}. Debug output written to {}",
            name,
            debug_path.display()
        );
    }
}

// Keep in sync with tests/lower.rs — every non-ignored fixture gets a
// round-trip test.

// Large real-registry fixtures (atom / scanner / molecule / organism) — exercise
// the printer over the full annotation surface (entity-field @description/@synonyms,
// config @label/@tier, events, ticks, sexpr filters, uses + trait-ref overrides,
// version quoting) on production-sized inputs. NOTE: these verify *semantic*
// preservation (lowered .orb equality); `;;` comments are dropped at the lexer and
// are out of scope for the round-trip.
#[test]
fn large_01_modal_atom() {
    assert_round_trip("large-01-modal-atom");
}

#[test]
fn large_02_lifecycle_scanner() {
    assert_round_trip("large-02-lifecycle-scanner");
}

#[test]
fn large_03_list_molecule() {
    assert_round_trip("large-03-list-molecule");
}

#[test]
fn large_04_generic_app_organism() {
    assert_round_trip("large-04-generic-app-organism");
}

// Sexpr edge cases that broke the formatter on the io registry: reserved-word
// string values (`"now"`, `"true"`/`"false"` data arrays — bare-printing them
// flips them to the `@now` binding / a boolean literal / a call), and a sigil
// carrying an array index (`@config.sections[0].bullets`) that can't print bare.
#[test]
fn large_05_sexpr_reserved_words() {
    assert_round_trip("large-05-sexpr-reserved-words");
}

#[test]
fn large_06_sexpr_bool_array() {
    assert_round_trip("large-06-sexpr-bool-array");
}

#[test]
fn large_07_sexpr_sigil_index() {
    assert_round_trip("large-07-sexpr-sigil-index");
}

/// Comment preservation: every `;;` comment in the source must survive into the
/// formatted output verbatim (the round-trip test above can't catch this, since
/// comments never reach the `.orb`). Covers a comment-heavy atom and the big
/// organism. Also asserts idempotency over the comment-bearing output.
#[test]
fn comments_preserved_in_large_fixtures() {
    let dir = fixtures_dir();
    for name in ["large-01-modal-atom", "large-04-generic-app-organism"] {
        let source = fs::read_to_string(dir.join(format!("{name}.lolo"))).expect("read lolo");
        let ast = parse_str(&source).expect("parse original");
        let formatted = format_program(&ast);

        let src_comments: Vec<String> = source
            .lines()
            .map(|l| l.trim().to_string())
            .filter(|l| l.starts_with(";;") && l.len() > 2)
            .collect();
        assert!(
            !src_comments.is_empty(),
            "{name}: fixture has no ;; comments to test"
        );
        for c in &src_comments {
            assert!(
                formatted.contains(c.as_str()),
                "{name}: comment dropped by formatter: {c:?}"
            );
        }

        // Idempotent over comments too: re-formatting keeps them.
        let formatted2 = format_program(&parse_str(&formatted).expect("parse formatted"));
        for c in &src_comments {
            assert!(
                formatted2.contains(c.as_str()),
                "{name}: comment lost on second format: {c:?}"
            );
        }
    }
}

#[test]
fn fixture_01_cart_derived() {
    assert_round_trip("01-cart-derived");
}

#[test]
fn fixture_02_multi_state_machine() {
    assert_round_trip("02-multi-state-machine");
}

#[test]
fn fixture_03_modal_close_cancel() {
    assert_round_trip("03-modal-close-cancel");
}

#[test]
fn fixture_04_render_ui_pattern() {
    assert_round_trip("04-render-ui-pattern");
}

#[test]
fn fixture_05_behavior_composition() {
    assert_round_trip("05-behavior-composition");
}

#[test]
fn fixture_06_type_aliases() {
    assert_round_trip("06-type-aliases");
}

#[test]
fn fixture_07_time_bounded_state() {
    assert_round_trip("07-time-bounded-state");
}

#[test]
fn fixture_08_listens_cross_trait() {
    assert_round_trip("08-listens-cross-trait");
}

#[test]
fn fixture_09_derived_array_ops() {
    assert_round_trip("09-derived-array-ops");
}

#[test]
fn fixture_10_persist_crud() {
    assert_round_trip("10-persist-crud");
}

#[test]
fn fixture_11_fetch_with_config() {
    assert_round_trip("11-fetch-with-config");
}

#[test]
fn fixture_12_navigate_with_params() {
    assert_round_trip("12-navigate-with-params");
}

#[test]
fn fixture_13_notify_with_config() {
    assert_round_trip("13-notify-with-config");
}

#[test]
fn fixture_14_emit_with_payload() {
    assert_round_trip("14-emit-with-payload");
}

/// Page annotations (`@icon`/`@label`) survive a parse → format → re-parse →
/// re-lower round-trip with identical lowered `.orb`. Proves the annotation is
/// carried through AST → formatter → re-parse → lower without loss.
#[test]
fn page_annotations_round_trip() {
    let src = "orbital O {\n  entity E { id : string! }\n  trait T -> E { state s { I -> s } }\n  page \"/foo\" as FooPage -> T\n    @icon \"users\"\n    @label \"Foo\"\n}";
    let ast = parse_str(src).expect("parse original");
    let formatted = format_program(&ast);
    // The annotation surface must appear in the formatted output.
    assert!(
        formatted.contains("@icon \"users\""),
        "@icon missing: {formatted}"
    );
    assert!(
        formatted.contains("@label \"Foo\""),
        "@label missing: {formatted}"
    );
    let schema1 = parse_and_lower_str(src).expect("lower original");
    let schema2 = parse_and_lower_str(&formatted).expect("lower formatted");
    let v1 = canonicalize(strip_nulls(serde_json::to_value(&schema1).unwrap()));
    let v2 = canonicalize(strip_nulls(serde_json::to_value(&schema2).unwrap()));
    assert_eq!(v1, v2, "page annotation lost in round-trip");
}

#[test]
fn fixture_15_agent_invocation() {
    assert_round_trip("15-agent-invocation");
}

#[test]
fn fixture_16_config_required() {
    assert_round_trip("16-config-required");
}

#[test]
fn fixture_17_type_entity_decl() {
    assert_round_trip("17-type-entity-decl");
}

#[test]
fn fixture_19_type_composition() {
    assert_round_trip("19-type-composition");
}

#[test]
fn fixture_20_type_annotation() {
    assert_round_trip("20-type-annotation");
}

#[test]
fn fixture_23_entity_composition_typed() {
    assert_round_trip("23-entity-composition-typed");
}

#[test]
fn fixture_24_trait_binding() {
    assert_round_trip("24-trait-binding");
}

#[test]
fn fixture_25_emit_config() {
    assert_round_trip("25-emit-config");
}

#[test]
fn fixture_26_typed_payload_form_a() {
    assert_round_trip("26-typed-payload-form-a");
}

#[test]
fn fixture_27_typed_payload_form_b() {
    assert_round_trip("27-typed-payload-form-b");
}

#[test]
fn fixture_28_trait_instance_modifier() {
    assert_round_trip("28-trait-instance-modifier");
}

#[test]
fn fixture_29_event_type_internal() {
    assert_round_trip("29-event-type-internal");
}

#[test]
fn fixture_30_event_type_external() {
    assert_round_trip("30-event-type-external");
}

#[test]
fn fixture_31_event_type_anonymous_payload() {
    assert_round_trip("31-event-type-anonymous-payload");
}

#[test]
fn fixture_32_inline_trait_ref() {
    // The formatter must preserve `@(...)` inline trait-reference sugar rather
    // than expanding it into synthesized top-level `trait` declarations.
    // Re-parsing the formatted output must still lower to the same schema
    // (semantics-preserving).
    assert_round_trip("32-inline-trait-ref");

    let dir = fixtures_dir();
    let source = fs::read_to_string(dir.join("32-inline-trait-ref.lolo")).expect("read lolo");
    let ast = parse_str(&source).expect("parse");
    let formatted = format_program(&ast);
    // JSX is now the canonical render surface: inline trait refs format as
    // `<Alias.traits.X …>` and named `@trait.X` refs as `<trait.X />`.
    assert!(
        formatted.contains("<UiSpinner.traits.SpinnerRender"),
        "formatter must emit JSX for inline trait refs; got:\n{formatted}"
    );
    assert!(
        formatted.contains("<Stack"),
        "formatter must emit JSX for render pattern objects; got:\n{formatted}"
    );
    assert!(
        !formatted.contains("trait Inline"),
        "formatter must not emit synthesized inline trait declarations; got:\n{formatted}"
    );
}

/// Consistency cleanup (docs/Almadar_LOLO_Audit.md): the permissive forms —
/// optional colon after block keywords (`initial idle`, `emits:`/`listens:`),
/// optional comma in trait-modifier (`[interaction collection]`) and page-trait
/// (`-> V W`) lists — must lower to exactly the same schema as the canonical
/// comma+`initial:`+colonless form. Proves the fixes are non-breaking sugar.
#[test]
fn consistency_optional_colon_and_comma_forms() {
    let permissive = r#"app t "1.0.0"
orbital O {
  entity Item [runtime] {
    id : string!
  }
  trait V -> Item [interaction collection] {
    initial idle
    state idle {
      INIT -> idle (render-ui main { type: "box" })
    }
    emits: { PING { } }
    listens: { PING { } }
  }
  page "/a" as A -> V V
}
"#;
    let canonical = r#"app t "1.0.0"
orbital O {
  entity Item [runtime] {
    id : string!
  }
  trait V -> Item [interaction, collection] {
    initial: idle
    state idle {
      INIT -> idle (render-ui main { type: "box" })
    }
    emits { PING { } }
    listens { PING { } }
  }
  page "/a" as A -> V, V
}
"#;
    let a = parse_and_lower_str(permissive).expect("permissive lowers");
    let b = parse_and_lower_str(canonical).expect("canonical lowers");
    let va = canonicalize(strip_nulls(serde_json::to_value(&a).unwrap()));
    let vb = canonicalize(strip_nulls(serde_json::to_value(&b).unwrap()));
    assert_eq!(
        va, vb,
        "permissive optional-colon/comma forms must lower identically to canonical"
    );
}

/// The `[identity]` tag must survive a reformat. `fmt_program` is a whole-AST
/// reprint, so a tag the formatter does not re-emit is DELETED from the author's
/// source on the next `orb fmt` — this test is the guard for that class.
#[test]
fn identity_tag_survives_reformat() {
    let source = r#"app test-identity "1.0.0"
"Identity tag round-trip."
orbital PersonOrbital {
  entity Person [persistent: people, identity] {
    id : string!
    name : string!
    role : "clinician" | "frontdesk" | "patient" = clinician
  }
  trait V -> Person [interaction, collection] {
    initial: idle
    state idle {
      INIT -> idle (render-ui main { type: "box" })
    }
  }
  page "/a" as A -> V
}
"#;
    let ast = parse_str(source).expect("parse original");
    let formatted = format_program(&ast);
    assert!(
        formatted.contains("[persistent: people, identity]"),
        "formatter dropped the identity tag; got:\n{formatted}"
    );

    let schema1 = parse_and_lower_str(source).expect("lower original");
    let schema2 = parse_and_lower_str(&formatted).expect("lower formatted");
    let v1 = canonicalize(strip_nulls(serde_json::to_value(&schema1).unwrap()));
    let v2 = canonicalize(strip_nulls(serde_json::to_value(&schema2).unwrap()));
    assert_eq!(v1, v2, "identity tag changed semantics across a reformat");

    let entity = schema1.orbitals[0]
        .entity
        .as_inline()
        .expect("inline entity");
    assert!(entity.identity, "identity flag did not reach the IR");
    assert!(!entity.shared, "identity must NOT imply shared");
}

/// The four entity access directives (`@read`/`@create`/`@update`/`@delete`)
/// survive a reformat with unchanged semantics.
#[test]
fn access_directives_survive_reformat() {
    let source = r#"app test-access "1.0.0"
"Access directive round-trip."
orbital TicketOrbital {
  entity Ticket [persistent: tickets] {
    @read (or ["=", @user.role, "supervisor"] ["=", @entity.assignee, @user.id])
    @create ["=", @user.role, "supervisor"]
    @update ["=", @entity.assignee, @user.id]
    @delete ["=", @user.role, "supervisor"]
    id : string!
    assignee : string!
  }
  trait V -> Ticket [interaction, collection] {
    initial: idle
    state idle {
      INIT -> idle (render-ui main { type: "box" })
    }
  }
  page "/a" as A -> V
}
"#;
    let ast = parse_str(source).expect("parse original");
    let formatted = format_program(&ast);
    for sigil in ["@read", "@create", "@update", "@delete"] {
        assert!(
            formatted.contains(sigil),
            "formatter dropped `{sigil}`; got:\n{formatted}"
        );
    }

    let schema1 = parse_and_lower_str(source).expect("lower original");
    let schema2 = parse_and_lower_str(&formatted).expect("lower formatted");
    let v1 = canonicalize(strip_nulls(serde_json::to_value(&schema1).unwrap()));
    let v2 = canonicalize(strip_nulls(serde_json::to_value(&schema2).unwrap()));
    assert_eq!(
        v1, v2,
        "access directives changed semantics across a reformat"
    );

    let ticket = schema1.orbitals[0]
        .entity
        .as_inline()
        .expect("inline entity");
    assert!(
        ticket.read_policy.is_some(),
        "read_policy did not reach the IR"
    );
    assert!(
        ticket.create_policy.is_some(),
        "create_policy did not reach the IR"
    );
    assert!(
        ticket.update_policy.is_some(),
        "update_policy did not reach the IR"
    );
    assert!(
        ticket.delete_policy.is_some(),
        "delete_policy did not reach the IR"
    );
}

/// A duplicate directive of the same kind is a parse error.
#[test]
fn duplicate_access_directive_is_rejected() {
    let source = r#"app test-access-dup "1.0.0"
"Duplicate directive rejection."
orbital TicketOrbital {
  entity Ticket [persistent: tickets] {
    @read ["=", @user.id, @entity.assignee]
    @read ["=", @user.id, @entity.assignee]
    id : string!
    assignee : string!
  }
}
"#;
    let err = parse_str(source).expect_err("duplicate @read must be a parse error");
    let message = format!("{err}");
    assert!(
        message.contains("ELOLO_DUPLICATE_ACCESS_DIRECTIVE"),
        "unexpected error: {message}"
    );
}

/// `[identity]` is orthogonal to both persistence and `shared`, in any order.
#[test]
fn identity_composes_with_runtime_and_shared() {
    for tag in [
        "[runtime, identity]",
        "[identity, runtime]",
        "[persistent: people, identity, shared]",
        "[identity]",
    ] {
        let source = format!(
            r#"app test-identity "1.0.0"
"Identity tag composition."
orbital PersonOrbital {{
  entity Person {tag} {{
    id : string!
  }}
  trait V -> Person [interaction, collection] {{
    initial: idle
    state idle {{
      INIT -> idle (render-ui main {{ type: "box" }})
    }}
  }}
  page "/a" as A -> V
}}
"#
        );
        let schema =
            parse_and_lower_str(&source).unwrap_or_else(|e| panic!("{tag} must parse: {e:?}"));
        let entity = schema.orbitals[0]
            .entity
            .as_inline()
            .expect("inline entity");
        assert!(entity.identity, "{tag} did not set identity");
        assert_eq!(
            entity.shared,
            tag.contains("shared"),
            "{tag} got the wrong shared flag"
        );
    }
}

#[test]
fn page_annotation_lands_in_orb() {
    let src = "orbital O {\n  entity E { id : string! }\n  trait T -> E { state s { I -> s } }\n  page \"/foo\" as FooPage -> T\n    @icon \"users\"\n    @label \"Foo\"\n}";
    let schema = parse_and_lower_str(src).expect("lower");
    let v = serde_json::to_value(&schema).unwrap();
    let page = &v["orbitals"][0]["pages"][0];
    assert_eq!(page["icon"], serde_json::json!("users"));
    assert_eq!(page["label"], serde_json::json!("Foo"));
}

#[test]
fn page_roles_annotation_lands_in_orb_and_round_trips() {
    let src = "orbital O {\n  entity E { id : string! }\n  trait T -> E { state s { I -> s } }\n  page \"/foo\" as FooPage -> T\n    @label \"Foo\"\n    @roles \"manager, owner\"\n}";
    let schema = parse_and_lower_str(src).expect("lower");
    let v = serde_json::to_value(&schema).unwrap();
    assert_eq!(
        v["orbitals"][0]["pages"][0]["roles"],
        serde_json::json!(["manager", "owner"])
    );
    let formatted = format_program(&parse_str(src).expect("parse"));
    assert!(
        formatted.contains("@roles \"manager, owner\""),
        "{formatted}"
    );
    let again = parse_and_lower_str(&formatted).expect("lower formatted");
    assert_eq!(
        canonicalize(strip_nulls(v)),
        canonicalize(strip_nulls(serde_json::to_value(&again).unwrap()))
    );
}

#[test]
fn control_a_page_without_roles_carries_none() {
    let src = "orbital O {\n  entity E { id : string! }\n  trait T -> E { state s { I -> s } }\n  page \"/foo\" as FooPage -> T\n    @label \"Foo\"\n}";
    let v = serde_json::to_value(parse_and_lower_str(src).expect("lower")).unwrap();
    assert!(v["orbitals"][0]["pages"][0].get("roles").is_none());
}

#[test]
fn a_page_reference_carries_its_nav_annotations() {
    let src = "uses Up from \"./up\"\norbital O {\n  entity E { id : string! }\n  trait T -> E { state s { I -> s } }\n  page \"/foo\" = Up.pages.FooPage -> T\n    @label \"Foo\"\n    @icon \"star\"\n    @roles \"staff\"\n}";
    let v = serde_json::to_value(parse_and_lower_str(src).expect("lower")).unwrap();
    let page = &v["orbitals"][0]["pages"][0];
    assert_eq!(page["label"], serde_json::json!("Foo"), "{page}");
    assert_eq!(page["icon"], serde_json::json!("star"), "{page}");
    assert_eq!(page["roles"], serde_json::json!(["staff"]), "{page}");
}

/// `expects` declarations survive a reformat with unchanged semantics, print
/// after the `uses` group with one blank line between the groups, and re-format
/// to a stable output (idempotent).
#[test]
fn expects_survive_reformat() {
    let source = r#"app test-expects "1.0.0"
"Expects round-trip."
orbital ProductOrbital {
  uses AppShell from "std/behaviors/std-app-layout"
  expects identity Customer {
    role : "store-manager" | "operations-director"
  }
  expects entity OrderRecord {
    status : "placed" | "shipped" | "delivered"
    totalAmount : number
  }
  expects entity CartItem
  expects event Checkout.ORDER_PLACED
  entity Product [persistent: products] {
    id : string!
    title : string!
  }
  trait V -> Product [interaction, collection] {
    initial: idle
    state idle {
      INIT -> idle (render-ui main { type: "box" })
    }
  }
  page "/a" as A -> V
}
"#;
    let ast = parse_str(source).expect("parse original");
    let formatted = format_program(&ast);
    for needle in [
        "expects identity Customer {",
        "expects entity OrderRecord {",
        "expects entity CartItem",
        "expects event Checkout.ORDER_PLACED",
    ] {
        assert!(
            formatted.contains(needle),
            "formatter dropped `{needle}`; got:\n{formatted}"
        );
    }
    // Layout: expects group after the uses group, one blank line between.
    let uses_pos = formatted.find("uses AppShell").expect("uses printed");
    let expects_pos = formatted.find("expects identity").expect("expects printed");
    assert!(uses_pos < expects_pos, "expects must print after uses");
    assert!(
        formatted[uses_pos..expects_pos].contains("\n\n"),
        "expected one blank line between the uses and expects groups; got:\n{formatted}"
    );

    let schema1 = parse_and_lower_str(source).expect("lower original");
    let schema2 = parse_and_lower_str(&formatted).expect("lower formatted");
    let v1 = canonicalize(strip_nulls(serde_json::to_value(&schema1).unwrap()));
    let v2 = canonicalize(strip_nulls(serde_json::to_value(&schema2).unwrap()));
    assert_eq!(v1, v2, "expects changed semantics across a reformat");

    let expects = schema1.orbitals[0].expects.as_ref().expect("expects in IR");
    assert_eq!(expects.len(), 4);

    // Idempotent: formatting the formatted output changes nothing.
    let formatted2 = format_program(&parse_str(&formatted).expect("parse formatted"));
    assert_eq!(
        formatted, formatted2,
        "formatter is not idempotent over expects"
    );
}

// Orbital-import Stage A (docs/Almadar_Orbital_Import.md §4) round-trips:
// app-level `config {}` + file-level `uses { config {} }`, an orbital
// reference with every override block populated, and an inline orbital's
// own `config {}`. Kept in sync with tests/lower.rs (module doc requires a
// golden-lowering test for each).
#[test]
fn orbital_import_app_config_round_trips() {
    assert_round_trip("orbital-import-app-config");
}

#[test]
fn orbital_import_reference_full_round_trips() {
    assert_round_trip("orbital-import-reference-full");
}

#[test]
fn orbital_import_inline_config_round_trips() {
    assert_round_trip("orbital-import-inline-config");
}

// Orbital-import Stage B (docs/Almadar_Orbital_Import.md §4, plan
// tender-booping-moler.md B1-R) round-trips: every override block —
// `entity` / `entities` / `fields` / `roles` (single + list forms) /
// `pages` with a mount tail / `omit` / `config` / `events` — plus two
// sibling `trait` declarations, one of them mounted. Kept in sync with
// tests/lower.rs.
#[test]
fn orbital_ref_stage_b_round_trips() {
    assert_round_trip("orbital_ref_stage_b");
}

/// `extend { … }` (Stage C) — printed right after `fields {}`, full
/// entity-field syntax including a relation-typed field with a `@label`.
#[test]
fn orbital_ref_extend_round_trips() {
    assert_round_trip("orbital_ref_extend");
}

// `children: []` (an explicit `<Tag></Tag>` body) vs an absent `children`
// key (`<Tag />`) — the formatter must print the FORMER as `<Tag></Tag>`,
// never collapse it to self-closing, or the re-parse silently drops the
// explicit empty children array. Covers both a plain pattern element and an
// inline-ref trait, one of each closing form. Kept in sync with
// tests/lower.rs.
#[test]
fn jsx_empty_children_round_trips() {
    assert_round_trip("jsx_empty_children");
}

#[test]
fn fixture_34_cancel_call() {
    assert_round_trip("34-cancel-call");
}

/// A union nested in a `Map` value or an array element is written with parentheses:
/// without them `Map string string | number` re-reads as a union of a map and a number.
#[test]
fn a_union_nested_in_a_map_or_list_keeps_its_grouping_through_the_formatter() {
    let src = r#"
orbital OrbG {
  entity Rec [persistent: recs] {
    id : string!
    bag : Map string (string | number)
    list : [(string | number)]
  }
  trait RecTrait -> Rec [interaction] { state idle { INIT -> idle (ref Rec) } }
  page "/recs" -> RecTrait
}
"#;
    let program = parse_str(src).expect("parenthesized types parse");
    let formatted = format_program(&program);
    assert!(
        formatted.contains("Map string (string | number)"),
        "{formatted}"
    );
    assert!(formatted.contains("[(string | number)]"), "{formatted}");
    let again = parse_str(&formatted).expect("the formatted text parses");
    assert_eq!(format_program(&again), formatted);
    let before = parse_and_lower_str(src).expect("lowers");
    let after = parse_and_lower_str(&formatted).expect("lowers again");
    assert_eq!(
        strip_identity_keys(serde_json::to_value(&before).unwrap()),
        strip_identity_keys(serde_json::to_value(&after).unwrap())
    );
}

#[test]
fn fetch_deliver_and_total_lower_to_the_ir_and_survive_the_formatter() {
    let src = r#"
orbital T {
  entity Item [runtime] { id : string! }
  trait R -> Item [interaction] {
    initial: idle
    state idle {
      INIT -> idle
        (fetch Item { deliver: ids, total: none, emit: { success: Loaded } })
    }
  }
  page "/" -> R
}
"#;
    let lowered = serde_json::to_value(parse_and_lower_str(src).expect("lowers")).unwrap();
    let fetch =
        &lowered["orbitals"][0]["traits"][0]["stateMachine"]["transitions"][0]["effects"][0];
    assert_eq!(fetch[2]["deliver"], "ids", "{fetch}");
    assert_eq!(fetch[2]["total"], "none", "{fetch}");

    let formatted = format_program(&parse_str(src).expect("parses"));
    assert!(formatted.contains("deliver: ids"), "{formatted}");
    assert!(formatted.contains("total: none"), "{formatted}");
    let again =
        serde_json::to_value(parse_and_lower_str(&formatted).expect("lowers again")).unwrap();
    assert_eq!(
        canonicalize(strip_nulls(lowered)),
        canonicalize(strip_nulls(again))
    );
}

/// A shape field name may be a quoted string (`"@href"`, `"#text"`): the keys `xml/parse` writes.
mod quoted_field_names {
    use super::*;

    const BASE: &str = r#"app qf [
      version: "1.0.0",
      description: "x"
    ]
    "#;

    fn program(types: &str) -> String {
        format!(
            "{BASE}{types}\norbital O {{\n  entity E [runtime] {{\n    id : string!\n    link : Link\n  }}\n  trait T -> E [interaction, instance] {{\n    initial: idle\n    state idle {{\n      INIT -> idle\n    }}\n  }}\n  page \"/\" -> T\n}}\n"
        )
    }

    fn lowered_link_names(source: &str) -> Vec<String> {
        let schema = parse_and_lower_str(source).expect("lowers");
        let value = serde_json::to_value(&schema).unwrap();
        let fields = value["orbitals"][0]["entity"]["fields"].as_array().unwrap();
        let link = fields.iter().find(|f| f["name"] == "link").unwrap();
        link["properties"]
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect()
    }

    #[test]
    fn a_quoted_field_name_is_stored_verbatim() {
        let src =
            program(r##"type Link = { "@href" : string, "@rel" : string, "#text" : string }"##);
        let mut names = lowered_link_names(&src);
        names.sort();
        assert_eq!(names, vec!["#text", "@href", "@rel"]);
    }

    #[test]
    fn a_quoted_name_and_a_bare_name_mix_in_one_shape() {
        let src = program(r##"type Link = { "@href" : string!, title : string }"##);
        let mut names = lowered_link_names(&src);
        names.sort();
        assert_eq!(names, vec!["@href", "title"]);
    }

    #[test]
    fn a_quoted_name_may_hold_characters_no_identifier_can() {
        let src = program(r##"type Link = { "xlink:href" : string, "a b" : string }"##);
        let mut names = lowered_link_names(&src);
        names.sort();
        assert_eq!(names, vec!["a b", "xlink:href"]);
    }

    #[test]
    fn an_inline_object_type_accepts_a_quoted_name() {
        let src = format!(
            "{BASE}orbital O {{\n  entity E [runtime] {{\n    id : string!\n    link : {{ \"@href\" : string }}\n  }}\n  trait T -> E [interaction, instance] {{\n    initial: idle\n    state idle {{\n      INIT -> idle\n    }}\n  }}\n  page \"/\" -> T\n}}\n"
        );
        assert_eq!(lowered_link_names(&src), vec!["@href"]);
    }

    #[test]
    fn the_formatter_prints_a_quoted_name_back_quoted_and_round_trips() {
        let src = program(r##"type Link = { "@href" : string, rel : string, "#text" : string }"##);
        let formatted = format_program(&parse_str(&src).expect("parse"));
        assert!(formatted.contains(r#""@href" : string"#), "{formatted}");
        assert!(formatted.contains(r##""#text" : string"##), "{formatted}");
        assert!(
            formatted.contains("rel : string"),
            "bare names stay bare: {formatted}"
        );
        let mut first = lowered_link_names(&src);
        let mut second = lowered_link_names(&formatted);
        first.sort();
        second.sort();
        assert_eq!(first, second);
    }

    #[test]
    fn an_empty_quoted_name_is_an_error() {
        let src = program(r#"type Link = { "" : string }"#);
        let err = parse_str(&src).expect_err("empty name");
        assert!(
            format!("{err}").contains("ELOLO_SHAPE_FIELD_NAME_EMPTY"),
            "{err}"
        );
    }

    #[test]
    fn a_duplicate_quoted_name_is_an_error() {
        let src = program(r#"type Link = { "@href" : string, "@href" : string }"#);
        let err = parse_str(&src).expect_err("duplicate");
        assert!(
            format!("{err}").contains("ELOLO_SHAPE_FIELD_NAME_DUPLICATE"),
            "{err}"
        );
    }

    #[test]
    fn a_quoted_name_that_repeats_a_bare_name_is_an_error() {
        let src = program(r#"type Link = { href : string, "href" : string }"#);
        let err = parse_str(&src).expect_err("duplicate");
        assert!(
            format!("{err}").contains("ELOLO_SHAPE_FIELD_NAME_DUPLICATE"),
            "{err}"
        );
    }

    #[test]
    fn control_distinct_quoted_names_are_accepted() {
        let src = program(r#"type Link = { "@href" : string, "@rel" : string }"#);
        assert!(parse_str(&src).is_ok());
    }
}

/// `xml/parse` over a shape that declares list fields lowers with the list paths as its third argument.
mod xml_parse_list_paths {
    use super::*;

    fn program(shape: &str) -> String {
        format!(
            "app xl [\n  version: \"1.0.0\",\n  description: \"x\"\n]\ntype Author = {{ name : string }}\ntype Entry = {{ id : string, author : [Author] }}\ntype Feed = {{ title : string, entry : [Entry] }}\ntype Row = {{ id : string, title : string }}\norbital O {{\n  entity E [runtime] {{\n    id : string!\n    body : string\n    out : string\n  }}\n  trait T -> E [interaction, instance] {{\n    initial: idle\n    state idle {{\n      INIT -> idle\n        (set @entity.out (object/get (xml/parse \"<a/>\" {shape}) title \"\"))\n    }}\n  }}\n  page \"/\" -> T\n}}\n"
        )
    }

    fn parse_call(shape: &str) -> Value {
        let lowered =
            serde_json::to_value(parse_and_lower_str(&program(shape)).expect("lowers")).unwrap();
        lowered["orbitals"][0]["traits"][0]["stateMachine"]["transitions"][0]["effects"][0][2][1]
            .clone()
    }

    #[test]
    fn a_shape_with_list_fields_carries_every_list_path() {
        assert_eq!(
            parse_call("Feed"),
            serde_json::json!([
                "xml/parse",
                "<a/>",
                "Feed",
                ["list", ["list", "entry"], ["list", "entry", "author"]]
            ])
        );
    }

    #[test]
    fn unlowering_drops_the_baked_paths_and_relowering_bakes_them_again() {
        let schema = parse_and_lower_str(&program("Feed")).expect("lowers");
        let lolo = orbital_lolo::schema_to_lolo(&schema).expect("unlowers");
        assert!(lolo.contains("(xml/parse \"<a/>\" Feed)"), "{lolo}");
        let relowered = serde_json::to_value(parse_and_lower_str(&lolo).expect("relowers")).unwrap();
        assert_eq!(relowered, serde_json::to_value(&schema).unwrap());
    }

    #[test]
    fn control_a_shape_without_list_fields_stays_two_arguments() {
        assert_eq!(
            parse_call("Row"),
            serde_json::json!(["xml/parse", "<a/>", "Row"])
        );
    }
}
