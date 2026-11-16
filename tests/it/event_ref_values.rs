// An `event`-typed config value may use `listens` addressing: `EVENT`,
// `Trait.EVENT`, `Orbital.Trait.EVENT`. A dotted value is legal only inside a
// config value and only at a position whose declared type is `event`.

use orbital_lolo::{parse_and_lower_str, parse_str};
use serde_json::Value;

fn program(config: &str, effects: &str) -> String {
    let knob = config.trim_start().split(' ').next().unwrap_or_default();
    format!(
        r#"app Probe
orbital TaskOrbital {{
  type AgentTool = {{
    event : event
    label : string
  }}
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
      DO_CREATE -> input {{ data : {{ title : string! }} }}
    }}
  }}
  trait Assistant -> Task {{
    config {{
{config}
    }}
    initial: idle
    state idle {{
      INIT -> idle
        (render-ui main {{ type: "stack", probe: @config.{knob} }})
{effects}
    }}
  }}
  page "/" -> TaskPersistor, Assistant [initial]
}}"#
    )
}

fn assistant_config(src: &str) -> Value {
    let schema = parse_and_lower_str(src).expect("program lowers");
    let json = serde_json::to_value(schema).expect("schema serializes");
    json["orbitals"][0]["traits"][1]["config"].clone()
}

#[test]
fn a_scalar_event_knob_holds_a_trait_qualified_event() {
    let config = assistant_config(&program("      one : event = TaskPersistor.DO_CREATE", ""));
    assert_eq!(
        config["one"]["default"], "TaskPersistor.DO_CREATE",
        "{config}"
    );
}

#[test]
fn a_struct_event_member_holds_an_orbital_qualified_event() {
    let config = assistant_config(&program(
        "      tools : [AgentTool] = [{ event: TaskOrbital.TaskPersistor.DO_CREATE, label: \"Create\" }]",
        "",
    ));
    assert_eq!(
        config["tools"]["default"][0]["event"], "TaskOrbital.TaskPersistor.DO_CREATE",
        "{config}"
    );
}

#[test]
fn control_a_bare_event_value_is_unchanged() {
    let config = assistant_config(&program("      one : event = DO_CREATE", ""));
    assert_eq!(config["one"]["default"], "DO_CREATE");
}

#[test]
fn a_dotted_value_at_a_string_knob_is_rejected() {
    let err = parse_and_lower_str(&program("      one : string = TaskPersistor.DO_CREATE", ""))
        .expect_err("a dotted value outside an event position must not lower");
    assert!(
        err.to_string().contains("ELOLO_T_EVENT_REF_POSITION"),
        "{err}"
    );
}

#[test]
fn a_dotted_value_at_a_string_struct_member_is_rejected() {
    let err = parse_and_lower_str(&program(
        "      tools : [AgentTool] = [{ event: DO_CREATE, label: TaskPersistor.DO_CREATE }]",
        "",
    ))
    .expect_err("a dotted value at a string member must not lower");
    assert!(
        err.to_string().contains("ELOLO_T_EVENT_REF_POSITION"),
        "{err}"
    );
}

#[test]
fn control_a_dotted_atom_outside_config_still_does_not_parse() {
    let src = program(
        "      one : event = DO_CREATE",
        "        (emit TaskPersistor.DO_CREATE)",
    );
    assert!(
        parse_str(&src).is_err(),
        "dotted atoms are config-value-only"
    );
}

#[test]
fn a_call_site_override_accepts_a_qualified_event() {
    let src = r#"app Probe
orbital TaskOrbital {
  entity Task [persistent: tasks] {
    id : string!
  }
  trait Assistant = Agent.traits.Assistant -> Task {
    config {
      tools: [{ event: TaskOrbital.TaskPersistor.DO_CREATE }]
    }
  }
  page "/" -> Assistant [initial]
}"#;
    assert!(parse_str(src).is_ok(), "{:?}", parse_str(src).err());
}
