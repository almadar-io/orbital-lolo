//! Behavior-package contract resolution (ruling 3, `Almadar_Behavior_Packages.md`
//! §5): the loader finds a `uses`-imported behavior's parse-time contract in
//! an INSTALLED package's `lolo_contracts` file — `@almadar/std` included, no
//! embedded fallback (Phase 2).
//!
//! No chdir, no env: every case builds a `BehaviorPackage` fixture pointing at
//! a temp dir and passes it through `LowerOptions`, so filesystem discovery
//! (`orbital_core::behavior_packages::discover`) is never exercised here —
//! that's Wave 1's `orbital-core` test suite.

use orbital_core::behavior_packages::BehaviorPackage;
use orbital_lolo::{parse_and_lower_str_with, LoloError, LowerError, LowerOptions};
use std::fs;
use std::path::Path;
use tempfile::TempDir;

/// Writes a minimal installed-package layout (`package.json` + the `orb`
/// block's declared sidecar files) under `tmp`, and returns the
/// `BehaviorPackage` L1 resolves against — bypassing `discover()` entirely,
/// per this file's no-chdir/no-env rule.
fn fake_package(
    tmp: &Path,
    name: &str,
    prefix: &str,
    version: &str,
    contracts_json: &str,
) -> BehaviorPackage {
    let root = tmp.join(name.replace(['/', '@'], "_"));
    let registry = root.join("behaviors/registry");
    fs::create_dir_all(&registry).expect("mkdir registry");

    let index = root.join("behaviors/behaviors-registry.json");
    fs::write(&index, "{}").expect("write index");

    let factory_signatures = registry.join("factory-signatures.json");
    fs::write(&factory_signatures, "{}").expect("write factory signatures");

    let lolo_contracts = root.join("behaviors/lolo-behavior-contracts.json");
    fs::write(&lolo_contracts, contracts_json).expect("write lolo contracts");

    let signatures = root.join("behaviors/signatures.lolo");
    fs::write(&signatures, "").expect("write signatures");

    let package_json = serde_json::json!({
        "name": name,
        "version": version,
        "orb": {
            "prefix": prefix,
            "registry": "behaviors/registry",
            "index": "behaviors/behaviors-registry.json",
            "factorySignatures": "behaviors/registry/factory-signatures.json",
            "loloContracts": "behaviors/lolo-behavior-contracts.json",
            "signatures": "behaviors/signatures.lolo",
            "compiler": ">=0.0.0",
        },
    });
    fs::write(
        root.join("package.json"),
        serde_json::to_string_pretty(&package_json).unwrap(),
    )
    .expect("write package.json");

    BehaviorPackage {
        name: name.to_string(),
        version: version.to_string(),
        prefix: prefix.to_string(),
        root,
        registry,
        index,
        factory_signatures,
        lolo_contracts,
        signatures,
        embeddings: Vec::new(),
        compiler_range: ">=0.0.0".to_string(),
        std_peer_range: None,
        core_range: None,
        is_link: false,
    }
}

/// A `.lolo` orbital that `uses` one behavior and references `Alias.traits.<member>`.
fn using_lolo(from: &str, member: &str) -> String {
    format!(
        r#"
orbital FixtureOrbital {{
  uses Widget from "{from}"

  entity Item [persistent: items] {{
    id : string!
  }}

  trait ItemWidget = Widget.traits.{member} -> Item {{
  }}
}}
"#
    )
}

/// A minimal installed `@almadar/std` package with no declared behaviors.
/// Every lower now requires SOME installed std (Phase 2, ruling 1 —
/// `LoloContracts::load` errors otherwise) even when a test's source only
/// exercises a non-std package's contracts.
fn minimal_std_package(tmp: &Path) -> BehaviorPackage {
    fake_package(
        tmp,
        "@almadar/std",
        "almadar-std",
        "0.0.1",
        r#"{"behaviors":{}}"#,
    )
}

fn lower_error(source: &str, options: &LowerOptions) -> LowerError {
    match parse_and_lower_str_with(source, options).expect_err("must fail to lower") {
        LoloError::Lower { inner, .. } => inner,
        other => panic!("expected a LowerError, got {other:?}"),
    }
}

#[test]
fn behavior_key_from_path_strips_the_prefix_segment() {
    assert_eq!(
        orbital_lolo::registry::behavior_key_from_path("almadar-behaviors/fake-test-widget"),
        "fake-test-widget"
    );
}

