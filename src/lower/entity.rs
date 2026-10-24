use super::*;
use std::cell::RefCell;

thread_local! {
    /// Alias names currently being lowered on this thread. Guards the
    /// self-referential alias cycle that tagged unions make reachable.
    /// `pub(crate)` so `emit_listen.rs`'s payload-position struct lowering
    /// (`struct_payload_properties` and friends) shares this ONE guard
    /// instead of a second copy — same cycle, same set, checked from a
    /// different call site.
    pub(crate) static RESOLVING: RefCell<HashSet<String>> = RefCell::new(HashSet::new());
    /// Tagged-union variants expanded since the last named alias was entered:
    /// re-entering one without a named alias in between is an unbounded inline cycle.
    static VARIANTS: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

pub(crate) fn lower_entity(
    entity: &Entity,
    aliases: &BTreeMap<String, TypeAliasResolved>,
    entity_names: &HashSet<String>,
) -> Result<EntityDefinition, LowerError> {
    if let Some(word) = entity.retired_words.first() {
        return Err(crate::error::entity_header_word_error(&word.word).located(word.span));
    }
    check_shared_entity_has_persistence_mode(entity)?;
    let mut fields = Vec::with_capacity(entity.fields.len());
    let allow: &[&str] = if matches!(entity.persistence, Some(Persistence::Runtime)) {
        crate::parser::RUNTIME_ENTITY_ALLOWED
    } else {
        &[]
    };
    for f in &entity.fields {
        fields.push(lower_field(f, aliases, entity_names, allow)?);
    }
    let persistence = match &entity.persistence {
        Some(Persistence::Persistent) => Some(CorePersistence::Persistent),
        Some(Persistence::Runtime) => Some(CorePersistence::Runtime),
        None => None,
    };
    Ok(EntityDefinition {
        timestamps: None,
        soft_delete: None,
        description: None,
        visual_prompt: None,
        asset_ref: None,
        store: entity.store.clone(),
        name: entity.name.clone(),
        collection: entity.collection.clone(),
        persistence,
        fields,
        instances: (!entity.instances.is_empty()).then(|| entity.instances.clone()),
        shared: entity.shared,
        identity: entity.identity,
        browser: entity.browser,
        locale_instances: (!entity.locale_instances.is_empty())
            .then(|| entity.locale_instances.clone()),
        read_policy: entity.read_policy.clone(),
        create_policy: entity.create_policy.clone(),
        update_policy: entity.update_policy.clone(),
        delete_policy: entity.delete_policy.clone(),
        access_waivers: access_waivers_of(&entity.access_waivers),
        id: None,
        invariants: (!entity.invariants.is_empty()).then(|| entity.invariants.clone()),
    })
}

/// The `@<op> none "<reason>"` declarations as the IR's waiver record; `None` when there are none.
pub(crate) fn access_waivers_of(
    declared: &[(String, String)],
) -> Option<orbital_core_base::schema::AccessWaivers> {
    if declared.is_empty() {
        return None;
    }
    let mut waivers = orbital_core_base::schema::AccessWaivers::default();
    for (op, reason) in declared {
        match op.as_str() {
            "read" => waivers.read = Some(reason.clone()),
            "create" => waivers.create = Some(reason.clone()),
            "update" => waivers.update = Some(reason.clone()),
            "delete" => waivers.delete = Some(reason.clone()),
            _ => {}
        }
    }
    Some(waivers)
}

/// The config-only type an entity field reaches, following record aliases: the parser's own list
/// (`type_expr_config_only_type`) checks a field's written type, and a record alias can carry one
/// in. Returns the type and the alias it came through.
fn config_only_through_aliases(
    t: &TypeExpr,
    aliases: &BTreeMap<String, TypeAliasResolved>,
    allow: &[&str],
    seen: &mut HashSet<String>,
) -> Option<(&'static str, String)> {
    match t {
        TypeExpr::Named(name) => match aliases.get(name) {
            Some(TypeAliasResolved::Type(inner, _)) if seen.insert(name.clone()) => {
                config_only_through_aliases(inner, aliases, allow, seen)
                    .map(|(ty, via)| (ty, if via.is_empty() { name.clone() } else { via }))
            }
            _ => None,
        },
        TypeExpr::Array(inner) | TypeExpr::Optional(inner) => {
            config_only_through_aliases(inner, aliases, allow, seen)
        }
        TypeExpr::Tuple(parts) | TypeExpr::Union(parts) => parts
            .iter()
            .find_map(|p| config_only_through_aliases(p, aliases, allow, seen)),
        TypeExpr::Map(key, value) => config_only_through_aliases(key, aliases, allow, seen)
            .or_else(|| config_only_through_aliases(value, aliases, allow, seen)),
        TypeExpr::Object(fields) => fields
            .iter()
            .find_map(|(_, ty, _)| config_only_through_aliases(ty, aliases, allow, seen)),
        TypeExpr::OpenObject { fields, additional } => fields
            .iter()
            .find_map(|(_, ty, _)| config_only_through_aliases(ty, aliases, allow, seen))
            .or_else(|| config_only_through_aliases(additional, aliases, allow, seen)),
        other => crate::parser::type_expr_config_only_type_allowing(other, allow)
            .map(|ty| (ty, String::new())),
    }
}

pub(crate) fn source_nullable(
    ty: &TypeExpr,
    aliases: &BTreeMap<String, TypeAliasResolved>,
) -> bool {
    fn visit(
        ty: &TypeExpr,
        aliases: &BTreeMap<String, TypeAliasResolved>,
        seen: &mut HashSet<String>,
    ) -> bool {
        match ty {
            TypeExpr::Optional(_) => true,
            TypeExpr::Named(name) if seen.insert(name.clone()) => {
                let result = match aliases.get(name) {
                    Some(TypeAliasResolved::Type(inner, _)) => visit(inner, aliases, seen),
                    _ => orbital_core_base::language::program_types::program_type_field(name)
                        .and_then(|field| field.nullable)
                        .unwrap_or(false),
                };
                seen.remove(name);
                result
            }
            TypeExpr::Union(parts) => parts.iter().any(|part| visit(part, aliases, seen)),
            TypeExpr::FieldOf {
                resolved: Some(inner),
                ..
            } => visit(inner, aliases, seen),
            _ => false,
        }
    }
    visit(ty, aliases, &mut HashSet::new())
}

pub(crate) fn source_additional_type<'a>(
    ty: &'a TypeExpr,
    aliases: &'a BTreeMap<String, TypeAliasResolved>,
) -> Option<&'a TypeExpr> {
    let mut ty = ty;
    let mut seen = HashSet::new();
    loop {
        match ty {
            TypeExpr::OpenObject { additional, .. } => return Some(additional),
            TypeExpr::Optional(inner)
            | TypeExpr::FieldOf {
                resolved: Some(inner),
                ..
            } => ty = inner,
            TypeExpr::Named(name) if seen.insert(name) => match aliases.get(name) {
                Some(TypeAliasResolved::Type(inner, _)) => ty = inner,
                _ => return None,
            },
            _ => return None,
        }
    }
}

