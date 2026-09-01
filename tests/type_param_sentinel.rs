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

// ---------------------------------------------------------------------------
// Sigil type constructors (2026-09-01): `@payload <knob>`, `@entity <knob>`,
// `@user`. In TYPE position a sigil denotes the type of what it names, applied
// by juxtaposition like `Map K V`. These assert the PRE-INLINE lowering — the
// sentinel the compiler's resolution pass later consumes.
// ---------------------------------------------------------------------------

fn config_knob_type(src: &str, knob: &str) -> String {
    let schema = parse_and_lower_str(src).expect("must lower");
    let TraitReference::Inline(t) = &schema.orbitals[0].traits[0] else {
        panic!("inline trait expected")
    };
    t.config
        .as_ref()
        .expect("trait has config")
        .get(knob)
        .unwrap_or_else(|| panic!("knob `{knob}` missing"))
        .field_type
        .clone()
}

const SIGIL_SRC: &str = r#"
orbital SigilOrbital {
  entity SigilData [runtime] {
    id : string!
  }

  trait SigilRender -> SigilData [interaction, instance] {
    initial: idle
    state idle {
      INIT -> idle
        (render-ui main { type: box  padding: md })
    }
    config {
      action : event = "DO_THING"
      actionPayload : @payload @config.action
      targetEntity : entity = "SigilData"
      targetRow : @entity @config.targetEntity
      actor : @user
    }
  }

  page "/sigil" -> SigilRender
}
"#;

#[test]
fn payload_of_knob_lowers_to_a_knob_carrying_sentinel() {
    // The knob NAME must survive lowering — resolution needs it to read the
    // sibling's value, and the generic `type_expr_to_config_string` path would
    // have thrown it away.
    assert_eq!(config_knob_type(SIGIL_SRC, "actionPayload"), "@payload(action)");
}

#[test]
fn entity_of_knob_lowers_to_a_knob_carrying_sentinel() {
    assert_eq!(config_knob_type(SIGIL_SRC, "targetRow"), "@entity(targetEntity)");
}

#[test]
fn user_lowers_to_the_identity_sentinel() {
    assert_eq!(config_knob_type(SIGIL_SRC, "actor"), "@user");
}

#[test]
fn bare_at_payload_in_type_position_is_rejected() {
    // Arity is part of the constructor's contract. `@entity` bare is the bound
    // entity; `@payload` bare would have to mean "the current event's payload",
    // which a config knob has no notion of — so it must be a parse error, not a
    // dangling sentinel lowered into the .orb.
    let src = SIGIL_SRC.replace("@payload @config.action", "@payload");
    let err = parse_and_lower_str(&src).expect_err("bare @payload must not parse");
    let msg = format!("{err:?}");
    assert!(
        msg.contains("@payload"),
        "error should name the construct, got: {msg}"
    );
}

// ---------------------------------------------------------------------------
// Primitive-variant unions (`scalar | FileItem`)
// ---------------------------------------------------------------------------

/// A union alias whose members are all KNOWN TYPES lowers to a real union, not
/// to a string enum of the members' own names. Before this, `scalar | FileItem`
/// declared a field whose only legal values were the literal words "scalar" and
/// "FileItem" — it accepted the variant's NAME and rejected the variant's VALUE.
#[test]
fn union_of_primitive_and_struct_variants_lowers_to_a_union() {
    let src = r#"
app u "1.0.0"
"probe"
orbital P {
  type FileItem = { name : string!, url : string! }
  type Ctl = scalar | FileItem
  entity Item [runtime] {
    id : string!
    v : Ctl
  }
  trait R -> Item [interaction, instance] {
    initial: idle
    state idle {
      INIT -> idle
        (set @entity.v "x")
    }
  }
  page "/p" as PP -> R
}
"#;
    let schema = parse_and_lower_str(src).expect("union alias must lower");
    let ent = match &schema.orbitals[0].entity {
        orbital_core::schema::EntityRef::Inline(e) => e,
        _ => panic!("inline entity expected"),
    };
    let v = ent
        .fields
        .iter()
        .find(|f| f.name.as_deref() == Some("v"))
        .expect("field v");
    assert_eq!(
        v.field_type,
        orbital_core::schema::FieldType::Union,
        "all-known-type members must lower to Union, got {:?}",
        v.field_type
    );
    let props = v.properties.as_ref().expect("union carries its variants");
    assert!(
        props.contains_key("scalar") && props.contains_key("FileItem"),
        "both variants must be carried, got {:?}",
        props.keys().collect::<Vec<_>>()
    );
}

/// The safety property. A union with even ONE member that names no type keeps
/// its enum reading — this is what protects `format : date | currency | number`
/// and `up | down | left | right`, both live in the corpus.
#[test]
fn union_with_a_non_type_member_stays_an_enum() {
    let src = r#"
app u "1.0.0"
"probe"
orbital P {
  type Fmt = date | currency | number
  entity Item [runtime] {
    id : string!
    f : Fmt
  }
  trait R -> Item [interaction, instance] {
    initial: idle
    state idle {
      INIT -> idle
        (set @entity.f date)
    }
  }
  page "/p" as PP -> R
}
"#;
    let schema = parse_and_lower_str(src).expect("enum alias must lower");
    let ent = match &schema.orbitals[0].entity {
        orbital_core::schema::EntityRef::Inline(e) => e,
        _ => panic!("inline entity expected"),
    };
    let f = ent
        .fields
        .iter()
        .find(|x| x.name.as_deref() == Some("f"))
        .expect("field f");
    assert_ne!(
        f.field_type,
        orbital_core::schema::FieldType::Union,
        "`currency` names no type, so this must stay a string enum"
    );
    assert_eq!(
        f.values.as_deref(),
        Some(["date", "currency", "number"].map(String::from).as_slice()),
        "enum members must survive verbatim"
    );
}
