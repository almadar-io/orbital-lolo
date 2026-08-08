//! Diagnostics for malformed `(set …)` effects: too many positional args
//! (and the legitimate 3-arg `{ emit: … }` options-object exception), and a
//! nested `(set …)` used as another `set`'s value (illegal — `set` is a
//! statement, not an expression).

use orbital_lolo::error::ParseError;
use orbital_lolo::parse_str;

fn program(effect: &str) -> String {
    format!(
        r#"orbital ProbeOrbital {{
  entity Probe [runtime] {{
    a : number = 0
    b : number = 0
  }}

  trait ProbeRun -> Probe [lifecycle, instance] {{
    initial: idle
    state idle {{
      RUN -> idle
        {effect}
    }}
  }}
}}
"#
    )
}

fn parse_err(effect: &str) -> ParseError {
    match parse_str(&program(effect)) {
        Ok(_) => panic!("expected a parse error for effect: {effect}"),
        Err(e) => e,
    }
}

fn parse_ok(effect: &str) {
    if let Err(e) = parse_str(&program(effect)) {
        panic!("expected effect to parse: {effect}\n  got: {e:?}");
    }
}

#[test]
fn four_positional_args_is_set_extra_args() {
    let err = parse_err("(set @entity.a 1 2 3)");
    match err {
        ParseError::SetExtraArgs { got, .. } => assert_eq!(got, 4),
        other => panic!("expected SetExtraArgs, got {other:?}"),
    }
    let (suggestion, fix) = err.hint();
    assert!(suggestion.is_some());
    assert!(fix.unwrap().contains("(do"));
}

#[test]
fn three_args_without_options_object_is_set_extra_args() {
    // A 3rd arg is only legal when it's the `{ emit: … }` options object.
    let err = parse_err("(set @entity.a 1 2)");
    assert!(matches!(err, ParseError::SetExtraArgs { got: 3, .. }));
}

#[test]
fn nested_set_as_value_is_rejected() {
    let err = parse_err("(set @entity.a (set @entity.b 1))");
    match err {
        ParseError::SetNestedValue { .. } => {}
        other => panic!("expected SetNestedValue, got {other:?}"),
    }
    let (suggestion, fix) = err.hint();
    assert!(suggestion.is_some());
    assert!(fix.unwrap().contains("(do"));
}

#[test]
fn binary_set_parses() {
    parse_ok("(set @entity.a 1)");
}

#[test]
fn set_with_emit_options_object_parses() {
    parse_ok(r#"(set @entity.a 1 { emit: { success: "SAVED" } })"#);
}

#[test]
fn sequenced_sets_via_do_parses() {
    parse_ok("(do (set @entity.a 1) (set @entity.b 2))");
}