#[test]
fn a_behavior_only_in_an_installed_io_package_is_found_and_validates() {
    let tmp = TempDir::new().unwrap();
    let contracts = serde_json::json!({
        "behaviors": {
            "fake-test-widget": {
                "traits": { "Basic": { "requires": [], "config": {} } },
                "pages": [],
                "orbitals": {},
                "appConfig": {},
            }
        }
    })
    .to_string();
    let pkg = fake_package(
        tmp.path(),
        "@almadar-io/behaviors",
        "almadar-behaviors",
        "0.0.1",
        &contracts,
    );
    let options = LowerOptions {
        packages: vec![minimal_std_package(tmp.path()), pkg],
    };

    let source = using_lolo("almadar-behaviors/fake-test-widget", "Basic");
    parse_and_lower_str_with(&source, &options)
        .expect("Basic is declared on fake-test-widget in the installed package's contracts");
}

#[test]
fn an_unknown_member_on_a_live_package_behavior_is_a_hard_error() {
    let tmp = TempDir::new().unwrap();
    let contracts = serde_json::json!({
        "behaviors": {
            "fake-test-widget": {
                "traits": { "Basic": { "requires": [], "config": {} } },
                "pages": [],
                "orbitals": {},
                "appConfig": {},
            }
        }
    })
    .to_string();
    let pkg = fake_package(
        tmp.path(),
        "@almadar-io/behaviors",
        "almadar-behaviors",
        "0.0.1",
        &contracts,
    );
    let options = LowerOptions {
        packages: vec![minimal_std_package(tmp.path()), pkg],
    };

    let source = using_lolo("almadar-behaviors/fake-test-widget", "NotReal");
    let err = lower_error(&source, &options);
    assert!(
        matches!(&err, LowerError::UnknownTraitRef { member, .. } if member == "NotReal"),
        "expected UnknownTraitRef naming NotReal, got {err}"
    );
}

#[test]
fn the_same_behavior_name_in_two_non_std_packages_is_ambiguous() {
    let tmp = TempDir::new().unwrap();
    let contracts = serde_json::json!({
        "behaviors": {
            "shared-widget": {
                "traits": { "Basic": { "requires": [], "config": {} } },
                "pages": [],
                "orbitals": {},
                "appConfig": {},
            }
        }
    })
    .to_string();
    let pkg_a = fake_package(tmp.path(), "@fixture/pkg-a", "pkg-a", "1.0.0", &contracts);
    let pkg_b = fake_package(tmp.path(), "@fixture/pkg-b", "pkg-b", "1.0.0", &contracts);
    let options = LowerOptions {
        packages: vec![minimal_std_package(tmp.path()), pkg_a, pkg_b],
    };

    // L1 resolves by bare behavior name (`behavior_key_from_path`), so which
    // package's prefix the `uses` path names is irrelevant to the ambiguity —
    // both packages declare `shared-widget`.
    let source = using_lolo("pkg-a/shared-widget", "Basic");
    let err = lower_error(&source, &options);
    assert!(
        matches!(&err, LowerError::AmbiguousBehavior { name, .. } if name == "shared-widget"),
        "expected AmbiguousBehavior naming shared-widget, got {err}"
    );
}

#[test]
fn an_installed_std_package_is_authoritative_with_no_fallback() {
    let tmp = TempDir::new().unwrap();
    // std-modal IS declared here (so the lookup doesn't skip as "unknown
    // import"), but WITHOUT `ModalRecordModal`. There is no embedded slice to
    // fall back to (Phase 2) — a miss on the installed std is simply a miss.
    let contracts = serde_json::json!({
        "behaviors": {
            "std-modal": {
                "traits": {},
                "pages": [],
                "orbitals": {},
                "appConfig": {},
            }
        }
    })
    .to_string();
    let pkg = fake_package(
        tmp.path(),
        "@almadar/std",
        "almadar-std",
        "0.0.1",
        &contracts,
    );
    let options = LowerOptions {
        packages: vec![pkg],
    };

    let source = using_lolo("std/behaviors/std-modal", "ModalRecordModal");
    let err = lower_error(&source, &options);
    assert!(
        matches!(&err, LowerError::UnknownTraitRef { member, behavior, .. }
            if member == "ModalRecordModal" && behavior == "std-modal"),
        "expected UnknownTraitRef against the INSTALLED std-modal, got {err}"
    );
}

#[test]
fn no_installed_std_package_reports_the_install_hint() {
    // `LoloContracts::load` requires an `almadar-std`-prefixed package on
    // EVERY lower (Phase 2, ruling 1) — a completely empty package list
    // fails before any `uses`-import check even runs.
    let source = using_lolo("std/behaviors/std-modal", "ModalRecordModal");
    let options = LowerOptions { packages: vec![] };
    let err = lower_error(&source, &options);
    assert!(
        matches!(&err, LowerError::BehaviorPackageNotInstalled { package }
            if package == "@almadar/std"),
        "expected the structured not-installed variant naming @almadar/std, got {err}"
    );
    let message = err.to_string();
    assert!(
        message.contains("@almadar/std is not installed")
            && message.contains("orb behaviors install @almadar/std")
            && message.contains("--global"),
        "expected the not-installed hint naming both install commands, got {message}"
    );
}
