//! `uses lazy`: the imported behavior is compiled to its own `.orb` and loaded
//! when one of its pages is opened. The modifier lowers to `lazy: true` on the
//! use declaration; everything else about `uses` is unchanged.

use orbital_lolo::{parse_and_lower_str, parse_str};

const LAZY: &str = r#"
uses lazy WhyAlmadar from "./blog/why-we-started-almadar"
uses Hero from "std/behaviors/std-hero"

orbital BlogWhyAlmadar = WhyAlmadar.orbitals.Post {
  pages {
    "/post": "/blog/why-we-started-almadar"
  }
}
"#;

#[test]
fn uses_lazy_parses_the_modifier() {
    let program = parse_str(LAZY).expect("parse");
    let lazy = program.uses.iter().find(|u| u.alias == "WhyAlmadar").expect("lazy use");
    assert!(lazy.lazy);
    assert_eq!(lazy.path, "./blog/why-we-started-almadar");
}

#[test]
fn control_a_plain_uses_is_not_lazy() {
    let program = parse_str(LAZY).expect("parse");
    let eager = program.uses.iter().find(|u| u.alias == "Hero").expect("eager use");
    assert!(!eager.lazy);
}

#[test]
fn uses_lazy_lowers_to_lazy_true_and_plain_uses_to_none() {
    let schema = parse_and_lower_str(LAZY).expect("lower");
    let orbital = schema.orbitals.iter().find(|o| o.name == "BlogWhyAlmadar").expect("orbital");
    let uses = orbital.uses.as_ref().expect("uses");
    let lazy = uses.iter().find(|u| u.alias == "WhyAlmadar").expect("lazy use");
    let eager = uses.iter().find(|u| u.alias == "Hero").expect("eager use");
    assert_eq!(lazy.lazy, Some(true));
    assert_eq!(eager.lazy, None, "an eager use carries no lazy key, so existing .orb output is unchanged");
}

#[test]
fn edge_uses_lazy_inside_an_orbital_block() {
    let source = r#"
orbital Site {
  uses lazy WhyAlmadar from "./blog/why-we-started-almadar"
  entity SiteState [runtime] {
    id : string
  }
}
"#;
    let schema = parse_and_lower_str(source).expect("lower");
    let uses = schema.orbitals[0].uses.as_ref().expect("uses");
    assert_eq!(uses[0].lazy, Some(true));
}

#[test]
fn edge_lazy_is_not_an_alias() {
    let source = r#"uses lazy from "./blog/x""#;
    assert!(parse_str(source).is_err(), "`lazy` must be followed by an alias");
}

#[test]
fn the_formatter_keeps_the_lazy_modifier() {
    let formatted = orbital_lolo::format_program(&parse_str(LAZY).expect("parse"));
    assert!(formatted.contains("uses lazy WhyAlmadar from \"./blog/why-we-started-almadar\""), "{formatted}");
    assert!(formatted.contains("uses Hero from \"std/behaviors/std-hero\""), "control: an eager use stays plain\n{formatted}");
}
