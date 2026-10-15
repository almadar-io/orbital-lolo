// `(persist upsert <Entity> <data>)` is an authored persist action: it parses, lowers to the same
// `["persist", <action>, Entity, data]` row as `create`, and prints back as the same source (EXEC-STEP-8).

use orbital_lolo::{parse_and_lower_str, schema_to_lolo};
use serde_json::Value;

fn program(action: &str) -> String {
    format!(
        r#"app Probe
orbital MixOrbital {{
  entity Mix [persistent: mixes] {{
    id : string!
    level : number
  }}
  trait MixSave -> Mix [interaction] {{
    initial: idle
    state idle {{
      INIT -> idle
      SAVE -> idle
        (persist {action} Mix {{ id: "mix", level: 3 }})
    }}
  }}
  page "/" -> MixSave [initial]
}}"#
    )
}

fn save_effect(src: &str) -> Value {
    let schema = parse_and_lower_str(src).expect("program lowers");
    let json = serde_json::to_value(&schema).expect("schema serializes");
    json["orbitals"][0]["traits"][0]["stateMachine"]["transitions"]
        .as_array()
        .expect("transitions")
        .iter()
        .find(|t| t["event"] == "SAVE")
        .expect("SAVE transition")["effects"][0]
        .clone()
}

#[test]
fn upsert_lowers_to_a_persist_row_with_the_upsert_action() {
    let effect = save_effect(&program("upsert"));
    assert_eq!(effect[0], "persist");
    assert_eq!(effect[1], "upsert");
    assert_eq!(effect[2], "Mix");
    assert_eq!(effect[3]["id"], "mix");
}

#[test]
fn control_create_lowers_with_its_own_action() {
    assert_eq!(save_effect(&program("create"))[1], "create");
}

#[test]
fn upsert_prints_back_and_lowers_to_the_same_effect() {
    let schema = parse_and_lower_str(&program("upsert")).expect("program lowers");
    let printed = schema_to_lolo(&schema).expect("unlowers");
    assert!(
        printed.contains("persist upsert Mix"),
        "the printed source keeps the action word:\n{printed}"
    );
    assert_eq!(save_effect(&printed), save_effect(&program("upsert")));
}
