//! Lower a `.lolo` AST into an `OrbitalSchema` from `orbital-core`.
//!
//! The lowering is mechanical and (when both inputs are canonical) byte
//! identical to a hand-authored `.orb`. See `Lolo_Language_Spec_v3.md`
//! "Lowering to .orb" table for the rules.

use crate::ast::*;
use crate::error::LowerError;
use crate::patterns_meta::event_prop_names;
use crate::types::check_program;
use orbital_core::schema::{
    Cardinality, ConfigField as SchemaConfigField, EmitDefinition, EntityDefinition,
    EntityFieldContract, EntityRef, EventDefinition,
    EventScope, FieldDefinition, FieldType, ListenDefinition, ListenSource,
    Orbital as CoreOrbital, OrbitalSchema, PageDefinition, PageRef, PageRefObject, PayloadField,
    Persistence as CorePersistence, RelationDefinition, SExpression, StateDefinition, StateMachine,
    StringOrArray, StringOrNumber, TickDefinition, TraitCategory, TraitDefinition, TraitReference,
    TransitionDefinition, UseDeclaration,
};
use std::collections::{BTreeMap, HashMap, HashSet};

/// Lower an entire `.lolo` program into an `OrbitalSchema`.
pub fn lower_to_orb(ast: &Program) -> Result<OrbitalSchema, LowerError> {
    // Phase 0: run the type-system resolver. Any unknown type / cycle /
    // kind-mismatch / composition-conflict / annotation-mismatch is reported
    // as `LowerError::Type(..)` before lowering proceeds. Types are stripped
    // after this — the emitted `.orb` is byte-identical to a `.lolo` that
    // declared no types at all.
    check_program(ast).map_err(LowerError::Type)?;

    // Build a global map of type aliases (file-level) for enum substitution.
    let mut type_aliases: BTreeMap<String, TypeAliasResolved> = BTreeMap::new();
    for ta in &ast.type_aliases {
        type_aliases.insert(ta.name.clone(), resolve_type_alias(&ta.body));
    }

    // Entity-shaped type aliases retained in their full field form so the
    // emit-payload lowerer can flatten `:: TypeName` references. The regular
    // `type_aliases` table loses struct fields (collapsed to "object") because
    // those callers only need enum/primitive info; the typed-payload flattener
    // needs the original field list.
    let file_entity_bodies = collect_entity_type_bodies(&ast.type_aliases);
    let file_event_aliases = collect_event_type_bodies(&ast.type_aliases);

    // Phase 9.6.A: collect the set of names that designate ENTITIES (relation
    // targets) — distinct from type aliases that happen to be record-shaped.
    // Members: every orbital's primary `entity` block in the program, plus any
    // type alias whose body is `TypeBody::Entity` (declared with the `entity`
    // keyword). `lower_type_expr` uses this to decide whether `[Name]` lowers
    // to a `cardinality: Many` Relation field (entity) or a regular array of
    // an inline object schema (struct alias).
    let mut file_entity_names: HashSet<String> = HashSet::new();
    for orb in &ast.orbitals {
        if let Some(e) = &orb.entity {
            file_entity_names.insert(e.name.clone());
        }
    }
    for ta in &ast.type_aliases {
        if matches!(ta.body, TypeBody::Entity { .. }) {
            file_entity_names.insert(ta.name.clone());
        }
    }

    let mut orbitals = Vec::with_capacity(ast.orbitals.len());
    for orb in &ast.orbitals {
        // Merge orbital-scoped type aliases on top of file-level ones.
        let mut local_aliases = type_aliases.clone();
        for ta in &orb.type_aliases {
            local_aliases.insert(ta.name.clone(), resolve_type_alias(&ta.body));
        }
        let mut local_entities = file_entity_bodies.clone();
        local_entities.extend(collect_entity_type_bodies(&orb.type_aliases));
        let mut local_events = file_event_aliases.clone();
        local_events.extend(collect_event_type_bodies(&orb.type_aliases));
        // Per-orbital entity-name set: file-level set + any orbital-scoped
        // `type X = Entity { ... }` aliases.
        let mut local_entity_names = file_entity_names.clone();
        for ta in &orb.type_aliases {
            if matches!(ta.body, TypeBody::Entity { .. }) {
                local_entity_names.insert(ta.name.clone());
            }
        }
        // The orbital's own entity is also addressable by name in emit payloads
        // (`emits { SAVED :: CartItem }` where CartItem is the orbital entity).
        if let Some(e) = &orb.entity {
            local_entities.insert(
                e.name.clone(),
                e.fields
                    .iter()
                    .map(|f| (f.name.clone(), f.type_expr.clone(), f.required))
                    .collect(),
            );
        }
        orbitals.push(lower_orbital(
            orb,
            &local_aliases,
            &local_entities,
            &local_entity_names,
            &local_events,
        )?);
    }

    check_listens_integrity(&orbitals)?;
    check_button_transitions(&orbitals)?;
    check_navigate_targets(&orbitals)?;
    check_emit_payload_literals(&orbitals)?;

    Ok(OrbitalSchema {
        name: if ast.app.name.is_empty() {
            ast.orbitals
                .first()
                .map(|o| o.name.clone())
                .unwrap_or_else(|| "App".to_string())
        } else {
            ast.app.name.clone()
        },
        version: ast.app.version.clone(),
        description: ast.app.description.clone(),
        orbitals,
        metadata: None,
    })
}

#[derive(Debug, Clone)]
enum TypeAliasResolved {
    /// `type X = a | b | c` lowers to a string field with values [...].
    Enum(Vec<String>),
    /// Other type aliases are passthrough — we record the resolved type
    /// expression and re-evaluate it on demand.
    Type(TypeExpr),
}

/// Phase 2.5: resolved event type alias.
#[derive(Debug, Clone)]
pub(crate) struct EventTypeResolved {
    /// Payload type expression from the `type X = Event <TypeExpr>`
    /// declaration. Flattened by the lowerer at emit lookup time using
    /// the same rules as Form A `:: TypeName` (named entities expand
    /// into their fields; anonymous records expand verbatim; primitives
    /// become a single `value` field).
    pub payload: TypeExpr,
    /// Declared event scope. `None` means the declaration carried no
    /// bracket modifier; lowerer defaults to internal.
    pub scope: Option<EventScopeAst>,
    /// Optional description from `type X = Event T "description"`.
    /// Flows through to `EmitDefinition.description` on every reference.
    pub description: Option<String>,
}

/// Collect `type X = Event <T> [scope] "description"` aliases into a lookup
/// table. Mirrors `collect_entity_type_bodies`'s scoping: aliases live at
/// file scope and are extended per-orbital from `orb.type_aliases`.
fn collect_event_type_bodies(
    aliases: &[TypeAlias],
) -> BTreeMap<String, EventTypeResolved> {
    let mut out: BTreeMap<String, EventTypeResolved> = BTreeMap::new();
    for ta in aliases {
        if let TypeBody::Event {
            payload,
            scope,
            description,
        } = &ta.body
        {
            out.insert(
                ta.name.clone(),
                EventTypeResolved {
                    payload: payload.clone(),
                    scope: *scope,
                    description: description.clone(),
                },
            );
        }
    }
    out
}

/// Build a map from alias name to its entity-shaped field list.
///
/// Only `type X = Entity { ... }` and `type X = { ... }` (struct form)
/// contribute; other alias shapes are irrelevant to emit-payload flattening.
/// Returns (field_name, type_expr, required) triples so the caller can emit
/// them as primitive `PayloadField` entries.
fn collect_entity_type_bodies(
    aliases: &[TypeAlias],
) -> BTreeMap<String, Vec<(String, TypeExpr, bool)>> {
    let mut out: BTreeMap<String, Vec<(String, TypeExpr, bool)>> = BTreeMap::new();
    for ta in aliases {
        match &ta.body {
            TypeBody::Entity { fields, .. } => {
                out.insert(
                    ta.name.clone(),
                    fields
                        .iter()
                        .map(|f| (f.name.clone(), f.type_expr.clone(), f.required))
                        .collect(),
                );
            }
            TypeBody::Struct(items) => {
                out.insert(
                    ta.name.clone(),
                    items
                        .iter()
                        .map(|(n, t, req)| (n.clone(), t.clone(), *req))
                        .collect(),
                );
            }
            _ => {}
        }
    }
    out
}

fn resolve_type_alias(body: &TypeBody) -> TypeAliasResolved {
    match body {
        TypeBody::Enum(v) => TypeAliasResolved::Enum(v.clone()),
        TypeBody::Alias(t) => TypeAliasResolved::Type(t.clone()),
        // Preserve struct field shape by re-projecting onto TypeExpr::Object
        // — `TypeExpr::Object` and `TypeBody::Struct` share `Vec<(String, TypeExpr)>`,
        // so referenced struct aliases lower to a structured object instead of
        // a flat `object` primitive (which previously collapsed to `Json`).
        TypeBody::Struct(items) => TypeAliasResolved::Type(TypeExpr::Object(items.clone())),
        // Shape aliases (Entity/Trait/Page/Orbital/Composition) are not
        // field-type-resolvable — they live in the type-check pass, not in
        // field lowering. Treat them as opaque object for any accidental
        // field reference until Stage 2 wires the real resolver.
        TypeBody::Entity { .. }
        | TypeBody::Trait { .. }
        | TypeBody::Page { .. }
        | TypeBody::Orbital { .. }
        | TypeBody::Event { .. }
        | TypeBody::Composition(_) => {
            TypeAliasResolved::Type(TypeExpr::Primitive("object".to_string()))
        }
    }
}

fn lower_orbital(
    orb: &Orbital,
    aliases: &BTreeMap<String, TypeAliasResolved>,
    entities: &BTreeMap<String, Vec<(String, TypeExpr, bool)>>,
    entity_names: &HashSet<String>,
    event_aliases: &BTreeMap<String, EventTypeResolved>,
) -> Result<CoreOrbital, LowerError> {
    let entity = orb.entity.as_ref().ok_or(LowerError::MissingEntity)?;
    let entity_def = lower_entity(entity, aliases, entity_names)?;

    // Reject lolo where an entity's name matches a sibling trait's
    // generated `<TraitName>State` or `<TraitName>Event` union — the TS
    // codegen would emit both a `type <Name>State = …` declaration AND
    // `import type { <Name>State } from '@app/shared'`, tripping
    // TS2440 "Import declaration conflicts with local declaration".
    // The convention fix is to rename the entity (e.g.
    // `<TraitName>Data`); catching this at lower time keeps the issue
    // visible at author time instead of surfacing only when orbital-verify
    // typechecks the generated shell.
    for t in &orb.traits {
        let state_shadow = format!("{}State", t.name);
        let event_shadow = format!("{}Event", t.name);
        if entity_def.name == state_shadow {
            return Err(LowerError::EntityNameShadowsTraitUnion {
                orbital: orb.name.clone(),
                entity_name: entity_def.name.clone(),
                trait_name: t.name.clone(),
                kind: "state".to_string(),
            });
        }
        if entity_def.name == event_shadow {
            return Err(LowerError::EntityNameShadowsTraitUnion {
                orbital: orb.name.clone(),
                entity_name: entity_def.name.clone(),
                trait_name: t.name.clone(),
                kind: "event".to_string(),
            });
        }
    }

    let uses = if orb.uses.is_empty() {
        None
    } else {
        Some(
            orb.uses
                .iter()
                .map(|u| UseDeclaration {
                    from: u.path.clone(),
                    alias: u.alias.clone(),
                })
                .collect::<Vec<_>>(),
        )
    };

    // Intrinsic field names per entity — view-state/machinery the owning trait
    // computes itself, marked `@intrinsic` in the .lolo. These are excluded from
    // a rebindable trait's `entity_contract.requires` so the post-inline rebind
    // validator does NOT demand them on the rebind target (they travel WITH the
    // trait via the inline field-merge). Collected from the orbital's primary
    // entity (the only entity a `@rebindable` trait binds in std/app atoms).
    let mut intrinsic_fields: HashSet<String> = HashSet::new();
    for f in &entity.fields {
        if f.is_intrinsic {
            intrinsic_fields.insert(f.name.clone());
        }
    }

    let mut traits = Vec::new();
    for t in &orb.traits {
        traits.push(lower_trait_decl(
            t,
            aliases,
            entities,
            entity_names,
            event_aliases,
            &intrinsic_fields,
        )?);
    }

    // Top-level s-expressions (e.g. `(behavior/compose ...)`) are surfaced as
    // a synthetic trait `__Composition` with a single `INIT` transition whose
    // effects array contains the calls verbatim. The existing Compose pass in
    // `orbital-compiler` walks transitions and resolves `behavior/*` calls in
    // place. After resolution, the synthetic trait carries the resolved
    // schema fragment as an effect; downstream passes ignore unrecognised
    // effect operators inside ignored synthetic traits.
    //
    // TODO(lolo, conservative): once a richer surface for orbital-body
    // composition lands in the core schema, replace this with a dedicated
    // field. For now this is the minimal change that keeps the call visible
    // to the Compose pass.
    if !orb.body.is_empty() {
        let synth = TraitDefinition {
            name: "__Composition".to_string(),
            entity_rebindable: None,
            entity_contract: None,
            category: None,
            capabilities: Vec::new(),
            description: None,
            linked_entity: Some(entity_def.name.clone()),
            emits: None,
            listens: None,
            state_machine: StateMachine {
                states: vec![StateDefinition {
                    name: "init".to_string(),
                    is_initial: Some(true),
                    is_terminal: None,
                    description: None,
                }],
                events: Some(vec![EventDefinition {
                    key: "INIT".to_string(),
                    name: None,
                    description: None,
                    synonyms: None,
                    tier: None,
                    payload_schema: None,
                }]),
                transitions: vec![TransitionDefinition {
                    from: StringOrArray::Single("init".to_string()),
                    to: "init".to_string(),
                    event: "INIT".to_string(),
                    guard: None,
                    effects: Some(orb.body.clone()),
                }],
            },
            ticks: None,
            config: None,
            data_entities: None,
            scope: None,
            source_behavior: None,
            source_entity_definition: None,
        };
        traits.push(TraitReference::Inline(synth));
    }

    let pages = if orb.pages.is_empty() {
        None
    } else {
        Some(orb.pages.iter().map(lower_page_decl).collect::<Vec<_>>())
    };

    Ok(CoreOrbital {
        name: orb.name.clone(),
        uses,
        theme: None,
        services: None,
        entity: EntityRef::Inline(entity_def),
        auxiliary_entities: None,
        traits,
        pages,
        emits: None,
        listens: None,
        exposes: None,
        domain_context: None,
        design: None,
    })
}

