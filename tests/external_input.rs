// A declared external input: `listens { EVENT -> external { payload } }` lowers onto the
// trait's own event definition, marked external, with its payload and annotations.

use orbital_lolo::{parse_and_lower_str, parse_str};
use serde_json::Value;

fn program(listens: &str) -> String {
    format!(
        r#"app Probe
orbital TaskOrbital {{
  entity Task [persistent: tasks] {{
    id : string!
    title : string
  }}
  trait TaskPersistor -> Task [lifecycle] {{
    initial: idle
    state idle {{
      INIT -> idle
      DO_CREATE -> idle
        (persist create Task ?data)
    }}
    listens {{
{listens}
    }}
  }}
  page "/" -> TaskPersistor [initial]
}}"#
    )
}

fn event(schema: &orbital_core_base::schema::OrbitalSchema, key: &str) -> Value {
    let json = serde_json::to_value(schema).expect("schema serializes");
    let events = json["orbitals"][0]["traits"][0]["stateMachine"]["events"]
        .as_array()
        .expect("events array")
        .clone();
    events
        .into_iter()
        .find(|e| e["key"] == key)
        .unwrap_or_else(|| panic!("event {key} missing"))
}

#[test]
fn an_external_input_lowers_marked_with_its_payload_and_annotations() {
    let src = program(
        "      DO_CREATE -> external { data : { title : string! } }\n        @description \"Create a task\"\n        @synonyms \"add task, new task\"",
    );
    let schema = parse_and_lower_str(&src).expect("external input lowers");
    let ev = event(&schema, "DO_CREATE");
    assert_eq!(ev["external"], Value::Bool(true));
    assert_eq!(ev["description"], "Create a task");
    assert_eq!(ev["synonyms"], "add task, new task");
    assert_eq!(ev["payloadSchema"][0]["name"], "data");
}

#[test]
fn control_a_local_listen_without_the_arrow_is_not_external() {
    let src = program("      DO_CREATE { data : { title : string! } }");
    let schema = parse_and_lower_str(&src).expect("local listen lowers");
    let ev = event(&schema, "DO_CREATE");
    assert!(ev.get("external").is_none(), "{ev}");
}

#[test]
fn an_external_input_must_declare_its_payload() {
    let src = program("      DO_CREATE -> external");
    assert!(
        parse_str(&src).is_err(),
        "`-> external` with no payload braces must not parse"
    );
}

#[test]
fn an_empty_payload_is_a_declared_payload() {
    let src = program("      DO_CREATE -> external {}");
    let schema = parse_and_lower_str(&src).expect("empty declared payload lowers");
    assert_eq!(event(&schema, "DO_CREATE")["external"], Value::Bool(true));
}

#[test]
fn only_external_follows_the_arrow_on_a_local_listen() {
    let src = program("      DO_CREATE -> internal { data : string }");
    assert!(
        parse_str(&src).is_err(),
        "`-> internal` is not an input declaration"
    );
}

#[test]
fn a_sourced_listen_is_unchanged() {
    let src = program("      Src.DONE -> DO_CREATE");
    assert!(parse_str(&src).is_ok());
}
