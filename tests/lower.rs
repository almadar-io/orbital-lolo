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

fn is_identity_key(k: &str) -> bool {
    k == "id" || k == "ledger" || k == "schemaVersion" || k.ends_with("Id") || k.ends_with("Ids")
}

/// Drops the V4 identity fields (opaque entropy-minted ids + ledger +
/// schemaVersion): these snapshot goldens capture semantic structure, and an
/// id minted fresh each lowering cannot be a stable golden value.
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
/// values produced from `HashMap`-backed structs compare deterministically.
fn canonicalize(value: Value) -> Value {
    orbital_core_eval::util::canonical_json(&strip_identity_keys(value))
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
fn fixture_33_emit_entity_nested_struct_field() {
    // C1-R3 item 3: `emits { X :: Entity }` where the entity's OWN field is
    // an array-of-named-struct-alias (`nodes : [GraphNodeOut]`) whose
    // element carries a nested named-struct-alias field (`badge : Badge`).
    // `resolve_payload_type` used to flatten the entity's fields via
    // `type_expr_to_string` alone, one level shallower than the entity-field
    // path — `nodes` got its `[object]` wrapper + `properties` (handled by
    // the emit_listen caller), but `badge` INSIDE those properties flattened
    // to a bare `"Badge"` string with `properties: None`. Fixed to delegate
    // to `struct_payload_properties`, so `badge` keeps its own
    // `properties: [{ name: "text", ... }]`.
    assert_lowering_matches("33-emit-entity-nested-struct-field");
}

#[test]
fn fixture_25_emit_config() {
    // Positive fixture for `emit:` config on the operator shapes:
    //   fetch, persist, set, call-service.
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
  trait WidgetView -> Widget {
    config {
      title : string!
    }
    state idle {
      LOAD -> idle
        (fetch Widget { id: ?id })
        (render-ui main { type: typography, content: @config.title })
    }
  }
}
"#;
    let schema = parse_and_lower_str(source).expect("lower");
    let orbital_core_base::schema::TraitReference::Inline(trait_def) = &schema.orbitals[0].traits[0]
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
  trait WidgetView -> Widget {
    config {
      title : string! = "Untitled"
      icon : string = "layout-panel-top"
    }
    state idle {
      LOAD -> idle
        (fetch Widget { id: ?id })
        (render-ui main { type: page-header, title: @config.title, icon: @config.icon })
    }
  }
}
"#;
    let schema = parse_and_lower_str(source).expect("lower");
    let orbital_core_base::schema::TraitReference::Inline(trait_def) = &schema.orbitals[0].traits[0]
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

#[test]
fn arrow_inside_sexpr_yields_transition_placement_hint() {
    let src = r#"
orbital Demo {
  entity Item { name: string }
  trait Flow {
    state idle {
      SUBMIT -> saving
        (set @self.name (concat SUBMIT -> done))
    }
    state saving {}
    state done {}
  }
}
"#;
    let err = parse_and_lower_str(src).expect_err("arrow in s-expression must fail");
    let msg = format!("{err}");
    assert!(
        msg.contains("transitions are written at state-block level"),
        "expected placement hint, got: {msg}"
    );
}

// An `expects entity` name is a legal field type standalone: the declaration
// promises the entity exists at compose time, so `customer : Customer` lowers
// to a cardinality:One relation (and `[Customer]` to Many), matching the L2
// relations validator's license for expects-named relation targets.
#[test]
fn expects_entity_name_lowers_to_relation_field() {
    use orbital_core_base::schema::{Cardinality, EntityRef, FieldType};

    let source = r#"
orbital OrbE {
  expects entity Customer
  entity Order [persistent: orders] {
    id : string!
    customer : Customer
    watchers : [Customer]
  }
  trait OrderTrait -> Order { state idle { INIT -> idle (fetch Order) } }
  page "/orders" -> OrderTrait
}
"#;
    let schema = parse_and_lower_str(source).expect("parse_and_lower");
    let EntityRef::Inline(entity) = &schema.orbitals[0].entity else {
        panic!("expected inline entity");
    };
    let customer = entity
        .fields
        .iter()
        .find(|f| f.name.as_deref() == Some("customer"))
        .expect("customer field");
    assert_eq!(customer.field_type, FieldType::Relation);
    let relation = customer.relation.as_ref().expect("customer relation");
    assert_eq!(relation.entity, "Customer");
    assert_eq!(relation.cardinality, Some(Cardinality::One));

    let watchers = entity
        .fields
        .iter()
        .find(|f| f.name.as_deref() == Some("watchers"))
        .expect("watchers field");
    assert_eq!(watchers.field_type, FieldType::Relation);
    let relation = watchers.relation.as_ref().expect("watchers relation");
    assert_eq!(relation.entity, "Customer");
    assert_eq!(relation.cardinality, Some(Cardinality::Many));
}

// Orbital-import Stage A (docs/Almadar_Orbital_Import.md §4) lowering
// goldens — kept in sync with the round-trip tests in tests/formatter.rs.
#[test]
fn orbital_import_app_config_lowers() {
    assert_lowering_matches("orbital-import-app-config");
}

#[test]
fn orbital_import_reference_full_lowers() {
    assert_lowering_matches("orbital-import-reference-full");
}

#[test]
fn orbital_import_inline_config_lowers() {
    assert_lowering_matches("orbital-import-inline-config");
}

// Orbital-import Stage B golden — kept in sync with the round-trip test in
// tests/formatter.rs.
#[test]
fn orbital_ref_stage_b_lowers() {
    assert_lowering_matches("orbital_ref_stage_b");
}

// Orbital-import Stage C `extend {}` golden — kept in sync with the
// round-trip test in tests/formatter.rs.
#[test]
fn orbital_ref_extend_lowers() {
    assert_lowering_matches("orbital_ref_extend");
}

// `children: []` lowering golden — kept in sync with the round-trip test in
// tests/formatter.rs.
#[test]
fn jsx_empty_children_lowers() {
    assert_lowering_matches("jsx_empty_children");
}

#[test]
fn inline_theme_typed_axes_lower_to_typed_tokens() {
    let src = "orbital Foo {\n  entity Bar {\n    id : string!\n  }\n  theme {\n    name: \"deco\"\n    tokens {\n      typeScale {\n        fontImport: \"https://fonts.example/deco.css\"\n        headingTracking: \"0.2em\"\n      }\n      geometry {\n        cornerShape: \"notch\"\n      }\n      surface {\n        backdrop: \"blur(8px)\"\n      }\n    }\n    variants {\n      dark {\n        elevation {\n          interactiveElevation: \"none\"\n        }\n      }\n    }\n  }\n  page \"/\" -> Foo\n}";
    let schema = parse_and_lower_str(src).expect("lowers");
    let json = serde_json::to_value(&schema).unwrap();
    let theme = &json["orbitals"][0]["theme"];
    assert_eq!(
        theme["tokens"]["typeScale"]["fontImport"],
        "https://fonts.example/deco.css"
    );
    assert_eq!(theme["tokens"]["typeScale"]["headingTracking"], "0.2em");
    assert_eq!(theme["tokens"]["geometry"]["cornerShape"], "notch");
    assert_eq!(theme["tokens"]["surface"]["backdrop"], "blur(8px)");
    assert!(
        theme["tokens"]["elevation"].is_null(),
        "control: base elevation absent"
    );
    assert_eq!(
        theme["variants"]["dark"]["elevation"]["interactiveElevation"],
        "none"
    );
}

#[test]
fn inline_theme_motion_category_lowers_into_motion_shapes() {
    let src = "orbital Foo {\n  entity Bar {\n    id : string!\n  }\n  theme {\n    name: \"snappy\"\n    tokens {\n      motion {\n        modalEnter: \"scale(0.9)\"\n        pageEnter: \"none\"\n      }\n    }\n  }\n  page \"/\" -> Foo\n}";
    let schema = parse_and_lower_str(src).expect("lowers");
    let json = serde_json::to_value(&schema).unwrap();
    let motion = &json["orbitals"][0]["theme"]["tokens"]["motion"];
    assert_eq!(motion["shapes"]["modalEnter"], "scale(0.9)");
    assert_eq!(motion["shapes"]["pageEnter"], "none");
    assert!(
        motion["durations"].is_null(),
        "control: only shapes are authored from .lolo"
    );
}

#[test]
fn inline_theme_motion_category_rejects_non_shape_keys() {
    let src = "orbital Foo {\n  entity Bar {\n    id : string!\n  }\n  theme {\n    name: \"snappy\"\n    tokens {\n      motion {\n        durations: \"200ms\"\n      }\n    }\n  }\n  page \"/\" -> Foo\n}";
    assert!(parse_and_lower_str(src).is_err());
}

