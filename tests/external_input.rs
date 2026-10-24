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
  trait TaskPersistor -> Task {{
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

fn union_program(field: &str) -> String {
    format!(
        r#"app Probe
type Adv = {{ id : number }}
type Holder = {{ via : [(Adv | string)] }}
orbital TaskOrbital {{
  entity Task [persistent: tasks] {{
    id : string!
  }}
  trait TaskPersistor -> Task {{
    initial: idle
    state idle {{
      INIT -> idle
      DO_SCAN -> idle
    }}
    listens {{
      DO_SCAN -> external {{ {field} }}
    }}
  }}
  page "/" -> TaskPersistor [initial]
}}"#
    )
}

#[test]
fn a_list_of_a_union_keeps_its_element_union_in_items() {
    let schema = parse_and_lower_str(&union_program("via : [(Adv | string)]")).expect("lowers");
    let field = &event(&schema, "DO_SCAN")["payloadSchema"][0];
    assert_eq!(field["name"], "via");
    assert_eq!(field["type"], "array");
    assert_eq!(field["items"]["type"], "union");
    assert_eq!(field["items"]["name"], "item");
    let variants: Vec<&str> = field["items"]["properties"].as_array().expect("variants").iter().map(|v| v["name"].as_str().unwrap_or("")).collect();
    assert_eq!(variants, vec!["Adv", "string"]);
}

#[test]
fn a_struct_alias_member_that_is_a_list_of_a_union_keeps_it_too() {
    let schema = parse_and_lower_str(&union_program("holder : Holder")).expect("lowers");
    let holder = &event(&schema, "DO_SCAN")["payloadSchema"][0];
    let via = holder["properties"].as_array().expect("holder members").iter().find(|m| m["name"] == "via").expect("via member").clone();
    assert_eq!(via["type"], "array");
    assert_eq!(via["items"]["type"], "union");
}

#[test]
fn control_lists_of_structs_and_of_primitives_are_unchanged() {
    let schema = parse_and_lower_str(&union_program("advs : [Adv]\n        names : [string]")).expect("lowers");
    let ev = event(&schema, "DO_SCAN");
    assert_eq!(ev["payloadSchema"][0]["type"], "[object]");
    assert_eq!(ev["payloadSchema"][1]["type"], "[string]");
    assert!(ev["payloadSchema"][0].get("items").is_none());
    assert!(ev["payloadSchema"][1].get("items").is_none());
}

#[test]
fn a_map_of_a_union_keeps_its_value_union_in_items() {
    let schema = parse_and_lower_str(&union_program("props : Map string (string | number | boolean)")).expect("lowers");
    let field = &event(&schema, "DO_SCAN")["payloadSchema"][0];
    assert_eq!(field["type"], "object");
    assert!(field.get("properties").is_none());
    assert_eq!(field["items"]["type"], "union");
}

#[test]
fn control_a_map_of_one_kind_keeps_its_type_string() {
    let schema = parse_and_lower_str(&union_program("tags : Map string number")).expect("lowers");
    let field = &event(&schema, "DO_SCAN")["payloadSchema"][0];
    assert_eq!(field["type"], "Map<string,number>");
    assert!(field.get("items").is_none());
}
