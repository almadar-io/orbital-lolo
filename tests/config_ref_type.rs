//! A `@config.X` binding on a trait override is type-checked against the knob
//! it names.
//!
//! Before `config_ref_knob_type`, `literal_value_type` returned `None` for every
//! `@`-string, so check 3.4's `if let (Some(expected), Some(got))` destructure
//! failed and the check silently no-opped on EVERY reference. A number-typed
//! knob wired to a string-typed prop validated clean, and the mismatch only
//! surfaced downstream when something resolved the reference — which is how
//! `minTileWidth : number = 320` reached a `string` prop and stayed there.
//!
//! Its own test binary (separate process) so the `ALMADAR_DEV` manipulation
//! cannot race `contract_dev_gate.rs`, which toggles the same process-global var.

use orbital_lolo::parse_and_lower_str;

/// `title` on ModalRecordModal is declared `string`; the knob it is wired to is
/// `number`. Decidably wrong at the call site, in one file.
const MISMATCHED_REF: &str = r#"
orbital ConfigRefOrbital {
  uses Modal from "std/behaviors/std-modal"

  entity Item [persistent: items] {
    id : string!
    name : string
  }

  trait ItemModal = Modal.traits.ModalRecordModal -> Item {
    config {
      title: @config.headingWidth
    }
  }

  trait Knobs -> Item {
    config {
      headingWidth : number = 320
    }
  }
}
"#;

/// Same shape, but the knob is a string — the legal form, and the control that
/// proves the check is discriminating rather than rejecting all references.
const MATCHED_REF: &str = r#"
orbital ConfigRefOrbital {
  uses Modal from "std/behaviors/std-modal"

  entity Item [persistent: items] {
    id : string!
    name : string
  }

  trait ItemModal = Modal.traits.ModalRecordModal -> Item {
    config {
      title: @config.headingText
    }
  }

  trait Knobs -> Item {
    config {
      headingText : string = "Hello"
    }
  }
}
"#;

#[test]
fn config_reference_is_typechecked_against_the_knob_it_names() {
    // Check 3.4 is skipped under ALMADAR_DEV (the baked contracts slice lags the
    // live registries), so this must assert with the gate off.
    let prev = std::env::var("ALMADAR_DEV").ok();
    std::env::remove_var("ALMADAR_DEV");

    let mismatched = parse_and_lower_str(MISMATCHED_REF);
    let matched = parse_and_lower_str(MATCHED_REF);

    match prev {
        Some(v) => std::env::set_var("ALMADAR_DEV", v),
        None => std::env::remove_var("ALMADAR_DEV"),
    }

    let err = mismatched
        .expect_err("a number-typed knob wired to a string-typed prop must be rejected");
    let msg = err.to_string();
    assert!(
        msg.contains("title"),
        "error names the offending config key: {msg}"
    );

    matched.expect("a string-typed knob on a string-typed prop is legal");
}
