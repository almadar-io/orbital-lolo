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
  trait TaskBrowse -> Task [interaction] {{
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
        "      DO_CREATE -> external { data : { title : string! } }",
        "",
    );
    let ev = event(&src, "DO_CREATE");
    assert_eq!(ev["listened"], Value::Bool(true));
    assert_eq!(ev["external"], Value::Bool(true));
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
