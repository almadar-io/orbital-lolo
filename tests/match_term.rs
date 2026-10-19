//! Transition-position term-level `match` (Phase 2, pattern-matching doc
//! §1(a)) — sugar over ordered guarded transitions. Covers: parse happy
//! paths, every parser/L1 error form, the format round-trip, and THE key
//! proof — the match spelling of a redis-EXEC-shaped guard triad lowers to
//! `.orb` transitions BYTE-EQUIVALENT (modulo ids) to the hand-guarded
//! spelling's lowering. NO corpus `.lolo` edits — this file's fixtures are
//! self-contained.

use orbital_core_base::schema::TraitReference;
use orbital_lolo::{format_program, parse_and_lower_str, parse_str, LoloError};

/// One entity (`op`/`key` fields, closed 3-member `op` vocabulary) shared by
/// every entity-scrutinee test below — the same shape a service atom's
/// `op` field has (a closed dispatch vocabulary set from config at INIT).
fn wrap_entity_trait(trait_body: &str) -> String {
    format!(
        r#"orbital MatchTermOrbital {{
  entity RedisRow [runtime] {{
    id : string!
    op : "set" | "del" | "get" = "get"
    key : string = ""
  }}

  trait RedisExec -> RedisRow [interaction] {{
{trait_body}
  }}
}}
"#
    )
}