fn lower_entity(
    entity: &Entity,
    aliases: &BTreeMap<String, TypeAliasResolved>,
    entity_names: &HashSet<String>,
) -> Result<EntityDefinition, LowerError> {
    let mut fields = Vec::with_capacity(entity.fields.len());
    for f in &entity.fields {
        fields.push(lower_field(f, aliases, entity_names)?);
    }
    let persistence = match &entity.persistence {
        Some(Persistence::Persistent) => Some(CorePersistence::Persistent),
        Some(Persistence::Runtime) => Some(CorePersistence::Runtime),
        Some(Persistence::Singleton) => Some(CorePersistence::Singleton),
        Some(Persistence::Instance) => Some(CorePersistence::Instance),
        Some(Persistence::Local) => Some(CorePersistence::Runtime),
        None => None,
    };
    Ok(EntityDefinition {
        name: entity.name.clone(),
        collection: entity.collection.clone(),
        persistence,
        fields,
        instances: None,
    })
}

fn lower_field(
    f: &EntityField,
    aliases: &BTreeMap<String, TypeAliasResolved>,
    entity_names: &HashSet<String>,
) -> Result<FieldDefinition, LowerError> {
    // Resolve the type expression. Named types may be enum aliases (lower to
    // string + values list), entity references (lower to a Relation field with
    // cardinality), or struct aliases (lower to inline object schema).
    let (field_type, values, items, properties, relation) =
        lower_type_expr(&f.type_expr, aliases, entity_names);
    Ok(FieldDefinition {
        name: Some(f.name.clone()),
        field_type,
        required: if f.required { Some(true) } else { None },
        primary_key: if f.is_key { Some(true) } else { None },
        unique: if f.is_unique { Some(true) } else { None },
        default: f.default.clone(),
        values,
        relation,
        items,
        properties,
        intrinsic: if f.is_intrinsic { Some(true) } else { None },
        description: f.description.clone(),
        synonyms: f.synonyms.clone(),
    })
}

type LoweredType = (
    FieldType,
    Option<Vec<String>>,
    Option<Box<FieldDefinition>>,
    Option<HashMap<String, FieldDefinition>>,
    Option<RelationDefinition>,
);

fn lower_type_expr(
    ty: &TypeExpr,
    aliases: &BTreeMap<String, TypeAliasResolved>,
    entity_names: &HashSet<String>,
) -> LoweredType {
    match ty {
        TypeExpr::Primitive(name) => (primitive_to_field_type(name), None, None, None, None),
        TypeExpr::Trait => (FieldType::Trait, None, None, None, None),
        TypeExpr::Slot => (FieldType::Slot, None, None, None, None),
        TypeExpr::Pattern => (FieldType::Pattern, None, None, None, None),
        TypeExpr::Named(name) => {
            if let Some(alias) = aliases.get(name) {
                match alias {
                    TypeAliasResolved::Enum(values) => {
                        (FieldType::String, Some(values.clone()), None, None, None)
                    }
                    TypeAliasResolved::Type(inner) => lower_type_expr(inner, aliases, entity_names),
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
                    }),
                )
            } else {
                // Unknown named type — preserve as a string field.
                (FieldType::String, None, None, None, None)
            }
        }
        TypeExpr::Array(inner) => {
            // Special-case `[EntityName]` (array of entity references): lower to
            // a single Relation field with cardinality:Many rather than an Array
            // wrapper around a foreign-key string. Companion to the bare-entity
            // case above. Type aliases that happen to look like entities still
            // take the regular array-of-struct path because they appear in
            // `aliases`, not in `entity_names`.
            if let TypeExpr::Named(name) = inner.as_ref() {
                if entity_names.contains(name) && !aliases.contains_key(name) {
                    return (
                        FieldType::Relation,
                        None,
                        None,
                        None,
                        Some(RelationDefinition {
                            entity: name.clone(),
                            cardinality: Some(Cardinality::Many),
                        }),
                    );
                }
            }
            let (inner_ft, inner_values, inner_items, inner_props, _inner_rel) =
                lower_type_expr(inner, aliases, entity_names);
            let item = FieldDefinition {
                name: None,
                field_type: inner_ft,
                required: None,
                primary_key: None,
                unique: None,
                default: None,
                values: inner_values,
                relation: None,
                items: inner_items,
                properties: inner_props,
                intrinsic: None,
                description: None,
                synonyms: None,
            };
            (FieldType::Array, None, Some(Box::new(item)), None, None)
        }
        TypeExpr::Object(fields) => {
            let mut props = HashMap::new();
            for (k, v, req) in fields {
                let (ft, values, items, properties, relation) =
                    lower_type_expr(v, aliases, entity_names);
                props.insert(
                    k.clone(),
                    FieldDefinition {
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
                    },
                );
            }
            (FieldType::Object, None, None, Some(props), None)
        }
        TypeExpr::Union(parts) => {
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
        TypeExpr::StringLiteral(_) => {
            // A bare literal type outside a union is degenerate — lower to
            // plain string. (Inside a union the `Union` branch above collects
            // all literals into `values`.)
            (FieldType::String, None, None, None, None)
        }
        TypeExpr::Optional(inner) => lower_type_expr(inner, aliases, entity_names),
        TypeExpr::RelationMany(_) | TypeExpr::RelationManyNonEmpty(_) => {
            (FieldType::Array, None, None, None, None)
        }
    }
}

fn primitive_to_field_type(name: &str) -> FieldType {
    match name {
        "string" | "money" | "any" => FieldType::String,
        "int" | "float" | "number" | "duration" => FieldType::Number,
        "bool" | "boolean" => FieldType::Boolean,
        "date" => FieldType::Date,
        "datetime" => FieldType::Datetime,
        "timestamp" => FieldType::Timestamp,
        "object" => FieldType::Object,
        _ => FieldType::String,
    }
}

/// Render a `TypeExpr` as the string form the resolve phase expects in
/// `ConfigField.field_type` ("string", "[string]", "number", etc.).
/// Mirrors the strings recognized by
/// `orbital-compiler::phases::resolve::type_string_to_oir`. Unknown
/// shapes fall back to "string" — same default the resolve phase uses
/// for unrecognized types, so the ts-config interface ends up typed
/// `field?: string` rather than failing the whole pipeline.
fn type_expr_to_config_string(ty: &TypeExpr) -> String {
    match ty {
        TypeExpr::Primitive(name) => name.clone(),
        TypeExpr::Named(name) => name.clone(),
        TypeExpr::Array(inner) => format!("[{}]", type_expr_to_config_string(inner)),
        TypeExpr::Optional(inner) => type_expr_to_config_string(inner),
        TypeExpr::StringLiteral(_) => "string".to_string(),
        TypeExpr::Union(_) => "string".to_string(),
        TypeExpr::Object(_) => "object".to_string(),
        TypeExpr::RelationMany(_) | TypeExpr::RelationManyNonEmpty(_) => "array".to_string(),
        TypeExpr::Trait => "trait".to_string(),
        TypeExpr::Slot => "slot".to_string(),
        TypeExpr::Pattern => "pattern".to_string(),
    }
}

/// Extract the literal-union variants when a config field is declared as
/// `"a" | "b" | "c"` (mirrors the entity-side `lower_type_expr`
/// `TypeExpr::Union` branch — same shape, same semantics). Returns
/// `None` when the type isn't a closed enum-style union, so downstream
/// consumers can tell "no constraint" from "constrained to {a, b, c}".
/// Unwraps a single layer of `Optional` so `"a" | "b"` and
/// `?"a" | "b"` both surface their members.
fn type_expr_to_config_values(ty: &TypeExpr) -> Option<Vec<String>> {
    match ty {
        TypeExpr::Optional(inner) => type_expr_to_config_values(inner),
        TypeExpr::Union(parts) => parts
            .iter()
            .map(|p| match p {
                TypeExpr::StringLiteral(s) => Some(s.clone()),
                TypeExpr::Named(n) => Some(n.clone()),
                TypeExpr::Primitive(n) => Some(n.clone()),
                _ => None,
            })
            .collect(),
        _ => None,
    }
}

/// Dispatch a `Trait` AST node to either an inline definition or a reference,
/// returning the `TraitReference` shape that the core schema expects.
fn lower_trait_decl(
    t: &Trait,
    aliases: &BTreeMap<String, TypeAliasResolved>,
    entities: &BTreeMap<String, Vec<(String, TypeExpr, bool)>>,
    entity_names: &HashSet<String>,
    event_aliases: &BTreeMap<String, EventTypeResolved>,
    intrinsic_fields: &HashSet<String>,
) -> Result<TraitReference, LowerError> {
    if let Some(ref_data) = &t.reference {
        Ok(lower_trait_reference(t, ref_data))
    } else {
        Ok(TraitReference::Inline(lower_trait(
            t,
            aliases,
            entities,
            entity_names,
            event_aliases,
            intrinsic_fields,
        )?))
    }
}

/// Build a `TraitReference::Reference` from a parsed `trait Name = Alias.X { ... }`.
fn lower_trait_reference(t: &Trait, ref_data: &TraitRefData) -> TraitReference {
    let events = if ref_data.events.is_empty() {
        None
    } else {
        Some(
            ref_data
                .events
                .iter()
                .cloned()
                .collect::<HashMap<String, String>>(),
        )
    };

    // Split listens into cross-orbital (keep on the trait's `listens` array)
    // and local incoming payload declarations (already attached above to
    // `stateMachine.events[].payload` in inline-trait lowering; dropped here
    // in trait-reference lowering since references don't declare inputs).
    let cross_orbital_listens: Vec<&Listen> =
        t.listens.iter().filter(|l| l.event.contains('.')).collect();
    let listens = if cross_orbital_listens.is_empty() {
        None
    } else {
        Some(
            cross_orbital_listens
                .into_iter()
                .map(lower_listen)
                .collect(),
        )
    };

    let emits_scope = ref_data.emits_scope.as_deref().and_then(|s| match s {
        "internal" => Some(EventScope::Internal),
        "external" => Some(EventScope::External),
        _ => None,
    });

    let config = if ref_data.config.is_empty() {
        None
    } else {
        Some(
            ref_data
                .config
                .iter()
                .cloned()
                .collect::<HashMap<String, SExpression>>(),
        )
    };

    let fields = if ref_data.fields.is_empty() {
        None
    } else {
        Some(
            ref_data
                .fields
                .iter()
                .cloned()
                .collect::<HashMap<String, String>>(),
        )
    };

    TraitReference::Reference {
        reference: Some(ref_data.reference.clone()),
        name: Some(t.name.clone()),
        linked_entity: t.linked_entity.clone(),
        config,
        events,
        fields,
        listens,
        emits_scope,
    }
}

