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
fn a_computed_key_is_rejected() {
    let source = HOME.replace(r#"(i18n/t "hero.title")"#, r#"(i18n/t (str/concat "hero." "title"))"#);
    let err = parse_and_lower_str(&source).expect_err("dynamic key");
    assert!(err.to_string().contains("ORB_I18N_KEY_DYNAMIC"), "{err}");
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
