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
        suggestion
            .as_deref()
            .is_some_and(|s| s.contains("closed the trait early")),
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
        suggestion
            .as_deref()
            .is_some_and(|s| s.contains("AFTER the payload block")),
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
        suggestion
            .as_deref()
            .is_some_and(|s| s.contains("grouping block")),
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
        suggestion
            .as_deref()
            .is_some_and(|s| s.contains("operator call")),
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
        suggestion
            .as_deref()
            .is_some_and(|s| s.contains("not a code block")),
        "expected brace-body hint, got {suggestion:?} for {err:?}"
    );
    assert!(fix.as_deref().is_some_and(|f| f.contains("(do")));
}

#[test]
fn unquoted_multiword_value_gets_quote_hint() {
    // Observed live (C-UNQUOTED-STRING): bare multi-word text value + commas.
    let src = "orbital O {\n  entity E [runtime] {\n    x : number = 0\n  }\n  trait T -> E [interaction] {\n    initial: idle\n    state idle {\n      RUN -> idle\n        (render-ui main { type: typography, content: Next Review })\n    }\n  }\n}";
    let err = parse_err(src);
    let (suggestion, fix) = err.hint();
    assert!(
        suggestion
            .as_deref()
            .is_some_and(|s| s.contains("must be quoted")),
        "expected quoting hint, got {suggestion:?} for {err:?}"
    );
    assert!(fix
        .as_deref()
        .is_some_and(|f| f.contains("\\\"") || f.contains('"')));
}

#[test]
fn emits_scope_form_in_listens_gets_direction_hint() {
    // Observed live (C-LISTENS-ARROW): emits' `EVENT -> internal { … }` form
    // copied into a listens block. `-> external` is the external-input form.
    let src = "orbital O {\n  entity E [runtime] {\n    x : number = 0\n  }\n  trait T -> E [interaction] {\n    initial: idle\n    state idle {\n      RUN -> idle\n        (set @entity.x 1)\n    }\n    listens {\n      PING -> internal {\n        x : number\n      }\n    }\n  }\n}";
    let err = parse_err(src);
    let (suggestion, fix) = err.hint();
    assert!(
        suggestion
            .as_deref()
            .is_some_and(|s| s.contains("belong to `emits`")),
        "expected listens-form hint, got {suggestion:?} for {err:?}"
    );
    assert!(fix.as_deref().is_some_and(|f| f.contains("-> external")));
}

#[test]
fn external_input_form_in_listens_parses() {
    let src = "orbital O {\n  entity E [runtime] {\n    x : number = 0\n  }\n  trait T -> E [interaction] {\n    initial: idle\n    state idle {\n      PING -> idle\n        (set @entity.x ?x)\n    }\n    listens {\n      PING -> external {\n        x : number\n      }\n    }\n  }\n}";
    assert!(parse_str(src).is_ok());
}

#[test]
fn single_quoted_string_gets_double_quote_hint() {
    // Observed live (C-SINGLE-QUOTE): single-quoted string burned 45 turns.
    let src = "orbital O {\n  entity E [runtime] {\n    x : string = 'hello'\n  }\n}";
    let err = parse_err(src);
    let (suggestion, _fix) = err.hint();
    assert!(
        suggestion
            .as_deref()
            .is_some_and(|s| s.contains("double-quoted")),
        "expected quote hint, got {suggestion:?} for {err:?}"
    );
}

#[test]
fn unquoted_multiword_value_before_newline_gets_quote_hint() {
    // Same class, newline shape: `label: Next Review` then newline.
    let src = "orbital O {\n  entity E [runtime] {\n    x : number = 0\n  }\n  trait T -> E [interaction] {\n    initial: idle\n    state idle {\n      RUN -> idle\n        (render-ui main { type: typography\n          content: Next Review\n          variant: caption })\n    }\n  }\n}";
    let err = parse_err(src);
    let (suggestion, _fix) = err.hint();
    assert!(
        suggestion
            .as_deref()
            .is_some_and(|s| s.contains("must be quoted")),
        "expected quoting hint, got {suggestion:?} for {err:?}"
    );
}

#[test]
fn bare_uses_gets_from_hint() {
    // Observed live (W-USES-BARE): `uses std-event-log` with no from clause.
    let src =
        "orbital O {\n  uses std-event-log\n  entity E [runtime] {\n    x : number = 0\n  }\n}";
    let err = parse_err(src);
    let (suggestion, fix) = err.hint();
    assert!(
        suggestion
            .as_deref()
            .is_some_and(|s| s.contains("from \"std/behaviors/")),
        "expected uses-from hint, got {suggestion:?} for {err:?}"
    );
    assert!(fix
        .as_deref()
        .is_some_and(|f| f.contains("flat canonical form")));
}

#[test]
fn early_orbital_close_gets_brace_hint() {
    // Observed live (Run 17): an extra `}` closes the ORBITAL, so the next
    // `page` declaration lands at file level.
    let src = "orbital O {\n  entity E [runtime] {\n    x : number = 0\n  }\n  trait T -> E [interaction] {\n    initial: idle\n    state idle {\n      RUN -> idle\n        (set @entity.x 1)\n    }\n  }\n}\n\n  page \"/\" as P -> T\n}";
    let err = parse_err(src);
    let (suggestion, fix) = err.hint();
    assert!(
        suggestion
            .as_deref()
            .is_some_and(|s| s.contains("closed the orbital early")),
        "expected orbital-early-close hint, got {suggestion:?} for {err:?}"
    );
    assert!(fix.as_deref().is_some_and(|f| f.contains("surplus `}`")));
}

#[test]
fn rparen_inside_object_gets_missing_brace_hint() {
    // Observed live (Run 17): closer pile-up `}))` where one `{` never got
    // its `}` — the parser is mid-object when `)` arrives.
    let src = "orbital O {\n  entity E [runtime] {\n    x : number = 0\n  }\n  trait T -> E [interaction] {\n    initial: idle\n    state idle {\n      RUN -> idle\n        (render-ui main { type: stack\n          children: [{ type: typography\n            content: \"hi\"\n          ]\n        })\n    }\n  }\n}";
    let err = parse_err(src);
    let (suggestion, _fix) = err.hint();
    assert!(
        suggestion
            .as_deref()
            .is_some_and(|s| s.contains("missing its `}`") || s.contains("still open")),
        "expected missing-brace hint, got {suggestion:?} for {err:?}"
    );
}

#[test]
fn semicolon_comments_still_parse() {
    let src = "orbital O {\n  entity E [runtime] {\n    x : number = 0\n  }\n  trait T -> E [interaction] {\n    initial: idle\n    state idle {\n      ;; handled elsewhere\n      RUN -> idle\n        (set @entity.x 1)\n    }\n  }\n}";
    assert!(parse_str(src).is_ok(), "`;;` comments must stay legal");
}