fn lower_trait(
    t: &Trait,
    aliases: &BTreeMap<String, TypeAliasResolved>,
    entities: &BTreeMap<String, Vec<(String, TypeExpr, bool)>>,
    entity_names: &HashSet<String>,
    event_aliases: &BTreeMap<String, EventTypeResolved>,
    intrinsic_fields: &HashSet<String>,
) -> Result<TraitDefinition, LowerError> {
    // Build the state machine: states (with isInitial), events (collected
    // from transitions), transitions (one per Transition AST node, plus
    // auto-generated TIMEOUT entries for time-bound states).
    let mut states = Vec::new();
    let mut transitions = Vec::new();
    let mut event_keys: Vec<String> = Vec::new();
    let mut event_seen: HashSet<String> = HashSet::new();

    let initial_name = t
        .initial_state
        .clone()
        .or_else(|| t.states.first().map(|s| s.name.clone()));

    for state in &t.states {
        let is_initial = initial_name.as_deref() == Some(state.name.as_str());
        states.push(StateDefinition {
            name: state.name.clone(),
            is_initial: if is_initial { Some(true) } else { None },
            is_terminal: None,
            description: None,
        });

        for tr in &state.transitions {
            if event_seen.insert(tr.event.clone()) {
                event_keys.push(tr.event.clone());
            }
            transitions.push(TransitionDefinition {
                from: StringOrArray::Single(state.name.clone()),
                to: tr.target.clone(),
                event: tr.event.clone(),
                guard: tr.guard.clone(),
                effects: if tr.effects.is_empty() {
                    None
                } else {
                    Some(tr.effects.clone())
                },
            });
        }

        // Time-bound: auto-generate TIMEOUT transition that targets whatever
        // explicit TIMEOUT transition the user wrote (if any). If none, the
        // TIMEOUT transition stays self-targeted (matches the spec example).
        if let Some(_dur) = state.time_bound {
            // The duration is recorded in metadata; the runtime owns the timer.
            // We do NOT add a synthetic TIMEOUT transition here unless the user
            // omitted one. The user typically writes one explicitly.
            //
            // Per spec: "for <duration> on a state declaration auto-generates
            // a TIMEOUT transition that fires after the duration if no other
            // transition has been taken first." We implement: if the user did
            // not write a TIMEOUT transition, add one targeting the same state.
            let has_timeout = state.transitions.iter().any(|tr| tr.event == "TIMEOUT");
            if !has_timeout {
                if event_seen.insert("TIMEOUT".to_string()) {
                    event_keys.push("TIMEOUT".to_string());
                }
                transitions.push(TransitionDefinition {
                    from: StringOrArray::Single(state.name.clone()),
                    to: state.name.clone(),
                    event: "TIMEOUT".to_string(),
                    guard: None,
                    effects: None,
                });
            }
        }
    }

    // Sort states so the initial one is first; everyone else in declaration order.
    // (This matches the convention in the existing .orb fixtures.)
    states.sort_by_key(|s| if s.is_initial == Some(true) { 0 } else { 1 });

    let entity_hint = t.linked_entity.as_deref();

    // The `stateMachine.events[]` list is the union of every event key the
    // trait names: transition triggers (already collected above), emit keys
    // (outgoing events may also be listened-to internally), and local listens
    // (incoming payloads). Append emit keys so they appear in the events
    // list too — matching the shape of the committed `.orb` fixtures.
    for e in &t.emits {
        if event_seen.insert(e.event.clone()) {
            event_keys.push(e.event.clone());
        }
    }

    // Collect local listens by event key so the state-machine events array can
    // attach the declared payload shape. Cross-orbital listens (those with a
    // dotted `Source.EVENT` key) are skipped here — they're carried on the
    // `listens` array, not merged into the state-machine events list.
    let mut listen_payloads: HashMap<String, Vec<PayloadField>> = HashMap::new();
    for l in &t.listens {
        if l.event.contains('.') {
            continue; // cross-orbital, handled by listens array below
        }
        // Ensure the local listen's event key is on the events list too,
        // even if no transition fires on it yet.
        if event_seen.insert(l.event.clone()) {
            event_keys.push(l.event.clone());
        }
        let fields = if !l.payload.is_empty() {
            l.payload
                .iter()
                .map(|f| PayloadField {
                    name: f.name.clone(),
                    field_type: type_expr_to_string(&f.type_expr),
                    required: if f.required { Some(true) } else { None },
                    description: None,
                })
                .collect()
        } else {
            // Bare local listen entry: rely on the orbital-scope
            // `type X = Event { ... }` alias to supply the payload shape;
            // the loop below (`event_keys`) will resolve aliases when the
            // events list is finalized. Skip explicit insertion here.
            continue;
        };
        listen_payloads.insert(l.event.clone(), fields);
    }

    // Per-event annotations, keyed by event key, pulled from the emit/listen
    // that declared them. The emit (firing site) wins over a listen for the
    // same key; both feed the unified registry so every event carries its
    // `@description`/`@synonyms`/`@tier` the way fields/config knobs do.
    type EventAnn = (Option<String>, Option<String>, Option<String>);
    let mut event_annotations: HashMap<String, EventAnn> = HashMap::new();
    for l in &t.listens {
        if l.event.contains('.') {
            continue;
        }
        if l.description.is_some() || l.synonyms.is_some() || l.tier.is_some() {
            event_annotations
                .insert(l.event.clone(), (l.description.clone(), l.synonyms.clone(), l.tier.clone()));
        }
    }
    for e in &t.emits {
        if e.description.is_some() || e.synonyms.is_some() || e.tier.is_some() {
            event_annotations
                .insert(e.event.clone(), (e.description.clone(), e.synonyms.clone(), e.tier.clone()));
        }
    }

    let events: Option<Vec<EventDefinition>> = if event_keys.is_empty() {
        None
    } else {
        Some(
            event_keys
                .into_iter()
                .map(|k| -> Result<EventDefinition, LowerError> {
                    let name = humanize_event_key(&k, entity_hint);
                    let (description, synonyms, tier) =
                        event_annotations.get(&k).cloned().unwrap_or((None, None, None));
                    // Payload resolution precedence for stateMachine.events[]:
                    //   1. Explicit local listen payload (e.g. `listens { X { f: T } }`)
                    //   2. Orbital-scope `type X = Event <T>` declaration
                    // Without (2) the render-ui validator can't resolve
                    // `@payload.*` bindings on events that are only typed at
                    // orbital scope, even though the author declared the shape.
                    let payload = if let Some(fields) = listen_payloads.remove(&k) {
                        Some(fields)
                    } else if let Some(resolved) = event_aliases.get(&k) {
                        Some(type_expr_to_payload_fields(
                            &k,
                            &resolved.payload,
                            aliases,
                            entities,
                        )?)
                    } else {
                        None
                    };
                    Ok(EventDefinition {
                        key: k,
                        name: Some(name),
                        description,
                        synonyms,
                        tier,
                        payload_schema: payload,
                    })
                })
                .collect::<Result<Vec<_>, _>>()?,
        )
    };

    let category = match t.category.as_deref() {
        Some("interaction") => Some(TraitCategory::Interaction),
        Some("integration") => Some(TraitCategory::Integration),
        Some("lifecycle") => Some(TraitCategory::Lifecycle),
        Some("game-core") => Some(TraitCategory::GameCore),
        Some("game-entity") => Some(TraitCategory::GameEntity),
        Some("game-ui") => Some(TraitCategory::GameUi),
        Some(other) => {
            return Err(LowerError::InvalidCategory {
                value: other.to_string(),
            })
        }
        None => None,
    };

    // Single pass over the unified `emits { ... }` block. Each entry is
    // either a bare name (alias lookup against `event_aliases`) or has an
    // inline payload. `lower_emit` decides per entry.
    let emits = if t.emits.is_empty() {
        None
    } else {
        let mut lowered: Vec<EmitDefinition> = Vec::with_capacity(t.emits.len());
        for e in &t.emits {
            lowered.push(lower_emit(e, aliases, entities, event_aliases)?);
        }
        Some(lowered)
    };
    // Split listens into cross-orbital (keep on the trait's `listens` array)
    // and local incoming payload declarations (already attached above to
    // `stateMachine.events[].payload` in inline-trait lowering; dropped here
    // in trait-reference lowering since references don't declare inputs).
    let cross_orbital_listens: Vec<&Listen> =
        t.listens.iter().filter(|l| l.event.contains('.')).collect();
    let listens = if cross_orbital_listens.is_empty() {
        None
    } else {
        Some(
            cross_orbital_listens
                .into_iter()
                .map(lower_listen)
                .collect(),
        )
    };
    let ticks = if t.ticks.is_empty() {
        None
    } else {
        Some(t.ticks.iter().map(lower_tick).collect())
    };

    let scope = t.scope.as_ref().map(|s| match s {
        crate::ast::TraitScope::Instance => orbital_core::schema::TraitScope::Instance,
        crate::ast::TraitScope::Collection => orbital_core::schema::TraitScope::Collection,
    });

    // Lower the trait's declared `config { ... }` block into the schema's
    // `Option<HashMap<String, ConfigField>>` shape. The resolve phase
    // (`orbital-compiler::phases::resolve::resolve_trait`) reads this map
    // to populate `OirTrait.declared_config_schema`, which the TypeScript
    // backend uses to emit a strongly-typed `<TraitName>Config` interface
    // and a `DEFAULT_<TRAIT>_CONFIG` constant. Pre-fix this was always
    // None so atoms like `std-modal` declared `config { icon, title,
    // fields, mode }` but the generated interface only had the implicit
    // `linkedEntity?` and `entityId?` — which then failed typecheck on
    // any view that read `config?.icon`, `config?.title`, etc.
    let config = if t.config.is_empty() {
        None
    } else {
        let mut map: HashMap<String, SchemaConfigField> = HashMap::new();
        for cf in &t.config {
            // Run the structural lowerer alongside the string-tag lowerer.
            // `lower_type_expr` resolves named aliases (e.g. `[MetricSpec]`)
            // into `FieldType::Array` + `items: Box<FieldDefinition>` with the
            // struct's fields fully populated in `properties` — same path
            // entity fields take. The string `field_type` stays as the
            // human/readable tag so the existing resolve/codegen consumers
            // keep working; `items` / `properties` are additive.
            let (_, _, items, properties, _) =
                lower_type_expr(&cf.type_expr, aliases, entity_names);
            map.insert(
                cf.name.clone(),
                SchemaConfigField {
                    field_type: type_expr_to_config_string(&cf.type_expr),
                    default: cf.default.clone(),
                    label: cf.label.clone(),
                    description: cf.description.clone(),
                    synonyms: cf.synonyms.clone(),
                    tier: cf.tier.clone(),
                    values: type_expr_to_config_values(&cf.type_expr),
                    items,
                    properties,
                },
            );
        }
        Some(map)
    };

    // Explicit-binding rule (L1): every @entity.<field> read must be backed
    // by an explicit (set @entity.<field> ...) somewhere in the trait. The L2
    // validator runs the same check post-inline; this gives faster feedback
    // at parse time. Skipped when the trait has no linkedEntity.
    if t.linked_entity.is_some() {
        check_entity_fields_have_setters(t, intrinsic_fields)?;
        check_set_value_types(t, entities)?;
    }

    let mut capabilities: Vec<String> = t.capabilities.clone();
    capabilities.sort();
    capabilities.dedup();

    // Rebindable binding: emit the opt-in marker + the inferred field
    // contract (reads via `@entity.X` → requires; writes via `(set
    // @entity.X)` → provides). Only when the author wrote `-> @rebindable`,
    // so non-rebindable traits stay byte-identical in the .orb.
    let (entity_rebindable, entity_contract) = if t.entity_rebindable {
        // DATA-field satisfaction is enforced at author time by the
        // explicit-binding rule (`check_entity_fields_have_setters`, now
        // intrinsic-aware) above, and post-inline by `ORB_T_REBIND_MISSING_FIELDS`
        // (which consults this contract's intrinsic-pruned `requires`).
        (Some(true), Some(infer_entity_contract(t, intrinsic_fields)))
    } else {
        (None, None)
    };

    Ok(TraitDefinition {
        name: t.name.clone(),
        entity_rebindable,
        entity_contract,
        category,
        capabilities,
        description: None,
        linked_entity: t.linked_entity.clone(),
        emits,
        listens,
        state_machine: StateMachine {
            states,
            events,
            transitions,
        },
        ticks,
        config,
        data_entities: None,
        scope,
        source_behavior: None,
        source_entity_definition: None,
    })
}

/// Infer the `@rebindable` binding's field contract from the trait's own
/// `@entity.<field>` usage. Reuses `collect_entity_field_refs` (the single
/// SExpr walker — no second walker) across all transitions/guards/ticks:
/// reads become `requires`, `(set @entity.X ...)` writes become `provides`.
/// `id`/`createdAt`/`updatedAt` are already excluded from reads by the
/// collector (auto-managed fields every entity has). Both lists sorted +
/// deduped for a deterministic `.orb`.
fn infer_entity_contract(t: &Trait, intrinsic_fields: &HashSet<String>) -> EntityFieldContract {
    let mut referenced_fields: HashMap<String, String> = HashMap::new();
    let mut set_fields: HashSet<String> = HashSet::new();
    for state in &t.states {
        for tr in &state.transitions {
            let event = tr.event.as_str();
            if let Some(guard) = &tr.guard {
                collect_entity_field_refs(guard, &mut referenced_fields, &mut set_fields, event);
            }
            for effect in &tr.effects {
                collect_entity_field_refs(effect, &mut referenced_fields, &mut set_fields, event);
            }
        }
    }
    for tick in &t.ticks {
        if let Some(guard) = &tick.guard {
            collect_entity_field_refs(guard, &mut referenced_fields, &mut set_fields, "TICK");
        }
        for effect in &tick.effects {
            collect_entity_field_refs(effect, &mut referenced_fields, &mut set_fields, "TICK");
        }
    }
    // The walker records a `(set @entity.X ...)` target in BOTH set_fields
    // (it's a write) and referenced_fields (it recurses into arr[1], the
    // `@entity.X` literal). Subtract the writes so `requires` is PURE reads
    // and `provides` is writes — a clean split for the Phase 3 relax option
    // (strict = requires ∪ provides must exist; relaxed = only requires).
    // Auto-managed fields every entity carries — requiring a rebind target to
    // have them is trivially true, so keep them out of the contract. (Reads of
    // these are already excluded by the collector; the SET branch is not.)
    // INTRINSIC fields (`@intrinsic` on the entity decl) are machinery the trait
    // computes itself; they travel WITH the trait onto a rebind target via the
    // inline field-merge, so a rebind target is NOT required to pre-declare them.
    // Exclude them from `requires` so the post-inline validator only demands
    // genuine DATA fields the consumer must supply.
    let is_auto = |f: &String| f == "id" || f == "createdAt" || f == "updatedAt";
    let mut requires: Vec<String> = referenced_fields
        .into_keys()
        .filter(|f| !set_fields.contains(f) && !is_auto(f) && !intrinsic_fields.contains(f))
        .collect();
    requires.sort();
    requires.dedup();
    let mut provides: Vec<String> = set_fields.into_iter().filter(|f| !is_auto(f)).collect();
    provides.sort();
    provides.dedup();
    EntityFieldContract { requires, provides }
}

/// Walk an SExpression collecting every `@entity.<field>` reference into
/// `referenced_fields` (with the originating event name) and every
/// `["set", "@entity.<field>", ...]` setter call into `set_fields`.
fn collect_entity_field_refs(
    expr: &SExpression,
    referenced_fields: &mut HashMap<String, String>,
    set_fields: &mut HashSet<String>,
    transition_event: &str,
) {
    match expr {
        SExpression::String(s) if s.starts_with("@entity.") => {
            let path = &s[8..];
            let field = path.split('.').next().unwrap_or("");
            let field = field.split('[').next().unwrap_or(field);
            if !field.is_empty()
                && field != "id"
                && field != "createdAt"
                && field != "updatedAt"
            {
                referenced_fields
                    .entry(field.to_string())
                    .or_insert_with(|| transition_event.to_string());
            }
        }
        SExpression::Array(arr) => {
            if arr.len() >= 2 {
                if let (SExpression::String(op), SExpression::String(target)) = (&arr[0], &arr[1]) {
                    if op == "set" && target.starts_with("@entity.") {
                        let setter_path = &target[8..];
                        let setter_field = setter_path.split('.').next().unwrap_or("");
                        let setter_field = setter_field.split('[').next().unwrap_or(setter_field);
                        if !setter_field.is_empty() {
                            set_fields.insert(setter_field.to_string());
                        }
                    }
                }
            }
            for item in arr {
                collect_entity_field_refs(item, referenced_fields, set_fields, transition_event);
            }
        }
        SExpression::Object(obj) => {
            for value in obj.values() {
                collect_entity_field_refs(value, referenced_fields, set_fields, transition_event);
            }
        }
        _ => {}
    }
}

/// Per the explicit-binding rule, every `@entity.X` referenced in a trait's
/// transitions/guards/ticks must be backed by some explicit
/// `(set @entity.X <value>)`. Returns the first violation as a
/// `LowerError::EntityFieldNeverSet` so the lolo CLI surfaces it at parse
/// time. Skipped by the caller when the trait has no `linkedEntity`.
/// `@intrinsic` fields are EXEMPT — they are machinery the trait/runtime owns
/// and may be read without a local setter.
fn check_entity_fields_have_setters(
    t: &Trait,
    intrinsic_fields: &HashSet<String>,
) -> Result<(), LowerError> {
    let mut referenced_fields: HashMap<String, String> = HashMap::new();
    let mut set_fields: HashSet<String> = HashSet::new();

    for state in &t.states {
        for tr in &state.transitions {
            let event = tr.event.as_str();
            if let Some(guard) = &tr.guard {
                collect_entity_field_refs(guard, &mut referenced_fields, &mut set_fields, event);
            }
            for effect in &tr.effects {
                collect_entity_field_refs(effect, &mut referenced_fields, &mut set_fields, event);
            }
        }
    }
    for tick in &t.ticks {
        if let Some(guard) = &tick.guard {
            collect_entity_field_refs(guard, &mut referenced_fields, &mut set_fields, "TICK");
        }
        for effect in &tick.effects {
            collect_entity_field_refs(effect, &mut referenced_fields, &mut set_fields, "TICK");
        }
    }

    // Sort referenced fields for deterministic error ordering (BTreeMap-like).
    let mut entries: Vec<(&String, &String)> = referenced_fields.iter().collect();
    entries.sort_by(|a, b| a.0.cmp(b.0));
    for (field, first_event) in entries {
        if !set_fields.contains(field) && !intrinsic_fields.contains(field) {
            return Err(LowerError::EntityFieldNeverSet {
                trait_name: t.name.clone(),
                field: field.clone(),
                first_event: first_event.clone(),
            });
        }
    }
    Ok(())
}