/// G-ORB-093: an `SExpr` struct member lowers to `FieldType::SExpr`, not a
/// shapeless `Object` that rejects every atom.
#[test]
fn sexpr_struct_member_lowers_to_sexpr_field_type() {
    use orbital_core_base::schema::FieldType;
    let source = r#"
orbital W {
  type Step = {
    type : string!
    args : [SExpr]!
    guard : SExpr
    meta : { note : string }
  }
  entity Widget [runtime] {
    id : string!
  }
  trait WidgetView -> Widget {
    config {
      steps : [Step] = []
    }
    state idle {
      INIT -> idle
        (render-ui main { type: page-header, title: "Steps" })
      SHOW -> idle when (> (array/len @config.steps) 0)
        (render-ui main { type: page-header, title: "Has steps" })
    }
  }
}
"#;
    let schema = parse_and_lower_str(source).expect("lower");
    let orbital_core_base::schema::TraitReference::Inline(trait_def) = &schema.orbitals[0].traits[0]
    else {
        panic!("expected inline trait");
    };
    let steps = trait_def
        .config
        .as_ref()
        .and_then(|c| c.get("steps"))
        .expect("steps knob");
    let props = steps
        .items
        .as_ref()
        .and_then(|i| i.properties.as_ref())
        .expect("Step members");
    let args = props.get("args").expect("args member");
    assert_eq!(args.field_type, FieldType::Array);
    assert_eq!(
        args.items.as_ref().map(|i| i.field_type.clone()),
        Some(FieldType::SExpr)
    );
    assert_eq!(
        props.get("guard").map(|g| g.field_type.clone()),
        Some(FieldType::SExpr)
    );
    // Control: an inline object member still lowers to an object.
    assert_eq!(
        props.get("meta").map(|m| m.field_type.clone()),
        Some(FieldType::Object)
    );
}

#[test]
fn fixture_34_cancel_call() {
    // `call-service` options carry `key` and `emit.cancelled`; `(cancel-call key)`
    // lowers to `["cancel-call", <expr>]` — the IR shape both runtimes read.
    assert_lowering_matches("34-cancel-call");
}

#[test]
fn fixture_10b_persist_batch() {
    assert_lowering_matches("10b-persist-batch");
}

mod checked_local_annotations {
    use orbital_core_base::ir::types::{DeclaredShape, OirType};
    use orbital_core_base::schema::TraitReference;
    use orbital_lolo::{format_program, parse_and_lower_str, parse_str};

    fn source(bindings: &str, body: &str) -> String {
        format!(
            r#"app locals [version: "1.0.0"]
    type ValueMap = Map string string
    orbital Probe {{
     entity Data [runtime] {{
      id : string!
      result : string
     }}
     trait Run -> Data {{
      initial: ready
      state ready {{
       INIT -> ready
        (let ({bindings}) {body})
      }}
     }}
    }}
    "#
        )
    }

    #[test]
    fn checked_local_annotation_parses_and_formats_existing_type_expressions() {
        for ty in [
            "ValueMap",
            "Map string string",
            "[string]",
            "{ value : string! }",
        ] {
            let src = source(
                &format!("(vm : {ty} {{}})"),
                "(set @entity.result \"done\")",
            );
            let parsed = parse_str(&src).unwrap_or_else(|e| panic!("{ty}: {e}"));
            let formatted = format_program(&parsed);
            assert!(formatted.contains(&format!("vm : {ty}")), "{formatted}");
            parse_str(&formatted).expect("formatted annotation parses");
        }
    }

    #[test]
    fn checked_local_annotation_preserves_sequential_and_plain_bindings() {
        let src = source(
            "(first : string \"one\") (next @first) (plain 1)",
            "(set @entity.result @next)",
        );
        let parsed = parse_str(&src).expect("annotation and dependent binding parse");
        let formatted = format_program(&parsed);
        assert!(formatted.contains("(first : string one)"), "{formatted}");
        assert!(formatted.contains("(next @first)"), "{formatted}");
        assert!(formatted.contains("(plain 1)"), "{formatted}");
        let roundtrip = parse_str(&formatted).expect("plain and annotated bindings roundtrip");
        assert_eq!(
            parsed.orbitals[0].traits[0].states[0].transitions[0].effects,
            roundtrip.orbitals[0].traits[0].states[0].transitions[0].effects
        );
    }

    #[test]
    fn checked_local_annotation_formats_sequential_binding_literal_without_capture() {
        let src = source(
            "(first : string \"one\") (literal : string \"first\") (next @first)",
            "(set @entity.result @literal)",
        );
        let parsed = parse_str(&src).unwrap();
        let formatted = format_program(&parsed);
        let roundtrip = parse_str(&formatted).unwrap_or_else(|e| panic!("{e}: {formatted}"));
        assert_eq!(
            parsed.orbitals[0].traits[0].states[0].transitions[0].effects,
            roundtrip.orbitals[0].traits[0].states[0].transitions[0].effects
        );
    }

    #[test]
    fn checked_local_annotation_rejects_malformed_declarations() {
        for bindings in ["(vm : {})", "(vm : ValueMap)", "(vm : ValueMap {} {})"] {
            assert!(
                parse_str(&source(bindings, "(set @entity.result \"done\")")).is_err(),
                "{bindings}"
            );
        }
    }

    #[test]
    fn checked_local_annotation_is_a_third_binding_item_not_an_operator() {
        let src = source("(vm : ValueMap {})", "(set @entity.result \"done\")");
        let parsed = parse_str(&src).expect("annotation parses");
        let effect = &parsed.orbitals[0].traits[0].states[0].transitions[0].effects;
        let json = serde_json::to_string(effect).unwrap();
        assert!(!json.contains("typed"), "{json}");
        let binding = &effect[0].as_array().unwrap()[1].as_array().unwrap()[0];
        assert_eq!(binding.as_array().map(Vec::len), Some(3), "{json}");
        let plain = parse_str(&source("(vm {})", "(set @entity.result \"done\")")).unwrap();
        let effect = &plain.orbitals[0].traits[0].states[0].transitions[0].effects;
        let binding = &effect[0].as_array().unwrap()[1].as_array().unwrap()[0];
        assert_eq!(binding.as_array().map(Vec::len), Some(2), "control: a plain binding stays a pair");
    }

    #[test]
    fn checked_local_annotation_emits_canonical_declared_shape_without_source_ast() {
        for (ty, value) in [
            ("ValueMap", "{}"),
            ("Viewer", "null"),
            ("{ value : string! }", "{ value: \"ok\" }"),
        ] {
            let schema = parse_and_lower_str(&source(
                &format!("(vm : {ty} {value})"),
                "(set @entity.result \"done\")",
            ))
            .unwrap_or_else(|e| panic!("{ty}: {e}"));
            let trait_def = schema.orbitals[0]
                .traits
                .iter()
                .find_map(|t| match t {
                    TraitReference::Inline(t) => Some(t),
                    _ => None,
                })
                .unwrap();
            let effect = &trait_def.state_machine.transitions[0]
                .effects
                .as_ref()
                .unwrap()[0];
            let binding = &effect.as_array().unwrap()[1].as_array().unwrap()[0];
            let declaration = &binding.as_array().unwrap()[2];
            let encoded = declaration
                .as_str()
                .expect("L1 removes source-only AST descriptor");
            let shape = DeclaredShape::parse(encoded).expect("canonical declared-shape descriptor");
            if ty == "ValueMap" {
                assert_eq!(shape.name.as_deref(), Some("ValueMap"));
                assert!(matches!(shape.ty, OirType::Map { .. }), "{shape:?}");
            }
            if ty == "Viewer" {
                assert!(matches!(shape.ty, OirType::Struct { .. }), "{shape:?}");
            }
        }
    }

    #[test]
    fn checked_local_annotation_unknown_named_type_refuses_during_source_lowering() {
        assert!(parse_and_lower_str(&source(
            "(vm : MissingLocalType {})",
            "(set @entity.result \"done\")",
        ))
        .is_err());
    }

    #[test]
    fn checked_local_annotation_resolves_existing_field_projection() {
        let src = source("(vm : Holder.value \"ok\")", "(set @entity.result @vm)").replace(
            "type ValueMap = Map string string",
            "type ValueMap = Map string string\ntype Holder = { value : string! }",
        );
        let schema =
            parse_and_lower_str(&src).expect("existing source field projection resolves in annotation");
        let trait_def = schema.orbitals[0]
            .traits
            .iter()
            .find_map(|t| match t {
                TraitReference::Inline(t) => Some(t),
                _ => None,
            })
            .unwrap();
        let effect = &trait_def.state_machine.transitions[0]
            .effects
            .as_ref()
            .unwrap()[0];
        let binding = &effect.as_array().unwrap()[1].as_array().unwrap()[0];
        let encoded = binding.as_array().unwrap()[2]
            .as_str()
            .unwrap();
        let shape = DeclaredShape::parse(encoded).unwrap();
        assert_eq!(shape.ty, OirType::String);
    }

