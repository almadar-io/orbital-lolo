// `[T1, T2, …]` — a fixed-length tuple type (two or more slots; `[T]` stays an array).
// Canvas lighting's `position : [number, number, number]` needed it: as `[number]` the
// compiled config type was `number[]`, not the component's tuple.

use orbital_lolo::parse_and_lower_str;
use serde_json::Value;

fn program(member: &str, knob: &str, default: &str) -> String {
    format!(
        r#"app Probe
orbital LightOrbital {{
  type Light = {{
    position : {member}
  }}
  entity Lamp [runtime] {{
    id : string!
  }}
  trait LampView -> Lamp [interaction] {{
    config {{
      light : Light = {{ position: [1, 2, 3] }}
      pos : {knob} = {default}
    }}
    initial: idle
    state idle {{
      INIT -> idle
        (render-ui main {{ type: "stack", a: @config.light, b: @config.pos }})
    }}
  }}
  page "/" -> LampView [initial]
}}"#
    )
}

fn config(src: &str) -> Value {
    let schema = parse_and_lower_str(src).expect("program lowers");
    let json = serde_json::to_value(schema).expect("schema serializes");
    json["orbitals"][0]["traits"][0]["config"].clone()
}

#[test]
fn a_tuple_member_and_knob_lower_as_tuple_types() {
    let c = config(&program("[number, number, number]", "[number, number]", "[4, 5]"));
    let position = &c["light"]["properties"]["position"];
    assert_eq!(position["type"], "tuple", "{c}");
    assert_eq!(position["properties"].as_object().map(|o| o.len()), Some(3), "{c}");
    for slot in ["0", "1", "2"] {
        assert_eq!(position["properties"][slot]["type"], "number", "{c}");
        assert_eq!(position["properties"][slot]["name"], slot, "a slot is a field, named by its index: {c}");
    }
    assert_eq!(c["pos"]["type"], "[number, number]", "{c}");
    assert_eq!(c["pos"]["default"], serde_json::json!([4.0, 5.0]), "{c}");
}

#[test]
fn control_a_single_slot_stays_an_array() {
    let c = config(&program("[number]", "[number]", "[4, 5, 6]"));
    assert_eq!(c["light"]["properties"]["position"]["type"], "array", "{c}");
    assert_eq!(c["light"]["properties"]["position"]["items"]["type"], "number", "{c}");
    assert_eq!(c["pos"]["type"], "[number]", "{c}");
}

// A default's arity and slot types are checked by `orb validate`
// (orbital-compiler `config_type::tuple_shape_tests`), where config defaults are checked.

