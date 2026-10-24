//! Field-level rebindability (`@intrinsic`) + author-time rebind validation.
//!
//! - `@intrinsic` on an entity field marks machinery the trait computes itself;
//!   it lowers to `FieldDefinition.intrinsic = Some(true)` and is excluded from
//!   the rebindable trait's `entityContract.requires`.
//! - A `@rebindable` trait that reads a DATA `@entity.<field>` (not written, not
//!   intrinsic) absent from the bound entity is rejected at lower time with
//!   `LowerError::RebindMissingDataField` (test d).

use orbital_core_base::schema::TraitReference;
use orbital_lolo::parse_and_lower_str;

/// (Fix 1) `@intrinsic` parses, lowers onto the field, and the intrinsic field
/// is kept OUT of the rebindable trait's `requires` contract — a rebind target
/// is not required to declare it.
#[test]
fn intrinsic_field_lowers_and_is_excluded_from_requires() {
    let src = r#"app test-intrinsic "1.0.0"
"intrinsic marking test"

orbital WidgetOrbital {
  entity WidgetView [runtime] {
    id          : string!
    viewState   : string @intrinsic = ""
  }

  trait WidgetReview -> @rebindable WidgetView {
    initial: idle
    state idle {
      RENDER -> idle
        (set @entity.viewState "computed")
        (render-ui main { type: "typography", content: "@entity.viewState" })
    }
  }
}"#;

    let schema = parse_and_lower_str(src).expect("lolo with @intrinsic must lower");
    let orbital = &schema.orbitals[0];

    // Field carries intrinsic flag.
    let entity = match &orbital.entity {
        orbital_core_base::schema::EntityRef::Inline(e) => e,
        _ => panic!("expected inline entity"),
    };
    let vs = entity
        .fields
        .iter()
        .find(|f| f.name.as_deref() == Some("viewState"))
        .expect("viewState field present");
    assert_eq!(
        vs.intrinsic,
        Some(true),
        "@intrinsic must lower to FieldDefinition.intrinsic = Some(true)"
    );

    // Contract excludes the intrinsic field from `requires` (it's also written,
    // so it lands in `provides`, never `requires`).
    let TraitReference::Inline(td) = &orbital.traits[0] else {
        panic!("expected inline trait");
    };
    let contract = td
        .entity_contract
        .as_ref()
        .expect("rebindable trait has a contract");
    assert!(
        !contract.requires.contains(&"viewState".to_string()),
        "intrinsic field must not appear in entityContract.requires, got {:?}",
        contract.requires
    );
}

/// `@description "..."` and `@synonyms "..."` on an entity field parse as trailing
/// string annotations and lower onto `FieldDefinition.{description,synonyms}`. They
/// coexist with flag annotations (`@intrinsic`) and a trailing `= default`.
#[test]
fn description_and_synonyms_lower_onto_field() {
    let src = r#"app test-field-annots "1.0.0"
"field annotation test"

orbital ApptOrbital {
  entity Appt [runtime] {
    id    : string!
    start : datetime @description "appointment start time" @synonyms "scheduled, begins"
    range : string @intrinsic @description "selected range" = "month"
  }

  trait ApptShow -> Appt {
    initial: idle
    state idle {
      RENDER -> idle
        (render-ui main { type: "typography", content: "Appointment" })
    }
  }
}"#;

    let schema = parse_and_lower_str(src).expect("lolo with @description/@synonyms must lower");
    let entity = match &schema.orbitals[0].entity {
        orbital_core_base::schema::EntityRef::Inline(e) => e,
        _ => panic!("expected inline entity"),
    };
    let field = |n: &str| {
        entity
            .fields
            .iter()
            .find(|f| f.name.as_deref() == Some(n))
            .unwrap_or_else(|| panic!("{n} field present"))
    };

    let start = field("start");
    assert_eq!(start.description.as_deref(), Some("appointment start time"));
    assert_eq!(start.synonyms.as_deref(), Some("scheduled, begins"));

    // Coexists with @intrinsic and a `= default` on the same field.
    let range = field("range");
    assert_eq!(range.intrinsic, Some(true));
    assert_eq!(range.description.as_deref(), Some("selected range"));
    assert_eq!(range.synonyms, None);
}

