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
  trait WidgetView -> Widget [interaction] {
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
  trait OrderTrait -> Order [interaction] { state idle { INIT -> idle (ref Order) } }
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
  trait WidgetView -> Widget [interaction] {
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
     trait Run -> Data [integration] {{
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
    fn checked_local_annotation_does_not_expose_internal_typed_operator() {
        let src = source("(vm (typed {} ValueMap))", "(set @entity.result \"done\")");
        assert!(parse_str(&src).is_err());
        let quoted = source(
            "(vm (quote (typed {} ValueMap)))",
            "(set @entity.result \"done\")",
        );
        assert!(parse_str(&quoted).is_ok(), "quoted data remains data");
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
            let declaration = &binding.as_array().unwrap()[1].as_array().unwrap()[2];
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
        let encoded = binding.as_array().unwrap()[1].as_array().unwrap()[2]
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
        let encoded = binding.as_array().unwrap()[1].as_array().unwrap()[2].as_str().unwrap();
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
            let declaration = binding.as_array().unwrap()[1].as_array().unwrap()[2].as_str().unwrap();
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
     trait Run -> @rebindable RootEntity [integration] {{
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
      out : Out
     }}
     trait Run -> Cell [integration] {{
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
