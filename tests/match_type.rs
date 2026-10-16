//! Type-level `match` (§25) — replaces `@typeWhen` outright, one concept,
//! one spelling. This is the only test coverage for the construct in
//! `orbital-lolo`; `@typeWhen` itself never had any (the gap the plan named
//! explicitly). Covers: parse happy paths, every parser/L1 error form, the
//! format round-trip, and the lowered wire-JSON equivalence to the unchanged
//! `PayloadTypeWhen` IR.

use orbital_core_base::schema::TraitReference;
use orbital_lolo::{format_program, parse_and_lower_str, parse_str, LoloError};

/// Wrap a `trait` body (emits/config/listens blocks) in a minimal orbital —
/// one entity, one trait, one no-op state — so every case below only needs
/// to author the surface actually under test.
fn wrap(trait_body: &str) -> String {
    format!(
        r#"orbital MatchOrbital {{
  entity Widget [runtime] {{
    id : string!
  }}

  trait WidgetInput -> Widget [interaction] {{
    state idle {{
      INIT -> idle
    }}

{trait_body}
  }}
}}
"#
    )
}

fn trait_named<'a>(
    schema: &'a orbital_core_base::schema::OrbitalSchema,
    name: &str,
) -> &'a orbital_core_base::schema::TraitDefinition {
    schema.orbitals[0]
        .traits
        .iter()
        .find_map(|t| match t {
            TraitReference::Inline(def) if def.name == name => Some(def),
            _ => None,
        })
        .unwrap_or_else(|| panic!("expected inline trait {name}"))
}

fn lower_err(source: &str) -> String {
    match parse_and_lower_str(source) {
        Err(LoloError::Lower { inner, .. }) => inner.to_string(),
        Err(LoloError::Parse { inner, .. }) => {
            panic!("expected a LOWER error, got a PARSE error: {inner}")
        }
        Ok(_) => panic!("expected an error, lowering succeeded"),
        Err(other) => panic!("expected a Lower error, got {other}"),
    }
}

fn parse_err(source: &str) -> String {
    match parse_str(source) {
        Err(e) => e.to_string(),
        Ok(_) => panic!("expected a parse error, parsing succeeded"),
    }
}

// ---------------------------------------------------------------------------
// Happy paths
// ---------------------------------------------------------------------------

const EMIT_MATCH: &str = r#"
    emits {
      CHANGED -> external {
        value : match @config.inputType {
          "number" -> number
          _        -> string
        }!
      }
    }

    config {
      inputType : string = "text"
    }
"#;

#[test]
fn match_lowers_emit_payload_field_to_the_wire_type_when_list() {
    let schema = parse_and_lower_str(&wrap(EMIT_MATCH)).expect("match on an emit payload lowers");
    let trait_def = trait_named(&schema, "WidgetInput");
    let emit = trait_def
        .emits
        .as_ref()
        .and_then(|es| es.iter().find(|e| e.event == "CHANGED"))
        .expect("CHANGED emit present");
    let value = emit
        .payload_schema
        .as_ref()
        .and_then(|fs| fs.iter().find(|f| f.name == "value"))
        .expect("value field present");
    assert_eq!(
        value.field_type, "string",
        "the `_` arm is the field's base type"
    );
    assert_eq!(
        value.required,
        Some(true),
        "the whole-field `!` (after the match's closing brace) is honored"
    );
    let conditions = value
        .type_when
        .as_ref()
        .expect("non-wildcard arm produced a condition");
    assert_eq!(conditions.len(), 1);
    assert_eq!(conditions[0].knob, "inputType");
    assert_eq!(conditions[0].equals, "number");
    assert_eq!(conditions[0].ty, "number");
}

const LISTEN_MATCH: &str = r#"
    listens {
      UPDATE_VALUE {
        value : match @config.inputType {
          "number" -> number
          _        -> string
        }
      }
    }

    config {
      inputType : string = "text"
    }
"#;

#[test]
fn match_lowers_local_listen_payload_field_identically_to_emit() {
    let schema =
        parse_and_lower_str(&wrap(LISTEN_MATCH)).expect("match on a local listen payload lowers");
    let trait_def = trait_named(&schema, "WidgetInput");
    let event = trait_def
        .state_machine
        .events
        .as_ref()
        .into_iter()
        .flatten()
        .find(|e| e.key == "UPDATE_VALUE")
        .expect("UPDATE_VALUE carries its declared payload on stateMachine.events[]");
    let value = event
        .payload_schema
        .as_ref()
        .and_then(|fs| fs.iter().find(|f| f.name == "value"))
        .expect("value field present");
    assert_eq!(value.field_type, "string");
    let conditions = value.type_when.as_ref().expect("condition present");
    assert_eq!(conditions.len(), 1);
    assert_eq!(
        conditions[0],
        orbital_core_base::schema::PayloadTypeWhen {
            knob: "inputType".to_string(),
            equals: "number".to_string(),
            ty: "number".to_string(),
        }
    );
}

