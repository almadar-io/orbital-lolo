//! Expression-level `match` (Phase 3b, pattern-matching doc §3b) —
//! s-expression sugar legal in VALUE positions: `(match <scrutinee>
//! (<pattern> <value>) … (_ <value>))`, desugared at lowering to nested
//! `(if (== <scrutinee> <lit>) <val> <else>)`. Covers: parse happy paths,
//! every parser/L1 error form, THE key proof — desugar byte-equivalence vs
//! the hand-written nested-if spelling — typed literals, exhaustiveness,
//! scrutinee purity, effect-head gating, the format round-trip, and nested
//! expression-match. NO corpus `.lolo` edits — self-contained fixtures.

use orbital_core_base::schema::TraitReference;
use orbital_lolo::{format_program, parse_and_lower_str, parse_str, LoloError};

/// One entity with a closed 3-member string field (`op`), a boolean field
/// (`active`), an open string field (`freeform`), and an int field
/// (`priority`) — enough surface for every scrutinee-kind test below.
fn wrap_entity_trait(trait_body: &str) -> String {
    format!(
        r#"orbital MatchExprOrbital {{
  entity Row [runtime] {{
    id : string!
    op : "set" | "del" | "get" = "get"
    active : boolean = false
    freeform : string = ""
    priority : int = 0
    label : string = ""
  }}

  trait Exec -> Row {{
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

fn exec_transitions(
    schema: &orbital_core_base::schema::OrbitalSchema,
) -> Vec<orbital_core_base::schema::TransitionDefinition> {
    trait_named(schema, "Exec")
        .state_machine
        .transitions
        .iter()
        .filter(|t| t.event == "GO")
        .cloned()
        .collect()
}

// ---------------------------------------------------------------------------
// THE key test: desugar byte-equivalence vs the hand-written nested-if
// ---------------------------------------------------------------------------

#[test]
fn match_desugars_byte_equivalent_to_hand_written_nested_if() {
    let match_src = wrap_entity_trait(
        r#"
    state idle {
      INIT -> idle
        (set @entity.op "get")
      GO -> idle
        (set @entity.priority (match @entity.op ("set" 3) ("del" 1) (_ 0)))
    }
"#,
    );
    let nested_if_src = wrap_entity_trait(
        r#"
    state idle {
      INIT -> idle
        (set @entity.op "get")
      GO -> idle
        (set @entity.priority (if (== @entity.op "set") 3 (if (== @entity.op "del") 1 0)))
    }
"#,
    );

    let match_schema = parse_and_lower_str(&match_src).expect("match spelling lowers");
    let nested_if_schema =
        parse_and_lower_str(&nested_if_src).expect("hand-written nested-if lowers");

    let match_transitions = exec_transitions(&match_schema);
    let nested_if_transitions = exec_transitions(&nested_if_schema);

    assert_eq!(match_transitions.len(), 1);
    assert_eq!(nested_if_transitions.len(), 1);

    let match_json = strip_ids(serde_json::to_value(&match_transitions).unwrap());
    let nested_if_json = strip_ids(serde_json::to_value(&nested_if_transitions).unwrap());
    assert_eq!(
        match_json, nested_if_json,
        "the match-spelled effect must desugar BYTE-EQUIVALENT to the hand-written nested-if \
         spelling"
    );

    // Sanity on the actual desugared shape.
    assert_eq!(
        serde_json::to_value(&match_transitions[0].effects).unwrap(),
        serde_json::json!([[
            "set",
            "@entity.priority",
            [
                "if",
                ["==", "@entity.op", "set"],
                3.0,
                ["if", ["==", "@entity.op", "del"], 1.0, 0.0]
            ]
        ]])
    );
}

// ---------------------------------------------------------------------------
// Typed literals
// ---------------------------------------------------------------------------

#[test]
fn bool_and_int_arms_produce_typed_literals_in_the_synthesized_guard() {
    let src = wrap_entity_trait(
        r#"
    state idle {
      INIT -> idle
        (set @entity.active false)
      GO -> idle
        (set @entity.label (match @entity.active (true "yes") (false "no")))
    }
"#,
    );
    let schema = parse_and_lower_str(&src).expect("bool-scrutinee match lowers");
    let transitions = exec_transitions(&schema);
    let effect = &transitions[0].effects.as_ref().unwrap()[0];
    let json = serde_json::to_value(effect).unwrap();
    assert_eq!(
        json,
        serde_json::json!([
            "set",
            "@entity.label",
            ["if", ["==", "@entity.active", true], "yes", "no"]
        ])
    );
}

#[test]
fn int_arm_produces_a_typed_number_literal() {
    let src = wrap_entity_trait(
        r#"
    state idle {
      INIT -> idle
        (set @entity.priority 0)
      GO -> idle
        (set @entity.label (match @entity.priority (1 "one") (_ "other")))
    }
"#,
    );
    let schema = parse_and_lower_str(&src).expect("int-scrutinee match lowers");
    let transitions = exec_transitions(&schema);
    let effect = &transitions[0].effects.as_ref().unwrap()[0];
    let json = serde_json::to_value(effect).unwrap();
    assert_eq!(
        json,
        serde_json::json!([
            "set",
            "@entity.label",
            ["if", ["==", "@entity.priority", 1.0], "one", "other"]
        ])
    );
}

// ---------------------------------------------------------------------------
// Exhaustiveness family
// ---------------------------------------------------------------------------

#[test]
fn closed_boolean_scrutinee_total_coverage_needs_no_wildcard() {
    let src = wrap_entity_trait(
        r#"
    state idle {
      INIT -> idle
        (set @entity.active false)
      GO -> idle
        (set @entity.label (match @entity.active (true "yes") (false "no")))
    }
"#,
    );
    parse_and_lower_str(&src)
        .expect("a fully-covered closed vocabulary needs no `_` arm (the exhaustiveness payoff)");
}

#[test]
fn missing_variant_on_a_closed_vocabulary_without_wildcard_is_inexhaustive() {
    let src = wrap_entity_trait(
        r#"
    state idle {
      INIT -> idle
      GO -> idle
        (set @entity.label (match @entity.op ("set" "s")))
    }
"#,
    );
    let msg = lower_err(&src);
    assert!(msg.contains("ELOLO_MATCH_INEXHAUSTIVE"), "got: {msg}");
    assert!(msg.contains("del"), "got: {msg}");
    assert!(msg.contains("get"), "got: {msg}");
}

#[test]
fn open_scrutinee_type_needs_a_wildcard_arm() {
    let src = wrap_entity_trait(
        r#"
    state idle {
      INIT -> idle
      GO -> idle
        (set @entity.label (match @entity.freeform ("x" "1")))
    }
"#,
    );
    let msg = lower_err(&src);
    assert!(msg.contains("ELOLO_MATCH_WILDCARD_REQUIRED"), "got: {msg}");
}

#[test]
fn arm_after_wildcard_is_unreachable() {
    let src = wrap_entity_trait(
        r#"
    state idle {
      INIT -> idle
      GO -> idle
        (set @entity.label (match @entity.op (_ "x") ("set" "y")))
    }
"#,
    );
    let msg = lower_err(&src);
    assert!(msg.contains("ELOLO_MATCH_ARM_UNREACHABLE"), "got: {msg}");
    assert!(msg.contains("must be last"), "got: {msg}");
}

#[test]
fn duplicate_arm_spelling_is_a_lower_error() {
    let src = wrap_entity_trait(
        r#"
    state idle {
      INIT -> idle
      GO -> idle
        (set @entity.label (match @entity.op ("set" "a") ("set" "b") (_ "c")))
    }
"#,
    );
    let msg = lower_err(&src);
    assert!(msg.contains("ELOLO_MATCH_ARM_DUPLICATE"), "got: {msg}");
}

#[test]
fn arm_outside_closed_vocabulary_is_unreachable() {
    let src = wrap_entity_trait(
        r#"
    state idle {
      INIT -> idle
      GO -> idle
        (set @entity.label (match @entity.op ("flush" "a") (_ "b")))
    }
"#,
    );
    let msg = lower_err(&src);
    assert!(msg.contains("ELOLO_MATCH_ARM_UNREACHABLE"), "got: {msg}");
}

// ---------------------------------------------------------------------------
// Scrutinee purity restriction
// ---------------------------------------------------------------------------

#[test]
fn operator_call_scrutinee_is_a_parse_error() {
    let src = wrap_entity_trait(
        r#"
    state idle {
      INIT -> idle
      GO -> idle
        (set @entity.label (match (+ 1 2) (3 "a") (_ "b")))
    }
"#,
    );
    let msg = parse_err(&src);
    assert!(
        msg.contains("ELOLO_MATCH_EXPR_SCRUTINEE_IMPURE"),
        "got: {msg}"
    );
}

#[test]
fn user_sigil_scrutinee_is_a_parse_error() {
    let src = wrap_entity_trait(
        r#"
    state idle {
      INIT -> idle
      GO -> idle
        (set @entity.label (match @user.role ("admin" "a") (_ "b")))
    }
"#,
    );
    let msg = parse_err(&src);
    assert!(
        msg.contains("ELOLO_MATCH_EXPR_SCRUTINEE_IMPURE"),
        "got: {msg}"
    );
}

#[test]
fn literal_scrutinee_is_accepted_as_pure() {
    let src = wrap_entity_trait(
        r#"
    state idle {
      INIT -> idle
      GO -> idle
        (set @entity.label (match "set" ("set" "matched") (_ "other")))
    }
"#,
    );
    parse_and_lower_str(&src).expect("a plain literal scrutinee is a pure reference too");
}

#[test]
fn question_field_sugar_scrutinee_is_accepted() {
    let src = format!(
        r#"orbital MatchExprPayloadOrbital {{
  entity Unused [runtime] {{
    id : string!
  }}

  trait Dispatch [interaction] {{
    listens {{
      GO {{ kind : "a" | "b" }}
    }}
    state idle {{
      GO -> idle
        (log {{ message: (match ?kind ("a" "first") (_ "other")) }})
    }}
  }}
}}
"#
    );
    parse_and_lower_str(&src).expect("`?field` is the same pure reference as @payload.field");
}

// ---------------------------------------------------------------------------
// Zero non-wildcard arms
// ---------------------------------------------------------------------------

#[test]
fn zero_non_wildcard_arms_is_a_parse_error() {
    let src = wrap_entity_trait(
        r#"
    state idle {
      INIT -> idle
      GO -> idle
        (set @entity.label (match @entity.op (_ "x")))
    }
"#,
    );
    let msg = parse_err(&src);
    assert!(msg.contains("ELOLO_MATCH_EXPR_NO_ARMS"), "got: {msg}");
}

// ---------------------------------------------------------------------------
// Value-position gating: effect-head rejection
// ---------------------------------------------------------------------------

#[test]
fn match_as_a_bare_effect_head_is_a_parse_error() {
    let src = wrap_entity_trait(
        r#"
    state idle {
      INIT -> idle
      GO -> idle
        (match @entity.op ("set" (set @entity.label "a")) (_ (set @entity.label "b")))
    }
"#,
    );
    let msg = parse_err(&src);
    assert!(msg.contains("ELOLO_MATCH_EXPR_EFFECT_HEAD"), "got: {msg}");
}

#[test]
fn match_as_a_guard_expression_is_accepted() {
    let src = wrap_entity_trait(
        r#"
    state idle {
      INIT -> idle
      GO -> idle when (match @entity.op (true "unused") (_ (== @entity.op "get")))
        (set @entity.label "matched")
    }
"#,
    );
    // Contrived (a match producing a guard's boolean isn't idiomatic) but
    // legal per the grammar — guard is a value position like any other.
    let _ = parse_and_lower_str(&src);
}

// ---------------------------------------------------------------------------
// Regression: a `let` binding NAMED `match` must not be mistaken for
// zero-arm match sugar (found live in the corpus: std-civics-checks.lolo,
// std-learn-derivation.lolo both bind `(match (array/first …))`).
// ---------------------------------------------------------------------------

#[test]
fn let_binding_named_match_is_not_mistaken_for_match_sugar() {
    let src = wrap_entity_trait(
        r#"
    state idle {
      INIT -> idle
        (set @entity.label
          (let (
              (match (array/first [1, 2, 3]))
              (isMatch (!= @match null)))
            (if @isMatch "found" "missing")))
    }
"#,
    );
    let schema = parse_and_lower_str(&src)
        .expect("a let-binding literally named `match` must parse and lower unchanged");
    let transitions = trait_named(&schema, "Exec")
        .state_machine
        .transitions
        .iter()
        .filter(|t| t.event == "INIT")
        .cloned()
        .collect::<Vec<_>>();
    let effect = &transitions[0].effects.as_ref().unwrap()[0];
    let json = serde_json::to_value(effect).unwrap();
    // The `let` binding's `match` name/value pair survives completely
    // unrewritten — desugar only recurses into the VALUE, never mistakes
    // the 2-element `[name, value]` pair itself for match sugar.
    assert_eq!(
        json,
        serde_json::json!([
            "set",
            "@entity.label",
            [
                "let",
                [
                    ["match", ["array/first", [1.0, 2.0, 3.0]]],
                    ["isMatch", ["!=", "@match", null]]
                ],
                ["if", "@isMatch", "found", "missing"]
            ]
        ])
    );
}

// ---------------------------------------------------------------------------
// Nested expression-match
// ---------------------------------------------------------------------------

#[test]
fn nested_match_in_an_arm_value_desugars_recursively() {
    let src = wrap_entity_trait(
        r#"
    state idle {
      INIT -> idle
        (set @entity.op "get")
        (set @entity.active false)
      GO -> idle
        (set @entity.label (match @entity.op ("set" (match @entity.active (true "set-on") (false "set-off"))) (_ "other")))
    }
"#,
    );
    let schema = parse_and_lower_str(&src).expect("nested expr-match lowers");
    let transitions = exec_transitions(&schema);
    let effect = &transitions[0].effects.as_ref().unwrap()[0];
    let json = serde_json::to_value(effect).unwrap();
    assert_eq!(
        json,
        serde_json::json!([
            "set",
            "@entity.label",
            [
                "if",
                ["==", "@entity.op", "set"],
                ["if", ["==", "@entity.active", true], "set-on", "set-off"],
                "other"
            ]
        ])
    );
}

// ---------------------------------------------------------------------------
// Formatter round-trip
// ---------------------------------------------------------------------------

#[test]
fn format_round_trip_is_idempotent_and_semantics_preserving() {
    let source = wrap_entity_trait(
        r#"
    state idle {
      INIT -> idle
        (set @entity.op "get")
      GO -> idle
        (set @entity.priority (match @entity.op ("set" 3) ("del" 1) (_ 0)))
    }
"#,
    );
    let ast1 = parse_str(&source).expect("parse original");
    let formatted1 = format_program(&ast1);
    let ast2 = parse_str(&formatted1).expect("parse formatted output");
    let formatted2 = format_program(&ast2);
    assert_eq!(
        formatted1, formatted2,
        "formatting a formatted expr-match block must be idempotent"
    );
    assert!(
        !formatted1.contains("when"),
        "the expr-match grammar never introduces `when` — got:\n{formatted1}"
    );
    assert!(
        formatted1.contains("match @entity.op"),
        "got:\n{formatted1}"
    );

    let schema1 = parse_and_lower_str(&source).expect("lower original");
    let schema2 = parse_and_lower_str(&formatted1).expect("lower formatted");
    let v1 = strip_ids(serde_json::to_value(&schema1).unwrap());
    let v2 = strip_ids(serde_json::to_value(&schema2).unwrap());
    assert_eq!(
        v1, v2,
        "the formatted expr-match block must lower to the same schema (modulo fresh ids)"
    );
}
