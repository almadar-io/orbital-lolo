//! `(tree/fold value step Shape)` lowers to `(tree/fold value step Shape definition)`: `definition` is the
//! shape's `.orb` field definition as JSON text, built from the alias table so every recursive back-reference
//! keeps the by-name form `{type: union, values: [Shape]}` the kernel reads child positions from
//! (`orbital_core_eval` `operators::tree`). The orbital `types` section cannot serve: it expands one level.

use super::TypeAliasResolved;
use crate::ast::{Span, TypeExpr};
use crate::error::{LowerError, TypeError};
use orbital_core_base::ir::types::{field_shape, DeclaredShape, OirField, OirType};
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

fn field_definition(
    ty: &TypeExpr,
    aliases: &BTreeMap<String, TypeAliasResolved>,
    entity_names: &HashSet<String>,
    params: &[&str],
) -> Result<FieldDefinition, LowerError> {
    super::entity::lower_source_field(ty, aliases, entity_names, params)
}

fn no_shape(error: orbital_core_base::ir::types::ShapeError) -> LowerError {
    LowerError::Coded { code: "ELOLO_T_GENERIC_TYPE", message: error.to_string() }
}

fn source_type(
    ty: &TypeExpr,
    aliases: &BTreeMap<String, TypeAliasResolved>,
    entity_names: &HashSet<String>,
    params: &[&str],
) -> Result<OirType, LowerError> {
    Ok(match ty {
        TypeExpr::Optional(inner) => {
            OirType::optional(source_type(inner, aliases, entity_names, params)?)
        }
        TypeExpr::Array(inner) => {
            OirType::array(source_type(inner, aliases, entity_names, params)?)
        }
        TypeExpr::Map(key, value) => OirType::map(
            source_type(key, aliases, entity_names, params)?,
            source_type(value, aliases, entity_names, params)?,
        ),
        TypeExpr::Tuple(items) => OirType::Tuple {
            items: items
                .iter()
                .map(|item| source_type(item, aliases, entity_names, params))
                .collect::<Result<_, _>>()?,
        },
        TypeExpr::Object(fields) | TypeExpr::OpenObject { fields, .. } => OirType::Struct {
            additional: match ty {
                TypeExpr::OpenObject { additional, .. } => Some(Box::new(source_type(
                    additional,
                    aliases,
                    entity_names,
                    params,
                )?)),
                _ => None,
            },
            fields: fields
                .iter()
                .map(|(name, ty, required)| {
                    let mut field =
                        OirField::new(name, source_type(ty, aliases, entity_names, params)?);
                    field.required = *required;
                    Ok(field)
                })
                .collect::<Result<_, LowerError>>()?,
        },
        TypeExpr::Named(name) if params.contains(&name.as_str()) => {
            OirType::Var { name: name.clone().into() }
        }
        TypeExpr::Named(name) if entity_names.contains(name) => OirType::reference(name),
        TypeExpr::Named(name) if aliases.contains_key(name) => {
            OirType::TypeRef { name: name.clone() }
        }
        TypeExpr::FieldOf {
            resolved: Some(inner),
            ..
        } => source_type(inner, aliases, entity_names, params)?,
        TypeExpr::FieldOf { resolved: None, .. } => {
            return Err(LowerError::Internal(
                "a local type projection was not resolved".into(),
            ))
        }
        _ => {
            let def = field_definition(ty, aliases, entity_names, params)?;
            if let (
                TypeExpr::Union(parts),
                orbital_core_base::schema::FieldType::Union,
                Some(names),
            ) = (ty, &def.field_type, &def.values)
            {
                OirType::Union {
                    variants: names
                        .iter()
                        .zip(parts)
                        .map(|(name, part)| {
                            Ok(OirField::new(
                                name,
                                source_type(part, aliases, entity_names, params)?,
                            ))
                        })
                        .collect::<Result<_, LowerError>>()?,
                }
            } else if let Some(values) = def
                .values
                .as_deref()
                .and_then(orbital_core_base::type_compat::literal_union_text)
            {
                if def.field_type == orbital_core_base::schema::FieldType::String {
                    OirType::Enum { name: values }
                } else {
                    field_shape(&def).map_err(no_shape)?
                }
            } else {
                field_shape(&def).map_err(no_shape)?
            }
        }
    })
}