/// Multiple non-wildcard arms preserve AUTHORING ORDER in the lowered
/// condition list (first-match-wins is `resolve_payload_type_when`'s linear
/// `find_map`, unchanged) — and `Word`/`Bool`/`Int` arm patterns all lower
/// to their canonical spelling, matching `config_literal_spelling` exactly.
const MULTI_ARM_MATCH: &str = r#"
    emits {
      CHANGED -> external {
        value : match @config.mode {
          row   -> [string]
          batch -> number
          _     -> string
        }!
      }
    }

    config {
      mode : "row" | "batch" = "row"
    }
"#;

#[test]
fn word_pattern_arms_lower_in_authoring_order_with_canonical_spelling() {
    let schema = parse_and_lower_str(&wrap(MULTI_ARM_MATCH)).expect("bare-word arms lower");
    let trait_def = trait_named(&schema, "WidgetInput");
    let emit = trait_def.emits.as_ref().unwrap().first().unwrap();
    let value = emit
        .payload_schema
        .as_ref()
        .unwrap()
        .iter()
        .find(|f| f.name == "value")
        .unwrap();
    assert_eq!(value.field_type, "string");
    let conditions = value.type_when.as_ref().unwrap();
    assert_eq!(conditions.len(), 2, "two non-wildcard arms");
    assert_eq!(conditions[0].equals, "row");
    assert_eq!(conditions[0].ty, "[string]");
    assert_eq!(conditions[1].equals, "batch");
    assert_eq!(conditions[1].ty, "number");
}

const BOOL_ARM_MATCH: &str = r#"
    emits {
      CHANGED -> external {
        value : match @config.multiple {
          true  -> [string]
          false -> string
          _     -> string
        }!
      }
    }

    config {
      multiple : boolean = false
    }
"#;

#[test]
fn bool_pattern_arms_lower_to_canonical_true_false_spelling() {
    let schema = parse_and_lower_str(&wrap(BOOL_ARM_MATCH)).expect("bool arms lower");
    let trait_def = trait_named(&schema, "WidgetInput");
    let emit = trait_def.emits.as_ref().unwrap().first().unwrap();
    let value = emit
        .payload_schema
        .as_ref()
        .unwrap()
        .iter()
        .find(|f| f.name == "value")
        .unwrap();
    let conditions = value.type_when.as_ref().unwrap();
    assert_eq!(conditions[0].equals, "true");
    assert_eq!(conditions[0].ty, "[string]");
    assert_eq!(conditions[1].equals, "false");
    assert_eq!(conditions[1].ty, "string");
}

// ---------------------------------------------------------------------------
// Parser errors
// ---------------------------------------------------------------------------

#[test]
fn per_arm_bang_is_a_parse_error() {
    let src = wrap(
        r#"
    emits {
      CHANGED -> external {
        value : match @config.inputType {
          "number" -> number!
          _        -> string
        }
      }
    }
    config { inputType : string = "text" }
"#,
    );
    let msg = parse_err(&src);
    assert!(msg.contains("ELOLO_MATCH_ARM_BANG_FORBIDDEN"), "got: {msg}");
}

#[test]
fn missing_scrutinee_is_a_parse_error() {
    let src = wrap(
        r#"
    emits {
      CHANGED -> external {
        value : match {
          "number" -> number
          _        -> string
        }
      }
    }
"#,
    );
    let msg = parse_err(&src);
    assert!(msg.contains("ELOLO_MATCH_SCRUTINEE_REQUIRED"), "got: {msg}");
}

#[test]
fn float_arm_pattern_is_a_parse_error() {
    let src = wrap(
        r#"
    emits {
      CHANGED -> external {
        value : match @config.precision {
          1.5 -> number
          _   -> string
        }
      }
    }
    config { precision : number = 1 }
"#,
    );
    let msg = parse_err(&src);
    assert!(
        msg.contains("ELOLO_MATCH_ARM_PATTERN_INVALID"),
        "got: {msg}"
    );
}

#[test]
fn match_nested_inside_an_array_type_is_a_parse_error() {
    let src = wrap(
        r#"
    emits {
      CHANGED -> external {
        value : [match @config.inputType {
          "number" -> number
          _        -> string
        }]
      }
    }
    config { inputType : string = "text" }
"#,
    );
    let msg = parse_err(&src);
    assert!(
        msg.contains("ELOLO_MATCH_POSITION_UNSUPPORTED"),
        "got: {msg}"
    );
}

