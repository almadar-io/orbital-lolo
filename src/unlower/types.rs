//! Schema field/payload/config type descriptors back to `.lolo` type expressions.

use super::UnlowerError;
use crate::ast::{
    ConfigField, EntityField, EventField, MatchArm, MatchPattern, PayloadOfTarget, Span, TypeExpr,
};
use orbital_core_base::ir::types::{DeclaredShape, OirField, OirType};
use orbital_core_base::schema::{
    Cardinality, ConfigField as SchemaConfigField, FieldDefinition, FieldType, PayloadField,
};
use std::collections::HashMap;

thread_local! {
    static LEGACY_NULLABILITY: std::cell::Cell<bool> = const { std::cell::Cell::new(true) };
}

pub(super) struct PrintTypeContract(bool);
impl PrintTypeContract {
    pub(super) fn enter(version: Option<u32>) -> Self {
        Self(LEGACY_NULLABILITY.with(|mode| mode.replace(version.is_none())))
    }
}
impl Drop for PrintTypeContract {
    fn drop(&mut self) {
        LEGACY_NULLABILITY.with(|mode| mode.set(self.0));
    }
}

pub(super) fn printed_nullable(nullable: Option<bool>) -> bool {
    nullable.unwrap_or_else(|| LEGACY_NULLABILITY.with(std::cell::Cell::get))
}

fn source_reference(
    reference: &orbital_core_base::schema::FieldProjection,
    resolved: Option<TypeExpr>,
) -> TypeExpr {
    match &reference.field {
        Some(field) => TypeExpr::FieldOf {
            ty: reference.type_name.clone(),
            field: field.clone(),
            resolved: resolved.map(Box::new),
        },
        None => TypeExpr::Named(reference.type_name.clone()),
    }
}

fn field_record_type(record: Option<&str>) -> Result<TypeExpr, UnlowerError> {
    match record {
        Some(text) => parse_type_string(text),
        None => Err(UnlowerError::new("field reference without a record")),
    }
}

fn primitive(name: &str) -> TypeExpr {
    TypeExpr::Primitive(name.to_string())
}

fn sorted<'a, V: 'a>(
    map: impl IntoIterator<Item = (&'a String, &'a V)>,
) -> Vec<(&'a String, &'a V)> {
    let mut entries: Vec<_> = map.into_iter().collect();
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
    let base = field_def_type_inner(fd)?;
    Ok(if printed_nullable(fd.nullable) {
        TypeExpr::Optional(Box::new(base))
    } else {
        base
    })
}

