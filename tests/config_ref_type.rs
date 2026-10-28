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

  config {
    headingWidth : number = 320
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

  config {
    headingText : string = "Hello"
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
        ..Default::default()
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

const WIRED: &str = r#"
orbital WiringOrbital {
  uses Modal from "std/behaviors/std-modal"

  entity Item [persistent: items] {
    id : string!
  }

  config {
    headingText : string = "Hello"
  }

  trait ItemModal = Modal.traits.ModalRecordModal -> Item {
    config {
      title: @config.headingText
      size: lg
    }
  }
}
"#;

fn modal_config(source: &str) -> serde_json::Value {
    let schema = orbital_lolo::parse_and_lower_str(source).expect("lowers");
    let json = serde_json::to_value(&schema).unwrap();
    json["orbitals"][0]["traits"][0]["config"].clone()
}

#[test]
fn a_wired_config_entry_carries_the_source_knobs_declared_type() {
    let config = modal_config(WIRED);
    assert_eq!(config["title"]["type"], "string", "{config}");
    assert_eq!(config["title"]["wired"], true, "{config}");
    assert_eq!(
        config["title"]["default"], "@config.headingText",
        "{config}"
    );
    assert_eq!(
        config["size"]["type"], "unknown",
        "control: a literal value has no source knob: {config}"
    );
    assert_eq!(config["size"]["wired"], true, "{config}");
}

#[test]
fn a_wiring_from_an_undeclared_knob_is_a_lowering_error() {
    let err = orbital_lolo::parse_and_lower_str(
        &WIRED.replace("@config.headingText", "@config.headingTxt"),
    )
    .expect_err("a typo'd source knob must not lower");
    let message = err.to_string();
    assert!(
        message.contains("headingTxt") && message.contains("declares no config knob"),
        "{message}"
    );
}

#[test]
fn an_app_level_knob_is_a_source_too_and_a_wired_entry_unlowers_as_plain_wiring() {
    let source = WIRED.replace(
        "  config {\n    headingText : string = \"Hello\"\n  }\n",
        "",
    );
    let app = format!("app wiring [version: \"1.0.0\"]\nconfig {{\n  headingText : string = \"Hello\"\n}}\n{source}");
    let config = modal_config(&app);
    assert_eq!(config["title"]["type"], "string", "{config}");
    let schema = orbital_lolo::parse_and_lower_str(&app).unwrap();
    let printed = orbital_lolo::schema_to_lolo(&schema).unwrap();
    assert!(printed.contains("title: @config.headingText"), "{printed}");
    let again = orbital_lolo::parse_and_lower_str(&printed).expect("relowers");
    assert_eq!(
        serde_json::to_value(&again).unwrap()["orbitals"][0]["traits"][0]["config"]["title"]
            ["type"],
        "string"
    );
}
