//! A lowering reads the behavior contracts of every installed package. Live
//! behavior packages make one `orb resolve` lower dozens of imported `.lolo`
//! files, so the contracts are loaded once per process per set of packages, and
//! an edited `.lolo` is seen on the next lookup.

use orbital_core_base::behavior_packages::discover;
use orbital_lolo::registry::LoloContracts;
use std::sync::Arc;

const LOLO: &str = "app zz-new [version: \"1.0.0\", description: \"x\"]\norbital ZzOrbital {\n  entity ZzView [runtime] {\n    id : string!\n  }\n  trait ZzBar -> @rebindable ZzView {\n    initial: idle\n    state idle {\n      INIT -> idle\n        (render-ui main { type: typography, content: \"x\" })\n    }\n  }\n  page \"/zz\" as ZzPage -> ZzBar\n}\n";

#[test]
fn unchanged_packages_load_once_and_a_new_lolo_is_seen() {
    let tmp = tempfile::tempdir().unwrap();
    let pkg = orbital_test_support::fake_package(tmp.path(), "@almadar/std", "almadar-std", "1.0.0", &[]);
    orbital_test_support::link_installed_core(tmp.path(), std::path::Path::new(env!("CARGO_MANIFEST_DIR")));
    let packages = discover(tmp.path(), &tmp.path().join("store")).unwrap();
    let first = LoloContracts::load_shared(&packages).unwrap();
    let again = LoloContracts::load_shared(&packages).unwrap();
    assert!(Arc::ptr_eq(&first, &again), "an unchanged package set is not loaded again");
    assert!(first.get_behavior("zz-new").unwrap().is_none(), "control: not yet authored");

    std::fs::write(pkg.join("behaviors/lolo/zz-new.lolo"), LOLO).unwrap();
    let behavior = first.get_behavior("zz-new").unwrap().expect("a new .lolo is derived on demand");
    assert!(behavior.has_trait("ZzBar"));
}
