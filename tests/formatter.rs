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

fn canonicalize(value: Value) -> Value {
    // Drop V4 identity fields: opaque per-lowering ids + ledger + schemaVersion.
    // These tests assert *semantic* preservation across a reformat; ids are
    // minted fresh each (independent) lowering and are not semantic content.
    fn is_identity_key(k: &str) -> bool {
        k == "id" || k == "ledger" || k == "schemaVersion" || k.ends_with("Id") || k.ends_with("Ids")
    }
    match value {
        Value::Object(map) => {
            let mut sorted: std::collections::BTreeMap<String, Value> =
                std::collections::BTreeMap::new();
            for (k, v) in map {
                if is_identity_key(&k) {
                    continue;
                }
                sorted.insert(k, canonicalize(v));
            }
            let mut new_map = serde_json::Map::new();
            for (k, v) in sorted {
                new_map.insert(k, v);
            }
            Value::Object(new_map)
        }
        Value::Array(items) => Value::Array(items.into_iter().map(canonicalize).collect()),
        v => v,
    }
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
    assert_eq!(v1, v2, "access directives changed semantics across a reformat");

    let ticket = schema1.orbitals[0]
        .entity
        .as_inline()
        .expect("inline entity");
    assert!(ticket.read_policy.is_some(), "read_policy did not reach the IR");
    assert!(ticket.create_policy.is_some(), "create_policy did not reach the IR");
    assert!(ticket.update_policy.is_some(), "update_policy did not reach the IR");
    assert!(ticket.delete_policy.is_some(), "delete_policy did not reach the IR");
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
        let schema = parse_and_lower_str(&source).unwrap_or_else(|e| panic!("{tag} must parse: {e:?}"));
        let entity = schema.orbitals[0].entity.as_inline().expect("inline entity");
        assert!(entity.identity, "{tag} did not set identity");
        assert_eq!(
            entity.shared,
            tag.contains("shared"),
            "{tag} got the wrong shared flag"
        );
    }
}
