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
    EntityFieldContract, EntityRef, EventDefinition, EventScope, ExpectDeclaration, FieldDefinition,
    FieldType,
    ListenDefinition, ListenSource, Orbital as CoreOrbital, OrbitalSchema, PageDefinition, PageRef,
    PageRefObject, PayloadField, Persistence as CorePersistence, RelationDefinition, SExpression,
    StateDefinition, StateMachine, StringOrArray, StringOrNumber, TickDefinition, TraitCategory,
    TraitDefinition, TraitReference, TransitionDefinition, UseDeclaration,
};
use std::collections::{BTreeMap, HashMap, HashSet};

mod checks;
mod emit_listen;
mod entity;
mod page;
mod payload_contract;
mod r#trait;
mod type_helpers;

pub(crate) use checks::{
    check_behavior_contracts, check_button_transitions, check_emit_payload_literals,
    check_entity_fields_have_setters, check_listens_integrity, check_set_value_types,
};
pub(crate) use emit_listen::{lower_emit, lower_listen, lower_tick, type_expr_to_payload_fields};
pub(crate) use entity::{lower_entity, lower_field, lower_type_expr, primitive_field_type};
pub(crate) use page::lower_page_decl;
pub(crate) use r#trait::lower_trait_decl;
pub(crate) use type_helpers::{
    emit_primitive_compatible, humanize_event_key, type_expr_to_compat_str,
    type_expr_to_config_string, type_expr_to_config_values, type_expr_to_string,
};

/// Lower an entire `.lolo` program into an `OrbitalSchema` (no identity
/// stamping — names only). Back-compat entry point; prefer
/// [`lower_to_orb_with_prior`] when a prior `.orb` is available so ids stay
/// stable across re-lowers.
pub fn lower_to_orb(ast: &Program) -> Result<OrbitalSchema, LowerError> {
    lower_to_orb_with_prior(ast, None)
}

/// Lower `ast` and stamp V4 identity: mint ids for authored nodes, resolve
/// in-file references to id edges, and emit the ledger + `schemaVersion`.
/// `prior` (a previously-emitted schema for the same source) anchors id reuse —
/// re-lowering the same `.lolo` with its own output as `prior` yields
/// byte-identical ids (zero churn). See [`self::identity`].
pub fn lower_to_orb_with_prior(
    ast: &Program,
    prior: Option<&OrbitalSchema>,
) -> Result<OrbitalSchema, LowerError> {
    lower_to_orb_with_prior_and_map(ast, prior).map(|(schema, _map)| schema)
}

/// [`lower_to_orb`] additionally returning the L2 source map (see
/// [`crate::source_map::LoloSourceMap`]).
pub fn lower_to_orb_with_map(
    ast: &Program,
) -> Result<(OrbitalSchema, crate::source_map::LoloSourceMap), LowerError> {
    lower_to_orb_with_prior_and_map(ast, None)
}

/// [`lower_to_orb_with_prior`] additionally returning the L2 source map. The
/// map never enters the schema itself (the emitted `.orb` stays byte
/// identical) — it's a side channel built alongside the ordinary lowering.
pub fn lower_to_orb_with_prior_and_map(
    ast: &Program,
    prior: Option<&OrbitalSchema>,
) -> Result<(OrbitalSchema, crate::source_map::LoloSourceMap), LowerError> {
    let mut source_map = crate::source_map::LoloSourceMap::new();
    let mut schema = lower_to_orb_names_only(ast, &mut source_map)?;
    let synthetic_traits: HashSet<String> = ast
        .orbitals
        .iter()
        .flat_map(|o| o.traits.iter())
        .filter(|t| t.is_inline_synthetic)
        .map(|t| t.name.clone())
        .collect();
    orbital_core::stamp::stamp_schema_identity(&mut schema, prior, &synthetic_traits);
    Ok((schema, source_map))
}

