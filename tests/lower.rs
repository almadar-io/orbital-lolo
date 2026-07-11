//! Integration tests for the .lolo lowering pipeline.
//!
//! Each fixture in `tests/fixtures/` has a `.lolo` source and a
//! `.orb.golden.json` golden. The test parses the `.lolo`, lowers to an
//! `OrbitalSchema`, serializes to JSON via serde_json::Value, normalizes the
//! key ordering (because `HashMap` is unordered), and compares against the
//! golden after the same normalization.

use orbital_lolo::parse_and_lower_str;
use serde_json::Value;
use std::fs;
use std::path::PathBuf;

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

/// Recursively sort all object keys in a serde_json::Value so two values
/// produced from `HashMap`-backed structs compare deterministically.
fn canonicalize(value: Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut sorted: std::collections::BTreeMap<String, Value> =
                std::collections::BTreeMap::new();
            for (k, v) in map {
                sorted.insert(k, canonicalize(v));
            }
            // Convert back to a Value::Object (which round-trips through
            // serde_json's serializer in the BTreeMap's iteration order).
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

/// Strip serde_json `null` values from objects so that the goldens don't
/// have to spell out every `Option::None` field. Mirrors the
/// `skip_serializing_if = "Option::is_none"` patterns used in real `.orb`
/// fixtures.
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

fn assert_lowering_matches(name: &str) {
    let dir = fixtures_dir();
    let lolo_path = dir.join(format!("{name}.lolo"));
    let golden_path = dir.join(format!("{name}.orb.golden.json"));

    let source = fs::read_to_string(&lolo_path).expect("read lolo");

    let schema = parse_and_lower_str(&source).expect("parse_and_lower");
    let actual: Value = serde_json::to_value(&schema).expect("to_value");
    let actual_n = canonicalize(strip_nulls(actual));

    // Bootstrap mode: if the env var LOLO_GOLDEN_UPDATE is set OR the golden
    // file does not exist, write the canonicalized actual output to disk.
    let update_mode = std::env::var("LOLO_GOLDEN_UPDATE").is_ok();
    if update_mode || !golden_path.exists() {
        let pretty = serde_json::to_string_pretty(&actual_n).unwrap();
        fs::write(&golden_path, format!("{pretty}\n")).expect("write golden");
        if !update_mode {
            // Bootstrap a missing golden: succeed silently the first time so
            // the test author can review the file. Subsequent runs are
            // strict.
            return;
        }
    }

    let golden = fs::read_to_string(&golden_path).expect("read golden");
    let expected: Value = serde_json::from_str(&golden).expect("parse golden");
    let expected_n = canonicalize(strip_nulls(expected));

    if actual_n != expected_n {
        let actual_str = serde_json::to_string_pretty(&actual_n).unwrap();
        let expected_str = serde_json::to_string_pretty(&expected_n).unwrap();
        panic!(
            "fixture {name} lowering mismatch\n\n--- expected ---\n{expected_str}\n\n--- actual ---\n{actual_str}\n"
        );
    }
}

fn assert_parse_error(name: &str) {
    let dir = fixtures_dir().join("errors");
    let lolo_path = dir.join(format!("{name}.lolo"));
    let source = fs::read_to_string(&lolo_path).expect("read lolo");
    if parse_and_lower_str(&source).is_ok() {
        panic!("expected error for fixture {name}, got success");
    }
}

#[test]
fn fixture_01_cart_derived() {
    assert_lowering_matches("01-cart-derived");
}

#[test]
fn fixture_26_typed_payload_form_a() {
    assert_lowering_matches("26-typed-payload-form-a");
}

#[test]
fn fixture_27_typed_payload_form_b() {
    assert_lowering_matches("27-typed-payload-form-b");
}

#[test]
fn fixture_28_trait_instance_modifier() {
    assert_lowering_matches("28-trait-instance-modifier");
}

#[test]
fn fixture_29_event_type_internal() {
    assert_lowering_matches("29-event-type-internal");
}

#[test]
fn fixture_30_event_type_external() {
    assert_lowering_matches("30-event-type-external");
}

#[test]
fn fixture_31_event_type_anonymous_payload() {
    assert_lowering_matches("31-event-type-anonymous-payload");
}

#[test]
fn error_fixture_emits_list_form_rejected() {
    assert_parse_error("11-emits-list-form-rejected");
}

#[test]
fn error_fixture_listens_space_source_rejected() {
    assert_parse_error("12-listens-space-source-rejected");
}

#[test]
fn error_fixture_emits_typename_rejected() {
    assert_parse_error("13-emits-typename-rejected");
}

#[test]
fn fixture_02_multi_state_machine() {
    assert_lowering_matches("02-multi-state-machine");
}

#[test]
fn fixture_03_modal_close_cancel() {
    assert_lowering_matches("03-modal-close-cancel");
}

#[test]
fn fixture_04_render_ui_pattern() {
    assert_lowering_matches("04-render-ui-pattern");
}

#[test]
fn fixture_05_behavior_composition() {
    assert_lowering_matches("05-behavior-composition");
}

#[test]
fn fixture_06_type_aliases() {
    assert_lowering_matches("06-type-aliases");
}

#[test]
fn fixture_07_time_bounded_state() {
    assert_lowering_matches("07-time-bounded-state");
}

#[test]
fn fixture_08_listens_cross_trait() {
    assert_lowering_matches("08-listens-cross-trait");
}

#[test]
fn fixture_09_derived_array_ops() {
    assert_lowering_matches("09-derived-array-ops");
}

#[test]
fn fixture_10_persist_crud() {
    assert_lowering_matches("10-persist-crud");
}

#[test]
fn fixture_11_fetch_with_config() {
    assert_lowering_matches("11-fetch-with-config");
}

#[test]
fn fixture_12_navigate_with_params() {
    assert_lowering_matches("12-navigate-with-params");
}

#[test]
fn fixture_13_notify_with_config() {
    assert_lowering_matches("13-notify-with-config");
}

#[test]
fn fixture_14_emit_with_payload() {
    assert_lowering_matches("14-emit-with-payload");
}

#[test]
fn fixture_15_agent_invocation() {
    assert_lowering_matches("15-agent-invocation");
}

#[test]
#[ignore = "fixture uses `on EVENT { ... }` trait-ref effects-override syntax, removed in Phase 9.5.H. Parser errors as designed."]
fn fixture_16_use_imports() {
    assert_lowering_matches("16-use-imports");
}

#[test]
fn fixture_17_type_entity_decl() {
    assert_lowering_matches("17-type-entity-decl");
}

#[test]
#[ignore = "fixture uses `on EVENT { ... }` trait-ref effects-override syntax, removed in Phase 9.5.H. Parser errors as designed."]
fn fixture_18_type_trait_param() {
    assert_lowering_matches("18-type-trait-param");
}

#[test]
fn fixture_19_type_composition() {
    assert_lowering_matches("19-type-composition");
}

#[test]
fn fixture_20_type_annotation() {
    assert_lowering_matches("20-type-annotation");
}

#[test]
#[ignore = "fixture uses `on EVENT { ... }` trait-ref effects-override syntax, removed in Phase 9.5.H. Parser errors as designed."]
fn fixture_21_type_orbital_shape() {
    assert_lowering_matches("21-type-orbital-shape");
}

#[test]
#[ignore = "fixture uses `on EVENT { ... }` trait-ref effects-override syntax, removed in Phase 9.5.H. Parser errors as designed."]
fn fixture_22_crud_orbital_typed() {
    assert_lowering_matches("22-crud-orbital-typed");
}

#[test]
fn fixture_23_entity_composition_typed() {
    assert_lowering_matches("23-entity-composition-typed");
}

#[test]
fn fixture_24_trait_binding() {
    // Positive fixture: `@trait.X` string children inside a layout trait's
    // render-ui pattern tree parse and lower cleanly. See
    // `docs/Almadar_Std_Gaps.md` §3.8 for the language spec.
    assert_lowering_matches("24-trait-binding");
}

#[test]
fn fixture_32_inline_trait_ref() {
    // Inline `@(Alias.traits.Upstream -> Entity { config {} })` sugar.
    // Lowers byte-identically to the hand-written `trait Name = …` +
    // `@trait.Name` two-step form (modulo the synthesized `Inline_<X>_n`
    // name). See `docs/Almadar_Compiler_Trait_Reference.md`.
    assert_lowering_matches("32-inline-trait-ref");
}

#[test]
fn fixture_25_emit_config() {
    // Positive fixture for `emit:` config on all six operator shapes:
    //   fetch, persist, set, call-service, ref, os/watch-http.
    // Ensures the lolo surface parses + lowers emit options intact.
    assert_lowering_matches("25-emit-config");
}

#[test]
fn error_01_unterminated_string() {
    assert_parse_error("01-unterminated-string");
}

#[test]
fn error_02_unknown_keyword() {
    assert_parse_error("02-unknown-keyword");
}

#[test]
fn error_03_missing_arrow() {
    assert_parse_error("03-missing-arrow");
}

#[test]
fn error_04_invalid_persistence() {
    assert_parse_error("04-invalid-persistence");
}

#[test]
fn error_05_unclosed_paren() {
    assert_parse_error("05-unclosed-paren");
}

#[test]
fn error_06_type_unknown() {
    assert_parse_error("06-type-unknown");
}

#[test]
fn error_07_type_cycle() {
    assert_parse_error("07-type-cycle");
}

#[test]
fn error_08_type_kind_mismatch() {
    assert_parse_error("08-type-kind-mismatch");
}

#[test]
fn error_09_type_composition_conflict() {
    assert_parse_error("09-type-composition-conflict");
}

#[test]
fn error_10_type_annotation_mismatch() {
    assert_parse_error("10-type-annotation-mismatch");
}

// `!` required marker on config knobs — mirrors the entity-field / event-field
// `!` → `required: bool` pattern. `required` may coexist with a `default`
// (the default serves the atom's own standalone demo; call sites must still
// supply the key — enforced by the L2 ORB_T_CONFIG_MISSING_REQUIRED_KEY rule).
#[test]
fn config_field_required_without_default() {
    let source = r#"
orbital W {
  entity Widget [persistent: widgets] {
    id : string!
  }
  trait WidgetView -> Widget [interaction] {
    config {
      title : string!
    }
    state idle {
      LOAD -> idle
        (fetch Widget { id: ?id })
    }
  }
}
"#;
    let schema = parse_and_lower_str(source).expect("lower");
    let orbital_core::schema::TraitReference::Inline(trait_def) = &schema.orbitals[0].traits[0]
    else {
        panic!("expected inline trait");
    };
    let field = trait_def
        .config
        .as_ref()
        .and_then(|c| c.get("title"))
        .expect("title config field");
    assert_eq!(field.required, Some(true));
    assert!(field.default.is_none());
}

#[test]
fn config_field_required_with_default() {
    let source = r#"
orbital W {
  entity Widget [persistent: widgets] {
    id : string!
  }
  trait WidgetView -> Widget [interaction] {
    config {
      title : string! = "Untitled"
      icon : string = "layout-panel-top"
    }
    state idle {
      LOAD -> idle
        (fetch Widget { id: ?id })
    }
  }
}
"#;
    let schema = parse_and_lower_str(source).expect("lower");
    let orbital_core::schema::TraitReference::Inline(trait_def) = &schema.orbitals[0].traits[0]
    else {
        panic!("expected inline trait");
    };
    let config = trait_def.config.as_ref().expect("declared config");
    let title = config.get("title").expect("title config field");
    assert_eq!(title.required, Some(true));
    assert!(title.default.is_some(), "required knob keeps its default");
    // Non-required knob stays untagged (omitted, not `Some(false)`) so
    // published .orb JSON doesn't churn for every existing atom.
    let icon = config.get("icon").expect("icon config field");
    assert_eq!(icon.required, None);
}