fn alias_type(
    name: &str,
    aliases: &BTreeMap<String, TypeAliasResolved>,
    entity_names: &HashSet<String>,
    params: &[&str],
) -> Result<Option<OirType>, LowerError> {
    if params.contains(&name) || entity_names.contains(name) {
        return Ok(None);
    }
    match aliases.get(name) {
        Some(TypeAliasResolved::Type(ty, _)) => {
            source_type(ty, aliases, entity_names, params).map(Some)
        }
        Some(TypeAliasResolved::Enum(_)) => {
            let def =
                field_definition(&TypeExpr::Named(name.into()), aliases, entity_names, params)?;
            Ok(Some(
                match def
                    .values
                    .as_deref()
                    .and_then(orbital_core_base::type_compat::literal_union_text)
                {
                    Some(values) => OirType::Enum { name: values },
                    None => field_shape(&def).map_err(no_shape)?,
                },
            ))
        }
        None => Ok(orbital_core_base::language::program_types::program_type(name).cloned()),
    }
}

pub(crate) fn declared_source_shape(
    ty: &TypeExpr,
    aliases: &BTreeMap<String, TypeAliasResolved>,
    entity_names: &HashSet<String>,
    params: &[&str],
) -> Result<DeclaredShape, LowerError> {
    let name = match ty {
        TypeExpr::Named(name) => Some(name.clone()),
        _ => None,
    };
    let root = match &name {
        Some(name) => match alias_type(name, aliases, entity_names, params)? {
            Some(ty) => ty,
            None => source_type(ty, aliases, entity_names, params)?,
        },
        None => source_type(ty, aliases, entity_names, params)?,
    };
    let error = std::cell::RefCell::new(None);
    let shape = DeclaredShape::with_resolver(name, root, |name| {
        match alias_type(name, aliases, entity_names, params) {
            Ok(shape) => Ok(shape),
            Err(found) => {
                error.borrow_mut().get_or_insert(found);
                Ok(None)
            }
        }
    });
    if let Some(error) = error.into_inner() {
        return Err(error);
    }
    let mut shape = shape.map_err(no_shape)?;
    shape.version = Some(1);
    Ok(shape)
}

fn alias_identity(
    name: &str,
    aliases: &BTreeMap<String, TypeAliasResolved>,
) -> Result<String, LowerError> {
    let mut current = name;
    let mut seen = Vec::new();
    loop {
        if seen.contains(&current) {
            let mut cycle = seen.into_iter().map(str::to_string).collect::<Vec<_>>();
            cycle.push(current.to_string());
            return Err(LowerError::Type(TypeError::CircularType {
                cycle,
                span: Span::default(),
            }));
        }
        seen.push(current);
        let Some(TypeAliasResolved::Type(ty, _)) = aliases.get(current) else {
            return Ok(current.to_string());
        };
        let mut ty = ty;
        while let TypeExpr::Optional(inner) = ty {
            ty = inner;
        }
        match ty {
            TypeExpr::Named(next) if aliases.contains_key(next) => current = next,
            _ => return Ok(current.to_string()),
        }
    }
}

fn canonical_references(
    def: &mut FieldDefinition,
    aliases: &BTreeMap<String, TypeAliasResolved>,
) -> Result<(), LowerError> {
    if let Some(name) = def.by_name_reference() {
        def.values = Some(vec![alias_identity(name, aliases)?]);
    }
    if let Some(item) = &mut def.items {
        canonical_references(item, aliases)?;
    }
    if let Some(properties) = &mut def.properties {
        for child in properties.values_mut() {
            canonical_references(child, aliases)?;
        }
    }
    Ok(())
}

fn definition(
    shape: &str,
    aliases: &BTreeMap<String, TypeAliasResolved>,
    entity_names: &HashSet<String>,
) -> Result<String, LowerError> {
    let mut def = field_definition(
        &TypeExpr::Named(shape.to_string()),
        aliases,
        entity_names,
        &[],
    )?;
    def.name = Some(alias_identity(shape, aliases)?);
    canonical_references(&mut def, aliases)?;
    let json = serde_json::to_value(&def).expect("a field definition serializes to JSON");
    Ok(sorted(json).to_string())
}

