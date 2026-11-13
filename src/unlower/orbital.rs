//! Orbitals, entities, pages, `uses`, `expects`.

use super::theme::theme_decl;
use super::trait_::{cross_listen, trait_reference, EventAlias};
use super::types::{
    collect_config_shapes, collect_field_shapes, config_fields, entity_field, event_fields,
    AliasShapes,
};
use super::UnlowerError;
use crate::ast::{
    Entity, ExpectedAccess, ExpectsDecl, ExpectsKind, Orbital, Page, Persistence, Span, TypeAlias,
    TypeBody, TypeExpr, UseDecl,
};
use orbital_core_base::schema::{
    EntityDefinition, EntityRef, ExpectDeclaration, FieldDefinition, Orbital as CoreOrbital,
    PageRef, Persistence as CorePersistence, TraitReference, UseDeclaration,
};
use std::collections::{BTreeMap, BTreeSet};

fn sorted_pairs(map: &orbital_core_base::collections::IndexMap<String, String>) -> Vec<(String, String, Span)> {
    let mut v: Vec<_> = map
        .iter()
        .map(|(k, v)| (k.clone(), v.clone(), Span::dummy()))
        .collect();
    v.sort_by(|a, b| a.0.cmp(&b.0));
    v
}

fn sorted_str_pairs(map: &orbital_core_base::collections::IndexMap<String, String>) -> Vec<(String, String)> {
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
    use orbital_core_base::schema::FieldType as Ft;
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
    if element.field_type == orbital_core_base::schema::FieldType::Union {
        unions.push((alias, element.values.clone().unwrap_or_default()));
    }
    Some(te)
}

/// The IR's waiver record as the `(op, reason)` pairs the `.lolo` AST holds.
fn waiver_pairs(w: Option<&orbital_core_base::schema::AccessWaivers>) -> Vec<(String, String)> {
    let Some(w) = w else {
        return Vec::new();
    };
    [
        ("read", &w.read),
        ("read server", &w.read_server),
        ("create", &w.create),
        ("create server", &w.create_server),
        ("update", &w.update),
        ("delete", &w.delete),
    ]
    .into_iter()
    .filter_map(|(op, reason)| reason.as_ref().map(|r| (op.to_string(), r.clone())))
    .collect()
}

