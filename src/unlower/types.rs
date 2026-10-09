//! Schema field/payload/config type descriptors back to `.lolo` type expressions.

use super::UnlowerError;
use crate::ast::{
    ConfigField, EntityField, EventField, MatchArm, MatchPattern, PayloadOfTarget, Span, TypeExpr,
};
use orbital_core::schema::{
    Cardinality, ConfigField as SchemaConfigField, FieldDefinition, FieldType, PayloadField,
};
use std::collections::HashMap;

fn primitive(name: &str) -> TypeExpr {
    TypeExpr::Primitive(name.to_string())
}

fn sorted<V>(map: &HashMap<String, V>) -> Vec<(&String, &V)> {
    let mut entries: Vec<_> = map.iter().collect();
    entries.sort_by(|a, b| a.0.cmp(b.0));
    entries
}

fn values_union(values: &[String]) -> TypeExpr {
    if values.len() == 1 {
        TypeExpr::StringLiteral(values[0].clone())
    } else {
        TypeExpr::Union(
            values
                .iter()
                .cloned()
                .map(TypeExpr::StringLiteral)
                .collect(),
        )
    }
}

pub(super) fn field_def_type(fd: &FieldDefinition) -> Result<TypeExpr, UnlowerError> {
    if let Some(p) = &fd.projected_from {
        return Ok(TypeExpr::FieldOf {
            ty: p.type_name.clone(),
            field: p.field.clone(),
            resolved: None,
        });
    }
    Ok(match fd.field_type {
        FieldType::String => match &fd.values {
            Some(values) if !values.is_empty() => values_union(values),
            _ => primitive("string"),
        },
        FieldType::Enum => match &fd.values {
            Some(values) => values_union(values),
            None => primitive("string"),
        },
        FieldType::Number => primitive("number"),
        FieldType::Boolean => primitive("boolean"),
        FieldType::Date => primitive("date"),
        FieldType::Datetime => primitive("datetime"),
        FieldType::Timestamp => primitive("timestamp"),
        FieldType::Email => primitive("email"),
        FieldType::Url => primitive("url"),
        FieldType::Phone => primitive("phone"),
        FieldType::Uuid => primitive("uuid"),
        FieldType::Image => primitive("image"),
        FieldType::Money => primitive("money"),
        FieldType::File => primitive("file"),
        FieldType::Node => TypeExpr::Named("node".to_string()),
        FieldType::Scalar => primitive("scalar"),
        FieldType::SExpr => TypeExpr::SExpr,
        FieldType::Trait => TypeExpr::Trait,
        FieldType::Orbital => TypeExpr::Orbital,
        FieldType::Slot => TypeExpr::Slot,
        FieldType::Pattern => TypeExpr::Pattern,
        FieldType::Event => TypeExpr::EventRef,
        FieldType::EventAddress => TypeExpr::EventAddress,
        FieldType::Array => match &fd.items {
            Some(item) => TypeExpr::Array(Box::new(field_def_type(item)?)),
            None => TypeExpr::Array(Box::new(primitive("string"))),
        },
        FieldType::Object => match (&fd.properties, &fd.items) {
            (Some(props), _) => match props.values().find_map(back_reference) {
                Some(alias) => TypeExpr::Named(alias.to_string()),
                None => TypeExpr::Object(object_members(props)?),
            },
            (None, Some(value)) => TypeExpr::Map(
                Box::new(primitive("string")),
                Box::new(field_def_type(value)?),
            ),
            (None, None) => primitive("object"),
        },
        FieldType::Relation => {
            let rel = fd
                .relation
                .as_ref()
                .ok_or_else(|| UnlowerError::new("relation field without a relation target"))?;
            match rel.cardinality {
                Some(Cardinality::Many) => {
                    TypeExpr::Array(Box::new(TypeExpr::Named(rel.entity.clone())))
                }
                _ => TypeExpr::Named(rel.entity.clone()),
            }
        }
        FieldType::Tuple => TypeExpr::Tuple(
            fd.tuple_slots()
                .into_iter()
                .map(field_def_type)
                .collect::<Result<_, _>>()?,
        ),
        FieldType::Union => {
            let names = fd
                .values
                .as_ref()
                .filter(|v| !v.is_empty())
                .ok_or_else(|| UnlowerError::new("union field without variant names"))?;
            let part = |name: &String| -> Result<TypeExpr, UnlowerError> {
                match fd.properties.as_ref().and_then(|p| p.get(name)) {
                    None => Ok(TypeExpr::Named(name.clone())),
                    Some(v) if v.field_type == FieldType::Object && v.properties.is_some() => {
                        Ok(TypeExpr::Named(name.clone()))
                    }
                    Some(v) => field_def_type(v),
                }
            };
            if names.len() == 1 {
                part(&names[0])?
            } else {
                TypeExpr::Union(names.iter().map(part).collect::<Result<_, _>>()?)
            }
        }
    })
}