/// A trait with no `linkedEntity`, for `@payload.<f>` scrutinee tests. The
/// orbital still needs SOME entity declared (an unrelated `Unused` one) —
/// `PayloadDispatch` itself just doesn't bind to it.
fn wrap_payload_trait(trait_body: &str) -> String {
    format!(
        r#"orbital MatchTermPayloadOrbital {{
  entity Unused [runtime] {{
    id : string!
  }}

  trait PayloadDispatch [interaction] {{
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

// ---------------------------------------------------------------------------
// THE key test: lowering byte-equivalence vs the hand-guarded spelling
// ---------------------------------------------------------------------------

#[test]
fn match_lowers_to_the_same_transitions_as_the_hand_guarded_spelling() {
    let source = wrap_entity_trait(
        r#"
    state idle {
      INIT -> idle
        (set @entity.op "get")
        (set @entity.key "k")
      EXEC -> executing match @entity.op {
        "set" -> (call-service test-svc set { key: @entity.key } { emit: { failure: FAILED, success: RedisDone } })
        "del" -> (call-service test-svc delete { key: @entity.key } { emit: { failure: FAILED, success: RedisDone } })
        _     -> (call-service test-svc get { key: @entity.key } { emit: { failure: FAILED, success: RedisDone } })
      }
    }
    state executing {
      RedisDone -> idle
      FAILED -> idle
    }
"#,
    );
    let guarded = r#"orbital MatchTermOrbitalGuarded {
  entity RedisRow [runtime] {
    id : string!
    op : "set" | "del" | "get" = "get"
    key : string = ""
  }

  trait RedisExec -> RedisRow [interaction] {
    state idle {
      INIT -> idle
        (set @entity.op "get")
        (set @entity.key "k")
      EXEC -> executing when (== @entity.op "set")
        (call-service test-svc set { key: @entity.key } { emit: { failure: FAILED, success: RedisDone } })
      EXEC -> executing when (== @entity.op "del")
        (call-service test-svc delete { key: @entity.key } { emit: { failure: FAILED, success: RedisDone } })
      EXEC -> executing
        (call-service test-svc get { key: @entity.key } { emit: { failure: FAILED, success: RedisDone } })
    }
    state executing {
      RedisDone -> idle
      FAILED -> idle
    }
  }
}
"#
    .to_string();

    let match_schema = parse_and_lower_str(&source).expect("match spelling lowers");
    let guarded_schema = parse_and_lower_str(&guarded).expect("hand-guarded spelling lowers");

    let match_transitions = trait_named(&match_schema, "RedisExec")
        .state_machine
        .transitions
        .iter()
        .filter(|t| t.event == "EXEC")
        .cloned()
        .collect::<Vec<_>>();
    let guarded_transitions = trait_named(&guarded_schema, "RedisExec")
        .state_machine
        .transitions
        .iter()
        .filter(|t| t.event == "EXEC")
        .cloned()
        .collect::<Vec<_>>();

    assert_eq!(match_transitions.len(), 3, "one transition per arm");
    assert_eq!(
        guarded_transitions.len(),
        3,
        "one transition per hand-written guard"
    );

    let match_json = strip_ids(serde_json::to_value(&match_transitions).unwrap());
    let guarded_json = strip_ids(serde_json::to_value(&guarded_transitions).unwrap());
    assert_eq!(
        match_json, guarded_json,
        "match-spelled EXEC triad must lower BYTE-EQUIVALENT (modulo ids) to the \
         hand-guarded spelling: same guards, same order, `_` = bare arm"
    );

    // Guard shape itself: `["==", "@entity.op", "set"]`, mirroring the
    // corpus's own `["==", "@config.op", "set"]` .orb shape exactly.
    let set_guard = match_transitions[0].guard.as_ref().expect("guard present");
    assert_eq!(
        serde_json::to_value(set_guard).unwrap(),
        serde_json::json!(["==", "@entity.op", "set"])
    );
    assert!(
        match_transitions[2].guard.is_none(),
        "`_` arm lowers to the unguarded transition"
    );
}

// ---------------------------------------------------------------------------
// Happy paths
// ---------------------------------------------------------------------------

#[test]
fn payload_scrutinee_match_lowers() {
    let source = wrap_payload_trait(
        r#"
    listens {
      DISPATCH { kind : "a" | "b" }
    }
    state idle {
      DISPATCH -> idle match @payload.kind {
        "a" -> (log { message: "a" })
        "b" -> (log { message: "b" })
        _   -> (log { message: "other" })
      }
    }
"#,
    );
    let schema = parse_and_lower_str(&source).expect("payload scrutinee match lowers");
    let trait_def = trait_named(&schema, "PayloadDispatch");
    let dispatch: Vec<_> = trait_def
        .state_machine
        .transitions
        .iter()
        .filter(|t| t.event == "DISPATCH")
        .collect();
    assert_eq!(dispatch.len(), 3);
    assert_eq!(
        serde_json::to_value(dispatch[0].guard.as_ref().unwrap()).unwrap(),
        serde_json::json!(["==", "@payload.kind", "a"])
    );
    assert!(dispatch[2].guard.is_none());
}

#[test]
fn wildcard_optional_when_closed_vocabulary_is_fully_covered() {
    let source = r#"orbital ToggleOrbital {
  entity ToggleRow [runtime] {
    id : string!
    active : boolean = false
  }

  trait ToggleExec -> ToggleRow [interaction] {
    state idle {
      INIT -> idle
        (set @entity.active false)
      FLIP -> idle match @entity.active {
        true  -> (set @entity.active false)
        false -> (set @entity.active true)
      }
    }
  }
}
"#
    .to_string();
    let schema = parse_and_lower_str(&source)
        .expect("a fully-covered closed vocabulary needs no `_` arm (the exhaustiveness payoff)");
    let trait_def = trait_named(&schema, "ToggleExec");
    let flips: Vec<_> = trait_def
        .state_machine
        .transitions
        .iter()
        .filter(|t| t.event == "FLIP")
        .collect();
    assert_eq!(flips.len(), 2);
    assert!(flips.iter().all(|t| t.guard.is_some()), "no `_` arm here");
}

#[test]
fn ensures_on_a_match_transition_is_owed_by_every_arm_and_formats_back() {
    let source = r#"orbital ToggleOrbital {
  entity ToggleRow [runtime] {
    id : string!
    active : boolean = false
  }

  trait ToggleExec -> ToggleRow [interaction] {
    state idle {
      INIT -> idle
        (set @entity.active false)
      FLIP -> idle
        @ensures (!= @entity.active @fromEntity.active)
        match @entity.active {
        true  -> (set @entity.active false)
        false -> (set @entity.active true)
      }
    }
  }
}
"#;
    let schema = parse_and_lower_str(source).expect("@ensures before `match` parses");
    let trait_def = trait_named(&schema, "ToggleExec");
    let flips: Vec<_> = trait_def.state_machine.transitions.iter().filter(|t| t.event == "FLIP").collect();
    assert_eq!(flips.len(), 2);
    let owed = serde_json::json!([["!=", "@entity.active", "@fromEntity.active"]]);
    for t in &flips {
        assert_eq!(serde_json::to_value(&t.ensures).unwrap(), owed, "every expanded arm owes the contract");
    }
    let init = trait_def.state_machine.transitions.iter().find(|t| t.event == "INIT").unwrap();
    assert!(init.ensures.is_none(), "control: INIT owes nothing");
    let printed = format_program(&parse_str(source).unwrap());
    assert!(printed.contains("@ensures (!= @entity.active @fromEntity.active)"), "{printed}");
    let again = parse_and_lower_str(&printed).expect("formatted source re-parses");
    let again_flips = trait_named(&again, "ToggleExec").state_machine.transitions.iter().filter(|t| t.event == "FLIP" && t.ensures.is_some()).count();
    assert_eq!(again_flips, 2, "{printed}");
}

// ---------------------------------------------------------------------------
// Parser errors
// ---------------------------------------------------------------------------

#[test]
fn match_and_when_together_is_a_parse_error() {
    let src = wrap_entity_trait(
        r#"
    state idle {
      INIT -> idle (set @entity.op "get")
      EXEC -> executing when (== @entity.op "set") match @entity.op {
        "set" -> (set @entity.op "set")
        _     -> (set @entity.op "get")
      }
    }
"#,
    );
    let msg = parse_err(&src);
    assert!(msg.contains("ELOLO_MATCH_TERM_WITH_WHEN"), "got: {msg}");
}

#[test]
fn config_scrutinee_is_a_parse_error() {
    let src = wrap_entity_trait(
        r#"
    state idle {
      INIT -> idle (set @entity.op "get")
      EXEC -> executing match @config.op {
        "set" -> (set @entity.op "set")
        _     -> (set @entity.op "get")
      }
    }
"#,
    );
    let msg = parse_err(&src);
    assert!(
        msg.contains("ELOLO_MATCH_TERM_SCRUTINEE_CONFIG"),
        "got: {msg}"
    );
}

#[test]
fn user_scrutinee_is_a_parse_error() {
    let src = wrap_entity_trait(
        r#"
    state idle {
      INIT -> idle (set @entity.op "get")
      EXEC -> executing match @user.role {
        "admin" -> (set @entity.op "set")
        _       -> (set @entity.op "get")
      }
    }
"#,
    );
    let msg = parse_err(&src);
    assert!(
        msg.contains("ELOLO_MATCH_TERM_SCRUTINEE_USER"),
        "got: {msg}"
    );
}

#[test]
fn unsupported_scrutinee_root_is_a_parse_error() {
    let src = wrap_entity_trait(
        r#"
    state idle {
      INIT -> idle (set @entity.op "get")
      EXEC -> executing match @now {
        "set" -> (set @entity.op "set")
        _     -> (set @entity.op "get")
      }
    }
"#,
    );
    let msg = parse_err(&src);
    assert!(
        msg.contains("ELOLO_MATCH_TERM_SCRUTINEE_UNSUPPORTED"),
        "got: {msg}"
    );
}

// ---------------------------------------------------------------------------
// L1 checks — check_term_match_types
// ---------------------------------------------------------------------------

#[test]
fn undeclared_entity_field_is_a_lower_error() {
    let src = wrap_entity_trait(
        r#"
    state idle {
      INIT -> idle (set @entity.op "get")
      EXEC -> executing match @entity.nope {
        "set" -> (set @entity.op "set")
        _     -> (set @entity.op "get")
      }
    }
"#,
    );
    let msg = lower_err(&src);
    assert!(
        msg.contains("ELOLO_MATCH_SCRUTINEE_UNDECLARED"),
        "got: {msg}"
    );
    assert!(msg.contains("nope"), "got: {msg}");
}

#[test]
fn entity_scrutinee_without_linked_entity_is_a_lower_error() {
    let src = wrap_payload_trait(
        r#"
    state idle {
      INIT -> idle
      EXEC -> idle match @entity.op {
        "set" -> (log { message: "set" })
        _     -> (log { message: "other" })
      }
    }
"#,
    );
    let msg = lower_err(&src);
    assert!(
        msg.contains("ELOLO_MATCH_SCRUTINEE_UNDECLARED"),
        "got: {msg}"
    );
    assert!(msg.contains("linkedEntity"), "got: {msg}");
}

#[test]
fn undeclared_payload_field_is_a_lower_error() {
    let src = wrap_payload_trait(
        r#"
    listens {
      DISPATCH { kind : "a" | "b" }
    }
    state idle {
      DISPATCH -> idle match @payload.nope {
        "a" -> (log { message: "a" })
        _   -> (log { message: "other" })
      }
    }
"#,
    );
    let msg = lower_err(&src);
    assert!(
        msg.contains("ELOLO_MATCH_SCRUTINEE_UNDECLARED"),
        "got: {msg}"
    );
    assert!(msg.contains("nope"), "got: {msg}");
}

#[test]
fn payload_scrutinee_on_an_event_with_no_declared_payload_is_a_lower_error() {
    let src = wrap_payload_trait(
        r#"
    state idle {
      DISPATCH -> idle match @payload.kind {
        "a" -> (log { message: "a" })
        _   -> (log { message: "other" })
      }
    }
"#,
    );
    let msg = lower_err(&src);
    assert!(
        msg.contains("ELOLO_MATCH_SCRUTINEE_UNDECLARED"),
        "got: {msg}"
    );
}

#[test]
fn missing_arm_on_a_closed_vocabulary_without_wildcard_is_inexhaustive() {
    let src = wrap_entity_trait(
        r#"
    state idle {
      INIT -> idle (set @entity.op "get")
      EXEC -> executing match @entity.op {
        "set" -> (set @entity.op "set")
      }
    }
    state executing {
      DONE -> idle
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
    let src = r#"orbital FreeOrbital {
  entity FreeRow [runtime] {
    id : string!
    op : string = ""
  }

  trait FreeExec -> FreeRow [interaction] {
    state idle {
      INIT -> idle
        (set @entity.op "x")
      EXEC -> executing match @entity.op {
        "a" -> (set @entity.op "a")
      }
    }
    state executing {
      DONE -> idle
    }
  }
}
"#
    .to_string();
    let msg = lower_err(&src);
    assert!(msg.contains("ELOLO_MATCH_WILDCARD_REQUIRED"), "got: {msg}");
}

#[test]
fn duplicate_arm_spelling_is_a_lower_error() {
    let src = wrap_entity_trait(
        r#"
    state idle {
      INIT -> idle (set @entity.op "get")
      EXEC -> executing match @entity.op {
        "set" -> (set @entity.op "set")
        "set" -> (set @entity.op "set")
        _     -> (set @entity.op "get")
      }
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
      INIT -> idle (set @entity.op "get")
      EXEC -> executing match @entity.op {
        "flush" -> (set @entity.op "get")
        _       -> (set @entity.op "get")
      }
    }
"#,
    );
    let msg = lower_err(&src);
    assert!(msg.contains("ELOLO_MATCH_ARM_UNREACHABLE"), "got: {msg}");
}

#[test]
fn arm_after_wildcard_is_unreachable() {
    let src = wrap_entity_trait(
        r#"
    state idle {
      INIT -> idle (set @entity.op "get")
      EXEC -> executing match @entity.op {
        _     -> (set @entity.op "get")
        "set" -> (set @entity.op "set")
      }
    }
"#,
    );
    let msg = lower_err(&src);
    assert!(msg.contains("ELOLO_MATCH_ARM_UNREACHABLE"), "got: {msg}");
    assert!(msg.contains("must be last"), "got: {msg}");
}

// ---------------------------------------------------------------------------
// Phase 3a — dotted destructuring scrutinees over tagged struct unions
// ---------------------------------------------------------------------------

/// A tagged struct union payload (`DrawItem = DrawShape | DrawText`,
/// discriminated by each variant's single-valued closed `type` member; both
/// variants also share the OPEN `layer` member, so a non-discriminating
/// member exists for the negative tests).
fn wrap_draw_trait(trait_body: &str) -> String {
    format!(
        r#"orbital DrawOrbital {{
  type DrawShape = {{ type : "draw-shape"!, width : number, height : number, layer : number }}
  type DrawText = {{ type : "draw-text"!, text : string, layer : number }}
  type DrawItem = DrawShape | DrawText

  entity DrawState [runtime] {{
    id : string!
    lastWidth : number = 0
    lastText : string = ""
  }}

  trait DrawDispatch -> DrawState [lifecycle, instance] {{
{trait_body}
  }}
}}
"#
    )
}

