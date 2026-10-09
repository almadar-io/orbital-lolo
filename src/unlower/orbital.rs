//! Orbitals, entities, pages, `uses`, `expects`.

use super::theme::theme_decl;
use super::trait_::{cross_listen, trait_reference, EventAlias};
use super::types::{
    collect_config_shapes, collect_field_shapes, config_fields, entity_field, event_fields,
    AliasShapes,
};
use super::UnlowerError;
use crate::ast::{
    Entity, ExpectsDecl, ExpectsKind, Orbital, Page, Persistence, Span, TypeAlias, TypeBody,
    TypeExpr, UseDecl,
};
use orbital_core::schema::{
    EntityDefinition, EntityRef, ExpectDeclaration, FieldDefinition, Orbital as CoreOrbital,
    PageRef, Persistence as CorePersistence, TraitReference, UseDeclaration,
};
use std::collections::{BTreeMap, BTreeSet, HashMap};

fn sorted_pairs(map: &HashMap<String, String>) -> Vec<(String, String, Span)> {
    let mut v: Vec<_> = map
        .iter()
        .map(|(k, v)| (k.clone(), v.clone(), Span::dummy()))
        .collect();
    v.sort_by(|a, b| a.0.cmp(&b.0));
    v
}

fn sorted_str_pairs(map: &HashMap<String, String>) -> Vec<(String, String)> {
    let mut v: Vec<_> = map.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    v.sort();
    v
}

fn entity_fields(fields: &[FieldDefinition]) -> Result<Vec<crate::ast::EntityField>, UnlowerError> {
    fields.iter().map(entity_field).collect()
}

/// An entity field typed by a named alias keeps only the expansion in its
/// definition; the payload of any event that carries the entity records the
/// alias name in its type string. Take the name from there when the shapes agree.
/// Returns the printed type, the alias name, and the element definition.
fn payload_named<'a>(
    entity: &str,
    fd: &'a FieldDefinition,
    hints: &AliasShapes,
) -> Option<(TypeExpr, String, &'a FieldDefinition)> {
    let name = fd.name.as_deref()?;
    let raw = hints
        .entity_field_types
        .get(&(entity.to_string(), name.to_string()))?;
    let te = super::types::parse_type_string(raw).ok()?;
    let (alias, element) = match &te {
        TypeExpr::Named(n) => (n.clone(), fd),
        TypeExpr::Array(inner) => match (inner.as_ref(), fd.items.as_deref()) {
            (TypeExpr::Named(n), Some(item)) => (n.clone(), item),
            _ => return None,
        },
        _ => return None,
    };
    if crate::lower::primitive_field_type(&alias).is_some() || hints.entities.contains(&alias) {
        return None;
    }
    use orbital_core::schema::FieldType as Ft;
    match (&element.field_type, &element.values, &element.properties) {
        (Ft::Object, _, Some(_)) if hints.known_structs.contains(&alias) => {
            Some((te, alias, element))
        }
        (Ft::Union, Some(values), _) if values.len() > 1 => Some((te, alias, element)),
        _ => None,
    }
}

fn named_by_payload(
    entity: &str,
    fd: &FieldDefinition,
    hints: &AliasShapes,
    unions: &mut Vec<(String, Vec<String>)>,
) -> Option<TypeExpr> {
    let (te, alias, element) = payload_named(entity, fd, hints)?;
    if element.field_type == orbital_core::schema::FieldType::Union {
        unions.push((alias, element.values.clone().unwrap_or_default()));
    }
    Some(te)
}

pub(super) fn entity(
    def: &EntityDefinition,
    hints: &AliasShapes,
    unions: &mut Vec<(String, Vec<String>)>,
) -> Result<Entity, UnlowerError> {
    let mut waivers = Vec::new();
    if let Some(w) = &def.access_waivers {
        for (op, reason) in [
            ("read", &w.read),
            ("create", &w.create),
            ("update", &w.update),
            ("delete", &w.delete),
        ] {
            if let Some(r) = reason {
                waivers.push((op.to_string(), r.clone()));
            }
        }
    }
    Ok(Entity {
        name: def.name.clone(),
        type_annotation: None,
        persistence: def.persistence.as_ref().map(|p| match p {
            CorePersistence::Persistent => Persistence::Persistent,
            CorePersistence::Runtime => Persistence::Runtime,
        }),
        collection: def.collection.clone(),
        shared: def.shared,
        identity: def.identity,
        local: def.local,
        seed_mock: def.seed_mock,
        instances: def.instances.clone().unwrap_or_default(),
        locale_instances: def.locale_instances.clone().unwrap_or_default(),
        read_policy: def.read_policy.clone(),
        create_policy: def.create_policy.clone(),
        update_policy: def.update_policy.clone(),
        delete_policy: def.delete_policy.clone(),
        access_waivers: waivers,
        fields: def
            .fields
            .iter()
            .map(|fd| {
                let mut f = entity_field(fd)?;
                if let Some(te) = named_by_payload(&def.name, fd, hints, unions) {
                    f.type_expr = te;
                }
                Ok(f)
            })
            .collect::<Result<_, UnlowerError>>()?,
        span: Span::dummy(),
    })
}

