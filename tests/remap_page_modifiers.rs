//! A whole-orbital import's `pages {}` remap entry takes the page modifier
//! bracket: `"/snake": "/play/snake" [access: public, indexing: noindex]`. It
//! lowers to `reference.pageModifiers`, keyed by the upstream path. A page
//! takes `translationOf:` to name its translation group.

use orbital_core::schema::{PageAccess, PageIndexing, PageMeta, PageRef};
use orbital_lolo::{format_program, parse_and_lower_str, parse_str};

fn import(entry: &str) -> String {
    format!(
        r#"app arcade [version: "1.0.0", locales: [en, ar]]

uses Snake from "./snake"

orbital PlaySnake = Snake.orbitals.SnakeOrbital {{
  pages {{
    {entry}
  }}
}}
"#
    )
}

fn reference(source: &str) -> orbital_core::schema::OrbitalRefObject {
    let schema = parse_and_lower_str(source).expect("lower");
    schema.orbitals[0].reference.clone().expect("reference")
}

#[test]
fn a_remap_entry_bracket_lowers_to_page_modifiers_by_upstream_path() {
    let r = reference(&import(
        r#""/snake": "/play/snake" [access: public, indexing: noindex, title: (i18n/t "game.snake.title")]"#,
    ));
    assert_eq!(
        r.pages
            .as_ref()
            .and_then(|p| p.get("/snake"))
            .map(String::as_str),
        Some("/play/snake")
    );
    let m = r
        .page_modifiers
        .as_ref()
        .and_then(|m| m.get("/snake"))
        .expect("modifiers");
    assert_eq!(m.access, Some(PageAccess::Public));
    assert_eq!(m.indexing, Some(PageIndexing::Noindex));
    assert_eq!(m.title, Some(PageMeta::message("arcade:game.snake.title")));
    assert!(m.description.is_none() && m.translation_of.is_none());
}

#[test]
fn control_a_remap_entry_without_a_bracket_declares_none() {
    let r = reference(&import(r#""/snake": "/play/snake""#));
    assert!(r.page_modifiers.is_none());
    let json = serde_json::to_string(&r).unwrap();
    assert!(!json.contains("pageModifiers"), "{json}");
}

#[test]
fn edge_a_bracket_follows_a_mount_tail() {
    let r = reference(&import(
        r#""/snake": "/play/snake" -> Extra [access: public]"#,
    ));
    assert_eq!(
        r.page_modifiers
            .as_ref()
            .and_then(|m| m.get("/snake"))
            .and_then(|m| m.access),
        Some(PageAccess::Public)
    );
}

#[test]
fn a_page_only_key_is_rejected_on_a_remap_entry() {
    let err = parse_and_lower_str(&import(r#""/snake": "/play/snake" [view: list]"#)).unwrap_err();
    assert!(err.to_string().contains("ORB_L_OPTION_UNKNOWN"), "{err}");
}

#[test]
fn a_remap_entry_bracket_survives_formatting() {
    let source = import(
        r#""/snake": "/play/snake" [access: public, indexing: noindex, translationOf: "snake"]"#,
    );
    let formatted = format_program(&parse_str(&source).expect("parse"));
    assert!(
        formatted.contains("translationOf: \"snake\""),
        "{formatted}"
    );
    assert_eq!(
        serde_json::to_value(reference(&formatted)).unwrap(),
        serde_json::to_value(reference(&source)).unwrap()
    );
}

#[test]
fn a_page_names_its_translation_group() {
    let source = r#"app blog [version: "1.0.0"]

orbital Post {
  entity PostState [runtime] {
    id : string!
  }

  trait PostView -> PostState [interaction, instance] {
    initial: idle
    state idle {
      INIT -> idle
        (render-ui main { type: typography, content: "Post" })
    }
  }

  page "/ar/blog/x" as PostAR -> PostView [access: public, translationOf: "blog/x"]
}
"#;
    let schema = parse_and_lower_str(source).expect("lower");
    let PageRef::Inline(page) = &schema.orbitals[0].pages.as_ref().unwrap()[0] else {
        panic!("inline page")
    };
    assert_eq!(page.translation_of.as_deref(), Some("blog/x"));
    let formatted = format_program(&parse_str(source).expect("parse"));
    assert!(
        formatted.contains("translationOf: \"blog/x\""),
        "{formatted}"
    );
}

#[test]
fn a_non_string_translation_group_is_rejected() {
    let err = parse_and_lower_str(&import(r#""/snake": "/play/snake" [translationOf: snake]"#))
        .unwrap_err();
    assert!(err.to_string().contains("ORB_L_OPTION_VALUE"), "{err}");
}