pub(super) fn object_members(
    props: &HashMap<String, FieldDefinition>,
) -> Result<Vec<(String, TypeExpr, bool)>, UnlowerError> {
    sorted(props)
        .into_iter()
        .map(|(k, v)| Ok((k.clone(), field_def_type(v)?, v.required == Some(true))))
        .collect()
}

pub(super) fn entity_field(fd: &FieldDefinition) -> Result<EntityField, UnlowerError> {
    let name = fd
        .name
        .clone()
        .ok_or_else(|| UnlowerError::new("entity field without a name"))?;
    Ok(EntityField {
        name,
        type_expr: field_def_type(fd)?,
        required: fd.required == Some(true),
        nullable: false,
        is_key: fd.primary_key == Some(true),
        is_unique: fd.unique == Some(true),
        is_intrinsic: fd.intrinsic == Some(true),
        description: fd.description.clone(),
        synonyms: fd.synonyms.clone(),
        mock: fd.mock.clone(),
        default: fd.default.clone(),
        derived: None,
        span: Span::dummy(),
    })
}

fn split_top_level(s: &str, sep: char) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut depth = 0i32;
    let mut start = 0;
    for (i, c) in s.char_indices() {
        match c {
            '[' | '<' | '(' => depth += 1,
            ']' | '>' | ')' => depth -= 1,
            c if c == sep && depth == 0 => {
                parts.push(&s[start..i]);
                start = i + c.len_utf8();
            }
            _ => {}
        }
    }
    parts.push(&s[start..]);
    parts
}

/// The inverse of `type_expr_to_config_string` / `type_expr_to_string`.
pub(super) fn parse_type_string(raw: &str) -> Result<TypeExpr, UnlowerError> {
    let s = raw.trim();
    if let Some(inner) = s.strip_prefix('[').and_then(|r| r.strip_suffix(']')) {
        let parts = split_top_level(inner, ',');
        return if parts.len() > 1 {
            Ok(TypeExpr::Tuple(
                parts
                    .into_iter()
                    .map(parse_type_string)
                    .collect::<Result<_, _>>()?,
            ))
        } else {
            Ok(TypeExpr::Array(Box::new(parse_type_string(inner)?)))
        };
    }
    if let Some(inner) = s.strip_prefix("Map<").and_then(|r| r.strip_suffix('>')) {
        let parts = split_top_level(inner, ',');
        if parts.len() == 2 {
            return Ok(TypeExpr::Map(
                Box::new(parse_type_string(parts[0])?),
                Box::new(parse_type_string(parts[1])?),
            ));
        }
        return Err(UnlowerError::new(format!("malformed map type `{raw}`")));
    }
    if let Some(rest) = s.strip_prefix('$') {
        return Ok(TypeExpr::Named(rest.to_string()));
    }
    if let Some(inner) = s
        .strip_prefix("@payload(")
        .and_then(|r| r.strip_suffix(')'))
    {
        return Ok(TypeExpr::EventPayloadOf(match inner.strip_prefix('.') {
            Some(member) => PayloadOfTarget::SiblingMember(member.to_string()),
            None => PayloadOfTarget::ConfigKnob(inner.to_string()),
        }));
    }
    if let Some(inner) = s.strip_prefix("@entity(").and_then(|r| r.strip_suffix(')')) {
        return Ok(TypeExpr::NamedEntityRow(inner.to_string()));
    }
    Ok(match s {
        "@entity" => TypeExpr::BoundEntityRow,
        "@user" => TypeExpr::ExpectedIdentityRow,
        "trait" => TypeExpr::Trait,
        "orbital" => TypeExpr::Orbital,
        "entity" => TypeExpr::EntityRef,
        "event" => TypeExpr::EventRef,
        "EventAddress" => TypeExpr::EventAddress,
        "slot" => TypeExpr::Slot,
        "pattern" => TypeExpr::Pattern,
        "secret" => TypeExpr::Secret,
        "SExpr" => TypeExpr::SExpr,
        name if crate::parser::is_primitive_type(name) => TypeExpr::Primitive(name.to_string()),
        name => TypeExpr::Named(name.to_string()),
    })
}

fn payload_members(props: &[PayloadField]) -> Result<Vec<(String, TypeExpr, bool)>, UnlowerError> {
    props
        .iter()
        .map(|p| Ok((p.name.clone(), payload_type(p)?, p.required == Some(true))))
        .collect()
}