fn inline_entity(
    e: &EntityRef,
    hints: &AliasShapes,
    unions: &mut Vec<(String, Vec<String>)>,
) -> Result<Entity, UnlowerError> {
    match e {
        EntityRef::Inline(def) => entity(def, hints, unions),
        _ => Err(UnlowerError::new(
            "entity reference (only inline entities come from .lolo lowering)",
        )),
    }
}

fn use_decl(u: &UseDeclaration) -> Result<UseDecl, UnlowerError> {
    Ok(UseDecl {
        alias: u.alias.clone(),
        path: u.from.clone(),
        lazy: u.lazy == Some(true),
        config: config_fields(&u.config)?,
        span: Span::dummy(),
    })
}

fn expects_decl(e: &ExpectDeclaration) -> Result<ExpectsDecl, UnlowerError> {
    let shape = |s: &Option<Vec<FieldDefinition>>| -> Result<_, UnlowerError> {
        s.as_ref().map(|f| entity_fields(f)).transpose()
    };
    let kind = match e {
        ExpectDeclaration::Identity { name, shape: s } => ExpectsKind::Identity {
            name: name.clone(),
            shape: shape(s)?,
        },
        ExpectDeclaration::Entity { name, shape: s } => ExpectsKind::Entity {
            name: name.clone(),
            shape: shape(s)?,
        },
        ExpectDeclaration::Event { trait_name, event } => ExpectsKind::Event {
            trait_name: trait_name.clone(),
            event: event.clone(),
        },
        ExpectDeclaration::Page { path } => ExpectsKind::Page { path: path.clone() },
    };
    Ok(ExpectsDecl {
        kind,
        span: Span::dummy(),
    })
}

fn trait_names(traits: &Option<Vec<TraitReference>>) -> Result<Vec<String>, UnlowerError> {
    traits
        .iter()
        .flatten()
        .map(|t| match t {
            TraitReference::Reference {
                reference: Some(r), ..
            } => Ok(r.clone()),
            _ => Err(UnlowerError::new("page trait that is not a bare reference")),
        })
        .collect()
}

fn page(p: &PageRef) -> Result<Page, UnlowerError> {
    match p {
        PageRef::Inline(d) => {
            let derived = crate::lower::page_name_from_path(&d.path);
            Ok(Page {
                path: d.path.clone(),
                type_annotation: None,
                traits: trait_names(&d.traits)?,
                view: d.view_type.clone(),
                primary_entity: d.primary_entity.clone(),
                is_initial: d.is_initial == Some(true),
                span: Span::dummy(),
                reference: None,
                name: (d.name != derived).then(|| d.name.clone()),
                icon: d.icon.clone(),
                label: d.label.clone(),
                access: d.access,
                indexing: d.indexing,
                title: d.title.clone(),
                description: d.description.clone(),
                translation_of: d.translation_of.clone(),
            })
        }
        PageRef::Object(o) => Ok(Page {
            path: o
                .path
                .clone()
                .ok_or_else(|| UnlowerError::new("page reference without a path"))?,
            type_annotation: None,
            traits: trait_names(&o.traits)?,
            view: None,
            primary_entity: o.linked_entity.clone(),
            is_initial: false,
            span: Span::dummy(),
            reference: Some(o.reference.clone()),
            name: None,
            icon: None,
            label: None,
            access: o.access,
            indexing: o.indexing,
            title: o.title.clone(),
            description: o.description.clone(),
            translation_of: o.translation_of.clone(),
        }),
        PageRef::Reference(_) => Err(UnlowerError::new("bare string page reference")),
    }
}