#[test]
fn match_forbidden_in_entity_fields() {
    let src = r#"
orbital MatchOrbital {
  entity Widget [runtime] {
    id : string!
    value : match @config.inputType {
      "number" -> number
      _        -> string
    }
  }

  trait WidgetInput -> Widget [interaction] {
    state idle {
      INIT -> idle
    }
    config { inputType : string = "text" }
  }
}
"#;
    let msg = parse_err(src);
    assert!(
        msg.contains("ELOLO_CONFIG_TYPE_FORBIDDEN_IN_ENTITY"),
        "entity fields reuse the existing config-only-type gate: {msg}"
    );
}

#[test]
fn match_forbidden_in_expects_shape() {
    let src = r#"
orbital MatchOrbital {
  expects entity Upstream {
    id : string!
    value : match @config.inputType {
      "number" -> number
      _        -> string
    }
  }

  entity Widget [runtime] {
    id : string!
  }

  trait WidgetInput -> Widget [interaction] {
    state idle {
      INIT -> idle
    }
  }
}
"#;
    let msg = parse_err(src);
    assert!(
        msg.contains("ELOLO_CONFIG_TYPE_FORBIDDEN_IN_ENTITY"),
        "got: {msg}"
    );
}

#[test]
fn match_forbidden_in_config_fields() {
    let src = wrap(
        r#"
    config {
      inputType : string = "text"
      computed : match @config.inputType {
        "number" -> number
        _        -> string
      }
    }
"#,
    );
    let msg = parse_err(&src);
    assert!(
        msg.contains("ELOLO_MATCH_POSITION_UNSUPPORTED"),
        "got: {msg}"
    );
}

#[test]
fn match_forbidden_in_event_type_alias_payload() {
    // The real hazard this gate closes: an Event alias's payload feeds the
    // alias-resolution lowering path (`type_expr_to_payload_fields`), which
    // never reads `type_when` — a match here would silently drop its
    // conditional through for every consumer (local listen or cross-orbital).
    let src = format!(
        r#"
type CHANGED = Event {{
  value : match @config.inputType {{
    "number" -> number
    _        -> string
  }}
}}

{}
"#,
        wrap("    config { inputType : string = \"text\" }\n")
    );
    let msg = parse_err(&src);
    assert!(
        msg.contains("ELOLO_MATCH_POSITION_UNSUPPORTED"),
        "got: {msg}"
    );
}

// ---------------------------------------------------------------------------
// L1 checks — check_match_types (the five ELOLO_MATCH_* codes)
// ---------------------------------------------------------------------------

#[test]
fn undeclared_knob_is_a_lower_error() {
    let src = wrap(
        r#"
    emits {
      CHANGED -> external {
        value : match @config.inputType {
          "number" -> number
          _        -> string
        }
      }
    }
"#,
    );
    let msg = lower_err(&src);
    assert!(msg.contains("ELOLO_MATCH_KNOB_UNDECLARED"), "got: {msg}");
}

#[test]
fn str_and_word_duplicate_spelling_is_a_lower_error() {
    // "number" (Str) and number (Word) are the SAME spelling.
    let src = wrap(
        r#"
    emits {
      CHANGED -> external {
        value : match @config.inputType {
          "number" -> number
          number   -> number
          _        -> string
        }
      }
    }
    config { inputType : string = "text" }
"#,
    );
    let msg = lower_err(&src);
    assert!(msg.contains("ELOLO_MATCH_ARM_DUPLICATE"), "got: {msg}");
}

#[test]
fn arm_outside_boolean_vocabulary_is_a_lower_error() {
    let src = wrap(
        r#"
    emits {
      CHANGED -> external {
        value : match @config.multiple {
          true    -> [string]
          "maybe" -> string
          _       -> string
        }
      }
    }
    config { multiple : boolean = false }
"#,
    );
    let msg = lower_err(&src);
    assert!(msg.contains("ELOLO_MATCH_ARM_UNREACHABLE"), "got: {msg}");
}

#[test]
fn arm_after_wildcard_is_a_lower_error() {
    let src = wrap(
        r#"
    emits {
      CHANGED -> external {
        value : match @config.inputType {
          _        -> string
          "number" -> number
        }
      }
    }
    config { inputType : string = "text" }
"#,
    );
    let msg = lower_err(&src);
    assert!(msg.contains("ELOLO_MATCH_ARM_UNREACHABLE"), "got: {msg}");
}