pub(super) fn payload_type(pf: &PayloadField) -> Result<TypeExpr, UnlowerError> {
    if let Some(p) = &pf.projected_from {
        return Ok(TypeExpr::FieldOf {
            ty: p.type_name.clone(),
            field: p.field.clone(),
            resolved: None,
        });
    }
    let base = match (pf.field_type.as_str(), &pf.properties, &pf.entity) {
        ("object", Some(_), Some(entity)) => TypeExpr::Named(entity.clone()),
        ("[object]", Some(_), Some(entity)) => {
            TypeExpr::Array(Box::new(TypeExpr::Named(entity.clone())))
        }
        ("object", Some(props), None) => TypeExpr::Object(payload_members(props)?),
        ("[object]", Some(props), None) => {
            TypeExpr::Array(Box::new(TypeExpr::Object(payload_members(props)?)))
        }
        ("union", Some(variants), _) => TypeExpr::Union(
            variants
                .iter()
                .map(
                    |v| match (v.name.as_str(), v.field_type.as_str(), &v.properties) {
                        ("array", "[object]", Some(props)) => Ok(TypeExpr::Array(Box::new(
                            TypeExpr::Object(payload_members(props)?),
                        ))),
                        ("array", other, _) => parse_type_string(other),
                        (name, "object", Some(_)) => Ok(TypeExpr::Named(name.to_string())),
                        (_, other, _) => parse_type_string(other),
                    },
                )
                .collect::<Result<_, UnlowerError>>()?,
        ),
        (other, None, _) if pf.values.is_some() => {
            let values = pf.values.as_deref().unwrap_or_default();
            let set = match values {
                [one] => TypeExpr::StringLiteral(one.clone()),
                many => TypeExpr::Union(many.iter().map(|v| TypeExpr::StringLiteral(v.clone())).collect()),
            };
            if other.starts_with('[') {
                TypeExpr::Array(Box::new(set))
            } else {
                set
            }
        }
        (other, _, _) => parse_type_string(other)?,
    };
    match &pf.type_when {
        None => Ok(base),
        Some(conditions) => {
            let knob = conditions
                .first()
                .map(|c| c.knob.clone())
                .ok_or_else(|| UnlowerError::new("empty typeWhen"))?;
            let mut arms: Vec<MatchArm> = conditions
                .iter()
                .map(|c| {
                    Ok(MatchArm {
                        pattern: MatchPattern::Str(c.equals.clone()),
                        ty: parse_type_string(&c.ty)?,
                        span: Span::dummy(),
                    })
                })
                .collect::<Result<_, UnlowerError>>()?;
            arms.push(MatchArm {
                pattern: MatchPattern::Wildcard,
                ty: base,
                span: Span::dummy(),
            });
            Ok(TypeExpr::Match { knob, arms })
        }
    }
}

pub(super) fn event_fields(fields: &[PayloadField]) -> Result<Vec<EventField>, UnlowerError> {
    fields
        .iter()
        .map(|f| {
            Ok(EventField {
                name: f.name.clone(),
                type_expr: payload_type(f)?,
                required: f.required == Some(true),
                span: Span::dummy(),
            })
        })
        .collect()
}

pub(super) fn config_field(
    name: &str,
    cf: &SchemaConfigField,
) -> Result<ConfigField, UnlowerError> {
    let base = if cf.field_type == "object" {
        match &cf.properties {
            Some(props) => TypeExpr::Object(object_members(props)?),
            None => primitive("object"),
        }
    } else {
        parse_type_string(&cf.field_type)?
    };
    let with_values = match (&cf.values, &base) {
        (Some(values), TypeExpr::Primitive(p)) if p == "string" => values_union(values),
        _ => base,
    };
    let type_expr = match &cf.projected_from {
        Some(p) => TypeExpr::FieldOf {
            ty: p.type_name.clone(),
            field: p.field.clone(),
            resolved: Some(Box::new(with_values)),
        },
        None => with_values,
    };
    Ok(ConfigField {
        name: name.to_string(),
        type_expr,
        default: cf.default.clone(),
        label: cf.label.clone(),
        description: cf.description.clone(),
        synonyms: cf.synonyms.clone(),
        tier: cf.tier.clone(),
        required: cf.required == Some(true),
        event_refs: Vec::new(),
        span: Span::dummy(),
    })
}

pub(super) fn config_fields(
    map: &Option<HashMap<String, SchemaConfigField>>,
) -> Result<Vec<ConfigField>, UnlowerError> {
    match map {
        None => Ok(Vec::new()),
        Some(m) => sorted(m)
            .into_iter()
            .map(|(k, v)| config_field(k, v))
            .collect(),
    }
}

#[derive(Default)]
pub(super) struct AliasShapes {
    pub structs: HashMap<String, Vec<(String, TypeExpr, bool)>>,
    /// `type X = A | B` over struct aliases: the variant names.
    pub unions: HashMap<String, Vec<String>>,
    /// Names of the orbital's struct aliases (its `types` map).
    pub known_structs: std::collections::HashSet<String>,
    /// Entity names of the whole program (relation and payload targets).
    pub entities: std::collections::HashSet<String>,
    /// `(entity, field)` → the type string a payload carrying that entity recorded for it.
    pub entity_field_types: HashMap<(String, String), String>,
}

