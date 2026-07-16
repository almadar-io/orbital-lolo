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

    std::env::set_var("ALMADAR_DEV", "1");
    let lowered = parse_and_lower_str(UNKNOWN_MEMBER_LOLO);

    match prev {
        Some(v) => std::env::set_var("ALMADAR_DEV", v),
        None => std::env::remove_var("ALMADAR_DEV"),
    }

    lowered.expect("under ALMADAR_DEV the stale-slice member check is skipped");
}
