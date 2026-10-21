//! `(tree/fold value step Shape)` lowers to `(tree/fold value step Shape definition)`: `definition` is the
//! shape's `.orb` field definition as JSON text, built from the alias table so every recursive back-reference
//! keeps the by-name form `{type: union, values: [Shape]}` the kernel reads child positions from
//! (`orbital_core_eval` `operators::tree`). The orbital `types` section cannot serve: it expands one level.

use super::entity::lower_type_expr;
use super::TypeAliasResolved;
use crate::ast::{Span, TypeExpr};
use crate::error::{LowerError, TypeError};
use orbital_core_base::schema::{FieldDefinition, Orbital as CoreOrbital};
use serde_json::Value as J;
use std::collections::{BTreeMap, HashSet};

/// Object keys sorted at every depth, so the baked text is byte-stable (`properties` is a hash map).
fn sorted(value: J) -> J {
    match value {
        J::Object(map) => {
            let mut entries: Vec<(String, J)> = map.into_iter().collect();
            entries.sort_by(|a, b| a.0.cmp(&b.0));
            J::Object(entries.into_iter().map(|(k, v)| (k, sorted(v))).collect())
        }
        J::Array(items) => J::Array(items.into_iter().map(sorted).collect()),
        other => other,
    }
}

fn definition(
    shape: &str,
    aliases: &BTreeMap<String, TypeAliasResolved>,
    entity_names: &HashSet<String>,
) -> Result<String, LowerError> {
    let (field_type, values, items, properties, relation) =
        lower_type_expr(&TypeExpr::Named(shape.to_string()), aliases, entity_names, &[])?;
    let def = FieldDefinition {
        merged_from: None,
        name: Some(shape.to_string()),
        field_type,
        required: None,
        primary_key: None,
        unique: None,
        default: None,
        values,
        relation,
        items,
        properties,
        intrinsic: None,
        description: None,
        synonyms: None,
        mock: None,
        projected_from: None,
    };
    let json = serde_json::to_value(&def).expect("a field definition serializes to JSON");
    Ok(sorted(json).to_string())
}

fn bake(
    node: &mut J,
    aliases: &BTreeMap<String, TypeAliasResolved>,
    entity_names: &HashSet<String>,
    span: Span,
) -> Result<bool, LowerError> {
    let mut changed = false;
    match node {
        J::Array(items) => {
            if let [J::String(op), _, _, J::String(shape)] = items.as_slice() {
                if op == "tree/fold" {
                    if !aliases.contains_key(shape) {
                        let known: Vec<String> = aliases.keys().cloned().collect();
                        return Err(LowerError::Type(TypeError::UnknownType {
                            name: shape.clone(),
                            suggestion: crate::types::closest_name(shape, &known),
                            span,
                        }));
                    }
                    let text = definition(shape, aliases, entity_names)?;
                    items.push(J::String(text));
                    changed = true;
                }
            }
            for item in items.iter_mut() {
                changed |= bake(item, aliases, entity_names, span)?;
            }
        }
        J::Object(map) => {
            for child in map.values_mut() {
                changed |= bake(child, aliases, entity_names, span)?;
            }
        }
        _ => {}
    }
    Ok(changed)
}