pub(crate) fn lower_source_field(
    ty: &TypeExpr,
    aliases: &BTreeMap<String, TypeAliasResolved>,
    entity_names: &HashSet<String>,
    params: &[&str],
) -> Result<FieldDefinition, LowerError> {
    let (field_type, values, items, properties, relation) =
        lower_type_expr(ty, aliases, entity_names, params)?;
    let additional_properties = if field_type == FieldType::Object {
        source_additional_type(ty, aliases)
            .map(|additional| {
                lower_source_field(additional, aliases, entity_names, params).map(Box::new)
            })
            .transpose()?
    } else {
        None
    };
    Ok(FieldDefinition {
        nullable: Some(source_nullable(ty, aliases)),
        additional_properties,
        merged_from: None,
        name: None,
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
        projected_from: ty.projection(),
    })
}

pub(crate) fn lower_field(
    f: &EntityField,
    aliases: &BTreeMap<String, TypeAliasResolved>,
    entity_names: &HashSet<String>,
    allow: &[&str],
) -> Result<FieldDefinition, LowerError> {
    // Resolve the type expression. Named types may be enum aliases (lower to
    // string + values list), entity references (lower to a Relation field with
    // cardinality), or struct aliases (lower to inline object schema).
    let shape = lower_source_field(&f.type_expr, aliases, entity_names, &[])?;
    let field = FieldDefinition {
        merged_from: None,
        name: Some(f.name.clone()),
        field_type: shape.field_type,
        required: if f.required { Some(true) } else { None },
        primary_key: if f.is_key { Some(true) } else { None },
        unique: if f.is_unique { Some(true) } else { None },
        default: canonicalize_unbound_trait_default(&f.type_expr, f.default.clone()),
        values: shape.values,
        relation: shape.relation,
        items: shape.items,
        properties: shape.properties,
        nullable: shape.nullable,
        additional_properties: shape.additional_properties,
        intrinsic: if f.is_intrinsic { Some(true) } else { None },
        description: f.description.clone(),
        synonyms: f.synonyms.clone(),
        mock: f.mock.clone(),
        projected_from: f.type_expr.projection(),
    };
    if let Some((ty, via)) =
        config_only_through_aliases(&f.type_expr, aliases, allow, &mut HashSet::new())
    {
        return Err(LowerError::ConfigTypeInEntity {
            field: f.name.clone(),
            path: via,
            ty: ty.to_string(),
        });
    }
    Ok(field)
}

type LoweredType = (
    FieldType,
    Option<Vec<String>>,
    Option<Box<FieldDefinition>>,
    Option<HashMap<String, FieldDefinition>>,
    Option<RelationDefinition>,
);