/// Stamp V4 identity directly onto an already-lowered `OrbitalSchema` that has
/// no `.lolo`/AST behind it — e.g. a name-only factory `.orb`. Mints ids for
/// every declaration, resolves in-file name refs to id edges, and writes the
/// ledger + `schemaVersion`. `prior` anchors id reuse (zero churn on re-stamp).
/// Factory schemas carry no parser-hoisted render-embed traits, so callers pass
/// an empty `synthetic_traits` set. Same minting authority as the `.lolo` path.
/// Re-exports the canonical minter, which now lives in `orbital_core::stamp`
/// so the compiler (a sibling crate of lolo) can auto-stamp on load.
pub fn stamp_schema_identity(
    schema: &mut OrbitalSchema,
    prior: Option<&OrbitalSchema>,
    synthetic_traits: &HashSet<String>,
) {
    orbital_core::stamp::stamp_schema_identity(schema, prior, synthetic_traits);
}

/// The name-only lowering (pre-identity). Kept private; the public entry
/// points wrap it.
fn lower_to_orb_names_only(
    ast: &Program,
    source_map: &mut crate::source_map::LoloSourceMap,
) -> Result<OrbitalSchema, LowerError> {
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
    // Members: every orbital's primary `entity` block in the program, any
    // `expects entity`/`expects identity` name (supplied by a sibling at
    // compose time), plus any type alias whose body is `TypeBody::Entity`
    // (declared with the `entity` keyword). `lower_type_expr` uses this to
    // decide whether `[Name]` lowers to a `cardinality: Many` Relation field
    // (entity) or a regular array of an inline object schema (struct alias).
    let mut file_entity_names: HashSet<String> = HashSet::new();
    // Entities declared `[shared]` — several bound traits co-mutate one shared
    // instance, so the per-trait explicit-binding check (a read with no LOCAL
    // setter) would false-positive when a sibling trait owns the setter.
    // `TypeBody::Entity` (type-alias-declared entities) carries no `shared`
    // flag today, so only inline `entity Name [shared] { ... }` blocks
    // contribute here.
    let mut file_shared_entity_names: HashSet<String> = HashSet::new();
    for orb in &ast.orbitals {
        if let Some(e) = &orb.entity {
            file_entity_names.insert(e.name.clone());
            if e.shared {
                file_shared_entity_names.insert(e.name.clone());
            }
        }
        for e in &orb.extra_entities {
            file_entity_names.insert(e.name.clone());
            if e.shared {
                file_shared_entity_names.insert(e.name.clone());
            }
        }
        // `expects entity`/`expects identity` names are legal relation
        // targets too — the sibling orbital supplies the declaration at
        // compose time (the L2 relations validator licenses the same names).
        file_entity_names.extend(
            crate::types::expected_entity_names(orb)
                .into_iter()
                .map(str::to_string),
        );
    }
    for ta in &ast.type_aliases {
        if matches!(ta.body, TypeBody::Entity { .. }) {
            file_entity_names.insert(ta.name.clone());
        }
    }

    let mut orbitals = Vec::with_capacity(ast.orbitals.len());
    for (orbital_index, orb) in ast.orbitals.iter().enumerate() {
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
        // The orbital's own entities (primary and extras) are also addressable
        // by name in emit payloads (`emits { SAVED :: CartItem }`).
        for e in orb.entity.iter().chain(orb.extra_entities.iter()) {
            local_entities.insert(
                e.name.clone(),
                e.fields
                    .iter()
                    .map(|f| (f.name.clone(), f.type_expr.clone(), f.required))
                    .collect(),
            );
        }
        let orbital_base = format!("orbitals[{orbital_index}]");
        orbitals.push(lower_orbital(
            orb,
            &local_aliases,
            &local_entities,
            &local_entity_names,
            &local_events,
            &file_shared_entity_names,
            source_map,
            &orbital_base,
        )?);
    }

    check_listens_integrity(&orbitals)?;
    check_button_transitions(&orbitals)?;
    check_emit_payload_literals(&orbitals)?;
    check_behavior_contracts(&orbitals)?;

    let mut schema = OrbitalSchema {
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
        theme: ast
            .app
            .theme
            .clone()
            .map(orbital_core::schema::ThemeRef::Reference),
        orbitals,
        metadata: None,
        ledger: None,
        schema_version: None,
    };

    // Canonicalize flat `fn` lambdas to the grouped form so the emitted/baked
    // `.orb` carries one shape (see `orbital_core::schema::normalize_fn_shapes`).
    orbital_core::schema::normalize_fn_shapes(&mut schema);

    Ok(schema)
}

