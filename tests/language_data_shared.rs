//! L1 (`orbital-lolo`) and L2 (`orbital-compiler`) read ONE registry: the
//! installed `@almadar/core`, through `orbital_core::language_data`. A pattern
//! and prop that exist only in the installed JSON are visible to both, with no
//! rebuild. Own test binary: the language data is process-wide and installed once.

use orbital_core_base::language_data::{install_language_data, load_language_data};
use std::fs;

#[test]
fn a_prop_only_in_the_installed_json_is_visible_to_l1_and_l2() {
    let tmp = tempfile::tempdir().unwrap();
    let core = tmp.path().join("node_modules/@almadar/core");
    let data = core.join("dist/patterns");
    fs::create_dir_all(&data).unwrap();
    fs::write(
        core.join("package.json"),
        r#"{"name":"@almadar/core","version":"0.0.1","orbLanguage":{"compiler":">=0.0.0",
            "patterns":"dist/patterns/patterns-registry.json",
            "componentMapping":"dist/patterns/component-mapping.json",
            "eventContracts":"dist/patterns/event-contracts.json",
            "services":"dist/patterns/services-registry.json",
            "bindingRules":"dist/language/binding-rules.json",
            "programTypes":"dist/language/program-types.json",
            "themes":"themes"}}"#,
    )
    .unwrap();
    fs::write(
        data.join("patterns-registry.json"),
        r#"{"patterns":{"zz-widget":{"type":"zz-widget","category":"x","propsSchema":{
            "zzFire":{"types":["string"],"kind":"event"},
            "zzRecord":{"types":["object"],"kind":"entity"}}}}}"#,
    )
    .unwrap();
    fs::write(
        data.join("component-mapping.json"),
        r#"{"mappings":{"zz-widget":{"component":"ZzWidget","importPath":"x","category":"x"}}}"#,
    )
    .unwrap();
    fs::write(data.join("event-contracts.json"), r#"{"contracts":{}}"#).unwrap();
    fs::write(data.join("services-registry.json"), r#"{"services":{}}"#).unwrap();
    fs::create_dir_all(core.join("dist/language")).unwrap();
    fs::write(
        core.join("dist/language/binding-rules.json"),
        r#"{"guard":{"allowed":["zzRoot"]},"effect":{"allowed":["entity"]},"tick":{"allowed":[]}}"#,
    )
    .unwrap();
    fs::write(core.join("dist/language/program-types.json"), "{}").unwrap();
    fs::create_dir_all(core.join("themes")).unwrap();
    fs::write(core.join("themes/zz.json"), r#"{"name":"zz","tokens":{}}"#).unwrap();

    let loaded = load_language_data(tmp.path(), &tmp.path().join("store")).unwrap();
    install_language_data(loaded).unwrap();

    assert!(orbital_lolo::patterns_meta::event_prop_names().contains("zzFire"));
    assert_eq!(
        orbital_lolo::patterns_meta::record_prop("zz-widget", "zzRecord"),
        Some(true)
    );
    assert!(orbital_compiler::embedded::patterns().contains_key("zz-widget"));
    assert!(orbital_compiler::embedded::patterns()["zz-widget"]
        .props_schema
        .contains_key("zzFire"));
    assert!(orbital_compiler::pattern_registry::get_pattern_definition("zz-widget").is_some());
    assert!(orbital_compiler::pattern_registry::is_registered_pattern(
        "zz-widget"
    ));
    assert!(
        !orbital_lolo::patterns_meta::event_prop_names().contains("action"),
        "control: the baked/real registry is NOT in play"
    );
}
