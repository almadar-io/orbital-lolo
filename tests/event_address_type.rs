// `EventAddress` — a value addressing a declared external input (`Trait.EVENT`,
// `Orbital.Trait.EVENT`). Its own type on the wire, so a reader (the L1 fill, a
// service contract) knows it is an address, not a bare event.

use orbital_lolo::parse_and_lower_str;
use serde_json::Value;

fn program(member_type: &str, default: &str) -> String {
    format!(
        r#"app Probe
orbital TaskOrbital {{
  type AgentTool = {{
    event : {member_type}
  }}
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
      DO_CREATE -> external {{ data : {{ title : string! }} }}
    }}
  }}
  trait Assistant -> Task [interaction] {{
    config {{
      tools : [AgentTool] = {default}
    }}
    initial: idle
    state idle {{
      INIT -> idle
        (render-ui main {{ type: "stack", probe: @config.tools }})
    }}
  }}
  page "/" -> TaskPersistor, Assistant [initial]
}}"#
    )
}

fn tools(src: &str) -> Value {
    let schema = parse_and_lower_str(src).expect("program lowers");
    let json = serde_json::to_value(schema).expect("schema serializes");
    json["orbitals"][0]["traits"][1]["config"]["tools"].clone()
}

#[test]
fn an_event_address_member_lowers_as_its_own_type() {
    let t = tools(&program("EventAddress", "[]"));
    assert_eq!(
        t["items"]["properties"]["event"]["type"], "EventAddress",
        "{t}"
    );
}

#[test]
fn an_address_is_legal_at_an_event_address_position() {
    let t = tools(&program(
        "EventAddress",
        "[{ event: TaskPersistor.DO_CREATE }]",
    ));
    assert_eq!(t["default"][0]["event"], "TaskPersistor.DO_CREATE", "{t}");
}

#[test]
fn a_call_key_member_lowers_as_its_own_type() {
    let t = tools(&program("CallKey", "[]"));
    assert_eq!(t["items"]["properties"]["event"]["type"], "CallKey", "{t}");
}

#[test]
fn control_a_string_literal_is_still_accepted_at_a_bytes_position() {
    assert!(parse_and_lower_str(&program("bytes", "[{ event: \"AAEC\" }]")).is_ok());
}

#[test]
fn control_an_event_member_still_lowers_as_event() {
    let t = tools(&program("event", "[]"));
    assert_eq!(t["items"]["properties"]["event"]["type"], "event", "{t}");
}

#[test]
fn control_an_address_at_a_string_position_is_refused() {
    assert!(
        parse_and_lower_str(&program("string", "[{ event: TaskPersistor.DO_CREATE }]")).is_err()
    );
}
