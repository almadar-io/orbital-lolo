//! §52 (C-RESOLVE-ATOM-MERGE-FIRST-ENTITY): multi-entity orbitals are
//! first-class — every declared entity is a legal relation target (forward
//! and cross-orbital), aux entity fields are type-checked, emit payloads may
//! name aux entities, intrinsic exemptions follow the trait's binding, and
//! the formatter round-trips extra entity blocks.

use orbital_lolo::{format_program, parse_and_lower_str, parse_str};
use orbital_lolo::types::check_program;
use orbital_lolo::TypeError;
use serde_json::Value;

fn lower_json(src: &str) -> Value {
    let schema = parse_and_lower_str(src).expect("parse+lower");
    serde_json::to_value(&schema).expect("serialize")
}

const FORWARD_REF_SRC: &str = r#"
orbital PostOrbital {
  entity Post [persistent: posts] {
    id : string!
    title : string!
    tagIds : [Tag]
  }
  entity Tag [persistent: tags] {
    id : string!
    name : string!
  }
  trait PostTrait -> Post [interaction] { state idle { INIT -> idle (ref Post) } }
  page "/posts" -> PostTrait
}
"#;

#[test]
fn forward_reference_to_later_sibling_entity_resolves() {
    let json = lower_json(FORWARD_REF_SRC);
    let orbital = &json["orbitals"][0];
    let fields = orbital["entity"]["fields"].as_array().expect("fields");
    let tag_ids = fields
        .iter()
        .find(|f| f["name"] == "tagIds")
        .expect("tagIds field");
    assert_eq!(tag_ids["type"], "relation", "got {tag_ids}");
    assert_eq!(tag_ids["relation"]["entity"], "Tag");
    assert_eq!(tag_ids["relation"]["cardinality"], "many");
    let aux = orbital["auxiliaryEntities"].as_array().expect("aux");
    assert_eq!(aux.len(), 1);
    assert_eq!(aux[0]["name"], "Tag");
}

#[test]
fn non_first_entity_resolves_as_cross_orbital_relation_target() {
    let src = r#"
orbital TaxonomyOrbital {
  entity Topic [persistent: topics] {
    id : string!
    name : string!
  }
  entity Label [persistent: labels] {
    id : string!
    name : string!
  }
  trait TopicTrait -> Topic [interaction] { state idle { INIT -> idle (ref Topic) } }
  page "/topics" -> TopicTrait
}

orbital ArticleOrbital {
  entity Article [persistent: articles] {
    id : string!
    title : string!
    labelIds : [Label]
    primaryLabel : Label
  }
  trait ArticleTrait -> Article [interaction] { state idle { INIT -> idle (ref Article) } }
  page "/articles" -> ArticleTrait
}
"#;
    let json = lower_json(src);
    let fields = json["orbitals"][1]["entity"]["fields"]
        .as_array()
        .expect("fields");
    let many = fields.iter().find(|f| f["name"] == "labelIds").unwrap();
    assert_eq!(many["type"], "relation", "got {many}");
    assert_eq!(many["relation"]["entity"], "Label");
    assert_eq!(many["relation"]["cardinality"], "many");
    let one = fields.iter().find(|f| f["name"] == "primaryLabel").unwrap();
    assert_eq!(one["type"], "relation", "got {one}");
    assert_eq!(one["relation"]["cardinality"], "one");
}

#[test]
fn aux_entity_unknown_field_type_is_rejected() {
    let src = r#"
orbital PostOrbital {
  entity Post [persistent: posts] {
    id : string!
    title : string!
  }
  entity Tag [persistent: tags] {
    id : string!
    kind : Bogus
  }
  trait PostTrait -> Post [interaction] { state idle { INIT -> idle (ref Post) } }
  page "/posts" -> PostTrait
}
"#;
    let program = parse_str(src).expect("parse");
    let err = check_program(&program).unwrap_err();
    assert!(matches!(err, TypeError::UnknownType { .. }), "got {err:?}");
}

#[test]
fn emit_payload_may_name_aux_entity() {
    let src = r#"
orbital PostOrbital {
  entity Post [persistent: posts] {
    id : string!
    title : string!
  }
  entity Tag [persistent: tags] {
    id : string!
    name : string!
  }
  trait PostTrait -> Post [interaction] {
    state idle { INIT -> idle (ref Post) }
    emits {
      TAG_PICKED { tag : Tag }
    }
  }
  page "/posts" -> PostTrait
}
"#;
    parse_and_lower_str(src).expect("aux entity name legal in emit payload");
}

#[test]
fn intrinsic_exemption_follows_trait_binding() {
    let src = r#"
orbital PickOrbital {
  entity Post [persistent: posts] {
    id : string!
    title : string!
  }
  entity Tag [persistent: tags] {
    id : string!
    name : string!
    sel : string @intrinsic
  }
  trait TagPick -> @rebindable Tag [interaction] {
    state idle {
      PICK -> idle
        (set @name @entity.sel)
      SELECT -> idle
        (set @sel ?value)
    }
  }
  trait PostTrait -> Post [interaction] { state idle { INIT -> idle (ref Post) } }
  page "/posts" -> PostTrait
}
"#;
    let json = lower_json(src);
    let traits = json["orbitals"][0]["traits"].as_array().expect("traits");
    let pick = traits.iter().find(|t| t["name"] == "TagPick").unwrap();
    let contract = &pick["entityContract"];
    let requires: Vec<&str> = contract["requires"]
        .as_array()
        .map(|a| a.iter().filter_map(|v| v.as_str()).collect())
        .unwrap_or_default();
    assert!(
        !requires.contains(&"sel"),
        "aux-bound trait must exempt the aux entity's @intrinsic field, requires={requires:?}"
    );
}

#[test]
fn multi_entity_orbital_formatter_round_trips() {
    let program = parse_str(FORWARD_REF_SRC).expect("parse");
    let formatted = format_program(&program);
    assert!(
        formatted.contains("entity Tag"),
        "extra entity dropped by formatter:\n{formatted}"
    );
    let reparsed = parse_str(&formatted).expect("reparse formatted");
    assert_eq!(reparsed.orbitals[0].extra_entities.len(), 1);
    let refmt = format_program(&reparsed);
    assert_eq!(formatted, refmt, "formatter not idempotent on multi-entity orbital");
}
