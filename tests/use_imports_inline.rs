//! End-to-end roundtrip test for `.lolo` `use` imports.
//!
//! Proves that a `.lolo` source file with `uses` declarations and trait/page
//! reference forms (`trait Name = Alias.traits.X { ... }`,
//! `page "/p" = Alias.pages.X -> Trait`) lowers to an `OrbitalSchema` whose
//! shape is accepted by `orbital-compiler`'s inline phase. The inliner must
//! resolve `Modal.traits.ModalRecordModal` against the installed std-modal
//! package (Phase 2: no embedded fallback), apply the `linkedEntity` /
//! `events` / `effects` overrides, and produce a fully self-contained
//! `TraitReference::Inline` for the downstream phases.

use orbital_compiler::loader::LoaderConfig;
use orbital_compiler::phases::inline::{inline_schema, InlineConfig};
use orbital_core_base::schema::types::{PageRef, TraitReference};
use orbital_lolo::parse_and_lower_str;
use std::path::Path;

const USE_IMPORTS_LOLO: &str = r#"
orbital CartItemOrbital {
  uses Modal from "std/behaviors/std-modal"

  entity CartItem [persistent: cartitems] {
    id : string!
    name : string
    pendingId : string
  }

  trait CartItemAddItem = Modal.traits.ModalRecordModal -> CartItem {
    events {
      OPEN: ADD_ITEM
    }
    on ADD_ITEM {
      (fetch CartItem)
      (render-ui modal { type: "form-section", entity: "CartItem", mode: "create" })
    }
    on SAVE {
      (persist create CartItem @payload.data)
      (render-ui modal null)
    }
  }

  page "/cart" = Modal.pages.ModalRecordModalPage -> CartItemAddItem
}
"#;