pub(crate) fn lower_type_expr(
    ty: &TypeExpr,
    aliases: &BTreeMap<String, TypeAliasResolved>,
    entity_names: &HashSet<String>,
    params: &[&str],
) -> Result<LoweredType, LowerError> {
    Ok(match ty {
        TypeExpr::Primitive(name) => (primitive_to_field_type(name), None, None, None, None),
        // `@entity` is only legal in event payload positions (enforced by the
        // Stage-2 checker); an entity/config field reaching here is already an
        // error upstream — lower defensively to a shapeless object.
        TypeExpr::BoundEntityRow
        | TypeExpr::EventPayloadOf(PayloadOfTarget::ConfigKnob(_))
        | TypeExpr::NamedEntityRow(_)
        | TypeExpr::ExpectedIdentityRow => (FieldType::Object, None, None, None, None),
        // `@payload <sibling>` inside a struct member position (a
        // `type X = { action: event, actionPayload: @payload <action> }`
        // body): the concrete event isn't knowable here — `action` is a TYPE
        // (`event`), not a literal — so there is no single named shape to
        // carry through the closed `FieldType` wire tag. Lowers to a
        // shapeless `Object` (no `properties`, no `items`) — the same
        // "target declares nothing" carrier `resolve_field_shape` already
        // collapses to `OirType::Json`, which codegen renders as the generic
        // `EventPayload` TS type (`shell_typescript::backend::shared`).
        // `types::check_payload_sibling_refs` is the actual gate: it
        // validates the sibling exists and is `event`-typed, and (where both
        // are literal) that a literal instance's value matches the sibling's
        // declared payload — entirely at L1, against the AST, before this
        // erasure happens.
        TypeExpr::EventPayloadOf(PayloadOfTarget::SiblingMember(_)) => {
            (FieldType::Object, None, None, None, None)
        }
        TypeExpr::Trait => (FieldType::Trait, None, None, None, None),
        TypeExpr::Orbital => (FieldType::Orbital, None, None, None, None),
        TypeExpr::Behavior => (FieldType::Behavior, None, None, None, None),
        TypeExpr::Slot => (FieldType::Slot, None, None, None, None),
        TypeExpr::Pattern => (FieldType::Pattern, None, None, None, None),
        // Config-only (parser rejects it on entity fields); defensively a string ref.
        TypeExpr::Secret => (FieldType::String, None, None, None, None),
        // Config-only brand (gated by `config_only_type` in parser/mod.rs):
        // an S-expression as data — atoms, lists and option objects (G-ORB-093).
        TypeExpr::SExpr => (FieldType::SExpr, None, None, None, None),
        // Payload-position-only (gated by `type_expr_config_only_type` in
        // parser/mod.rs); an entity field reaching here is already an error
        // upstream — lower defensively to an opaque object, same treatment
        // as the other config/payload-only brands above.
        TypeExpr::Match { .. } => (FieldType::Object, None, None, None, None),
        TypeExpr::EntityRef => (FieldType::String, None, None, None, None),
        // Phase B: an `event`-typed struct-alias field (e.g. `ItemAction.event`)
        // keeps its type tag through lowering instead of erasing to `string` —
        // `check_field_def_shape`'s closed-circuit-parity acceptance check
        // (`config_type.rs`) needs the tag to fire. Entity-field positions stay
        // protected by the parser gate (`ELOLO_CONFIG_TYPE_FORBIDDEN_IN_ENTITY`) and,
        // through record aliases, by `config_only_through_aliases` in `lower_field`.
        TypeExpr::EventRef => (FieldType::Event, None, None, None, None),
        TypeExpr::EventAddress => (FieldType::EventAddress, None, None, None, None),
        TypeExpr::FieldOf {
            resolved: Some(inner),
            ..
        } => lower_type_expr(inner, aliases, entity_names, params)?,
        // Unresolved only if lowering skipped `projection::resolve_field_projections`.
        TypeExpr::FieldOf { resolved: None, .. } => (FieldType::String, None, None, None, None),
        TypeExpr::Named(name) => {
            if let Some(alias) = aliases.get(name) {
                match alias {
                    TypeAliasResolved::Enum(values) => {
                        (FieldType::String, Some(values.clone()), None, None, None)
                    }
                    TypeAliasResolved::Type(inner, own_param) => {
                        // CYCLE GUARD. A recursive alias — `DrawGroupItem.items :
                        // [DrawItem]` where `DrawItem` includes `DrawGroupItem` —
                        // would recurse forever here (verified: `orb validate`
                        // aborted with a stack overflow before this existed).
                        // A back-reference lowers to the documented by-name form:
                        // `Union` carrying the NAME in `values` and no
                        // `properties`, which `resolve` maps to Json rather than
                        // expanding.
                        if !RESOLVING.with(|r| r.borrow_mut().insert(name.clone())) {
                            return Ok((
                                FieldType::Union,
                                Some(vec![name.clone()]),
                                None,
                                None,
                                None,
                            ));
                        }
                        let scope: Vec<&str> = own_param.iter().map(String::as_str).collect();
                        let outer = VARIANTS.with(|v| std::mem::take(&mut *v.borrow_mut()));
                        let out = lower_type_expr(inner, aliases, entity_names, &scope);
                        VARIANTS.with(|v| *v.borrow_mut() = outer);
                        RESOLVING.with(|r| {
                            r.borrow_mut().remove(name);
                        });
                        out?
                    }
                }
            } else if entity_names.contains(name) {
                // Phase 9.6.A: bare entity reference (`field : EntityName`) lowers
                // to a cardinality:One relation so @almadar/runtime's
                // `populateRelations` can hydrate it on `(fetch X { include: ["field"] })`.
                // Previously these lowered to `FieldType::String` with a TODO marker.
                (
                    FieldType::Relation,
                    None,
                    None,
                    None,
                    Some(RelationDefinition {
                        entity: name.clone(),
                        cardinality: Some(Cardinality::One),
                        entity_id: None,
                        on_delete: None,
                    }),
                )
            } else if let Some(ft) = primitive_field_type(name) {
                // Primitive spelled as a Named node (the parser's
                // `is_primitive_type` list is narrower than the full primitive
                // vocabulary — `json`/`node`/`render-ui` arrive here). Routes
                // `node` to its first-class tag; the erased spellings keep
                // their historical lowering.
                (ft, None, None, None, None)
            } else if let Some(def) =
                orbital_core_base::language::program_types::program_type_field(name)
            {
                (
                    def.field_type.clone(),
                    def.values.clone(),
                    def.items.clone(),
                    def.properties.clone(),
                    def.relation.clone(),
                )
            } else if params.contains(&name.as_str()) {
                (FieldType::type_var(name), None, None, None, None)
            } else {
                let mut known: Vec<String> = aliases.keys().cloned().collect();
                known.extend(entity_names.iter().cloned());
                return Err(LowerError::Type(crate::error::TypeError::UnknownType {
                    name: name.clone(),
                    suggestion: crate::types::closest_name(name, &known),
                    span: crate::ast::Span::default(),
                }));
            }
        }
        TypeExpr::Array(inner) => {
            // Special-case `[EntityName]` (array of entity references): lower to
            // a single Relation field with cardinality:Many rather than an Array
            // wrapper around a foreign-key string. Companion to the bare-entity
            // case above. Type aliases that happen to look like entities still
            // take the regular array-of-struct path because they appear in
            // `aliases`, not in `entity_names`.
            if let TypeExpr::Named(name) = inner.peel() {
                if entity_names.contains(name) && !aliases.contains_key(name) {
                    return Ok((
                        FieldType::Relation,
                        None,
                        None,
                        None,
                        Some(RelationDefinition {
                            entity: name.clone(),
                            cardinality: Some(Cardinality::Many),
                            entity_id: None,
                            on_delete: None,
                        }),
                    ));
                }
            }
            // FC-5b: `[json]` is an array of free JSON values (typically object
            // records). Lower the element to an UNCONSTRAINED object — identical
            // to `[object]` — NOT `{type:"string"}` (the old unknown-name
            // fallback), which mis-typed every object element and tripped
            // `ORB_T_CONFIG_SHAPE_MISMATCH`. Authors wanting typed elements
            // declare a named alias (`type X = { … }`) and write `[X]`.
            // `json` is not in `is_primitive_type`, so `[json]` parses as
            // `Array(Named("json"))`; accept both the Named and Primitive forms.
            let inner_is_json = matches!(
                inner.peel(),
                TypeExpr::Named(p) | TypeExpr::Primitive(p) if p == "json"
            );
            let (inner_ft, inner_values, inner_items, inner_props, inner_rel) = if inner_is_json {
                (FieldType::Object, None, None, None, None)
            } else {
                lower_type_expr(inner, aliases, entity_names, params)?
            };
            let item = FieldDefinition {
                nullable: Some(source_nullable(inner, aliases)),
                additional_properties: source_additional_type(inner, aliases)
                    .map(|ty| lower_source_field(ty, aliases, entity_names, params).map(Box::new))
                    .transpose()?,
                mock: None,
                merged_from: None,
                name: None,
                field_type: inner_ft,
                required: None,
                primary_key: None,
                unique: None,
                default: None,
                values: inner_values,
                relation: inner_rel,
                items: inner_items,
                properties: inner_props,
                intrinsic: None,
                description: None,
                synonyms: None,
                projected_from: inner.projection(),
            };
            (FieldType::Array, None, Some(Box::new(item)), None, None)
        }
        // `[T1, T2, …]` — each slot's resolved shape rides `properties`, keyed by index.
        TypeExpr::Tuple(slots) => {
            let props = slots
                .iter()
                .enumerate()
                .map(|(i, slot)| {
                    let (ft, values, items, properties, relation) =
                        lower_type_expr(slot, aliases, entity_names, params)?;
                    let field = FieldDefinition {
                        nullable: Some(source_nullable(slot, aliases)),
                        additional_properties: source_additional_type(slot, aliases)
                            .map(|ty| {
                                lower_source_field(ty, aliases, entity_names, params).map(Box::new)
                            })
                            .transpose()?,
                        mock: None,
                        merged_from: None,
                        name: Some(i.to_string()),
                        field_type: ft,
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
                        projected_from: slot.projection(),
                    };
                    Ok((i.to_string(), field))
                })
                .collect::<Result<_, LowerError>>()?;
            (FieldType::Tuple, None, None, Some(props), None)
        }
        // `Map K V` — carry the VALUE's resolved shape in the `items` carrier
        // (resolving named-alias values to their struct, exactly like array
        // elements). `resolve_config_field_shape` disambiguates map-vs-array by
        // the `Map<…>` field-type string and inlines this value shape into
        // `OirType::Map { key, value }`. The `FieldType::Object` tag keeps an
        // entity-level map (rare) as a plain object.
        TypeExpr::Map(_key, value) => {
            let (v_ft, v_values, v_items, v_props, v_rel) =
                lower_type_expr(value, aliases, entity_names, params)?;
            // A map keyed to an entity holds that entity's rows, not their ids.
            let v_intrinsic = v_rel.as_ref().map(|_| true);
            let value_field = FieldDefinition {
                nullable: Some(source_nullable(value, aliases)),
                additional_properties: source_additional_type(value, aliases)
                    .map(|ty| lower_source_field(ty, aliases, entity_names, params).map(Box::new))
                    .transpose()?,
                mock: None,
                merged_from: None,
                name: None,
                field_type: v_ft,
                required: None,
                primary_key: None,
                unique: None,
                default: None,
                values: v_values,
                relation: v_rel,
                items: v_items,
                properties: v_props,
                intrinsic: v_intrinsic,
                description: None,
                synonyms: None,
                projected_from: value.projection(),
            };
            (
                FieldType::Object,
                None,
                Some(Box::new(value_field)),
                None,
                None,
            )
        }
        TypeExpr::Object(fields) | TypeExpr::OpenObject { fields, .. } => {
            let mut props = std::collections::HashMap::new();
            for (k, v, req) in fields {
                let (ft, values, items, properties, relation) =
                    lower_type_expr(v, aliases, entity_names, params)?;
                props.insert(
                    k.clone(),
                    FieldDefinition {
                        nullable: Some(source_nullable(v, aliases)),
                        additional_properties: source_additional_type(v, aliases)
                            .map(|ty| {
                                lower_source_field(ty, aliases, entity_names, params).map(Box::new)
                            })
                            .transpose()?,
                        mock: None,
                        merged_from: None,
                        name: Some(k.clone()),
                        field_type: ft,
                        // Mirror the lolo `!` marker — `name : T!` lowers to
                        // `required: Some(true)`, unmarked stays `Some(false)`
                        // so the codegen emits optional-by-default TS fields
                        // matching lolo authoring semantics.
                        required: Some(*req),
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
                        projected_from: v.projection(),
                    },
                );
            }
            (FieldType::Object, None, None, Some(props), None)
        }
        TypeExpr::Union(parts) => {
            // A union whose members are all STRUCT aliases is a tagged union,
            // not a string enum. Checked FIRST: the `all_members` collapse
            // below would otherwise turn `DrawShape | DrawText` into a field
            // whose only legal values are those two literal WORDS — it would
            // accept the variant's name and reject the variant's value.
            if let Some(variants) = struct_union_variants(parts, aliases, entity_names, params)? {
                let names: Vec<String> = variants.iter().map(|(n, _)| n.clone()).collect();
                let props: HashMap<String, FieldDefinition> = variants.into_iter().collect();
                return Ok((FieldType::Union, Some(names), None, Some(props), None));
            }
            // If every variant is a bare Named / Primitive (`a | b | c`) or
            // a string literal (`"a" | "b" | "c"`), treat as a string enum and
            // flatten into `values: [...]` on the field.
            let all_members: Option<Vec<String>> = parts
                .iter()
                .map(|p| match p {
                    TypeExpr::Named(n) => Some(n.clone()),
                    TypeExpr::Primitive(n) => Some(n.clone()),
                    TypeExpr::StringLiteral(s) => Some(s.clone()),
                    _ => None,
                })
                .collect();
            if let Some(values) = all_members {
                (FieldType::String, Some(values), None, None, None)
            } else {
                (FieldType::String, None, None, None, None)
            }
        }
        TypeExpr::StringLiteral(lit) => {
            // A single-literal field type is a closed vocabulary of ONE — the
            // field's only legal value is that literal. Carrying it in `values`
            // (rather than erasing to plain `string`) is what makes
            // `type : "draw-shape"` a usable DISCRIMINATOR: union variant
            // selection reads single-valued vocabularies to decide which
            // variant a value belongs to. Erased, every variant looked
            // untagged and a value whose tag matched nothing was silently
            // accepted as the first variant.
            (FieldType::String, Some(vec![lit.clone()]), None, None, None)
        }
        TypeExpr::Optional(inner) => lower_type_expr(inner, aliases, entity_names, params)?,
        TypeExpr::RelationMany(_) | TypeExpr::RelationManyNonEmpty(_) => {
            (FieldType::Array, None, None, None, None)
        }
    })
}
/// The canonical primitive-type-name set and its `FieldType` mapping — the
/// single source consulted by BOTH the lowerer (`primitive_to_field_type`) and
/// the type checker (`types::is_primitive`), so the two layers can't disagree on
/// what counts as a primitive. `None` for a non-primitive name (an alias /
/// entity reference / typo), which the type checker then rejects.
pub(crate) fn primitive_field_type(name: &str) -> Option<FieldType> {
    FieldType::from_primitive_name(name)
}

