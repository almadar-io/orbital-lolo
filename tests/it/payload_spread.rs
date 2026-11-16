//! Payload spread (P-040): `...T` inside an event payload puts a struct's fields flat beside the payload's own.
//! A struct alias of the file expands at lowering; a trait type parameter lowers to a `payloadSpread` marker the
//! inline phase fills from the call site's argument (or the parameter's declared default).

use orbital_core_base::schema::types::TraitReference;
use orbital_lolo::{parse_and_lower_str, LoloError, LowerError, TypeError};

fn src(header: &str, emits: &str, listens: &str) -> String {
    format!(
        r#"
type ListReq = {{ search : string?, limit : number? }}

orbital Relay {{
  entity RelayRun [runtime] {{
    id : string!
  }}

  trait Front {header} -> RelayRun {{
    state idle {{
      INIT -> idle (fetch RelayRun)
    }}
    emits
      {emits}
    listens {{
      {listens}
    }}
  }}

  page "/relay" -> Front
}}
"#
    )
}

fn names(fields: &[orbital_core_base::schema::types::PayloadField]) -> Vec<&str> {
    fields.iter().map(|f| f.name.as_str()).collect()
}

fn front(src: &str) -> orbital_core_base::schema::types::TraitDefinition {
    let schema = parse_and_lower_str(src).expect("parse_and_lower");
    let TraitReference::Inline(def) = schema.orbitals[0].traits[0].clone() else {
        panic!("an inline trait");
    };
    (*def).clone()
}

#[test]
fn a_spread_struct_alias_expands_flat_at_lowering() {
    let def = front(&src("", "Asked { ...ListReq, key : string! }", "ASK -> input { key : string! }"));
    let emit = def.emits.as_ref().unwrap().iter().find(|e| e.event == "Asked").unwrap();
    assert_eq!(names(emit.payload_schema.as_ref().unwrap()), ["search", "limit", "key"]);
    assert!(emit.payload_spread.is_none());
}

#[test]
fn a_spread_type_parameter_lowers_to_a_spread_marker_beside_the_payloads_own_fields() {
    let def = front(&src("(q : Type = ListReq)", "Asked { ...q, key : string! }", "ASK -> input { key : string! }"));
    let emit = def.emits.as_ref().unwrap().iter().find(|e| e.event == "Asked").unwrap();
    assert_eq!(emit.payload_spread.as_deref(), Some("$q"));
    assert_eq!(names(emit.payload_schema.as_ref().unwrap()), ["key"]);
}

#[test]
fn a_spread_type_parameter_in_a_declared_input_lowers_to_a_spread_marker() {
    let def = front(&src("(q : Type = ListReq)", "Asked { key : string! }", "ASK -> input { ...q, key : string!, pageBound : boolean? }"));
    let events = def.state_machine.events.as_ref().unwrap();
    let ask = events.iter().find(|e| e.key == "ASK").unwrap();
    assert_eq!(ask.payload_spread.as_deref(), Some("$q"));
    assert_eq!(names(ask.payload_schema.as_ref().unwrap()), ["key", "pageBound"]);
}

#[test]
fn control_a_payload_without_a_spread_carries_no_marker() {
    let def = front(&src("(q : Type = ListReq)", "Asked { key : string! }", "ASK -> input { key : string! }"));
    let emit = def.emits.as_ref().unwrap().iter().find(|e| e.event == "Asked").unwrap();
    assert!(emit.payload_spread.is_none());
}

#[test]
fn a_spread_type_parameter_without_a_default_is_refused() {
    let err = parse_and_lower_str(&src("(q : Type)", "Asked { ...q, key : string! }", "ASK -> input { key : string! }"))
        .expect_err("a spread needs a default to validate alone");
    assert!(matches!(&err, LoloError::Lower { inner, .. } if matches!(inner.innermost(), LowerError::PayloadSpreadNoDefault { .. })), "{err:?}");
}

#[test]
fn a_spread_of_an_unknown_name_is_an_unknown_type() {
    let err = parse_and_lower_str(&src("", "Asked { ...Nowhere, key : string! }", "ASK -> input { key : string! }"))
        .expect_err("an unknown spread");
    assert!(matches!(&err, LoloError::Lower { inner, .. } if matches!(inner.innermost(), LowerError::Type(TypeError::UnknownType { .. }))), "{err:?}");
}

