//! C-LOLO-CONTRACT-SLICE-STALE-DEV: the parse-time `uses`-import contract
//! check reads the baked `lolo-behavior-contracts.json` slice, which can lag
//! the live registries mid-dev-loop (a freshly authored trait is absent from
//! it). Under `ALMADAR_DEV` an unknown member on a KNOWN behavior degrades
//! from a hard `LowerError` to a skip — the compiler's post-inline validation
//! reads the live registries in dev and stays authoritative.
//!
//! Both directions live in ONE test because the gate is a process-global env
//! var — splitting them would race under the parallel test runner.

use orbital_lolo::parse_and_lower_str;

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

#[test]
fn unknown_trait_ref_errors_without_dev_and_skips_with_dev() {
    let prev = std::env::var("ALMADAR_DEV").ok();

    std::env::remove_var("ALMADAR_DEV");
    let err = parse_and_lower_str(UNKNOWN_MEMBER_LOLO)
        .expect_err("unknown trait member on a known behavior must error without ALMADAR_DEV");
    let msg = err.to_string();
    assert!(
        msg.contains("TotallyNotARealTrait"),
        "error names the unknown member: {msg}"
    );

    // Check 3.4 (typed config overrides) reads the SAME baked slice, so it has
    // the same staleness window: changing a knob's type in the live workspace
    // made every consumer fail `orb emit orb` until the next publish + rebake.
    // It was the only one of the three checks not gated.
    let cfg_err = parse_and_lower_str(CONFIG_TYPE_MISMATCH_LOLO)
        .expect_err("a number on a string-typed knob must error without ALMADAR_DEV");
    assert!(
        cfg_err.to_string().contains("title"),
        "error names the mismatched config key: {cfg_err}"
    );

    std::env::set_var("ALMADAR_DEV", "1");
    let lowered = parse_and_lower_str(UNKNOWN_MEMBER_LOLO);
    let lowered_cfg = parse_and_lower_str(CONFIG_TYPE_MISMATCH_LOLO);

    match prev {
        Some(v) => std::env::set_var("ALMADAR_DEV", v),
        None => std::env::remove_var("ALMADAR_DEV"),
    }

    lowered.expect("under ALMADAR_DEV the stale-slice member check is skipped");
    // Safe to skip because the compiler re-checks this post-inline against the
    // LIVE registries: the inliner folds the override onto the base trait's
    // declared ConfigField keeping `field_type` (phases/inline/trait.rs), and
    // validation/config_type.rs then raises ORB_T_CONFIG_SHAPE_MISMATCH.
    lowered_cfg.expect("under ALMADAR_DEV the stale-slice config-type check is skipped");
}

/// Check 3.4 (typed config overrides) reads the SAME baked slice as the
/// unknown-member checks above, so it has the same staleness window: changing a
/// knob's type in the live workspace made every consumer fail `orb emit orb`
/// until the next publish + rebake. It was the only one of the three not gated,
/// which is the inconsistency this pins.
///
/// Safe to skip in dev because the compiler re-checks it post-inline against the
/// LIVE registries — the inliner folds the override onto the base trait's
/// declared `ConfigField` (`phases/inline/trait.rs`) keeping `field_type`, and
/// `validation/config_type.rs` then raises `ORB_T_CONFIG_SHAPE_MISMATCH`.
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

// NOTE: intentionally NOT a second `#[test]`. `ALMADAR_DEV` is process-global and
// cargo runs tests in a file on parallel threads, so a second function toggling
// it would race the one above — exactly what this file's header warns about.
// The assertions are folded into that single test instead.
