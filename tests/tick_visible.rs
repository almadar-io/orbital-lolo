// The tick `[visible]` tag: a browser tick that pauses while its trait's output is off screen.
// Parses (en/sl), lowers to `whileVisible`, round-trips, and cannot combine with `[background]`.

use orbital_lolo::{format_program, parse_and_lower_str, parse_str, ParseError};
use serde_json::Value;

fn source(tick_tags: &str) -> String {
    format!(
        r#"app Clock
orbital ClockOrbital {{
  entity Frame [runtime] {{
    id : string!
    n : number = 0
  }}
  trait Spin -> Frame [interaction] {{
    initial: idle
    state idle {{
      INIT -> idle
        (render-ui main {{ type: typography, content: "spin" }})
    }}
    ticks {{
      turn every 100{tick_tags}
        (set @entity.n (+ @entity.n 1))
    }}
  }}
}}"#
    )
}

fn first_tick(orb: &orbital_core::OrbitalSchema) -> Value {
    let json = serde_json::to_value(orb).unwrap();
    json["orbitals"][0]["traits"][0]["ticks"][0].clone()
}

#[test]
fn visible_tag_lowers_to_while_visible() {
    let tick = first_tick(&parse_and_lower_str(&source(" [visible]")).unwrap());
    assert_eq!(tick["whileVisible"], Value::Bool(true), "{tick}");
    assert!(tick.get("runsInBackground").is_none(), "{tick}");
}

#[test]
fn an_untagged_tick_keeps_running_off_screen() {
    let tick = first_tick(&parse_and_lower_str(&source("")).unwrap());
    assert!(
        tick.get("whileVisible").is_none(),
        "running off screen is the default: {tick}"
    );
}

#[test]
fn visible_and_background_cannot_combine() {
    let err =
        parse_str(&source(" [visible, background]")).expect_err("a background tick has no screen");
    assert!(
        matches!(&err, ParseError::ConflictingTickTags { .. }),
        "{err:?}"
    );
}

#[test]
fn the_formatter_round_trips_the_tag() {
    let program = parse_str(&source(" [visible]")).unwrap();
    let formatted = format_program(&program);
    assert!(
        formatted.contains("turn every 100ms [visible]"),
        "{formatted}"
    );
    let again = first_tick(&parse_and_lower_str(&formatted).unwrap());
    assert_eq!(again["whileVisible"], Value::Bool(true));
}

#[test]
fn the_slovenian_tag_lowers_identically() {
    let sl = r#"aplikacija Clock
orbitala ClockOrbital {
  entiteta Frame [izvajanje] {
    id : niz!
  }
  znacilnost Spin -> Frame [interakcija] {
    zacetno: idle
    stanje idle {
      INIT -> idle
    }
    utripi {
      turn vsak 100 [vidno]
        (nastavi @entity.id "x")
    }
  }
}"#;
    let tick = first_tick(&parse_and_lower_str(sl).unwrap());
    assert_eq!(tick["whileVisible"], Value::Bool(true), "{tick}");
}