#[test]
fn dotted_union_scrutinee_lowers_to_dotted_discriminant_guards() {
    let source = wrap_draw_trait(
        r#"
    listens {
      DISPATCH { item : DrawItem! }
    }
    state idle {
      INIT -> idle
        (set @entity.lastWidth 0)
        (set @entity.lastText "")
      DISPATCH -> idle match @payload.item.type {
        "draw-shape" -> (set @entity.lastWidth @payload.item.width)
        "draw-text"  -> (set @entity.lastText @payload.item.text)
      }
    }
"#,
    );
    let schema = parse_and_lower_str(&source).expect("dotted destructuring match lowers");
    let dispatch: Vec<_> = trait_named(&schema, "DrawDispatch")
        .state_machine
        .transitions
        .iter()
        .filter(|t| t.event == "DISPATCH")
        .collect();
    assert_eq!(
        dispatch.len(),
        2,
        "one transition per arm; total discriminant coverage needs no `_`"
    );
    // Byte-level guard shape: the synthesized guard is exactly the s-expr
    // JSON of `(== @payload.item.type "draw-shape")`.
    assert_eq!(
        serde_json::to_value(dispatch[0].guard.as_ref().unwrap()).unwrap(),
        serde_json::json!(["==", "@payload.item.type", "draw-shape"])
    );
    assert_eq!(
        serde_json::to_value(dispatch[1].guard.as_ref().unwrap()).unwrap(),
        serde_json::json!(["==", "@payload.item.type", "draw-text"])
    );
}

