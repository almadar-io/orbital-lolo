//! `event` / event-address types are for config blocks only. An entity field that reaches one
//! through a record alias (`steps : [Step]` where `Step.event : event`) is refused like a direct one;
//! found 2026-10-13 when `std-avl-cosmic-zoom` lowered but could not round-trip.

use orbital_lolo::{parse_and_lower_str, LoloError};

fn lower_err(source: &str) -> String {
    match parse_and_lower_str(source) {
        Err(LoloError::Lower { inner, .. }) => inner.to_string(),
        Err(other) => panic!("expected a Lower error, got {other}"),
        Ok(_) => panic!("expected an error, lowering succeeded"),
    }
}

fn program(entity_field: &str, config: &str, read: &str) -> String {
    format!(
        r#"
type Step = {{
  trait : string!
  event : event!
}}

orbital Player {{
  entity Run [runtime] {{
    id : string!
    {entity_field}
  }}

  trait Runner -> Run [interaction] {{
    state idle {{
      INIT -> idle
        (set @entity.id {read})
    }}
    config {{ {config} }}
  }}
}}
"#
    )
}

#[test]
fn an_entity_field_reaching_event_through_a_record_is_refused() {
    let msg = lower_err(&program("steps : [Step] = []", "label : string = \"x\"", "@config.label"));
    assert!(msg.contains("ELOLO_CONFIG_TYPE_FORBIDDEN_IN_ENTITY"), "{msg}");
    assert!(msg.contains("steps"), "{msg}");
}

#[test]
fn edge_a_single_record_field_is_refused_too() {
    let msg = lower_err(&program("last : Step", "label : string = \"x\"", "@config.label"));
    assert!(msg.contains("ELOLO_CONFIG_TYPE_FORBIDDEN_IN_ENTITY"), "{msg}");
}

#[test]
fn control_the_same_record_in_a_config_knob_lowers() {
    let src = program("count : number = 0", "step : Step", "@config.step.trait");
    assert!(parse_and_lower_str(&src).is_ok(), "{:?}", parse_and_lower_str(&src).err());
}

#[test]
fn edge_sexpr_through_a_nested_record_is_refused() {
    let src = r#"
type Run = {
  args : [SExpr]!
}
type Step = {
  trait : string!
  run : Run!
}

orbital Player {
  entity Log [persistent] {
    id : string!
    steps : [Step] = []
  }

  trait Runner -> Log [interaction] {
    state idle {
      INIT -> idle
    }
  }
}
"#;
    let msg = lower_err(src);
    assert!(msg.contains("ELOLO_CONFIG_TYPE_FORBIDDEN_IN_ENTITY") && msg.contains("SExpr"), "{msg}");
}

fn sexpr_program(persistence: &str, field: &str) -> String {
    format!(
        r#"
type Run = {{
  args : [SExpr]!
}}

orbital Player {{
  entity Log [{persistence}] {{
    id : string!
    {field}
  }}

  trait Runner -> Log [interaction] {{
    state idle {{
      INIT -> idle
    }}
  }}
}}
"#
    )
}

fn any_error(source: &str) -> Option<String> {
    match parse_and_lower_str(source) {
        Ok(_) => None,
        Err(e) => Some(e.to_string()),
    }
}

#[test]
fn a_runtime_entity_may_hold_sexpr_through_a_record() {
    assert_eq!(any_error(&sexpr_program("runtime", "runs : [Run] = []")), None);
}

#[test]
fn a_runtime_entity_may_hold_sexpr_directly() {
    assert_eq!(any_error(&sexpr_program("runtime", "args : [SExpr] = []")), None);
}

#[test]
fn control_a_persisted_entity_still_refuses_sexpr_through_a_record() {
    let msg = any_error(&sexpr_program("persistent", "runs : [Run] = []")).expect("refused");
    assert!(msg.contains("ELOLO_CONFIG_TYPE_FORBIDDEN_IN_ENTITY") && msg.contains("SExpr"), "{msg}");
}

#[test]
fn control_a_persisted_entity_still_refuses_sexpr_directly() {
    let msg = any_error(&sexpr_program("persistent", "args : [SExpr] = []")).expect("refused");
    assert!(msg.contains("ELOLO_CONFIG_TYPE_FORBIDDEN_IN_ENTITY") && msg.contains("SExpr"), "{msg}");
}

#[test]
fn control_a_runtime_entity_still_refuses_event_through_a_record() {
    let msg = lower_err(&program("steps : [Step] = []", "label : string = \"x\"", "@config.label"));
    assert!(msg.contains("ELOLO_CONFIG_TYPE_FORBIDDEN_IN_ENTITY"), "{msg}");
}
