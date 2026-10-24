//! Diagnostics for the classic "forgot `(do …)`" mistake: a `let`/`if`/
//! `when`/`fn`/`lambda` s-expression written with more body expressions than
//! its head accepts. Before this, the extras silently became extra array
//! elements and surfaced (if at all) as an opaque downstream type/paren
//! error. Now the parser catches it directly as `ParseError::TooManyArgs`,
//! with a span on the first extra expression and a hint pointing at `(do …)`.

use orbital_lolo::error::ParseError;
use orbital_lolo::parse_str;

fn program(effect: &str) -> String {
    format!(
        r#"orbital ProbeOrbital {{
  entity Probe [runtime] {{
    total : number = 0
  }}

  trait ProbeRun -> Probe {{
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
fn multi_body_let_is_too_many_args() {
    // Two writes stacked as extra `let` args instead of `(do …)`.
    let err = parse_err("(let ((x 1)) (set @entity.total 1) (set @entity.total 2))");
    match &err {
        ParseError::TooManyArgs {
            head,
            max,
            got,
            span,
        } => {
            assert_eq!(head, "let");
            assert_eq!(*max, 2);
            assert_eq!(*got, 3);
            // Span must point at the SECOND body expr (the first extra one),
            // not the first (legitimately allowed) body expr. Both are on
            // the same source line here (single-line effect), so the span
            // check that matters is the arg accounting above; assert the
            // span at least lands on the effect's line.
            assert_eq!(span.line, 10);
        }
        other => panic!("expected TooManyArgs, got {other:?}"),
    }
    let (suggestion, fix) = err.hint();
    assert!(suggestion.unwrap().contains("let"));
    assert!(fix.unwrap().contains("(do"));
}

#[test]
fn four_arg_if_is_too_many_args() {
    let err = parse_err(
        "(if @entity.total (set @entity.total 1) (set @entity.total 2) (set @entity.total 3))",
    );
    match &err {
        ParseError::TooManyArgs { head, max, got, .. } => {
            assert_eq!(head, "if");
            assert_eq!(*max, 3);
            assert_eq!(*got, 4);
        }
        other => panic!("expected TooManyArgs, got {other:?}"),
    }
    let (suggestion, fix) = err.hint();
    assert!(suggestion.unwrap().contains("if"));
    assert!(fix.unwrap().contains("(if cond then else)"));
}

#[test]
fn three_arg_when_is_too_many_args() {
    // `when` (guard sugar inside an s-expr, e.g. std cond-arms) takes cond +
    // body — a 3rd positional arg is one too many.
    let err = parse_err("(when @entity.total (set @entity.total 1) (set @entity.total 2))");
    match err {
        ParseError::TooManyArgs { head, max, got, .. } => {
            assert_eq!(head, "when");
            assert_eq!(max, 2);
            assert_eq!(got, 3);
        }
        other => panic!("expected TooManyArgs, got {other:?}"),
    }
}

#[test]
fn multi_body_fn_is_too_many_args() {
    let err = parse_err(
        "(set @entity.total (array/len (array/filter @entity.total (fn it (set @entity.total 1) (set @entity.total 2)))))",
    );
    match err {
        ParseError::TooManyArgs { head, max, .. } => {
            assert_eq!(head, "fn");
            assert_eq!(max, 2);
        }
        other => panic!("expected TooManyArgs, got {other:?}"),
    }
}

#[test]
fn multi_body_lambda_is_too_many_args() {
    let err = parse_err(
        "(set @entity.total (array/len (array/filter @entity.total (lambda it (set @entity.total 1) (set @entity.total 2)))))",
    );
    assert!(matches!(err, ParseError::TooManyArgs { .. }));
}

#[test]
fn do_wrapped_multi_effect_body_parses() {
    parse_ok("(do (set @entity.total 1) (set @entity.total 2) (set @entity.total 3))");
}

#[test]
fn flat_multi_param_fn_parses() {
    parse_ok("(set @entity.total (array/reduce @entity.total 0 (fn acc it (+ @acc @it))))");
}

#[test]
fn parenthesized_param_fn_parses() {
    // Third surface form: `(fn (a b) body)` — params reach the arg loop as a
    // single s-expression and are canonicalized by `normalize_fn_shapes`.
    parse_ok("(set @entity.total (array/reduce @entity.total 0 (fn (acc it) (+ @acc @it))))");
}

#[test]
fn parenthesized_param_fn_multi_body_is_too_many_args() {
    let err = parse_err(
        "(set @entity.total (array/len (array/filter @entity.total (fn (it) (set @entity.total 1) (set @entity.total 2)))))",
    );
    match err {
        ParseError::TooManyArgs { head, max, got, .. } => {
            assert_eq!(head, "fn");
            assert_eq!(max, 2);
            assert_eq!(got, 3);
        }
        other => panic!("expected TooManyArgs, got {other:?}"),
    }
}

#[test]
fn two_arg_if_must_still_parse() {
    // L2's job to flag a missing `else`, not L1's.
    parse_ok("(if @entity.total (set @entity.total 1))");
}

#[test]
fn cond_with_many_arms_parses() {
    // `cond` is untouched by this check — variable arity is legitimate.
    parse_ok("(set @entity.total (cond (eq @entity.total 1) 1 (eq @entity.total 2) 2 3))");
}
