// A `listens` source naming a trait of an imported orbital (`orbital Tasks =
// Tm.orbitals.Board {}`) is not visible at L1 — the upstream's traits arrive at
// inline — so L1 defers it to the post-inline check instead of rejecting it.

use orbital_lolo::parse_and_lower_str;

fn program(source_orbital: &str) -> String {
    format!(
        r#"app Probe
uses Tm from "almadar-behaviors/std-task-manager"

orbital Tasks = Tm.orbitals.TaskBoardOrbital {{}}

orbital Local {{
  entity Thing [persistent: things] {{
    id : string!
  }}
  trait Source -> Thing {{
    initial: idle
    state idle {{
      INIT -> idle
        (render-ui main {{ type: "stack" }})
    }}
  }}
}}

orbital Watcher {{
  entity Note [persistent: notes] {{
    id : string!
  }}
  trait NoteLog -> Note {{
    initial: idle
    state idle {{
      INIT -> idle
        (render-ui main {{ type: "stack" }})
      LOG -> idle
    }}
    listens {{
      {source_orbital}.TaskPersistor.TASK_CREATED -> LOG
    }}
  }}
  page "/notes" -> NoteLog
}}"#
    )
}

#[test]
fn a_source_in_an_imported_orbital_is_deferred_to_inline() {
    let src = program("Tasks");
    assert!(
        parse_and_lower_str(&src).is_ok(),
        "{:?}",
        parse_and_lower_str(&src).err()
    );
}

#[test]
fn control_an_unknown_trait_of_a_local_orbital_is_still_rejected() {
    let err = parse_and_lower_str(&program("Local")).expect_err("Local has no TaskPersistor");
    assert!(err.to_string().contains("TaskPersistor"), "{err}");
}