#[test]
fn entity_rooted_dotted_scrutinee_lowers_with_wildcard_unguarded_last() {
    let source = r#"orbital DrawEntityOrbital {
  type DrawShape = { type : "draw-shape"!, width : number }
  type DrawText = { type : "draw-text"!, text : string }
  type DrawItem = DrawShape | DrawText

  entity DrawState [runtime] {
    id : string!
    item : DrawItem
    lastWidth : number = 0
  }

  trait DrawApply -> DrawState [lifecycle, instance] {
    listens {
      DISPATCH { item : DrawItem! }
    }
    state idle {
      DISPATCH -> idle
        (set @entity.item @payload.item)
      APPLY -> idle match @entity.item.type {
        "draw-shape" -> (set @entity.lastWidth 1)
        _            -> (set @entity.lastWidth 0)
      }
    }
  }
}
"#
    .to_string();
    let schema = parse_and_lower_str(&source).expect("entity-rooted dotted match lowers");
    let apply: Vec<_> = trait_named(&schema, "DrawApply")
        .state_machine
        .transitions
        .iter()
        .filter(|t| t.event == "APPLY")
        .collect();
    assert_eq!(apply.len(), 2);
    assert_eq!(
        serde_json::to_value(apply[0].guard.as_ref().unwrap()).unwrap(),
        serde_json::json!(["==", "@entity.item.type", "draw-shape"])
    );
    assert!(apply[1].guard.is_none(), "`_` arm lowers unguarded, last");
}