    #[test]
    fn checked_local_annotation_keeps_initializer_source_span_at_its_real_slot() {
        let parsed = parse_str(&source(
            "(vm : ValueMap (object/merge {} {}))",
            "(set @entity.result \"done\")",
        ))
        .unwrap();
        let transition = &parsed.orbitals[0].traits[0].states[0].transitions[0];
        assert!(transition.effect_spans[0]
            .iter()
            .any(|(path, _)| path == "[1][0][1][1]"));
    }
    pub(super) fn composed_checked_local_schema() -> orbital_core_base::schema::OrbitalSchema {
        let base = super::payload_wrapper_provenance::generic_source(
            "type TreeValue = string | { kind : \"branch\"!, children : [TreeValue]! }\ntype ValueMap = Map string TreeValue\ntype Saved = Event @entity",
        ).replace(
            "state ready { INIT -> ready  Saved -> ready }",
            "state ready {\n INIT -> ready\n  (let ((vm : ValueMap {})) (set @entity.rootOnly (json/stringify @vm)))\n  (emit Saved @entity)\n Saved -> ready\n}",
        );
        super::payload_wrapper_provenance::two_hops_bound_with_remaps(base, "", "", true)
    }

    #[test]
    fn checked_local_annotation_closed_aliases_survive_two_imports_and_remaps() {
        let schema = composed_checked_local_schema();
        let trait_def = super::payload_wrapper_provenance::first_trait(&schema);
        assert_eq!(trait_def.linked_entity.as_deref(), Some("FinalEntity"));
        let transition = trait_def.state_machine.transitions.iter().find(|t| {
            t.effects.as_ref().is_some_and(|effects| !effects.is_empty())
        }).unwrap();
        let effect = &transition.effects.as_ref().unwrap()[0];
        let binding = &effect.as_array().unwrap()[1].as_array().unwrap()[0];
        let encoded = binding.as_array().unwrap()[2].as_str().unwrap();
        let shape = DeclaredShape::parse(encoded).expect("canonical declaration survives import");
        assert_eq!(shape.name.as_deref(), Some("ValueMap"));
        assert!(matches!(shape.ty, OirType::Map { .. }), "{shape:?}");
        assert!(shape.named.contains_key("TreeValue"), "{shape:?}");
        assert!(trait_def.state_machine.transitions.iter().any(|t| t.event == "FinalSaved"));
        let body = &effect.as_array().unwrap()[2];
        assert_eq!(body.as_array().unwrap()[1].as_str(), Some("@entity.finalOnly"));
        let recursive = serde_json::to_value(shape.named.get("TreeValue").unwrap()).unwrap();
        assert!(recursive.to_string().contains("TreeValue"), "{recursive}");
        let verdict = orbital_compiler::phases::validation::validate_comprehensive_without_services(&schema);
        assert!(verdict.errors.is_empty(), "{:?}", verdict.errors);
        assert!(verdict.warnings.is_empty(), "{:?}", verdict.warnings);
    }

    #[test]
    fn checked_local_annotation_substitutes_declared_type_parameter_through_two_imports() {
        for (argument, expected, accepted) in [
            ("string", OirType::String, true),
            ("number", OirType::Number, false),
        ] {
            let base = super::payload_wrapper_provenance::generic_source("type Saved = Event @entity")
                .replace("trait Run ->", "trait Run (v : Type) ->")
                .replace(
                    "state ready { INIT -> ready  Saved -> ready }",
                    "state ready {\n INIT -> ready\n  (let ((value : v \"ok\")) (set @entity.rootOnly (json/stringify @value)))\n  (emit Saved @entity)\n Saved -> ready\n}",
                );
            let schema = super::payload_wrapper_provenance::two_hops_bound_with_remaps(
                base, &format!(":: v {argument}"), "", true,
            );
            let trait_def = super::payload_wrapper_provenance::first_trait(&schema);
            let effect = &trait_def.state_machine.transitions.iter().find_map(|t| t.effects.as_ref().filter(|e| !e.is_empty())).unwrap()[0];
            let binding = &effect.as_array().unwrap()[1].as_array().unwrap()[0];
            let declaration = binding.as_array().unwrap()[2].as_str().unwrap();
            let shape = DeclaredShape::parse(declaration).expect("recorded generic declaration");
            assert_eq!(shape.ty, expected, "{argument}: {shape:?}");
            assert_ne!(shape.name.as_deref(), Some("v"));
            let verdict = orbital_compiler::phases::validation::validate_comprehensive_without_services(&schema);
            if accepted {
                assert!(verdict.errors.is_empty(), "{:?}", verdict.errors);
                assert!(verdict.warnings.is_empty(), "{:?}", verdict.warnings);
            } else {
                assert!(verdict.errors.iter().any(|e| e.code == orbital_compiler::phases::validation::ErrorCode::OperatorArgTypeMismatch), "{:?}", verdict.errors);
            }
        }
    }

}

mod payload_wrapper_provenance {
    use orbital_compiler::phases::inline::{inline_schema, InlineConfig};
    use orbital_compiler::phases::validation::{validate_comprehensive_without_services, ErrorCode};
    use orbital_core_base::schema::{OrbitalSchema, PayloadField, TraitDefinition, TraitReference};
    use orbital_lolo::parse_and_lower_str;

