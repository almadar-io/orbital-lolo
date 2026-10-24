//! Call-site `[client]` / `[optimistic]` on a trait rebind: riya's client-held game
//! traits are rebinds of std atoms onto a `[runtime]` entity; a rebind declares its
//! dispatch mode the same way an authored trait does. The retired `[local]` is a
//! lowering error naming `[client]`.

use orbital_core_base::schema::{DispatchMode, TraitReference};
use orbital_lolo::{parse_and_lower_str, parse_str};

const SOURCE: &str = r#"
uses Body from "std/behaviors/std-platformer-body"

orbital Level {
  entity World [runtime, shared] {
    id : string
  }
  trait LevelBody = Body.traits.PlatformerBody -> World [client] {
    config {
      running: true
    }
  }
  trait PlainBody = Body.traits.PlatformerBody -> World {
    config {
      running: true
    }
  }
  trait HopeBody = Body.traits.PlatformerBody -> World [optimistic] {
    config {
      running: true
    }
  }
  trait HostBody = Body.traits.PlatformerBody -> World [server] {
    config {
      running: true
    }
  }
}
"#;

fn reference_dispatch(name: &str) -> Option<DispatchMode> {
    let schema = parse_and_lower_str(SOURCE).expect("lower");
    schema.orbitals[0]
        .traits
        .iter()
        .find_map(|t| match t {
            TraitReference::Reference { name: n, dispatch, .. } if n.as_deref() == Some(name) => Some(dispatch.clone()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no reference named {name}"))
}

#[test]
fn a_rebind_marked_client_lowers_client() {
    assert_eq!(reference_dispatch("LevelBody"), Some(DispatchMode::Client));
}

#[test]
fn a_rebind_marked_optimistic_lowers_optimistic() {
    assert_eq!(reference_dispatch("HopeBody"), Some(DispatchMode::Optimistic));
}

#[test]
fn a_rebind_marked_server_lowers_the_explicit_server_mode() {
    assert_eq!(reference_dispatch("HostBody"), Some(DispatchMode::Server));
}

#[test]
fn control_a_plain_rebind_declares_no_dispatch() {
    assert_eq!(reference_dispatch("PlainBody"), None);
}

#[test]
fn the_retired_local_rebind_word_errors_with_the_client_fix_it() {
    let err = parse_and_lower_str(&SOURCE.replace("World [client]", "World [local]"))
        .unwrap_err()
        .to_string();
    assert!(err.contains("ORB_T_HEADER_WORD_RETIRED") && err.contains("write `[client]`"), "{err}");
}

#[test]
fn rebind_dispatch_round_trips_through_format() {
    let program = parse_str(SOURCE).expect("parse");
    let printed = orbital_lolo::format::format_program(&program);
    assert!(printed.contains("-> World [client] {"), "{printed}");
    assert!(printed.contains("-> World [optimistic] {"), "{printed}");
    assert!(printed.contains("-> World [server] {"), "{printed}");
    let reparsed = parse_str(&printed).expect("reparse");
    let modes: Vec<_> = reparsed.orbitals[0].traits.iter().map(|t| t.dispatch).collect();
    assert_eq!(
        modes,
        vec![
            Some(orbital_lolo::ast::TraitDispatch::Client),
            None,
            Some(orbital_lolo::ast::TraitDispatch::Optimistic),
            Some(orbital_lolo::ast::TraitDispatch::Server)
        ]
    );
}