fn bake(
    node: &mut J,
    aliases: &BTreeMap<String, TypeAliasResolved>,
    entity_names: &HashSet<String>,
    span: Span,
    params: &[&str],
) -> Result<bool, LowerError> {
    let mut changed = false;
    match node {
        J::Array(items) => {
            if items.first().and_then(J::as_str) == Some("quote") {
                return Ok(false);
            }
            if items.first().and_then(J::as_str) == Some("let") {
                if let Some(J::Array(bindings)) = items.get_mut(1) {
                    for binding in bindings {
                        let Some(parts) = binding.as_array_mut() else {
                            continue;
                        };
                        if parts.len() == 3 && parts[2].is_object() {
                            let ty = serde_json::from_value::<TypeExpr>(parts[2].clone()).map_err(
                                |error| {
                                    LowerError::Internal(format!(
                                        "invalid source type descriptor: {error}"
                                    ))
                                },
                            )?;
                            let shape = declared_source_shape(&ty, aliases, entity_names, params)?;
                            parts[2] = J::String(
                                serde_json::to_string(&shape).expect("a declared shape serializes"),
                            );
                            changed = true;
                        }
                    }
                }
            }
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
                changed |= bake(item, aliases, entity_names, span, params)?;
            }
        }
        J::Object(map) => {
            for child in map.values_mut() {
                changed |= bake(child, aliases, entity_names, span, params)?;
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
    for reference in &mut orbital.traits {
        let orbital_core_base::schema::TraitReference::Inline(trait_def) = reference else {
            continue;
        };
        let names = trait_def
            .type_params
            .as_deref()
            .unwrap_or_default()
            .iter()
            .map(|param| param.name.clone())
            .collect::<Vec<_>>();
        if names.is_empty() {
            continue;
        }
        let params = names.iter().map(String::as_str).collect::<Vec<_>>();
        let mut tree = serde_json::to_value(&*trait_def).expect("a trait serializes to JSON");
        if bake(&mut tree, aliases, entity_names, span, &params)? {
            *trait_def = serde_json::from_value(tree).expect("a baked trait reads back");
        }
    }
    let mut tree = serde_json::to_value(&*orbital).expect("an orbital serializes to JSON");
    if bake(&mut tree, aliases, entity_names, span, &[])? {
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
            "app tf [\n  version: \"1.0.0\",\n  description: \"x\"\n]\n{types}\norbital O {{\n  entity E [runtime] {{\n    id : string!\n    out : string\n  }}\n  trait T -> E {{\n    initial: idle\n    state idle {{\n      INIT -> idle\n        (set @entity.out (tree/fold @payload.v (fn (n k) @n.label) {shape}))\n    }}\n  }}\n  page \"/\" -> T\n}}\n"
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
    fn an_alias_keeps_its_declared_identity_and_canonical_recursive_root() {
        let source = program(
            "type V = string | number | boolean | [V?]\ntype Alias = V",
            "Alias",
        );
        let schema = crate::parse_and_lower_str(&source).unwrap();
        let types = schema.orbitals[0].types.as_ref().unwrap();
        let alias = &types["Alias"];
        assert_eq!(alias.len(), 1);
        assert_eq!(alias[0].name, "");
        assert_eq!(alias[0].field_type, "V");
        assert_eq!(baked_definition(&source).name.as_deref(), Some("V"));
        assert_eq!(
            fold_call(&source)[3],
            "Alias",
            "authored diagnostic identity stays intact"
        );
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
            assert!(
                lolo.contains(&format!("(fn (n k) @n.label) {shape}))")),
                "{lolo}"
            );
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
            assert_eq!(
                crate::schema_to_lolo(&relowered).unwrap(),
                lolo,
                "byte-stable"
            );
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
            matches!(
                inner.innermost(),
                LowerError::Type(TypeError::CircularType { .. })
            ),
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
type Verdict = { f : boolean!, t : boolean!, lit : boolean!, s : string!, str : boolean!, j : string! }
orbital O {
  entity E [runtime] {
    id : string!
    out : Verdict
    count : number = 0
  }
  trait T -> E {
    initial: idle
    state idle {
      INIT -> idle
        (set @entity.out (tree/fold @payload.v (fn (n k)
          (if (validate/array @n)
            (let ((call (and (> (array/len @k) 0) (object/get (array/first @k) "str")))
                  (op (if (and (> (array/len @k) 0) (object/get (array/first @k) "str")) (object/get (array/first @k) "s") ""))
                  (args (array/map (array/drop @k 1) (fn c (if (validate/object @c) @c { f: true, t: false, lit: true, s: "", str: false, j: "null" })))))
              (if (not @call) { f: (= (array/len @k) 0), t: (> (array/len @k) 0), lit: false, s: "", str: false, j: "" }
              (if (= @op "and") { f: (array/some @args (fn a (object/get @a "f"))), t: (and (> (array/len @args) 0) (array/every @args (fn a (object/get @a "t")))), lit: false, s: "", str: false, j: "" }
              (if (= @op "or") { f: (and (> (array/len @args) 0) (array/every @args (fn a (object/get @a "f")))), t: (array/some @args (fn a (object/get @a "t"))), lit: false, s: "", str: false, j: "" }
              (if (= @op "not") { f: (and (= (array/len @args) 1) (object/get (array/first @args) "t")), t: (and (= (array/len @args) 1) (object/get (array/first @args) "f")), lit: false, s: "", str: false, j: "" }
              (if (or (= @op "=") (= @op "==") (= @op "!="))
                (let ((both (and (= (array/len @args) 2) (array/every @args (fn a (object/get @a "lit")))))
                      (eq (= (object/get (array/first @args) "j") (object/get (array/last @args) "j"))))
                  { f: (and @both (= @eq (= @op "!="))), t: (and @both (= @eq (!= @op "!="))), lit: false, s: "", str: false, j: "" })
                { f: false, t: false, lit: false, s: "", str: false, j: "" }))))))
            { f: (or (and (validate/boolean @n) (not @n)) (and (validate/number @n) (= @n 0)) (and (validate/string @n) (= @n ""))),
              t: (or (and (validate/boolean @n) @n) (and (validate/number @n) (!= @n 0)) (and (validate/string @n) (!= @n "") (not (str/startsWith @n (str/slice "x@" 1))))),
              lit: (or (validate/boolean @n) (validate/number @n) (and (validate/string @n) (not (str/startsWith @n (str/slice "x@" 1))))),
              s: (if (validate/string @n) @n ""),
              str: (validate/string @n),
              j: (json/stringify @n) }))
          Expr))
        (set @entity.count (tree/fold @payload.v (fn (n k) (if (validate/array @n) (+ 1 (array/sum @k)) 1)) Expr))
    }
  }
  page "/" -> T
}
"#;

    fn expr_folds() -> (
        orbital_core_base::schema::sexpr::SExpression,
        orbital_core_base::schema::sexpr::SExpression,
    ) {
        let lowered =
            serde_json::to_value(crate::parse_and_lower_str(EXPR_PROGRAM).expect("lowers"))
                .unwrap();
        let effects =
            &lowered["orbitals"][0]["traits"][0]["stateMachine"]["transitions"][0]["effects"];
        let call = |i: usize| {
            serde_json::from_value(effects[i][2].clone()).expect("the fold is an S-expression")
        };
        (call(0), call(1))
    }

    fn run_fold(
        call: &orbital_core_base::schema::sexpr::SExpression,
        v: orbital_core_base::value::Value,
    ) -> Result<orbital_core_base::value::Value, String> {
        let mut payload = orbital_core_base::value::ValueMap::new();
        payload.insert("v".into(), v);
        let ctx = orbital_core_eval::evaluator::BindingContextBuilder::new()
            .payload(orbital_core_base::value::Value::Object(payload.into()))
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
        assert_eq!(
            run_fold(&count, value(guard)).unwrap(),
            value(serde_json::json!(12))
        );
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
            let rust: orbital_core_base::schema::sexpr::SExpression =
                serde_json::from_value(case.clone()).unwrap();
            let verdict =
                run_fold(&dead, value(case.clone())).unwrap_or_else(|e| panic!("{case}: {e}"));
            let orbital_core_base::value::Value::Object(verdict) = verdict else {
                panic!("{case}: a verdict object");
            };
            assert_eq!(
                verdict.get("f"),
                Some(&orbital_core_base::value::Value::Bool(
                    rust.is_statically_false()
                )),
                "{case}"
            );
            assert_eq!(
                verdict.get("t"),
                Some(&orbital_core_base::value::Value::Bool(
                    rust.is_statically_true()
                )),
                "{case}"
            );
        }
    }

    #[test]
    fn control_a_flat_list_and_an_atom_fold_once_per_node() {
        let (_, count) = expr_folds();
        assert_eq!(
            run_fold(&count, value(serde_json::json!(["a", "b", "c"]))).unwrap(),
            value(serde_json::json!(4))
        );
        assert_eq!(
            run_fold(&count, value(serde_json::json!("a"))).unwrap(),
            value(serde_json::json!(1))
        );
        assert_eq!(
            run_fold(&count, value(serde_json::json!({ "kind": "x" }))).unwrap(),
            orbital_core_base::value::Value::Null,
            "control: an object is no variant of Expr, so the failable fold answers null (owner ruling 2026-10-25)"
        );
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
                    nested = Value::Array(
                        vec![Value::String(("not".to_string()).into()), nested].into(),
                    );
                }
                let out = run_fold(&count, nested);
                out.map(|v| match v {
                    Value::Number(n) => Some(n),
                    _ => None,
                })
            })
            .unwrap();
        assert_eq!(
            worker.join().expect("no stack overflow").unwrap(),
            Some(2001.0)
        );
    }
}