fn tuple_json_schema(declarations: &str) -> orbital_core_base::schema::OrbitalSchema {
    parse_and_lower_str(&format!(r#"app TupleJson
{declarations}
orbital Probe {{
 entity Row [runtime] {{
 id : string!
 result : string = ""
 }}
 trait Check -> Row {{
  initial: ready
  state ready {{
   CHECK -> ready
    (set @entity.result (object/get (array/first (object/get (json/parse "{{\"entries\":[[\"entry\",{{\"repo\":\"active/core\"}}]]}}" Baseline) "entries")) "1.repo"))
  }}
 }}
}}"#)).expect("tuple JSON program lowers")
}

fn captured_tuple_shape(declarations: &str) -> orbital_core_base::ir::types::DeclaredShape {
    let schema = tuple_json_schema(declarations);
    orbital_core_base::ir::types::DeclaredShape::of_alias(
        "Baseline",
        schema.orbitals[0].types.as_ref().unwrap(),
    )
    .unwrap()
}

fn parse_with_shape(
    shape: &orbital_core_base::ir::types::DeclaredShape,
    value: Value,
) -> orbital_core_base::value::Value {
    let expr = serde_json::from_value(serde_json::json!([
        "json/parse",
        value.to_string(),
        serde_json::to_string(shape).unwrap()
    ]))
    .unwrap();
    orbital_core_eval::evaluator::eval_expr(
        &expr,
        &orbital_core_eval::evaluator::context::BindingContext::new(),
    )
    .unwrap()
}

#[test]
fn tuple_json_shapes_keep_named_inline_and_nested_record_members() {
    for declarations in [
        "type Entry = [string, { repo : string!, optional : number }]\ntype Baseline = { entries : [Entry]! }",
        "type Baseline = { entries : [[string, { repo : string!, optional : number }]]! }",
    ] {
        let shape = captured_tuple_shape(declarations);
        let valid = serde_json::json!({"entries":[["entry",{"repo":"active/core"}]]});
        assert!(!parse_with_shape(&shape, valid).is_null(), "{declarations}: {shape:?}");
        for invalid in [serde_json::json!({"entries":[["entry",{}]]}),serde_json::json!({"entries":[["entry",{"repo":3}]]}),serde_json::json!({"entries":[["entry"]]}),serde_json::json!({"entries":[["entry",{"repo":"active/core"},"extra"]]})] {
            assert!(parse_with_shape(&shape,invalid).is_null(), "required members and tuple arity remain checked: {shape:?}");
        }
    }
    let nested = captured_tuple_shape(
        "type Baseline = { entries : [[string, { nested : [number, { repo : string! }]! }]]! }",
    );
    assert!(!parse_with_shape(
        &nested,
        serde_json::json!({"entries":[["entry",{"nested":[1,{"repo":"core"}]}]]})
    )
    .is_null());
    assert!(parse_with_shape(
        &nested,
        serde_json::json!({"entries":[["entry",{"nested":[1,{}]}]]})
    )
    .is_null());
}

#[test]
fn recursive_tuple_aliases_remain_finite_and_check_nested_rows() {
    let shape = captured_tuple_shape(
        "type Entry = [string, { children : [Entry]! }]\ntype Baseline = { entries : [Entry]! }",
    );
    assert!(
        shape.named.contains_key("Entry"),
        "tuple alias must be captured: {shape:?}"
    );
    assert!(!parse_with_shape(
        &shape,
        serde_json::json!({"entries":[["root",{"children":[["leaf",{"children":[]}]]}]]})
    )
    .is_null());
    assert!(parse_with_shape(
        &shape,
        serde_json::json!({"entries":[["root",{"children":[["leaf",{}]]}]]})
    )
    .is_null());
}

#[test]
fn tuple_record_paths_are_inferred_without_accepting_missing_fields() {
    for declarations in [
        "type Entry = [string, { repo : string! }]\ntype Baseline = { entries : [Entry]! }",
        "type Baseline = { entries : [[string, { repo : string! }]]! }",
    ] {
        let schema = tuple_json_schema(declarations);
        let verdict = orbital_compiler::validate_comprehensive_without_services(&schema);
        let issues: Vec<_> = verdict
            .all_issues()
            .filter(|i| {
                i.code == orbital_compiler::phases::validation::ErrorCode::OperatorArgTypeMismatch
            })
            .collect();
        assert!(issues.is_empty(), "supported tuple field: {issues:?}");
        let mut bad = serde_json::to_string(&schema).unwrap();
        bad = bad.replace("1.repo", "1.missing");
        let verdict = orbital_compiler::validate_comprehensive_without_services(
            &serde_json::from_str(&bad).unwrap(),
        );
        assert!(
            verdict.all_issues().any(|i| i.code
                == orbital_compiler::phases::validation::ErrorCode::OperatorArgTypeMismatch),
            "unknown tuple member still rejected"
        );
    }
}

#[test]
fn inline_event_tuple_members_use_the_same_structural_capture() {
    let schema = parse_and_lower_str(
        r#"app PayloadTuple
orbital Probe {
 entity Row [runtime] { id : string! }
 trait Check -> Row {
  initial: ready
  state ready { INIT -> ready }
  emits { RESULT { entry : [string, { repo : string! }]! } }
 }
}"#,
    )
    .unwrap();
    let orbital_core_base::schema::TraitReference::Inline(trait_def) =
        &schema.orbitals[0].traits[0]
    else {
        panic!("inline trait")
    };
    let field = &trait_def.emits.as_ref().unwrap()[0]
        .payload_schema
        .as_ref()
        .unwrap()[0];
    let orbital_core_base::ir::types::OirType::Tuple { items } =
        orbital_core_base::ir::types::payload_field_shape(field)
    else {
        panic!("tuple field")
    };
    let orbital_core_base::ir::types::OirType::Struct { fields } = &items[1] else {
        panic!("tuple record")
    };
    assert_eq!(fields[0].name, "repo");
    assert!(fields[0].required);
}
