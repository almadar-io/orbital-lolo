use orbital_lolo::parse_and_lower_str;
use serde_json::Value;

fn program(payload_type: &str) -> String {
    format!(
        r#"app correlation-union [version: "1.0.0"]
type Correlation = string | number
type Request = {{ requestId : Correlation! }}
orbital Probe {{
  entity Session [runtime] {{
    id : string!
    requestId : Correlation = 0
    request : Request
  }}
  trait Main -> Session {{
    initial: ready
    state ready {{ INIT -> ready }}
    emits {{ ANSWER -> external {{ requestId : {payload_type}! }} }}
    listens {{ REQUEST {{ requestId : {payload_type}! }} }}
  }}
}}
"#
    )
}

#[test]
fn primitive_union_alias_payload_matches_inline_union_in_every_value_position() {
    let lower = |ty: &str| -> Value {
        serde_json::to_value(parse_and_lower_str(&program(ty)).unwrap()).unwrap()
    };
    let aliased = lower("Correlation");
    let inline = lower("(string | number)");
    let a = &aliased["orbitals"][0]["traits"][0];
    let b = &inline["orbitals"][0]["traits"][0];
    assert_eq!(a["emits"], b["emits"]);
    assert_eq!(a["stateMachine"]["events"], b["stateMachine"]["events"]);
    assert_eq!(a["emits"][0]["payloadSchema"][0]["type"], "union");
    let fields = aliased["orbitals"][0]["entity"]["fields"]
        .as_array()
        .unwrap();
    assert_eq!(
        fields.iter().find(|f| f["name"] == "requestId").unwrap()["type"],
        "union"
    );
    assert_eq!(
        fields.iter().find(|f| f["name"] == "request").unwrap()["properties"]["requestId"]["type"],
        "union"
    );
}

#[test]
fn unknown_and_vocabulary_alias_payload_controls_remain_rejected() {
    let unknown = program("Missing");
    assert!(parse_and_lower_str(&unknown).is_err());
    let vocabulary = program("Correlation").replace(
        "type Correlation = string | number",
        "type Correlation = idle | active",
    );
    assert!(parse_and_lower_str(&vocabulary).is_err());
}
