//! `(quote x)` (G-CROSS-041): the parser encodes the quoted expression into
//! one canonical JSON string (`orbital_core::schema::quote`), so no walker,
//! inliner or binding interpolator ever sees inside it, and the formatter
//! prints it back as `(quote x)`.

use orbital_core_base::schema::quote::{decode_quote_body, encode_quote_body, quote_body_of};
use orbital_core_base::schema::SExpression;
use orbital_lolo::{format_program, parse_and_lower_str, parse_str};
use serde_json::Value;

fn program(head: &str, transition: &str) -> String {
    format!(
        r#"orbital QuoteOrbital {{
  entity Order [runtime] {{
    id : string!
    status : string = "pending"
  }}

  trait Explain -> Order {{
    initial: idle

    state idle {{
      INIT -> idle
        (render-ui main {{ type: avl-transition-explainer, orbitalName: "Orders", traitName: "Checkout", transition: {transition} }})
    }}
  }}

  page "/explain" -> Explain
}}
"#
    )
    .replace("QUOTE", head)
}

const GUARD_QUOTED: &str = r#"{ from: "pending", to: "paid", event: "PAY", guard: (QUOTE (> @payload.amount 0)), effects: [{ type: "set", args: (QUOTE [@entity.status "paid"]) }], index: 0 }"#;
const WHOLE_QUOTED: &str = r#"(QUOTE { from: "pending", to: "paid", event: "PAY", guard: (> @payload.amount 0), effects: [{ type: "set", args: [@entity.status "paid"] }, { type: "persist", args: [update Order @entity] }], index: 0 })"#;

fn quote_bodies(v: &Value, out: &mut Vec<String>) {
    match v {
        Value::Array(items) => {
            if items.len() == 2 && items[0] == Value::String("quote".into()) {
                if let Value::String(body) = &items[1] {
                    out.push(body.clone());
                    return;
                }
            }
            for i in items {
                quote_bodies(i, out);
            }
        }
        Value::Object(map) => map.values().for_each(|i| quote_bodies(i, out)),
        _ => {}
    }
}

fn lowered(src: &str) -> Value {
    let schema = parse_and_lower_str(src).unwrap_or_else(|e| panic!("lowering failed: {e}"));
    serde_json::to_value(&schema).unwrap()
}

fn sexpr(json: &str) -> SExpression {
    serde_json::from_str(json).unwrap()
}

#[test]
fn quoted_fields_lower_to_encoded_bodies() {
    let mut bodies = Vec::new();
    quote_bodies(&lowered(&program("quote", GUARD_QUOTED)), &mut bodies);
    bodies.sort();
    let mut want = vec![
        encode_quote_body(&sexpr(r#"[">", "@payload.amount", 0]"#)),
        encode_quote_body(&sexpr(r#"["@entity.status", "paid"]"#)),
    ];
    want.sort();
    assert_eq!(bodies, want);
    assert!(bodies.iter().all(|b| !b.contains('@')), "{bodies:?}");
}

#[test]
fn a_whole_quoted_object_decodes_verbatim() {
    let mut bodies = Vec::new();
    quote_bodies(&lowered(&program("quote", WHOLE_QUOTED)), &mut bodies);
    assert_eq!(bodies.len(), 1, "{bodies:?}");
    assert_eq!(
        decode_quote_body(&bodies[0]).unwrap(),
        sexpr(
            r#"{
            "from": "pending", "to": "paid", "event": "PAY",
            "guard": [">", "@payload.amount", 0],
            "effects": [
                { "type": "set", "args": ["@entity.status", "paid"] },
                { "type": "persist", "args": ["update", "Order", "@entity"] }
            ],
            "index": 0
        }"#
        )
    );
}

#[test]
fn control_unquoted_guard_stays_a_call() {
    let src = program("quote", GUARD_QUOTED).replace("(quote (> @payload.amount 0))", "(> @payload.amount 0)");
    let out = lowered(&src);
    let mut bodies = Vec::new();
    quote_bodies(&out, &mut bodies);
    assert_eq!(bodies.len(), 1);
    assert!(out.to_string().contains("\"@payload.amount\""));
    let quoted = lowered(&program("quote", GUARD_QUOTED)).to_string();
    assert!(!quoted.contains("@payload.amount"), "{quoted}");
}

#[test]
fn arabic_and_slovenian_heads_canonicalize_to_quote() {
    for lang in orbital_core_base::i18n::lang::all() {
        let native = lang.def.effects.get("quote").expect("quote in every effects table");
        assert_eq!(lang.canon_head(native), Some("quote"), "{}", lang.code);
    }
}

#[test]
fn quote_takes_exactly_one_argument() {
    for bad in ["(quote)", "(quote 1 2)"] {
        let src = program("quote", &format!(r#"{{ from: "a", to: "b", event: "E", guard: {bad}, effects: [], index: 0 }}"#));
        let err = parse_str(&src).err().map(|e| e.to_string()).unwrap_or_default();
        assert!(err.contains("quote"), "{bad}: {err}");
    }
}

#[test]
fn formatter_round_trips_quote() {
    let src = program("quote", WHOLE_QUOTED);
    let formatted = format_program(&parse_str(&src).unwrap());
    assert!(formatted.contains("(quote {"), "{formatted}");
    let mut a = Vec::new();
    let mut b = Vec::new();
    quote_bodies(&lowered(&src), &mut a);
    quote_bodies(&lowered(&formatted), &mut b);
    assert_eq!(a, b);
}

#[test]
fn quote_body_of_reads_the_lowered_form() {
    let mut bodies = Vec::new();
    quote_bodies(&lowered(&program("quote", GUARD_QUOTED)), &mut bodies);
    let call = SExpression::Array(vec![SExpression::String("quote".into()), SExpression::String((bodies[0].clone()).into())].into());
    assert_eq!(quote_body_of(&call), Some(bodies[0].as_str()));
}
