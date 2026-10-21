//! §52 (C-RESOLVE-ATOM-MERGE-FIRST-ENTITY): multi-entity orbitals are
//! first-class — every declared entity is a legal relation target (forward
//! and cross-orbital), aux entity fields are type-checked, emit payloads may
//! name aux entities, intrinsic exemptions follow the trait's binding, and
//! the formatter round-trips extra entity blocks.

use orbital_compiler::{compile_schema, validate_comprehensive_without_services, CompilerOptions};
use orbital_core_base::ir::OirType;
use orbital_lolo::types::check_program;
use orbital_lolo::TypeError;
use orbital_lolo::{format_program, parse_and_lower_str, parse_str};
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
    assert_eq!(
        formatted, refmt,
        "formatter not idempotent on multi-entity orbital"
    );
}

fn keyed_rows_src(effect: &str) -> String {
    format!(
        r#"
orbital BoardOrbital {{
  entity Board [runtime] {{
    id : string!
    label : string
    byKey : Map string Asset = {{}}
    nested : Map string Map string Asset = {{}}
    lists : Map string [Asset] = {{}}
    packs : [Pack] = []
  }}
  entity Asset [runtime] {{
    id : string!
    url : string
  }}
  type Pack = {{ name : string  byKey : Map string Asset }}
  trait BoardTrait -> Board [interaction] {{
    initial: idle
    state idle {{
      INIT -> idle
        (set @entity.byKey {{}})
        (set @entity.nested {{}})
      PICK -> idle
        {effect}
    }}
  }}
  page "/board" -> BoardTrait
}}
"#
    )
}

fn keyed_rows_errors(effect: &str) -> Vec<String> {
    match parse_and_lower_str(&keyed_rows_src(effect)) {
        Err(e) => vec![format!("lower: {e}")],
        Ok(schema) => validate_comprehensive_without_services(&schema)
            .errors
            .iter()
            .map(|e| format!("{}: {}", e.code, e.message))
            .collect(),
    }
}

fn peel(ty: &OirType) -> &OirType {
    match ty {
        OirType::Optional { inner } => peel(inner),
        other => other,
    }
}

fn asset_row() -> OirType {
    OirType::row("Asset")
}

#[test]
fn map_of_entity_rows_validates_compiles_and_types_its_reads() {
    let read = r#"(set @entity.label (object/get (object/get @entity.byKey "k") url))"#;
    let errors = keyed_rows_errors(read);
    assert!(
        errors.is_empty(),
        "a keyed map of Asset rows with `{{}}` defaults validates: {errors:?}"
    );

    let schema = parse_and_lower_str(&keyed_rows_src(read)).expect("lowers");
    let mut options = CompilerOptions::default();
    options.inline.skip_inline = true;
    let module =
        compile_schema(schema, &options).expect("resolves without a relation-definition error");
    let board = module
        .entities
        .iter()
        .find(|e| e.name == "Board")
        .expect("Board");
    let field = |name: &str| {
        board
            .fields
            .iter()
            .find(|f| f.name == name)
            .map(|f| peel(&f.field_type).clone())
    };
    let map_of = |value: OirType| OirType::map(OirType::String, value);
    assert_eq!(field("byKey"), Some(map_of(asset_row())));
    assert_eq!(field("nested"), Some(map_of(map_of(asset_row()))));
    assert_eq!(field("lists"), Some(map_of(OirType::array(asset_row()))));
    let Some(OirType::Array { inner }) = field("packs") else {
        panic!("packs is a list")
    };
    let OirType::Struct { fields } = peel(&inner) else {
        panic!("Pack is a struct, got {inner:?}")
    };
    let member = fields
        .iter()
        .find(|f| f.name == "byKey")
        .expect("byKey member");
    assert_eq!(peel(&member.field_type), &map_of(asset_row()));
}

#[test]
fn a_read_of_a_field_the_entity_lacks_is_refused_through_the_map() {
    let errors = keyed_rows_errors(
        r#"(set @entity.label (object/get (object/get @entity.byKey "k") nope))"#,
    );
    assert!(errors.iter().any(|e| e.contains("nope")), "got {errors:?}");

    let nested =
        r#"(set @entity.label (object/get (object/get (object/get @entity.nested "a") "b") nope))"#;
    let errors = keyed_rows_errors(nested);
    assert!(errors.iter().any(|e| e.contains("nope")), "got {errors:?}");

    let control =
        r#"(set @entity.label (object/get (object/get (object/get @entity.nested "a") "b") url))"#;
    let errors = keyed_rows_errors(control);
    assert!(
        errors.is_empty(),
        "control: the nested read of a declared field validates: {errors:?}"
    );
}