fn empty_orbital(name: &str) -> Orbital {
    Orbital {
        name: name.to_string(),
        type_annotation: None,
        uses: Vec::new(),
        expects: Vec::new(),
        type_aliases: Vec::new(),
        entity: None,
        extra_entities: Vec::new(),
        traits: Vec::new(),
        pages: Vec::new(),
        theme: None,
        body: Vec::new(),
        reference: None,
        entity_rebind: None,
        entity_rebind_tags: None,
        fields_remap: Vec::new(),
        pages_remap: Vec::new(),
        page_modifiers: Vec::new(),
        omit: Vec::new(),
        only: Vec::new(),
        config_overrides: Vec::new(),
        events_remap: Vec::new(),
        roles_remap: Vec::new(),
        entities_remap: Vec::new(),
        mounts: Vec::new(),
        config: Vec::new(),
        extend_fields: Vec::new(),
        mock_overrides: Vec::new(),
        retype_fields: Vec::new(),
        trait_overrides: Vec::new(),
        import_listens: Vec::new(),
        span: Span::dummy(),
    }
}

fn type_alias(name: &str, body: TypeBody) -> TypeAlias {
    TypeAlias {
        name: name.to_string(),
        type_param: None,
        type_param_kind: None,
        body,
        span: Span::dummy(),
    }
}

pub(super) fn alias_shapes(
    o: &CoreOrbital,
    hints: &AliasShapes,
) -> Result<AliasShapes, UnlowerError> {
    let mut shapes = AliasShapes::default();
    shapes.known_structs = o.types.iter().flat_map(|t| t.keys().cloned()).collect();
    for e in std::iter::once(&o.entity).chain(o.auxiliary_entities.iter().flatten()) {
        if let EntityRef::Inline(def) = e {
            for f in &def.fields {
                collect_field_shapes(f, &mut shapes)?;
                if let Some((_, alias, element)) = payload_named(&def.name, f, hints) {
                    if let (false, Some(props)) = (
                        element.field_type == orbital_core::schema::FieldType::Union,
                        &element.properties,
                    ) {
                        shapes
                            .structs
                            .insert(alias, super::types::object_members(props)?);
                    }
                }
            }
        }
    }
    let mut knobs: Vec<&HashMap<String, orbital_core::schema::ConfigField>> = Vec::new();
    knobs.extend(o.config.iter());
    for t in &o.traits {
        if let TraitReference::Inline(td) = t {
            knobs.extend(td.config.iter());
        }
    }
    for m in knobs {
        for cf in m.values() {
            collect_config_shapes(cf, &mut shapes)?;
        }
    }
    Ok(shapes)
}