    pub(super) fn generic_source(alias: &str) -> String {
        format!(r#"app origin [version: "1.0.0"]
    {alias}
    orbital Root {{
     entity RootEntity [runtime] {{
      id : string!
      rootOnly : string
     }}
     trait Run -> @rebindable RootEntity {{
      initial: ready
      state ready {{ INIT -> ready  Saved -> ready }}
      emits {{ Saved }}
     }}
    }}
    "#)
    }

    pub(super) fn first_trait(schema: &OrbitalSchema) -> &TraitDefinition {
        let traits = &schema.orbitals[0].traits;
        traits.iter().find_map(|t| match t {
            TraitReference::Inline(t) if t.name == "Bound" => Some(t), _ => None,
        }).or_else(|| traits.iter().find_map(|t| match t {
            TraitReference::Inline(t) => Some(t), _ => None,
        })).unwrap()
    }

    fn both_payloads(trait_def: &TraitDefinition) -> [&[PayloadField]; 2] {
        [trait_def.emits.as_ref().unwrap()[0].payload_schema.as_deref().unwrap(),
         trait_def.state_machine.events.as_ref().unwrap().iter().find(|e| e.key == "Saved").unwrap().payload_schema.as_deref().unwrap()]
    }

    #[test]
    fn bare_payload_origin_is_explicit_and_authored_wrappers_have_none() {
        for (alias, expected) in [
            ("type Saved = Event @entity", Some("@entity")),
            ("type Saved (e : Entity) = Event e", Some("$e")),
            ("type Saved = Event RootEntity", Some("RootEntity")),
            ("type Saved = Event { value : @entity }", None),
            ("type Saved (e : Entity) = Event { value : e }", None),
            ("type Saved = Event { row : @entity }", None),
            ("type Saved = Event [@entity]", None),
            ("type Saved = Event string", None),
        ] {
            let schema = parse_and_lower_str(&generic_source(alias)).unwrap();
            let t = first_trait(&schema);
            assert_eq!(t.emits.as_ref().unwrap()[0].payload_entity.as_deref(), expected, "{alias}");
            assert_eq!(t.state_machine.events.as_ref().unwrap().iter().find(|e| e.key == "Saved").unwrap().payload_entity.as_deref(), expected, "{alias}");
        }
    }

    fn two_hops(alias: &str) -> OrbitalSchema {
        two_hops_bound(generic_source(alias), "", "")
    }

    fn two_hops_bound(base_source: String, annotation: &str, extra_entity: &str) -> OrbitalSchema {
        two_hops_bound_with_remaps(base_source, annotation, extra_entity, false)
    }

    pub(super) fn two_hops_bound_with_remaps(base_source: String, annotation: &str, extra_entity: &str, remap: bool) -> OrbitalSchema {
        let dir = tempfile::TempDir::new().unwrap();
        let mut base = parse_and_lower_str(&base_source).unwrap();
        inline_schema(&mut base, &InlineConfig::default(), None).unwrap();
        std::fs::write(dir.path().join("base.orb"), serde_json::to_vec(&base).unwrap()).unwrap();
        for (name, from, member, entity, extra) in [
            ("middle", "base", "Run", "MiddleEntity", "middleOnly"),
            ("final", "middle", "Bound", "FinalEntity", "finalOnly"),
        ] {
            let annotation = if name == "middle" { annotation } else { "" };
            let extra_entity = if name == "middle" { extra_entity } else { "" };
            let overrides = if remap {
                if name == "middle" {
                    "fields { rootOnly: middleOnly } events { Saved: MiddleSaved }"
                } else {
                    "fields { middleOnly: finalOnly } events { MiddleSaved: FinalSaved }"
                }
            } else { "" };
            let source = format!(r#"app consumer [version: "1.0.0"]
    uses Base from "./{from}.orb"
    orbital Consumer {{
     entity {entity} [runtime] {{
      id : string!
      {extra} : string
     }}
     {extra_entity}
     trait Bound {annotation} = Base.traits.{member} -> {entity} {{ {overrides} }}
    }}
    "#);
            let mut schema = parse_and_lower_str(&source).unwrap();
            let path = dir.path().join(format!("{name}.orb"));
            inline_schema(&mut schema, &InlineConfig::default(), Some(&path)).unwrap();
            if name == "final" { return schema; }
            std::fs::write(path, serde_json::to_vec(&schema).unwrap()).unwrap();
        }
        unreachable!()
    }

    #[test]
    fn genuine_bare_generic_payload_follows_two_composed_rebindings() {
        for alias in ["type Saved = Event @entity", "type Saved (e : Entity) = Event e", "type Saved = Event RootEntity"] {
            let schema = two_hops(alias);
            for payload in both_payloads(first_trait(&schema)) {
                assert!(payload.iter().any(|f| f.name == "finalOnly"), "{alias}: {payload:?}");
                assert!(!payload.iter().any(|f| f.name == "value"), "{alias}: {payload:?}");
            }
        }
    }

    #[test]
    fn authored_value_and_row_wrappers_keep_their_paths_through_two_hops() {
        for (alias, wrapper) in [
            ("type Saved = Event { value : @entity }", "value"),
            ("type Saved (e : Entity) = Event { value : e }", "value"),
            ("type Saved = Event { row : @entity }", "row"),
        ] {
            let schema = two_hops(alias);
            for payload in both_payloads(first_trait(&schema)) {
                assert_eq!(payload.len(), 1, "{alias}: {payload:?}");
                assert_eq!(payload[0].name, wrapper, "{alias}");
                assert!(payload[0].properties.as_ref().unwrap().iter().any(|f| f.name == "finalOnly"), "{alias}: {payload:?}");
            }
        }
    }

    #[test]
    fn authored_named_record_wrapper_reaches_structural_validation_unchanged() {
        for (source_type, mismatch) in [
            ("{ title : string!, count : number! }", false),
            ("{ title : number!, count : number! }", true),
            ("{ title : string! }", true),
            ("number", true),
        ] {
            let source = format!(r#"app records [version: "1.0.0"]
    type NoteRecord = {{ title : string!, count : number! }}
    type Out = {{ value : {source_type}! }}
    orbital Probe {{
     entity Cell [runtime] {{
      id : string!
      out : Out!
     }}
     trait Run -> Cell {{
      initial: ready
      state ready {{ ASK -> ready
       (set @entity.out ?input)
       (emit DONE {{ value: (object/get @entity.out "value") }})
      }}
      listens {{ ASK {{ input : Out! }} @description "Probe." }}
      emits {{ DONE -> external {{ value : NoteRecord! }} @description "Probe." }}
     }}
    }}
    "#);
            let mut schema = parse_and_lower_str(&source).unwrap();
            inline_schema(&mut schema, &InlineConfig::default(), None).unwrap();
            let payload = first_trait(&schema).emits.as_ref().unwrap()[0].payload_schema.as_ref().unwrap();
            assert_eq!(payload.len(), 1, "{source_type}: {payload:?}");
            assert_eq!(payload[0].name, "value");
            let result = validate_comprehensive_without_services(&schema);
            assert_eq!(result.errors.iter().any(|e| e.code == ErrorCode::EmitPayloadFieldTypeMismatch), mismatch, "{source_type}: {:?}", result.errors);
            if !mismatch { assert!(result.errors.is_empty(), "{:?}", result.errors); assert!(result.warnings.is_empty(), "{:?}", result.warnings); }
        }
    }

    #[test]
    fn a_primitive_union_takes_a_literal_of_one_of_its_members() {
        let program = |literal: &str, union: &str| {
            format!(r#"app unions [version: "1.0.0"]
    type Correlation = {union}
    orbital Probe {{
     entity Cell [runtime] {{
      id : string!
     }}
     trait Run -> Cell {{
      initial: ready
      state ready {{ INIT -> ready (emit ANSWER {{ requestId: {literal} }}) }}
      emits {{ ANSWER -> external {{ requestId : Correlation! }} @description "Probe." }}
     }}
     page boot as ProbeStart -> Run
    }}
    "#)
        };
        for literal in ["\"a\"", "7"] {
            let mut schema = parse_and_lower_str(&program(literal, "string | number")).unwrap_or_else(|e| panic!("{literal}: {e}"));
            inline_schema(&mut schema, &InlineConfig::default(), None).unwrap();
            let result = validate_comprehensive_without_services(&schema);
            assert!(
                !result.errors.iter().any(|e| e.code == ErrorCode::EmitPayloadFieldTypeMismatch),
                "{literal}: {:?}",
                result.errors
            );
        }
        let refused = parse_and_lower_str(&program("true", "string | number"));
        assert!(refused.is_err(), "control: a boolean is not a member of string | number");
        let inline = parse_and_lower_str(&program("\"a\"", "string | number").replace("requestId : Correlation!", "requestId : string | number!"));
        assert!(inline.is_ok(), "the inline union agrees with the alias: {:?}", inline.err());
    }

    #[test]
    fn a_guard_that_rules_out_an_enum_literal_narrows_the_enum() {
        let program = |guard: &str| {
            format!(r#"app tiers [version: "1.0.0"]
    orbital Probe {{
     entity Build [runtime] {{
      id : string!
      tier : "none" | "fast" | "full" = none
     }}
     trait Run -> Build {{
      initial: ready
      state ready {{
       INIT -> ready
        (set @entity.tier "fast")
       GO -> ready {guard}
        (emit DONE {{ tier: @entity.tier, build: {{ tier: @entity.tier }} }})
      }}
      listens {{ GO {{ }} }}
      emits {{ DONE -> external {{ tier : "fast" | "full"!  build : {{ tier : "fast" | "full"! }}! }} @description "Probe." }}
     }}
     page boot as ProbeStart -> Run
    }}
    "#)
        };
        let mismatches = |guard: &str| {
            let mut schema = parse_and_lower_str(&program(guard)).unwrap_or_else(|e| panic!("{guard}: {e}"));
            inline_schema(&mut schema, &InlineConfig::default(), None).unwrap();
            validate_comprehensive_without_services(&schema)
                .errors
                .into_iter()
                .filter(|e| e.code == ErrorCode::EmitPayloadFieldTypeMismatch)
                .count()
        };
        assert_eq!(mismatches("when (!= @entity.tier \"none\")"), 0, "the guard rules out none");
        assert_eq!(mismatches(""), 2, "control: unguarded, the tier may be none (top level and nested record)");
        assert_eq!(mismatches("when (!= @entity.tier \"fast\")"), 2, "control: ruling out another member leaves none");
    }

    #[test]
    fn a_field_of_a_knob_named_entity_reads_its_type_from_that_entity() {
        let source = r#"app keyed [version: "1.0.0"]
    orbital Probe {
     entity Item [runtime] {
      id : string!
      status : string = ""
      kept : [string] = []
     }
     trait Run -> Item {
      initial: ready
      state ready {
       INIT -> ready
       LOADED -> ready
        (set @entity.kept (array/map ?data (fn r (default (object/get @r @config.filterField) ""))))
      }
      listens { LOADED { data : [@entity @config.rowEntity]! } }
      config {
       rowEntity : entity = Item
       filterField : field (@entity @config.rowEntity) string = status
      }
     }
     page boot as ProbeStart -> Run
    }
    "#;
        let mut schema = parse_and_lower_str(source).unwrap();
        inline_schema(&mut schema, &InlineConfig::default(), None).unwrap();
        let result = validate_comprehensive_without_services(&schema);
        assert!(
            !result.errors.iter().any(|e| e.code == ErrorCode::OperatorArgUndetermined),
            "{:?}",
            result.errors
        );
    }

    #[test]
    fn a_field_reference_inside_a_list_of_records_is_checked_against_the_knob_named_entity() {
        let source = |name: &str| {
            format!(
                r#"app probenest [version: "1.0.0"]
orbital Probe {{
  type Where = {{ field : field (@entity @config.rowEntity) number!, bound : number! }}
  entity Sale [runtime] {{
    id : string!
    amount : number!
    note : string!
  }}
  entity Item [runtime] {{
    id : string!
    total : number = 0
  }}
  trait Run -> Item {{
    initial: ready
    state ready {{
      INIT -> ready
      LOAD -> ready
        (set @entity.total (array/len (array/filter ?data (fn row (array/every @config.where (fn w (> (default (object/get @row @w.field) 0) @w.bound)))))))
    }}
    listens {{ LOAD {{ data : [@entity @config.rowEntity]! }} }}
    config {{
      rowEntity : entity = Sale
      where : [Where] = [{{ field: {name}, bound: 3 }}]
    }}
  }}
  page boot as ProbeStart -> Run
}}
"#
            )
        };
        let codes = |name: &str| {
            let mut schema = parse_and_lower_str(&source(name)).unwrap();
            inline_schema(&mut schema, &InlineConfig::default(), None).unwrap();
            validate_comprehensive_without_services(&schema)
                .errors
                .into_iter()
                .map(|e| (e.code, e.message))
                .collect::<Vec<_>>()
        };
        let good = codes("amount");
        assert!(good.is_empty(), "{good:?}");
        assert!(codes("nope").iter().any(|(c, _)| *c == ErrorCode::ConfigFieldRefUnknown), "an unknown column is reported");
        assert!(codes("note").iter().any(|(c, _)| *c == ErrorCode::ConfigFieldRefTypeMismatch), "a text column is not a number");
    }

    #[test]
    fn a_relation_reads_as_the_related_rows_id() {
        let source = |predicate: &str| {
            format!(
                r#"app relid [version: "1.0.0"]
orbital StaffOrbital {{
  entity Staff [persistent: staff] {{
    id : string!
    name : string
  }}
  entity Employee [persistent: employees] {{
    id : string!
    staffAccount : Staff
    name : string = "none"
    owner : string = "none"
  }}
  trait Self -> Employee {{
    initial: loading
    state loading {{
      INIT -> loading
        (fetch Employee {{ emit: {{ failure: SelfFailed, success: SelfLoaded }} }})
      SelfLoaded -> loading
        (set @entity.name (default (object/get (array/find ?data (fn row {predicate})) name) "none"))
        (set @entity.owner (default (object/get (array/first ?data) staffAccount) "none"))
      SelfFailed -> loading
    }}
    emits {{ SelfLoaded {{ data : [Employee]! }} SelfFailed {{ error : string }} }}
    listens {{ SelfLoaded {{ data : [Employee]! }} SelfFailed {{ error : string }} }}
  }}
  page boot as SelfStart -> Self
}}
"#
            )
        };
        let codes = |predicate: &str| {
            let mut schema = parse_and_lower_str(&source(predicate)).unwrap();
            inline_schema(&mut schema, &InlineConfig::default(), None).unwrap();
            validate_comprehensive_without_services(&schema)
                .errors
                .into_iter()
                .filter(|e| matches!(e.code, ErrorCode::OperatorArgTypeMismatch | ErrorCode::SetReturnTypeMismatch | ErrorCode::OperatorArgUndetermined))
                .map(|e| e.message)
                .collect::<Vec<_>>()
        };
        let by_id = codes("(= @row.staffAccount @user.id)");
        assert!(by_id.is_empty(), "a relation compares with an id: {by_id:?}");
        assert!(!codes("(= @row.staffAccount 5)").is_empty(), "control: an id is not a number");
    }

    #[test]
    fn a_set_whose_branches_each_fit_the_declared_list_is_admitted() {
        let source = |generated: &str| {
            format!(
                r#"app branches [version: "1.0.0"]
orbital Probe {{
  type Tile = {{ id : string, x : number!, y : number!, passable : boolean }}
  entity Board [runtime] {{
    id : string!
    tiles : [Tile] = []
  }}
  trait Run -> Board [client] {{
    initial: idle
    state idle {{
      INIT -> idle
        (set @entity.tiles (if (> (array/len @config.tiles) 0) @config.tiles (array/map (array/range 0 3) (fn gx {generated}))))
    }}
    config {{
      tiles : [Tile] = []
    }}
  }}
  page boot as ProbeStart -> Run
}}
"#
            )
        };
        let errors = |generated: &str| {
            let mut schema = parse_and_lower_str(&source(generated)).unwrap();
            inline_schema(&mut schema, &InlineConfig::default(), None).unwrap();
            validate_comprehensive_without_services(&schema).errors
        };
        let fits = errors("{ id: (str/concat \"t-\" @gx), x: @gx, y: 0, passable: true }");
        assert!(fits.is_empty(), "{fits:?}");
        assert!(
            !errors("{ id: (str/concat \"t-\" @gx), y: 0 }").is_empty(),
            "control: a generated tile without its required x does not fit"
        );
    }

    #[test]
    fn an_empty_config_list_keeps_its_declared_element_type() {
        let source = |default: &str| {
            format!(
                r#"app emptylist [version: "1.0.0"]
orbital Probe {{
  type Item = {{ key : string!, name : string }}
  entity Holder [runtime] {{
    id : string!
    byKey : Map string Item = {{}}
  }}
  trait Run -> Holder [client] {{
    initial: idle
    state idle {{
      INIT -> idle
        (set @entity.byKey (object/fromEntries (array/map @config.keys (fn k [@k {{ key: @k, name: @k }}]))))
    }}
    config {{
      keys : [string] = {default}
    }}
  }}
  page boot as ProbeStart -> Run
}}
"#
            )
        };
        for default in ["[\"a\"]", "[]"] {
            let mut schema = parse_and_lower_str(&source(default)).unwrap();
            inline_schema(&mut schema, &InlineConfig::default(), None).unwrap();
            let errors = validate_comprehensive_without_services(&schema).errors;
            assert!(errors.is_empty(), "{default}: {errors:?}");
        }
    }

    #[test]
    fn an_annotated_let_keeps_the_narrowing_of_an_if_inside_it() {
        let source = |condition: &str| {
            format!(
                r#"app probehas [version: "1.0.0"]
type PA = {{ scope : "declared"! }}
type PB = {{ read : string! }}
type PU = PA | PB
orbital Probe {{
  entity Item [runtime] {{
    id : string!
    names : [[string]] = []
  }}
  trait Run -> Item {{
    initial: ready
    state ready {{
      INIT -> ready
      LOAD -> ready
        (set @entity.names (array/map ?tools (fn t (let ((r : [string] (if {condition} (let ((rd : PB @t)) [@rd.read]) ["other"]))) @r))))
    }}
    listens {{ LOAD {{ tools : [PU]! }} }}
  }}
  page boot as ProbeStart -> Run
}}
"#
            )
        };
        let errors = |condition: &str| {
            let mut schema = parse_and_lower_str(&source(condition)).unwrap();
            inline_schema(&mut schema, &InlineConfig::default(), None).unwrap();
            validate_comprehensive_without_services(&schema).errors
        };
        let narrowed = errors("(object/has @t read)");
        assert!(narrowed.is_empty(), "{narrowed:?}");
        assert!(
            errors("(object/has @t scope)").iter().any(|e| e.code == ErrorCode::OperatorArgTypeMismatch),
            "control: the scope variant is not a PB"
        );
    }

    #[test]
    fn a_null_test_on_a_let_local_narrows_it_inside_a_record_the_let_returns() {
        let source = |value: &str| {
            format!(
                r#"app probenull [version: "1.0.0"]
type Pol = {{ name : string!, hasRead : boolean! }}
type Reg = {{ label : string!, hasRead : boolean! }}
orbital Probe {{
  entity Item [runtime] {{
    id : string!
    key : string = ""
    reg : Reg? = null
    regs : [Reg] = []
  }}
  trait Run -> Item {{
    initial: ready
    state ready {{
      INIT -> ready
      LOAD -> ready
        (set @entity.key "a")
        (let ((pols : Map string Pol {{}}))
          {value})
    }}
  }}
  page boot as ProbeStart -> Run
}}
"#
            )
        };
        let errors = |value: &str| {
            let mut schema = parse_and_lower_str(&source(value)).unwrap();
            inline_schema(&mut schema, &InlineConfig::default(), None).unwrap();
            validate_comprehensive_without_services(&schema).errors
        };
        let po = r#"(object/get @pols @entity.key null)"#;
        let refused = [
            format!(r#"(set @entity.reg (let ((po {po})) {{ label: "x", hasRead: (if (= @po null) false (object/get @po "hasRead")) }}))"#),
            format!(r#"(set @entity.regs [{{ label: "x", hasRead: (let ((po {po})) (if (= @po null) false (object/get @po "hasRead"))) }}])"#),
            format!(r#"(set @entity.regs [{{ label: "x", hasRead: (let ((po {po})) (if (!= @po null) (object/get @po "hasRead") false)) }}])"#),
            format!(r#"(set @entity.regs (let ((inner : Map string Pol {{}})) [{{ label: "x", hasRead: (let ((po (object/get @inner @entity.key null))) (if (= @po null) false (object/get @po "hasRead"))) }}]))"#),
        ]
        .into_iter()
        .filter_map(|value| {
            let found = errors(&value);
            (!found.is_empty()).then(|| format!("{value}: {found:?}"))
        })
        .collect::<Vec<_>>();
        assert!(refused.is_empty(), "{refused:#?}");
        assert!(
            errors(&format!(r#"(set @entity.regs [{{ label: "x", hasRead: (let ((po {po})) (object/get @po "hasRead")) }}])"#))
                .iter()
                .any(|e| matches!(e.code, ErrorCode::SetReturnTypeMismatch | ErrorCode::OperatorNullOperand)),
            "control: an untested local stays maybe-missing (object/get raises on it, G-ORB-546)"
        );
    }

    #[test]
    fn a_relation_the_fetch_includes_reads_as_the_related_row() {
        let source = |include: &str, second: &str| {
            format!(
                r#"app probeinclude [version: "1.0.0"]
orbital Probe {{
  entity Trader [persistent: traders] {{
    id : string!
    name : string!
  }}
  entity Position [persistent: positions] {{
    id : string!
    trader : Trader
  }}
  entity Desk [runtime] {{
    id : string!
    traderName : string = ""
  }}
  trait Run -> Desk {{
    initial: ready
    state ready {{
      INIT -> ready
        (fetch Position {{ {include} emit: {{ success: PositionsLoaded, failure: PositionsFailed }} }})
        {second}
      PositionsLoaded -> ready
        (set @entity.traderName (if (> (array/len ?data) 0) (default (object/get (object/get (array/first ?data) "trader") "name") "") ""))
      PositionsFailed -> ready
    }}
    emits {{
      PositionsLoaded {{ data : [Position]! }}
      PositionsFailed {{ error : string! }}
    }}
  }}
  page boot as ProbeStart -> Run
}}
"#
            )
        };
        let errors = |include: &str, second: &str| {
            let mut schema = parse_and_lower_str(&source(include, second)).unwrap();
            inline_schema(&mut schema, &InlineConfig::default(), None).unwrap();
            validate_comprehensive_without_services(&schema).errors
        };
        let included = errors("include: [trader]", "");
        assert!(included.is_empty(), "{included:?}");
        assert!(!errors("", "").is_empty(), "control: without include the relation is the related row's id");
        let partly = errors(
            "include: [trader]",
            "(fetch Position { emit: { success: PositionsLoaded, failure: PositionsFailed } })",
        );
        assert!(!partly.is_empty(), "edge: one fetch of the event without the include leaves it an id");
    }

    #[test]
    fn the_retired_equals_derived_field_no_longer_parses_and_the_annotation_lowers() {
        let retired = "orbital O {\n  entity E [runtime] {\n    id : string!\n    count : int = derived (array/len [])\n  }\n}";
        parse_and_lower_str(retired).expect_err("`= derived (...)` is retired");
        let annotated = "orbital O {\n  entity E [runtime] {\n    id : string!\n    count : int @derived (array/len [])\n  }\n}";
        assert!(parse_and_lower_str(annotated).is_ok(), "the annotation is the one spelling");
        let defaulted = "orbital O {\n  entity E [runtime] {\n    id : string!\n    count : int = 0\n  }\n}";
        assert!(parse_and_lower_str(defaulted).is_ok(), "control: a default still lowers");
    }

    #[test]
    fn a_field_whose_value_is_a_knob_named_entity_reads_the_related_row() {
        let source = r#"app keyed [version: "1.0.0"]
    orbital Probe {
     entity Owner [runtime] {
      id : string!
      name : string!
     }
     entity Item [runtime] {
      id : string!
      owner : Owner
      label : string = "none"
      last : Item @intrinsic
     }
     trait Run -> Item {
      initial: ready
      state ready {
       INIT -> ready
       LOADED -> ready
        (set @entity.label (if (!= @config.via null) (let ((owner (object/get ?data @config.via))) (if (= @owner null) "none" (default (object/get @owner @config.shown) "none"))) "none"))
      }
      listens { LOADED { data : Item! } }
      config {
       viaEntity : entity = Item
       via : (field @entity (@entity @config.viaEntity))?
       shown : (field (@entity @config.viaEntity) string)
      }
     }
     page boot as ProbeStart -> Run
    }
    "#;
        let mut schema = parse_and_lower_str(source).unwrap();
        inline_schema(&mut schema, &InlineConfig::default(), None).unwrap();
        let result = validate_comprehensive_without_services(&schema);
        assert!(
            result.errors.is_empty(),
            "{:?}",
            result.errors
        );
    }

    #[test]
    fn a_scalar_type_argument_has_no_entity_origin_after_two_hops() {
        let source = generic_source("type Saved (v : Type) = Event v")
            .replace("trait Run ->", "trait Run (v : Type) ->")
            .replace("INIT -> ready  Saved -> ready", "ASK -> ready (emit Saved { value: ?value })\n Saved -> ready")
            .replace("emits { Saved }", "listens { ASK { value : v! } }\n emits { Saved }");
        let schema = two_hops_bound(source, ":: v string", "");
        let t = first_trait(&schema);
        assert!(t.emits.as_ref().unwrap()[0].payload_entity.is_none());
        assert!(t.state_machine.events.as_ref().unwrap().iter().find(|e| e.key == "Saved").unwrap().payload_entity.is_none());
        for payload in both_payloads(t) {
            assert_eq!(payload.len(), 1);
            assert_eq!(payload[0].name, "value");
            assert_eq!(payload[0].field_type, "string");
        }
        let result = validate_comprehensive_without_services(&schema);
        assert!(result.errors.is_empty(), "{:?}", result.errors);
        assert!(result.warnings.is_empty(), "{:?}", result.warnings);
    }

    #[test]
    fn an_explicit_entity_argument_keeps_its_origin_across_linked_rebinds() {
        let source = generic_source("type Saved (e : Entity) = Event e")
            .replace("trait Run ->", "trait Run (e : Entity) ->");
        let schema = two_hops_bound(source.clone(), ":: e TargetEntity", r#"entity TargetEntity [runtime] {
     id : string!
     explicitOnly : number
    }"#);
        let t = first_trait(&schema);
        assert_eq!(t.emits.as_ref().unwrap()[0].payload_entity.as_deref(), Some("TargetEntity"));
        assert_eq!(t.state_machine.events.as_ref().unwrap().iter().find(|e| e.key == "Saved").unwrap().payload_entity.as_deref(), Some("TargetEntity"));
        for payload in both_payloads(t) {
            assert!(payload.iter().any(|f| f.name == "explicitOnly"), "{payload:?}");
            assert!(!payload.iter().any(|f| f.name == "finalOnly"), "{payload:?}");
        }
        let implicit = two_hops_bound(source, "", "");
        let t = first_trait(&implicit);
        assert_eq!(t.emits.as_ref().unwrap()[0].payload_entity.as_deref(), Some("$e"));
        for payload in both_payloads(t) {
            assert!(payload.iter().any(|f| f.name == "finalOnly"), "{payload:?}");
            assert!(!payload.iter().any(|f| f.name == "explicitOnly"), "{payload:?}");
        }
    }

    #[test]
    fn array_and_literal_scalar_payloads_remain_single_values_after_rebinding() {
        for (alias, expected) in [
            ("type Saved = Event [@entity]", "[object]"),
            ("type Saved = Event string", "string"),
        ] {
            let schema = two_hops(alias);
            let t = first_trait(&schema);
            assert!(t.emits.as_ref().unwrap()[0].payload_entity.is_none());
            for payload in both_payloads(t) {
                assert_eq!(payload.len(), 1);
                assert_eq!(payload[0].name, "value");
                assert_eq!(payload[0].field_type, expected);
                if expected == "[object]" {
                    assert!(payload[0].properties.as_ref().unwrap().iter().any(|f| f.name == "finalOnly"));
                }
            }
        }
    }

    #[test]
    fn a_named_value_record_keeps_its_own_shape_when_the_linked_entity_changes() {
        let schema = two_hops("type WrappedRecord = { title : string!, count : number! }\ntype Saved = Event { value : WrappedRecord }");
        let t = first_trait(&schema);
        assert!(t.emits.as_ref().unwrap()[0].payload_entity.is_none());
        for payload in both_payloads(t) {
            assert_eq!(payload.len(), 1);
            assert_eq!(payload[0].name, "value");
            let fields = payload[0].properties.as_ref().unwrap();
            assert_eq!(fields.len(), 2);
            assert!(fields.iter().any(|f| f.name == "title"));
            assert!(fields.iter().any(|f| f.name == "count"));
            assert!(!fields.iter().any(|f| f.name == "finalOnly"));
        }
    }
}

mod native_record_contracts {
    use orbital_lolo::{format_program, parse_and_lower_str, parse_str};
    use serde_json::{json, Value};

    fn source(declarations: &str) -> String {
        format!(r#"app records [version: "1.0.0"]
{declarations}
orbital Contract {{
 entity State [runtime] {{
  id : string!
  value : Probe
 }}
}}
"#)
    }

    fn value_field(schema: &Value) -> &Value {
        schema["orbitals"][0]["entity"]["fields"].as_array().unwrap().iter()
            .find(|field| field["name"] == "value").unwrap()
    }

    #[test]
    fn native_record_contracts_keep_presence_independent_from_nullability() {
        let schema = parse_and_lower_str(&source("type Probe = { optional : string, required : string!, nullable : string?, requiredNullable : string?! }")).unwrap();
        let encoded = serde_json::to_value(schema).unwrap();
        let fields = &value_field(&encoded)["properties"];
        for (name, required, nullable) in [("optional", false, false), ("required", true, false), ("nullable", false, true), ("requiredNullable", true, true)] {
            assert_eq!(fields[name]["required"].as_bool().unwrap_or(false), required, "{name}: {}", fields[name]);
            assert_eq!(fields[name]["nullable"].as_bool().unwrap_or(false), nullable, "{name}: {}", fields[name]);
        }
    }

    #[test]
    fn native_record_contracts_catchall_roundtrips_without_reinterpreting_literal_keys() {
        let src = source(r#"type Probe = { id : string, "*" : string, "..." : number, ... : string? }"#);
        let parsed = parse_str(&src).unwrap();
        let formatted = format_program(&parsed);
        assert!(formatted.contains("... : string?"), "{formatted}");
        parse_str(&formatted).expect("formatted typed catchall parses");
        let lowered = serde_json::to_value(parse_and_lower_str(&src).unwrap()).unwrap();
        let field = value_field(&lowered);
        assert!(field["properties"].get("*").is_some());
        assert!(field["properties"].get("...").is_some());
        assert_eq!(field["properties"]["id"]["type"], "string");
        assert_eq!(field["additionalProperties"]["type"], "string");
        assert_eq!(field["additionalProperties"]["nullable"], true);
    }

    #[test]
    fn native_record_contracts_ordinary_record_does_not_gain_a_catchall() {
        let encoded = serde_json::to_value(parse_and_lower_str(&source("type Probe = { id : string, * : string }")).unwrap()).unwrap();
        assert!(value_field(&encoded).get("additionalProperties").is_none());
        assert_eq!(value_field(&encoded)["properties"]["*"]["type"], "string");
    }

    #[test]
    fn native_record_contracts_reject_duplicate_or_required_catchall_and_unknown_type() {
        for declaration in ["type Probe = { ... : string, ... : number }", "type Probe = { ... : string! }", "type Probe = { ... : MissingType }"] {
            assert!(parse_and_lower_str(&source(declaration)).is_err(), "{declaration}");
        }
    }

    #[test]
    fn native_record_contracts_type_only_source_retains_named_recursive_declarations() {
        let src = r#"type FieldValue = string | number | boolean | [FieldValue] | Map string FieldValue
type RowWriteFailureMode = "atomic" | "stop"
type RowWriteBatch = { ops : [FieldValue]!, onFailure : RowWriteFailureMode! }
"#;
        let encoded = serde_json::to_value(parse_and_lower_str(src).unwrap()).unwrap();
        assert_eq!(encoded["orbitals"], json!([]));
        let types = encoded["types"].as_object().expect("type-only source exports existing named-type metadata");
        for name in ["FieldValue", "RowWriteFailureMode", "RowWriteBatch"] {
            assert!(types.contains_key(name), "{encoded}");
        }
        assert!(types["FieldValue"].to_string().contains("FieldValue"));
        let mode = &types["RowWriteFailureMode"][0];
        assert_eq!(mode["type"], "string");
        assert_eq!(mode["values"], json!(["atomic", "stop"]));
        assert_eq!(mode["nullable"], false);
        let failure = types["RowWriteBatch"].as_array().unwrap().iter()
            .find(|field| field["name"] == "onFailure").unwrap();
        assert_eq!(failure["type"], "string");
        assert_eq!(failure["values"], mode["values"]);
        assert_eq!(failure["required"], true);
        assert_eq!(failure["nullable"], false);
        let schema = parse_and_lower_str(src).unwrap();
        let rendered = format_program(&orbital_lolo::unlower(&schema).unwrap());
        assert_eq!(parse_and_lower_str(&rendered).unwrap().types, schema.types);
    }
    #[test]
    fn native_record_contracts_legacy_print_spells_nullable_fields_without_losing_presence() {
        let legacy: orbital_core_base::schema::OrbitalSchema = serde_json::from_value(json!({
            "name":"Legacy", "orbitals":[{"name":"Legacy", "entity":{"name":"Row", "fields":[
                {"name":"required", "type":"string", "required":true},
                {"name":"optional", "type":"string", "required":false}
            ]}, "traits":[], "pages":[]}]
        })).unwrap();
        let printed = format_program(&orbital_lolo::unlower(&legacy).unwrap());
        assert!(printed.contains("required : string?!"), "{printed}");
        assert!(printed.contains("optional : string?"), "{printed}");
        let restored = serde_json::to_value(parse_and_lower_str(&printed).unwrap()).unwrap();
        for field in restored["orbitals"][0]["entity"]["fields"].as_array().unwrap() {
            assert_eq!(field["nullable"], true, "{field}");
        }
    }

}

#[test]
fn shape_aliases_export_their_merged_field_lists_and_no_type_is_a_bare_object() {
    let source = r#"app shapes [version: "1.0.0"]
type Timestamped = Entity {
  id : string!
  createdAt : string
}
type Auditable = Timestamped & Entity {
  createdBy : string
  updatedBy : string
}
type Saved = Event string
orbital AuditOrbital {
  entity Rec :: Auditable [persistent: recs] {
    id : string!
    createdAt : string
    createdBy : string
    updatedBy : string
  }
  trait RecBrowse -> Rec {
    state idle {
      INIT -> idle
        (fetch Rec)
    }
  }
  page "/recs" -> RecBrowse
}
"#;
    let schema = serde_json::to_value(parse_and_lower_str(source).unwrap()).unwrap();
    let names = |entry: &Value| -> Vec<String> {
        entry
            .as_array()
            .unwrap()
            .iter()
            .map(|f| f["name"].as_str().unwrap().to_string())
            .collect()
    };
    for types in [&schema["types"], &schema["orbitals"][0]["types"]] {
        assert_eq!(names(&types["Timestamped"]), ["id", "createdAt"], "{types}");
        assert_eq!(
            names(&types["Auditable"]),
            ["id", "createdAt", "createdBy", "updatedBy"],
            "{types}"
        );
        assert!(types.get("Saved").is_none(), "an event alias is no exported type: {types}");
        for (alias, entry) in types.as_object().unwrap() {
            for member in entry.as_array().unwrap() {
                let bare = member["type"] == "object"
                    && member["name"] == ""
                    && member.get("properties").is_none();
                assert!(!bare, "`{alias}` is exported as a bare object: {member}");
            }
        }
    }
}

#[test]
fn an_object_literal_fills_a_map_payload_field_and_a_number_literal_an_instant() {
    let source = |value: &str, ty: &str| {
        format!(
            r#"app probemap [version: "1.0.0"]
orbital Probe {{
  entity Item [runtime] {{
    id : string!
  }}
  trait Run -> Item {{
    initial: ready
    state ready {{
      INIT -> ready
      GO -> ready
        (emit DONE {{ v: {value} }})
    }}
    emits {{ DONE {{ v : {ty}! }} }}
  }}
  page boot as ProbeStart -> Run
}}
"#
        )
    };
    let lowers = |value: &str, ty: &str| parse_and_lower_str(&source(value, ty)).is_ok();
    assert!(lowers("{}", "Map string string"), "an empty map literal");
    assert!(lowers(r#"{ a: "x" }"#, "Map string string"), "a map literal with members");
    assert!(lowers("0", "instant"), "a literal moment");
    assert!(!lowers(r#""x""#, "Map string string"), "control: a string is not a map");
    assert!(!lowers(r#""x""#, "instant"), "control: a string is not an instant");
}

fn index_program(directives: &str) -> String {
    format!(
        r#"app indexprobe [version: "1.0.0"]
orbital EdgeOrbital {{
  entity Edge [persistent: edges] {{
{directives}
    id : string!
    graphId : string!
    relation : string
    weight : number
  }}
  entity Plain [persistent: plains] {{
    id : string!
  }}
}}
"#
    )
}

fn entity_json(schema: &orbital_core_base::schema::OrbitalSchema, name: &str) -> Value {
    let value = serde_json::to_value(schema).unwrap();
    let orbital = &value["orbitals"][0];
    std::iter::once(&orbital["entity"])
        .chain(orbital["auxiliaryEntities"].as_array().into_iter().flatten())
        .find(|e| e["name"] == name)
        .unwrap_or_else(|| panic!("entity {name} is declared"))
        .clone()
}

#[test]
fn index_directives_lower_to_entity_indexes_in_declaration_order() {
    let schema = parse_and_lower_str(&index_program(
        "    @index { fields: [\"graphId\"] }\n    @index { fields: [\"graphId\", \"relation\"], orderBy: \"weight:desc\" }",
    ))
    .expect("lowers");
    let edge = entity_json(&schema, "Edge");
    assert_eq!(
        edge["indexes"],
        serde_json::json!([
            { "fields": ["graphId"] },
            { "fields": ["graphId", "relation"], "orderBy": "weight:desc" }
        ])
    );
    assert!(
        entity_json(&schema, "Plain").get("indexes").is_none(),
        "control: an entity with no @index serializes no `indexes` key"
    );
}

#[test]
fn control_an_entity_without_index_directives_has_no_indexes() {
    let schema = parse_and_lower_str(&index_program("")).expect("lowers");
    assert!(entity_json(&schema, "Edge").get("indexes").is_none());
}

#[test]
fn an_index_orders_by_the_fetch_string_verbatim_and_a_bare_field_is_ascending() {
    let schema = parse_and_lower_str(&index_program(
        "    @index { fields: [\"graphId\"], orderBy: \"weight\" }",
    ))
    .expect("lowers");
    assert_eq!(
        entity_json(&schema, "Edge")["indexes"],
        serde_json::json!([{ "fields": ["graphId"], "orderBy": "weight" }])
    );
}

#[test]
fn index_directive_errors_name_their_code_and_valid_forms_lower() {
    let lower = |directive: &str| {
        parse_and_lower_str(&index_program(&format!("    @index {directive}")))
            .map(|_| ())
            .map_err(|e| format!("{e}"))
    };
    for (directive, code) in [
        (r#"{ fields: ["graphId"], unique: true }"#, "ELOLO_INDEX_UNKNOWN_KEY"),
        (r#"{ fields: [] }"#, "ELOLO_INDEX_FIELDS_INVALID"),
        (r#"{ orderBy: "weight" }"#, "ELOLO_INDEX_FIELDS_INVALID"),
        (r#"{ fields: "graphId" }"#, "ELOLO_INDEX_FIELDS_INVALID"),
        (r#"{ fields: [1] }"#, "ELOLO_INDEX_FIELDS_INVALID"),
        (r#"{ fields: ["graphId"], orderBy: 3 }"#, "ELOLO_INDEX_ORDER_BY_INVALID"),
        (r#"{ fields: ["graphId"], orderBy: ["weight"] }"#, "ELOLO_INDEX_ORDER_BY_INVALID"),
        (r#""graphId""#, "ELOLO_INDEX_NOT_A_RECORD"),
    ] {
        let err = lower(directive).expect_err(directive);
        assert!(err.contains(code), "{directive}: expected {code}, got {err}");
    }
    // Controls: the same shapes, valid, lower.
    lower(r#"{ fields: ["graphId"] }"#).expect("one field");
    lower(r#"{ fields: ["graphId", "relation"], orderBy: "weight:desc" }"#).expect("two fields and order");
}

#[test]
fn index_directive_may_repeat_unlike_the_access_directives() {
    let source = |directive: &str| index_program(&format!("    {directive}\n    {directive}"));
    parse_and_lower_str(&source(r#"@index { fields: ["graphId"] }"#)).expect("@index repeats");
    let err = parse_and_lower_str(&source(r#"@read (= @entity.graphId "g")"#))
        .map(|_| ())
        .expect_err("control: @read may not repeat");
    assert!(format!("{err}").contains("ELOLO_DUPLICATE_ACCESS_DIRECTIVE"));
}

#[test]
fn index_directive_inside_expects_entity_is_refused_and_a_field_there_still_parses() {
    let source = |body: &str| {
        format!(
            "app expectsprobe [version: \"1.0.0\"]\norbital Spec {{\n  expects entity Edge [persistent] {{\n{body}\n    graphId : string!\n  }}\n}}\n"
        )
    };
    let err = parse_and_lower_str(&source(r#"    @index { fields: ["graphId"] }"#))
        .map(|_| ())
        .expect_err("@index is not part of an expects shape");
    assert!(format!("{err}").contains("field declaration or }"), "{err}");
    parse_and_lower_str(&source("")).expect("control: the same shape without @index parses");
}

fn derived_program(field: &str) -> String {
    format!(
        r#"app derivedprobe [version: "1.0.0"]
orbital PersonOrbital {{
  entity Person [persistent: people] {{
    id : string!
    email : string!
    {field}
  }}
}}
"#
    )
}

fn field_json(schema: &orbital_core_base::schema::OrbitalSchema, entity: &str, field: &str) -> Value {
    entity_json(schema, entity)["fields"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["name"] == field)
        .unwrap_or_else(|| panic!("field {field} is declared"))
        .clone()
}

#[test]
fn a_derived_annotation_lowers_to_the_field_derived_expression() {
    let schema = parse_and_lower_str(&derived_program(
        "emailKey : string! @derived (str/lower (object/get @entity email))",
    ))
    .expect("lowers");
    let field = field_json(&schema, "Person", "emailKey");
    assert_eq!(
        field["derived"],
        serde_json::json!(["str/lower", ["object/get", "@entity", "email"]])
    );
    assert_eq!(field["required"], serde_json::json!(true));
    assert!(
        field_json(&schema, "Person", "email").get("derived").is_none(),
        "control: a plain field serializes no `derived` key"
    );
}

#[test]
fn a_derived_field_may_be_unique_and_carry_the_other_annotations() {
    let schema = parse_and_lower_str(&derived_program(
        "emailKey : string! @unique @description \"lower-cased email\" @derived (str/lower (object/get @entity email))",
    ))
    .expect("lowers");
    let field = field_json(&schema, "Person", "emailKey");
    assert_eq!(field["unique"], serde_json::json!(true));
    assert!(field.get("derived").is_some());
}

#[test]
fn a_derived_field_with_a_default_is_refused_and_each_alone_lowers() {
    let err = parse_and_lower_str(&derived_program(
        "emailKey : string! @derived (str/lower (object/get @entity email)) = \"x\"",
    ))
    .map(|_| ())
    .expect_err("a computed field has no default");
    assert!(format!("{err}").contains("ELOLO_DERIVED_WITH_DEFAULT"), "{err}");
    parse_and_lower_str(&derived_program("emailKey : string! @derived (str/lower (object/get @entity email))"))
        .expect("control: derived alone");
    parse_and_lower_str(&derived_program("emailKey : string = \"x\"")).expect("control: default alone");
}

#[test]
fn the_retired_equals_derived_spelling_is_a_parse_error() {
    let err = parse_and_lower_str(&derived_program("emailKey : string! = derived (str/lower (object/get @entity email))"))
        .map(|_| ())
        .expect_err("`= derived (...)` is retired");
    assert!(!format!("{err}").is_empty());
}

#[test]
fn a_derived_annotation_without_an_expression_is_refused() {
    parse_and_lower_str(&derived_program("emailKey : string! @derived"))
        .map(|_| ())
        .expect_err("@derived needs its expression");
}

#[test]
fn a_field_named_derived_is_an_ordinary_field_again() {
    let schema = parse_and_lower_str(&derived_program("derived : string")).expect("the word is free");
    assert!(field_json(&schema, "Person", "derived").get("derived").is_none());
}
