//! `UnclosedDelimiter` diagnostics: a source truncated mid-`(`, mid-`{`, or
//! mid-`[` must report the OPENING delimiter's line:col (not a generic
//! "unexpected end of input"), and nested unclosed delimiters report the
//! INNERMOST one (the deepest parser frame fails first). A stray extra `)`
//! is a different failure mode (not EOF) and gets its own "stray" hint.

use orbital_lolo::error::ParseError;
use orbital_lolo::parse_str;

fn parse_err(src: &str) -> ParseError {
    match parse_str(src) {
        Ok(_) => panic!("expected a parse error for:\n{src}"),
        Err(e) => e,
    }
}

#[test]
fn truncated_mid_paren_reports_opening_paren() {
    let src = "orbital O {\n  entity E [runtime] {\n    x : number = 0\n  }\n  trait T -> E [interaction] {\n    state s {\n      EVT -> s\n        (set @entity.x 1";
    let err = parse_err(src);
    match err {
        ParseError::UnclosedDelimiter {
            delimiter,
            closer,
            open_span,
            ..
        } => {
            assert_eq!(delimiter, '(');
            assert_eq!(closer, ')');
            assert_eq!(open_span.line, 8, "must point at the `(` that opened `(set ...`");
        }
        other => panic!("expected UnclosedDelimiter('(', ')'), got {other:?}"),
    }
}

#[test]
fn missing_paren_mid_file_reports_opening_paren() {
    // Not truncated: the state block's `}` closes over the unterminated
    // s-expression — the error must point at the CAUSE (the open `(`), not
    // report an "unexpected RBrace" consequence.
    let src = "orbital O {\n  entity E [runtime] {\n    x : number = 0\n  }\n  trait T -> E [interaction] {\n    state s {\n      EVT -> s\n        (set @entity.x (+ @entity.x 1)\n    }\n  }\n}";
    let err = parse_err(src);
    match err {
        ParseError::UnclosedDelimiter {
            delimiter,
            closer,
            open_span,
            found,
        } => {
            assert_eq!(delimiter, '(');
            assert_eq!(closer, ')');
            assert_eq!(found, "`}`");
            assert_eq!(open_span.line, 8, "must point at the `(` that opened `(set ...`");
        }
        other => panic!("expected UnclosedDelimiter('(', ')'), got {other:?}"),
    }
}

#[test]
fn truncated_mid_brace_reports_opening_brace() {
    let src = "orbital O {\n  entity E [runtime] {\n    x : number = 0\n  }\n  trait T -> E [interaction] {\n    state s {\n      EVT -> s\n        (render-ui main { type: stack";
    let err = parse_err(src);
    match err {
        ParseError::UnclosedDelimiter {
            delimiter, closer, ..
        } => {
            assert_eq!(delimiter, '{');
            assert_eq!(closer, '}');
        }
        other => panic!("expected UnclosedDelimiter('{{', '}}'), got {other:?}"),
    }
}

#[test]
fn truncated_mid_bracket_reports_opening_bracket() {
    let src = "orbital O {\n  entity E [runtime] {\n    items : [any] = []\n  }\n  trait T -> E [interaction] {\n    state s {\n      EVT -> s\n        (set @entity.items [1 2 3";
    let err = parse_err(src);
    match err {
        ParseError::UnclosedDelimiter {
            delimiter, closer, ..
        } => {
            assert_eq!(delimiter, '[');
            assert_eq!(closer, ']');
        }
        other => panic!("expected UnclosedDelimiter('[', ']'), got {other:?}"),
    }
}

#[test]
fn nested_unclosed_reports_innermost_opener() {
    // The outer `(set …` DOES have a matching close eventually in a
    // well-formed program, but the inner `(add 1 2` never closes — the
    // reported span must be the inner `(`, not the outer one.
    let src = "orbital O {\n  entity E [runtime] {\n    x : number = 0\n  }\n  trait T -> E [interaction] {\n    state s {\n      EVT -> s\n        (set @entity.x (add 1 2";
    let err = parse_err(src);
    match err {
        ParseError::UnclosedDelimiter {
            delimiter,
            closer,
            open_span,
            ..
        } => {
            assert_eq!(delimiter, '(');
            assert_eq!(closer, ')');
            // Line 8 is `        (set @entity.x (add 1 2` — both parens open
            // on the same line, so distinguish by column: the inner `(add`
            // opens strictly after the outer `(set`.
            assert_eq!(open_span.line, 8);
            let outer_open_col = src
                .lines()
                .nth(7)
                .unwrap()
                .find("(set")
                .unwrap() as u32
                + 1;
            assert!(
                open_span.col > outer_open_col,
                "expected the INNER `(add` opener (col > {outer_open_col}), got col {}",
                open_span.col
            );
        }
        other => panic!("expected UnclosedDelimiter('(', ')'), got {other:?}"),
    }
}

#[test]
fn stray_closing_paren_in_trait_body_hints_stray() {
    // One extra `)` after a fully-balanced effect — not an EOF case, a
    // different (already-existing) UnexpectedToken path, but the RParen
    // shape should route to the "stray `)`" hint added alongside the new
    // EOF diagnostics.
    let src = "orbital O {\n  entity E [runtime] {\n    x : number = 0\n  }\n  trait T -> E [interaction] {\n    state s {\n      EVT -> s\n        (set @entity.x 1))\n    }\n  }\n}\n";
    let err = parse_err(src);
    let (suggestion, _fix) = err.hint();
    let suggestion = suggestion.unwrap_or_default();
    assert!(
        suggestion.contains("stray"),
        "expected a 'stray' hint, got: {suggestion:?} (error: {err:?})"
    );
}
