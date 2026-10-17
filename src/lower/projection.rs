//! `T.f` resolution: every `TypeExpr::FieldOf` in the program gets `T`'s declared type of
//! field `f` (an entity's field or a struct type's member), before any lowering reads a type.

use crate::ast::{
    Emit, Entity, EntityField, EventField, ExpectsKind, Listen, Orbital, Program, Trait, TypeAlias,
    TypeBody, TypeExpr,
};
use crate::error::LowerError;
use std::collections::HashMap;

/// The declared fields of every entity and struct type, by type name.
type Declared = HashMap<String, Vec<HashMap<String, TypeExpr>>>;

pub(crate) fn resolve_field_projections(program: &Program) -> Result<Program, LowerError> {
    let declared = declared_types(program);
    let mut out = program.clone();
    let file_aliases: Vec<String> = program.uses.iter().map(|u| u.alias.clone()).collect();
    for orbital in &mut out.orbitals {
        qualify_alias_names(orbital, &file_aliases, &declared)?;
    }
    let mut visit = |t: &mut TypeExpr| resolve(t, &declared, &mut Vec::new());
    for alias in &mut out.type_aliases {
        visit_alias(alias, &mut visit)?;
    }
    for orbital in &mut out.orbitals {
        visit_orbital(orbital, &mut visit)?;
    }
    Ok(out)
}

fn declared_types(program: &Program) -> Declared {
    let mut declared: Declared = HashMap::new();
    let mut add = |name: &str, fields: HashMap<String, TypeExpr>| {
        declared.entry(name.to_string()).or_default().push(fields);
    };
    let entity_fields = |fields: &[EntityField]| -> HashMap<String, TypeExpr> {
        fields.iter().map(|f| (f.name.clone(), f.type_expr.clone())).collect()
    };
    let alias_fields = |alias: &TypeAlias| -> Option<HashMap<String, TypeExpr>> {
        match &alias.body {
            TypeBody::Struct(members) => {
                Some(members.iter().map(|(n, t, _)| (n.clone(), t.clone())).collect())
            }
            TypeBody::Entity { fields, .. } => Some(entity_fields(fields)),
            _ => None,
        }
    };
    let aliases = program
        .type_aliases
        .iter()
        .chain(program.orbitals.iter().flat_map(|o| o.type_aliases.iter()));
    for alias in aliases {
        if let Some(fields) = alias_fields(alias) {
            add(&alias.name, fields);
        }
    }
    for orbital in &program.orbitals {
        for entity in orbital.entity.iter().chain(orbital.extra_entities.iter()) {
            add(&entity.name, entity_fields(&entity.fields));
        }
    }
    declared
}