/// L1.4 check: for every `(set @entity.X <value>)` in this trait,
/// verify the value's resolved type is assignable to the linked
/// entity's declared field type for X.
///
/// Resolution sources covered at parse time (mirrors the L2 F3 binding
/// resolver but with parser AST access only):
/// - **Literal** → primitive type from the SExpression node.
/// - **`@payload.<F>`** → type from the trait's `listens { EVENT { F : T } }`
///   block where EVENT matches the enclosing transition's event.
/// - **`@config.<F>`** → type from the trait's `config { F : T }` block.
/// - **`@entity.<F>`** → type from the linked entity's declared fields.
///
/// SExpression operator calls (`["op", ...]`) defer to L2 — that branch
/// needs canonical-operators metadata + F2's `returnSemantics` lattice
/// which the lolo crate doesn't currently embed. The L2 ORB_BINDING_*
/// suite catches the operator case post-resolve.
fn check_set_value_types(
    t: &Trait,
    entities: &BTreeMap<String, Vec<(String, TypeExpr, bool)>>,
) -> Result<(), LowerError> {
    let Some(linked) = &t.linked_entity else {
        return Ok(());
    };
    let Some(entity_fields) = entities.get(linked) else {
        return Ok(());
    };

    // Build per-listened-event payload schema lookup. Keyed by the LOCAL
    // event name (the trigger for cross-orbital listens, the bare event
    // for local listens). When a transition fires on event E, `@payload.X`
    // resolves against the matching listen entry's payload.
    let mut listen_payloads: HashMap<&str, &[EventField]> = HashMap::new();
    for listen in &t.listens {
        // Cross-orbital listens use `Source.EVENT -> TRIGGER` form; the
        // bare event name on the bus side is the TRIGGER (or the bare
        // event for local listens). The payload AST field carries the
        // declared local-shape schema — only populated for local
        // listens, not cross-orbital ones.
        let key = listen.triggers.as_deref().unwrap_or(&listen.event);
        if !listen.payload.is_empty() {
            listen_payloads.insert(key, listen.payload.as_slice());
        }
    }

    // Per-config-field type lookup.
    let config_fields: HashMap<&str, &TypeExpr> = t
        .config
        .iter()
        .map(|cf| (cf.name.as_str(), &cf.type_expr))
        .collect();

    // Per-entity-field type lookup on the linked entity.
    let entity_field_types: HashMap<&str, &TypeExpr> = entity_fields
        .iter()
        .map(|(name, ty, _required)| (name.as_str(), ty))
        .collect();

    for state in &t.states {
        for tr in &state.transitions {
            for effect in &tr.effects {
                check_set_in_expr(
                    effect,
                    &tr.event,
                    t,
                    &entity_field_types,
                    &listen_payloads,
                    &config_fields,
                )?;
            }
        }
    }
    for tick in &t.ticks {
        for effect in &tick.effects {
            check_set_in_expr(
                effect,
                "TICK",
                t,
                &entity_field_types,
                &listen_payloads,
                &config_fields,
            )?;
        }
    }
    Ok(())
}

/// Recursively search for `["set", "@entity.X", <value>]` SExpressions
/// and validate each one. Descends into nested arrays and objects so
/// `(set ...)` calls inside `(if cond ...)` / `(do ...)` / nested
/// render-ui patterns are caught.
fn check_set_in_expr(
    expr: &SExpression,
    event: &str,
    t: &Trait,
    entity_fields: &HashMap<&str, &TypeExpr>,
    listen_payloads: &HashMap<&str, &[EventField]>,
    config_fields: &HashMap<&str, &TypeExpr>,
) -> Result<(), LowerError> {
    if let SExpression::Array(arr) = expr {
        if arr.len() >= 3 {
            if let (SExpression::String(op), SExpression::String(target)) =
                (&arr[0], &arr[1])
            {
                if op == "set" && target.starts_with("@entity.") {
                    let path = &target[8..];
                    let field = path.split('.').next().unwrap_or("").split('[').next().unwrap_or("");
                    if !field.is_empty() {
                        if let Some(declared) = entity_fields.get(field) {
                            check_one_set_value(
                                t,
                                field,
                                declared,
                                &arr[2],
                                event,
                                listen_payloads,
                                config_fields,
                                entity_fields,
                            )?;
                        }
                    }
                }
            }
        }
        for item in arr {
            check_set_in_expr(item, event, t, entity_fields, listen_payloads, config_fields)?;
        }
    } else if let SExpression::Object(obj) = expr {
        for value in obj.values() {
            check_set_in_expr(value, event, t, entity_fields, listen_payloads, config_fields)?;
        }
    }
    Ok(())
}

/// Resolve the value's type from its SExpression form and compare against
/// the declared entity field type. Returns the first mismatch as a
/// `LowerError::SetValueTypeMismatch`. Bindings whose source isn't
/// declared in the trait (cross-orbital, unknown event, etc.) skip
/// silently — those defer to L2's F3 binding resolver.
fn check_one_set_value(
    t: &Trait,
    field: &str,
    declared: &TypeExpr,
    value: &SExpression,
    event: &str,
    listen_payloads: &HashMap<&str, &[EventField]>,
    config_fields: &HashMap<&str, &TypeExpr>,
    entity_fields: &HashMap<&str, &TypeExpr>,
) -> Result<(), LowerError> {
    let declared_str = type_expr_to_compat_str(declared);

    // Resolve value's type. Returns (actual_type_str, value_source).
    // None when the value is a binding/operator we don't handle at L1
    // (defer to L2).
    let resolved = match value {
        SExpression::Number(_) => Some(("number".to_string(), "literal".to_string())),
        SExpression::Bool(_) => Some(("boolean".to_string(), "literal".to_string())),
        SExpression::Object(_) => Some(("object".to_string(), "literal".to_string())),
        SExpression::Array(_) => {
            // Could be an array literal OR an SExpression operator call.
            // Defer to L2 — distinguishing them at parse time without
            // canonical-operators metadata is brittle.
            None
        }
        SExpression::Null => None,
        SExpression::String(s) => {
            if let Some(rest) = s.strip_prefix("@payload.") {
                resolve_top_segment(rest).and_then(|f| {
                    let payload = listen_payloads.get(event)?;
                    let declared_payload_field = payload.iter().find(|pf| pf.name == f)?;
                    Some((
                        type_expr_to_compat_str(&declared_payload_field.type_expr),
                        format!("@payload.{}", f),
                    ))
                })
            } else if let Some(rest) = s.strip_prefix("@config.") {
                resolve_top_segment(rest).and_then(|f| {
                    config_fields
                        .get(f.as_str())
                        .map(|ty| (type_expr_to_compat_str(ty), format!("@config.{}", f)))
                })
            } else if let Some(rest) = s.strip_prefix("@entity.") {
                resolve_top_segment(rest).and_then(|f| {
                    entity_fields
                        .get(f.as_str())
                        .map(|ty| (type_expr_to_compat_str(ty), format!("@entity.{}", f)))
                })
            } else if s.starts_with('@') {
                // Other sigils (@row.X, @item.X inside fn-form lambdas,
                // etc.) defer to L2.
                None
            } else {
                Some(("string".to_string(), "literal".to_string()))
            }
        }
    };

    let Some((actual_str, value_source)) = resolved else {
        return Ok(());
    };

    if !primitive_type_compatible(&declared_str, &actual_str) {
        return Err(LowerError::SetValueTypeMismatch {
            orbital: t.name.clone(), // diagnostic context: trait carries its parent orbital name in error path
            trait_name: t.name.clone(),
            field: field.to_string(),
            expected_type: declared_str,
            actual_type: actual_str,
            value_source,
        });
    }
    Ok(())
}

/// Extract the top segment of a binding path, returning `None` when the
/// path goes deeper than one segment. Nested paths like
/// `@payload.row.title` need recursion into the row's entity definition
/// to resolve the sub-field's type — that's L2's job (F3 binding
/// resolver). Returning `None` here makes L1 silently defer those cases
/// rather than emit false positives.
///
/// Also returns `None` for empty / malformed inputs, and strips array
/// indexers (`field[0]` → `field`).
fn resolve_top_segment(path: &str) -> Option<String> {
    // Reject nested-field paths — e.g. "row.title" must defer to L2.
    let trimmed = path.trim_end_matches('.');
    if trimmed.contains('.') {
        return None;
    }
    let field = trimmed.split('[').next().unwrap_or("");
    if field.is_empty() {
        None
    } else {
        Some(field.to_string())
    }
}

/// Reduce a `TypeExpr` to a primitive string the compatibility check
/// understands. Mirrors the L2 emit-payload primitive vocabulary so the
/// two layers' decisions agree.
fn type_expr_to_compat_str(t: &TypeExpr) -> String {
    match t {
        TypeExpr::Primitive(p) => match p.as_str() {
            "int" | "float" | "double" | "long" => "number".to_string(),
            "bool" => "boolean".to_string(),
            other => other.to_string(),
        },
        TypeExpr::Named(n) => n.clone(), // entity / struct reference
        TypeExpr::StringLiteral(_) => "string".to_string(),
        TypeExpr::Array(_) => "array".to_string(),
        TypeExpr::Object(_) => "object".to_string(),
        TypeExpr::Union(members) => {
            // String-literal unions reduce to "string" (matches lowering's
            // enum-as-string emission). Mixed unions report as the first
            // member's type — best-effort; cross-type unions are rare in
            // practice.
            if members
                .iter()
                .all(|m| matches!(m, TypeExpr::StringLiteral(_)))
            {
                "string".to_string()
            } else {
                members
                    .first()
                    .map(type_expr_to_compat_str)
                    .unwrap_or_else(|| "object".to_string())
            }
        }
        TypeExpr::Optional(inner) => type_expr_to_compat_str(inner),
        TypeExpr::RelationMany(_) | TypeExpr::RelationManyNonEmpty(_) => "array".to_string(),
        TypeExpr::Trait => "trait".to_string(),
        TypeExpr::Slot => "slot".to_string(),
        TypeExpr::Pattern => "pattern".to_string(),
    }
}

/// Primitive-type compatibility check between declared and actual.
/// Mirrors the L2 emit-payload validator's `primitive_types_compatible`.
/// `any` matches anything; `date`/`datetime`/`timestamp` accept string
/// or number; `entity`/`relation` accept object or string; named entity
/// types (e.g. `Article`) accept object.
fn primitive_type_compatible(declared: &str, actual: &str) -> bool {
    let d = declared.trim_end_matches('!');
    let a = actual.trim_end_matches('!');
    if d == a {
        return true;
    }
    match d {
        "any" => true,
        "date" | "datetime" | "timestamp" => a == "string" || a == "number",
        "entity" | "relation" => a == "object" || a == "string",
        // Named entity / struct types accept object literals or string
        // (id reference).
        s if s.chars().next().map(|c| c.is_uppercase()).unwrap_or(false) => {
            a == "object" || a == "string"
        }
        _ => false,
    }
}

fn lower_emit(
    e: &Emit,
    aliases: &BTreeMap<String, TypeAliasResolved>,
    entities_by_name: &BTreeMap<String, Vec<(String, TypeExpr, bool)>>,
    event_aliases: &BTreeMap<String, EventTypeResolved>,
) -> Result<EmitDefinition, LowerError> {
    // Bare-name path: when the entry has no inline payload, look up
    // `type EVENT_NAME = Event { ... }` in the orbital-scope alias registry.
    // If found, the alias supplies payload (and scope, if the entry didn't
    // override it). If not found, the emit is payloadless. Inline payload
    // (next branch) always wins over alias lookup.
    if e.payload.is_empty() {
        if let Some(resolved) = event_aliases.get(&e.event) {
            let payload = Some(type_expr_to_payload_fields(
                &e.event,
                &resolved.payload,
                aliases,
                entities_by_name,
            )?);
            // Call-site `-> external|internal` overrides the alias's scope;
            // missing call-site scope falls back to the alias's scope, then
            // to internal.
            let scope = match e.scope.as_deref() {
                Some("internal") => Some(EventScope::Internal),
                Some("external") => Some(EventScope::External),
                _ => match resolved.scope {
                    Some(EventScopeAst::External) => Some(EventScope::External),
                    Some(EventScopeAst::Internal) | None => Some(EventScope::Internal),
                },
            };
            return Ok(EmitDefinition {
                event: e.event.clone(),
                // Inline `@description` wins; fall back to the alias's description.
                description: e.description.clone().or_else(|| resolved.description.clone()),
                synonyms: e.synonyms.clone(),
                tier: e.tier.clone(),
                scope,
                payload_schema: payload,
            });
        }
        // No alias and no inline payload → payloadless emit.
        return Ok(EmitDefinition {
            event: e.event.clone(),
            description: e.description.clone(),
            synonyms: e.synonyms.clone(),
            tier: e.tier.clone(),
            scope: e.scope.as_deref().and_then(|s| match s {
                "internal" => Some(EventScope::Internal),
                "external" => Some(EventScope::External),
                _ => None,
            }),
            payload_schema: None,
        });
    }
    // Inline payload path: each field whose type_expr is `TypeExpr::Named`
    // is resolved against the type registry and flattened into prefixed
    // `name.subname` entries; primitive fields stay flat.
    let mut fields: Vec<PayloadField> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for f in &e.payload {
        if let TypeExpr::Named(type_name) = &f.type_expr {
            let nested = resolve_payload_type(type_name, aliases, entities_by_name)?;
            for nf in nested {
                let prefixed = format!("{}.{}", f.name, nf.name);
                if !seen.insert(prefixed.clone()) {
                    return Err(LowerError::PayloadFieldCollision {
                        event: e.event.clone(),
                        field: prefixed,
                    });
                }
                fields.push(PayloadField {
                    name: prefixed,
                    field_type: nf.field_type,
                    required: nf.required,
                    description: None,
                });
            }
        } else {
            if !seen.insert(f.name.clone()) {
                return Err(LowerError::PayloadFieldCollision {
                    event: e.event.clone(),
                    field: f.name.clone(),
                });
            }
            fields.push(PayloadField {
                name: f.name.clone(),
                field_type: type_expr_to_string(&f.type_expr),
                required: if f.required { Some(true) } else { None },
                description: None,
            });
        }
    }
    Ok(EmitDefinition {
        event: e.event.clone(),
        description: e.description.clone(),
        synonyms: e.synonyms.clone(),
        tier: e.tier.clone(),
        scope: e.scope.as_deref().and_then(|s| match s {
            "internal" => Some(EventScope::Internal),
            "external" => Some(EventScope::External),
            _ => None,
        }),
        payload_schema: Some(fields),
    })
}

