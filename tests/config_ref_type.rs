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
//! The check used to be skipped under a dev-mode flag; that carve-out is
//! gone, the check now always runs. Resolves the real installed
//! `@almadar/std` package (Phase 2: no embedded fallback) so `title`'s
//! declared type on `ModalRecordModal` is checked against the real contract
//! shape.

use orbital_lolo::{parse_and_lower_str_with, LowerOptions};
use std::path::Path;

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
    let pkg = orbital_test_support::installed_package(
        "almadar-std",
        Path::new(env!("CARGO_MANIFEST_DIR")),
    );
    let options = LowerOptions {
        packages: vec![pkg],
    };

    let mismatched = parse_and_lower_str_with(MISMATCHED_REF, &options);
    let matched = parse_and_lower_str_with(MATCHED_REF, &options);

    let err =
        mismatched.expect_err("a number-typed knob wired to a string-typed prop must be rejected");
    let msg = err.to_string();
    assert!(
        msg.contains("title"),
        "error names the offending config key: {msg}"
    );

    matched.expect("a string-typed knob on a string-typed prop is legal");
}