#[derive(Debug, Clone)]
pub(crate) enum TypeAliasResolved {
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
    /// The alias's type-parameter name, when declared with either the bare
    /// (`type X T = ...`) or parenthesized kinded (`type X (E : Entity) =
    /// ...`) form. A payload field referencing this name lowers to the `$E`
    /// sentinel (increment 3-I) instead of erroring as an unknown type — the
    /// compiler's inline phase substitutes it with the using trait's bound
    /// entity.
    pub type_param: Option<String>,
}

/// Collect `type X = Event <T> [scope] "description"` aliases into a lookup
/// table. Mirrors `collect_entity_type_bodies`'s scoping: aliases live at
/// file scope and are extended per-orbital from `orb.type_aliases`.
fn collect_event_type_bodies(aliases: &[TypeAlias]) -> BTreeMap<String, EventTypeResolved> {
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
                    type_param: ta.type_param.clone(),
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

fn lower_expects_decl(
    decl: &ExpectsDecl,
    aliases: &BTreeMap<String, TypeAliasResolved>,
    entity_names: &HashSet<String>,
) -> Result<ExpectDeclaration, LowerError> {
    let lower_shape = |shape: &Option<Vec<EntityField>>| {
        shape
            .as_ref()
            .map(|fields| {
                fields
                    .iter()
                    .map(|f| lower_field(f, aliases, entity_names))
                    .collect::<Result<Vec<_>, _>>()
            })
            .transpose()
    };
    Ok(match &decl.kind {
        ExpectsKind::Identity { name, shape } => ExpectDeclaration::Identity {
            name: name.clone(),
            shape: lower_shape(shape)?,
        },
        ExpectsKind::Entity { name, shape } => ExpectDeclaration::Entity {
            name: name.clone(),
            shape: lower_shape(shape)?,
        },
        // Trait and event names pass through verbatim — no case-folding.
        ExpectsKind::Event { trait_name, event } => ExpectDeclaration::Event {
            trait_name: trait_name.clone(),
            event: event.clone(),
        },
    })
}

fn lower_orbital(
    orb: &Orbital,
    aliases: &BTreeMap<String, TypeAliasResolved>,
    entities: &BTreeMap<String, Vec<(String, TypeExpr, bool)>>,
    entity_names: &HashSet<String>,
    event_aliases: &BTreeMap<String, EventTypeResolved>,
    shared_entity_names: &HashSet<String>,
    source_map: &mut crate::source_map::LoloSourceMap,
    base: &str,
) -> Result<CoreOrbital, LowerError> {
    let entity = orb.entity.as_ref().ok_or(LowerError::MissingEntity)?;
    let entity_def = lower_entity(entity, aliases, entity_names)?;
    // Entity declarations beyond the first lower into `auxiliary_entities` —
    // the IR slot the validators, binding checks, and mock tier already
    // accept (Gap #22) — so a multi-collection orbital (e.g. a dashboard
    // reading two collections) is expressible.
    let extra_entity_defs: Vec<EntityDefinition> = orb
        .extra_entities
        .iter()
        .map(|e| lower_entity(e, aliases, entity_names))
        .collect::<Result<_, _>>()?;

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

    let expects = if orb.expects.is_empty() {
        None
    } else {
        orb.expects
            .iter()
            .map(|e| lower_expects_decl(e, aliases, entity_names))
            .collect::<Result<Vec<_>, _>>()
            .map(Some)?
    };

    // Intrinsic field names per entity — view-state/machinery the owning trait
    // computes itself, marked `@intrinsic` in the .lolo. These are excluded from
    // a rebindable trait's `entity_contract.requires` so the post-inline rebind
    // validator does NOT demand them on the rebind target (they travel WITH the
    // trait via the inline field-merge). Collected per entity (primary and
    // extras); each trait gets the set of the entity its `->` binding names,
    // falling back to the primary's set when unbound or unmatched.
    let mut intrinsic_by_entity: HashMap<&str, HashSet<String>> = HashMap::new();
    for e in std::iter::once(entity).chain(orb.extra_entities.iter()) {
        let set: HashSet<String> = e
            .fields
            .iter()
            .filter(|f| f.is_intrinsic)
            .map(|f| f.name.clone())
            .collect();
        intrinsic_by_entity.insert(e.name.as_str(), set);
    }
    let primary_intrinsics = &intrinsic_by_entity[entity.name.as_str()];

    let mut traits = Vec::new();
    for (trait_index, t) in orb.traits.iter().enumerate() {
        let trait_base = format!("{base}.traits[{trait_index}]");
        let intrinsic_fields = t
            .linked_entity
            .as_deref()
            .and_then(|n| intrinsic_by_entity.get(n))
            .unwrap_or(primary_intrinsics);
        traits.push(lower_trait_decl(
            t,
            aliases,
            entities,
            entity_names,
            event_aliases,
            intrinsic_fields,
            shared_entity_names,
            source_map,
            &trait_base,
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
                    time_bound_ms: None,
                }],
                events: Some(vec![EventDefinition {
                    key: "INIT".to_string(),
                    id: None,
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
                    event_id: None,
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
            id: None,
            linked_entity_id: None,
            entity_ref_ids: None,
            trait_embed_ids: None,
            type_params: None,
            type_args: None,
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
        expects,
        theme: orb.theme.as_ref().map(|name| {
            orbital_core::schema::ThemeRef::Reference(name.clone())
        }),
        services: None,
        entity: EntityRef::Inline(entity_def),
        auxiliary_entities: if extra_entity_defs.is_empty() {
            None
        } else {
            Some(
                extra_entity_defs
                    .into_iter()
                    .map(EntityRef::Inline)
                    .collect(),
            )
        },
        traits,
        pages,
        emits: None,
        listens: None,
        exposes: None,
        domain_context: None,
        design: None,
        id: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lower::checks::resolve_top_segment;
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
        assert!(s.theme.is_none());
    }

    #[test]
    fn lower_app_theme_and_format_round_trip() {
        let src = "app MyApp \"1.0.0\"\n\"Desc\"\ntheme \"linear-clean-light\"\norbital Foo {\n  entity Bar {\n    id : string!\n  }\n}";
        let s = lower(src);
        match &s.theme {
            Some(orbital_core::schema::ThemeRef::Reference(k)) => {
                assert_eq!(k, "linear-clean-light")
            }
            other => panic!("expected app theme reference, got {:?}", other),
        }
        // Formatter must re-emit the header theme line or `orb fmt` eats it.
        let ast = parse_str(src).expect("parse");
        let formatted = crate::format::format_program(&ast);
        assert!(
            formatted.contains("theme \"linear-clean-light\""),
            "formatted output lost the app theme line:\n{formatted}"
        );
        let reparsed = parse_str(&formatted).expect("reparse formatted");
        assert_eq!(reparsed.app.theme, Some("linear-clean-light".to_string()));
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

    fn lowered_entity(src: &str) -> EntityDefinition {
        let s = lower(src);
        match &s.orbitals[0].entity {
            EntityRef::Inline(e) => e.clone(),
            _ => panic!("expected inline entity"),
        }
    }

    #[test]
    fn lower_shared_with_runtime() {
        let ent = lowered_entity("orbital Foo { entity Bar [runtime, shared] { id : string! } }");
        assert!(ent.shared);
        assert_eq!(ent.persistence, Some(CorePersistence::Runtime));
    }

    #[test]
    fn lower_shared_with_persistent() {
        let ent =
            lowered_entity("orbital Foo { entity Bar [persistent, shared] { id : string! } }");
        assert!(ent.shared);
        assert_eq!(ent.persistence, Some(CorePersistence::Persistent));
    }

    #[test]
    fn lower_shared_alone_defaults_persistence_none() {
        let ent = lowered_entity("orbital Foo { entity Bar [shared] { id : string! } }");
        assert!(ent.shared);
        assert_eq!(ent.persistence, None);
    }

    #[test]
    fn lower_runtime_alone_is_not_shared() {
        let ent = lowered_entity("orbital Foo { entity Bar [runtime] { id : string! } }");
        assert!(!ent.shared);
        assert_eq!(ent.persistence, Some(CorePersistence::Runtime));
    }

    #[test]
    fn lower_shared_tag_order_independent() {
        let ent = lowered_entity("orbital Foo { entity Bar [shared, runtime] { id : string! } }");
        assert!(ent.shared);
        assert_eq!(ent.persistence, Some(CorePersistence::Runtime));
    }

    #[test]
    fn unknown_entity_tag_errors_at_parse() {
        let err = crate::parser::parse_str("orbital Foo { entity Bar [bogus] { id : string! } }")
            .expect_err("expected parse to reject unknown entity tag");
        assert!(matches!(
            err,
            crate::error::ParseError::InvalidPersistenceMode { .. }
        ));
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
        let state_s = t
            .state_machine
            .states
            .iter()
            .find(|st| st.name == "s")
            .expect("expected state 's'");
        assert_eq!(
            state_s.time_bound_ms,
            Some(30_000),
            "expected the 30s duration to survive lowering onto the state, not be discarded"
        );
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
            Err(LowerError::EntityFieldNeverSet {
                trait_name, field, ..
            }) => {
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
    fn lower_shared_entity_defers_setter_check_to_sibling_trait() {
        // Entity is [shared]: trait Reader reads @entity.count but never sets
        // it locally; sibling trait Writer owns the setter. The per-trait L1
        // check must defer (not false-positive) — L2 checks the whole shared
        // group post-inline.
        let src = "
            orbital O {
                entity E [shared] {
                    id : string!
                    count : number = 0
                }
                trait Reader -> E [interaction, instance] {
                    state s {
                        INIT -> s
                            (render-ui main { type: \"text\", value: \"@entity.count\" })
                    }
                }
                trait Writer -> E [interaction, instance] {
                    state s {
                        INCREMENT -> s
                            (set @entity.count 1)
                    }
                }
            }
        ";
        let ast = parse_str(src).expect("parse");
        let result = lower_to_orb(&ast);
        assert!(
            result.is_ok(),
            "a [shared] entity should defer the setter check to L2: {:?}",
            result.err()
        );
    }

    #[test]
    fn lower_non_shared_entity_still_errors_on_read_without_local_setter() {
        // Same shape as above (a sibling trait sets the field) but the entity
        // is NOT [shared] — each trait gets an isolated per-trait copy, so
        // Reader's read is genuinely never backed by a local setter and must
        // still hard-error.
        let src = "
            orbital O {
                entity E {
                    id : string!
                    count : number = 0
                }
                trait Reader -> E [interaction, instance] {
                    state s {
                        INIT -> s
                            (render-ui main { type: \"text\", value: \"@entity.count\" })
                    }
                }
                trait Writer -> E [interaction, instance] {
                    state s {
                        INCREMENT -> s
                            (set @entity.count 1)
                    }
                }
            }
        ";
        let ast = parse_str(src).expect("parse");
        let result = lower_to_orb(&ast);
        match result {
            Err(LowerError::EntityFieldNeverSet {
                trait_name, field, ..
            }) => {
                assert_eq!(trait_name, "Reader");
                assert_eq!(field, "count");
            }
            other => panic!(
                "expected EntityFieldNeverSet for count on non-shared entity, got: {:?}",
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
        assert!(
            result.is_ok(),
            "Trait without linkedEntity should skip rule"
        );
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
    fn navigate_to_undeclared_page_lowers_deferring_reachability_to_l2() {
        // L1 no longer checks navigate reachability (it collected pages per
        // parse-unit, so a single orbital extracted from a project failed on a
        // legitimate sibling-route navigate). A dangling target now lowers
        // cleanly at L1 and is caught post-inline by L2's
        // `ORB_EFF_NAVIGATE_TARGET_UNREACHABLE`, which sees project-wide pages.
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
        assert!(
            result.is_ok(),
            "L1 defers navigate reachability to L2: {:?}",
            result.err()
        );
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
    fn set_literal_outside_declared_enum_errors() {
        // `(set @entity.status pendng)` violates the declared union — Gap
        // G-LOLO-ENUM-UNENFORCED; the closest member feeds the suggestion.
        let src = r#"
            orbital TicketOrbital {
                entity Ticket {
                    id : string!
                    status : "open" | "closed" | "pending"
                }
                trait TicketFlow -> Ticket [interaction] {
                    initial: idle
                    state idle {
                        INIT -> idle
                            (set @entity.status pendng)
                    }
                }
            }
        "#;
        let ast = parse_str(src).expect("parse");
        match lower_to_orb(&ast) {
            Err(LowerError::EnumValueNotMember {
                field,
                value,
                suggestion,
                ..
            }) => {
                assert_eq!(field, "status");
                assert_eq!(value, "pendng");
                assert_eq!(suggestion.as_deref(), Some("pending"));
            }
            other => panic!("expected EnumValueNotMember, got {:?}", other),
        }
    }

    #[test]
    fn set_literal_member_of_declared_enum_passes() {
        // A declared member assigns clean.
        let src = r#"
            orbital TicketOrbital {
                entity Ticket {
                    id : string!
                    status : "open" | "closed" | "pending"
                }
                trait TicketFlow -> Ticket [interaction] {
                    initial: idle
                    state idle {
                        INIT -> idle
                            (set @entity.status closed)
                    }
                }
            }
        "#;
        let ast = parse_str(src).expect("parse");
        assert!(
            lower_to_orb(&ast).is_ok(),
            "a declared enum member should pass the membership check"
        );
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

    #[test]
    fn lower_expects_all_forms() {
        let s = lower(
            "orbital O {\n  expects identity Customer {\n    role : \"store-manager\" | \"operations-director\"\n  }\n  expects entity OrderRecord {\n    totalAmount : number\n  }\n  expects entity CartItem\n  expects event Checkout.ORDER_PLACED\n  entity E { id : string! }\n}",
        );
        let expects = s.orbitals[0].expects.as_ref().expect("expects lowered");
        assert_eq!(expects.len(), 4);
        match &expects[0] {
            ExpectDeclaration::Identity { name, shape } => {
                assert_eq!(name.as_deref(), Some("Customer"));
                let shape = shape.as_ref().expect("identity shape");
                assert_eq!(shape.len(), 1);
                assert_eq!(shape[0].name.as_deref(), Some("role"));
                assert_eq!(
                    shape[0].values.as_deref(),
                    Some(&["store-manager".to_string(), "operations-director".to_string()][..])
                );
            }
            other => panic!("expected identity, got {other:?}"),
        }
        match &expects[1] {
            ExpectDeclaration::Entity { name, shape } => {
                assert_eq!(name, "OrderRecord");
                let shape = shape.as_ref().expect("entity shape");
                assert_eq!(shape.len(), 1);
                assert_eq!(shape[0].name.as_deref(), Some("totalAmount"));
                assert!(matches!(shape[0].field_type, FieldType::Number));
            }
            other => panic!("expected entity, got {other:?}"),
        }
        match &expects[2] {
            ExpectDeclaration::Entity { name, shape } => {
                assert_eq!(name, "CartItem");
                assert!(shape.is_none());
            }
            other => panic!("expected entity, got {other:?}"),
        }
        match &expects[3] {
            ExpectDeclaration::Event { trait_name, event } => {
                // Verbatim passthrough — no case-folding on either segment.
                assert_eq!(trait_name, "Checkout");
                assert_eq!(event, "ORDER_PLACED");
            }
            other => panic!("expected event, got {other:?}"),
        }
    }

    #[test]
    fn lower_expects_absent_is_none() {
        let s = lower("orbital Foo { entity Bar { id : string! } }");
        assert!(s.orbitals[0].expects.is_none());
    }
}
