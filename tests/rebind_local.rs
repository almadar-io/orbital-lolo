//! Call-site `[local]` on a trait rebind (owner ruling 2026-09-24): riya's
//! client-held game traits are rebinds of std atoms onto a `[runtime]` entity;
//! a rebind marks itself client-only the same way an authored trait does.

use orbital_core_base::schema::TraitReference;
use orbital_lolo::{parse_and_lower_str, parse_str};

const SOURCE: &str = r#"
uses Body from "std/behaviors/std-platformer-body"

orbital Level {
  entity World [runtime, shared] {
    id : string
  }
  trait LevelBody = Body.traits.PlatformerBody -> World [local] {
    config {
      running: true
    }
  }
  trait PlainBody = Body.traits.PlatformerBody -> World {
    config {
      running: true
    }
  }
}
"#;

fn reference_local(name: &str) -> bool {
    let schema = parse_and_lower_str(SOURCE).expect("lower");
    schema.orbitals[0]
        .traits
        .iter()
        .find_map(|t| match t {
            TraitReference::Reference { name: n, local, .. } if n.as_deref() == Some(name) => Some(*local),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no reference named {name}"))
}

#[test]
fn a_rebind_marked_local_lowers_local() {
    assert!(reference_local("LevelBody"));
}

#[test]
fn control_a_plain_rebind_is_not_local() {
    assert!(!reference_local("PlainBody"));
}

#[test]
fn rebind_local_round_trips_through_format() {
    let program = parse_str(SOURCE).expect("parse");
    let printed = orbital_lolo::format::format_program(&program);
    assert!(printed.contains("-> World [local] {"), "{printed}");
    assert_eq!(parse_str(&printed).expect("reparse").orbitals[0].traits.iter().filter(|t| t.local).count(), 1);
}
