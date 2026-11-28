// Deadline ticks: `NAME at <expr> in <states>` parses, lowers to `at`, round-trips, and `at` stays a usable field name.

use orbital_core_base::schema::types::OrbitalSchema;
use orbital_lolo::{format_program, parse_and_lower_str, parse_str, schema_to_lolo, unlower};
use serde_json::{json, Value};

fn source(tick: &str) -> String {
    format!(
        r#"app Clock
orbital ClockOrbital {{
  entity Reminder [persistent: reminders] {{
    id : string!
    dueAt : number = 0
    startedAt : number = 0
    fired : number = 0
  }}
  trait ReminderScan -> Reminder {{
    initial: armed
    state armed {{
      INIT -> armed
    }}
    state waiting {{
      INIT -> waiting
    }}
    ticks {{
      {tick}
    }}
  }}
}}"#
    )
}

fn first_tick(orb: &OrbitalSchema) -> Value {
    let json = serde_json::to_value(orb).unwrap();
    json["orbitals"][0]["traits"][0]["ticks"][0].clone()
}

const BINDING_TICK: &str = "fire at @entity.dueAt in armed [background]\n        (set @entity.fired 1)";

#[test]
fn an_at_tick_lowers_to_at_with_no_interval() {
    let tick = first_tick(&parse_and_lower_str(&source(BINDING_TICK)).unwrap());
    assert_eq!(tick["name"], "fire");
    assert!(tick["interval"].is_null(), "{tick}");
    assert_eq!(tick["at"], json!("@entity.dueAt"), "{tick}");
    assert_eq!(tick["appliesTo"], json!(["armed"]), "{tick}");
    assert_eq!(tick["runsInBackground"], Value::Bool(true), "{tick}");
}

#[test]
fn a_compound_at_expression_lowers_to_the_operator_call() {
    let tick = first_tick(
        &parse_and_lower_str(&source(
            "fire at (time/add @entity.startedAt 500 \"ms\") in waiting\n        (set @entity.fired 1)",
        ))
        .unwrap(),
    );
    assert_eq!(tick["at"], json!(["time/add", "@entity.startedAt", 500.0, "ms"]), "{tick}");
    assert!(tick["interval"].is_null(), "{tick}");
}

#[test]
fn an_every_tick_keeps_its_interval_and_has_no_at() {
    let tick = first_tick(
        &parse_and_lower_str(&source("fire every 100ms in armed\n        (set @entity.fired 1)")).unwrap(),
    );
    assert_eq!(tick["interval"], json!(100.0), "{tick}");
    assert!(tick["at"].is_null(), "control: every has no at: {tick}");
}

#[test]
fn at_stays_a_field_name_in_an_entity_and_a_payload() {
    let src = r#"app Clock
orbital ClockOrbital {
  entity Spawn [runtime] {
    id : string!
    at : number = 0
  }
  trait SpawnWatch -> Spawn {
    initial: idle
    state idle {
      RUN -> idle
        (set @entity.at 1)
    }
    emits {
      VU_SPAWN -> external {
        at : number!
      }
    }
  }
}"#;
    parse_and_lower_str(src).expect("`at` as a field name must keep parsing");
}

#[test]
fn the_formatter_round_trips_an_at_tick() {
    let program = parse_str(&source(BINDING_TICK)).unwrap();
    let formatted = format_program(&program);
    assert!(formatted.contains("fire at @entity.dueAt in armed [background]"), "{formatted}");
    let again = first_tick(&parse_and_lower_str(&formatted).unwrap());
    assert_eq!(again["at"], json!("@entity.dueAt"), "{again}");
}

#[test]
fn unlower_restores_an_at_tick() {
    let schema = parse_and_lower_str(&source(BINDING_TICK)).unwrap();
    let text = schema_to_lolo(&schema).expect("schema_to_lolo");
    assert!(text.contains("fire at @entity.dueAt in armed [background]"), "{text}");
}

#[test]
fn a_tick_with_neither_interval_nor_at_fails_to_unlower() {
    let schema = parse_and_lower_str(&source(BINDING_TICK)).unwrap();
    let mut value = serde_json::to_value(&schema).unwrap();
    let tick = value["orbitals"][0]["traits"][0]["ticks"][0].as_object_mut().unwrap();
    tick.remove("at");
    tick.remove("interval");
    let broken: OrbitalSchema = serde_json::from_value(value).unwrap();
    let err = unlower(&broken).expect_err("a tick needs an interval or an at");
    assert!(err.to_string().contains("neither an interval nor an at"), "{err}");
}
