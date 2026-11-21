//! An integer literal beyond ±(2^53 − 1) is refused (G-CROSS-753): it lowers to a number no host holds exactly, so the
//! program would carry a different value than the one written, and a JavaScript host cannot read it at all.

use orbital_lolo::parse_str;

fn program(literal: &str) -> String {
    format!(
        r#"orbital Counter {{
  entity Tally [runtime] {{
    id : string!
    n : number = 0
  }}

  trait Count -> Tally {{
    initial: idle

    state idle {{
      INIT -> idle
        (set @entity.n {literal})
    }}
  }}
}}
"#
    )
}

#[test]
fn an_integer_literal_beyond_the_exact_range_is_refused() {
    for literal in ["9223372036854775000", "-9223372036854775000", "9007199254740992", "-9007199254740992"] {
        let err = parse_str(&program(literal)).err().map(|e| e.to_string()).unwrap_or_default();
        assert!(err.contains("ELOLO_INTEGER_INEXACT"), "{literal}: {err}");
    }
}

#[test]
fn control_integers_at_the_edge_and_decimals_beyond_it_parse() {
    for literal in ["9007199254740991", "-9007199254740991", "0", "9223372036854775000.0"] {
        assert!(parse_str(&program(literal)).is_ok(), "{literal}: {:?}", parse_str(&program(literal)).err().map(|e| e.to_string()));
    }
}