fn field_def_type_inner(fd: &FieldDefinition) -> Result<TypeExpr, UnlowerError> {
    if let Some(p) = &fd.projected_from {
        return Ok(source_reference(p, None));
    }
    if let (FieldType::Object, None, Some(record)) = (&fd.field_type, &fd.properties, &fd.record) {
        return parse_type_string(record);
    }
    Ok(match &fd.field_type {
        FieldType::String => match &fd.values {
            Some(values) if !values.is_empty() => values_union(values),
            _ => primitive("string"),
        },
        FieldType::Enum => match &fd.values {
            Some(values) => values_union(values),
            None => primitive("string"),
        },
        FieldType::Number => primitive("number"),
        FieldType::Int => primitive("int"),
        FieldType::Boolean => primitive("boolean"),
        FieldType::Date => primitive("date"),
        FieldType::Datetime => primitive("datetime"),
        FieldType::Timestamp => primitive("timestamp"),
        FieldType::Instant => primitive("instant"),
        FieldType::Email => primitive("email"),
        FieldType::Url => primitive("url"),
        FieldType::Phone => primitive("phone"),
        FieldType::Uuid => primitive("uuid"),
        FieldType::Image => primitive("image"),
        FieldType::Money => primitive("money"),
        FieldType::File => primitive("file"),
        FieldType::Bytes => primitive("bytes"),
        FieldType::CallKey => primitive("CallKey"),
        FieldType::Node => TypeExpr::Named("node".to_string()),
        FieldType::SExpr => TypeExpr::SExpr,
        FieldType::Trait => TypeExpr::Trait,
        FieldType::Orbital => TypeExpr::Orbital,
        FieldType::Behavior => TypeExpr::Behavior,
        FieldType::Slot => TypeExpr::Slot,
        FieldType::Pattern => TypeExpr::Pattern,
        FieldType::Event => TypeExpr::EventRef,
        FieldType::EventAddress => TypeExpr::EventAddress,
        FieldType::Field => TypeExpr::FieldRef {
            record: Box::new(field_record_type(fd.record.as_deref())?),
            value: match &fd.items {
                Some(item) => Some(Box::new(field_def_type(item)?)),
                None => None,
            },
        },
        FieldType::TypeVar(v) => TypeExpr::Named(v.0.clone()),
        FieldType::Array => match &fd.items {
            Some(item) => TypeExpr::Array(Box::new(field_def_type(item)?)),
            None => TypeExpr::Array(Box::new(primitive("string"))),
        },
        FieldType::Object if fd.additional_properties.is_some() => TypeExpr::OpenObject {
            fields: fd
                .properties
                .as_ref()
                .map(object_members)
                .transpose()?
                .unwrap_or_default(),
            additional: Box::new(field_def_type(
                fd.additional_properties.as_deref().unwrap(),
            )?),
        },
        FieldType::Object => match (&fd.properties, &fd.items) {
            (Some(props), _) => match props
                .values()
                .find_map(back_reference)
                .or_else(|| program_type_name(fd))
            {
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
            // `(@entity <knob>)` lowers its target to the `@entity(<knob>)` sigil form.
            let target = if rel.entity.starts_with('@') {
                parse_type_string(&rel.entity)?
            } else {
                TypeExpr::Named(rel.entity.clone())
            };
            match rel.cardinality {
                Some(Cardinality::Many) => TypeExpr::Array(Box::new(target)),
                _ => target,
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
    props: &orbital_core_base::collections::IndexMap<String, FieldDefinition>,
) -> Result<Vec<(String, TypeExpr, bool)>, UnlowerError> {
    props
        .iter()
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
        derived: fd.derived.clone(),
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
        "behavior" => TypeExpr::Behavior,
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

/// A payload struct holding the lowerer's by-name back-reference (`properties:
/// Some([])` under a bare alias type) is that alias's own body: print the alias,
/// since an inline body would let a re-lower expand it one level deeper.
fn struct_or_alias(props: &[PayloadField]) -> Result<TypeExpr, UnlowerError> {
    for p in props {
        if p.properties.as_ref().is_some_and(Vec::is_empty) && p.projected_from.is_none() {
            match parse_type_string(&p.field_type)? {
                TypeExpr::Named(n) => return Ok(TypeExpr::Named(n)),
                TypeExpr::Array(inner) => {
                    if let TypeExpr::Named(n) = *inner {
                        return Ok(TypeExpr::Named(n));
                    }
                }
                _ => {}
            }
        }
    }
    Ok(TypeExpr::Object(payload_members(props)?))
}

fn payload_members(props: &[PayloadField]) -> Result<Vec<(String, TypeExpr, bool)>, UnlowerError> {
    props
        .iter()
        .map(|p| Ok((p.name.clone(), payload_type(p)?, p.required == Some(true))))
        .collect()
}

pub(super) fn payload_type(pf: &PayloadField) -> Result<TypeExpr, UnlowerError> {
    let base = payload_type_inner(pf)?;
    Ok(if printed_nullable(pf.nullable) {
        TypeExpr::Optional(Box::new(base))
    } else {
        base
    })
}

fn payload_type_inner(pf: &PayloadField) -> Result<TypeExpr, UnlowerError> {
    if let Some(p) = &pf.projected_from {
        return Ok(source_reference(p, None));
    }
    let base = if let Some(additional) = &pf.additional_properties {
        TypeExpr::OpenObject {
            fields: pf
                .properties
                .as_deref()
                .map(payload_members)
                .transpose()?
                .unwrap_or_default(),
            additional: Box::new(payload_type(additional)?),
        }
    } else if let Some(item) = &pf.items {
        let parsed = parse_type_string(&pf.field_type)?;
        match parsed {
            TypeExpr::Array(_) => TypeExpr::Array(Box::new(payload_type(item)?)),
            TypeExpr::Map(key, _) => TypeExpr::Map(key, Box::new(payload_type(item)?)),
            _ => payload_type_without_items(pf)?,
        }
    } else {
        payload_type_without_items(pf)?
    };
    match &pf.type_when {
        None => Ok(base),
        Some(conditions) => {
            let knob = conditions
                .first()
                .map(|c| c.knob.clone())
                .ok_or_else(|| UnlowerError::new("empty typeWhen"))?;
            let mut arms = conditions
                .iter()
                .map(|c| {
                    Ok(MatchArm {
                        pattern: MatchPattern::Str(c.equals.clone()),
                        ty: parse_type_string(&c.ty)?,
                        span: Span::dummy(),
                    })
                })
                .collect::<Result<Vec<_>, UnlowerError>>()?;
            arms.push(MatchArm {
                pattern: MatchPattern::Wildcard,
                ty: base,
                span: Span::dummy(),
            });
            Ok(TypeExpr::Match { knob, arms })
        }
    }
}

/// A union variant's type from its type string; the string cannot say whether a list or map variant's element
/// is nullable, so that comes from the variant's `items`.
fn variant_container(text: &str, variant: &PayloadField) -> Result<TypeExpr, UnlowerError> {
    let nullable = variant
        .items
        .as_deref()
        .is_some_and(|item| printed_nullable(item.nullable));
    let optional = |element: Box<TypeExpr>| match (nullable, *element) {
        (true, element @ TypeExpr::Optional(_)) | (false, element) => Box::new(element),
        (true, element) => Box::new(TypeExpr::Optional(Box::new(element))),
    };
    Ok(match parse_type_string(text)? {
        TypeExpr::Array(element) => TypeExpr::Array(optional(element)),
        TypeExpr::Map(key, element) => TypeExpr::Map(key, optional(element)),
        other => other,
    })
}

fn payload_type_without_items(pf: &PayloadField) -> Result<TypeExpr, UnlowerError> {
    Ok(match (pf.field_type.as_str(), &pf.properties, &pf.entity) {
        ("array", None, _) if pf.items.is_some() => match pf.items.as_deref() {
            Some(item) => TypeExpr::Array(Box::new(payload_type(item)?)),
            None => parse_type_string("array")?,
        },
        ("object", None, _) if pf.items.is_some() => match pf.items.as_deref() {
            Some(item) => TypeExpr::Map(
                Box::new(TypeExpr::Primitive("string".to_string())),
                Box::new(payload_type(item)?),
            ),
            None => parse_type_string("object")?,
        },
        ("object", Some(_), Some(entity)) => TypeExpr::Named(entity.clone()),
        ("[object]", Some(_), Some(entity)) => {
            TypeExpr::Array(Box::new(TypeExpr::Named(entity.clone())))
        }
        ("object", Some(props), None) => struct_or_alias(props)?,
        ("[object]", Some(props), None) => TypeExpr::Array(Box::new(struct_or_alias(props)?)),
        ("union", Some(variants), _) => TypeExpr::Union(
            variants
                .iter()
                .map(|v| {
                    let variant = match (v.name.as_str(), v.field_type.as_str(), &v.properties) {
                        ("array", "[object]", Some(props)) => {
                            Ok(TypeExpr::Array(Box::new(match v.items.as_deref() {
                                Some(item) if item.projected_from.is_some() => payload_type(item)?,
                                _ => TypeExpr::Object(payload_members(props)?),
                            })))
                        }
                        ("array", other, _) => variant_container(other, v),
                        ("object", "object", Some(props)) => {
                            Ok(TypeExpr::Object(payload_members(props)?))
                        }
                        (name, "object", Some(_)) => Ok(TypeExpr::Named(name.to_string())),
                        (_, other, _) => variant_container(other, v),
                    }?;
                    Ok(if v.nullable == Some(true) {
                        TypeExpr::Optional(Box::new(variant))
                    } else {
                        variant
                    })
                })
                .collect::<Result<_, UnlowerError>>()?,
        ),
        (other, None, _) if pf.values.is_some() => {
            let values = pf.values.as_deref().unwrap_or_default();
            let set = match values {
                [one] => TypeExpr::StringLiteral(one.clone()),
                many => TypeExpr::Union(
                    many.iter()
                        .map(|v| TypeExpr::StringLiteral(v.clone()))
                        .collect(),
                ),
            };
            if other.starts_with('[') {
                TypeExpr::Array(Box::new(set))
            } else {
                set
            }
        }
        (other, slots, _) => match (parse_type_string(other)?, slots) {
            (TypeExpr::Tuple(parts), Some(slots)) if parts.len() == slots.len() => TypeExpr::Tuple(
                slots
                    .iter()
                    .map(payload_type)
                    .collect::<Result<_, UnlowerError>>()?,
            ),
            (parsed, _) => parsed,
        },
    })
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
    if cf.wired {
        return Ok(ConfigField {
            name: name.to_string(),
            type_expr: TypeExpr::Primitive("unknown".to_string()),
            default: cf.default.clone(),
            label: None,
            description: None,
            synonyms: None,
            tier: None,
            required: false,
            event_refs: Vec::new(),
            span: Span::dummy(),
        });
    }
    // A call-site `key: value` override carries the parser's `"unknown"` sentinel, not a type.
    let base = if let Some(additional) = &cf.additional_properties {
        TypeExpr::OpenObject {
            fields: cf
                .properties
                .as_ref()
                .map(object_members)
                .transpose()?
                .unwrap_or_default(),
            additional: Box::new(field_def_type(additional)?),
        }
    } else if cf.field_type == "field" {
        TypeExpr::FieldRef {
            record: Box::new(field_record_type(cf.record.as_deref())?),
            value: match &cf.items {
                Some(item) => Some(Box::new(field_def_type(item)?)),
                None => None,
            },
        }
    } else if let ("[field]", Some(item)) = (cf.field_type.as_str(), &cf.items) {
        TypeExpr::Array(Box::new(field_def_type(item)?))
    } else if cf.field_type == "unknown" {
        TypeExpr::Primitive("unknown".to_string())
    } else if cf.field_type == "object" {
        match &cf.properties {
            Some(props) => TypeExpr::Object(object_members(props)?),
            None => primitive("object"),
        }
    } else {
        parse_type_string(&cf.field_type)?
    };
    let item_values = cf
        .items
        .as_ref()
        .and_then(|i| i.values.as_ref())
        .filter(|v| !v.is_empty());
    let with_values = match (&cf.values, &base, item_values) {
        (Some(values), TypeExpr::Primitive(p), _) if p == "string" => values_union(values),
        // A list of a closed string set (`["a" | "b"]`) keeps the set on its items.
        (_, TypeExpr::Array(inner), Some(values)) if matches!(inner.as_ref(), TypeExpr::Primitive(p) if p == "string") => {
            TypeExpr::Array(Box::new(values_union(values)))
        }
        _ => base,
    };
    let type_expr = match &cf.projected_from {
        Some(p) => source_reference(p, Some(with_values)),
        None => with_values,
    };
    let type_expr = if cf.field_type != "unknown" && printed_nullable(cf.nullable) {
        TypeExpr::Optional(Box::new(type_expr))
    } else {
        type_expr
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
    map: &Option<orbital_core_base::collections::IndexMap<String, SchemaConfigField>>,
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

fn sorted_keys<'a, V: 'a>(m: impl IntoIterator<Item = (&'a String, &'a V)>) -> Vec<String> {
    let mut k: Vec<String> = m.into_iter().map(|(k, _)| k.clone()).collect();
    k.sort();
    k
}

fn walk_field(fd: &FieldDefinition, out: &mut AliasShapes) -> Result<(), UnlowerError> {
    if let (Some(origin), Some(props)) = (&fd.projected_from, &fd.properties) {
        if origin.field.is_none() && !out.structs.contains_key(&origin.type_name) {
            out.structs
                .insert(origin.type_name.clone(), object_members(props)?);
        }
    }
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
/// The installed program type (`Viewer`, …) whose definition `fd` carries verbatim: a `Named` program type lowers
/// to a copy of its members, so the printer names it again instead of spelling the record inline.
fn program_type_name(fd: &FieldDefinition) -> Option<&'static str> {
    let shape = |f: &FieldDefinition| {
        serde_json::to_value((&f.field_type, &f.values, &f.items, &f.properties)).ok()
    };
    let own = shape(fd)?;
    orbital_core_base::language::program_types::program_type_fields()
        .iter()
        .find(|(_, def)| shape(def).as_ref() == Some(&own))
        .map(|(name, _)| name.as_str())
}

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
    td: &orbital_core_base::schema::TraitDefinition,
    out: &mut AliasShapes,
) {
    for e in td.emits.iter().flatten() {
        collect_payload_hints(e.payload_schema.as_deref().unwrap_or(&[]), out);
    }
    for ev in td.state_machine.events.iter().flatten() {
        collect_payload_hints(ev.payload_schema.as_deref().unwrap_or(&[]), out);
    }
}

/// The type a baked local annotation (`[name, value, "<declared shape>"]`) was written as: the alias the
/// shape names, else its type spelled back as a type expression.
pub(super) fn declared_shape_type(text: &str) -> Result<TypeExpr, UnlowerError> {
    let shape = DeclaredShape::read(text)
        .map_err(|e| UnlowerError::new(format!("local annotation shape: {e}")))?;
    match shape.name {
        Some(name) => Ok(TypeExpr::Named(name)),
        None => oir_type_expr(&shape.ty),
    }
}

fn oir_members(fields: &[OirField]) -> Result<Vec<(String, TypeExpr, bool)>, UnlowerError> {
    fields
        .iter()
        .map(|f| Ok((f.name.clone(), oir_type_expr(&f.field_type)?, f.required)))
        .collect()
}

fn oir_type_expr(ty: &OirType) -> Result<TypeExpr, UnlowerError> {
    Ok(match ty {
        OirType::String => primitive("string"),
        OirType::Number => primitive("number"),
        OirType::Integer => primitive("int"),
        OirType::Boolean => primitive("boolean"),
        OirType::Date => primitive("date"),
        OirType::DateTime => primitive("datetime"),
        OirType::Instant => primitive("instant"),
        OirType::Email => primitive("email"),
        OirType::Url => primitive("url"),
        OirType::Phone => primitive("phone"),
        OirType::Uuid => primitive("uuid"),
        OirType::Image => primitive("image"),
        OirType::Money => primitive("money"),
        OirType::File => primitive("file"),
        OirType::Bytes => primitive("bytes"),
        OirType::CallKey => primitive("CallKey"),
        OirType::Node => TypeExpr::Named("node".to_string()),
        OirType::SExpr => TypeExpr::SExpr,
        OirType::Trait => TypeExpr::Trait,
        OirType::Orbital => TypeExpr::Orbital,
        OirType::Behavior => TypeExpr::Behavior,
        OirType::Array { inner } => TypeExpr::Array(Box::new(oir_type_expr(inner)?)),
        OirType::Optional { inner } => TypeExpr::Optional(Box::new(oir_type_expr(inner)?)),
        OirType::Map { key, value } => TypeExpr::Map(
            Box::new(oir_type_expr(key)?),
            Box::new(oir_type_expr(value)?),
        ),
        OirType::Tuple { items } => {
            TypeExpr::Tuple(items.iter().map(oir_type_expr).collect::<Result<_, _>>()?)
        }
        OirType::Struct { fields, additional } => match additional {
            Some(additional) => TypeExpr::OpenObject {
                fields: oir_members(fields)?,
                additional: Box::new(oir_type_expr(additional)?),
            },
            None => TypeExpr::Object(oir_members(fields)?),
        },
        OirType::Union { variants } => TypeExpr::Union(
            variants
                .iter()
                .map(|v| oir_type_expr(&v.field_type))
                .collect::<Result<_, _>>()?,
        ),
        OirType::Enum { name } => {
            let words = orbital_core_base::type_compat::literal_union_members(name)
                .ok_or_else(|| UnlowerError::new(format!("local annotation enum `{name}`")))?;
            values_union(&words)
        }
        OirType::FieldRef { record, value } => TypeExpr::FieldRef {
            record: Box::new(oir_type_expr(record)?),
            value: match value {
                Some(value) => Some(Box::new(oir_type_expr(value)?)),
                None => None,
            },
        },
        OirType::Reference { entity: name } | OirType::TypeRef { name } | OirType::Var { name } => {
            TypeExpr::Named(name.clone())
        }
        OirType::Json | OirType::Row { .. } => {
            return Err(UnlowerError::new(format!(
                "a local annotation of type {ty:?} has no source spelling"
            )))
        }
    })
}
