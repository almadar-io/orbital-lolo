//! The `app` header `egress` key and the browser host effects that read it. The `http` effect is retired (EXEC-HTTP-1): L1
//! gives `(http …)` no form of its own, so it lowers as a plain call and `orb validate` refuses the unknown operator.

use orbital_core_base::schema::TraitReference;
use orbital_lolo::{format_program, parse_and_lower_str, parse_str, schema_to_lolo, unlower};
use serde_json::{json, Value};

fn program(egress: &str, effect: &str) -> String {
    format!(
        r#"app HttpDemo {egress}

orbital DemoOrbital {{
  entity Demo [runtime] {{
    id : string!
  }}

  trait DemoFlow -> Demo {{
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
    let sm = serde_json::to_value(&td.state_machine).unwrap();
    let transitions = sm["transitions"].as_array().expect("transitions").clone();
    transitions
        .into_iter()
        .find(|t| t["event"] == event)
        .and_then(|t| t["effects"].as_array().cloned())
        .expect("transition effects")
}

const CALL: &str = r#"(browser/beacon "https://x.test/e" { text: "v" } { emit: { success: S, failure: F } })"#;
const RETIRED_HTTP_CALL: &str = r#"(http GET "https://x.test" "/a" { query: { q: "v" } } { emit: { success: S, failure: F } })"#;

#[test]
fn a_retired_http_call_lowers_as_a_plain_call_with_no_form_of_its_own() {
    let effects = effects_of(&program("[ version: \"1.0.0\", egress: [\"https://x.test\"] ]", RETIRED_HTTP_CALL), "GO");
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
fn control_a_known_host_effect_lowers_to_its_contract_form() {
    let effects = effects_of(&program("[ version: \"1.0.0\", egress: [\"https://x.test\"] ]", CALL), "GO");
    assert_eq!(
        effects,
        vec![json!(["browser/beacon", "https://x.test/e", { "text": "v" }, { "emit": { "success": "S", "failure": "F" } }])]
    );
}

#[test]
fn egress_lowers_to_the_schema_and_absent_stays_none() {
    let with = parse_and_lower_str(&program(
        "[ version: \"1.0.0\", egress: [\"https://x.test\", \"https://y.test:8443\"] ]",
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
fn egress_and_its_host_effect_survive_unlower_and_format() {
    let src = program("[ version: \"1.0.0\", egress: [\"https://x.test\"] ]", CALL);
    let schema = parse_and_lower_str(&src).unwrap();
    let ast = unlower(&schema).expect("unlower");
    assert_eq!(ast.app.egress, vec!["https://x.test".to_string()]);
    let formatted = format_program(&parse_str(&src).expect("parse"));
    assert!(formatted.contains("egress: [\"https://x.test\"]"), "{formatted}");
    assert!(formatted.contains("(browser/beacon"), "{formatted}");
    let unlowered = schema_to_lolo(&schema).expect("schema_to_lolo");
    assert!(unlowered.contains("egress: [\"https://x.test\"]"), "{unlowered}");
    assert!(unlowered.contains("(browser/beacon"), "{unlowered}");
    let again = parse_and_lower_str(&formatted).unwrap();
    assert_eq!(again.egress, schema.egress);
}

#[test]
fn an_unknown_app_key_still_errors() {
    let err = parse_and_lower_str(&program("[ egres: [\"https://x.test\"] ]", CALL))
        .expect_err("unknown key");
    assert!(err.to_string().contains("ORB_L_OPTION_UNKNOWN"), "{err}");
}

#[test]
fn page_facts_and_beacon_lower_to_the_contract_forms() {
    let facts = effects_of(
        &program("[ version: \"1.0.0\", egress: [\"https://x.test\"] ]", r#"(browser/page-facts [referrer, screenWidth] { emit: { success: S, failure: F } })"#),
        "GO",
    );
    assert_eq!(facts, vec![json!(["browser/page-facts", ["referrer", "screenWidth"], { "emit": { "success": "S", "failure": "F" } }])]);

    let beacon = effects_of(
        &program(
            "[ version: \"1.0.0\", egress: [\"https://x.test\"] ]",
            r#"(browser/beacon "https://x.test/e" { text: @entity.id, key: @entity.id } { emit: { success: S, failure: F, cancelled: C } })"#,
        ),
        "GO",
    );
    assert_eq!(
        beacon,
        vec![json!([
            "browser/beacon",
            "https://x.test/e",
            { "text": "@entity.id", "key": "@entity.id" },
            { "emit": { "success": "S", "failure": "F", "cancelled": "C" } }
        ])]
    );
}

#[test]
fn on_device_model_effects_lower_to_the_contract_forms() {
    let prompt = effects_of(
        &program(
            "",
            r#"(browser/prompt @entity.id (json/stringify { type: "object" }) { key: @entity.id, emit: { success: S, failure: F, cancelled: C } })"#,
        ),
        "GO",
    );
    assert_eq!(
        prompt,
        vec![json!([
            "browser/prompt",
            "@entity.id",
            ["json/stringify", { "type": "object" }],
            { "key": "@entity.id", "emit": { "success": "S", "failure": "F", "cancelled": "C" } }
        ])]
    );

    let embed = effects_of(&program("", r#"(browser/embed [@entity.id, "b"] { emit: { success: S } })"#), "GO");
    assert_eq!(embed, vec![json!(["browser/embed", ["@entity.id", "b"], { "emit": { "success": "S" } }])]);

    let bare = effects_of(&program("", r#"(browser/embed ["a"])"#), "GO");
    assert_eq!(bare, vec![json!(["browser/embed", ["a"]])]);
}
