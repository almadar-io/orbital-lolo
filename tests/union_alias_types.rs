//! A tagged-union alias (`type DrawItem = DrawShape | DrawText`) reaches `Orbital.types` as a definition of its
//! own, so a payload field typed by the alias name has something to resolve and a member read type-checks.

use orbital_lolo::parse_and_lower_str;

fn types_of(
    source: &str,
) -> std::collections::BTreeMap<String, Vec<orbital_core_base::schema::PayloadField>> {
    let schema = parse_and_lower_str(source).expect("lowers");
    schema.orbitals[0].types.clone().unwrap_or_default()
}

const TAGGED: &str = r#"orbital DrawOrbital {
  type DrawShape = { type : "draw-shape"!, width : number }
  type DrawText = { type : "draw-text"!, text : string }
  type DrawItem = DrawShape | DrawText
  type Mood = happy | sad

  entity DrawState [runtime] { id : string! }

  trait DrawDispatch -> DrawState {
    listens { DISPATCH { item : DrawItem!, mood : Mood } }
    state idle { INIT -> idle }
  }

  page "/draw" -> DrawDispatch
}
"#;

#[test]
fn a_tagged_union_alias_is_defined_in_types_with_its_variants() {
    let types = types_of(TAGGED);
    let [only] = types
        .get("DrawItem")
        .expect("the alias itself is defined")
        .as_slice()
    else {
        panic!("a union alias is one unnamed entry");
    };
    assert_eq!(only.name, "");
    assert_eq!(only.field_type, "union");
    let variants: Vec<&str> = only
        .properties
        .iter()
        .flatten()
        .map(|v| v.name.as_str())
        .collect();
    assert_eq!(variants, ["DrawShape", "DrawText"]);
    let shape = &only.properties.as_ref().unwrap()[0];
    let members: Vec<&str> = shape
        .properties
        .iter()
        .flatten()
        .map(|m| m.name.as_str())
        .collect();
    assert_eq!(members, ["type", "width"], "each variant keeps its members");
}

#[test]
fn the_variants_stay_defined_and_a_vocabulary_alias_is_not_a_union_definition() {
    let types = types_of(TAGGED);
    assert!(types.contains_key("DrawShape") && types.contains_key("DrawText"));
    assert!(
        !types.contains_key("Mood"),
        "control: a string vocabulary is an enum, not a union definition"
    );
}

#[test]
fn a_union_alias_survives_print_and_lower_again() {
    let first = parse_and_lower_str(TAGGED).expect("lowers");
    let printed = orbital_lolo::schema_to_lolo(&first).expect("prints");
    assert!(
        printed.contains("DrawItem = DrawShape | DrawText"),
        "the alias prints as written:\n{printed}"
    );
    let second = parse_and_lower_str(&printed).expect("the printed source lowers");
    assert_eq!(first.orbitals[0].types, second.orbitals[0].types);
}
