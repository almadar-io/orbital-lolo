//! Hints for small-model authoring habits imported from other languages:
//! C-style `//` comments (`.lolo` uses `;;`) and JS object-shorthand keys
//! (`{ @entity.x }` — `.lolo` objects are always explicit `key: value`).
//! Observed live in rabit L3 runs (docs/Almadar_Confusions.md: C-COMMENT-SLASH,
//! C-OBJ-KEY-SIGIL): the generic errors sent a 27B model thrashing for its
//! whole turn budget.

use orbital_lolo::error::ParseError;
use orbital_lolo::parse_str;

fn parse_err(src: &str) -> ParseError {
    match parse_str(src) {
        Ok(_) => panic!("expected a parse error"),
        Err(e) => e,
    }
}

#[test]
fn slash_comment_gets_semicolon_hint() {
    let src = "orbital O {\n  entity E [runtime] {\n    x : number = 0\n  }\n  trait T -> E [interaction] {\n    initial: idle\n    state idle {\n      // handled elsewhere\n      RUN -> idle\n        (set @entity.x 1)\n    }\n  }\n}";
    let err = parse_err(src);
    let (suggestion, fix) = err.hint();
    assert!(
        suggestion.as_deref().is_some_and(|s| s.contains(";;")),
        "expected `;;` comment hint, got {suggestion:?} for {err:?}"
    );
    assert!(fix.as_deref().is_some_and(|f| f.contains(";;")));
}

#[test]
fn sigil_object_key_gets_explicit_pair_hint() {
    let src = "orbital O {\n  entity E [runtime] {\n    x : number = 0\n  }\n  trait T -> E [interaction] {\n    initial: idle\n    state idle {\n      RUN -> idle\n        (emit DONE { @entity.x })\n    }\n  }\n}";
    let err = parse_err(src);
    let (suggestion, _fix) = err.hint();
    assert!(
        suggestion
            .as_deref()
            .is_some_and(|s| s.contains("key: value")),
        "expected explicit-pair hint, got {suggestion:?} for {err:?}"
    );
}

#[test]
fn trait_level_transition_gets_placement_hint() {
    // Observed live (C-TRAIT-LEVEL-TRANSITION): the model closes a state and
    // writes the next `EVENT -> target` arm at trait level.
    let src = "orbital O {\n  entity E [runtime] {\n    x : number = 0\n  }\n  trait T -> E [interaction] {\n    initial: idle\n    state idle {\n      RUN -> idle\n        (set @entity.x 1)\n    }\n\n    ADVANCE -> idle\n      (set @entity.x 2)\n  }\n}";
    let err = parse_err(src);
    let (suggestion, fix) = err.hint();
    assert!(
        suggestion.as_deref().is_some_and(|s| s.contains("OUTSIDE")),
        "expected placement hint, got {suggestion:?} for {err:?}"
    );
    assert!(fix.as_deref().is_some_and(|f| f.contains("state <name>")));
}

#[test]
fn early_trait_close_gets_brace_hint() {
    // Observed live (C-EARLY-TRAIT-CLOSE): an extra `}` closes the trait, so
    // the next `state` block lands at orbital level.
    let src = "orbital O {\n  entity E [runtime] {\n    x : number = 0\n  }\n  trait T -> E [interaction] {\n    initial: idle\n    state idle {\n      RUN -> running\n        (set @entity.x 1)\n    }\n  }\n\n    state running {\n      STOP -> idle\n        (set @entity.x 0)\n    }\n}";
    let err = parse_err(src);
    let (suggestion, fix) = err.hint();
    assert!(
        suggestion.as_deref().is_some_and(|s| s.contains("closed the trait early")),
        "expected early-close hint, got {suggestion:?} for {err:?}"
    );
    assert!(fix.as_deref().is_some_and(|f| f.contains("surplus `}`")));
}

#[test]
fn annotation_inside_payload_gets_placement_hint() {
    // Observed live (C-ANNOTATION-IN-PAYLOAD): `@description` written inside
    // the emits payload braces instead of after the closing `}`.
    let src = "orbital O {\n  entity E [runtime] {\n    x : number = 0\n  }\n  trait T -> E [interaction] {\n    initial: idle\n    state idle {\n      RUN -> idle\n        (set @entity.x 1)\n    }\n    emits {\n      DONE -> external {\n        @description \"finished\"\n        x : number\n      }\n    }\n  }\n}";
    let err = parse_err(src);
    let (suggestion, fix) = err.hint();
    assert!(
        suggestion.as_deref().is_some_and(|s| s.contains("AFTER the payload block")),
        "expected annotation-placement hint, got {suggestion:?} for {err:?}"
    );
    assert!(fix.as_deref().is_some_and(|f| f.contains("field : type")));
}

#[test]
fn bare_paren_group_gets_do_hint() {
    // Observed live (C-BARE-PAREN-GROUP): a bare `(` opened as a grouping
    // block for a multi-expression lambda body.
    let src = "orbital O {\n  entity E [runtime] {\n    x : number = 0\n  }\n  trait T -> E [interaction] {\n    initial: idle\n    state idle {\n      RUN -> idle\n        (set @entity.x (array/len (array/map @entity.x (fn c (\n          (if true 1 2)))))\n    }\n  }\n}";
    let err = parse_err(src);
    let (suggestion, fix) = err.hint();
    assert!(
        suggestion.as_deref().is_some_and(|s| s.contains("grouping block")),
        "expected bare-paren hint, got {suggestion:?} for {err:?}"
    );
    assert!(fix.as_deref().is_some_and(|f| f.contains("(do")));
}

#[test]
fn parenthesized_value_gets_operator_hint() {
    // Observed live (C-SIGIL-OPERATOR): JS-style parenthesized value.
    let src = "orbital O {\n  entity E [runtime] {\n    x : number = 0\n  }\n  trait T -> E [interaction] {\n    initial: idle\n    state idle {\n      RUN -> idle\n        (set @entity.x (@entity.x))\n    }\n  }\n}";
    let err = parse_err(src);
    let (suggestion, _fix) = err.hint();
    assert!(
        suggestion.as_deref().is_some_and(|s| s.contains("operator call")),
        "expected parenthesized-value hint, got {suggestion:?} for {err:?}"
    );
}

#[test]
fn brace_lambda_body_gets_do_hint() {
    // Observed live (C-BRACE-LAMBDA-BODY): JS arrow-function block body.
    let src = "orbital O {\n  entity E [runtime] {\n    x : number = 0\n  }\n  trait T -> E [interaction] {\n    initial: idle\n    state idle {\n      RUN -> idle\n        (set @entity.x (array/len (array/map @entity.x (fn b {\n          (set @entity.x 1)\n        }))))\n    }\n  }\n}";
    let err = parse_err(src);
    let (suggestion, fix) = err.hint();
    assert!(
        suggestion.as_deref().is_some_and(|s| s.contains("not a code block")),
        "expected brace-body hint, got {suggestion:?} for {err:?}"
    );
    assert!(fix.as_deref().is_some_and(|f| f.contains("(do")));
}

#[test]
fn semicolon_comments_still_parse() {
    let src = "orbital O {\n  entity E [runtime] {\n    x : number = 0\n  }\n  trait T -> E [interaction] {\n    initial: idle\n    state idle {\n      ;; handled elsewhere\n      RUN -> idle\n        (set @entity.x 1)\n    }\n  }\n}";
    assert!(parse_str(src).is_ok(), "`;;` comments must stay legal");
}