pub(crate) fn primitive_to_field_type(name: &str) -> FieldType {
    // The type checker rejects unknown type names up front, so by lowering time
    // every name here is a known primitive; the String fallback is defensive.
    primitive_field_type(name).unwrap_or(FieldType::String)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::TypeExpr;
    use std::collections::{BTreeMap, HashSet};

    fn lower(ty: &TypeExpr) -> LoweredType {
        lower_type_expr(ty, &BTreeMap::new(), &HashSet::new(), &[]).expect("lowers")
    }

    #[test]
    fn json_array_lowers_to_unconstrained_object_items() {
        // FC-5b: `[json]` element must be an unconstrained object (like
        // `[object]`), NOT `{type:"string"}` (which trips ORB_T_CONFIG_SHAPE_MISMATCH).
        // `json` is not in `is_primitive_type`, so it parses as `Named("json")`.
        let ty = TypeExpr::Array(Box::new(TypeExpr::Named("json".to_string())));
        let (ft, _values, items, _props, _rel) = lower(&ty);
        assert!(matches!(ft, FieldType::Array));
        let item = items.expect("array carries an item schema");
        assert!(
            matches!(item.field_type, FieldType::Object),
            "[json] element must lower to an object, got {:?}",
            item.field_type
        );
        assert!(
            item.properties.is_none(),
            "[json] element must be UNCONSTRAINED (no properties)"
        );
    }

    #[test]
    fn node_lowers_first_class_scalar_and_array() {
        // Type-integrity campaign: `node` is renderable content and must keep
        // its tag — never erase to string. `node` is not in
        // `is_primitive_type`, so it parses as `Named("node")` like `json`.
        let (ft, _, _, _, _) = lower(&TypeExpr::Named("node".to_string()));
        assert!(matches!(ft, FieldType::Node), "scalar node keeps its tag");

        let ty = TypeExpr::Array(Box::new(TypeExpr::Named("node".to_string())));
        let (ft, _values, items, _props, _rel) = lower(&ty);
        assert!(matches!(ft, FieldType::Array));
        let item = items.expect("array carries an item schema");
        assert!(
            matches!(item.field_type, FieldType::Node),
            "[node] element must lower to Node, got {:?}",
            item.field_type
        );
    }

    #[test]
    fn program_type_field_lowers_to_its_record_shape() {
        let viewer = orbital_core_base::language::program_types::program_type_field("Viewer")
            .expect("the installed language declares Viewer");
        let (ft, values, items, props, rel) = lower(&TypeExpr::Named("Viewer".to_string()));
        assert!(
            matches!(ft, FieldType::Object),
            "Viewer is a record, got {ft:?}"
        );
        let props = props.expect("Viewer carries its members");
        assert!(
            props.contains_key("id"),
            "Viewer has id: {:?}",
            props.keys()
        );
        assert_eq!(
            props.len(),
            viewer.properties.as_ref().map_or(0, |p| p.len())
        );
        assert_eq!(values, viewer.values);
        assert!(items.is_none() && rel.is_none());

        let (ft, _, items, _, _) = lower(&TypeExpr::Array(Box::new(TypeExpr::Named(
            "Viewer".to_string(),
        ))));
        assert!(matches!(ft, FieldType::Array));
        let item = items.expect("[Viewer] carries an item schema");
        assert!(matches!(item.field_type, FieldType::Object) && item.properties.is_some());
    }

    fn unknown_name(result: Result<LoweredType, LowerError>) -> Option<String> {
        match result {
            Err(LowerError::Type(crate::error::TypeError::UnknownType { name, .. })) => Some(name),
            _ => None,
        }
    }

    #[test]
    fn an_undefined_type_name_is_an_error_not_a_string() {
        let err = lower_type_expr(
            &TypeExpr::Named("Nope".to_string()),
            &BTreeMap::new(),
            &HashSet::new(),
            &[],
        );
        assert_eq!(unknown_name(err), Some("Nope".to_string()));
        let nested = lower_type_expr(
            &TypeExpr::Object(vec![(
                "v".to_string(),
                TypeExpr::Array(Box::new(TypeExpr::Named("Nope".to_string()))),
                false,
            )]),
            &BTreeMap::new(),
            &HashSet::new(),
            &[],
        );
        assert_eq!(unknown_name(nested), Some("Nope".to_string()));
    }

    #[test]
    fn an_alias_type_parameter_is_in_scope_in_its_own_body_only() {
        let mut aliases = BTreeMap::new();
        aliases.insert(
            "Box".to_string(),
            TypeAliasResolved::Type(
                TypeExpr::Object(vec![(
                    "v".to_string(),
                    TypeExpr::Named("T".to_string()),
                    false,
                )]),
                Some("T".to_string()),
            ),
        );
        let (ft, _, _, props, _) = lower_type_expr(
            &TypeExpr::Named("Box".to_string()),
            &aliases,
            &HashSet::new(),
            &[],
        )
        .expect("Box lowers");
        assert!(matches!(ft, FieldType::Object));
        assert_eq!(
            props.expect("Box members")["v"].field_type,
            FieldType::type_var("T")
        );
        let outside = lower_type_expr(
            &TypeExpr::Named("T".to_string()),
            &aliases,
            &HashSet::new(),
            &[],
        );
        assert_eq!(
            unknown_name(outside),
            Some("T".to_string()),
            "control: T is Box's, not global"
        );
    }

    #[test]
    fn a_trait_type_parameter_is_in_scope_in_the_trait() {
        let list = TypeExpr::Array(Box::new(TypeExpr::Named("t".to_string())));
        let (ft, _, items, _, _) =
            lower_type_expr(&list, &BTreeMap::new(), &HashSet::new(), &["t"]).expect("[t] lowers");
        assert!(matches!(ft, FieldType::Array));
        assert_eq!(
            items.expect("[t] item").field_type,
            FieldType::type_var("t")
        );
        let unbound = lower_type_expr(&list, &BTreeMap::new(), &HashSet::new(), &["u"]);
        assert_eq!(
            unknown_name(unbound),
            Some("t".to_string()),
            "control: another param"
        );
    }

    #[test]
    fn a_generic_alias_lowers_its_variable_and_binds_by_substitution() {
        let mut aliases = BTreeMap::new();
        aliases.insert(
            "Page".to_string(),
            TypeAliasResolved::Type(
                TypeExpr::Object(vec![
                    (
                        "items".to_string(),
                        TypeExpr::Array(Box::new(TypeExpr::Named("t".to_string()))),
                        true,
                    ),
                    (
                        "cursor".to_string(),
                        TypeExpr::Optional(Box::new(TypeExpr::Primitive("string".to_string()))),
                        false,
                    ),
                ]),
                Some("t".to_string()),
            ),
        );
        aliases.insert(
            "Plain".to_string(),
            TypeAliasResolved::Type(
                TypeExpr::Object(vec![(
                    "n".to_string(),
                    TypeExpr::Primitive("string".to_string()),
                    true,
                )]),
                None,
            ),
        );
        let entities: HashSet<String> = ["Order".to_string()].into();
        let (_, _, _, props, _) = lower_type_expr(
            &TypeExpr::Named("Page".to_string()),
            &aliases,
            &entities,
            &[],
        )
        .expect("Page");
        let mut page: FieldDefinition =
            serde_json::from_value(serde_json::json!({ "type": "object" })).expect("object");
        page.properties = props;
        let items = &page.properties.as_ref().expect("members")["items"];
        assert_eq!(
            items.items.as_ref().expect("elem").field_type,
            FieldType::type_var("t")
        );
        assert_eq!(
            page.properties.as_ref().expect("members")["cursor"].field_type,
            FieldType::String
        );
        let (oft, _, _, _, orel) = lower_type_expr(
            &TypeExpr::Named("Order".to_string()),
            &aliases,
            &entities,
            &[],
        )
        .expect("Order");
        let mut order: FieldDefinition =
            serde_json::from_value(serde_json::json!({ "type": "object" })).expect("object");
        order.field_type = oft;
        order.relation = orel;
        page.substitute_type_vars(&std::collections::HashMap::from([("t".to_string(), order)]));
        let elem = page.properties.as_ref().expect("members")["items"]
            .items
            .clone()
            .expect("elem");
        assert!(matches!(elem.field_type, FieldType::Relation));
        assert_eq!(elem.relation.expect("rel").entity, "Order");
        let (pft, _, _, pprops, _) = lower_type_expr(
            &TypeExpr::Named("Plain".to_string()),
            &aliases,
            &entities,
            &[],
        )
        .expect("Plain");
        assert!(
            matches!(pft, FieldType::Object),
            "control: a non-generic alias is unchanged"
        );
        assert!(matches!(
            pprops.expect("n")["n"].field_type,
            FieldType::String
        ));
    }

    #[test]
    fn a_union_of_a_primitive_and_records_is_a_union() {
        let record = |key: &str| {
            TypeAliasResolved::Type(
                TypeExpr::Object(vec![(
                    key.to_string(),
                    TypeExpr::Primitive("string".to_string()),
                    true,
                )]),
                None,
            )
        };
        let mut aliases = BTreeMap::new();
        aliases.insert("AnchoredEnv".to_string(), record("env"));
        aliases.insert("AnchoredPath".to_string(), record("path"));
        aliases.insert(
            "CliCommand".to_string(),
            TypeAliasResolved::Type(
                TypeExpr::Union(vec![
                    TypeExpr::Primitive("string".to_string()),
                    TypeExpr::Named("AnchoredEnv".to_string()),
                    TypeExpr::Named("AnchoredPath".to_string()),
                ]),
                None,
            ),
        );
        let list = TypeExpr::Array(Box::new(TypeExpr::Named("CliCommand".to_string())));
        let (_, _, items, _, _) =
            lower_type_expr(&list, &aliases, &HashSet::new(), &[]).expect("lowers");
        let item = items.expect("item schema");
        assert!(
            matches!(item.field_type, FieldType::Union),
            "got {:?}",
            item.field_type
        );
        let variants = item.properties.expect("variants");
        assert!(matches!(variants["string"].field_type, FieldType::String));
        assert!(matches!(
            variants["AnchoredEnv"].field_type,
            FieldType::Object
        ));
    }

    #[test]
    fn inline_record_union_keeps_its_fields_and_recursive_reference() {
        let tree = TypeExpr::Union(vec![
            TypeExpr::Primitive("string".into()),
            TypeExpr::Object(vec![
                (
                    "kind".into(),
                    TypeExpr::StringLiteral("branch".into()),
                    true,
                ),
                (
                    "children".into(),
                    TypeExpr::Array(Box::new(TypeExpr::Named("Tree".into()))),
                    true,
                ),
            ]),
        ]);
        let aliases = BTreeMap::from([("Tree".into(), TypeAliasResolved::Type(tree, None))]);
        let (kind, _, _, properties, _) = lower_type_expr(
            &TypeExpr::Named("Tree".into()),
            &aliases,
            &HashSet::new(),
            &[],
        )
        .unwrap();
        assert_eq!(kind, FieldType::Union);
        let variants = properties.unwrap();
        assert_eq!(variants["string"].field_type, FieldType::String);
        let record = variants["object"].properties.as_ref().unwrap();
        assert_eq!(
            record["kind"].values.as_deref(),
            Some(["branch".to_string()].as_slice())
        );
        assert_eq!(
            record["children"]
                .items
                .as_ref()
                .unwrap()
                .by_name_reference(),
            Some("Tree")
        );
        assert_eq!(record["children"].required, Some(true));
    }

    #[test]
    fn local_alias_and_entity_shadow_a_program_type() {
        let mut aliases = BTreeMap::new();
        aliases.insert(
            "Viewer".to_string(),
            TypeAliasResolved::Enum(vec!["a".to_string(), "b".to_string()]),
        );
        let (ft, values, _, props, _) = lower_type_expr(
            &TypeExpr::Named("Viewer".to_string()),
            &aliases,
            &HashSet::new(),
            &[],
        )
        .expect("lowers");
        assert!(matches!(ft, FieldType::String) && props.is_none());
        assert_eq!(values, Some(vec!["a".to_string(), "b".to_string()]));

        let entities: HashSet<String> = ["Viewer".to_string()].into_iter().collect();
        let (ft, _, _, props, rel) = lower_type_expr(
            &TypeExpr::Named("Viewer".to_string()),
            &BTreeMap::new(),
            &entities,
            &[],
        )
        .expect("lowers");
        assert!(matches!(ft, FieldType::Relation) && props.is_none());
        assert_eq!(rel.map(|r| r.entity), Some("Viewer".to_string()));
    }

    #[test]
    fn scalar_json_stays_string() {
        // The array-element refinement must not change the scalar `json` type.
        let (ft, _, _, _, _) = lower(&TypeExpr::Named("json".to_string()));
        assert!(matches!(ft, FieldType::String), "scalar json stays string");
    }

    /// Recursive named struct types (feature test 1): a PLAIN struct
    /// self-reference — `type MenuItem = { label : string!, subMenu :
    /// [MenuItem] }`, no union involved — must fire the SAME RESOLVING cycle
    /// guard the DrawItem tagged-union recursion introduced it for. Before
    /// this feature the guard already covered `TypeExpr::Named` generically
    /// (not union-specific), so this pins that behavior rather than changing
    /// it: full expansion at the root, a by-name `Union` back-reference
    /// (`values: ["MenuItem"]`, no `properties`) at the recursion point.
    #[test]
    fn plain_struct_self_reference_lowers_to_full_expansion_plus_backref() {
        let mut aliases = BTreeMap::new();
        aliases.insert(
            "MenuItem".to_string(),
            TypeAliasResolved::Type(
                TypeExpr::Object(vec![
                    (
                        "label".to_string(),
                        TypeExpr::Primitive("string".to_string()),
                        true,
                    ),
                    (
                        "subMenu".to_string(),
                        TypeExpr::Array(Box::new(TypeExpr::Named("MenuItem".to_string()))),
                        false,
                    ),
                ]),
                None,
            ),
        );
        let (ft, _values, _items, props, _rel) = lower_type_expr(
            &TypeExpr::Named("MenuItem".to_string()),
            &aliases,
            &HashSet::new(),
            &[],
        )
        .expect("lowers");
        assert!(
            matches!(ft, FieldType::Object),
            "root MenuItem expands fully to an object, got {:?}",
            ft
        );
        let props = props.expect("root struct carries its fields");
        let sub_menu = props.get("subMenu").expect("subMenu field present");
        assert!(matches!(sub_menu.field_type, FieldType::Array));
        let item = sub_menu
            .items
            .as_ref()
            .expect("subMenu is an array with an item schema");
        assert!(
            matches!(item.field_type, FieldType::Union),
            "the recursion point degrades to the by-name Union back-reference, got {:?}",
            item.field_type
        );
        assert_eq!(
            item.values.as_deref(),
            Some(["MenuItem".to_string()].as_slice()),
            "the back-reference names the alias it re-enters"
        );
        assert!(
            item.properties.is_none(),
            "a back-reference carries NO properties — it's a reference, not an expansion"
        );
        // The guard must be fully released after lowering completes, so a
        // SECOND top-level use of the same alias (e.g. a sibling field on the
        // same entity) also gets the full expansion, not a spurious
        // back-reference from a leaked guard entry.
        let (ft2, _, _, props2, _) = lower_type_expr(
            &TypeExpr::Named("MenuItem".to_string()),
            &aliases,
            &HashSet::new(),
            &[],
        )
        .expect("lowers");
        assert!(matches!(ft2, FieldType::Object));
        assert!(
            props2.is_some(),
            "guard release lets a second use re-expand"
        );
    }

    fn predicate_aliases(not_of: TypeExpr) -> BTreeMap<String, TypeAliasResolved> {
        let named = |n: &str| TypeExpr::Named(n.to_string());
        let tagged = |tag: &str, of: Option<TypeExpr>| {
            let mut fields = vec![(
                "kind".to_string(),
                TypeExpr::StringLiteral(tag.to_string()),
                true,
            )];
            fields.extend(of.map(|t| ("of".to_string(), t, true)));
            TypeAliasResolved::Type(TypeExpr::Object(fields), None)
        };
        let mut aliases = BTreeMap::new();
        aliases.insert("Leaf".to_string(), tagged("leaf", None));
        aliases.insert("Not".to_string(), tagged("not", Some(not_of)));
        aliases.insert(
            "Pred".to_string(),
            TypeAliasResolved::Type(TypeExpr::Union(vec![named("Leaf"), named("Not")]), None),
        );
        aliases
    }

    #[test]
    fn an_inline_union_re_entering_its_own_variant_is_a_type_cycle() {
        let inline = TypeExpr::Union(vec![
            TypeExpr::Named("Leaf".to_string()),
            TypeExpr::Named("Not".to_string()),
        ]);
        let aliases = predicate_aliases(inline);
        for root in ["Not", "Pred"] {
            let err = lower_type_expr(
                &TypeExpr::Named(root.to_string()),
                &aliases,
                &HashSet::new(),
                &[],
            )
            .unwrap_err();
            match err {
                LowerError::Type(crate::error::TypeError::CircularType { cycle, .. }) => {
                    assert_eq!(cycle, vec!["Not".to_string(), "Not".to_string()], "{root}");
                }
                other => panic!("ORB_T_TYPE_CYCLE from {root}, got {other:?}"),
            }
        }
    }

    #[test]
    fn control_a_variant_reached_again_through_a_named_alias_still_lowers() {
        let aliases = predicate_aliases(TypeExpr::Named("Pred".to_string()));
        for root in ["Not", "Pred"] {
            lower_type_expr(
                &TypeExpr::Named(root.to_string()),
                &aliases,
                &HashSet::new(),
                &[],
            )
            .unwrap_or_else(|e| panic!("{root} lowers: {e:?}"));
        }
    }

    /// DrawItem regression (feature test 4): the pre-existing TAGGED-UNION
    /// recursion — `DrawItem = DrawShape | DrawGroup`, `DrawGroup.items :
    /// [DrawItem]` — must lower to the EXACT same `.orb` wire shape it did
    /// before this feature: `FieldType::Union` at the root with variant names
    /// in `values` and shapes in `properties`, and the SAME by-name
    /// `FieldType::Union { values: ["DrawItem"], properties: None }`
    /// back-reference at the recursion point that a plain struct
    /// self-reference now also produces. This feature changes what
    /// `orbital-compiler`'s resolve phase does with that back-reference
    /// (`Json` → `TypeRef`) — never the lowered bytes themselves.
    #[test]
    fn tagged_union_self_reference_still_lowers_to_by_name_backref() {
        let mut aliases = BTreeMap::new();
        aliases.insert(
            "DrawShape".to_string(),
            TypeAliasResolved::Type(
                TypeExpr::Object(vec![(
                    "kind".to_string(),
                    TypeExpr::StringLiteral("shape".to_string()),
                    true,
                )]),
                None,
            ),
        );
        aliases.insert(
            "DrawGroup".to_string(),
            TypeAliasResolved::Type(
                TypeExpr::Object(vec![
                    (
                        "kind".to_string(),
                        TypeExpr::StringLiteral("group".to_string()),
                        true,
                    ),
                    (
                        "items".to_string(),
                        TypeExpr::Array(Box::new(TypeExpr::Named("DrawItem".to_string()))),
                        false,
                    ),
                ]),
                None,
            ),
        );
        aliases.insert(
            "DrawItem".to_string(),
            TypeAliasResolved::Type(
                TypeExpr::Union(vec![
                    TypeExpr::Named("DrawShape".to_string()),
                    TypeExpr::Named("DrawGroup".to_string()),
                ]),
                None,
            ),
        );
        let (ft, values, _items, props, _rel) = lower_type_expr(
            &TypeExpr::Named("DrawItem".to_string()),
            &aliases,
            &HashSet::new(),
            &[],
        )
        .expect("lowers");
        assert!(
            matches!(ft, FieldType::Union),
            "root DrawItem is a tagged union, got {:?}",
            ft
        );
        let names = values.expect("union carries variant names");
        assert_eq!(
            names,
            vec!["DrawShape".to_string(), "DrawGroup".to_string()]
        );
        let props = props.expect("union carries variant shapes");
        let group = props.get("DrawGroup").expect("DrawGroup variant present");
        let group_props = group.properties.as_ref().expect("DrawGroup fields");
        let items_field = group_props.get("items").expect("items field");
        let item = items_field
            .items
            .as_ref()
            .expect("items is an array with an element schema");
        assert!(
            matches!(item.field_type, FieldType::Union),
            "the recursion point re-enters DrawItem as a by-name back-reference, got {:?}",
            item.field_type
        );
        assert_eq!(
            item.values.as_deref(),
            Some(["DrawItem".to_string()].as_slice())
        );
        assert!(
            item.properties.is_none(),
            "a back-reference carries NO properties — unchanged from before this feature"
        );
    }

    fn assert_backref(field: &FieldDefinition, name: &str) {
        assert!(matches!(field.field_type, FieldType::Union), "{field:?}");
        assert_eq!(field.values.as_deref(), Some([name.to_string()].as_slice()));
        assert!(field.properties.is_none());
        assert_eq!(field.by_name_reference(), Some(name));
    }

    /// `type RowCell = string | number | [RowCell] | Map string RowCell`: a type
    /// union (a `Map` member is structural, never a vocabulary word) whose list and
    /// map variants re-enter `RowCell` by name.
    #[test]
    fn recursive_union_through_list_and_map_lowers_to_by_name_refs() {
        let row_cell = |value: TypeExpr| {
            TypeExpr::Union(vec![
                TypeExpr::Primitive("string".to_string()),
                TypeExpr::Primitive("number".to_string()),
                TypeExpr::Array(Box::new(value.clone())),
                TypeExpr::Map(
                    Box::new(TypeExpr::Primitive("string".to_string())),
                    Box::new(value),
                ),
            ])
        };
        let mut aliases = BTreeMap::new();
        aliases.insert(
            "RowCell".to_string(),
            TypeAliasResolved::Type(row_cell(TypeExpr::Named("RowCell".to_string())), None),
        );
        let (ft, values, _items, props, _rel) = lower_type_expr(
            &TypeExpr::Named("RowCell".to_string()),
            &aliases,
            &HashSet::new(),
            &[],
        )
        .expect("lowers");
        assert!(matches!(ft, FieldType::Union), "got {ft:?}");
        assert_eq!(
            values.as_deref(),
            Some(
                ["string", "number", "array", "map"]
                    .map(str::to_string)
                    .as_slice()
            )
        );
        let props = props.expect("variant shapes");
        let list = props.get("array").expect("list variant");
        assert!(matches!(list.field_type, FieldType::Array));
        assert_backref(list.items.as_deref().expect("list element"), "RowCell");
        let map = props.get("map").expect("map variant");
        assert!(matches!(map.field_type, FieldType::Object));
        assert_backref(map.items.as_deref().expect("map value"), "RowCell");

        // Control: the same union over a non-recursive value expands it inline.
        let (_, _, _, plain, _) = lower_type_expr(
            &row_cell(TypeExpr::Primitive("number".to_string())),
            &BTreeMap::new(),
            &HashSet::new(),
            &[],
        )
        .expect("lowers");
        let plain = plain.expect("variant shapes");
        let value = plain["map"].items.as_deref().expect("map value");
        assert!(matches!(value.field_type, FieldType::Number));
        assert_eq!(value.by_name_reference(), None);
    }

    fn lower_with_asset(ty: &TypeExpr) -> LoweredType {
        let entities: HashSet<String> = ["Asset".to_string()].into_iter().collect();
        lower_type_expr(ty, &BTreeMap::new(), &entities, &[]).expect("lowers")
    }

    fn map_of(value: TypeExpr) -> TypeExpr {
        TypeExpr::Map(
            Box::new(TypeExpr::Primitive("string".to_string())),
            Box::new(value),
        )
    }

    fn asset() -> TypeExpr {
        TypeExpr::Named("Asset".to_string())
    }

    fn assert_row_relation(value: &FieldDefinition, cardinality: Cardinality) {
        assert!(
            matches!(value.field_type, FieldType::Relation),
            "got {:?}",
            value.field_type
        );
        let rel = value
            .relation
            .as_ref()
            .expect("a map value keeps its relation definition");
        assert_eq!(rel.entity, "Asset");
        assert_eq!(rel.cardinality, Some(cardinality));
        assert_eq!(
            value.intrinsic,
            Some(true),
            "a map value holds the row, not its id"
        );
    }

    #[test]
    fn map_of_entity_carries_a_row_relation_value() {
        let (ft, _, items, props, rel) = lower_with_asset(&map_of(asset()));
        assert!(matches!(ft, FieldType::Object) && props.is_none() && rel.is_none());
        assert_row_relation(items.as_deref().expect("map value"), Cardinality::One);

        let (_, _, items, _, _) = lower_with_asset(&map_of(TypeExpr::Array(Box::new(asset()))));
        assert_row_relation(items.as_deref().expect("map value"), Cardinality::Many);

        let (_, _, items, _, _) =
            lower_with_asset(&map_of(TypeExpr::Primitive("string".to_string())));
        let control = items.expect("map value");
        assert!(matches!(control.field_type, FieldType::String));
        assert!(control.relation.is_none() && control.intrinsic.is_none());
    }

    #[test]
    fn nested_map_and_struct_member_of_entity_keep_the_relation() {
        let (_, _, items, _, _) = lower_with_asset(&map_of(map_of(asset())));
        let inner = items.expect("outer map value");
        assert!(matches!(inner.field_type, FieldType::Object) && inner.relation.is_none());
        assert_row_relation(
            inner.items.as_deref().expect("inner map value"),
            Cardinality::One,
        );

        let member = TypeExpr::Object(vec![
            (
                "name".to_string(),
                TypeExpr::Primitive("string".to_string()),
                false,
            ),
            ("byKey".to_string(), map_of(asset()), false),
        ]);
        let (_, _, _, props, _) = lower_with_asset(&member);
        let props = props.expect("struct members");
        let by_key = props.get("byKey").expect("byKey member");
        assert_row_relation(
            by_key.items.as_deref().expect("member map value"),
            Cardinality::One,
        );

        let (_, _, items, _, _) = lower_with_asset(&TypeExpr::Array(Box::new(map_of(asset()))));
        let element = items.expect("list element");
        assert_row_relation(
            element.items.as_deref().expect("element map value"),
            Cardinality::One,
        );

        let (_, _, items, _, _) = lower_with_asset(&TypeExpr::Array(Box::new(TypeExpr::Array(
            Box::new(asset()),
        ))));
        let element = items.expect("outer list element");
        let rel = element
            .relation
            .as_ref()
            .expect("[[Asset]] element keeps its relation");
        assert_eq!(rel.cardinality, Some(Cardinality::Many));
        assert_eq!(
            element.intrinsic, None,
            "a list element relation stays an id list"
        );
    }
}