fn sorted_keys<V>(m: &HashMap<String, V>) -> Vec<String> {
    let mut k: Vec<String> = m.keys().cloned().collect();
    k.sort();
    k
}

fn walk_field(fd: &FieldDefinition, out: &mut AliasShapes) -> Result<(), UnlowerError> {
    if fd.field_type == FieldType::Union {
        for (name, variant) in fd.properties.iter().flatten() {
            if let Some(props) = &variant.properties {
                out.structs.insert(name.clone(), object_members(props)?);
            }
        }
    }
    if let Some(props) = &fd.properties {
        for member in props.values() {
            if let Some(n) = back_reference(member) {
                out.structs.insert(n.to_string(), object_members(props)?);
            }
        }
    }
    for p in fd.properties.iter().flat_map(|m| m.values()) {
        walk_field(p, out)?;
    }
    if let Some(item) = &fd.items {
        walk_field(item, out)?;
    }
    Ok(())
}

/// A by-name back-reference the lowerer leaves where an alias recurses: the
/// object that holds it is that alias's own body.
fn back_reference(fd: &FieldDefinition) -> Option<&str> {
    fn marker(f: &FieldDefinition) -> Option<&str> {
        match (&f.field_type, f.properties.as_ref(), f.values.as_deref()) {
            (FieldType::Union, None, Some([name])) => Some(name.as_str()),
            _ => None,
        }
    }
    marker(fd).or_else(|| fd.items.as_deref().and_then(marker))
}

fn element_shape(
    te: &TypeExpr,
    fd: &FieldDefinition,
    out: &mut AliasShapes,
) -> Result<(), UnlowerError> {
    match te {
        TypeExpr::Named(n) => {
            if fd.field_type == FieldType::Union {
                if let Some(values) = &fd.values {
                    out.unions.insert(n.clone(), values.clone());
                }
            } else if let Some(p) = &fd.properties {
                out.structs.insert(n.clone(), object_members(p)?);
            }
        }
        TypeExpr::Array(inner) | TypeExpr::Map(_, inner) => {
            if let Some(i) = &fd.items {
                element_shape(inner, i, out)?;
            }
        }
        _ => {}
    }
    walk_field(fd, out)
}

/// Struct aliases' member types as the entity/config definitions carry them —
/// richer than the payload-flattened `types` map (which erases literal unions) —
/// and the union aliases a field names but `types` omits.
pub(super) fn collect_config_shapes(
    cf: &SchemaConfigField,
    out: &mut AliasShapes,
) -> Result<(), UnlowerError> {
    let te = parse_type_string(&cf.field_type)?;
    match &te {
        TypeExpr::Named(n) => {
            if let Some(p) = &cf.properties {
                if p.keys().all(|k| out.known_structs.contains(k)) && !out.known_structs.contains(n)
                {
                    out.unions.insert(n.clone(), sorted_keys(p));
                    for (variant, vd) in p {
                        if let Some(vp) = &vd.properties {
                            out.structs.insert(variant.clone(), object_members(vp)?);
                        }
                    }
                } else {
                    out.structs.insert(n.clone(), object_members(p)?);
                }
            }
        }
        TypeExpr::Array(inner) | TypeExpr::Map(_, inner) => {
            if let Some(i) = &cf.items {
                element_shape(inner, i, out)?;
            }
        }
        _ => {}
    }
    for p in cf.properties.iter().flat_map(|m| m.values()) {
        walk_field(p, out)?;
    }
    Ok(())
}

pub(super) fn collect_field_shapes(
    fd: &FieldDefinition,
    out: &mut AliasShapes,
) -> Result<(), UnlowerError> {
    walk_field(fd, out)
}

fn collect_payload_hints(fields: &[PayloadField], out: &mut AliasShapes) {
    for f in fields {
        if let (Some(entity), Some(props)) = (&f.entity, &f.properties) {
            for p in props {
                out.entity_field_types
                    .entry((entity.clone(), p.name.clone()))
                    .or_insert_with(|| p.field_type.clone());
            }
        }
        if let Some(props) = &f.properties {
            collect_payload_hints(props, out);
        }
    }
}

pub(super) fn collect_trait_hints(
    td: &orbital_core::schema::TraitDefinition,
    out: &mut AliasShapes,
) {
    for e in td.emits.iter().flatten() {
        collect_payload_hints(e.payload_schema.as_deref().unwrap_or(&[]), out);
    }
    for ev in td.state_machine.events.iter().flatten() {
        collect_payload_hints(ev.payload_schema.as_deref().unwrap_or(&[]), out);
    }
}
