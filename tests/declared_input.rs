// A local `listens { EVENT … }` entry is the trait's declared input: the lowering tags its
// event definition `listened`, so a resolved `.orb` still says which inputs the author declared.

use orbital_lolo::{parse_and_lower_str, schema_to_lolo};
use serde_json::Value;

fn program(listens: &str, emits: &str) -> String {
    format!(
        r#"app Probe
orbital TaskOrbital {{
  entity Task [persistent: tasks] {{
    id : string!
    title : string
  }}
  trait TaskBrowse -> Task {{
    initial: idle
    state idle {{
      INIT -> idle
      REFETCH_PAGE -> idle
      DO_CREATE -> idle
        (persist create Task ?data)
    }}
    listens {{
{listens}
    }}
    emits {{
{emits}
    }}
  }}
  page "/" -> TaskBrowse [initial]
}}"#
    )
}

fn event(src: &str, key: &str) -> Value {
    let schema = parse_and_lower_str(src).expect("program lowers");
    let json = serde_json::to_value(&schema).expect("schema serializes");
    json["orbitals"][0]["traits"][0]["stateMachine"]["events"]
        .as_array()
        .expect("events array")
        .iter()
        .find(|e| e["key"] == key)
        .cloned()
        .unwrap_or_else(|| panic!("event {key} missing"))
}

#[test]
fn a_local_listen_with_a_payload_is_tagged_listened() {
    let src = program("      REFETCH_PAGE { page : number! }", "");
    assert_eq!(event(&src, "REFETCH_PAGE")["listened"], Value::Bool(true));
}

#[test]
fn a_bare_local_listen_is_tagged_listened() {
    let src = program("      REFETCH_PAGE", "");
    assert_eq!(event(&src, "REFETCH_PAGE")["listened"], Value::Bool(true));
}

#[test]
fn an_external_input_is_also_a_declared_listen() {
    let src = program(
        "      DO_CREATE -> input { data : { title : string! } }",
        "",
    );
    let ev = event(&src, "DO_CREATE");
    assert_eq!(ev["listened"], Value::Bool(true));
    assert_eq!(ev["input"], Value::Bool(true));
}

#[test]
fn control_a_transition_only_event_is_not_tagged() {
    let src = program("      REFETCH_PAGE { page : number! }", "");
    assert!(event(&src, "INIT").get("listened").is_none());
}

#[test]
fn control_an_emit_declared_event_is_not_tagged() {
    let src = program("", "      DO_CREATE -> internal { data : string }");
    assert!(event(&src, "DO_CREATE").get("listened").is_none());
}

#[test]
fn control_a_sourced_listen_does_not_tag_its_local_event() {
    let src = program(
        "      TaskBrowse.DONE -> REFETCH_PAGE",
        "      DONE -> internal { id : string }",
    );
    assert!(event(&src, "REFETCH_PAGE").get("listened").is_none());
}

#[test]
fn a_bare_declared_listen_round_trips_through_unlower() {
    let src = program("      REFETCH_PAGE", "");
    let schema = parse_and_lower_str(&src).expect("program lowers");
    let lolo = schema_to_lolo(&schema).expect("schema unlowers");
    let again = parse_and_lower_str(&lolo).expect("unlowered source lowers");
    let json = serde_json::to_value(&again).expect("schema serializes");
    let ev = json["orbitals"][0]["traits"][0]["stateMachine"]["events"]
        .as_array()
        .expect("events array")
        .iter()
        .find(|e| e["key"] == "REFETCH_PAGE")
        .cloned()
        .expect("REFETCH_PAGE survives");
    assert_eq!(ev["listened"], Value::Bool(true), "{lolo}");
}

fn payload_field(src: &str, block: &str, field: &str) -> Value {
    let schema = parse_and_lower_str(src).expect("program lowers");
    let json = serde_json::to_value(&schema).expect("schema serializes");
    let t = &json["orbitals"][0]["traits"][0];
    let fields = if block == "emits" {
        t["emits"][0]["payloadSchema"].clone()
    } else {
        t["stateMachine"]["events"]
            .as_array()
            .expect("events")
            .iter()
            .find(|e| e["key"] == "UPLOADED")
            .expect("UPLOADED")["payloadSchema"]
            .clone()
    };
    fields
        .as_array()
        .expect("payload fields")
        .iter()
        .find(|f| f["name"] == field)
        .cloned()
        .expect("field present")
}

#[test]
fn an_emitted_array_of_inline_structs_keeps_its_element_shape_like_the_listen() {
    let line = "UPLOADED { files : [{ name : string, size : number }] }";
    let emitted = payload_field(&program("", &format!("      {}", line.replace("UPLOADED {", "UPLOADED -> internal {"))), "emits", "files");
    let listened = payload_field(&program(&format!("      {line}"), ""), "listens", "files");
    assert_eq!(emitted["type"], "[object]", "{emitted}");
    assert_eq!(emitted["properties"], listened["properties"], "{emitted} vs {listened}");
}

#[test]
fn control_an_emitted_array_of_scalars_has_no_element_shape() {
    let emitted = payload_field(
        &program("", "      UPLOADED -> internal { tags : [string] }"),
        "emits",
        "tags",
    );
    assert!(emitted.get("properties").is_none(), "{emitted}");
}

