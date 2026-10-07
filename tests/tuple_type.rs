// `[T1, T2, …]` — a fixed-length tuple type (two or more slots; `[T]` stays an array).
// Canvas lighting's `position : [number, number, number]` needed it: as `[number]` the
// compiled config type was `number[]`, not the component's tuple.

use orbital_lolo::parse_and_lower_str;
use serde_json::Value;

fn program(member: &str, knob: &str, default: &str) -> String {
    format!(
        r#"app Probe
orbital LightOrbital {{
  type Light = {{
    position : {member}
  }}
  entity Lamp [runtime] {{
    id : string!
  }}
  trait LampView -> Lamp [interaction] {{
    config {{
      light : Light = {{ position: [1, 2, 3] }}
      pos : {knob} = {default}
    }}
    initial: idle
    state idle {{
      INIT -> idle
        (render-ui main {{ type: "stack", a: @config.light, b: @config.pos }})
    }}
  }}
  page "/" -> LampView [initial]
}}"#
    )
}

fn config(src: &str) -> Value {
    let schema = parse_and_lower_str(src).expect("program lowers");
    let json = serde_json::to_value(schema).expect("schema serializes");
    json["orbitals"][0]["traits"][0]["config"].clone()
}

#[test]
fn a_tuple_member_and_knob_lower_as_tuple_types() {
    let c = config(&program("[number, number, number]", "[number, number]", "[4, 5]"));
    let position = &c["light"]["properties"]["position"];
    assert_eq!(position["type"], "tuple", "{c}");
    assert_eq!(position["properties"].as_object().map(|o| o.len()), Some(3), "{c}");
    for slot in ["0", "1", "2"] {
        assert_eq!(position["properties"][slot]["type"], "number", "{c}");
        assert_eq!(position["properties"][slot]["name"], slot, "a slot is a field, named by its index: {c}");
    }
    assert_eq!(c["pos"]["type"], "[number, number]", "{c}");
    assert_eq!(c["pos"]["default"], serde_json::json!([4.0, 5.0]), "{c}");
}

#[test]
fn control_a_single_slot_stays_an_array() {
    let c = config(&program("[number]", "[number]", "[4, 5, 6]"));
    assert_eq!(c["light"]["properties"]["position"]["type"], "array", "{c}");
    assert_eq!(c["light"]["properties"]["position"]["items"]["type"], "number", "{c}");
    assert_eq!(c["pos"]["type"], "[number]", "{c}");
}

// A default's arity and slot types are checked by `orb validate`
// (orbital-compiler `config_type::tuple_shape_tests`), where config defaults are checked.