#[test]
fn dotted_union_missing_variant_is_inexhaustive_and_named() {
    let source = wrap_draw_trait(
        r#"
    listens {
      DISPATCH { item : DrawItem! }
    }
    state idle {
      INIT -> idle
        (set @entity.lastWidth 0)
        (set @entity.lastText "")
      DISPATCH -> idle match @payload.item.type {
        "draw-shape" -> (set @entity.lastWidth @payload.item.width)
      }
    }
"#,
    );
    let msg = lower_err(&source);
    assert!(msg.contains("ELOLO_MATCH_INEXHAUSTIVE"), "got: {msg}");
    assert!(
        msg.contains("draw-text"),
        "the missing discriminant literal must be NAMED — got: {msg}"
    );
}

#[test]
fn dotted_scrutinee_on_a_non_union_field_is_rejected() {
    let source = wrap_draw_trait(
        r#"
    listens {
      DISPATCH { kind : string }
    }
    state idle {
      INIT -> idle
        (set @entity.lastWidth 0)
        (set @entity.lastText "")
      DISPATCH -> idle match @payload.kind.type {
        "x" -> (set @entity.lastWidth 1)
        _   -> (set @entity.lastWidth 0)
      }
    }
"#,
    );
    let msg = lower_err(&source);
    assert!(
        msg.contains("ELOLO_MATCH_SCRUTINEE_NOT_UNION"),
        "got: {msg}"
    );
    assert!(msg.contains("kind"), "got: {msg}");
}

