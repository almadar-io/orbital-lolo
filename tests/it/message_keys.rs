//! `locales [en, ar, sl]` declares the locales a program ships, and every
//! `(i18n/t "key")` it writes is qualified with the program's own name at
//! lowering, so catalogs from many behaviors merge without collisions.

use orbital_lolo::{format_program, parse_and_lower_str, parse_str};

const HOME: &str = r#"app std-almadar-home [version: "1.0.0", description: "Home", locales: [en, ar, sl]]

orbital Home {
  entity HomeState [runtime] {
    id : string!
  }

  trait HomeView -> HomeState {
    initial: idle

    state idle {
      INIT -> idle
        (render-ui main { type: typography, content: (i18n/t "hero.title") })
    }
  }

  page "/" as HomePage -> HomeView
}
"#;

#[test]
fn the_locales_header_lowers_to_schema_locales() {
    let schema = parse_and_lower_str(HOME).expect("lower");
    assert_eq!(schema.locales, Some(vec!["en".into(), "ar".into(), "sl".into()]));
}

#[test]
fn control_a_program_without_locales_has_none() {
    let schema = parse_and_lower_str(&HOME.replace(", locales: [en, ar, sl]", "")).expect("lower");
    assert_eq!(schema.locales, None);
}

#[test]
fn a_literal_key_is_qualified_with_the_app_name() {
    let schema = parse_and_lower_str(HOME).expect("lower");
    let json = serde_json::to_string(&schema).unwrap();
    assert!(json.contains(r#"["i18n/t","std-almadar-home:hero.title"]"#), "{json}");
}

#[test]
fn a_computed_key_lowers_unqualified_for_the_type_checker() {
    let source = HOME.replace(r#"(i18n/t "hero.title")"#, r#"(i18n/t (str/concat "hero." "title"))"#);
    let json = serde_json::to_string(&parse_and_lower_str(&source).expect("lower")).unwrap();
    assert!(json.contains(r#"["i18n/t",["str/concat","hero.","title"]]"#), "{json}");
}

const KNOB: &str = r#"app std-almadar-home [version: "1.0.0", description: "Home", locales: [en, ar, sl]]

orbital Home {
  entity HomeState [runtime] {
    id : string!
  }

  trait HomeView -> HomeState {
    initial: idle

    state idle {
      INIT -> idle
        (render-ui main { type: typography, content: (i18n/t @config.label), title: @config.plain, items: (array/map @config.seasons (fn s (i18n/t @s))) })
    }

    config {
      label : MessageKey = "hero.title"
      seasons : [MessageKey] = ["spring", "summer"]
      plain : string = "hero.title"
    }
  }

  page "/" as HomePage -> HomeView
}
"#;

#[test]
fn a_message_key_knob_literal_is_qualified_and_a_binding_key_is_not() {
    let json = serde_json::to_string(&parse_and_lower_str(KNOB).expect("lower")).unwrap();
    assert!(json.contains(r#""default":"std-almadar-home:hero.title""#), "{json}");
    assert!(json.contains(r#"["std-almadar-home:spring","std-almadar-home:summer"]"#), "{json}");
    assert!(json.contains(r#"["i18n/t","@config.label"]"#), "{json}");
    assert!(json.contains(r#""default":"hero.title""#), "control: a string knob is not a key: {json}");
}

#[test]
fn edge_a_message_key_knob_with_the_reserved_separator_is_rejected() {
    let source = KNOB.replace(r#"label : MessageKey = "hero.title""#, r#"label : MessageKey = "x:hero.title""#);
    let err = parse_and_lower_str(&source).expect_err("reserved separator");
    assert!(err.to_string().contains("ORB_I18N_KEY_INVALID"), "{err}");
}

#[test]
fn edge_a_key_with_the_reserved_separator_is_rejected() {
    let source = HOME.replace(r#""hero.title""#, r#""other:hero.title""#);
    let err = parse_and_lower_str(&source).expect_err("reserved separator");
    assert!(err.to_string().contains("ORB_I18N_KEY_INVALID"), "{err}");
}

#[test]
fn the_formatter_keeps_the_locales_header_and_the_unqualified_key() {
    let formatted = format_program(&parse_str(HOME).expect("parse"));
    assert!(formatted.contains("locales: [en, ar, sl]"), "{formatted}");
    assert!(formatted.contains(r#"(i18n/t "hero.title")"#), "{formatted}");
}

#[test]
fn the_locale_binding_parses_inside_render_props() {
    let source = HOME.replace(r#"content: (i18n/t "hero.title")"#, r#"content: (i18n/t "hero.title"), dir: @locale"#);
    let json = serde_json::to_string(&parse_and_lower_str(&source).expect("lower")).unwrap();
    assert!(json.contains(r#""dir":"@locale""#), "{json}");
}

#[test]
fn the_public_header_lowers_to_public_dir() {
    let source = HOME.replace("locales: [en, ar, sl]]", "locales: [en, ar, sl], public: \"./public\"]");
    assert_eq!(parse_and_lower_str(&source).expect("lower").public_dir.as_deref(), Some("./public"));
    let formatted = format_program(&parse_str(&source).expect("parse"));
    assert!(formatted.contains("public: \"./public\""), "{formatted}");
}

#[test]
fn control_no_public_header_means_no_public_dir() {
    assert_eq!(parse_and_lower_str(HOME).expect("lower").public_dir, None);
}

/// `formatLocale: "sl-SI"` names the region whose rules the app's dates, times and numbers follow (owner ruling
/// 2026-11-24: Project Friday shows a 24-hour clock); `locales` stays the message catalogs the app ships.
mod format_locale {
    use super::HOME;
    use orbital_lolo::{format_program, parse_and_lower_str, parse_str};

    fn with(value: &str) -> String {
        HOME.replace("locales: [en, ar, sl]]", &format!("locales: [en, ar, sl], formatLocale: \"{value}\"]"))
    }

    #[test]
    fn the_format_locale_lowers_beside_the_locales() {
        let schema = parse_and_lower_str(&with("sl-SI")).expect("lower");
        assert_eq!(schema.format_locale.as_deref(), Some("sl-SI"));
        let json = serde_json::to_string(&schema).unwrap();
        assert!(json.contains(r#""formatLocale":"sl-SI""#), "{json}");
    }

    #[test]
    fn control_a_program_without_one_has_none() {
        let schema = parse_and_lower_str(HOME).expect("lower");
        assert_eq!(schema.format_locale, None);
        assert!(!serde_json::to_string(&schema).unwrap().contains("formatLocale"));
    }

    #[test]
    fn a_format_locale_without_a_region_is_refused() {
        let err = parse_and_lower_str(&with("sl")).expect_err("a bare language names no region's rules");
        assert!(err.to_string().contains("formatLocale"), "{err}");
    }

    #[test]
    fn control_a_script_and_region_tag_is_accepted() {
        assert_eq!(parse_and_lower_str(&with("sr-Latn-RS")).expect("lower").format_locale.as_deref(), Some("sr-Latn-RS"));
    }

    #[test]
    fn the_formatter_keeps_it() {
        let program = parse_str(&with("en-GB")).expect("parse");
        let printed = format_program(&program);
        assert!(printed.contains(r#"formatLocale: "en-GB""#), "{printed}");
        assert_eq!(parse_and_lower_str(&printed).expect("lower").format_locale.as_deref(), Some("en-GB"));
    }
}
