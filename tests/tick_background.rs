// The tick `[background]` tag: parses (en/sl), lowers to `runsInBackground`, round-trips, and rejects unknown tick tags.

use orbital_lolo::{format_program, parse_and_lower_str, parse_str, ParseError};
use serde_json::Value;

fn source(tick_tags: &str) -> String {
    format!(
        r#"app Clock
orbital ClockOrbital {{
  entity Reminder [persistent: reminders] {{
    id : string!
    due : number = 0
  }}
  trait ReminderScan -> Reminder [lifecycle] {{
    initial: idle
    state idle {{
      INIT -> idle
    }}
    ticks {{
      scan every 1h{tick_tags} when (> @now 0)
        (set @entity.due @now)
    }}
  }}
}}"#
    )
}

fn first_tick(orb: &orbital_core_base::schema::types::OrbitalSchema) -> Value {
    let json = serde_json::to_value(orb).unwrap();
    json["orbitals"][0]["traits"][0]["ticks"][0].clone()
}

#[test]
fn background_tag_lowers_to_runs_in_background() {
    let tick = first_tick(&parse_and_lower_str(&source(" [background]")).unwrap());
    assert_eq!(tick["runsInBackground"], Value::Bool(true), "{tick}");
    assert_eq!(tick["name"], "scan");
}

#[test]
fn an_untagged_tick_omits_the_key() {
    let tick = first_tick(&parse_and_lower_str(&source("")).unwrap());
    assert!(
        tick.get("runsInBackground").is_none(),
        "mount-scoped is the default and is not written: {tick}"
    );
}

#[test]
fn an_unknown_tick_tag_is_a_precise_parse_error() {
    let err = parse_str(&source(" [always]")).expect_err("unknown tick tag must not parse");
    assert!(
        matches!(&err, ParseError::InvalidTickTag { value, .. } if value == "always"),
        "{err:?}"
    );
}

#[test]
fn the_formatter_round_trips_the_tag() {
    let program = parse_str(&source(" [background]")).unwrap();
    let formatted = format_program(&program);
    assert!(
        formatted.contains("scan every 1h [background] when"),
        "{formatted}"
    );
    let again = first_tick(&parse_and_lower_str(&formatted).unwrap());
    assert_eq!(again["runsInBackground"], Value::Bool(true));
}

#[test]
fn the_slovenian_tag_lowers_identically() {
    let sl = r#"aplikacija Clock
orbitala ClockOrbital {
  entiteta Reminder [trajno: reminders] {
    id : niz!
  }
  znacilnost ReminderScan -> Reminder [zivljenjskiCikel] {
    zacetno: idle
    stanje idle {
      INIT -> idle
    }
    utripi {
      scan vsak 1h [ozadno]
        (nastavi @entity.id "x")
    }
  }
}"#;
    let tick = first_tick(&parse_and_lower_str(sl).unwrap());
    assert_eq!(tick["runsInBackground"], Value::Bool(true), "{tick}");
}
