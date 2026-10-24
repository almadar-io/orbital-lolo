// A `.lolo` tick is declared at trait level, so it has no declaring state to lower into `appliesTo`; an author scopes it to a state with a `when (== @state ...)` guard, which lowers as written.

use orbital_lolo::parse_and_lower_str;
use serde_json::Value;

fn source(tick_guard: &str) -> String {
    format!(
        r#"app Clock
orbital ClockOrbital {{
  entity Reminder [persistent: reminders] {{
    id : string!
    due : number = 0
  }}
  trait ReminderScan -> Reminder {{
    initial: idle
    state idle {{
      INIT -> idle
    }}
    state running {{
      STOP -> idle
    }}
    ticks {{
      scan every 1h {tick_guard}
        (set @entity.due @now)
    }}
  }}
}}"#
    )
}

fn first_tick(src: &str) -> Value {
    let orb = parse_and_lower_str(src).unwrap();
    serde_json::to_value(&orb).unwrap()["orbitals"][0]["traits"][0]["ticks"][0].clone()
}

#[test]
fn a_state_guard_lowers_as_the_authors_guard_and_sets_no_applies_to() {
    let tick = first_tick(&source(r#"when (== @state "running")"#));
    assert!(tick.get("appliesTo").is_none(), "{tick}");
    assert!(tick["guard"].to_string().contains("@state"), "{tick}");
}

#[test]
fn control_an_unguarded_tick_lowers_with_neither() {
    let tick = first_tick(&source(""));
    assert!(tick.get("appliesTo").is_none(), "{tick}");
    assert!(tick.get("guard").is_none(), "{tick}");
}