/// Flatten a `TypeExpr` (from a `type X = Event <T>` alias or similar) into
/// a `Vec<PayloadField>`. Shared by `lower_emit`'s bare-name alias-lookup
/// path and the `stateMachine.events[]` builder (so events that only appear
/// in orbital-scope type declarations still carry their payload shape into
/// the IR).
fn type_expr_to_payload_fields(
    event_name: &str,
    payload: &TypeExpr,
    aliases: &BTreeMap<String, TypeAliasResolved>,
    entities_by_name: &BTreeMap<String, Vec<(String, TypeExpr, bool)>>,
) -> Result<Vec<PayloadField>, LowerError> {
    match payload {
        TypeExpr::Named(type_name) => {
            resolve_payload_type(type_name, aliases, entities_by_name)
        }
        TypeExpr::Object(fields) => {
            let mut flat: Vec<PayloadField> = Vec::with_capacity(fields.len());
            let mut seen: HashSet<String> = HashSet::new();
            for (fname, ftype, freq) in fields {
                if !seen.insert(fname.clone()) {
                    return Err(LowerError::PayloadFieldCollision {
                        event: event_name.to_string(),
                        field: fname.clone(),
                    });
                }
                flat.push(PayloadField {
                    name: fname.clone(),
                    field_type: type_expr_to_string(ftype),
                    required: if *freq { Some(true) } else { None },
                    description: None,
                });
            }
            Ok(flat)
        }
        other => {
            // Primitive (`Event string`), array, union, etc. Surface as a
            // single `value` field carrying the primitive type string. Keeps
            // the `.orb` shape predictable; avoids empty payloads for
            // primitive-valued events.
            Ok(vec![PayloadField {
                name: "value".to_string(),
                field_type: type_expr_to_string(other),
                required: None,
                description: None,
            }])
        }
    }
}

/// Resolve a named type referenced from an emit payload into a flat
/// `Vec<PayloadField>`. Looks up first the orbital-local alias registry
/// (for `type Foo = Entity { ... }` declarations) and then the same-orbital
/// entity declarations (so `emits { X :: MyEntity }` works when `MyEntity`
/// is the orbital's own entity). Unknown names surface as `LowerError::Type`;
/// the type resolver (`check_program`) catches most of these before lowering.
fn resolve_payload_type(
    name: &str,
    _aliases: &BTreeMap<String, TypeAliasResolved>,
    entities_by_name: &BTreeMap<String, Vec<(String, TypeExpr, bool)>>,
) -> Result<Vec<PayloadField>, LowerError> {
    if let Some(fields) = entities_by_name.get(name) {
        return Ok(fields
            .iter()
            .map(|(n, t, req)| PayloadField {
                name: n.clone(),
                field_type: type_expr_to_string(t),
                required: if *req { Some(true) } else { None },
                description: None,
            })
            .collect());
    }
    Err(LowerError::UnknownPayloadType {
        name: name.to_string(),
    })
}

fn lower_listen(l: &Listen) -> ListenDefinition {
    let payload_mapping = if l.payload_mapping.is_empty() {
        None
    } else {
        Some(
            l.payload_mapping
                .iter()
                .cloned()
                .collect::<HashMap<String, SExpression>>(),
        )
    };

    // Parse the parser-encoded `event` string into a bare event name +
    // structured source descriptor. See the grammar in `parser.rs` around
    // `parse_listen_handler` for the concatenation rules.
    //
    //   "*.EVENT"            → source = Any,      bare = EVENT
    //   "Trait.EVENT"        → source = Trait,    bare = EVENT
    //   "Orbital.Trait.EVENT"→ source = Orbital,  bare = EVENT
    //   "EVENT" (no dot)     → source = None,     bare = EVENT  (local payload-only)
    let (bare_event, source) = split_listen_source(&l.event);

    ListenDefinition {
        event: bare_event,
        triggers: l.triggers.clone(),
        scope: None,
        description: l.description.clone(),
        synonyms: l.synonyms.clone(),
        tier: l.tier.clone(),
        payload_mapping,
        guard: l.guard.clone(),
        source,
    }
}

/// Split the parser's concatenated event string into (bare_event, source).
/// Returns `(event, None)` when the event has no source prefix (local shape).
fn split_listen_source(
    raw: &str,
) -> (
    String,
    Option<orbital_core::schema::ListenSource>,
) {
    use orbital_core::schema::ListenSource;
    let parts: Vec<&str> = raw.split('.').collect();
    match parts.len() {
        0 | 1 => (raw.to_string(), None),
        2 => {
            let (src, ev) = (parts[0], parts[1]);
            if src == "*" {
                (ev.to_string(), Some(ListenSource::Any))
            } else {
                (
                    ev.to_string(),
                    Some(ListenSource::Trait {
                        trait_name: src.to_string(),
                    }),
                )
            }
        }
        n => {
            let ev = parts[n - 1];
            let trait_name = parts[n - 2];
            let orbital = parts[..n - 2].join(".");
            (
                ev.to_string(),
                Some(ListenSource::Orbital {
                    orbital,
                    trait_name: trait_name.to_string(),
                }),
            )
        }
    }
}

/// L1 cross-orbital integrity check.
///
/// Walks the lowered orbitals (post-`lower_orbital`, pre-validate) and
/// rejects any `listens` entry whose declared source trait/event isn't
/// emitted by the named source trait. The IR's `ListenSource` enum carries
/// the typed source descriptor (trait-only or orbital+trait), so this pass
/// uses enum variants and `String` equality only — no string parsing, no
/// pattern matching on event names.
///
/// Companion to the Phase-2 validator pass (`orbital-compiler` /
/// `validation/cross_orbital.rs`), which reruns this check post-resolve
/// so `use`-imported behaviors are covered.
fn check_listens_integrity(orbitals: &[CoreOrbital]) -> Result<(), LowerError> {
    // Index every (orbital, trait) → declared emit event names. Bare-event
    // listens (no source) are local-only and are not checked here — they
    // resolve against the trait's own state-machine transitions, which the
    // type-resolver already enforces.
    //
    // Reference-trait sources are also tracked here (by name only) so the
    // integrity check can defer those to the compiler's post-inline
    // validator (`cross_orbital::validate_listens_integrity`). At the
    // lolo lower stage, reference traits like
    // `trait CartItemAddItem = Modal.traits.ModalRecordModal -> ...`
    // haven't been resolved yet — their emits live in the imported atom
    // and aren't visible in this AST. Skipping the check for references
    // avoids false-positive "source not declared" failures and lets the
    // post-inline pass do the real check once references are materialized.
    let mut trait_emits: HashMap<(String, String), HashSet<String>> = HashMap::new();
    let mut reference_traits: HashSet<(String, String)> = HashSet::new();
    for orbital in orbitals {
        for trait_ref in &orbital.traits {
            match trait_ref {
                TraitReference::Inline(td) => {
                    let entry = trait_emits
                        .entry((orbital.name.clone(), td.name.clone()))
                        .or_default();
                    if let Some(emits) = &td.emits {
                        for emit in emits {
                            entry.insert(emit.event.clone());
                        }
                    }
                }
                TraitReference::Reference { name, reference, .. } => {
                    let trait_name = name
                        .clone()
                        .or_else(|| {
                            reference
                                .as_deref()
                                .and_then(|r| r.rsplit('.').next().map(str::to_owned))
                        });
                    if let Some(n) = trait_name {
                        reference_traits.insert((orbital.name.clone(), n));
                    }
                }
            }
        }
    }

    for orbital in orbitals {
        for trait_ref in &orbital.traits {
            let TraitReference::Inline(td) = trait_ref else {
                continue;
            };
            let Some(listens) = &td.listens else {
                continue;
            };
            for listen in listens {
                let (source_orbital, source_trait) = match &listen.source {
                    Some(ListenSource::Trait { trait_name }) => {
                        (orbital.name.clone(), trait_name.clone())
                    }
                    Some(ListenSource::Orbital {
                        orbital: o,
                        trait_name,
                    }) => (o.clone(), trait_name.clone()),
                    Some(ListenSource::Any) | None => continue,
                };
                // Defer to the compiler's post-inline validator when the
                // source trait is a reference — its emits aren't
                // visible at this stage.
                if reference_traits.contains(&(source_orbital.clone(), source_trait.clone())) {
                    continue;
                }
                let resolved = trait_emits
                    .get(&(source_orbital.clone(), source_trait.clone()))
                    .map(|events| events.contains(&listen.event))
                    .unwrap_or(false);
                if !resolved {
                    return Err(LowerError::ListenSourceUnresolved {
                        origin_orbital: orbital.name.clone(),
                        origin_trait: td.name.clone(),
                        source_orbital,
                        source_trait,
                        event: listen.event.clone(),
                    });
                }
            }
        }
    }
    Ok(())
}

/// Recursively collect literal button-action strings from a single effect.
/// Only descends into `render-ui` S-expressions; other effect forms are
/// ignored because they don't render clickable UI.
fn collect_render_ui_button_actions(effect: &SExpression, actions: &mut HashSet<String>) {
    let pattern = match effect {
        SExpression::Array(arr) if arr.len() >= 3 => {
            if let SExpression::String(op) = &arr[0] {
                if op == "render-ui" {
                    // The pattern is the 3rd argument (index 2).
                    &arr[2]
                } else {
                    return;
                }
            } else {
                return;
            }
        }
        _ => return,
    };
    collect_button_actions_recursive(pattern, actions);
}

/// Recursively walk an S-expression tree and collect literal string values
/// for keys that the patterns registry marks as event-name carriers.
///
/// "Event-name carrier" is a source-tagged contract: a prop counts when
/// its registry entry has `kind` in `{event, event-ref, event-listen,
/// callback}` (top-level prop holds the event-name string), or when
/// it's the `eventField` of a `kind: "event-list"` entry (inner
/// descriptor key). See `patterns_meta::event_prop_names` for the
/// derivation. No hardcoded prop list, no name heuristics — every
/// recognised name comes from a `kind` mark that pattern-sync wrote
/// because a component author typed the prop as `EventEmit<P>` /
/// `EventKey` / a function (callback) / an event-list descriptor.
fn collect_button_actions_recursive(expr: &SExpression, actions: &mut HashSet<String>) {
    match expr {
        SExpression::Array(arr) => {
            for item in arr {
                collect_button_actions_recursive(item, actions);
            }
        }
        SExpression::Object(obj) => {
            let event_keys = event_prop_names();
            for (key, value) in obj {
                if event_keys.contains(key) {
                    match value {
                        SExpression::String(s) => {
                            actions.insert(s.clone());
                        }
                        // Some legacy event props accept an array of
                        // strings (every element is an event name). The
                        // registry doesn't distinguish this shape from a
                        // single string; harvest all string elements.
                        SExpression::Array(arr) => {
                            for item in arr {
                                if let SExpression::String(s) = item {
                                    actions.insert(s.clone());
                                }
                            }
                        }
                        _ => {}
                    }
                }
                collect_button_actions_recursive(value, actions);
            }
        }
        _ => {}
    }
}

/// L1 check: for every button action rendered by trait A, if some sibling
/// inline trait B in the same orbital has a transition on that event but
/// B does NOT declare `listens { A event -> ... }`, fail fast.
///
/// Reference traits are skipped as targets (their transitions aren't
/// visible at the lolo lower stage — they're materialised later by the
/// inline phase). This avoids false positives on reference-style molecules
/// like `std-cart` and `std-list`; the compiler's post-inline validator
/// (`cross_orbital.rs`) handles those.
fn check_button_transitions(orbitals: &[CoreOrbital]) -> Result<(), LowerError> {
    // Index (orbital, trait) -> transition events.
    let mut trait_transitions: HashMap<(String, String), HashSet<String>> = HashMap::new();
    // Track reference-trait names so we can skip them as targets.
    let mut reference_traits: HashSet<(String, String)> = HashSet::new();

    for orbital in orbitals {
        for trait_ref in &orbital.traits {
            match trait_ref {
                TraitReference::Inline(td) => {
                    let entry = trait_transitions
                        .entry((orbital.name.clone(), td.name.clone()))
                        .or_default();
                    for trans in &td.state_machine.transitions {
                        entry.insert(trans.event.clone());
                    }
                }
                TraitReference::Reference { name, reference, .. } => {
                    let trait_name = name
                        .clone()
                        .or_else(|| {
                            reference
                                .as_deref()
                                .and_then(|r| r.rsplit('.').next().map(str::to_owned))
                        });
                    if let Some(n) = trait_name {
                        reference_traits.insert((orbital.name.clone(), n));
                    }
                }
            }
        }
    }

    // Index (orbital, trait) -> (source_orbital, source_trait, event) listens.
    let mut trait_listens: HashMap<(String, String), HashSet<(String, String, String)>> =
        HashMap::new();
    for orbital in orbitals {
        for trait_ref in &orbital.traits {
            let TraitReference::Inline(td) = trait_ref else { continue };
            let entry = trait_listens
                .entry((orbital.name.clone(), td.name.clone()))
                .or_default();
            if let Some(listens) = &td.listens {
                for listen in listens {
                    let (source_orbital, source_trait) = match &listen.source {
                        Some(ListenSource::Trait { trait_name }) => {
                            (orbital.name.clone(), trait_name.clone())
                        }
                        Some(ListenSource::Orbital {
                            orbital: o,
                            trait_name,
                        }) => (o.clone(), trait_name.clone()),
                        Some(ListenSource::Any) | None => continue,
                    };
                    entry.insert((source_orbital, source_trait, listen.event.clone()));
                }
            }
        }
    }

    // Scan each inline trait for button actions in render-ui effects.
    for orbital in orbitals {
        for trait_ref in &orbital.traits {
            let TraitReference::Inline(td) = trait_ref else { continue };
            let source_trait = &td.name;
            let source_key = (orbital.name.clone(), source_trait.clone());

            let mut actions: HashSet<String> = HashSet::new();
            for trans in &td.state_machine.transitions {
                if let Some(effects) = &trans.effects {
                    for effect in effects {
                        collect_render_ui_button_actions(effect, &mut actions);
                    }
                }
            }

            for action in actions {
                // Self-targeting: this trait handles the action itself.
                if let Some(events) = trait_transitions.get(&source_key) {
                    if events.contains(&action) {
                        continue;
                    }
                }

                // Find sibling inline traits in the same orbital with a
                // transition on this action.
                let mut found_target = false;
                for ((t_orbital, t_trait), events) in &trait_transitions {
                    if t_orbital != &orbital.name || t_trait == source_trait {
                        continue;
                    }
                    // Skip reference traits — their transitions aren't visible
                    // at this stage.
                    if reference_traits.contains(&(t_orbital.clone(), t_trait.clone())) {
                        continue;
                    }
                    if events.contains(&action) {
                        found_target = true;
                        let target_key = (t_orbital.clone(), t_trait.clone());
                        let has_listen = trait_listens
                            .get(&target_key)
                            .map(|listens| {
                                listens.contains(&(orbital.name.clone(), source_trait.clone(), action.clone()))
                            })
                            .unwrap_or(false);
                        if !has_listen {
                            return Err(LowerError::ButtonTransitionUnreachable {
                                orbital: orbital.name.clone(),
                                source_trait: source_trait.clone(),
                                action: action.clone(),
                                target_trait: t_trait.clone(),
                            });
                        }
                    }
                }

                // If no inline target exists, the action may target a
                // reference trait (deferred to L2) or an external listener.
                // Don't error — either case is valid.
                let _ = found_target;
            }
        }
    }

    Ok(())
}

