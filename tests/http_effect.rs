//! The `http` effect surface and the `app` header `egress` key.

use orbital_core::schema::TraitReference;
use orbital_lolo::{format_program, parse_and_lower_str, parse_str, schema_to_lolo, unlower};
use serde_json::{json, Value};

fn program(egress: &str, effect: &str) -> String {
    format!(
        r#"app HttpDemo "1.0.0" {egress}

orbital DemoOrbital {{
  entity Demo [runtime] {{
    id : string!
  }}

  trait DemoFlow -> Demo [interaction, instance] {{
    initial: idle
    state idle {{
      INIT -> idle
        (render-ui main {{ type: "box" }})
      GO -> idle
        {effect}
    }}
  }}
}}"#
    )
}

fn effects_of(src: &str, event: &str) -> Vec<Value> {
    let schema = parse_and_lower_str(src).expect("fixture must lower");
    let TraitReference::Inline(td) = &schema.orbitals[0].traits[0] else {
        panic!("expected an inline trait");
    };
    let sm = serde_json::to_value(td.state_machine.as_ref().expect("state machine")).unwrap();
    let transitions = sm["transitions"].as_array().expect("transitions").clone();
    transitions
        .into_iter()
        .find(|t| t["event"] == event)
        .and_then(|t| t["effects"].as_array().cloned())
        .expect("transition effects")
}

const CALL: &str = r#"(http GET "https://x.test" "/a" { query: { q: "v" } } { emit: { success: S, failure: F } })"#;

#[test]
fn http_lowers_to_the_contract_form() {
    let effects = effects_of(&program("[ egress: [\"https://x.test\"] ]", CALL), "GO");
    assert_eq!(
        effects,
        vec![json!([
            "http",
            "GET",
            "https://x.test",
            "/a",
            { "query": { "q": "v" } },
            { "emit": { "success": "S", "failure": "F" } }
        ])]
    );
}

#[test]
fn every_method_word_is_a_string_literal() {
    for method in ["GET", "POST", "PUT", "PATCH", "DELETE"] {
        let call = format!(r#"(http {method} "https://x.test" "/a")"#);
        let effects = effects_of(&program("[ egress: [\"https://x.test\"] ]", &call), "GO");
        assert_eq!(effects[0][1], json!(method), "method {method}");
    }
}

#[test]
fn egress_lowers_to_the_schema_and_absent_stays_none() {
    let with = parse_and_lower_str(&program(
        "[ egress: [\"https://x.test\", \"https://y.test:8443\"] ]",
        CALL,
    ))
    .unwrap();
    assert_eq!(
        with.egress,
        Some(vec!["https://x.test".to_string(), "https://y.test:8443".to_string()])
    );
    let without = parse_and_lower_str(&program("", CALL)).unwrap();
    assert_eq!(without.egress, None);
}

#[test]
fn egress_and_http_survive_unlower_and_format() {
    let src = program("[ egress: [\"https://x.test\"] ]", CALL);
    let schema = parse_and_lower_str(&src).unwrap();
    let ast = unlower(&schema).expect("unlower");
    assert_eq!(ast.app.egress, vec!["https://x.test".to_string()]);
    let formatted = format_program(&parse_str(&src).expect("parse"));
    assert!(formatted.contains("egress: [\"https://x.test\"]"), "{formatted}");
    assert!(formatted.contains("(http GET"), "{formatted}");
    let unlowered = schema_to_lolo(&schema).expect("schema_to_lolo");
    assert!(unlowered.contains("egress: [\"https://x.test\"]"), "{unlowered}");
    assert!(unlowered.contains("(http GET"), "{unlowered}");
    let again = parse_and_lower_str(&formatted).unwrap();
    assert_eq!(again.egress, schema.egress);
}

#[test]
fn an_unknown_app_key_still_errors() {
    let err = parse_and_lower_str(&program("[ egres: [\"https://x.test\"] ]", CALL))
        .expect_err("unknown key");
    assert!(err.to_string().contains("ORB_L_OPTION_UNKNOWN"), "{err}");
}
