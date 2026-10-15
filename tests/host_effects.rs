//! `random-bytes`, `credential-status` and `credential-forget` lower to the contract form and survive the formatter.

use orbital_core::schema::TraitReference;
use orbital_lolo::{format_program, parse_and_lower_str, parse_str};
use serde_json::{json, Value};

fn program(effect: &str) -> String {
    format!(
        r#"orbital DemoOrbital {{
  entity Demo [runtime] {{
    id : string!
  }}

  trait DemoFlow -> Demo [interaction, instance] {{
    config {{ token: secret = "TOKEN_NAME" }}
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

fn effects_of(src: &str) -> Vec<Value> {
    let schema = parse_and_lower_str(src).expect("fixture must lower");
    let TraitReference::Inline(td) = &schema.orbitals[0].traits[0] else {
        panic!("expected an inline trait");
    };
    let sm = serde_json::to_value(&td.state_machine).unwrap();
    sm["transitions"]
        .as_array()
        .expect("transitions")
        .iter()
        .find(|t| t["event"] == "GO")
        .and_then(|t| t["effects"].as_array().cloned())
        .expect("transition effects")
}

#[test]
fn random_bytes_lowers_to_the_contract_form() {
    let effects = effects_of(&program("(random-bytes 32 { emit: { success: DRAWN, failure: DRAW_FAILED } })"));
    assert_eq!(
        effects,
        vec![json!(["random-bytes", 32, { "emit": { "success": "DRAWN", "failure": "DRAW_FAILED" } }])]
    );
}

#[test]
fn the_credential_effects_lower_with_the_knob_binding() {
    for head in ["credential-status", "credential-forget"] {
        let effects = effects_of(&program(&format!("({head} @config.token {{ emit: {{ success: DONE }} }})")));
        assert_eq!(
            effects,
            vec![json!([head, "@config.token", { "emit": { "success": "DONE" } }])],
            "{head}"
        );
    }
}

#[test]
fn control_options_are_optional() {
    assert_eq!(effects_of(&program("(random-bytes 8)")), vec![json!(["random-bytes", 8])]);
}

#[test]
fn the_formatter_keeps_the_three_heads() {
    for call in [
        "(random-bytes 16 { emit: { success: DRAWN } })",
        "(credential-status @config.token { emit: { success: DONE } })",
        "(credential-forget @config.token)",
    ] {
        let src = program(call);
        let formatted = format_program(&parse_str(&src).expect("parse"));
        let head = call.split_whitespace().next().unwrap();
        assert!(formatted.contains(head), "{formatted}");
        assert_eq!(effects_of(&formatted), effects_of(&src), "{call}");
    }
}