fn inbound_program(header: &str, listens: &str) -> String {
    format!(
        r#"app Probe {header}
orbital HookOrbital {{
  entity Req [runtime] {{
    id : string!
  }}
  trait HookIn -> Req {{
    initial: idle
    state idle {{
      INIT -> idle
      PUSH -> idle
        (emit DONE {{ status: 200, body: "{{}}" }})
    }}
    listens {{
{listens}
    }}
    emits {{
      DONE -> internal {{
        status : number!
        body : string!
      }}
    }}
  }}
  page "/" -> HookIn [initial]
}}"#
    )
}

const INBOUND_LISTEN: &str =
    "      PUSH -> inbound \"google-calendar\" answers DONE { headers : Map string string, rawBody : string }";

#[test]
fn an_inbound_listen_lowers_to_the_events_inbound_declaration_and_is_not_an_external_input() {
    let src = inbound_program("[ inbound: [\"google-calendar\"] ]", INBOUND_LISTEN);
    let ev = event(&src, "PUSH");
    assert_eq!(ev["inbound"], serde_json::json!({ "name": "google-calendar", "answers": "DONE" }));
    assert_eq!(ev["listened"], Value::Bool(true));
    assert!(ev.get("input").is_none(), "{ev}");
    assert_eq!(ev["payloadSchema"][0]["name"], "headers", "{ev}");
    let schema = parse_and_lower_str(&src).unwrap();
    assert_eq!(schema.inbound, Some(vec!["google-calendar".to_string()]));
}

#[test]
fn control_no_header_entry_and_no_inbound_listen_leave_both_absent() {
    let src = inbound_program("", "      PUSH { rawBody : string }");
    assert_eq!(parse_and_lower_str(&src).unwrap().inbound, None);
    assert!(event(&src, "PUSH").get("inbound").is_none());
    let external = inbound_program("", "      PUSH -> input { rawBody : string }");
    assert_eq!(event(&external, "PUSH")["input"], Value::Bool(true));
    assert!(event(&external, "PUSH").get("inbound").is_none());
}

#[test]
fn inbound_survives_unlower_and_re_lowers_to_the_same_schema() {
    let src = inbound_program("[ inbound: [\"google-calendar\"] ]", INBOUND_LISTEN);
    let schema = parse_and_lower_str(&src).unwrap();
    let printed = schema_to_lolo(&schema).expect("schema_to_lolo");
    assert!(printed.contains("inbound: [\"google-calendar\"]"), "{printed}");
    assert!(printed.contains("-> inbound \"google-calendar\" answers DONE"), "{printed}");
    let again = parse_and_lower_str(&printed).expect("printed form lowers");
    assert_eq!(again.inbound, schema.inbound);
    assert_eq!(
        serde_json::to_value(&again).unwrap()["orbitals"][0]["traits"][0]["stateMachine"]["events"],
        serde_json::to_value(&schema).unwrap()["orbitals"][0]["traits"][0]["stateMachine"]["events"]
    );
}

#[test]
fn an_inbound_listen_needs_a_hook_name_and_an_answer_event() {
    for bad in [
        "      PUSH -> inbound answers DONE { rawBody : string }",
        "      PUSH -> inbound \"google-calendar\" { rawBody : string }",
        "      PUSH -> inbound \"google-calendar\" answers DONE",
    ] {
        assert!(parse_and_lower_str(&inbound_program("", bad)).is_err(), "{bad}");
    }
}

const VERIFIED_LISTEN: &str = "      PUSH -> inbound \"google-calendar\" answers DONE verifies \"x-goog-channel-token\" \"CHANNEL_TOKEN\" { headers : Map string string, verified : boolean }";

#[test]
fn a_verifies_clause_lowers_to_the_declaration_and_survives_unlower() {
    let src = inbound_program("[ inbound: [\"google-calendar\"] ]", VERIFIED_LISTEN);
    assert_eq!(
        event(&src, "PUSH")["inbound"],
        serde_json::json!({
            "name": "google-calendar",
            "answers": "DONE",
            "verifies": { "header": "x-goog-channel-token", "credential": "CHANNEL_TOKEN" }
        })
    );
    let schema = parse_and_lower_str(&src).unwrap();
    let printed = schema_to_lolo(&schema).expect("schema_to_lolo");
    assert!(
        printed.contains("answers DONE verifies \"x-goog-channel-token\" \"CHANNEL_TOKEN\""),
        "{printed}"
    );
    let again = parse_and_lower_str(&printed).expect("printed form lowers");
    assert_eq!(
        serde_json::to_value(&again).unwrap()["orbitals"][0]["traits"][0]["stateMachine"]["events"],
        serde_json::to_value(&schema).unwrap()["orbitals"][0]["traits"][0]["stateMachine"]["events"]
    );
}

#[test]
fn control_an_inbound_listen_without_verifies_declares_none_and_a_half_clause_is_an_error() {
    let src = inbound_program("[ inbound: [\"google-calendar\"] ]", INBOUND_LISTEN);
    assert!(event(&src, "PUSH")["inbound"].get("verifies").is_none());
    for bad in [
        "      PUSH -> inbound \"g\" answers DONE verifies \"x-token\" { rawBody : string }",
        "      PUSH -> inbound \"g\" answers DONE verifies { rawBody : string }",
    ] {
        assert!(parse_and_lower_str(&inbound_program("", bad)).is_err(), "{bad}");
    }
}