pub(super) fn entity(
    def: &EntityDefinition,
    hints: &AliasShapes,
    unions: &mut Vec<(String, Vec<String>)>,
) -> Result<Entity, UnlowerError> {
    let waivers = waiver_pairs(def.access_waivers.as_ref());
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
        browser: def.browser,
        retired_words: Vec::new(),
        store: def.store.clone(),
        instances: def.instances.clone().unwrap_or_default(),
        locale_instances: def.locale_instances.clone().unwrap_or_default(),
        read_policy: def.read_policy.clone(),
        create_policy: def.create_policy.clone(),
        update_policy: def.update_policy.clone(),
        delete_policy: def.delete_policy.clone(),
        access_waivers: waivers,
        invariants: def.invariants.clone().unwrap_or_default(),
        indexes: def.indexes.clone().unwrap_or_default(),
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

pub(super) fn use_decl(u: &UseDeclaration) -> Result<UseDecl, UnlowerError> {
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
        ExpectDeclaration::Identity { name, shape: s, .. } => ExpectsKind::Identity {
            name: name.clone(),
            shape: shape(s)?,
        },
        ExpectDeclaration::Entity {
            name,
            shape: s,
            persistence,
            shared,
            browser,
            own,
            read_policy,
            create_policy,
            update_policy,
            delete_policy,
            access_waivers,
            invariants,
            ..
        } => ExpectsKind::Entity {
            name: name.clone(),
            shape: shape(s)?,
            persistence: persistence.as_ref().map(|p| match p {
                CorePersistence::Persistent => Persistence::Persistent,
                CorePersistence::Runtime => Persistence::Runtime,
            }),
            shared: *shared,
            browser: *browser,
            own: *own,
            access: ExpectedAccess {
                read_policy: read_policy.clone(),
                create_policy: create_policy.clone(),
                update_policy: update_policy.clone(),
                delete_policy: delete_policy.clone(),
                access_waivers: waiver_pairs(access_waivers.as_ref()),
            },
            invariants: invariants.clone().unwrap_or_default(),
        },
        ExpectDeclaration::Event {
            trait_name,
            event,
            payload,
            ..
        } => ExpectsKind::Event {
            trait_name: trait_name.clone(),
            event: event.clone(),
            payload: payload
                .as_deref()
                .map(super::types::event_fields)
                .transpose()?
                .unwrap_or_default(),
        },
        ExpectDeclaration::Page {
            path,
            protocol,
            traits,
            ..
        } => ExpectsKind::Page {
            path: path.clone(),
            protocol: protocol.clone(),
            traits: traits.clone().unwrap_or_default(),
        },
        ExpectDeclaration::State {
            trait_name,
            state,
            holds,
            ..
        } => ExpectsKind::State {
            trait_name: trait_name.clone(),
            state: state.clone(),
            holds: holds.clone(),
        },
        ExpectDeclaration::Transition {
            trait_name,
            from,
            event,
            to,
            ensures,
            ..
        } => ExpectsKind::Transition {
            trait_name: trait_name.clone(),
            from: from.clone(),
            event: event.clone(),
            to: to.clone(),
            ensures: ensures.clone().unwrap_or_default(),
        },
        ExpectDeclaration::Config { name, knob, .. } => ExpectsKind::Config {
            field: super::types::config_field(name, knob)?,
        },
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
                protocol: d.protocol.clone(),
                address: d.address.clone(),
                path: d.path.clone(),
                type_annotation: None,
                traits: trait_names(&d.traits)?,
                view: d.view_type.clone(),
                primary_entity: d.primary_entity.clone(),
                is_initial: d.is_initial == Some(true),
                is_not_found: d.is_not_found == Some(true),
                span: Span::dummy(),
                reference: None,
                name: (d.protocol.is_some() || d.name != derived).then(|| d.name.clone()),
                icon: d.icon.clone(),
                label: d.label.clone(),
                roles: d.roles.clone(),
                access: d.access,
                indexing: d.indexing,
                title: d.title.clone(),
                description: d.description.clone(),
                translation_of: d.translation_of.clone(),
            })
        }
        PageRef::Object(o) => Ok(Page {
            protocol: o.protocol.clone(),
            address: o.address.clone(),
            path: match (&o.protocol, &o.path) {
                (Some(_), path) => path.clone().unwrap_or_default(),
                (None, path) => path
                    .clone()
                    .ok_or_else(|| UnlowerError::new("page reference without a path"))?,
            },
            type_annotation: None,
            traits: trait_names(&o.traits)?,
            view: None,
            primary_entity: o.linked_entity.clone(),
            is_initial: false,
            is_not_found: false,
            span: Span::dummy(),
            reference: Some(o.reference.clone()),
            name: None,
            icon: o.icon.clone(),
            label: o.label.clone(),
            roles: o.roles.clone(),
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
        alias_entity_refs: Vec::new(),
        os_grants: orbital_core_base::os_grants::OsGrants::default(),
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
                        element.field_type == orbital_core_base::schema::FieldType::Union,
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
    let mut knobs: Vec<&orbital_core_base::collections::IndexMap<String, orbital_core_base::schema::ConfigField>> = Vec::new();
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
    out.os_grants = o.os_grants.clone().unwrap_or_default();
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
            || r.browser;
        if has_tags {
            out.entity_rebind_tags = Some(crate::ast::EntityRebindTags {
                persistence: r.persistence.as_ref().map(|p| match p {
                    CorePersistence::Persistent => Persistence::Persistent,
                    CorePersistence::Runtime => Persistence::Runtime,
                }),
                collection: r.collection.clone(),
                shared: r.shared,
                identity: r.identity,
                browser: r.browser,
                retired_words: Vec::new(),
            });
        }
        let mut overrides: Vec<_> = r.traits.iter().flatten().collect();
        overrides.sort_by(|a, b| a.0.cmp(b.0));
        for (name, o) in overrides {
            out.trait_overrides.push(crate::ast::TraitOverride {
                trait_name: name.clone(),
                linked_entity: o.linked_entity.clone(),
                dispatch: o.dispatch.as_ref().map(super::trait_::unlower_dispatch),
                retired_words: Vec::new(),
                body: crate::ast::TraitRefBody {
                    events: o.events.as_ref().map(sorted_str_pairs).unwrap_or_default(),
                    fields: o.fields.as_ref().map(sorted_str_pairs).unwrap_or_default(),
                    config: config_fields(&o.config)?,
                    listens: o.listens.iter().flatten().map(cross_listen).collect(),
                    emits_scope: o.emits_scope.as_ref().map(|s| match s {
                        orbital_core_base::schema::EventScope::Internal => "internal".to_string(),
                        orbital_core_base::schema::EventScope::External => "external".to_string(),
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

    if o.is_spec() {
        let mut enums: BTreeMap<String, TypeExpr> = BTreeMap::new();
        declared_type_aliases(o, global, &mut out, &mut enums)?;
        for (name, body) in enums {
            out.type_aliases
                .push(type_alias(&name, TypeBody::Alias(body)));
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
    declared_type_aliases(o, global, &mut out, &mut enums)?;
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
    let referenced = field_record_names(o);
    for emit in out.traits.iter_mut().flat_map(|t| t.emits.iter_mut()) {
        if referenced.contains(&emit.event)
            && !emit.payload.is_empty()
            && !event_aliases.iter().any(|a| a.event == emit.event)
        {
            event_aliases.push(EventAlias {
                event: emit.event.clone(),
                payload: TypeExpr::Object(
                    std::mem::take(&mut emit.payload)
                        .into_iter()
                        .map(|f| (f.name, f.type_expr, f.required))
                        .collect(),
                ),
                description: None,
            });
        }
    }
    for listen in out.traits.iter_mut().flat_map(|t| t.listens.iter_mut()) {
        if referenced.contains(&listen.event)
            && !listen.payload.is_empty()
            && !event_aliases.iter().any(|a| a.event == listen.event)
        {
            event_aliases.push(EventAlias {
                event: listen.event.clone(),
                payload: TypeExpr::Object(
                    std::mem::take(&mut listen.payload)
                        .into_iter()
                        .map(|f| (f.name, f.type_expr, f.required))
                        .collect(),
                ),
                description: listen.description.take(),
            });
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
    Ok(out)
}

fn definition_record_names(field: &FieldDefinition, out: &mut BTreeSet<String>) {
    out.extend(field.record.iter().cloned());
    field
        .properties
        .iter()
        .flat_map(|m| m.values())
        .chain(field.items.as_deref())
        .for_each(|f| definition_record_names(f, out));
}

/// The names `field R T` types in this orbital's entities and trait configs: an event alias among them is
/// the only spelling that keeps that reference.
fn field_record_names(o: &CoreOrbital) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    if let EntityRef::Inline(def) = &o.entity {
        def.fields
            .iter()
            .for_each(|f| definition_record_names(f, &mut out));
    }
    for t in &o.traits {
        if let TraitReference::Inline(td) = t {
            for cf in td.config.iter().flat_map(|m| m.values()) {
                out.extend(cf.record.iter().cloned());
                cf.properties
                    .iter()
                    .flat_map(|m| m.values())
                    .chain(cf.items.as_deref())
                    .for_each(|f| definition_record_names(f, &mut out));
            }
        }
    }
    out
}

/// The orbital's recorded `types` as source aliases; member types the payload
/// form erased come back from the definition-side shape (`merge_member`).
fn declared_type_aliases(
    o: &CoreOrbital,
    global: &AliasShapes,
    out: &mut Orbital,
    enums: &mut BTreeMap<String, TypeExpr>,
) -> Result<(), UnlowerError> {
    out.type_aliases
        .extend(declared_type_aliases_from(o.types.as_ref(), global, enums)?);
    Ok(())
}

pub(super) fn declared_type_aliases_from(
    types: Option<&BTreeMap<String, Vec<orbital_core_base::schema::PayloadField>>>,
    global: &AliasShapes,
    enums: &mut BTreeMap<String, TypeExpr>,
) -> Result<Vec<TypeAlias>, UnlowerError> {
    let mut aliases = Vec::new();
    if let Some(types) = types {
        for (name, fields) in types {
            // A dictionary, list or union alias is one entry with the empty name: the alias's value is its type.
            if let [only] = fields.as_slice() {
                if only.name.is_empty() {
                    let value = event_fields(fields)?.remove(0).type_expr;
                    aliases.push(type_alias(name, TypeBody::Alias(value)));
                    continue;
                }
            }
            let richer = global.structs.get(name).cloned().unwrap_or_default();
            let members = event_fields(fields)?
                .into_iter()
                .map(|f| match richer.iter().find(|(n, _, _)| *n == f.name) {
                    Some((_, ty, _)) => (f.name, merge_member(f.type_expr, ty, enums), f.required),
                    None => (f.name, f.type_expr, f.required),
                })
                .collect();
            aliases.push(type_alias(
                name,
                if super::types::printed_nullable(None) {
                    TypeBody::Alias(TypeExpr::Optional(Box::new(TypeExpr::Object(members))))
                } else {
                    TypeBody::Struct(members)
                },
            ));
        }
    }
    Ok(aliases)
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
        (TypeExpr::Object(_) | TypeExpr::Union(_), r @ TypeExpr::Named(_)) => r.clone(),
        (_, r @ TypeExpr::FieldRef { .. }) => r.clone(),
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
        TypeExpr::OpenObject { fields, additional } => {
            fields.iter().for_each(|(_, t, _)| collect_named(t, out));
            collect_named(additional, out);
        }
        TypeExpr::Match { arms, .. } => arms.iter().for_each(|a| collect_named(&a.ty, out)),
        TypeExpr::FieldOf {
            resolved: Some(r), ..
        } => collect_named(r, out),
        _ => {}
    }
}
