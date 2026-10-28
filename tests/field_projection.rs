//! `T.f` in any type position is `T`'s declared type of field `f`, lowered as if written there,
//! with its provenance kept (`projectedFrom`) so a value can be drawn from `T`'s own `f`.

use orbital_lolo::{format_program, parse_and_lower_str, parse_str};
use orbital_core_base::schema::TraitReference;
use orbital_core_base::schema::types::{EntityRef, FieldProjection, FieldType, OrbitalSchema, PayloadField, TraitDefinition};

const APP: &str = r#"app projection-app [version: "1.0.0", description: "Projection"]

orbital NoteOrbital {
  entity Note [persistent: notes] {
    id : string!
    title : string!
    tagRef : Tag.id
    tagKind : Tag.kind
  }

  entity Tag [persistent: tags] {
    id : string!
    name : string!
    kind : "topic" | "person"
  }

  type Pick = { tag : Tag.id! }

  trait NoteView -> Note {
    config {
      defaultTag : Tag.name = "general"
    }

    initial: idle

    state idle {
      INIT -> idle
        (render-ui main { type: typography, content: @config.defaultTag })
      SHOW -> idle
        (render-ui main { type: typography, content: @payload.tagId })
    }

    emits {
      PICKED {
        pick : Pick
      }
    }

    listens {
      SHOW {
        tagId : Tag.id!
        row : Tag
      }
    }
  }

  page "/notes" -> NoteView
}
"#;

fn projection(type_name: &str, field: &str) -> Option<FieldProjection> {
    Some(FieldProjection { type_name: type_name.to_string(), field: Some(field.to_string()) })
}

fn lowered() -> OrbitalSchema {
    parse_and_lower_str(APP).expect("lower")
}

fn note_field(schema: &OrbitalSchema, name: &str) -> orbital_core_base::schema::types::FieldDefinition {
    let EntityRef::Inline(note) = &schema.orbitals[0].entity else { panic!("Note is the inline primary entity") };
    note.fields.iter().find(|f| f.name.as_deref() == Some(name)).cloned().expect(name)
}

fn note_view(schema: &OrbitalSchema) -> TraitDefinition {
    schema.orbitals[0]
        .traits
        .iter()
        .find_map(|t| match t {
            TraitReference::Inline(def) if def.name == "NoteView" => Some(TraitDefinition::clone(def)),
            _ => None,
        })
        .expect("NoteView")
}

fn payload_of(schema: &OrbitalSchema, event: &str) -> Vec<PayloadField> {
    let view = note_view(schema);
    let events = view.state_machine.events.clone().unwrap_or_default();
    let declared = events.iter().find(|e| e.key == event).expect(event);
    declared.payload_schema.clone().expect("payload")
}

#[test]
fn an_entity_field_typed_tag_id_is_a_string_projected_from_tag_id() {
    let field = note_field(&lowered(), "tagRef");
    assert_eq!(field.field_type, FieldType::String);
    assert_eq!(field.projected_from, projection("Tag", "id"));
}

#[test]
fn a_projected_closed_vocabulary_keeps_its_values() {
    let field = note_field(&lowered(), "tagKind");
    assert_eq!(field.values, Some(vec!["topic".to_string(), "person".to_string()]));
    assert_eq!(field.projected_from, projection("Tag", "kind"));
}

#[test]
fn a_payload_field_typed_tag_id_is_a_required_string_projected_from_tag_id() {
    let payload = payload_of(&lowered(), "SHOW");
    let tag_id = payload.iter().find(|f| f.name == "tagId").expect("tagId");
    assert_eq!(tag_id.field_type, "string");
    assert_eq!(tag_id.required, Some(true));
    assert_eq!(tag_id.entity, None);
    assert_eq!(tag_id.projected_from, projection("Tag", "id"));
}

#[test]
fn control_a_payload_field_typed_as_the_entity_stays_its_row() {
    let payload = payload_of(&lowered(), "SHOW");
    let row = payload.iter().find(|f| f.name == "row").expect("row");
    assert_eq!(row.field_type, "object");
    assert_eq!(row.entity.as_deref(), Some("Tag"));
    assert_eq!(row.projected_from, Some(FieldProjection { type_name: "Tag".into(), field: None }));
}

#[test]
fn a_config_knob_typed_tag_name_is_a_string_projected_from_tag_name() {
    let view = note_view(&lowered());
    let knob = view.config.as_ref().and_then(|c| c.get("defaultTag")).cloned().expect("defaultTag");
    assert_eq!(knob.field_type, "string");
    assert_eq!(knob.projected_from, projection("Tag", "name"));
}

#[test]
fn a_struct_type_member_typed_tag_id_is_projected_too() {
    let payload = payload_of(&lowered(), "PICKED");
    let pick = payload.iter().find(|f| f.name == "pick").expect("pick");
    let tag = pick.properties.as_ref().and_then(|p| p.iter().find(|f| f.name == "tag")).expect("tag");
    assert_eq!(tag.field_type, "string");
    assert_eq!(tag.projected_from, projection("Tag", "id"));
}

#[test]
fn edge_an_unknown_type_is_a_lowering_error_naming_it() {
    let err = parse_and_lower_str(&APP.replace("tagRef : Tag.id", "tagRef : Label.id")).expect_err("unknown type");
    assert!(err.to_string().contains("Label.id"), "{err}");
}

#[test]
fn edge_an_unknown_field_is_a_lowering_error_naming_it() {
    let err = parse_and_lower_str(&APP.replace("tagRef : Tag.id", "tagRef : Tag.slug")).expect_err("unknown field");
    assert!(err.to_string().contains("Tag.slug"), "{err}");
}

#[test]
fn the_formatter_round_trips_a_projection() {
    let formatted = format_program(&parse_str(APP).expect("parse"));
    assert!(formatted.contains("tagRef : Tag.id"), "{formatted}");
    assert!(formatted.contains("tagId : Tag.id!"), "{formatted}");
}


#[test]
fn named_source_type_reference_is_distinct_from_an_actual_field_projection() {
    let named: FieldProjection = serde_json::from_value(serde_json::json!({"type":"Tag"})).unwrap();
    assert_eq!(named.field, None);
    assert_eq!(serde_json::to_value(&named).unwrap(), serde_json::json!({"type":"Tag"}));
    let field: FieldProjection = serde_json::from_value(serde_json::json!({"type":"Tag","field":"id"})).unwrap();
    assert_eq!(field.field.as_deref(), Some("id"));
    for malformed in [serde_json::json!({"type":"Tag","field":""}), serde_json::json!({"type":"Tag","field":null})] {
        assert!(serde_json::from_value::<FieldProjection>(malformed).is_err());
    }
}