/// Write the shape definition into every `(tree/fold value step Shape)` of `orbital`. A `Shape` no alias in scope
/// declares is `ORB_T_TYPE_UNKNOWN`.
pub(crate) fn bake_tree_fold_definitions(
    orbital: &mut CoreOrbital,
    aliases: &BTreeMap<String, TypeAliasResolved>,
    entity_names: &HashSet<String>,
    span: Span,
) -> Result<(), LowerError> {
    let mut tree = serde_json::to_value(&*orbital).expect("an orbital serializes to JSON");
    if bake(&mut tree, aliases, entity_names, span)? {
        *orbital = serde_json::from_value(tree).expect("a baked orbital reads back");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::error::{LoloError, LowerError, TypeError};
    use orbital_core_base::schema::{FieldDefinition, FieldType};
    use serde_json::Value as J;

    fn program(types: &str, shape: &str) -> String {
        format!(
            "app tf [\n  version: \"1.0.0\",\n  description: \"x\"\n]\n{types}\norbital O {{\n  entity E [runtime] {{\n    id : string!\n    out : string\n  }}\n  trait T -> E [interaction, instance] {{\n    initial: idle\n    state idle {{\n      INIT -> idle\n        (set @entity.out (tree/fold @payload.v (fn (n k) @n.label) {shape}))\n    }}\n  }}\n  page \"/\" -> T\n}}\n"
        )
    }

    const TREE: &str = "type Tree = { label : string!, kids : [Tree], parent : Tree, named : Map string Tree, tags : [string] }";
    const PREDICATE: &str = "type Leaf = { kind : \"leaf\"!, label : string! }\ntype Not = { kind : \"not\"!, of : Pred! }\ntype All = { kind : \"all\"!, of : [Pred]! }\ntype Pred = Leaf | Not | All";

    fn fold_call(source: &str) -> J {
        let lowered =
            serde_json::to_value(crate::parse_and_lower_str(source).expect("lowers")).unwrap();
        lowered["orbitals"][0]["traits"][0]["stateMachine"]["transitions"][0]["effects"][0][2]
            .clone()
    }

    fn baked_definition(source: &str) -> FieldDefinition {
        let call = fold_call(source);
        let items = call.as_array().expect("the fold is a call");
        assert_eq!(items.len(), 5, "the definition is appended: {call}");
        serde_json::from_str(items[4].as_str().expect("the definition is JSON text")).unwrap()
    }

    #[test]
    fn a_recursive_struct_bakes_its_child_positions_as_by_name_references() {
        let def = baked_definition(&program(TREE, "Tree"));
        assert_eq!(def.name.as_deref(), Some("Tree"));
        let props = def.properties.expect("a struct declares its fields");
        assert_eq!(
            props["kids"]
                .items
                .as_deref()
                .and_then(|i| i.by_name_reference()),
            Some("Tree")
        );
        assert_eq!(props["parent"].by_name_reference(), Some("Tree"));
        assert_eq!(
            props["named"]
                .items
                .as_deref()
                .and_then(|i| i.by_name_reference()),
            Some("Tree")
        );
        assert_eq!(
            props["tags"]
                .items
                .as_deref()
                .and_then(|i| i.by_name_reference()),
            None,
            "control"
        );
    }

    #[test]
    fn a_tagged_union_bakes_its_variants_with_by_name_back_references() {
        let def = baked_definition(&program(PREDICATE, "Pred"));
        assert!(matches!(def.field_type, FieldType::Union));
        let variants = def
            .properties
            .expect("a tagged union declares its variants");
        let not = variants["Not"].properties.as_ref().unwrap();
        assert_eq!(not["of"].by_name_reference(), Some("Pred"));
        let all = variants["All"].properties.as_ref().unwrap();
        assert_eq!(
            all["of"]
                .items
                .as_deref()
                .and_then(|i| i.by_name_reference()),
            Some("Pred")
        );
        assert!(variants["Leaf"].properties.as_ref().unwrap()["label"]
            .by_name_reference()
            .is_none());
    }

    #[test]
    fn the_baked_text_is_byte_stable() {
        let source = program(PREDICATE, "Pred");
        let first = fold_call(&source);
        for _ in 0..4 {
            assert_eq!(fold_call(&source), first);
        }
    }

    #[test]
    fn control_a_non_recursive_shape_is_baked_with_no_back_reference() {
        let def = baked_definition(&program(
            "type Note = { label : string!, tags : [string] }",
            "Note",
        ));
        let props = def.properties.unwrap();
        assert!(props.values().all(|p| p.by_name_reference().is_none()
            && p.items
                .as_deref()
                .and_then(|i| i.by_name_reference())
                .is_none()));
    }

    #[test]
    fn an_undeclared_shape_is_an_unknown_type() {
        let err = crate::parse_and_lower_str(&program(TREE, "Tre")).unwrap_err();
        let LoloError::Lower { inner, .. } = &err else {
            panic!("a lowering error, got {err:?}");
        };
        match inner.innermost() {
            LowerError::Type(TypeError::UnknownType {
                name, suggestion, ..
            }) => {
                assert_eq!(name, "Tre");
                assert_eq!(suggestion.as_deref(), Some("Tree"));
            }
            other => panic!("ORB_T_TYPE_UNKNOWN, got {other:?}"),
        }
    }

    #[test]
    fn unlowering_drops_the_definition_and_relowering_bakes_it_again() {
        for (types, shape) in [
            ("type Node = { label : string!, kids : [Node] }", "Node"),
            (PREDICATE, "Pred"),
        ] {
            let schema = crate::parse_and_lower_str(&program(types, shape)).expect("lowers");
            let lolo = crate::schema_to_lolo(&schema).expect("unlowers");
            assert!(lolo.contains(&format!("(fn (n k) @n.label) {shape}))")), "{lolo}");
            assert!(
                !lolo.contains("\\\"union\\\""),
                "the definition is not source: {lolo}"
            );
            let relowered = crate::parse_and_lower_str(&lolo)
                .unwrap_or_else(|e| panic!("relowers: {e}\n{lolo}"));
            assert_eq!(
                serde_json::to_value(&relowered).unwrap(),
                serde_json::to_value(&schema).unwrap(),
                "{lolo}"
            );
            assert_eq!(crate::schema_to_lolo(&relowered).unwrap(), lolo, "byte-stable");
        }
    }

    #[test]
    fn an_inline_union_back_into_a_variant_is_refused_not_overflowed() {
        let types = "type Leaf = { kind : \"leaf\"!, label : string! }\ntype Not = { kind : \"not\"!, of : Leaf | Not! }\ntype Pred = Leaf | Not";
        let err = crate::parse_and_lower_str(&program(types, "Pred")).unwrap_err();
        let LoloError::Lower { inner, .. } = &err else {
            panic!("a lowering error, got {err:?}");
        };
        assert!(
            matches!(inner.innermost(), LowerError::Type(TypeError::CircularType { .. })),
            "ORB_T_TYPE_CYCLE, got {err:?}"
        );
    }

    /// The `.lolo` port of `SExpression::is_statically_false` (plus a node count) over `type Expr`, the shape of
    /// an S-expression as data: atoms and lists of `Expr` (G-ORB-433).
    const EXPR_PROGRAM: &str = r#"app tf [
  version: "1.0.0",
  description: "x"
]
type Expr = string | number | boolean | [Expr]
type Verdict = { f : boolean!, t : boolean!, lit : boolean! }
orbital O {
  entity E [runtime] {
    id : string!
    out : Verdict
    count : number = 0
  }
  trait T -> E [interaction, instance] {
    initial: idle
    state idle {
      INIT -> idle
        (set @entity.out (tree/fold @payload.v (fn (n k)
          (if (validate/array @n)
            (let ((call (and (> (array/len @n) 0) (validate/string (array/first @n))))
                  (op (if (and (> (array/len @n) 0) (validate/string (array/first @n))) (array/first @n) ""))
                  (args (array/map (array/drop @k 1) (fn c (if (= @c null) { f: true, t: false, lit: true } @c)))))
              (if (not @call) { f: (= (array/len @n) 0), t: (> (array/len @n) 0), lit: false }
              (if (= @op "and") { f: (array/some @args (fn a (object/get @a "f"))), t: (and (> (array/len @args) 0) (array/every @args (fn a (object/get @a "t")))), lit: false }
              (if (= @op "or") { f: (and (> (array/len @args) 0) (array/every @args (fn a (object/get @a "f")))), t: (array/some @args (fn a (object/get @a "t"))), lit: false }
              (if (= @op "not") { f: (and (= (array/len @args) 1) (object/get (array/first @args) "t")), t: (and (= (array/len @args) 1) (object/get (array/first @args) "f")), lit: false }
              (if (or (= @op "=") (= @op "==") (= @op "!="))
                (let ((both (and (= (array/len @args) 2) (array/every @args (fn a (object/get @a "lit")))))
                      (eq (= (array/nth @n 1) (array/nth @n 2))))
                  { f: (and @both (= @eq (= @op "!="))), t: (and @both (= @eq (!= @op "!="))), lit: false })
                { f: false, t: false, lit: false }))))))
            { f: (or (= @n false) (and (validate/number @n) (= @n 0)) (= @n "")),
              t: (or (= @n true) (and (validate/number @n) (!= @n 0)) (and (validate/string @n) (!= @n "") (not (str/startsWith @n "@")))),
              lit: (or (validate/boolean @n) (validate/number @n) (and (validate/string @n) (not (str/startsWith @n "@")))) }))
          Expr))
        (set @entity.count (tree/fold @payload.v (fn (n k) (if (validate/array @n) (+ 1 (array/sum @k)) 1)) Expr))
    }
  }
  page "/" -> T
}
"#;

    fn expr_folds() -> (orbital_core_base::schema::sexpr::SExpression, orbital_core_base::schema::sexpr::SExpression) {
        let lowered = serde_json::to_value(crate::parse_and_lower_str(EXPR_PROGRAM).expect("lowers")).unwrap();
        let effects = &lowered["orbitals"][0]["traits"][0]["stateMachine"]["transitions"][0]["effects"];
        let call = |i: usize| serde_json::from_value(effects[i][2].clone()).expect("the fold is an S-expression");
        (call(0), call(1))
    }

    fn run_fold(
        call: &orbital_core_base::schema::sexpr::SExpression,
        v: orbital_core_base::value::Value,
    ) -> Result<orbital_core_base::value::Value, String> {
        let mut payload = orbital_core_base::value::ValueMap::new();
        payload.insert("v".to_string(), v);
        let ctx = orbital_core_eval::evaluator::BindingContextBuilder::new()
            .payload(orbital_core_base::value::Value::Object(payload))
            .build();
        orbital_core_eval::evaluator::Evaluator::new()
            .evaluate(call, &ctx)
            .map_err(|e| e.to_string())
    }

    fn value(json: J) -> orbital_core_base::value::Value {
        serde_json::from_value(json).unwrap()
    }

    #[test]
    fn a_nested_s_expression_folds_through_its_list_variant() {
        let (_, count) = expr_folds();
        let guard = serde_json::json!(["and", ["=", 1, 2], ["or", false, ["not", true]]]);
        assert_eq!(run_fold(&count, value(guard)).unwrap(), value(serde_json::json!(12)));
    }

    #[test]
    fn the_lolo_dead_guard_port_agrees_with_is_statically_false() {
        let (dead, _) = expr_folds();
        let cases = [
            serde_json::json!(["and", false, "@entity.x"]),
            serde_json::json!(["and", "@entity.x", true]),
            serde_json::json!(["or", false, false]),
            serde_json::json!(["or", false, "@entity.x"]),
            serde_json::json!(["or"]),
            serde_json::json!(["not", true]),
            serde_json::json!(["not", ["=", 1, 2]]),
            serde_json::json!(["not", "@entity.x"]),
            serde_json::json!(["=", 1, 2]),
            serde_json::json!(["==", "a", "a"]),
            serde_json::json!(["!=", "a", "a"]),
            serde_json::json!(["=", "@entity.a", 1]),
            serde_json::json!(["and", ["not", ["=", 1, 1]], "@entity.x"]),
            serde_json::json!(["or", ["and", false, true], ["!=", 2, 2]]),
            serde_json::json!(["foo", false]),
            serde_json::json!([]),
            serde_json::json!([1, 2]),
            serde_json::json!(false),
            serde_json::json!(true),
            serde_json::json!(0),
            serde_json::json!(3),
            serde_json::json!(""),
            serde_json::json!("@entity.x"),
            serde_json::json!("open"),
        ];
        for case in cases {
            let rust: orbital_core_base::schema::sexpr::SExpression = serde_json::from_value(case.clone()).unwrap();
            let verdict = run_fold(&dead, value(case.clone())).unwrap_or_else(|e| panic!("{case}: {e}"));
            let orbital_core_base::value::Value::Object(verdict) = verdict else {
                panic!("{case}: a verdict object");
            };
            assert_eq!(
                verdict.get("f"),
                Some(&orbital_core_base::value::Value::Bool(rust.is_statically_false())),
                "{case}"
            );
            assert_eq!(
                verdict.get("t"),
                Some(&orbital_core_base::value::Value::Bool(rust.is_statically_true())),
                "{case}"
            );
        }
    }

    #[test]
    fn control_a_flat_list_and_an_atom_fold_once_per_node() {
        let (_, count) = expr_folds();
        assert_eq!(run_fold(&count, value(serde_json::json!(["a", "b", "c"]))).unwrap(), value(serde_json::json!(4)));
        assert_eq!(run_fold(&count, value(serde_json::json!("a"))).unwrap(), value(serde_json::json!(1)));
        let err = run_fold(&count, value(serde_json::json!({ "kind": "x" }))).unwrap_err();
        assert!(err.contains("no variant"), "control: an object is no variant of Expr: {err}");
    }

    #[test]
    fn a_thousand_deep_s_expression_folds_on_a_small_stack() {
        use orbital_core_base::value::Value;
        let worker = std::thread::Builder::new()
            .stack_size(1024 * 1024)
            .spawn(|| {
                let (_, count) = expr_folds();
                let mut nested = Value::Bool(true);
                for _ in 0..1000 {
                    nested = Value::Array(vec![Value::String("not".to_string()), nested]);
                }
                let out = run_fold(&count, nested);
                out.map(|v| match v {
                    Value::Number(n) => Some(n),
                    _ => None,
                })
            })
            .unwrap();
        assert_eq!(worker.join().expect("no stack overflow").unwrap(), Some(2001.0));
    }
}
