//! A live behavior package (`orb.source`, its `.lolo` lowered on read) publishes
//! its L1 contracts from those `.lolo` files, not from the generated
//! `lolo-behavior-contracts.json`: a trait added to one `.lolo` is composable
//! from another with no regeneration step. A materialized package declaring
//! `orb.source` takes the same path; the sidecar is ignored.

use orbital_core_base::behavior_packages::{BehaviorPackage, RegistrySource};
use orbital_lolo::{parse_and_lower_str_with, LowerOptions};
use std::fs;
use std::path::Path;

/// `zz-chrome`, an atom whose trait `trait_name` renders a bar.
fn chrome(trait_name: &str, config: &str) -> String {
    let content = if config.is_empty() { "\"bar\"" } else { "@config.title" };
    format!(
        r#"app zz-chrome [version: "1.0.0", description: "chrome"]
orbital ChromeOrbital {{
  entity ChromeView [runtime] {{
    id : string!
  }}

  trait {trait_name} -> @rebindable ChromeView [interaction, instance] {{
    initial: idle

    state idle {{
      INIT -> idle
        (render-ui main {{ type: typography, content: {content} }})
    }}
{config}
  }}

  page "/chrome" as ChromePage -> {trait_name}
}}
"#
    )
}

fn consumer(member: &str) -> String {
    format!(
        r#"
orbital BuilderOrbital {{
  uses Chrome from "zz/zz-chrome"

  entity Item [runtime] {{
    id : string!
  }}

  trait TopBar = Chrome.traits.{member} -> Item {{
  }}
}}
"#
    )
}

/// The `zz` package: `zz-chrome.lolo` under its source tree and a contracts
/// sidecar that knows nothing of it (stale, as before any regeneration).
fn package(tmp: &Path, live: bool, lolo: &str) -> BehaviorPackage {
    let root = tmp.join("zz");
    let source = root.join("behaviors/lolo/atoms");
    fs::create_dir_all(&source).unwrap();
    fs::write(source.join("zz-chrome.lolo"), lolo).unwrap();
    let contracts = root.join("behaviors/lolo-behavior-contracts.json");
    fs::write(&contracts, r#"{"behaviors":{"zz-chrome":{"traits":{"OldBar":{"requires":[],"config":{}}}}}}"#).unwrap();
    BehaviorPackage {
        name: "@zz/behaviors".into(),
        version: "1.0.0".into(),
        prefix: "zz".into(),
        root: root.clone(),
        registry: if live { RegistrySource::Live(root.join("behaviors/lolo")) } else { RegistrySource::Materialized(root.join("behaviors/registry")) },
        files: Vec::new(),
        lolo_source: Some(root.join("behaviors/lolo")),
        vectors: None,
        sidecars: Vec::new(),
        compiler_range: ">=0.0.0".into(),
        std_peer_range: None,
        core_range: None,
        is_link: false,
    }
}

fn options(pkg: BehaviorPackage) -> LowerOptions {
    let std = orbital_test_support::installed_package("almadar-std", Path::new(env!("CARGO_MANIFEST_DIR")));
    LowerOptions { packages: vec![std, pkg] }
}

#[test]
fn a_trait_added_to_a_live_lolo_is_composable_with_the_sidecar_untouched() {
    let tmp = tempfile::tempdir().unwrap();
    let opts = options(package(tmp.path(), true, &chrome("NewBar", "")));
    let sidecar = fs::read_to_string(&opts.packages[1].root.join("behaviors/lolo-behavior-contracts.json")).unwrap();
    parse_and_lower_str_with(&consumer("NewBar"), &opts).expect("the live trait resolves");
    assert_eq!(fs::read_to_string(&opts.packages[1].root.join("behaviors/lolo-behavior-contracts.json")).unwrap(), sidecar, "the sidecar is never written");
}

#[test]
fn a_materialized_package_with_a_source_ignores_a_stale_contracts_sidecar() {
    let tmp = tempfile::tempdir().unwrap();
    let opts = options(package(tmp.path(), false, &chrome("NewBar", "")));
    parse_and_lower_str_with(&consumer("NewBar"), &opts).expect("the .lolo trait resolves");
    assert!(parse_and_lower_str_with(&consumer("OldBar"), &opts).is_err(), "the sidecar's trait is not consulted");
}

#[test]
fn control_an_unknown_trait_still_errors_on_a_live_package() {
    let tmp = tempfile::tempdir().unwrap();
    let opts = options(package(tmp.path(), true, &chrome("NewBar", "")));
    let err = parse_and_lower_str_with(&consumer("OldBar"), &opts).expect_err("OldBar is only in the stale sidecar");
    assert!(err.to_string().contains("OldBar"), "{err}");
}

#[test]
fn editing_the_live_lolo_is_seen_on_the_next_lower() {
    let tmp = tempfile::tempdir().unwrap();
    let opts = options(package(tmp.path(), true, &chrome("NewBar", "")));
    parse_and_lower_str_with(&consumer("NewBar"), &opts).expect("before the edit");
    fs::write(tmp.path().join("zz/behaviors/lolo/atoms/zz-chrome.lolo"), chrome("RenamedBar", "")).unwrap();
    parse_and_lower_str_with(&consumer("RenamedBar"), &opts).expect("the renamed trait resolves after the edit");
    assert!(parse_and_lower_str_with(&consumer("NewBar"), &opts).is_err(), "the old name is gone after the edit");
}

#[test]
fn edge_a_live_traits_config_types_are_checked() {
    let tmp = tempfile::tempdir().unwrap();
    let opts = options(package(tmp.path(), true, &chrome("NewBar", "\n    config {\n      title : string = \"\"\n    }\n")));
    let bad = consumer("NewBar").replace("-> Item {\n  }", "-> Item {\n    config {\n      title: 42\n    }\n  }");
    let err = parse_and_lower_str_with(&bad, &opts).expect_err("a number on a string knob");
    assert!(err.to_string().contains("title"), "{err}");
}