pub(super) fn orbital(o: &CoreOrbital, global: &AliasShapes) -> Result<Orbital, UnlowerError> {
    let mut out = empty_orbital(&o.name);
    out.uses = o
        .uses
        .iter()
        .flatten()
        .map(use_decl)
        .collect::<Result<_, _>>()?;
    out.expects = o
        .expects
        .iter()
        .flatten()
        .map(expects_decl)
        .collect::<Result<_, _>>()?;

    if let Some(r) = &o.reference {
        out.reference = Some(r.reference.clone());
        out.entity_rebind = r.entity.clone();
        out.fields_remap = r.fields.as_ref().map(sorted_pairs).unwrap_or_default();
        out.pages_remap = r.pages.as_ref().map(sorted_pairs).unwrap_or_default();
        out.omit = r
            .omit
            .iter()
            .flatten()
            .map(|n| (n.clone(), Span::dummy()))
            .collect();
        out.only = r.only.clone().unwrap_or_default();
        out.config_overrides = config_fields(&r.config)?;
        let mut events: Vec<(String, String)> = r
            .events
            .iter()
            .flatten()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        events.sort();
        out.events_remap = events;
        let mut roles: Vec<_> = r
            .roles
            .iter()
            .flatten()
            .map(|(k, v)| (k.clone(), v.clone(), Span::dummy()))
            .collect();
        roles.sort_by(|a, b| a.0.cmp(&b.0));
        out.roles_remap = roles;
        out.entities_remap = r.entities.as_ref().map(sorted_pairs).unwrap_or_default();
        let mut mounts: Vec<_> = r
            .mounts
            .iter()
            .flatten()
            .map(|(k, v)| (k.clone(), v.clone(), Span::dummy()))
            .collect();
        mounts.sort_by(|a, b| a.0.cmp(&b.0));
        out.mounts = mounts;
        out.extend_fields = entity_fields(r.extend.as_deref().unwrap_or(&[]))?;
        out.mock_overrides = r.mock.as_ref().map(sorted_pairs).unwrap_or_default();
        out.retype_fields = entity_fields(r.retype.as_deref().unwrap_or(&[]))?;
        out.page_modifiers = r
            .page_modifiers
            .iter()
            .flatten()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        let has_tags = r.persistence.is_some()
            || r.collection.is_some()
            || r.shared
            || r.identity
            || r.local
            || r.seed_mock;
        if has_tags {
            out.entity_rebind_tags = Some(crate::ast::EntityRebindTags {
                persistence: r.persistence.as_ref().map(|p| match p {
                    CorePersistence::Persistent => Persistence::Persistent,
                    CorePersistence::Runtime => Persistence::Runtime,
                }),
                collection: r.collection.clone(),
                shared: r.shared,
                identity: r.identity,
                local: r.local,
                mock: r.seed_mock,
            });
        }
        let mut overrides: Vec<_> = r.traits.iter().flatten().collect();
        overrides.sort_by(|a, b| a.0.cmp(b.0));
        for (name, o) in overrides {
            out.trait_overrides.push(crate::ast::TraitOverride {
                trait_name: name.clone(),
                linked_entity: o.linked_entity.clone(),
                body: crate::ast::TraitRefBody {
                    events: o.events.as_ref().map(sorted_str_pairs).unwrap_or_default(),
                    fields: o.fields.as_ref().map(sorted_str_pairs).unwrap_or_default(),
                    config: config_fields(&o.config)?,
                    listens: o.listens.iter().flatten().map(cross_listen).collect(),
                    emits_scope: o.emits_scope.as_ref().map(|s| match s {
                        orbital_core::schema::EventScope::Internal => "internal".to_string(),
                        orbital_core::schema::EventScope::External => "external".to_string(),
                    }),
                    name_override: None,
                },
                span: Span::dummy(),
            });
        }
        out.import_listens = r
            .listens
            .iter()
            .flatten()
            .map(|l| {
                let mut listen = cross_listen(&l.listen);
                listen.target_trait = Some(l.trait_name.clone());
                listen
            })
            .collect();
        for t in &o.traits {
            out.traits.push(trait_reference(t)?.0);
        }
        return Ok(out);
    }

    out.theme = o.theme.as_ref().map(theme_decl).transpose()?;
    let mut hinted_unions: Vec<(String, Vec<String>)> = Vec::new();
    out.entity = Some(inline_entity(&o.entity, global, &mut hinted_unions)?);
    for e in o.auxiliary_entities.iter().flatten() {
        out.extra_entities
            .push(inline_entity(e, global, &mut hinted_unions)?);
    }
    let mut event_aliases: Vec<EventAlias> = Vec::new();
    for t in &o.traits {
        if let TraitReference::Inline(td) = t {
            if td.name == "__Composition" {
                out.body = td
                    .state_machine
                    .transitions
                    .iter()
                    .flat_map(|tr| tr.effects.iter().flatten().cloned())
                    .collect();
                continue;
            }
        }
        let (tr, aliases) = trait_reference(t)?;
        out.traits.push(tr);
        event_aliases.extend(aliases);
    }
    for p in o.pages.iter().flatten() {
        out.pages.push(page(p)?);
    }
    out.config = config_fields(&o.config)?;

    let shapes = alias_shapes(o, global)?;
    let mut enums: BTreeMap<String, TypeExpr> = BTreeMap::new();
    if let Some(types) = &o.types {
        for (name, fields) in types {
            let richer = global.structs.get(name).cloned().unwrap_or_default();
            let members = event_fields(fields)?
                .into_iter()
                .map(|f| match richer.iter().find(|(n, _, _)| *n == f.name) {
                    Some((_, ty, _)) => (
                        f.name,
                        merge_member(f.type_expr, ty, &mut enums),
                        f.required,
                    ),
                    None => (f.name, f.type_expr, f.required),
                })
                .collect();
            out.type_aliases
                .push(type_alias(name, TypeBody::Struct(members)));
        }
    }
    for (name, variants) in shapes.unions.into_iter().chain(hinted_unions) {
        if !out.type_aliases.iter().any(|t| t.name == name) {
            out.type_aliases
                .push(type_alias(&name, TypeBody::Enum(variants)));
        }
    }
    for (name, body) in enums {
        if !out.type_aliases.iter().any(|t| t.name == name) {
            out.type_aliases
                .push(type_alias(&name, TypeBody::Alias(body)));
        }
    }
    for a in event_aliases {
        if out.type_aliases.iter().any(|t| t.name == a.event) {
            continue;
        }
        out.type_aliases.push(type_alias(
            &a.event,
            TypeBody::Event {
                payload: a.payload,
                scope: None,
                description: a.description,
            },
        ));
    }
    declare_opaque_names(&mut out, global);
    Ok(out)
}

