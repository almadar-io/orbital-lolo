//! Free-compose agents mount a trait on a page by writing `@trait.X` under the page declaration,
//! or by opening a `{ }` body after it — neither is page syntax (traits follow `->`, comma-
//! separated). The parse error names the fix and the corrected line instead of a generic
//! "expected uses, expects, …".

use orbital_lolo::parse_and_lower_str;

const ORBITAL_HEAD: &str = r#"
orbital StatsOrbital {
  entity Stat [runtime] {
    id : string!
  }
  trait StatsView -> Stat [interaction] {
    state idle {
      INIT -> idle
    }
  }
  trait StatsLayout -> Stat [interaction] {
    state idle {
      INIT -> idle
    }
  }
"#;

fn source(page: &str) -> String {
    format!("{ORBITAL_HEAD}  {page}\n}}\n")
}

#[test]
fn a_trait_sigil_under_a_page_names_the_arrow_list_and_the_corrected_line() {
    let err = parse_and_lower_str(&source(
        "page \"/stats\" as StatsPage -> StatsView\n    @trait.StatsLayout",
    ))
    .expect_err("a @trait line under a page is not page syntax");
    let msg = err.to_string();
    assert!(msg.contains("follow `->`"), "{msg}");
    assert!(
        msg.contains("page \"/stats\" as StatsPage -> StatsView, StatsLayout"),
        "{msg}"
    );
}

#[test]
fn a_brace_body_after_a_page_names_the_arrow_list() {
    let err = parse_and_lower_str(&source(
        "page \"/stats\" as StatsPage -> StatsView {\n    @trait.StatsLayout\n  }",
    ))
    .expect_err("a page has no { } body");
    assert!(err.to_string().contains("follow `->`"), "{err}");
}

#[test]
fn control_traits_listed_after_the_arrow_parse() {
    assert!(parse_and_lower_str(&source(
        "page \"/stats\" as StatsPage -> StatsLayout, StatsView"
    ))
    .is_ok());
}

#[test]
fn control_icon_and_label_annotations_under_a_page_still_parse() {
    assert!(parse_and_lower_str(&source(
        "page \"/stats\" as StatsPage -> StatsView\n    @icon \"chart\"\n    @label \"Stats\""
    ))
    .is_ok());
}
