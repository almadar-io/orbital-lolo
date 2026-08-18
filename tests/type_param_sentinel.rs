//! Lowering test for the `(e : Entity)` type-parameter increment (3-I,
//! `docs/Almadar_LOLO.md` ~line 315/1387).
//!
//! `type X (e : Entity) = Event { row : e }` — a payload field referencing
//! the alias's own type parameter lowers to the compiler-substitution
//! sentinel string `"$e"` (`PayloadField.field_type`, `properties: None`),
//! never attempting entity/alias resolution against `E` as a literal type
//! name. The inline phase (`orbital-compiler`) substitutes the sentinel with
//! the using trait's bound entity — see
//! `orbital-compiler/tests/type_param_sentinel_inline.rs`.

use orbital_core::TraitReference;
use orbital_lolo::{format_program, parse_and_lower_str, parse_str};

const SRC: &str = r#"
type RecurrenceSaved (e : Entity) = Event { row : e }

orbital RecurrenceOrbital {
  entity Recurrence [persistent: recurrences] {
    id : string!
    name : string
  }

  trait RecurrenceSave -> Recurrence [interaction] {
    state idle {
      INIT -> idle (ref Recurrence)
    }
    emits
      RecurrenceSaved
  }

  page "/recurrences" -> RecurrenceSave
}
"#;

#[test]
fn event_alias_type_param_lowers_to_sentinel() {
    let schema = parse_and_lower_str(SRC).expect("parse_and_lower must succeed");
    assert_eq!(schema.orbitals.len(), 1);
    let orbital = &schema.orbitals[0];
    assert_eq!(orbital.traits.len(), 1);

    let TraitReference::Inline(trait_def) = &orbital.traits[0] else {
        panic!("expected an inline trait definition");
    };

    // The `emits` entry resolves its payload from the orbital-scope event
    // alias; the `row : e` field must carry the `$e` sentinel, not a
    // spuriously-resolved "e" type name or a validation error.
    let emits = trait_def.emits.as_ref().expect("emits[] must be populated");
    let recurrence_saved = emits
        .iter()
        .find(|e| e.event == "RecurrenceSaved")
        .expect("RecurrenceSaved emit entry");
    let payload = recurrence_saved
        .payload_schema
        .as_ref()
        .expect("payloadSchema must be populated from the alias");
    assert_eq!(payload.len(), 1);
    assert_eq!(payload[0].name, "row");
    assert_eq!(payload[0].field_type, "$e");
    assert!(
        payload[0].properties.is_none(),
        "sentinel field must carry no properties pre-inline"
    );

    // The same alias supplies `stateMachine.events[]`'s payload for the same
    // event key (Phase 2.5's `event_aliases` lookup, trait.rs ~line 421).
    let events = trait_def
        .state_machine
        .events
        .as_ref()
        .expect("events[] must be populated");
    let event_def = events
        .iter()
        .find(|e| e.key == "RecurrenceSaved")
        .expect("RecurrenceSaved event definition");
    let event_payload = event_def
        .payload_schema
        .as_ref()
        .expect("event payloadSchema must be populated");
    assert_eq!(event_payload.len(), 1);
    assert_eq!(event_payload[0].field_type, "$e");
}

/// The formatter must round-trip the parenthesized kinded form losslessly —
/// re-emitting the bare `type Name E = ...` shape would silently discard the
/// `Entity` kind annotation on every `orb fmt` pass.
#[test]
fn formatter_round_trips_parenthesized_kinded_param() {
    let program = parse_str(SRC).expect("parse");
    let formatted = format_program(&program);
    assert!(
        formatted.contains("type RecurrenceSaved (e : Entity) = "),
        "formatter must re-emit the parenthesized kinded form verbatim, got:\n{formatted}"
    );

    // Semantics-preserving: the reformatted source lowers to the same
    // sentinel-bearing payload as the original.
    let reformatted_lowering =
        parse_and_lower_str(&formatted).expect("reformatted source must still lower");
    let TraitReference::Inline(trait_def) = &reformatted_lowering.orbitals[0].traits[0] else {
        panic!("expected an inline trait definition");
    };
    let payload = trait_def.emits.as_ref().unwrap()[0]
        .payload_schema
        .as_ref()
        .unwrap();
    assert_eq!(payload[0].field_type, "$e");
}

// ---------------------------------------------------------------------------
// Syntax revision 2026-08-18: `@entity` bound-row self-reference + lowercase
// param enforcement + alias-inline consistency.
// ---------------------------------------------------------------------------

#[test]
fn bound_entity_row_lowers_to_the_at_entity_sentinel() {
    let src = r#"
type ThingSaved = Event { row : @entity } "Fired on persist"

orbital ThingOrbital {
  entity Thing [runtime] {
    id : string!
    name : string
  }

  trait ThingSim -> @rebindable Thing [interaction, instance] {
    initial: idle
    state idle {
      INIT -> idle
        (set @entity.name "x")
    }
    emits {
      ThingSaved
    }
  }

  page "/things" -> ThingSim
}
"#;
    let schema = parse_and_lower_str(src).expect("must lower");
    let orbital = &schema.orbitals[0];
    let TraitReference::Inline(t) = &orbital.traits[0] else {
        panic!("inline trait expected")
    };
    let emit = &t.emits.as_ref().unwrap()[0];
    let row = &emit.payload_schema.as_ref().unwrap()[0];
    assert_eq!(row.name, "row");
    assert_eq!(row.field_type, "@entity");
    assert!(row.properties.is_none());
}

#[test]
fn inline_restatement_matching_the_alias_passes() {
    let src = r#"
type ThingSaved = Event { row : @entity } "Fired on persist"

orbital ThingOrbital {
  entity Thing [runtime] {
    id : string!
  }

  trait ThingSim -> @rebindable Thing [interaction, instance] {
    initial: idle
    state idle {
      INIT -> idle
        (set @entity.id "x")
    }
    emits {
      ThingSaved { row : @entity }
    }
  }

  page "/things" -> ThingSim
}
"#;
    parse_and_lower_str(src).expect("a matching inline restatement must lower cleanly");
}

#[test]
fn inline_restatement_diverging_from_the_alias_errors() {
    let src = r#"
type ThingSaved = Event { row : @entity } "Fired on persist"

orbital ThingOrbital {
  entity Thing [runtime] {
    id : string!
  }

  trait ThingSim -> @rebindable Thing [interaction, instance] {
    initial: idle
    state idle {
      INIT -> idle
        (set @entity.id "x")
    }
    emits {
      ThingSaved { row : string }
    }
  }

  page "/things" -> ThingSim
}
"#;
    let err = parse_and_lower_str(src).expect_err("diverging restatement must error");
    assert!(
        format!("{err:?}").contains("PayloadAliasMismatch"),
        "expected PayloadAliasMismatch, got {err:?}"
    );
}

#[test]
fn uppercase_type_param_names_are_rejected() {
    let src = "type Foo (E : Entity) = Event { row : E }\norbital O {\n  entity T { id : string! }\n}";
    let err = parse_and_lower_str(src).expect_err("uppercase param must be a parse error");
    assert!(
        format!("{err:?}").contains("lowercase-initial"),
        "expected the lowercase guidance, got {err:?}"
    );
}
