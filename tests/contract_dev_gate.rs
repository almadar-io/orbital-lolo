//! C-LOLO-CONTRACT-SLICE-STALE-DEV (closed): the parse-time `uses`-import
//! contract check reads the loaded behavior-contract ladder
//! (`registry::LoloContracts`). It used to degrade an unknown-member error to
//! a skip under a dev-mode flag because the baked slice could lag the live
//! registries; that carve-out is gone — the check is now a hard error
//! unconditionally, in every mode. Resolves the real installed `@almadar/std`
//! package (Phase 2: no embedded fallback) so the trait/config names below
//! (`ModalRecordModal`, its `title` config key) are checked against the real
//! contract shape.

use orbital_lolo::{parse_and_lower_str_with, LowerOptions};
use std::path::Path;

const UNKNOWN_MEMBER_LOLO: &str = r#"
orbital DevGateOrbital {
  uses Modal from "std/behaviors/std-modal"

  entity Item [persistent: items] {
    id : string!
    name : string
  }

  trait ItemModal = Modal.traits.TotallyNotARealTrait -> Item {
  }
}
"#;

fn std_options() -> LowerOptions {
    let pkg = orbital_test_support::installed_package(
        "almadar-std",
        Path::new(env!("CARGO_MANIFEST_DIR")),
    );
    LowerOptions {
        packages: vec![pkg],
        ..Default::default()
    }
}

#[test]
fn unknown_trait_ref_always_errors() {
    let options = std_options();

    let err = parse_and_lower_str_with(UNKNOWN_MEMBER_LOLO, &options)
        .expect_err("unknown trait member on a known behavior must error");
    let msg = err.to_string();
    assert!(
        msg.contains("TotallyNotARealTrait"),
        "error names the unknown member: {msg}"
    );

    // Check 3.4 (typed config overrides) reads the same contracts ladder, so
    // it must error just as unconditionally.
    let cfg_err = parse_and_lower_str_with(CONFIG_TYPE_MISMATCH_LOLO, &options)
        .expect_err("a number on a string-typed knob must error");
    assert!(
        cfg_err.to_string().contains("title"),
        "error names the mismatched config key: {cfg_err}"
    );
}

/// Check 3.4 (typed config overrides) reads the same contracts ladder as the
/// unknown-member checks above.
const CONFIG_TYPE_MISMATCH_LOLO: &str = r#"
orbital DevGateConfigOrbital {
  uses Modal from "std/behaviors/std-modal"

  entity Item [persistent: items] {
    id : string!
    name : string
  }

  trait ItemModal = Modal.traits.ModalRecordModal -> Item {
    config {
      title: 42
    }
  }
}
"#;