#[test]
fn missing_wildcard_is_a_lower_error() {
    let src = wrap(
        r#"
    emits {
      CHANGED -> external {
        value : match @config.inputType {
          "number" -> number
        }
      }
    }
    config { inputType : string = "text" }
"#,
    );
    let msg = lower_err(&src);
    assert!(msg.contains("ELOLO_MATCH_WILDCARD_REQUIRED"), "got: {msg}");
}

#[test]
fn object_arm_type_is_a_lower_error() {
    let src = wrap(
        r#"
    emits {
      CHANGED -> external {
        value : match @config.inputType {
          "row" -> { id : string }
          _     -> string
        }
      }
    }
    config { inputType : string = "text" }
"#,
    );
    let msg = lower_err(&src);
    assert!(
        msg.contains("ELOLO_MATCH_ARM_TYPE_UNSUPPORTED"),
        "got: {msg}"
    );
}

#[test]
fn array_of_named_arm_type_is_a_lower_error() {
    // v1 caps array arms to `[Primitive]` — `[Widget]` (a named type) is not.
    let src = wrap(
        r#"
    emits {
      CHANGED -> external {
        value : match @config.inputType {
          "rows" -> [Widget]
          _      -> string
        }
      }
    }
    config { inputType : string = "text" }
"#,
    );
    let msg = lower_err(&src);
    assert!(
        msg.contains("ELOLO_MATCH_ARM_TYPE_UNSUPPORTED"),
        "got: {msg}"
    );
}

// ---------------------------------------------------------------------------
// Hazard hardening — an unrecognized `@sigil` right after a payload field is
// a hard parse error (no annotation-scanning loop survives to swallow it).
// ---------------------------------------------------------------------------

#[test]
fn unknown_sigil_after_a_payload_field_is_a_hard_parse_error() {
    let src = wrap(
        r#"
    emits {
      CHANGED -> external {
        value : string!
        @bogus "nope"
      }
    }
"#,
    );
    // Not asserting a specific code here — the point is this must be a
    // PARSE error (never a silent swallow that produces a confusing later
    // failure or, worse, a wrong-but-successful parse).
    let _ = parse_err(&src);
}

// ---------------------------------------------------------------------------
// Formatter round-trip
// ---------------------------------------------------------------------------

#[test]
fn format_round_trip_is_idempotent_and_semantics_preserving() {
    let source = wrap(EMIT_MATCH);
    let ast1 = parse_str(&source).expect("parse original");
    let formatted1 = format_program(&ast1);
    let ast2 = parse_str(&formatted1).expect("parse formatted output");
    let formatted2 = format_program(&ast2);
    assert_eq!(
        formatted1, formatted2,
        "formatting a formatted match block must be a no-op (idempotent)"
    );

    let schema1 = parse_and_lower_str(&source).expect("lower original");
    let schema2 = parse_and_lower_str(&formatted1).expect("lower formatted");
    let v1 = serde_json::to_value(&schema1).unwrap();
    let v2 = serde_json::to_value(&schema2).unwrap();
    assert_eq!(
        strip_ids(v1),
        strip_ids(v2),
        "the formatted match block must lower to the same schema (modulo fresh ids)"
    );
}

#[test]
fn format_renders_match_multi_line_with_aligned_arms() {
    let ast = parse_str(&wrap(EMIT_MATCH)).expect("parse");
    let formatted = format_program(&ast);
    assert!(
        formatted.contains("match @config.inputType {"),
        "formatted output:\n{formatted}"
    );
    assert!(
        formatted.contains("\"number\" -> number"),
        "formatted output:\n{formatted}"
    );
    // The `_` arm is padded to align its `->` under the widest pattern
    // (`"number"` is 8 chars).
    assert!(
        formatted.contains("_        -> string"),
        "formatted output:\n{formatted}"
    );
    // Whole-field `!` lands after the match's closing `}`, never inside it.
    assert!(formatted.contains("}!"), "formatted output:\n{formatted}");
}

fn strip_ids(value: serde_json::Value) -> serde_json::Value {
    fn is_identity_key(k: &str) -> bool {
        k == "id"
            || k == "ledger"
            || k == "schemaVersion"
            || k.ends_with("Id")
            || k.ends_with("Ids")
    }
    match value {
        serde_json::Value::Object(map) => {
            let mut out = serde_json::Map::new();
            for (k, v) in map {
                if is_identity_key(&k) {
                    continue;
                }
                out.insert(k, strip_ids(v));
            }
            serde_json::Value::Object(out)
        }
        serde_json::Value::Array(items) => {
            serde_json::Value::Array(items.into_iter().map(strip_ids).collect())
        }
        v => v,
    }
}
