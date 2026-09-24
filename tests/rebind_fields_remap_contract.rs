//! G-ORB-COMPILER-002 (§113): the L1 rebind row-constraint check must resolve
//! each field the trait requires THROUGH the rebind's `fields { … }` remap —
//! the remedy its own error message recommends — before testing the target
//! entity. `std-learn-transport`'s `LearnTransportSim` requires `done`.

use orbital_lolo::{parse_and_lower_str_with, LowerOptions};
use std::path::Path;

fn lolo(remap: &str) -> String {
    format!(
        r#"
orbital SimOrbital {{
  uses Transport from "std/behaviors/std-learn-transport"

  entity Scene [runtime, shared] {{
    id : string!
    active : boolean = false
    title : string = ""
    running : boolean = false
    steps : number = 0
    elapsedMs : number = 0
    finished : boolean = false
  }}

  trait Sim = Transport.traits.LearnTransportSim -> Scene {{
{remap}
  }}
}}
"#
    )
}

fn std_options() -> LowerOptions {
    let pkg = orbital_test_support::installed_package(
        "almadar-std",
        Path::new(env!("CARGO_MANIFEST_DIR")),
    );
    LowerOptions {
        packages: vec![pkg],
    }
}

#[test]
fn remapped_required_field_satisfies_the_row_constraint() {
    let src = lolo("    fields {\n      done: finished\n    }");
    if let Err(e) = parse_and_lower_str_with(&src, &std_options()) {
        panic!("a `fields` remap onto a declared field must satisfy the contract: {e}");
    }
}

#[test]
fn unremapped_missing_field_still_errors() {
    let err = match parse_and_lower_str_with(&lolo(""), &std_options()) {
        Ok(_) => panic!("expected a missing-field error"),
        Err(e) => e.to_string(),
    };
    assert!(err.contains("\"done\""), "{err}");
}

#[test]
fn remap_onto_an_undeclared_field_still_errors() {
    let src = lolo("    fields {\n      done: nowhere\n    }");
    let err = match parse_and_lower_str_with(&src, &std_options()) {
        Ok(_) => panic!("a remap target the entity does not declare must still fail"),
        Err(e) => e.to_string(),
    };
    assert!(err.contains("\"done\""), "{err}");
}
