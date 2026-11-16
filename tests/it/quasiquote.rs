//! `(quasiquote x)`: the parser encodes the template like `quote`, except each
//! `(unquote e)` becomes a numbered marker and `e` stays a live trailing
//! argument, so binding interpolation and renames still reach the holes. A
//! stray `(unquote e)` is a parse error; the formatter prints the template back.

use orbital_core_base::schema::quote::{decode_quote_body, quasiquote_of};
use orbital_core_base::schema::SExpression;
use orbital_lolo::{format_program, parse_and_lower_str, parse_str};

fn program(effect: &str) -> String {
    format!(
        r#"orbital Builder {{
  entity Draft [runtime] {{
    id : string!
    widget : string = ""
    program : string = ""
  }}

  trait Build -> Draft {{
    initial: idle

    state idle {{
      INIT -> idle
        (render-ui main {{ type: button, label: "Build", action: BUILD }})
      PICK -> idle
        (set @entity.widget @payload.widget)
      BUILD -> idle
        {effect}
    }}
  }}

  page "/build" -> Build
}}
"#
    )
}

const BUILD: &str = r#"(set @entity.program (quasiquote (orbital Page { pages: { "/dash": (unquote @entity.widget) } })))"#;

fn quasiquote_calls(v: &SExpression, out: &mut Vec<SExpression>) {
    if quasiquote_of(v).is_some() {
        out.push(v.clone());
        return;
    }
    match v {
        SExpression::Array(items) => items.iter().for_each(|i| quasiquote_calls(i, out)),
        SExpression::Object(map) => map.values().for_each(|i| quasiquote_calls(i, out)),
        _ => {}
    }
}

fn lowered_calls(src: &str) -> Vec<SExpression> {
    let schema = parse_and_lower_str(src).unwrap_or_else(|e| panic!("lowering failed: {e}"));
    let json: SExpression = serde_json::from_value(serde_json::to_value(&schema).unwrap()).unwrap();
    let mut out = Vec::new();
    quasiquote_calls(&json, &mut out);
    out
}

#[test]
fn the_template_is_encoded_and_the_hole_stays_live() {
    let calls = lowered_calls(&program(BUILD));
    assert_eq!(calls.len(), 1, "{calls:?}");
    let (body, holes) = quasiquote_of(&calls[0]).unwrap();
    assert!(!body.contains('@'), "{body}");
    assert_eq!(holes, &[SExpression::String("@entity.widget".into())]);
    let template: SExpression = serde_json::from_str(
        r#"["orbital", "Page", {"pages": {"/dash": ["unquote", 0]}}]"#,
    )
    .unwrap();
    assert_eq!(decode_quote_body(body), Some(template));
}

#[test]
fn control_a_plain_quote_keeps_no_live_hole() {
    let src = program(BUILD).replace("quasiquote", "quote").replace("(unquote @entity.widget)", "\"x\"");
    assert!(lowered_calls(&src).is_empty());
}

#[test]
fn an_unquote_outside_a_quasiquote_is_a_parse_error() {
    let src = program("(set @entity.program (unquote @entity.widget))");
    let err = parse_str(&src).err().map(|e| e.to_string()).unwrap_or_default();
    assert!(err.contains("ELOLO_UNQUOTE_OUTSIDE_QUASIQUOTE"), "{err}");
}

#[test]
fn a_quoted_string_that_reads_as_a_binding_is_a_parse_error() {
    for effect in [
        r#"(set @entity.widget "@payload.widget")"#,
        r#"(render-ui main { type: button, label: "@entity.widget", action: BUILD })"#,
        r#"(set @entity.program (quasiquote (a (unquote "@entity.widget"))))"#,
    ] {
        let err = parse_str(&program(effect)).err().map(|e| e.to_string()).unwrap_or_default();
        assert!(err.contains("ELOLO_QUOTED_BINDING"), "{effect}: {err}");
    }
}

#[test]
fn control_a_bare_binding_quoted_text_and_plain_text_parse() {
    for effect in [
        r#"(set @entity.widget @payload.widget)"#,
        r#"(set @entity.widget (quote "@payload.widget"))"#,
        r#"(set @entity.program (quasiquote (a "@entity.widget" (unquote @entity.widget))))"#,
        r#"(set @entity.widget "user@example.com")"#,
        r#"(set @entity.widget "@payload.rows[2].title")"#,
    ] {
        assert!(parse_str(&program(effect)).is_ok(), "{effect}: {:?}", parse_str(&program(effect)).err().map(|e| e.to_string()));
    }
}

#[test]
fn a_nested_quasiquote_is_a_parse_error() {
    let src = program("(set @entity.program (quasiquote (a (quasiquote (b)))))");
    let err = parse_str(&src).err().map(|e| e.to_string()).unwrap_or_default();
    assert!(err.contains("ELOLO_QUASIQUOTE_INVALID"), "{err}");
}

#[test]
fn arabic_and_slovenian_heads_canonicalize() {
    for lang in orbital_core_base::i18n::lang::all() {
        for head in ["quasiquote", "unquote"] {
            let native = lang.def.effects.get(head).expect("head in every effects table");
            assert_eq!(lang.canon_head(native), Some(head), "{} {head}", lang.code);
        }
    }
}

#[test]
fn formatter_round_trips_quasiquote() {
    let src = program(BUILD);
    let formatted = format_program(&parse_str(&src).unwrap());
    assert!(formatted.contains("(quasiquote "), "{formatted}");
    assert!(formatted.contains("(unquote @entity.widget)"), "{formatted}");
    assert_eq!(lowered_calls(&src), lowered_calls(&formatted));
}
