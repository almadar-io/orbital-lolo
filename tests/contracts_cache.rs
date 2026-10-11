//! A lowering reads the behavior contracts of every installed package. Live
//! behavior packages make one `orb resolve` lower dozens of imported `.lolo`
//! files, so the contracts are loaded once per process per set of files, and
//! re-read when a contracts file changes.

use orbital_core::behavior_packages::discover;
use orbital_lolo::registry::LoloContracts;
use std::sync::Arc;

#[test]
fn unchanged_contracts_files_load_once_and_an_edit_is_re_read() {
    let tmp = tempfile::tempdir().unwrap();
    let pkg =
        orbital_test_support::fake_package(tmp.path(), "@almadar/std", "almadar-std", "1.0.0", &[]);
    let packages = discover(tmp.path(), &tmp.path().join("store")).unwrap();
    let first = LoloContracts::load_shared(&packages).unwrap();
    let again = LoloContracts::load_shared(&packages).unwrap();
    assert!(
        Arc::ptr_eq(&first, &again),
        "unchanged files are not parsed again"
    );

    std::fs::write(
        pkg.join("behaviors/lolo-behavior-contracts.json"),
        r#"{"behaviors":{"zz-new":{"traits":{}}}}"#,
    )
    .unwrap();
    let edited = LoloContracts::load_shared(&packages).unwrap();
    assert!(
        !Arc::ptr_eq(&first, &edited),
        "control: an edited contracts file is re-read"
    );
    assert!(edited.get_behavior("zz-new").unwrap().is_some());
}