#[test]
fn member_missing_from_a_variant_is_not_a_discriminant() {
    let source = wrap_draw_trait(
        r#"
    listens {
      DISPATCH { item : DrawItem! }
    }
    state idle {
      INIT -> idle
        (set @entity.lastWidth 0)
        (set @entity.lastText "")
      DISPATCH -> idle match @payload.item.text {
        "1" -> (set @entity.lastWidth 1)
        _   -> (set @entity.lastWidth 0)
      }
    }
"#,
    );
    let msg = lower_err(&source);
    assert!(
        msg.contains("ELOLO_MATCH_DISCRIMINANT_INVALID"),
        "got: {msg}"
    );
    assert!(
        msg.contains("DrawShape") && msg.contains("has no member"),
        "the variant lacking the member must be named — got: {msg}"
    );
}

#[test]
fn open_member_shared_by_every_variant_is_not_a_discriminant() {
    let source = wrap_draw_trait(
        r#"
    listens {
      DISPATCH { item : DrawItem! }
    }
    state idle {
      INIT -> idle
        (set @entity.lastWidth 0)
        (set @entity.lastText "")
      DISPATCH -> idle match @payload.item.layer {
        "1" -> (set @entity.lastWidth 1)
        _   -> (set @entity.lastWidth 0)
      }
    }
"#,
    );
    let msg = lower_err(&source);
    assert!(
        msg.contains("ELOLO_MATCH_DISCRIMINANT_INVALID"),
        "got: {msg}"
    );
    assert!(
        msg.contains("single-valued closed member"),
        "an open (non-string-literal) member can't discriminate — got: {msg}"
    );
}

#[test]
fn ambiguous_discriminant_literal_is_rejected() {
    let source = r#"orbital AmbiguousOrbital {
  type VarA = { type : "same"!, a : number }
  type VarB = { type : "same"!, b : string }
  type Item = VarA | VarB

  entity Row [runtime] {
    id : string!
    n : number = 0
  }

  trait Dispatch -> Row [lifecycle, instance] {
    listens {
      DISPATCH { item : Item! }
    }
    state idle {
      INIT -> idle
        (set @entity.n 0)
      DISPATCH -> idle match @payload.item.type {
        "same" -> (set @entity.n 1)
        _      -> (set @entity.n 0)
      }
    }
  }
}
"#
    .to_string();
    let msg = lower_err(&source);
    assert!(
        msg.contains("ELOLO_MATCH_DISCRIMINANT_INVALID"),
        "got: {msg}"
    );
    assert!(
        msg.contains("exactly one variant"),
        "two variants sharing a literal can't be told apart — got: {msg}"
    );
}

