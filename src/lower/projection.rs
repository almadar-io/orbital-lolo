//! `T.f` resolution: every `TypeExpr::FieldOf` in the program gets `T`'s declared type of
//! field `f` (an entity's field or a struct type's member), before any lowering reads a type.

use crate::ast::{
    ConfigField, Emit, Entity, EntityField, EventField, ExpectsKind, Listen, Orbital, Program,
    Trait, TypeAlias, TypeBody, TypeExpr,
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
    visit_config(&mut out.app.config, &mut visit)?;
    for used in &mut out.uses {
        visit_config(&mut used.config, &mut visit)?;
    }
    for orbital in &mut out.orbitals {
        visit_orbital(orbital, &mut |t| resolve(t, &declared, &mut Vec::new()))?;
    }
    Ok(out)
}

fn declared_types(program: &Program) -> Declared {
    let mut declared: Declared = HashMap::new();
    let mut add = |name: &str, fields: HashMap<String, TypeExpr>| {
        declared.entry(name.to_string()).or_default().push(fields);
    };
    let entity_fields = |fields: &[EntityField]| -> HashMap<String, TypeExpr> {
        fields
            .iter()
            .map(|f| (f.name.clone(), f.type_expr.clone()))
            .collect()
    };
    let alias_fields = |alias: &TypeAlias| -> Option<HashMap<String, TypeExpr>> {
        match &alias.body {
            TypeBody::Struct(members) => Some(
                members
                    .iter()
                    .map(|(n, t, _)| (n.clone(), t.clone()))
                    .collect(),
            ),
            TypeBody::Entity { fields, .. } => Some(entity_fields(fields)),
            TypeBody::Alias(inner) => inner.record_fields().map(|members| {
                members
                    .iter()
                    .map(|(n, t, _)| (n.clone(), t.clone()))
                    .collect()
            }),
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

fn resolve(
    t: &mut TypeExpr,
    declared: &Declared,
    stack: &mut Vec<String>,
) -> Result<(), LowerError> {
    match t {
        TypeExpr::FieldOf {
            ty,
            field,
            resolved,
        } => {
            let projection = format!("{ty}.{field}");
            let error = |reason: String| LowerError::FieldProjection {
                projection: projection.clone(),
                reason,
            };
            let decls = declared.get(ty.as_str()).ok_or_else(|| {
                error(format!("no entity or struct type named `{ty}` is declared"))
            })?;
            let [fields] = decls.as_slice() else {
                return Err(error(format!("`{ty}` is declared {} times", decls.len())));
            };
            let mut inner = fields
                .get(field.as_str())
                .cloned()
                .ok_or_else(|| error(format!("`{ty}` declares no field `{field}`")))?;
            if stack.contains(&projection) {
                return Err(error(format!(
                    "projects itself through {}",
                    stack.join(" → ")
                )));
            }
            stack.push(projection);
            resolve(&mut inner, declared, stack)?;
            stack.pop();
            *resolved = Some(Box::new(inner));
            Ok(())
        }
        TypeExpr::Array(inner) | TypeExpr::Optional(inner) => {
            resolve(inner, declared, stack)
        }
        TypeExpr::Map(key, value) => {
            resolve(key, declared, stack)?;
            resolve(value, declared, stack)
        }
        TypeExpr::OpenObject { fields, additional } => {
            for (_, ty, _) in fields {
                resolve(ty, declared, stack)?;
            }
            resolve(additional, declared, stack)
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

pub(super) fn visit_alias(alias: &mut TypeAlias, visit: &mut Visit<'_>) -> Result<(), LowerError> {
    match &mut alias.body {
        TypeBody::Struct(members) => members.iter_mut().try_for_each(|(_, t, _)| visit(t)),
        TypeBody::Alias(t) => visit(t),
        TypeBody::Entity { fields, .. } => visit_entity_fields(fields, visit),
        TypeBody::Trait { linked_entity, .. } => {
            linked_entity.iter_mut().try_for_each(|t| visit(t))
        }
        TypeBody::Page { trait_types, .. } => trait_types.iter_mut().try_for_each(|t| visit(t)),
        TypeBody::Orbital {
            entity,
            trait_types,
            page_type,
        } => {
            entity.iter_mut().try_for_each(|t| visit(t))?;
            trait_types.iter_mut().try_for_each(|t| visit(t))?;
            page_type.iter_mut().try_for_each(|t| visit(t))
        }
        TypeBody::Event { payload, .. } => visit(payload),
        _ => Ok(()),
    }
}

fn visit_expression(
    expr: &mut orbital_core_base::schema::SExpression,
    visit: &mut Visit<'_>,
) -> Result<(), LowerError> {
    use orbital_core_base::schema::SExpression;
    if matches!(expr.operator(), Some("quote")) {
        return Ok(());
    }
    if let Some(mut ty) = crate::ast::local_annotation_type(expr) {
        visit(&mut ty)?;
        let parts = match expr {
            SExpression::Array(parts) => parts,
            _ => unreachable!(),
        };
        parts[2] =
            serde_json::from_value(serde_json::to_value(ty).expect("type expression serializes"))
                .expect("type expression is an s-expression");
        return visit_expression(&mut parts[1], visit);
    }
    match expr {
        SExpression::Array(items) => items
            .iter_mut()
            .try_for_each(|e| visit_expression(e, visit)),
        SExpression::Object(fields) => fields
            .values_mut()
            .try_for_each(|e| visit_expression(e, visit)),
        _ => Ok(()),
    }
}

pub(super) fn visit_config(
    fields: &mut [ConfigField],
    visit: &mut Visit<'_>,
) -> Result<(), LowerError> {
    for field in fields {
        visit(&mut field.type_expr)?;
        if let Some(default) = &mut field.default {
            visit_expression(default, visit)?;
        }
    }
    Ok(())
}

fn visit_entity_fields(
    fields: &mut [EntityField],
    visit: &mut Visit<'_>,
) -> Result<(), LowerError> {
    for field in fields {
        visit(&mut field.type_expr)?;
        for expr in field.default.iter_mut().chain(field.derived.iter_mut()) {
            visit_expression(expr, visit)?;
        }
    }
    Ok(())
}

fn visit_event_fields(fields: &mut [EventField], visit: &mut Visit<'_>) -> Result<(), LowerError> {
    fields.iter_mut().try_for_each(|f| visit(&mut f.type_expr))
}

fn visit_listens(listens: &mut [Listen], visit: &mut Visit<'_>) -> Result<(), LowerError> {
    for listen in listens {
        visit_event_fields(&mut listen.payload, visit)?;
        for expr in listen
            .guard
            .iter_mut()
            .chain(listen.payload_mapping.iter_mut().map(|(_, e)| e))
        {
            visit_expression(expr, visit)?;
        }
    }
    Ok(())
}

fn visit_emits(emits: &mut [Emit], visit: &mut Visit<'_>) -> Result<(), LowerError> {
    emits
        .iter_mut()
        .try_for_each(|e| visit_event_fields(&mut e.payload, visit))
}

fn visit_entity(entity: &mut Entity, visit: &mut Visit<'_>) -> Result<(), LowerError> {
    visit_entity_fields(&mut entity.fields, visit)?;
    for expr in entity
        .read_policy
        .iter_mut()
        .chain(entity.create_policy.iter_mut())
        .chain(entity.update_policy.iter_mut())
        .chain(entity.delete_policy.iter_mut())
        .chain(entity.invariants.iter_mut())
    {
        visit_expression(expr, visit)?;
    }
    Ok(())
}

fn visit_trait(t: &mut Trait, visit: &mut Visit<'_>) -> Result<(), LowerError> {
    visit_config(&mut t.config, visit)?;
    for state in &mut t.states {
        if let Some(expr) = &mut state.holds {
            visit_expression(expr, visit)?;
        }
        for transition in &mut state.transitions {
            for expr in transition
                .guard
                .iter_mut()
                .chain(transition.ensures.iter_mut())
                .chain(transition.effects.iter_mut())
            {
                visit_expression(expr, visit)?;
            }
            if let Some(term_match) = &mut transition.term_match {
                for arm in &mut term_match.arms {
                    for expr in &mut arm.effects {
                        visit_expression(expr, visit)?;
                    }
                }
            }
        }
    }
    for tick in &mut t.ticks {
        for expr in tick.guard.iter_mut().chain(tick.effects.iter_mut()) {
            visit_expression(expr, visit)?;
        }
    }
    visit_emits(&mut t.emits, visit)?;
    visit_listens(&mut t.listens, visit)?;
    if let Some(reference) = &mut t.reference {
        visit_config(&mut reference.config, visit)?;
    }
    Ok(())
}

pub(super) fn visit_orbital(
    orbital: &mut Orbital,
    visit: &mut Visit<'_>,
) -> Result<(), LowerError> {
    for alias in &mut orbital.type_aliases {
        visit_alias(alias, visit)?;
    }
    for expects in &mut orbital.expects {
        match &mut expects.kind {
            ExpectsKind::Identity { shape, .. } => {
                if let Some(fields) = shape {
                    visit_entity_fields(fields, visit)?;
                }
            }
            ExpectsKind::Entity {
                shape,
                access,
                invariants,
                ..
            } => {
                if let Some(fields) = shape {
                    visit_entity_fields(fields, visit)?;
                }
                for expr in access
                    .read_policy
                    .iter_mut()
                    .chain(access.create_policy.iter_mut())
                    .chain(access.update_policy.iter_mut())
                    .chain(access.delete_policy.iter_mut())
                    .chain(invariants.iter_mut())
                {
                    visit_expression(expr, visit)?;
                }
            }
            ExpectsKind::State { holds, .. } => {
                if let Some(expr) = holds {
                    visit_expression(expr, visit)?;
                }
            }
            ExpectsKind::Transition { ensures, .. } => {
                for expr in ensures {
                    visit_expression(expr, visit)?;
                }
            }
            ExpectsKind::Config { field } => visit_config(std::slice::from_mut(field), visit)?,
            ExpectsKind::Event { payload, .. } => visit_event_fields(payload, visit)?,
            ExpectsKind::Page { .. } => {}
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
    visit_config(&mut orbital.config, visit)?;
    visit_config(&mut orbital.config_overrides, visit)?;
    for expr in &mut orbital.body {
        visit_expression(expr, visit)?;
    }
    visit_entity_fields(&mut orbital.extend_fields, visit)?;
    visit_listens(&mut orbital.import_listens, visit)?;
    for u in &mut orbital.uses {
        visit_config(&mut u.config, visit)?;
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
        if let ExpectsKind::Identity {
            shape: Some(fields),
            ..
        }
        | ExpectsKind::Entity {
            shape: Some(fields),
            ..
        } = &mut expects.kind
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
        TypeExpr::Array(inner) | TypeExpr::Optional(inner) => {
            rewrite_alias_head(inner, aliases, declared, seen)
        }
        TypeExpr::Union(parts) => {
            for part in parts {
                rewrite_alias_head(part, aliases, declared, seen)?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}