/// Does `parts` read as a TYPE union rather than a string enum?
///
/// **This is the single reading of `A | B | C`, and every site that has to tell
/// the two apart must call it.** The two readings are mutually exclusive and
/// wildly different — a type union accepts each member's VALUES, a string enum
/// accepts each member's NAME as a literal word — so a second, separately-worded
/// copy of this rule is not a duplicate but a fork: the field lowers as a union
/// while a checker still measures it against the vocabulary, and the diagnostic
/// contradicts the type. That is exactly what
/// `checks::enum_members_of` used to do.
///
/// The rule is ALL-OR-NOTHING by design. One member that names no known type
/// and the whole union stays the enum it was authored as. That is what protects
/// the live corpus: `format : date | currency | number | boolean | percent` has
/// three primitive-NAMED members but `currency` names no type, so it keeps its
/// vocabulary reading, and `type Direction = up | down | left | right` has none
/// at all. Swept before this shipped — exactly one corpus union had
/// all-primitive members, and it was inside a `@description` string.
///
/// A member counts as a type when it is a struct alias, a primitive name, or a
/// structural expression (`[string]`, `Map K V`), which needs no alias to be unambiguous.
pub(crate) fn is_type_union(
    parts: &[TypeExpr],
    aliases: &BTreeMap<String, TypeAliasResolved>,
) -> bool {
    is_type_union_with(parts, |name| match aliases.get(name) {
        Some(TypeAliasResolved::Type(inner, _)) if inner.record_fields().is_some() => Some(true),
        Some(_) => Some(false),
        None => None,
    })
}

