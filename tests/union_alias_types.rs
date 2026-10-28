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
    let [mood] = types.get("Mood").expect("a vocabulary alias is defined as a string").as_slice() else {
        panic!("a vocabulary alias is one unnamed entry");
    };
    assert_eq!(mood.field_type, "string", "control: a string vocabulary is an enum, not a union definition");
    assert_eq!(mood.values, Some(vec!["happy".to_string(), "sad".to_string()]));
    assert!(mood.properties.is_none());
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

const PRED: &str = r#"orbital PredOrbital {
  type Leaf = { kind : "leaf"!, label : string! }
  type Not = { kind : "not"!, of : Pred! }
  type All = { kind : "all"!, of : [Pred]! }
  type Pred = Leaf | Not | All

  entity PredState [runtime] { id : string! }

  trait PredDispatch -> PredState {
    listens { EVAL { pred : Pred! } }
    state idle { EVAL -> idle }
  }

  page "/pred" -> PredDispatch
}
"#;

fn field<'a>(
    fields: &'a [orbital_core_base::schema::PayloadField],
    name: &str,
) -> &'a orbital_core_base::schema::PayloadField {
    fields.iter().find(|f| f.name == name).unwrap_or_else(|| panic!("no field {name}"))
}

#[test]
fn a_recursive_union_alias_in_field_position_keeps_its_by_name_reference() {
    let types = types_of(PRED);
    let of = field(&types["Not"], "of");
    assert_eq!(of.field_type, "Pred");
    assert_eq!(of.properties.as_deref(), Some(&[][..]), "the by-name marker, not an expansion");
    assert_eq!(field(&types["All"], "of").items.as_ref().unwrap().field_type, "Pred");
    let pred = &types["Pred"][0];
    assert_eq!(pred.field_type, "union", "the alias itself still defines its variants");
    let not = pred.properties.as_ref().unwrap().iter().find(|v| v.name == "Not").unwrap();
    assert_eq!(field(not.properties.as_ref().unwrap(), "of").field_type, "Pred");
}

#[test]
fn a_non_recursive_union_alias_in_field_position_is_still_expanded() {
    let source = TAGGED.replace(
        "type DrawItem = DrawShape | DrawText",
        "type DrawItem = DrawShape | DrawText\n  type Holder = { item : DrawItem }",
    );
    let types = types_of(&source);
    let item = field(&types["Holder"], "item");
    assert_eq!(item.field_type, "union", "control: no back-reference, nothing to keep by name");
    assert!(item.properties.as_ref().is_some_and(|p| p.len() == 2));
}

#[test]
fn a_recursive_union_alias_round_trips_through_print_and_lower() {
    let first = parse_and_lower_str(PRED).expect("lowers");
    let printed = orbital_lolo::schema_to_lolo(&first).expect("prints");
    assert!(printed.contains("of : Pred!"), "{printed}");
    let second = parse_and_lower_str(&printed).expect("the printed source lowers");
    assert_eq!(first.orbitals[0].types, second.orbitals[0].types);
}

fn map_alias_program(details_type: &str) -> String {
    format!(
        r#"orbital DeployOrbital {{
  type DeploymentDetails = Map string (string | number)

  entity Deploy [runtime] {{ id : string! }}

  trait DeployFlow -> Deploy {{
    emits {{ DEPLOYED -> external {{ details : {details_type} }} }}
    listens {{ REPORT {{ details : {details_type} }} }}
    state idle {{ REPORT -> idle }}
  }}

  page "/deploy" -> DeployFlow
}}
"#
    )
}

#[test]
fn a_map_alias_payload_field_lowers_like_the_inline_map() {
    for spelled in ["DeploymentDetails", "Map string (string | number)"] {
        parse_and_lower_str(&map_alias_program(spelled)).unwrap_or_else(|e| panic!("{spelled}: {e}"));
    }
}

#[test]
fn a_map_alias_payload_field_names_the_alias_and_the_alias_is_defined() {
    let schema = parse_and_lower_str(&map_alias_program("DeploymentDetails")).expect("lowers");
    let types = schema.orbitals[0].types.clone().unwrap_or_default();
    assert!(types.contains_key("DeploymentDetails"));
    let json = serde_json::to_string(&schema).unwrap();
    assert!(json.contains("\"name\":\"details\",\"type\":\"object\",\"projectedFrom\":{\"type\":\"DeploymentDetails\"}"), "{json}");
}

#[test]
fn a_vocabulary_alias_is_still_refused_as_a_payload_field_type() {
    let source = map_alias_program("Mood").replace(
        "type DeploymentDetails = Map string (string | number)",
        "type Mood = happy | sad",
    );
    let err = parse_and_lower_str(&source).map(|_| ()).expect_err("a vocabulary keeps its enum reading");
    assert!(err.to_string().contains("type kind mismatch"), "{err}");
}