/// L1.2 check: every `(navigate "<path>")` effect references a route
/// declared on some orbital's `pages[].path` in this file. Catches typos
/// and dangling links at parse time before they show up as silent
/// client-side router no-ops at runtime.
///
/// Path matching uses canonicalisation: dynamic placeholders on either
/// side reduce to `<seg>`, so a route declared `/projects/:id` matches a
/// navigate call `/projects/@payload.id` without authors having to thread
/// the literal `:id` through their effect strings. Static segments must
/// match verbatim.
///
/// Reference / Object page forms (`Alias.pages.X`, `{ ref: ... }`) are
/// skipped — those are imported from std behaviors and resolved by the
/// inline phase. The L2 validator's `cross_orbital.rs` picks up post-
/// inline reachability for those.
fn check_navigate_targets(orbitals: &[CoreOrbital]) -> Result<(), LowerError> {
    let mut canonical_pages: HashSet<String> = HashSet::new();
    for orbital in orbitals {
        if let Some(pages) = &orbital.pages {
            for page_ref in pages {
                if let PageRef::Inline(p) = page_ref {
                    canonical_pages.insert(canonicalise_route(&p.path));
                }
                // Object / Reference forms: deferred to post-inline L2.
                // The path override on a PageRefObject is also intentionally
                // skipped here — the imported page's full path tree only
                // resolves after `uses` substitution.
            }
        }
    }

    for orbital in orbitals {
        for trait_ref in &orbital.traits {
            let TraitReference::Inline(td) = trait_ref else { continue };
            for trans in &td.state_machine.transitions {
                if let Some(effects) = &trans.effects {
                    for effect in effects {
                        check_navigate_recursive(
                            effect,
                            &canonical_pages,
                            &orbital.name,
                            &td.name,
                        )?;
                    }
                }
            }
        }
    }
    Ok(())
}

/// Recursively walk an SExpression and validate every `["navigate", path]`
/// call site. Descends into `(if ...)`, `(do ...)`, and similar control-
/// flow operators that wrap effects.
fn check_navigate_recursive(
    expr: &SExpression,
    canonical_pages: &HashSet<String>,
    orbital_name: &str,
    trait_name: &str,
) -> Result<(), LowerError> {
    match expr {
        SExpression::Array(arr) => {
            // Direct `["navigate", "<path>", ...]` call?
            if let Some(SExpression::String(op)) = arr.first() {
                if op == "navigate" {
                    if let Some(SExpression::String(path)) = arr.get(1) {
                        let canonical = canonicalise_route(path);
                        if !canonical_pages.contains(&canonical) {
                            return Err(LowerError::NavigateTargetUnreachable {
                                orbital: orbital_name.to_string(),
                                trait_name: trait_name.to_string(),
                                path: path.clone(),
                            });
                        }
                    }
                }
            }
            // Always recurse — `navigate` may appear inside `(if cond
            // (navigate ...) (navigate ...))` or `(do (set ...)
            // (navigate ...))`.
            for item in arr {
                check_navigate_recursive(item, canonical_pages, orbital_name, trait_name)?;
            }
        }
        SExpression::Object(obj) => {
            for value in obj.values() {
                check_navigate_recursive(value, canonical_pages, orbital_name, trait_name)?;
            }
        }
        _ => {}
    }
    Ok(())
}