/// Core of [`is_type_union`], parameterized over alias lookup so the L1 check
/// phase (`types.rs`, which sees `ResolveCtx` rather than the lowered alias
/// map) applies the ONE predicate instead of keeping a copy.
/// `named_is_struct`: `Some(true)` = resolves to a struct alias, `Some(false)`
/// = resolves to any other alias kind, `None` = unknown name (falls through to
/// the primitive-name check).
pub(crate) fn is_type_union_with(
    parts: &[TypeExpr],
    named_is_struct: impl Fn(&str) -> Option<bool>,
) -> bool {
    fn variant(ty: &TypeExpr, named: &dyn Fn(&str) -> Option<bool>) -> bool {
        match ty {
            TypeExpr::Optional(inner) => variant(inner, named),
            TypeExpr::Named(name) => {
                named(name).unwrap_or_else(|| primitive_field_type(name).is_some())
            }
            TypeExpr::Primitive(name) => primitive_field_type(name).is_some(),
            TypeExpr::Array(_)
            | TypeExpr::Map(..)
            | TypeExpr::Object(_)
            | TypeExpr::OpenObject { .. } => true,
            _ => false,
        }
    }
    parts.len() >= 2 && parts.iter().all(|part| variant(part, &named_is_struct))
}

/// The variants of a TYPE union, as `(variant name, variant shape)` in
/// declaration order. `None` when [`is_type_union`] says this is a string enum,
/// in which case the caller falls back to the enum collapse.
///
/// ⚠️ Recursion is carried BY NAME, not inlined. `DrawGroup.items : [DrawItem]`
/// refers back to the union being lowered; inlining it would recurse forever
/// (`TypeAliasResolved::Type` lowering has no cycle guard). `lower_type_expr`
/// is therefore called on the variant body with the union's own members marked
/// in-progress, and a back-reference lowers to `FieldType::Union` carrying
/// `values` (the names) and NO `properties` — a reference, not an expansion.
fn struct_union_variants(
    parts: &[TypeExpr],
    aliases: &BTreeMap<String, TypeAliasResolved>,
    entity_names: &HashSet<String>,
    params: &[&str],
) -> Result<Option<Vec<(String, FieldDefinition)>>, LowerError> {
    if parts.len() < 2 {
        return Ok(None);
    }
    if !is_type_union(parts, aliases) {
        return Ok(None);
    }
    let mut out = Vec::with_capacity(parts.len());
    for p in parts {
        let mut candidate = p;
        while let TypeExpr::Optional(inner) = candidate {
            candidate = inner;
        }
        let (variant_name, inner, scope): (String, TypeExpr, Vec<&str>) = match candidate {
            TypeExpr::Named(name) => match aliases.get(name) {
                Some(TypeAliasResolved::Type(inner, own_param))
                    if inner.record_fields().is_some() =>
                {
                    (
                        name.clone(),
                        inner.clone(),
                        own_param.iter().map(String::as_str).collect(),
                    )
                }
                // A named PRIMITIVE (`string`, `number`, `node`, …).
                None if primitive_field_type(name).is_some() => {
                    (name.clone(), TypeExpr::Primitive(name.clone()), Vec::new())
                }
                _ => return Ok(None),
            },
            TypeExpr::Primitive(name) if primitive_field_type(name).is_some() => {
                (name.clone(), TypeExpr::Primitive(name.clone()), Vec::new())
            }
            // `[string]` and friends — a structural variant needs no alias.
            TypeExpr::Array(_) => ("array".to_string(), p.clone(), params.to_vec()),
            TypeExpr::Map(..) => ("map".to_string(), p.clone(), params.to_vec()),
            TypeExpr::Object(_) | TypeExpr::OpenObject { .. } => {
                ("object".to_string(), p.clone(), params.to_vec())
            }
            _ => return Ok(None),
        };
        let name = &variant_name;
        let inner = &inner;
        let guarded = matches!(candidate, TypeExpr::Named(_));
        if guarded {
            if let Some(cycle) = VARIANTS.with(|v| {
                let v = v.borrow();
                v.iter().position(|n| n == name).map(|at| {
                    let mut cycle = v[at..].to_vec();
                    cycle.push(name.clone());
                    cycle
                })
            }) {
                return Err(LowerError::Type(crate::error::TypeError::CircularType {
                    cycle,
                    span: crate::ast::Span::default(),
                }));
            }
            VARIANTS.with(|v| v.borrow_mut().push(name.clone()));
        }
        let lowered = lower_type_expr(inner, aliases, entity_names, &scope);
        if guarded {
            VARIANTS.with(|v| v.borrow_mut().pop());
        }
        let (ft, values, items, props, relation) = lowered?;
        out.push((
            name.clone(),
            FieldDefinition {
                nullable: Some(source_nullable(p, aliases)),
                additional_properties: source_additional_type(p, aliases)
                    .map(|ty| lower_source_field(ty, aliases, entity_names, &scope).map(Box::new))
                    .transpose()?,
                mock: None,
                merged_from: None,
                name: Some(name.clone()),
                field_type: ft,
                required: None,
                primary_key: None,
                unique: None,
                default: None,
                values,
                relation,
                items,
                properties: props,
                intrinsic: None,
                description: None,
                synonyms: None,
                projected_from: None,
            },
        ));
    }
    Ok(Some(out))
}
