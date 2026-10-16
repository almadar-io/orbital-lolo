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

const OAUTH_CLIENT: &str = r#"{ tag: google, mode: real, issuer: "https://accounts.google.com", clientId: @config.token, clientSecret: @config.token, clientAuth: post }"#;

fn oauth_client_json() -> Value {
    json!({ "tag": "google", "mode": "real", "issuer": "https://accounts.google.com", "clientId": "@config.token", "clientSecret": "@config.token", "clientAuth": "post" })
}

#[test]
fn the_oauth_effects_lower_with_bare_words_as_strings() {
    let first = |call: &str| effects_of(&program(&format!("{call}\n        (credential-forget @config.token)"))).remove(0);
    let status = first("(oauth-connection-status google { emit: { success: DONE } })");
    assert_eq!(status, json!(["oauth-connection-status", "google", { "emit": { "success": "DONE" } }]));
    let forget = first("(oauth-connection-forget github)");
    assert_eq!(forget, json!(["oauth-connection-forget", "github"]));
    let authorize = first(&format!(
        r#"(oauth-authorize {OAUTH_CLIENT} {{ redirectUri: "https://a/cb", scope: ["openid"], params: {{ access_type: "offline" }} }} {{ emit: {{ success: DONE }} }})"#
    ));
    assert_eq!(
        authorize,
        json!(["oauth-authorize", oauth_client_json(),
            { "redirectUri": "https://a/cb", "scope": ["openid"], "params": { "access_type": "offline" } },
            { "emit": { "success": "DONE" } }])
    );
}

#[test]
fn the_oauth_client_http_spelling_lowers_to_the_oauth_endpoint_target() {
    let call = format!(
        "(http POST oauth-endpoint token {{ auth: {{ scheme: oauth-client, op: connect, client: {OAUTH_CLIENT}, provider: google, code: @payload.code, state: @payload.state }} }} {{ emit: {{ success: DONE, failure: FAILED }} }})"
    );
    let effects = effects_of(&program(&call));
    assert_eq!(
        effects,
        vec![json!(["http", "POST", "oauth-endpoint", "token",
            { "auth": { "scheme": "oauth-client", "op": "connect", "client": oauth_client_json(), "provider": "google",
                "code": "@payload.code", "state": "@payload.state" } },
            { "emit": { "success": "DONE", "failure": "FAILED" } }])]
    );
    let discovery = effects_of(&program(&format!(
        "(http GET \"https://accounts.google.com\" \"/.well-known/openid-configuration\" {{ auth: {{ scheme: oauth-client, op: discovery, client: {OAUTH_CLIENT} }} }})"
    )));
    assert_eq!(discovery[0][1], "GET");
}

#[test]
fn the_formatter_keeps_the_oauth_heads() {
    for call in [
        "(oauth-connection-status google { emit: { success: DONE } })".to_string(),
        "(oauth-connection-forget google)".to_string(),
        format!("(oauth-authorize {OAUTH_CLIENT} {{ redirectUri: \"https://a/cb\" }})"),
    ] {
        let src = program(&format!("{call}\n        (credential-forget @config.token)"));
        let formatted = format_program(&parse_str(&src).expect("parse"));
        assert!(formatted.contains(call.split_whitespace().next().unwrap()), "{formatted}");
        assert_eq!(effects_of(&formatted), effects_of(&src), "{call}");
    }
}

#[test]
fn the_clients_map_and_the_authorize_key_lower_as_written() {
    let first = |call: &str| effects_of(&program(&format!("{call}\n        (credential-forget @config.token)"))).remove(0);
    let github = OAUTH_CLIENT.replace("tag: google", "tag: github");
    let routed = first(&format!(
        "(http POST oauth-endpoint token {{ auth: {{ scheme: oauth-client, op: code, clients: {{ google: {OAUTH_CLIENT}, github: {github} }}, code: @payload.code, state: @payload.state }} }} {{ emit: {{ success: DONE }} }})"
    ));
    let mut github_json = oauth_client_json();
    github_json["tag"] = json!("github");
    assert_eq!(
        routed[4]["auth"]["clients"],
        json!({ "google": oauth_client_json(), "github": github_json })
    );
    let keyed = first(&format!(
        r#"(oauth-authorize {OAUTH_CLIENT} {{ redirectUri: "https://a/cb" }} {{ key: @entity.id, emit: {{ success: DONE, failure: FAILED }} }})"#
    ));
    assert_eq!(keyed[3], json!({ "key": "@entity.id", "emit": { "success": "DONE", "failure": "FAILED" } }));
}