#[test]
fn a_spread_of_a_type_that_is_not_a_struct_is_refused() {
    let with_enum = src("", "Asked { ...Mode, key : string! }", "ASK -> input { key : string! }").replace("type ListReq", "type Mode = \"a\" | \"b\"\ntype ListReq");
    let err = parse_and_lower_str(&with_enum).expect_err("an enum spread");
    assert!(matches!(&err, LoloError::Lower { inner, .. } if matches!(inner.innermost(), LowerError::PayloadSpreadUnknown { .. })), "{err:?}");
}

#[test]
fn a_spread_field_that_collides_with_an_own_field_is_refused() {
    let err = parse_and_lower_str(&src("", "Asked { ...ListReq, search : string }", "ASK -> input { key : string! }"))
        .expect_err("a collision");
    assert!(matches!(&err, LoloError::Lower { inner, .. } if matches!(inner.innermost(), LowerError::PayloadFieldCollision { .. })), "{err:?}");
}

#[test]
fn two_spreads_in_one_payload_are_refused() {
    let err = parse_and_lower_str(&src("(q : Type = ListReq, r : Type = ListReq)", "Asked { ...q, ...r }", "ASK -> input { key : string! }"))
        .expect_err("one spread per payload");
    assert!(matches!(&err, LoloError::Lower { inner, .. } if matches!(inner.innermost(), LowerError::PayloadSpreadTwice { .. })), "{err:?}");
}

#[test]
fn a_spread_survives_printing_back_to_lolo_and_lowering_again() {
    let source = src("(q : Type = ListReq)", "Asked { ...q, key : string! }", "ASK -> input { ...q, key : string!, pageBound : boolean? }");
    let printed = orbital_lolo::schema_to_lolo(&parse_and_lower_str(&source).expect("lower")).expect("print");
    assert!(printed.contains("...q"), "{printed}");
    let again = front(&printed);
    let emit = again.emits.as_ref().unwrap().iter().find(|e| e.event == "Asked").unwrap();
    assert_eq!(emit.payload_spread.as_deref(), Some("$q"));
    let ask = again.state_machine.events.as_ref().unwrap().iter().find(|e| e.key == "ASK").unwrap();
    assert_eq!(ask.payload_spread.as_deref(), Some("$q"));
    assert_eq!(names(ask.payload_schema.as_ref().unwrap()), ["key", "pageBound"]);
}

fn computed_emit_src(effect: &str) -> String {
    format!(
        r#"
type ListReq = {{ search : string?, limit : number? }}

orbital Relay {{
  entity RelayRun [runtime] {{
    id : string!
    req : ListReq
  }}

  trait Front -> RelayRun {{
    state idle {{
      INIT -> idle {effect}
    }}
    emits
      FWD {{ ...ListReq, key : string! }}
  }}

  page "/relay" -> Front
}}
"#
    )
}

#[test]
fn a_computed_emit_payload_is_left_to_the_typed_check() {
    parse_and_lower_str(&computed_emit_src(r#"(emit FWD (object/merge { search: "s" } { key: "k" }))"#))
        .expect("a computed payload is typed after resolution, not refused at lowering");
}

#[test]
fn control_an_emit_with_no_payload_still_misses_its_required_field() {
    let err = parse_and_lower_str(&computed_emit_src("(emit FWD)")).expect_err("no payload");
    assert!(matches!(&err, LoloError::Lower { inner, .. } if matches!(inner.innermost(), LowerError::EmitPayloadMissingRequired { .. })), "{err:?}");
}

#[test]
fn control_a_literal_emit_payload_still_misses_its_required_field() {
    let err = parse_and_lower_str(&computed_emit_src(r#"(emit FWD { search: "s" })"#)).expect_err("literal without key");
    assert!(matches!(&err, LoloError::Lower { inner, .. } if matches!(inner.innermost(), LowerError::EmitPayloadMissingRequired { .. })), "{err:?}");
}