fn resolve(t: &mut TypeExpr, declared: &Declared, stack: &mut Vec<String>) -> Result<(), LowerError> {
    match t {
        TypeExpr::FieldOf { ty, field, resolved } => {
            let projection = format!("{ty}.{field}");
            let error = |reason: String| LowerError::FieldProjection { projection: projection.clone(), reason };
            let decls = declared
                .get(ty.as_str())
                .ok_or_else(|| error(format!("no entity or struct type named `{ty}` is declared")))?;
            let [fields] = decls.as_slice() else {
                return Err(error(format!("`{ty}` is declared {} times", decls.len())));
            };
            let mut inner = fields
                .get(field.as_str())
                .cloned()
                .ok_or_else(|| error(format!("`{ty}` declares no field `{field}`")))?;
            if stack.contains(&projection) {
                return Err(error(format!("projects itself through {}", stack.join(" → "))));
            }
            stack.push(projection);
            resolve(&mut inner, declared, stack)?;
            stack.pop();
            *resolved = Some(Box::new(inner));
            Ok(())
        }
        TypeExpr::Array(inner) | TypeExpr::Optional(inner) => resolve(inner, declared, stack),
        TypeExpr::Map(key, value) => {
            resolve(key, declared, stack)?;
            resolve(value, declared, stack)
        }
        TypeExpr::Object(members) => {
            for (_, member, _) in members {
                resolve(member, declared, stack)?;
            }
            Ok(())
        }
        TypeExpr::Union(parts) => {
            for part in parts {
                resolve(part, declared, stack)?;
            }
            Ok(())
        }
        TypeExpr::Match { arms, .. } => {
            for arm in arms {
                resolve(&mut arm.ty, declared, stack)?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

type Visit<'a> = dyn FnMut(&mut TypeExpr) -> Result<(), LowerError> + 'a;

fn visit_alias(alias: &mut TypeAlias, visit: &mut Visit<'_>) -> Result<(), LowerError> {
    match &mut alias.body {
        TypeBody::Struct(members) => members.iter_mut().try_for_each(|(_, t, _)| visit(t)),
        TypeBody::Alias(t) => visit(t),
        TypeBody::Entity { fields, .. } => visit_entity_fields(fields, visit),
        TypeBody::Trait { linked_entity, .. } => linked_entity.iter_mut().try_for_each(|t| visit(t)),
        TypeBody::Page { trait_types, .. } => trait_types.iter_mut().try_for_each(|t| visit(t)),
        TypeBody::Orbital { entity, trait_types, page_type } => {
            entity.iter_mut().try_for_each(|t| visit(t))?;
            trait_types.iter_mut().try_for_each(|t| visit(t))?;
            page_type.iter_mut().try_for_each(|t| visit(t))
        }
        TypeBody::Event { payload, .. } => visit(payload),
        _ => Ok(()),
    }
}

fn visit_entity_fields(fields: &mut [EntityField], visit: &mut Visit<'_>) -> Result<(), LowerError> {
    fields.iter_mut().try_for_each(|f| visit(&mut f.type_expr))
}

fn visit_event_fields(fields: &mut [EventField], visit: &mut Visit<'_>) -> Result<(), LowerError> {
    fields.iter_mut().try_for_each(|f| visit(&mut f.type_expr))
}

fn visit_listens(listens: &mut [Listen], visit: &mut Visit<'_>) -> Result<(), LowerError> {
    listens.iter_mut().try_for_each(|l| visit_event_fields(&mut l.payload, visit))
}

fn visit_emits(emits: &mut [Emit], visit: &mut Visit<'_>) -> Result<(), LowerError> {
    emits.iter_mut().try_for_each(|e| visit_event_fields(&mut e.payload, visit))
}

fn visit_entity(entity: &mut Entity, visit: &mut Visit<'_>) -> Result<(), LowerError> {
    visit_entity_fields(&mut entity.fields, visit)
}

fn visit_trait(t: &mut Trait, visit: &mut Visit<'_>) -> Result<(), LowerError> {
    t.config.iter_mut().try_for_each(|c| visit(&mut c.type_expr))?;
    visit_emits(&mut t.emits, visit)?;
    visit_listens(&mut t.listens, visit)?;
    if let Some(reference) = &mut t.reference {
        reference.config.iter_mut().try_for_each(|c| visit(&mut c.type_expr))?;
    }
    Ok(())
}

fn visit_orbital(orbital: &mut Orbital, visit: &mut Visit<'_>) -> Result<(), LowerError> {
    for alias in &mut orbital.type_aliases {
        visit_alias(alias, visit)?;
    }
    for expects in &mut orbital.expects {
        if let ExpectsKind::Identity { shape: Some(fields), .. } | ExpectsKind::Entity { shape: Some(fields), .. } =
            &mut expects.kind
        {
            visit_entity_fields(fields, visit)?;
        }
    }
    if let Some(entity) = &mut orbital.entity {
        visit_entity(entity, visit)?;
    }
    for entity in &mut orbital.extra_entities {
        visit_entity(entity, visit)?;
    }
    for t in &mut orbital.traits {
        visit_trait(t, visit)?;
    }
    orbital.config.iter_mut().try_for_each(|c| visit(&mut c.type_expr))?;
    orbital.config_overrides.iter_mut().try_for_each(|c| visit(&mut c.type_expr))?;
    visit_entity_fields(&mut orbital.extend_fields, visit)?;
    visit_listens(&mut orbital.import_listens, visit)?;
    for u in &mut orbital.uses {
        u.config.iter_mut().try_for_each(|c| visit(&mut c.type_expr))?;
    }
    Ok(())
}

/// `owner : Inner.Customer` in an `expects entity` shape, where `Inner` is a `uses` alias: the entity `Customer` of the
/// spec `Inner` imports, not a field projection. Rewritten to the bare name (a relation target the importer must
/// provide) and recorded on the orbital so the type check accepts it. A name that is both an alias and a declared type is
/// ambiguous and refused.
fn qualify_alias_names(
    orbital: &mut Orbital,
    file_aliases: &[String],
    declared: &Declared,
) -> Result<(), LowerError> {
    let aliases: Vec<String> = file_aliases
        .iter()
        .cloned()
        .chain(orbital.uses.iter().map(|u| u.alias.clone()))
        .collect();
    if aliases.is_empty() {
        return Ok(());
    }
    let mut seen = Vec::new();
    for expects in &mut orbital.expects {
        if let ExpectsKind::Identity { shape: Some(fields), .. } | ExpectsKind::Entity { shape: Some(fields), .. } =
            &mut expects.kind
        {
            for field in fields {
                rewrite_alias_head(&mut field.type_expr, &aliases, declared, &mut seen)?;
            }
        }
    }
    for name in seen {
        if !orbital.alias_entity_refs.contains(&name) {
            orbital.alias_entity_refs.push(name);
        }
    }
    Ok(())
}

fn rewrite_alias_head(
    t: &mut TypeExpr,
    aliases: &[String],
    declared: &Declared,
    seen: &mut Vec<String>,
) -> Result<(), LowerError> {
    match t {
        TypeExpr::FieldOf { ty, field, .. } if aliases.contains(ty) => {
            if declared.contains_key(ty.as_str()) {
                return Err(LowerError::FieldProjection {
                    projection: format!("{ty}.{field}"),
                    reason: format!("`{ty}` is both a `uses` alias and a declared type, so the qualified name is ambiguous"),
                });
            }
            seen.push(field.clone());
            *t = TypeExpr::Named(field.clone());
            Ok(())
        }
        TypeExpr::Array(inner) | TypeExpr::Optional(inner) => rewrite_alias_head(inner, aliases, declared, seen),
        TypeExpr::Union(parts) => {
            for part in parts {
                rewrite_alias_head(part, aliases, declared, seen)?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}