/// `@mock "..."` on an entity field parses the same way as `@description`/
/// `@synonyms` and lowers onto `FieldDefinition.mock` — the seeder (not this
/// parser) is what treats the string literally / splits it on commas.
#[test]
fn mock_lowers_onto_field_and_coexists_with_other_annotations() {
    let src = r#"app test-field-mock "1.0.0"
"field mock annotation test"

orbital ClientOrbital {
  entity Client [runtime] {
    id          : string!
    clientName  : string @mock "Acme Corp, Globex Inc, Wayne Enterprises"
    contactName : string @description "primary contact" @mock "person-name"
  }

  trait ClientShow -> Client {
    initial: idle
    state idle {
      RENDER -> idle
        (render-ui main { type: "typography", content: "Client" })
    }
  }
}"#;

    let schema = parse_and_lower_str(src).expect("lolo with @mock must lower");
    let entity = match &schema.orbitals[0].entity {
        orbital_core_base::schema::EntityRef::Inline(e) => e,
        _ => panic!("expected inline entity"),
    };
    let field = |n: &str| {
        entity
            .fields
            .iter()
            .find(|f| f.name.as_deref() == Some(n))
            .unwrap_or_else(|| panic!("{n} field present"))
    };

    let client_name = field("clientName");
    assert_eq!(
        client_name.mock.as_deref(),
        Some("Acme Corp, Globex Inc, Wayne Enterprises")
    );

    // Coexists with @description on the same field, order-independent.
    let contact_name = field("contactName");
    assert_eq!(contact_name.description.as_deref(), Some("primary contact"));
    assert_eq!(contact_name.mock.as_deref(), Some("person-name"));

    // A field with no @mock stays None.
    assert_eq!(field("id").mock, None);
}

#[test]
fn description_synonyms_tier_lower_onto_events() {
    let src = r#"app test-event-annots "1.0.0"
"event annotation test"

orbital EvtOrbital {
  entity Thing [runtime] {
    id : string!
  }

  trait ThingFlow -> Thing {
    initial: idle
    state idle {
      OPEN -> active
    }
    state active {
      CLOSE -> idle
    }
    emits {
      OPENED { }
        @description "fired when opened"
        @synonyms "launch, start"
        @tier "essential"
    }
    listens {
      REFRESH { id : string! }
        @description "reload request"
        @synonyms "reload, sync"
        @tier "advanced"
    }
  }
}"#;

    let schema = parse_and_lower_str(src).expect("lolo with event annotations must lower");
    let trait_def = match &schema.orbitals[0].traits[0] {
        orbital_core_base::schema::TraitReference::Inline(t) => t,
        _ => panic!("expected inline trait"),
    };

    // The emit carries its `@description`/`@synonyms`/`@tier`.
    let opened = trait_def
        .emits
        .as_ref()
        .expect("emits present")
        .iter()
        .find(|e| e.event == "OPENED")
        .expect("OPENED emit present");
    assert_eq!(opened.description.as_deref(), Some("fired when opened"));
    assert_eq!(opened.synonyms.as_deref(), Some("launch, start"));
    assert_eq!(opened.tier.as_deref(), Some("essential"));

    // The unified event registry carries annotations for BOTH the emit and the
    // (payload-only local) listen — the per-event analogue of field/config annotations.
    let registry = trait_def
        .state_machine
        .events
        .as_ref()
        .expect("event registry present");
    let event = |k: &str| {
        registry
            .iter()
            .find(|e| e.key == k)
            .unwrap_or_else(|| panic!("{k} in registry"))
    };
    let opened_reg = event("OPENED");
    assert_eq!(opened_reg.description.as_deref(), Some("fired when opened"));
    assert_eq!(opened_reg.tier.as_deref(), Some("essential"));
    let refresh_reg = event("REFRESH");
    assert_eq!(refresh_reg.description.as_deref(), Some("reload request"));
    assert_eq!(refresh_reg.synonyms.as_deref(), Some("reload, sync"));
    assert_eq!(refresh_reg.tier.as_deref(), Some("advanced"));
}

/// (Fix 2 / test d) A `@rebindable` trait reading a DATA field with no setter
/// and no `@intrinsic` mark is rejected at author time by the explicit-binding
/// rule. A consumer rebinding the trait could never satisfy the read either, so
/// the defect is caught at the atom.
#[test]
fn rebindable_trait_reading_unsatisfied_data_field_is_rejected() {
    let src = r#"app test-missing-data "1.0.0"
"unsatisfied rebind test"

orbital BadOrbital {
  entity BadView [runtime] {
    id       : string!
    headline : string = ""
  }

  trait BadReview -> @rebindable BadView {
    initial: idle
    state idle {
      RENDER -> idle
        (render-ui main { type: "typography", content: "@entity.headline" })
    }
  }
}"#;

    let err = parse_and_lower_str(src)
        .expect_err("a rebindable trait reading an unsatisfied DATA field must be rejected");
    let msg = format!("{err}");
    assert!(
        msg.contains("headline"),
        "error must name the unsatisfied DATA field, got: {msg}"
    );
}

/// (Fix 1, negative) Marking the otherwise-missing field `@intrinsic` makes the
/// same trait lower cleanly — intrinsic reads need no DATA backing.
#[test]
fn marking_missing_field_intrinsic_unblocks_lowering() {
    let src = r#"app test-intrinsic-unblock "1.0.0"
"intrinsic unblocks read test"

orbital OkOrbital {
  entity OkView [runtime] {
    id       : string!
    headline : string @intrinsic = ""
  }

  trait OkReview -> @rebindable OkView {
    initial: idle
    state idle {
      RENDER -> idle
        (render-ui main { type: "typography", content: "@entity.headline" })
    }
  }
}"#;

    parse_and_lower_str(src).expect("an intrinsic field read needs no DATA backing and must lower");
}