#[test]
fn three_segment_scrutinee_is_a_parse_error() {
    let src = wrap_entity_trait(
        r#"
    state idle {
      INIT -> idle (set @entity.op "get")
      EXEC -> executing match @payload.item.type.x {
        "set" -> (set @entity.op "set")
        _     -> (set @entity.op "get")
      }
    }
"#,
    );
    let msg = parse_err(&src);
    assert!(
        msg.contains("ELOLO_MATCH_TERM_SCRUTINEE_INVALID"),
        "got: {msg}"
    );
}

#[test]
fn dotted_scrutinee_format_round_trip_is_idempotent() {
    let source = wrap_draw_trait(
        r#"
    listens {
      DISPATCH { item : DrawItem! }
    }
    state idle {
      INIT -> idle
        (set @entity.lastWidth 0)
        (set @entity.lastText "")
      DISPATCH -> idle match @payload.item.type {
        "draw-shape" -> (set @entity.lastWidth @payload.item.width)
        "draw-text"  -> (set @entity.lastText @payload.item.text)
      }
    }
"#,
    );
    let ast1 = parse_str(&source).expect("parse original");
    let formatted1 = format_program(&ast1);
    let ast2 = parse_str(&formatted1).expect("parse formatted output");
    let formatted2 = format_program(&ast2);
    assert_eq!(
        formatted1, formatted2,
        "formatting a formatted dotted-scrutinee match must be idempotent"
    );
    assert!(
        formatted1.contains("match @payload.item.type {"),
        "the dotted spelling survives formatting — got:\n{formatted1}"
    );

    let schema1 = parse_and_lower_str(&source).expect("lower original");
    let schema2 = parse_and_lower_str(&formatted1).expect("lower formatted");
    let v1 = strip_ids(serde_json::to_value(&schema1).unwrap());
    let v2 = strip_ids(serde_json::to_value(&schema2).unwrap());
    assert_eq!(v1, v2, "formatted dotted match lowers to the same schema");
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
        (set @entity.key "k")
      EXEC -> executing match @entity.op {
        "set" -> (call-service test-svc set { key: @entity.key } { emit: { failure: FAILED, success: RedisDone } })
        "del" -> (call-service test-svc delete { key: @entity.key } { emit: { failure: FAILED, success: RedisDone } })
        _     -> (call-service test-svc get { key: @entity.key } { emit: { failure: FAILED, success: RedisDone } })
      }
    }
    state executing {
      RedisDone -> idle
      FAILED -> idle
    }
"#,
    );
    let ast1 = parse_str(&source).expect("parse original");
    let formatted1 = format_program(&ast1);
    let ast2 = parse_str(&formatted1).expect("parse formatted output");
    let formatted2 = format_program(&ast2);
    assert_eq!(
        formatted1, formatted2,
        "formatting a formatted term-match block must be idempotent"
    );
    assert!(
        !formatted1.contains("when"),
        "the term-match grammar never introduces `when` — got:\n{formatted1}"
    );
    assert!(
        formatted1.contains("match @entity.op {"),
        "`{{` opens on the match line — got:\n{formatted1}"
    );
    assert!(formatted1.contains("_ ->"), "got:\n{formatted1}");

    let schema1 = parse_and_lower_str(&source).expect("lower original");
    let schema2 = parse_and_lower_str(&formatted1).expect("lower formatted");
    let v1 = strip_ids(serde_json::to_value(&schema1).unwrap());
    let v2 = strip_ids(serde_json::to_value(&schema2).unwrap());
    assert_eq!(
        v1, v2,
        "the formatted term-match block must lower to the same schema (modulo fresh ids)"
    );
}
