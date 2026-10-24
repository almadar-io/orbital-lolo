//! A JSX/`@(…)`-hoisted inline fragment must mount only on pages that
//! declare its producing trait — not on every inline page in the orbital.
//! Regression coverage for the `pending_inline_traits` orbital-wide spill
//! (a page declaring only `TraitB` was getting `TraitA`'s hoisted children
//! too, tripping `ORB_PAGE_SECOND_CONTENT_BODY`).

use orbital_lolo::parse_str;

fn inline_trait_names(page_traits: &[String]) -> Vec<&str> {
    page_traits
        .iter()
        .filter(|t| t.starts_with("Inline"))
        .map(|t| t.as_str())
        .collect()
}

#[test]
fn disjoint_pages_get_only_their_own_producers_fragments() {
    let src = r#"orbital WidgetOrbital {
  uses UiSpinner from "std/behaviors/ui-spinner"
  uses UiButton from "std/behaviors/ui-button"

  entity Widget [persistent: widgets] {
    id : string!
    label : string
  }

  trait TraitA -> Widget {
    state idle {
      INIT -> idle
        (render-ui main {
          type: "stack",
          children: [
            @(UiSpinner.traits.SpinnerRender -> Widget { config { size: "sm" } })
          ]
        })
    }
  }

  trait TraitB -> Widget {
    state idle {
      INIT -> idle
        (render-ui main {
          type: "stack",
          children: [
            @(UiButton.traits.ButtonRender -> Widget { config { label: "Click" } })
          ]
        })
    }
  }

  page "/a" as PageA -> TraitA
  page "/b" as PageB -> TraitB
}
"#;
    let program = parse_str(src).expect("parse must succeed");
    let orbital = &program.orbitals[0];

    let page_a = orbital.pages.iter().find(|p| p.name.as_deref() == Some("PageA")).unwrap();
    let page_b = orbital.pages.iter().find(|p| p.name.as_deref() == Some("PageB")).unwrap();

    let a_inline = inline_trait_names(&page_a.traits);
    let b_inline = inline_trait_names(&page_b.traits);

    assert_eq!(a_inline.len(), 1, "PageA should carry exactly its own hoisted fragment, got {a_inline:?}");
    assert_eq!(b_inline.len(), 1, "PageB should carry exactly its own hoisted fragment, got {b_inline:?}");
    assert_ne!(a_inline[0], b_inline[0], "each page's fragment must be distinct");

    // Cross-check against the producing trait via the synthesized name's
    // upstream short-name segment (`InlineSpinnerRenderN` / `InlineButtonRenderN`).
    assert!(a_inline[0].contains("Spinner"), "PageA fragment should come from TraitA's SpinnerRender child, got {a_inline:?}");
    assert!(b_inline[0].contains("Button"), "PageB fragment should come from TraitB's ButtonRender child, got {b_inline:?}");
}

#[test]
fn single_page_orbital_keeps_prior_behavior() {
    // Matches fixture 32-inline-trait-ref.lolo's shape: one page, two hoisted
    // children off the same trait. Must land on the sole page exactly as
    // before the fix (no attribution ambiguity with only one destination).
    let src = r#"orbital WidgetOrbital {
  uses UiSpinner from "std/behaviors/ui-spinner"
  uses UiButton from "std/behaviors/ui-button"

  entity Widget [persistent: widgets] {
    id : string!
    label : string
  }

  trait WidgetView -> Widget {
    state idle {
      INIT -> idle
        (render-ui main {
          type: "stack",
          direction: "vertical",
          gap: "md",
          children: [
            @(UiSpinner.traits.SpinnerRender -> Widget { config { size: "sm" } }),
            @(UiButton.traits.ButtonRender -> Widget { config { label: "Click", variant: "primary" } })
          ]
        })
    }
  }

  page "/widgets" as WidgetPage -> WidgetView
}
"#;
    let program = parse_str(src).expect("parse must succeed");
    let orbital = &program.orbitals[0];
    assert_eq!(orbital.pages.len(), 1);
    let page = &orbital.pages[0];
    let inline = inline_trait_names(&page.traits);
    assert_eq!(inline.len(), 2, "the sole page must carry both hoisted fragments, got {inline:?}");
    assert!(page.traits.iter().any(|t| t == "WidgetView"));
}

#[test]
fn trait_declared_on_both_pages_gives_fragments_to_both() {
    let src = r#"orbital WidgetOrbital {
  uses UiSpinner from "std/behaviors/ui-spinner"

  entity Widget [persistent: widgets] {
    id : string!
    label : string
  }

  trait SharedTrait -> Widget {
    state idle {
      INIT -> idle
        (render-ui main {
          type: "stack",
          children: [
            @(UiSpinner.traits.SpinnerRender -> Widget { config { size: "sm" } })
          ]
        })
    }
  }

  trait OtherTrait -> Widget {
    state idle {
      INIT -> idle
    }
  }

  page "/a" as PageA -> SharedTrait, OtherTrait
  page "/b" as PageB -> SharedTrait
}
"#;
    let program = parse_str(src).expect("parse must succeed");
    let orbital = &program.orbitals[0];

    let page_a = orbital.pages.iter().find(|p| p.name.as_deref() == Some("PageA")).unwrap();
    let page_b = orbital.pages.iter().find(|p| p.name.as_deref() == Some("PageB")).unwrap();

    let a_inline = inline_trait_names(&page_a.traits);
    let b_inline = inline_trait_names(&page_b.traits);

    assert_eq!(a_inline.len(), 1, "PageA declares SharedTrait, so it gets its fragment, got {a_inline:?}");
    assert_eq!(b_inline.len(), 1, "PageB declares SharedTrait, so it gets its fragment too, got {b_inline:?}");
    assert_eq!(a_inline[0], b_inline[0], "both pages share the SAME synthesized fragment trait");
}

#[test]
fn a_hoisted_fragment_takes_its_producers_declared_dispatch_mode() {
    let src = r#"orbital WidgetOrbital {
  uses UiButton from "std/behaviors/ui-button"

  entity Widget [runtime] {
    id : string!
  }

  trait Fast -> Widget [client] {
    state idle {
      INIT -> idle
        (render-ui main { type: "stack", children: [@(UiButton.traits.ButtonRender -> Widget { config { label: "Go" } })] })
    }
  }

  trait Plain -> Widget {
    state idle {
      INIT -> idle
        (render-ui main { type: "stack", children: [@(UiButton.traits.ButtonRender -> Widget { config { label: "Stop" } })] })
    }
  }

  page "/fast" as FastPage -> Fast
  page "/plain" as PlainPage -> Plain
}
"#;
    let program = parse_str(src).expect("parse must succeed");
    let orbital = &program.orbitals[0];
    let fragments: Vec<_> = orbital.traits.iter().filter(|t| t.name.starts_with("Inline")).collect();
    assert_eq!(fragments.len(), 2);
    assert_eq!(fragments[0].dispatch, Some(orbital_lolo::ast::TraitDispatch::Client), "Fast's fragment");
    assert_eq!(fragments[1].dispatch, None, "control: Plain declares no mode, so its fragment stays on the server");
}