/// Authored member type from its payload-flattened form (which keeps the
/// authored names but erases literal unions) and the definition-side form
/// (which keeps the literals but inlines every name).
fn merge_member(
    payload: TypeExpr,
    richer: &TypeExpr,
    enums: &mut BTreeMap<String, TypeExpr>,
) -> TypeExpr {
    let literals = |t: &TypeExpr| -> Option<Vec<String>> {
        match t {
            TypeExpr::StringLiteral(s) => Some(vec![s.clone()]),
            TypeExpr::Union(parts) => parts
                .iter()
                .map(|p| match p {
                    TypeExpr::StringLiteral(s) => Some(s.clone()),
                    _ => None,
                })
                .collect(),
            _ => None,
        }
    };
    match (payload, richer) {
        (TypeExpr::Named(n), r @ (TypeExpr::Union(_) | TypeExpr::StringLiteral(_))) => {
            enums.insert(n.clone(), r.clone());
            TypeExpr::Named(n)
        }
        (TypeExpr::Primitive(p), r) if p == "string" && literals(r).is_some() => r.clone(),
        (TypeExpr::Array(a), TypeExpr::Array(b)) => {
            TypeExpr::Array(Box::new(merge_member(*a, b, enums)))
        }
        (TypeExpr::Map(k, a), TypeExpr::Map(_, b)) => {
            TypeExpr::Map(k, Box::new(merge_member(*a, b, enums)))
        }
        (TypeExpr::Object(ma), TypeExpr::Object(mb)) => TypeExpr::Object(
            ma.into_iter()
                .map(|(n, t, r)| match mb.iter().find(|(bn, _, _)| *bn == n) {
                    Some((_, bt, _)) => (n, merge_member(t, bt, enums), r),
                    None => (n, t, r),
                })
                .collect(),
        ),
        (TypeExpr::Object(_), r @ TypeExpr::Named(_)) => r.clone(),
        (p, _) => p,
    }
}

pub(super) fn collect_named(te: &TypeExpr, out: &mut BTreeSet<String>) {
    match te {
        TypeExpr::Named(n) => {
            out.insert(n.clone());
        }
        TypeExpr::Array(i) | TypeExpr::Optional(i) => collect_named(i, out),
        TypeExpr::Map(k, v) => {
            collect_named(k, out);
            collect_named(v, out);
        }
        TypeExpr::Tuple(items) | TypeExpr::Union(items) => {
            items.iter().for_each(|t| collect_named(t, out))
        }
        TypeExpr::Object(members) => members.iter().for_each(|(_, t, _)| collect_named(t, out)),
        TypeExpr::Match { arms, .. } => arms.iter().for_each(|a| collect_named(&a.ty, out)),
        TypeExpr::FieldOf {
            resolved: Some(r), ..
        } => collect_named(r, out),
        _ => {}
    }
}

/// A type name the lowerer leaves in a config or payload type string without
/// resolving it (an enum alias, a union alias used in a payload) needs some
/// alias in scope for the printed source to type-check; `= string` lowers to
/// the same IR, since those positions only record the name.
fn declare_opaque_names(out: &mut Orbital, global: &AliasShapes) {
    let mut used = BTreeSet::new();
    let mut params: BTreeSet<String> = BTreeSet::new();
    let mut fields: Vec<&[crate::ast::ConfigField]> = vec![&out.config];
    for t in &out.traits {
        params.extend(t.type_params.iter().map(|p| p.name.clone()));
        fields.push(&t.config);
        for e in &t.emits {
            e.payload
                .iter()
                .for_each(|f| collect_named(&f.type_expr, &mut used));
        }
        for l in &t.listens {
            l.payload
                .iter()
                .for_each(|f| collect_named(&f.type_expr, &mut used));
        }
        if let Some(r) = &t.reference {
            fields.push(&r.config);
        }
    }
    for list in fields {
        list.iter()
            .for_each(|c| collect_named(&c.type_expr, &mut used));
    }
    let declared: BTreeSet<&str> = out.type_aliases.iter().map(|a| a.name.as_str()).collect();
    let mut entities: BTreeSet<&str> = global.entities.iter().map(String::as_str).collect();
    entities.extend(
        out.entity
            .iter()
            .chain(&out.extra_entities)
            .map(|e| e.name.as_str()),
    );
    let missing: Vec<String> = used
        .into_iter()
        .filter(|n| {
            !declared.contains(n.as_str())
                && !entities.contains(n.as_str())
                && !params.contains(n)
                && crate::lower::primitive_field_type(n).is_none()
        })
        .collect();
    for n in missing {
        out.type_aliases.push(type_alias(
            &n,
            TypeBody::Alias(TypeExpr::Primitive("string".to_string())),
        ));
    }
}