/// Reduce a route string to its canonical structural form. Dynamic
/// segments — `:id` (page-side placeholder) and `@payload.X` /
/// `@entity.X` / any segment containing `@` (navigate-side
/// interpolation) — collapse to `<seg>`. Trailing slash normalised.
///
/// Examples:
/// - `/projects/:id`              → `/projects/<seg>`
/// - `/projects/@payload.id`      → `/projects/<seg>`
/// - `/projects/:id/preview`      → `/projects/<seg>/preview`
/// - `/`                           → `<empty>`
/// - `/contacts/`                 → `/contacts`
fn canonicalise_route(path: &str) -> String {
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() {
        return "<root>".to_string();
    }
    trimmed
        .split('/')
        .map(|seg| {
            if seg.starts_with(':') || seg.contains('@') {
                "<seg>".to_string()
            } else {
                seg.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("/")
}

/// L1.3 check: every `(emit EVT { ... })` literal payload conforms to
/// the declared `emits { EVT { ... } }` block on the same trait.
///
/// Three classes of mismatch caught at parse time (parse-time mirror of
/// L2's `EmitPayloadMissingRequired` / `EmitPayloadExtraField` /
/// `EmitPayloadFieldTypeMismatch` validators in
/// `orbital-compiler/src/phases/validation/emit_payload.rs`):
///
/// 1. Missing required field: declared with `!` but absent from the
///    literal payload.
/// 2. Extra field: present in the literal but not declared on the
///    event's payload schema.
/// 3. Primitive type mismatch: literal value's primitive type
///    (string / number / boolean / object / array) doesn't match the
///    declared `field_type`.
///
/// Bindings (`@payload.X`, `@entity.X`, etc.) skip the type-compare —
/// those resolve at L2 via the F3 binding resolver. This pass only
/// catches literal-vs-declared mismatches that don't need cross-IR
/// resolution.
///
/// Emits with no literal payload object (e.g. `(emit DONE)` / `(emit
/// DONE @some.binding)`) skip the extra-field and type-mismatch
/// branches but still get the missing-required check.
fn check_emit_payload_literals(orbitals: &[CoreOrbital]) -> Result<(), LowerError> {
    for orbital in orbitals {
        for trait_ref in &orbital.traits {
            let TraitReference::Inline(td) = trait_ref else { continue };

            // Build event -> payload schema lookup once per trait.
            let emit_schemas: HashMap<&str, &[PayloadField]> = td
                .emits
                .as_deref()
                .unwrap_or(&[])
                .iter()
                .map(|e| (e.event.as_str(), e.payload_schema.as_deref().unwrap_or(&[])))
                .collect();

            if emit_schemas.is_empty() {
                continue;
            }

            // Walk every transition's effects + every tick's effects.
            let trait_emits_walk =
                |effects: &[SExpression]| -> Result<(), LowerError> {
                    for effect in effects {
                        check_emit_literal_recursive(
                            effect,
                            &emit_schemas,
                            &orbital.name,
                            &td.name,
                        )?;
                    }
                    Ok(())
                };

            for trans in &td.state_machine.transitions {
                if let Some(effects) = &trans.effects {
                    trait_emits_walk(effects)?;
                }
            }
            if let Some(ticks) = &td.ticks {
                for tick in ticks {
                    trait_emits_walk(&tick.effects)?;
                }
            }
        }
    }
    Ok(())
}

/// Recursively walk an SExpression and validate every `["emit", event,
/// payload]` call site against the trait's emit schemas. Descends into
/// nested arrays so emits inside `(if cond ...)` / `(do ...)` are
/// caught.
fn check_emit_literal_recursive(
    expr: &SExpression,
    emit_schemas: &HashMap<&str, &[PayloadField]>,
    orbital_name: &str,
    trait_name: &str,
) -> Result<(), LowerError> {
    match expr {
        SExpression::Array(arr) => {
            // Direct `["emit", "<event>", <payload>]`?
            if let Some(SExpression::String(op)) = arr.first() {
                if op == "emit" {
                    if let Some(SExpression::String(event)) = arr.get(1) {
                        if let Some(&schema) = emit_schemas.get(event.as_str()) {
                            check_one_emit_literal(
                                event,
                                arr.get(2),
                                schema,
                                orbital_name,
                                trait_name,
                            )?;
                        }
                        // Unknown event names handled by the L2 validator's
                        // EffectEmitUndeclaredEvent — not duplicating that
                        // class at L1 in this commit.
                    }
                }
            }
            for item in arr {
                check_emit_literal_recursive(item, emit_schemas, orbital_name, trait_name)?;
            }
        }
        SExpression::Object(obj) => {
            for value in obj.values() {
                check_emit_literal_recursive(value, emit_schemas, orbital_name, trait_name)?;
            }
        }
        _ => {}
    }
    Ok(())
}

/// Validate a single `(emit EVT payload)` against the declared payload
/// schema. Errors fast on the first mismatch — sequential parse-time
/// reporting matches the rest of `lower.rs`'s style.
fn check_one_emit_literal(
    event: &str,
    payload: Option<&SExpression>,
    schema: &[PayloadField],
    orbital_name: &str,
    trait_name: &str,
) -> Result<(), LowerError> {
    let payload_obj = match payload {
        Some(SExpression::Object(o)) => Some(o),
        _ => None,
    };

    // Missing required fields.
    for field in schema.iter().filter(|f| f.required.unwrap_or(false)) {
        let present = payload_obj
            .map(|obj| obj.contains_key(&field.name))
            .unwrap_or(false);
        if !present {
            return Err(LowerError::EmitPayloadMissingRequired {
                orbital: orbital_name.to_string(),
                trait_name: trait_name.to_string(),
                event: event.to_string(),
                field: field.name.clone(),
                expected_type: field.field_type.clone(),
            });
        }
    }

    // Extra fields + primitive type mismatches — only when the emit
    // carries a literal payload object. Bindings or omitted payloads
    // skip these checks.
    let Some(obj) = payload_obj else {
        return Ok(());
    };
    let declared: HashMap<&str, &str> = schema
        .iter()
        .map(|f| (f.name.as_str(), f.field_type.as_str()))
        .collect();
    for (key, value) in obj.iter() {
        match declared.get(key.as_str()) {
            None => {
                return Err(LowerError::EmitPayloadExtraField {
                    orbital: orbital_name.to_string(),
                    trait_name: trait_name.to_string(),
                    event: event.to_string(),
                    field: key.clone(),
                });
            }
            Some(expected) => {
                if let Some(actual) = primitive_type_of_emit_value(value) {
                    if !emit_primitive_compatible(expected, actual) {
                        return Err(LowerError::EmitPayloadFieldTypeMismatch {
                            orbital: orbital_name.to_string(),
                            trait_name: trait_name.to_string(),
                            event: event.to_string(),
                            field: key.clone(),
                            declared: expected.to_string(),
                            actual: actual.to_string(),
                        });
                    }
                }
            }
        }
    }
    Ok(())
}

/// Primitive type of a literal emit-payload value. `None` for bindings
/// (`@payload.X`, `@entity.X`, etc.) and other compound expressions
/// whose type comes from the F3 binding resolver at L2 — those are
/// outside L1's scope and reported separately.
fn primitive_type_of_emit_value(value: &SExpression) -> Option<&'static str> {
    match value {
        SExpression::Number(_) => Some("number"),
        SExpression::Bool(_) => Some("boolean"),
        SExpression::String(s) => {
            if s.starts_with('@') || s.contains('@') {
                None
            } else {
                Some("string")
            }
        }
        SExpression::Null => None,
        SExpression::Object(_) => Some("object"),
        SExpression::Array(_) => Some("array"),
    }
}

/// Lenient primitive compatibility check for emit payload literals.
/// Mirrors the L2 validator's `primitive_types_compatible` so the two
/// detection layers agree. `any` matches anything; `date`/`datetime`/
/// `timestamp` accept string or number literals; `entity`/`relation`
/// accept object or string literals.
fn emit_primitive_compatible(declared: &str, actual: &str) -> bool {
    if declared == actual {
        return true;
    }
    // Strip optional `!` suffix on declared — payload field types
    // sometimes serialize as `string!` to mark required.
    let declared_core = declared.trim_end_matches('!');
    if declared_core == actual {
        return true;
    }
    match declared_core {
        "any" => true,
        "date" | "datetime" | "timestamp" => actual == "string" || actual == "number",
        "entity" | "relation" => actual == "object" || actual == "string",
        _ => false,
    }
}

fn lower_tick(t: &Tick) -> TickDefinition {
    let interval = match &t.interval {
        TickInterval::Millis(n) => StringOrNumber::Number(*n as f64),
        TickInterval::Cron(s) => StringOrNumber::String(s.clone()),
    };
    TickDefinition {
        name: t.name.clone(),
        interval,
        guard: t.guard.clone(),
        effects: t.effects.clone(),
        applies_to: None,
        description: None,
    }
}

/// Dispatch a `Page` AST node to either an inline definition or a reference,
/// returning the `PageRef` shape that the core schema expects.
fn lower_page_decl(p: &Page) -> PageRef {
    if let Some(reference) = &p.reference {
        let traits = if p.traits.is_empty() {
            None
        } else {
            Some(
                p.traits
                    .iter()
                    .map(|name| TraitReference::Reference {
                        reference: Some(name.clone()),
                        name: None,
                        linked_entity: None,
                        config: None,
                        events: None,
                        fields: None,
                        listens: None,
                        emits_scope: None,
                    })
                    .collect(),
            )
        };
        PageRef::Object(PageRefObject {
            reference: reference.clone(),
            path: Some(p.path.clone()),
            linked_entity: p.primary_entity.clone(),
            traits,
        })
    } else {
        lower_page(p)
    }
}

fn lower_page(p: &Page) -> PageRef {
    PageRef::Inline(PageDefinition {
        name: p.name.clone().unwrap_or_else(|| page_name_from_path(&p.path)),
        path: p.path.clone(),
        view_type: p.view.clone(),
        primary_entity: p.primary_entity.clone(),
        is_initial: if p.is_initial { Some(true) } else { None },
        traits: Some(
            p.traits
                .iter()
                .map(|name| TraitReference::Reference {
                    reference: Some(name.clone()),
                    name: None,
                    linked_entity: None,
                    config: None,
                    events: None,
                    fields: None,
                    listens: None,
                    emits_scope: None,
                })
                .collect(),
        ),
        sections: None,
    })
}

fn page_name_from_path(path: &str) -> String {
    let trimmed = path.trim_start_matches('/').trim_end_matches('/');
    if trimmed.is_empty() {
        return "Home".to_string();
    }
    let mut name = String::new();
    let mut upper = true;
    for c in trimmed.chars() {
        if c == '/' || c == '-' || c == '_' || c == ':' {
            upper = true;
        } else if upper {
            name.push(c.to_ascii_uppercase());
            upper = false;
        } else {
            name.push(c);
        }
    }
    if name.is_empty() {
        "Home".to_string()
    } else {
        name
    }
}

fn type_expr_to_string(ty: &TypeExpr) -> String {
    match ty {
        TypeExpr::Primitive(s) | TypeExpr::Named(s) => s.clone(),
        TypeExpr::StringLiteral(_) => "string".to_string(),
        TypeExpr::Array(inner) => format!("[{}]", type_expr_to_string(inner)),
        TypeExpr::Object(_) => "object".to_string(),
        TypeExpr::Union(_) => "string".to_string(),
        TypeExpr::Optional(inner) => type_expr_to_string(inner),
        TypeExpr::RelationMany(name) => format!("{name}*"),
        TypeExpr::RelationManyNonEmpty(name) => format!("{name}+"),
        TypeExpr::Trait => "trait".to_string(),
        TypeExpr::Slot => "slot".to_string(),
        TypeExpr::Pattern => "pattern".to_string(),
    }
}

/// Convert an event key to a human-readable display name for the
/// state-machine `events[].name` field. Rules (best-effort; authors override
/// via an events block when the trait-level metadata matters).
///
/// - `INIT` → `"Initialize"` (special case; convention from existing .orb).
/// - `UPPER_SNAKE_CASE` → Title Case: `REQUEST_REMOVE` → `"Request Remove"`.
/// - Single all-caps word → Title: `SAVE` → `"Save"`, `OPEN` → `"Open"`.
/// - `MixedCase` starting with the trait's linked entity name → the entity
///   name unchanged + space-lowercased suffix: given entity `ConfirmAction`,
///   `ConfirmActionLoaded` → `"ConfirmAction loaded"`,
///   `ConfirmActionLoadFailed` → `"ConfirmAction load failed"`.
/// - Unmatched cases return the key unchanged.
pub(crate) fn humanize_event_key(key: &str, entity_name: Option<&str>) -> String {
    if key == "INIT" {
        return "Initialize".to_string();
    }
    // UPPER_SNAKE_CASE.
    if !key.is_empty()
        && key
            .chars()
            .all(|c| c.is_ascii_uppercase() || c == '_' || c.is_ascii_digit())
    {
        return key
            .split('_')
            .filter(|w| !w.is_empty())
            .map(title_case_word)
            .collect::<Vec<_>>()
            .join(" ");
    }
    // MixedCase with a leading entity-name prefix.
    if let Some(ent) = entity_name {
        if key.starts_with(ent) && key.len() > ent.len() {
            let suffix = &key[ent.len()..];
            let humanized = split_camel(suffix)
                .into_iter()
                .map(|w| w.to_ascii_lowercase())
                .collect::<Vec<_>>()
                .join(" ");
            if humanized.is_empty() {
                return ent.to_string();
            }
            return format!("{ent} {humanized}");
        }
    }
    // Generic MixedCase split: keep leading word capitalised, lowercase tail.
    let parts = split_camel(key);
    if parts.len() >= 2 {
        let head = parts[0].clone();
        let tail = parts[1..]
            .iter()
            .map(|w| w.to_ascii_lowercase())
            .collect::<Vec<_>>()
            .join(" ");
        return format!("{head} {tail}");
    }
    key.to_string()
}

fn title_case_word(w: &str) -> String {
    let mut chars = w.chars();
    match chars.next() {
        Some(c) => c.to_ascii_uppercase().to_string() + &chars.as_str().to_ascii_lowercase(),
        None => String::new(),
    }
}

/// Split a CamelCase / PascalCase string into word segments at boundaries
/// between a lowercase char and an uppercase char. `ConfirmActionLoaded` →
/// `["Confirm", "Action", "Loaded"]`. Consecutive uppercase letters stay
/// together up to the next lowercase: `HTTPRequest` → `["HTTPRequest"]`
/// (degenerate, acceptable).
fn split_camel(s: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    let chars: Vec<char> = s.chars().collect();
    for i in 0..chars.len() {
        let c = chars[i];
        if i > 0 && c.is_ascii_uppercase() && chars[i - 1].is_ascii_lowercase() {
            out.push(std::mem::take(&mut cur));
        }
        cur.push(c);
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse_str;

    fn lower(src: &str) -> OrbitalSchema {
        let ast = parse_str(src).expect("parse");
        lower_to_orb(&ast).expect("lower")
    }

    #[test]
    fn lower_minimal() {
        let s = lower("orbital Foo { entity Bar { id : string! } }");
        assert_eq!(s.name, "Foo");
        assert_eq!(s.orbitals.len(), 1);
    }

    #[test]
    fn lower_persistence_runtime() {
        let s = lower("orbital Foo { entity Bar [runtime] { id : string! } }");
        let ent = match &s.orbitals[0].entity {
            EntityRef::Inline(e) => e,
            _ => panic!("expected inline entity"),
        };
        assert_eq!(ent.persistence, Some(CorePersistence::Runtime));
    }

    #[test]
    fn lower_field_required() {
        let s = lower("orbital Foo { entity Bar { id : string! } }");
        let ent = match &s.orbitals[0].entity {
            EntityRef::Inline(e) => e,
            _ => panic!(),
        };
        assert_eq!(ent.fields[0].required, Some(true));
    }

    #[test]
    fn lower_state_transition() {
        let s = lower(
            "orbital O { entity E { id : string! } trait T -> E [interaction] { state s { EVT -> s (set @id 1) } } }",
        );
        let t = match &s.orbitals[0].traits[0] {
            TraitReference::Inline(td) => td,
            _ => panic!(),
        };
        assert_eq!(t.state_machine.states.len(), 1);
        assert_eq!(t.state_machine.states[0].is_initial, Some(true));
        assert_eq!(t.state_machine.transitions.len(), 1);
        assert_eq!(t.state_machine.transitions[0].event, "EVT");
        assert_eq!(t.state_machine.transitions[0].to, "s");
    }

    #[test]
    fn lower_set_effect_lowers_self_sigil() {
        let s = lower(
            "orbital O { entity E { id : string! } trait T -> E { state s { E -> s (set @entity.x 1) } } }",
        );
        let t = match &s.orbitals[0].traits[0] {
            TraitReference::Inline(td) => td,
            _ => panic!(),
        };
        let eff = &t.state_machine.transitions[0].effects.as_ref().unwrap()[0];
        let arr = eff.as_array().unwrap();
        assert_eq!(arr[0].as_str(), Some("set"));
        assert_eq!(arr[1].as_str(), Some("@entity.x"));
    }

    #[test]
    fn lower_modal_close() {
        let s = lower(
            "orbital O { entity E { id : string! } trait T -> E { state s { CLOSE -> s (render-ui modal null) } } }",
        );
        let t = match &s.orbitals[0].traits[0] {
            TraitReference::Inline(td) => td,
            _ => panic!(),
        };
        let eff = &t.state_machine.transitions[0].effects.as_ref().unwrap()[0];
        let arr = eff.as_array().unwrap();
        assert_eq!(arr[2], SExpression::Null);
    }

    #[test]
    fn lower_emit_with_payload() {
        let s = lower(
            "orbital O { entity E { id : string! } trait T -> E { state s { E -> s (emit FOO { id: ?id }) } } }",
        );
        let t = match &s.orbitals[0].traits[0] {
            TraitReference::Inline(td) => td,
            _ => panic!(),
        };
        let eff = &t.state_machine.transitions[0].effects.as_ref().unwrap()[0];
        let arr = eff.as_array().unwrap();
        assert_eq!(arr[0].as_str(), Some("emit"));
        assert_eq!(arr[1].as_str(), Some("FOO"));
        let obj = arr[2].as_object().unwrap();
        assert_eq!(obj.get("id").unwrap().as_str(), Some("@payload.id"));
    }

    #[test]
    fn lower_time_bound_state_adds_timeout() {
        let s = lower(
            "orbital O { entity E { id : string! } trait T -> E { state s for 30s { EVT -> s } } }",
        );
        let t = match &s.orbitals[0].traits[0] {
            TraitReference::Inline(td) => td,
            _ => panic!(),
        };
        let timeout = t
            .state_machine
            .transitions
            .iter()
            .find(|tr| tr.event == "TIMEOUT");
        assert!(timeout.is_some(), "expected synthesized TIMEOUT transition");
    }

    #[test]
    fn lower_type_alias_to_enum_field() {
        let s = lower(
            "type Status = active | inactive\norbital O { entity E { id : string!\n status : Status } }",
        );
        let ent = match &s.orbitals[0].entity {
            EntityRef::Inline(e) => e,
            _ => panic!(),
        };
        let status = &ent.fields[1];
        assert_eq!(status.field_type, FieldType::String);
        assert_eq!(
            status.values,
            Some(vec!["active".to_string(), "inactive".to_string()])
        );
    }

    #[test]
    fn lower_let_bindings_form() {
        let s = lower(
            "orbital O { entity E { id : string! } trait T -> E { state s { EVT -> s (let ((x 1) (y 2)) (set @id 0)) } } }",
        );
        let t = match &s.orbitals[0].traits[0] {
            TraitReference::Inline(td) => td,
            _ => panic!(),
        };
        let eff = &t.state_machine.transitions[0].effects.as_ref().unwrap()[0];
        let arr = eff.as_array().unwrap();
        assert_eq!(arr[0].as_str(), Some("let"));
        let bindings = arr[1].as_array().unwrap();
        assert_eq!(bindings.len(), 2);
        let pair = bindings[0].as_array().unwrap();
        assert_eq!(pair[0].as_str(), Some("x"));
    }

    #[test]
    fn lower_behavior_compose_top_level() {
        let s = lower(
            "orbital O { entity E { id : string! } (behavior/compose { appName: \"Foo\" }) }",
        );
        // The composition s-expression is preserved on the orbital body for
        // the Compose phase to resolve later. We don't surface it on the core
        // schema yet (no field for that exists), so we just verify lowering
        // does not error.
        assert_eq!(s.name, "O");
    }

    // ==========================================================================
    // Explicit-binding rule (L1) — EntityFieldNeverSet
    // ==========================================================================

    #[test]
    fn lower_entity_field_never_set_fails() {
        // render-ui binds @entity.currentPage but no transition (set)s it.
        let src = "
            orbital O {
                entity E {
                    id : string!
                    currentPage : number = 1
                }
                trait T -> E [interaction, instance] {
                    state s {
                        INIT -> s
                            (render-ui main { type: \"pagination\", currentPage: \"@entity.currentPage\", totalPages: 1 })
                    }
                }
            }
        ";
        let ast = parse_str(src).expect("parse");
        let result = lower_to_orb(&ast);
        match result {
            Err(LowerError::EntityFieldNeverSet { trait_name, field, .. }) => {
                assert_eq!(trait_name, "T");
                assert_eq!(field, "currentPage");
            }
            other => panic!(
                "expected EntityFieldNeverSet for currentPage, got: {:?}",
                other
            ),
        }
    }

    #[test]
    fn lower_entity_field_set_explicitly_passes() {
        // (set @entity.currentPage 1) on INIT, then read by render-ui.
        let src = "
            orbital O {
                entity E {
                    id : string!
                    currentPage : number = 1
                }
                trait T -> E [interaction, instance] {
                    state s {
                        INIT -> s
                            (set @entity.currentPage 1)
                            (render-ui main { type: \"pagination\", currentPage: \"@entity.currentPage\", totalPages: 1 })
                    }
                }
            }
        ";
        let ast = parse_str(src).expect("parse");
        let result = lower_to_orb(&ast);
        assert!(
            result.is_ok(),
            "Field with explicit (set) must pass — got: {:?}",
            result.err()
        );
    }

    #[test]
    fn lower_entity_field_meta_id_skipped() {
        // @entity.id is a runtime-meta field; no setter required.
        let src = "
            orbital O {
                entity E { id : string! }
                trait T -> E [interaction, instance] {
                    state s {
                        INIT -> s
                            (render-ui main { type: \"text\", value: \"@entity.id\" })
                    }
                }
            }
        ";
        let ast = parse_str(src).expect("parse");
        let result = lower_to_orb(&ast);
        assert!(result.is_ok(), "@entity.id should be skipped");
    }

    #[test]
    fn lower_entity_field_no_linked_entity_skipped() {
        // Trait without `-> Entity` — rule skipped, no error even though
        // render-ui reads @entity.X.
        let src = "
            orbital O {
                entity E { id : string! }
                trait T [interaction, instance] {
                    state s {
                        INIT -> s
                            (render-ui main { type: \"text\", value: \"@entity.x\" })
                    }
                }
            }
        ";
        let ast = parse_str(src).expect("parse");
        let result = lower_to_orb(&ast);
        assert!(result.is_ok(), "Trait without linkedEntity should skip rule");
    }

    // ------------------------------------------------------------------
    // L1.2: ELOLO_NAVIGATE_TARGET_UNREACHABLE
    // ------------------------------------------------------------------

    #[test]
    fn navigate_to_declared_page_passes() {
        // `(navigate "/contacts")` is fine because the orbital declares
        // `page "/contacts"`. Reachable, no error.
        let src = r#"
            orbital ContactOrbital {
                entity Contact { id : string! }
                trait ContactBrowse -> Contact [interaction] {
                    initial: idle
                    state idle {
                        OPEN -> idle
                            (navigate "/contacts")
                    }
                }
                page "/contacts" -> ContactBrowse
            }
        "#;
        let ast = parse_str(src).expect("parse");
        let result = lower_to_orb(&ast);
        assert!(
            result.is_ok(),
            "navigate to a declared page path should pass: {:?}",
            result.err()
        );
    }

    #[test]
    fn navigate_with_dynamic_segment_matches_pattern_route() {
        // `(navigate "/contacts/@payload.id")` matches `page "/contacts/:id"`
        // — both canonicalise to `/contacts/<seg>`. This is the common
        // production shape (route patterns vs. interpolated paths).
        let src = r#"
            orbital ContactOrbital {
                entity Contact { id : string! }
                trait ContactBrowse -> Contact [interaction] {
                    initial: idle
                    state idle {
                        VIEW -> idle
                            (navigate "/contacts/@payload.id")
                    }
                }
                page "/contacts/:id" -> ContactBrowse
            }
        "#;
        let ast = parse_str(src).expect("parse");
        let result = lower_to_orb(&ast);
        assert!(
            result.is_ok(),
            "dynamic-segment navigate should match :id route: {:?}",
            result.err()
        );
    }

    #[test]
    fn navigate_to_undeclared_page_errors() {
        // No `page "/contacts"` exists; navigate should error.
        let src = r#"
            orbital ContactOrbital {
                entity Contact { id : string! }
                trait ContactBrowse -> Contact [interaction] {
                    initial: idle
                    state idle {
                        OPEN -> idle
                            (navigate "/contacts/typo")
                    }
                }
                page "/contacts" -> ContactBrowse
            }
        "#;
        let ast = parse_str(src).expect("parse");
        let result = lower_to_orb(&ast);
        match result {
            Err(LowerError::NavigateTargetUnreachable { path, .. }) => {
                assert_eq!(path, "/contacts/typo");
            }
            other => panic!("expected NavigateTargetUnreachable, got {:?}", other),
        }
    }

    #[test]
    fn navigate_to_root_passes() {
        // `(navigate "/")` matches `page "/"`. Both canonicalise to <root>.
        let src = r#"
            orbital HomeOrbital {
                entity Home [runtime] { id : string! }
                trait HomeView -> Home [interaction] {
                    initial: idle
                    state idle {
                        BACK -> idle
                            (navigate "/")
                    }
                }
                page "/" -> HomeView
            }
        "#;
        let ast = parse_str(src).expect("parse");
        let result = lower_to_orb(&ast);
        assert!(
            result.is_ok(),
            "navigate to root should match `/` page: {:?}",
            result.err()
        );
    }

    #[test]
    fn canonicalise_route_handles_dynamic_segments() {
        // Pure-function unit test for the canonicaliser. Both `:id` and
        // `@payload.X` reduce to `<seg>`; static segments compare as-is;
        // trailing slash is stripped.
        assert_eq!(canonicalise_route("/projects/:id"), "/projects/<seg>");
        assert_eq!(
            canonicalise_route("/projects/@payload.id"),
            "/projects/<seg>"
        );
        assert_eq!(canonicalise_route("/projects/:id/"), "/projects/<seg>");
        assert_eq!(canonicalise_route("/"), "<root>");
        assert_eq!(canonicalise_route("/contacts"), "/contacts");
        assert_eq!(
            canonicalise_route("/projects/:id/preview"),
            "/projects/<seg>/preview"
        );
    }

    // ------------------------------------------------------------------
    // L1.3: ELOLO_EMIT_PAYLOAD_LITERAL_MISMATCH
    // ------------------------------------------------------------------

    #[test]
    fn emit_with_complete_payload_passes() {
        // `(emit DELETED { id: "x" })` matches `emits { DELETED { id : string! } }`
        // — required field present, type matches, no extras.
        let src = r#"
            orbital ContactOrbital {
                entity Contact { id : string! }
                trait ContactDelete -> Contact [interaction] {
                    initial: idle
                    state idle {
                        DELETE -> idle
                            (emit DELETED { id: "abc" })
                    }
                    emits {
                        DELETED { id : string! }
                    }
                }
            }
        "#;
        let ast = parse_str(src).expect("parse");
        let result = lower_to_orb(&ast);
        assert!(
            result.is_ok(),
            "complete payload should pass: {:?}",
            result.err()
        );
    }

    #[test]
    fn emit_missing_required_field_errors() {
        // `(emit DELETED { /* no id */ })` violates `id : string!`.
        let src = r#"
            orbital ContactOrbital {
                entity Contact { id : string! }
                trait ContactDelete -> Contact [interaction] {
                    initial: idle
                    state idle {
                        DELETE -> idle
                            (emit DELETED { wrongField: "x" })
                    }
                    emits {
                        DELETED { id : string! }
                    }
                }
            }
        "#;
        let ast = parse_str(src).expect("parse");
        let result = lower_to_orb(&ast);
        match result {
            Err(LowerError::EmitPayloadMissingRequired { event, field, .. }) => {
                assert_eq!(event, "DELETED");
                assert_eq!(field, "id");
            }
            other => panic!("expected EmitPayloadMissingRequired, got {:?}", other),
        }
    }

    #[test]
    fn emit_extra_field_errors() {
        // `(emit DELETED { id, stale })` — `stale` isn't declared.
        let src = r#"
            orbital ContactOrbital {
                entity Contact { id : string! }
                trait ContactDelete -> Contact [interaction] {
                    initial: idle
                    state idle {
                        DELETE -> idle
                            (emit DELETED { id: "x", stale: "y" })
                    }
                    emits {
                        DELETED { id : string! }
                    }
                }
            }
        "#;
        let ast = parse_str(src).expect("parse");
        let result = lower_to_orb(&ast);
        match result {
            Err(LowerError::EmitPayloadExtraField { event, field, .. }) => {
                assert_eq!(event, "DELETED");
                assert_eq!(field, "stale");
            }
            other => panic!("expected EmitPayloadExtraField, got {:?}", other),
        }
    }

    #[test]
    fn emit_field_type_mismatch_errors() {
        // `id` declared as `string!` but literal value is a number.
        let src = r#"
            orbital ContactOrbital {
                entity Contact { id : string! }
                trait ContactDelete -> Contact [interaction] {
                    initial: idle
                    state idle {
                        DELETE -> idle
                            (emit DELETED { id: 42 })
                    }
                    emits {
                        DELETED { id : string! }
                    }
                }
            }
        "#;
        let ast = parse_str(src).expect("parse");
        let result = lower_to_orb(&ast);
        match result {
            Err(LowerError::EmitPayloadFieldTypeMismatch {
                event,
                field,
                declared,
                actual,
                ..
            }) => {
                assert_eq!(event, "DELETED");
                assert_eq!(field, "id");
                // Declared type may be "string" or "string!" depending on
                // how lowering normalises required markers.
                assert!(declared == "string" || declared == "string!");
                assert_eq!(actual, "number");
            }
            other => panic!("expected EmitPayloadFieldTypeMismatch, got {:?}", other),
        }
    }

    #[test]
    fn emit_with_binding_skips_type_check() {
        // `id: "@payload.id"` is a binding — type comes from the F3
        // resolver at L2, not from the literal. The L1 mirror skips
        // the type-compare branch for bindings.
        let src = r#"
            orbital ContactOrbital {
                entity Contact { id : string! }
                trait ContactDelete -> Contact [interaction] {
                    initial: idle
                    state idle {
                        DELETE -> idle
                            (emit DELETED { id: "@payload.id" })
                    }
                    listens {
                        DELETE { id : string! }
                    }
                    emits {
                        DELETED { id : string! }
                    }
                }
            }
        "#;
        let ast = parse_str(src).expect("parse");
        let result = lower_to_orb(&ast);
        assert!(
            result.is_ok(),
            "binding values should skip the L1 literal-type check: {:?}",
            result.err()
        );
    }

    // ------------------------------------------------------------------
    // L1.4: ELOLO_SET_VALUE_TYPE_MISMATCH
    // ------------------------------------------------------------------

    #[test]
    fn set_literal_string_to_string_field_passes() {
        // `(set @entity.title "hello")` matches `title : string`. No error.
        let src = r#"
            orbital ArticleOrbital {
                entity Article {
                    id : string!
                    title : string!
                }
                trait ArticleEditor -> Article [interaction] {
                    initial: idle
                    state idle {
                        INIT -> idle
                            (set @entity.title "Some title")
                    }
                }
            }
        "#;
        let ast = parse_str(src).expect("parse");
        let result = lower_to_orb(&ast);
        assert!(
            result.is_ok(),
            "literal string to string field should pass: {:?}",
            result.err()
        );
    }

    #[test]
    fn set_literal_number_to_string_field_errors() {
        // `(set @entity.title 42)` violates `title : string`.
        let src = r#"
            orbital ArticleOrbital {
                entity Article {
                    id : string!
                    title : string!
                }
                trait ArticleEditor -> Article [interaction] {
                    initial: idle
                    state idle {
                        INIT -> idle
                            (set @entity.title 42)
                    }
                }
            }
        "#;
        let ast = parse_str(src).expect("parse");
        let result = lower_to_orb(&ast);
        match result {
            Err(LowerError::SetValueTypeMismatch {
                field,
                expected_type,
                actual_type,
                value_source,
                ..
            }) => {
                assert_eq!(field, "title");
                assert_eq!(expected_type, "string");
                assert_eq!(actual_type, "number");
                assert_eq!(value_source, "literal");
            }
            other => panic!("expected SetValueTypeMismatch, got {:?}", other),
        }
    }

    #[test]
    fn set_payload_field_resolves_against_listen_block() {
        // `(set @entity.count @payload.count)` where the listen block
        // declares `count : number` and entity field is `count : int`
        // — both number, passes.
        let src = r#"
            orbital CounterOrbital {
                entity Counter {
                    id : string!
                    count : int
                }
                trait CounterEdit -> Counter [interaction] {
                    initial: idle
                    state idle {
                        UPDATE -> idle
                            (set @entity.count @payload.count)
                    }
                    listens {
                        UPDATE { count : int }
                    }
                }
            }
        "#;
        let ast = parse_str(src).expect("parse");
        let result = lower_to_orb(&ast);
        assert!(
            result.is_ok(),
            "matching @payload type should pass: {:?}",
            result.err()
        );
    }

    #[test]
    fn set_payload_wrong_type_errors() {
        // Listen declares `count : string` but entity expects `int`.
        let src = r#"
            orbital CounterOrbital {
                entity Counter {
                    id : string!
                    count : int
                }
                trait CounterEdit -> Counter [interaction] {
                    initial: idle
                    state idle {
                        UPDATE -> idle
                            (set @entity.count @payload.count)
                    }
                    listens {
                        UPDATE { count : string }
                    }
                }
            }
        "#;
        let ast = parse_str(src).expect("parse");
        let result = lower_to_orb(&ast);
        match result {
            Err(LowerError::SetValueTypeMismatch {
                field,
                expected_type,
                actual_type,
                value_source,
                ..
            }) => {
                assert_eq!(field, "count");
                assert_eq!(expected_type, "number");
                assert_eq!(actual_type, "string");
                assert_eq!(value_source, "@payload.count");
            }
            other => panic!("expected SetValueTypeMismatch, got {:?}", other),
        }
    }

    #[test]
    fn set_config_field_resolves_against_config_block() {
        // `(set @entity.limit @config.maxItems)` where config declares
        // `maxItems : int` and entity has `limit : int` — passes.
        let src = r#"
            orbital ListOrbital {
                entity List {
                    id : string!
                    limit : int
                }
                trait ListEdit -> List [interaction] {
                    initial: idle
                    state idle {
                        INIT -> idle
                            (set @entity.limit @config.maxItems)
                    }
                    config {
                        maxItems : int = 10
                    }
                }
            }
        "#;
        let ast = parse_str(src).expect("parse");
        let result = lower_to_orb(&ast);
        assert!(
            result.is_ok(),
            "matching @config type should pass: {:?}",
            result.err()
        );
    }

    #[test]
    fn set_nested_payload_path_defers_to_l2() {
        // `@payload.row.title` is a nested path — L1 doesn't recurse
        // into the row's entity definition. Defers to L2's F3 binding
        // resolver rather than emit a false positive. This is exactly
        // the std-cms ArticleView pattern where row : Article.
        let src = r#"
            orbital ArticleOrbital {
                entity Article {
                    id : string!
                    title : string!
                }
                trait ArticleView -> Article [interaction] {
                    initial: idle
                    state idle {
                        VIEW -> idle
                            (set @entity.title @payload.row.title)
                    }
                    listens {
                        VIEW { id : string!, row : Article }
                    }
                }
            }
        "#;
        let ast = parse_str(src).expect("parse");
        let result = lower_to_orb(&ast);
        assert!(
            result.is_ok(),
            "nested payload paths defer to L2, not error here: {:?}",
            result.err()
        );
    }

    #[test]
    fn resolve_top_segment_rejects_nested() {
        assert_eq!(resolve_top_segment("title"), Some("title".to_string()));
        assert_eq!(resolve_top_segment("row.title"), None);
        assert_eq!(resolve_top_segment("row.field.deep"), None);
        assert_eq!(resolve_top_segment("items[0]"), Some("items".to_string()));
        assert_eq!(resolve_top_segment(""), None);
    }

    #[test]
    fn emit_without_payload_block_skips_required_check() {
        // `emits { DONE }` declares no payload schema — emits like
        // `(emit DONE)` and `(emit DONE @payload)` both pass without
        // any field-shape check.
        let src = r#"
            orbital BatchOrbital {
                entity Batch [runtime] { id : string! }
                trait BatchRun -> Batch [interaction] {
                    initial: idle
                    state idle {
                        FINISH -> idle
                            (emit DONE)
                    }
                    emits {
                        DONE
                    }
                }
            }
        "#;
        let ast = parse_str(src).expect("parse");
        let result = lower_to_orb(&ast);
        assert!(
            result.is_ok(),
            "bare emits with no payload schema should pass: {:?}",
            result.err()
        );
    }
}