#[test]
#[ignore = "source uses `on EVENT { ... }` trait-ref effects-override syntax, removed in Phase 9.5.H. The remaining assertions still exercise the event-rename + linkedEntity override pipeline; source needs a rewrite away from the removed syntax before this can re-enable."]
fn lolo_use_imports_lower_to_inlinable_schema() {
    let mut schema = parse_and_lower_str(USE_IMPORTS_LOLO).expect("parse_and_lower must succeed");

    assert_eq!(schema.orbitals.len(), 1, "expected one orbital");
    let orbital = &schema.orbitals[0];

    // Pre-inline assertions: the lowering produced a uses[] entry, a trait
    // reference, and a page reference.
    let uses = orbital.uses.as_ref().expect("uses[] must be populated");
    assert_eq!(uses.len(), 1);
    assert_eq!(uses[0].alias, "Modal");
    assert_eq!(uses[0].from, "std/behaviors/std-modal");

    let pre_trait_ref = orbital
        .traits
        .iter()
        .find_map(|t| match t {
            TraitReference::Reference {
                reference,
                name,
                linked_entity,
                events,
                ..
            } => Some((
                reference.clone(),
                name.clone(),
                linked_entity.clone(),
                events.clone(),
            )),
            _ => None,
        })
        .expect("expected a TraitReference::Reference for the lowered cart trait");
    assert_eq!(
        pre_trait_ref.0.as_deref(),
        Some("Modal.traits.ModalRecordModal")
    );
    assert_eq!(pre_trait_ref.1.as_deref(), Some("CartItemAddItem"));
    assert_eq!(pre_trait_ref.2.as_deref(), Some("CartItem"));
    assert_eq!(
        pre_trait_ref.3.as_ref().and_then(|m| m.get("OPEN")),
        Some(&"ADD_ITEM".to_string())
    );
    // Note: per-event `effects` overrides on `TraitReference::Reference` were
    // removed in Phase 9.5.H — effects now live only in the inlined trait's
    // transitions, not as a separate override map on the reference.

    let pre_pages = orbital.pages.as_ref().expect("pages must be present");
    assert_eq!(pre_pages.len(), 1);
    match &pre_pages[0] {
        PageRef::Object(obj) => {
            assert_eq!(obj.reference, "Modal.pages.ModalRecordModalPage");
            assert_eq!(obj.path.as_deref(), Some("/cart"));
            let traits = obj.traits.as_ref().expect("traits override must be set");
            assert_eq!(traits.len(), 1);
        }
        other => panic!("expected PageRef::Object, got {other:?}"),
    }

    // Run the inline phase against the installed std package (Phase 2: no
    // embedded loader fallback).
    let std_pkg = orbital_test_support::installed_package(
        "almadar-std",
        Path::new(env!("CARGO_MANIFEST_DIR")),
    );
    let config = InlineConfig {
        loader_config: LoaderConfig {
            packages: vec![std_pkg],
            ..Default::default()
        },
        ..Default::default()
    };
    inline_schema(&mut schema, &config, None).expect("inline_schema must succeed");

    let inlined_orbital = &schema.orbitals[0];

    // After inlining: the user's CartItem entity is unchanged.
    let entity = inlined_orbital
        .entity
        .as_inline()
        .expect("entity must be inline after inlining");
    assert_eq!(entity.name, "CartItem");

    // After inlining: the trait reference is replaced by an inlined definition
    // whose linked_entity is the override (CartItem) and whose state machine's
    // ADD_ITEM event reflects the rename of OPEN.
    let inlined_trait = inlined_orbital
        .traits
        .iter()
        .find_map(|t| match t {
            TraitReference::Inline(def) if def.name == "CartItemAddItem" => Some(def),
            _ => None,
        })
        .expect("expected an inlined CartItemAddItem trait after inlining");
    assert_eq!(inlined_trait.linked_entity.as_deref(), Some("CartItem"));

    // The state machine must carry the renamed event (ADD_ITEM) — which is
    // the proof the inliner ran the F.3 substitution pass on the override.
    let event_keys: Vec<&str> = inlined_trait
        .state_machine
        .events
        .as_ref()
        .map(|evts| evts.iter().map(|e| e.key.as_str()).collect())
        .unwrap_or_default();
    assert!(
        event_keys.contains(&"ADD_ITEM"),
        "expected renamed ADD_ITEM event in inlined trait, got {:?}",
        event_keys
    );
    assert!(
        !event_keys.contains(&"OPEN"),
        "OPEN should have been renamed away, got {:?}",
        event_keys
    );

    // The ADD_ITEM transition's effects must be the override list (fetch +
    // render-ui), not the std-modal default. We just check the list is
    // non-empty and the first effect is `[fetch, CartItem]` to keep the
    // assertion stable across cosmetic shape changes.
    let add_item_effects = inlined_trait
        .state_machine
        .transitions
        .iter()
        .find(|tr| tr.event == "ADD_ITEM")
        .and_then(|tr| tr.effects.as_ref())
        .expect("ADD_ITEM transition must have effects after override");
    assert!(
        !add_item_effects.is_empty(),
        "ADD_ITEM effects must be non-empty after override"
    );
    let first_effect = &add_item_effects[0];
    let arr = first_effect
        .as_array()
        .expect("first effect must be an SExpression array");
    assert_eq!(arr[0].as_str(), Some("fetch"));
    assert_eq!(arr[1].as_str(), Some("CartItem"));

    // After inlining: the page reference is replaced by an inline page whose
    // path is the override (/cart) and whose primary entity is CartItem.
    let inlined_page = inlined_orbital
        .pages
        .as_ref()
        .and_then(|pages| pages.first())
        .and_then(|p| match p {
            PageRef::Inline(def) => Some(def),
            _ => None,
        })
        .expect("expected an inlined page after inlining");
    assert_eq!(inlined_page.path, "/cart");

    // The traits override on the page must point at our local CartItemAddItem.
    let page_traits = inlined_page
        .traits
        .as_ref()
        .expect("page must carry a traits list");
    let page_trait_names: Vec<Option<&str>> = page_traits
        .iter()
        .map(|t| match t {
            TraitReference::Reference { reference, .. } => reference.as_deref(),
            TraitReference::Inline(def) => Some(def.name.as_str()),
        })
        .collect();
    assert!(
        page_trait_names
            .iter()
            .any(|n| n == &Some("CartItemAddItem")),
        "expected page traits to reference CartItemAddItem, got {:?}",
        page_trait_names
    );

    // Belt-and-braces: the inline phase must have removed every leftover
    // `TraitReference::Reference` for the imported trait. The only remaining
    // references on the orbital itself are the original ones (which inline
    // collapses into Inline entries — the orbital should now contain ONLY
    // Inline trait entries for its own declared traits).
    let stray_orbital_ref = inlined_orbital
        .traits
        .iter()
        .find(|t| matches!(t, TraitReference::Reference { .. }));
    assert!(
        stray_orbital_ref.is_none(),
        "no stray TraitReference::Reference should remain on the orbital after inlining; \
         got {:#?}",
        inlined_orbital.traits
    );
}
