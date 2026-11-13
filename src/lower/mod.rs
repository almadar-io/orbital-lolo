//! Lower a `.lolo` AST into an `OrbitalSchema` from `orbital-core`.
//!
//! The lowering is mechanical and (when both inputs are canonical) byte
//! identical to a hand-authored `.orb`. See `Lolo_Language_Spec_v3.md`
//! "Lowering to .orb" table for the rules.

use crate::ast::*;
use std::sync::Arc;
use crate::error::{Located, LowerError};
use crate::patterns_meta::event_prop_names;
use crate::registry::LoloContracts;
use crate::types::check_program;
use orbital_core_base::behavior_packages::BehaviorPackage;
use orbital_core_base::effect_row::{EffectRow, EffectRowBuilder, EffectSite};
use orbital_core_base::schema::{
    Cardinality, ConfigField as SchemaConfigField, EmitDefinition, EntityDefinition,
    EntityFieldContract, EntityRef, EventDefinition, EventScope, ExpectDeclaration,
    FieldDefinition, FieldType, InboundDeclaration, InboundVerification, ListenDefinition,
    ListenSource, MotionShapeTokens, MotionTokens, Orbital as CoreOrbital, OrbitalRefObject,
    OrbitalSchema, PageDefinition, PageRef, PageRefObject, PayloadField,
    Persistence as CorePersistence, RelationDefinition, SExpression, StateDefinition, StateMachine,
    StringOrArray, StringOrNumber, ThemeDefinition, ThemeRef, ThemeTokens, ThemeVariant,
    TickDefinition, TraitDefinition, TraitReference, TransitionDefinition,
    UseDeclaration,
};
use std::collections::{BTreeMap, HashMap, HashSet};

mod checks;
mod config_knob_read;
mod emit_listen;
pub(crate) mod entity;
mod orbital_ref;
mod page;
mod payload_contract;
mod projection;
mod render_prop_types;
mod r#trait;
mod tree_fold;
mod type_helpers;

pub(crate) use checks::{
    check_behavior_contracts, check_button_transitions, check_emit_payload_literals,
    check_entity_fields_have_setters, check_expr_match_types, check_listens_integrity,
    check_match_types, check_set_value_types, check_shared_entity_has_persistence_mode,
    check_term_match_types,
};
pub(crate) use emit_listen::{
    bare_entity_alias_name, lower_emit, lower_listen, lower_tick, struct_payload_properties,
    type_expr_to_payload_fields,
};
pub(crate) use entity::{lower_entity, lower_field, lower_type_expr, primitive_field_type};
pub(crate) use page::{lower_page_decl, page_name_from_path};
pub(crate) use r#trait::{
    desugar_expr_match, desugar_notify, expand_term_match, is_expr_match_array, lower_trait_decl,
};
pub(crate) use type_helpers::{
    canonicalize_unbound_trait_default, emit_primitive_compatible, point_in_time_literal_fits, humanize_event_key,
    record_literal_fits,
    lower_config_fields_to_map, type_expr_to_compat_str, type_expr_to_config_string,
    type_expr_to_config_values, type_expr_to_string,
};

/// Explicit behavior-package set for a lower, bypassing filesystem discovery
/// (`orbital_core::behavior_packages::discover`). `packages: vec![]` (the
/// `Default`) resolves the std contracts from the embedded slice with no
/// other package searched — see [`crate::registry::LoloContracts::load`].
#[derive(Debug, Default, Clone)]
pub struct LowerOptions {
    pub packages: Vec<BehaviorPackage>,
    pub source_path: Option<std::path::PathBuf>,
}

/// Lower an entire `.lolo` program into an `OrbitalSchema` (no identity
/// stamping — names only). Back-compat entry point; prefer
/// [`lower_to_orb_with_prior`] when a prior `.orb` is available so ids stay
/// stable across re-lowers. Discovers installed behavior packages from the
/// current directory (see [`LowerOptions`] to pass an explicit set instead).
pub fn lower_to_orb(ast: &Program) -> Result<OrbitalSchema, LowerError> {
    lower_to_orb_with_prior(ast, None)
}

/// [`lower_to_orb`] against an explicit [`LowerOptions`] instead of
/// discovering packages from the current directory.
pub fn lower_to_orb_with(
    ast: &Program,
    options: &LowerOptions,
) -> Result<OrbitalSchema, LowerError> {
    lower_to_orb_with_prior_with(ast, None, options)
}

/// Lower `ast` and stamp V4 identity: mint ids for authored nodes, resolve
/// in-file references to id edges, and emit the ledger + `schemaVersion`.
/// `prior` (a previously-emitted schema for the same source) anchors id reuse —
/// re-lowering the same `.lolo` with its own output as `prior` yields
/// byte-identical ids (zero churn). See [`self::identity`]. Discovers
/// installed behavior packages from the current directory.
pub fn lower_to_orb_with_prior(
    ast: &Program,
    prior: Option<&OrbitalSchema>,
) -> Result<OrbitalSchema, LowerError> {
    let options = discovered_options()?;
    lower_to_orb_with_prior_with(ast, prior, &options)
}

/// [`lower_to_orb_with_prior`] against an explicit [`LowerOptions`].
pub fn lower_to_orb_with_prior_with(
    ast: &Program,
    prior: Option<&OrbitalSchema>,
    options: &LowerOptions,
) -> Result<OrbitalSchema, LowerError> {
    lower_to_orb_with_prior_and_map_with(ast, prior, options).map(|(schema, _map)| schema)
}

/// [`lower_to_orb`] additionally returning the L2 source map (see
/// [`crate::source_map::LoloSourceMap`]). Discovers installed behavior
/// packages from the current directory.
pub fn lower_to_orb_with_map(
    ast: &Program,
) -> Result<(OrbitalSchema, crate::source_map::LoloSourceMap), LowerError> {
    let options = discovered_options()?;
    lower_to_orb_with_prior_and_map_with(ast, None, &options)
}

/// [`lower_to_orb_with_map`] against an explicit [`LowerOptions`].
pub fn lower_to_orb_with_map_with(
    ast: &Program,
    options: &LowerOptions,
) -> Result<(OrbitalSchema, crate::source_map::LoloSourceMap), LowerError> {
    lower_to_orb_with_prior_and_map_with(ast, None, options)
}

/// [`lower_to_orb_with_prior`] additionally returning the L2 source map. The
/// map never enters the schema itself (the emitted `.orb` stays byte
/// identical) — it's a side channel built alongside the ordinary lowering.
/// Discovers installed behavior packages from the current directory.
pub fn lower_to_orb_with_prior_and_map(
    ast: &Program,
    prior: Option<&OrbitalSchema>,
) -> Result<(OrbitalSchema, crate::source_map::LoloSourceMap), LowerError> {
    let options = discovered_options()?;
    lower_to_orb_with_prior_and_map_with(ast, prior, &options)
}

/// [`lower_to_orb_with_prior_and_map`] against an explicit [`LowerOptions`].
/// The one internal entry point every public lowering function above funnels
/// into ([`lower_impl`]).
pub fn lower_to_orb_with_prior_and_map_with(
    ast: &Program,
    prior: Option<&OrbitalSchema>,
    options: &LowerOptions,
) -> Result<(OrbitalSchema, crate::source_map::LoloSourceMap), LowerError> {
    orbital_core_base::language_data::language_data().map_err(LowerError::LanguageData)?;
    let contracts = LoloContracts::load_shared(&options.packages).map_err(|e| match e {
        crate::registry::LoadError::StdNotInstalled => LowerError::BehaviorPackageNotInstalled {
            package: orbital_core_base::behavior_packages::STD_PACKAGE.to_string(),
        },
        other => LowerError::BehaviorPackages(other.to_string()),
    })?;
    lower_impl(ast, prior, &contracts)
}

/// Discover installed behavior packages from the current directory and the
/// user store (`orbital_core::behavior_packages::discover`), for the
/// zero-option entry points. A `DiscoverError` (e.g. a prefix conflict, or a
/// declared sidecar path that doesn't exist) is a hard `LowerError` — never
/// silently degraded to "no packages found".
pub(crate) fn discovered_options() -> Result<LowerOptions, LowerError> {
    let cwd = std::env::current_dir().map_err(|e| LowerError::BehaviorPackages(e.to_string()))?;
    let user_store =
        orbital_core_base::behavior_packages::user_store_dir().unwrap_or_else(|| cwd.clone());
    let packages = orbital_core_base::behavior_packages::discover(&cwd, &user_store)
        .map_err(|e| LowerError::BehaviorPackages(e.to_string()))?;
    Ok(LowerOptions {
        packages,
        ..Default::default()
    })
}

/// The one internal entry point every public `lower_to_orb*` function funnels
/// into: run the names-only lowering (which checks `contracts` mid-pass, see
/// [`checks::check_behavior_contracts`]) then stamp V4 identity.
fn lower_impl(
    ast: &Program,
    prior: Option<&OrbitalSchema>,
    contracts: &LoloContracts,
) -> Result<(OrbitalSchema, crate::source_map::LoloSourceMap), LowerError> {
    if ast.app.legacy_lines {
        return Err(crate::error::app_header_legacy_error().located(ast.app.span));
    }
    let mut source_map = crate::source_map::LoloSourceMap::new();
    let ast = &projection::resolve_field_projections(ast)?;
    let mut schema = lower_to_orb_names_only(ast, &mut source_map, contracts)?;
    orbital_core_eval::evaluator::bake_list_shapes(&mut schema)
        .expect("an orbital serializes to JSON and back");
    let synthetic_traits: HashSet<String> = ast
        .orbitals
        .iter()
        .flat_map(|o| o.traits.iter())
        .filter(|t| t.is_inline_synthetic)
        .map(|t| t.name.clone())
        .collect();
    orbital_core_base::stamp::stamp_schema_identity(&mut schema, prior, &synthetic_traits);
    for orbital in &mut schema.orbitals {
        for reference in &mut orbital.traits {
            if let orbital_core_base::schema::TraitReference::Inline(definition) = reference {
                orbital_core_base::schema::capture_trait_source_declaration(Arc::make_mut(definition))
                    .expect("a lowered trait serializes to JSON");
            }
        }
    }
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
    orbital_core_base::stamp::stamp_schema_identity(schema, prior, synthetic_traits);
}

/// The name-only lowering (pre-identity). Kept private; the public entry
/// points wrap it.
fn lower_to_orb_names_only(
    ast: &Program,
    source_map: &mut crate::source_map::LoloSourceMap,
    contracts: &LoloContracts,
) -> Result<OrbitalSchema, LowerError> {
    for u in ast
        .uses
        .iter()
        .chain(ast.orbitals.iter().flat_map(|o| o.uses.iter()))
    {
        source_map.insert_import(&u.path, u.span);
    }
    // Phase 0: run the type-system resolver. Any unknown type / cycle /
    // kind-mismatch / composition-conflict / annotation-mismatch is reported
    // as `LowerError::Type(..)` before lowering proceeds. Types are stripped
    // after this — the emitted `.orb` is byte-identical to a `.lolo` that
    // declared no types at all.
    check_program(ast).map_err(LowerError::Type)?;

    // Build a global map of type aliases (file-level) for enum substitution.
    let mut type_aliases: BTreeMap<String, TypeAliasResolved> = BTreeMap::new();
    for ta in &ast.type_aliases {
        type_aliases.insert(
            ta.name.clone(),
            resolve_type_alias(&ta.body, ta.type_param.as_ref()),
        );
    }
    reclassify_struct_unions(&mut type_aliases)?;

    // Entity-shaped type aliases retained in their full field form so the
    // emit-payload lowerer can flatten `:: TypeName` references. The regular
    // `type_aliases` table loses struct fields (collapsed to "object") because
    // those callers only need enum/primitive info; the typed-payload flattener
    // needs the original field list.
    let mut file_entity_bodies = collect_entity_type_bodies(&ast.type_aliases);
    file_entity_bodies.extend(crate::types::composed_entity_fields(&ast.type_aliases, &[]));
    let file_event_aliases = collect_event_type_bodies(&ast.type_aliases);

    // Phase 9.6.A: collect the set of names that designate ENTITIES (relation
    // targets) — distinct from type aliases that happen to be record-shaped.
    // Members: every orbital's primary `entity` block in the program, any
    // `expects entity`/`expects entity [identity]` name (supplied by a sibling at
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
        // `expects entity`/`expects entity [identity]` names are legal relation
        // targets too — the sibling orbital supplies the declaration at
        // compose time (the L2 relations validator licenses the same names).
        file_entity_names.extend(
            crate::types::expected_entity_names(orb)
                .into_iter()
                .map(str::to_string),
        );
        // An import's `entity X` rebind names the materialized primary entity.
        file_entity_names.extend(orb.entity_rebind.iter().cloned());
    }
    for ta in &ast.type_aliases {
        if matches!(ta.body, TypeBody::Entity { .. }) {
            file_entity_names.insert(ta.name.clone());
        }
    }

    let mut orbitals = Vec::with_capacity(ast.orbitals.len());
    let mut orbital_entity_names = Vec::with_capacity(ast.orbitals.len());
    for (orbital_index, orb) in ast.orbitals.iter().enumerate() {
        // Merge orbital-scoped type aliases on top of file-level ones.
        let mut local_aliases = type_aliases.clone();
        for ta in &orb.type_aliases {
            local_aliases.insert(
                ta.name.clone(),
                resolve_type_alias(&ta.body, ta.type_param.as_ref()),
            );
        }
        reclassify_struct_unions(&mut local_aliases)?;
        let mut local_entities = file_entity_bodies.clone();
        local_entities.extend(collect_entity_type_bodies(&orb.type_aliases));
        local_entities.extend(crate::types::composed_entity_fields(
            &ast.type_aliases,
            &orb.type_aliases,
        ));
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
        orbitals.push(
            lower_orbital(
                orb,
                &ast.uses,
                &local_aliases,
                &local_entities,
                &local_entity_names,
                &local_events,
                &file_shared_entity_names,
                source_map,
                &orbital_base,
            )
            .located(orb.span)?,
        );
        orbital_entity_names.push(local_entity_names.clone());
        if let Some(lowered) = orbitals.last_mut() {
            tree_fold::bake_tree_fold_definitions(
                lowered,
                &local_aliases,
                &local_entity_names,
                orb.span,
            )?;
        }
    }

    // App-level `config { ... }` (orbital-import Stage A, `OrbitalSchema.config`)
    // — ORGANISM-wide knobs, resolved against the file-level type/entity scope
    // (no trait type params at this level). Declaration site: the
    // self-reference guard applies.
    let app_config = lower_config_fields_to_map(
        &ast.app.config,
        &ast.app.name,
        true,
        &[],
        &type_aliases,
        &file_entity_names,
    )?;

    checks::type_config_wiring(&mut orbitals, app_config.as_ref())?;

    check_listens_integrity(&orbitals)?;
    check_button_transitions(&orbitals)?;
    check_emit_payload_literals(&orbitals, &orbital_entity_names)?;
    check_behavior_contracts(&orbitals, contracts)?;
    checks::type_call_site_message_keys(&mut orbitals, contracts);

    let mut schema = OrbitalSchema {
        type_contract_version: Some(1),
        resolved: None,        types: orbital_types(&type_aliases, &file_entity_bodies),
        store: ast.app.store.clone(),
        theme_definitions: None,
        themes: (!ast.app.themes.is_empty()).then(|| ast.app.themes.iter().map(lower_theme_decl).collect()),
        locales: (!ast.app.locales.is_empty()).then(|| ast.app.locales.clone()),
        messages: None,
        egress: (!ast.app.egress.is_empty()).then(|| ast.app.egress.clone()),
        inbound: (!ast.app.inbound.is_empty()).then(|| ast.app.inbound.clone()),
        public_dir: ast.app.public.clone(),
        os_grants: (!ast.app.os_grants.is_empty()).then(|| ast.app.os_grants.clone()),
        deployment: (!ast.app.deployment.is_empty())
            .then(|| ast.app.deployment.iter().cloned().collect()),
        site: (!ast.app.site.is_empty()).then(|| ast.app.site.clone()),
        lazy_pages: None,
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
        theme: ast.app.theme.as_ref().map(lower_theme_decl),
        orbitals,
        metadata: None,
        ledger: None,
        schema_version: None,
        config: app_config,
    };

    // Canonicalize flat `fn` lambdas to the grouped form so the emitted/baked
    // `.orb` carries one shape (see `orbital_core::schema::normalize_fn_shapes`).
    orbital_core_base::schema::normalize_fn_shapes(&mut schema);

    for orbital in &schema.orbitals {
        config_knob_read::check_orbital_config_knobs_read(orbital)?;
    }

    qualify_message_keys(schema)
}

/// Qualifies every `(i18n/t "key")` literal and every `MessageKey` knob literal with this program's name — see
/// `orbital_core::messages`.
fn qualify_message_keys(mut schema: OrbitalSchema) -> Result<OrbitalSchema, LowerError> {
    use orbital_core_base::messages::{qualify_config_message_keys, qualify_message_keys, MessageKeyIssue};
    let namespace = schema.name.clone();
    let knob_issues = qualify_config_message_keys(&mut schema, &namespace);
    let mut value = serde_json::to_value(&schema).map_err(|e| LowerError::MessageKeys {
        detail: e.to_string(),
    })?;
    if let Some(MessageKeyIssue::Invalid { path, key }) =
        knob_issues.into_iter().chain(qualify_message_keys(&mut value, &namespace)).next()
    {
        return Err(LowerError::MessageKeyInvalid { path, key });
    }
    serde_json::from_value(value).map_err(|e| LowerError::MessageKeys {
        detail: e.to_string(),
    })
}

/// `ThemeDecl` → `ThemeRef`: a quoted key stays a reference; an inline
/// `theme { … }` block becomes the schema's `ThemeDefinition`.
fn lower_theme_decl(decl: &ThemeDecl) -> ThemeRef {
    match decl {
        ThemeDecl::Reference(key) => ThemeRef::Reference(key.clone()),
        ThemeDecl::Inline(inline) => ThemeRef::Inline(ThemeDefinition {
            name: inline.name.clone(),
            display_name: inline.display_name.clone(),
            tokens: lower_theme_tokens(&inline.tokens),
            variants: if inline.variants.is_empty() {
                None
            } else {
                Some(
                    inline
                        .variants
                        .iter()
                        .map(|(name, maps)| (name.clone(), lower_theme_variant(maps)))
                        .collect(),
                )
            },
        }),
    }
}

fn theme_map(entries: &[(String, String)]) -> Option<orbital_core_base::schema::ThemeMap> {
    if entries.is_empty() {
        None
    } else {
        Some(entries.iter().cloned().collect())
    }
}

/// A typed-axis category's flat `key: "value"` entries → the typed struct.
/// Keys were checked against `THEME_TYPED_AXIS_KEYS` at parse time, so every
/// entry maps to a `String` field and deserialization cannot fail.
fn typed_theme_axis<T: serde::de::DeserializeOwned>(entries: &[(String, String)]) -> Option<T> {
    if entries.is_empty() {
        return None;
    }
    let obj: serde_json::Map<String, serde_json::Value> = entries
        .iter()
        .map(|(k, v)| (k.clone(), serde_json::Value::String(v.clone())))
        .collect();
    serde_json::from_value(serde_json::Value::Object(obj)).ok()
}

/// The `.lolo` `motion { … }` category carries only motion-shape keys; they
/// become `MotionTokens.shapes` (durations/easings/intents stay `.orb`-only).
fn motion_shapes(entries: &[(String, String)]) -> Option<MotionTokens> {
    typed_theme_axis::<MotionShapeTokens>(entries).map(|shapes| MotionTokens {
        durations: None,
        easings: None,
        intents: None,
        shapes: Some(shapes),
        entry: None,
        busy_delay: None,
        awaiting_skeleton: None,
    })
}

fn lower_theme_tokens(maps: &ThemeTokenMaps) -> ThemeTokens {
    ThemeTokens {
        colors: theme_map(&maps.colors),
        radii: theme_map(&maps.radii),
        spacing: theme_map(&maps.spacing),
        typography: theme_map(&maps.typography),
        shadows: theme_map(&maps.shadows),
        density: None,
        type_scale: typed_theme_axis(&maps.type_scale),
        motion: motion_shapes(&maps.motion),
        iconography: None,
        elevation: typed_theme_axis(&maps.elevation),
        geometry: typed_theme_axis(&maps.geometry),
        // Typed color/illustration axis (§157/C1): not yet authored from `.lolo`
        // theme blocks, same as every other still-unauthored knob above.
        color: None,
        illustration: None,
        diagram: typed_theme_axis(&maps.diagram),
        scene: typed_theme_axis(&maps.scene),
        surface: typed_theme_axis(&maps.surface),
    }
}

fn lower_theme_variant(maps: &ThemeTokenMaps) -> ThemeVariant {
    ThemeVariant {
        colors: theme_map(&maps.colors),
        radii: theme_map(&maps.radii),
        spacing: theme_map(&maps.spacing),
        typography: theme_map(&maps.typography),
        shadows: theme_map(&maps.shadows),
        density: None,
        type_scale: typed_theme_axis(&maps.type_scale),
        motion: motion_shapes(&maps.motion),
        iconography: None,
        elevation: typed_theme_axis(&maps.elevation),
        geometry: typed_theme_axis(&maps.geometry),
        color: None,
        illustration: None,
        diagram: typed_theme_axis(&maps.diagram),
        scene: typed_theme_axis(&maps.scene),
        surface: typed_theme_axis(&maps.surface),
    }
}

#[derive(Debug, Clone)]
pub(crate) enum TypeAliasResolved {
    /// `type X = a | b | c` lowers to a string field with values [...].
    Enum(Vec<String>),
    /// Other type aliases are passthrough — we record the resolved type
    /// expression and re-evaluate it on demand, with the alias's own type
    /// parameter (`T` in `type Box T = { v : T }`), which is in scope in it.
    Type(TypeExpr, Option<String>),
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

/// Reclassify `type X = A | B` from a string ENUM to a struct UNION when every
/// member names a struct alias.
///
/// The parser cannot tell these apart: `A | B` with all-bare-name members is
/// `TypeBody::Enum(["A","B"])`, indistinguishable at parse time from
/// `active | inactive`. The difference only becomes visible once every alias in
/// scope is known — hence a post-pass over the assembled map rather than a
/// change in `resolve_type_alias`, which sees one body at a time.
///
/// Without this, `DrawShape | DrawText` declares a field whose only legal
/// values are the literal WORDS "DrawShape"/"DrawText" — it accepts the
/// variant's name and rejects the variant's value.
fn reclassify_struct_unions(
    aliases: &mut BTreeMap<String, TypeAliasResolved>,
) -> Result<(), LowerError> {
    let mut rewrites: Vec<(String, TypeExpr)> = Vec::new();
    for (name, resolved) in aliases.iter() {
        let TypeAliasResolved::Enum(members) = resolved else {
            continue;
        };
        let parts: Vec<TypeExpr> = members.iter().map(|m| TypeExpr::Named(m.clone())).collect();
        if !crate::lower::entity::is_type_union(&parts, aliases) {
            let (types, words): (Vec<&String>, Vec<&String>) = members.iter().partition(|m| {
                matches!(aliases.get(m.as_str()), Some(TypeAliasResolved::Type(..)))
                    || orbital_core_base::language::program_types::program_type_field(m).is_some()
            });
            if !types.is_empty() && !words.is_empty() {
                let join = |v: Vec<&String>| v.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ");
                return Err(LowerError::Type(crate::error::TypeError::AmbiguousUnion {
                    alias: name.clone(),
                    types: join(types),
                    words: join(words),
                    span: crate::ast::Span::default(),
                }));
            }
            continue;
        }
        rewrites.push((name.clone(), TypeExpr::Union(parts)));
    }
    for (name, ty) in rewrites {
        aliases.insert(name, TypeAliasResolved::Type(ty, None));
    }
    Ok(())
}

fn resolve_type_alias(body: &TypeBody, param: Option<&String>) -> TypeAliasResolved {
    match body {
        TypeBody::Enum(v) => TypeAliasResolved::Enum(v.clone()),
        TypeBody::Alias(t) => TypeAliasResolved::Type(t.clone(), param.cloned()),
        // Preserve struct field shape by re-projecting onto TypeExpr::Object
        // — `TypeExpr::Object` and `TypeBody::Struct` share `Vec<(String, TypeExpr)>`,
        // so referenced struct aliases lower to a structured object instead of
        // a flat `object` primitive (which previously collapsed to `Json`).
        TypeBody::Struct(items) => {
            TypeAliasResolved::Type(TypeExpr::Object(items.clone()), param.cloned())
        }
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
            TypeAliasResolved::Type(TypeExpr::Primitive("object".to_string()), None)
        }
    }
}

pub(crate) fn lower_expects_decl(
    decl: &ExpectsDecl,
    strict: bool,
    aliases: &BTreeMap<String, TypeAliasResolved>,
    entities: &BTreeMap<String, Vec<(String, TypeExpr, bool)>>,
    entity_names: &HashSet<String>,
) -> Result<ExpectDeclaration, LowerError> {
    let lower_shape = |shape: &Option<Vec<EntityField>>| {
        shape
            .as_ref()
            .map(|fields| {
                fields
                    .iter()
                    .map(|f| lower_field(f, aliases, entity_names, &[]))
                    .collect::<Result<Vec<_>, _>>()
            })
            .transpose()
    };
    Ok(match &decl.kind {
        ExpectsKind::Identity { name, shape } => ExpectDeclaration::Identity {
            name: name.clone(),
            shape: lower_shape(shape)?,
            strict,
        },
        ExpectsKind::Entity {
            name,
            shape,
            persistence,
            shared,
            browser,
            own,
            access,
            invariants,
        } => ExpectDeclaration::Entity {
            name: name.clone(),
            shape: lower_shape(shape)?,
            persistence: persistence.as_ref().map(|p| match p {
                Persistence::Persistent => CorePersistence::Persistent,
                Persistence::Runtime => CorePersistence::Runtime,
            }),
            shared: *shared,
            browser: *browser,
            own: *own,
            read_policy: access.read_policy.clone(),
            create_policy: access.create_policy.clone(),
            update_policy: access.update_policy.clone(),
            delete_policy: access.delete_policy.clone(),
            access_waivers: entity::access_waivers_of(&access.access_waivers),
            invariants: (!invariants.is_empty()).then(|| invariants.clone()),
            strict,
        },
        // Trait and event names pass through verbatim — no case-folding.
        ExpectsKind::Event {
            trait_name,
            event,
            payload,
        } => {
            let payload = if payload.is_empty() {
                None
            } else {
                let emit = Emit {
                    event: event.clone(),
                    scope: None,
                    payload: payload.clone(),
                    description: None,
                    synonyms: None,
                    tier: None,
                    span: decl.span,
                };
                emit_listen::lower_emit(&emit, aliases, entities, &BTreeMap::new(), &[])
                    .located(decl.span)?
                    .payload_schema
            };
            ExpectDeclaration::Event {
                trait_name: trait_name.clone(),
                event: event.clone(),
                payload,
                strict,
            }
        }
        // A route pattern is a path, not an identifier — verbatim likewise.
        ExpectsKind::Page {
            path,
            protocol,
            traits,
        } => ExpectDeclaration::Page {
            path: path.clone(),
            protocol: match protocol {
                Some(word) => {
                    page::check_protocol_word(word)?;
                    Some(word.clone())
                }
                None => None,
            },
            traits: (!traits.is_empty()).then(|| traits.clone()),
            strict,
        },
        ExpectsKind::State {
            trait_name,
            state,
            holds,
        } => ExpectDeclaration::State {
            trait_name: trait_name.clone(),
            state: state.clone(),
            holds: holds.clone(),
            strict,
        },
        ExpectsKind::Transition {
            trait_name,
            from,
            event,
            to,
            ensures,
        } => ExpectDeclaration::Transition {
            trait_name: trait_name.clone(),
            from: from.clone(),
            event: event.clone(),
            to: to.clone(),
            ensures: (!ensures.is_empty()).then(|| ensures.clone()),
            strict,
        },
        ExpectsKind::Config { field } => {
            let mut lowered = lower_config_fields_to_map(
                std::slice::from_ref(field),
                "expects config",
                false,
                &[],
                aliases,
                entity_names,
            )?
            .unwrap_or_default();
            let knob = lowered.shift_remove(&field.name).ok_or_else(|| {
                LowerError::Internal(format!(
                    "expects config `{}` lowered to no knob",
                    field.name
                ))
            })?;
            ExpectDeclaration::Config {
                name: field.name.clone(),
                knob,
                strict,
            }
        }
    })
}

/// An orbital of only `expects` lines: a spec. It owns no entity, trait or page, so it can never
/// provide what it asks for; every declaration it lowers is a strict obligation.
pub(crate) fn is_spec_orbital(orb: &Orbital) -> bool {
    orb.reference.is_none()
        && orb.entity.is_none()
        && orb.extra_entities.is_empty()
        && orb.traits.is_empty()
        && orb.pages.is_empty()
        && !orb.expects.is_empty()
}

fn lower_spec_orbital(
    orb: &Orbital,
    file_uses: &[UseDecl],
    aliases: &BTreeMap<String, TypeAliasResolved>,
    entities: &BTreeMap<String, Vec<(String, TypeExpr, bool)>>,
    entity_names: &HashSet<String>,
) -> Result<CoreOrbital, LowerError> {
    let expects = orb
        .expects
        .iter()
        .map(|e| lower_expects_decl(e, true, aliases, entities, entity_names))
        .collect::<Result<Vec<_>, _>>()?;
    let merged_uses = merge_uses(file_uses, &orb.uses, aliases, entity_names)?;
    Ok(CoreOrbital {
        name: orb.name.clone(),
        uses: (!merged_uses.is_empty()).then_some(merged_uses),
        expects: Some(expects),
        spec: true,
        theme: None,
        services: None,
        entity: EntityRef::Inline(spec_placeholder_entity(&orb.name)),
        auxiliary_entities: None,
        traits: Vec::new(),
        pages: None,
        emits: None,
        listens: None,
        exposes: None,
        domain_context: None,
        os_grants: None,
        design: None,
        id: None,
        types: orbital_types(aliases, entities),
        reference: None,
        config: None,
    })
}

fn vocabulary_values(te: &TypeExpr) -> Option<Vec<String>> {
    match te {
        TypeExpr::Union(parts) => parts
            .iter()
            .map(|p| match p {
                TypeExpr::Named(n) | TypeExpr::Primitive(n) | TypeExpr::StringLiteral(n) => {
                    Some(n.clone())
                }
                _ => None,
            })
            .collect(),
        _ => None,
    }
}

fn vocabulary_entry(values: Vec<String>) -> PayloadField {
    PayloadField {
        nullable: Some(false),
        additional_properties: None,
        values: Some(values),
        name: String::new(),
        field_type: "string".to_string(),
        required: None,
        description: None,
        properties: None,
        type_when: None,
        entity: None,
        projected_from: None,
        items: None,
        relation: None,
    }
}

fn orbital_types(
    aliases: &BTreeMap<String, TypeAliasResolved>,
    shapes: &BTreeMap<String, Vec<(String, TypeExpr, bool)>>,
) -> Option<BTreeMap<String, Vec<PayloadField>>> {
    // Gap A: carry every struct-shaped alias visible in this orbital's scope
    // (file-level + orbital-local, already merged into `aliases`) so a `::`
    // type-param arg naming a local struct alias survives L1 — the `.orb`
    // otherwise has no record of type aliases at all. Enum/primitive/entity
    // aliases are not legal `::` args and are excluded.
    let mut types_map: BTreeMap<String, Vec<PayloadField>> = BTreeMap::new();
    for (name, resolved) in aliases {
        // A shape alias (`Entity { .. }`, `A & Entity { .. }`) resolves to the opaque `object` primitive; its
        // exported entry is the merged field list, and an alias with no field list (trait/page/orbital/event
        // shapes) exports nothing rather than a bare `object`.
        if matches!(resolved, TypeAliasResolved::Type(TypeExpr::Primitive(kind), _) if kind == "object") {
            if let Some(fields) = shapes.get(name) {
                if let Some(props) =
                    struct_payload_properties(&TypeExpr::Object(fields.clone()), aliases, &[])
                {
                    types_map.insert(name.clone(), props);
                }
            }
            continue;
        }
        match resolved {
            TypeAliasResolved::Type(
                te @ (TypeExpr::OpenObject { .. }
                | TypeExpr::Optional(_)
                | TypeExpr::FieldOf { .. }),
                _,
            ) => {
                types_map.insert(
                    name.clone(),
                    vec![emit_listen::source_payload_field("", te, aliases, &[])],
                );
            }
            TypeAliasResolved::Type(TypeExpr::Primitive(kind), _) => {
                types_map.insert(
                    name.clone(),
                    vec![emit_listen::source_payload_field(
                        "",
                        &TypeExpr::Primitive(kind.clone()),
                        aliases,
                        &[],
                    )],
                );
            }
            TypeAliasResolved::Enum(values) => {
                types_map.insert(name.clone(), vec![vocabulary_entry(values.clone())]);
            }
            TypeAliasResolved::Type(TypeExpr::Named(target), None) => {
                types_map.insert(
                    name.clone(),
                    vec![PayloadField {
                        nullable: Some(false),
                        additional_properties: None,
                        values: None,
                        name: String::new(),
                        field_type: target.clone(),
                        required: None,
                        description: None,
                        properties: None,
                        type_when: None,
                        entity: None,
                        projected_from: None,
                        items: None,
                        relation: None,
                    }],
                );
            }
            TypeAliasResolved::Type(te @ TypeExpr::Object(_), _) => {
                if let Some(props) = struct_payload_properties(te, aliases, &[]) {
                    types_map.insert(name.clone(), props);
                }
            }
            // A dictionary or list alias is not a struct: it is recorded as ONE entry with the empty name (no field
            // has one) whose type string is the alias's value, so `json/parse x Row` can name it as a shape.
            TypeAliasResolved::Type(
                te
                @ (TypeExpr::Map(..) | TypeExpr::Array(_) | TypeExpr::Tuple(_) | TypeExpr::SExpr),
                _,
            ) => {
                if let Some(tuple) = emit_listen::tuple_payload_field(te, aliases, &[]) {
                    types_map.insert(name.clone(), vec![tuple]);
                } else {
                    types_map.insert(
                        name.clone(),
                        vec![emit_listen::source_payload_field("", te, aliases, &[])],
                    );
                }
            }
            // A tagged union is recorded as ONE entry with the empty name, typed `union`, whose properties are its
            // variants: a payload field named by the alias resolves to that union and its members can be read.
            TypeAliasResolved::Type(te @ TypeExpr::Union(_), _) => {
                if let Some(variants) =
                    crate::lower::emit_listen::union_payload_variants(te, aliases, &[])
                {
                    types_map.insert(
                        name.clone(),
                        vec![PayloadField {
                            nullable: Some(false),
                            additional_properties: None,
                            values: None,
                            name: String::new(),
                            field_type: "union".to_string(),
                            required: None,
                            description: None,
                            properties: Some(variants),
                            type_when: None,
                            entity: None,
                            projected_from: None,
                            items: None,
                            relation: None,
                        }],
                    );
                } else if let Some(values) = vocabulary_values(te) {
                    types_map.insert(name.clone(), vec![vocabulary_entry(values)]);
                }
            }
            // A closed vocabulary (`"list" | "grid"`, `"x"`) is ONE empty-name `string` entry carrying its words.
            TypeAliasResolved::Type(TypeExpr::StringLiteral(word), _) => {
                types_map.insert(name.clone(), vec![vocabulary_entry(vec![word.clone()])]);
            }
            _ => {}
        }
    }
    if types_map.is_empty() {
        None
    } else {
        Some(types_map)
    }
}

/// The IR requires an entity on every orbital; a spec's is an empty, field-less shell named after
/// the spec, never matched by a real expectation.
fn spec_placeholder_entity(name: &str) -> EntityDefinition {
    EntityDefinition {
        timestamps: None,
        soft_delete: None,
        description: None,
        visual_prompt: None,
        asset_ref: None,
        store: None,
        shared: false,
        identity: false,
        browser: false,
        locale_instances: None,
        read_policy: None,
        create_policy: None,
        update_policy: None,
        delete_policy: None,
        access_waivers: None,
        name: name.to_string(),
        collection: None,
        persistence: None,
        fields: Vec::new(),
        instances: None,
        id: None,
        invariants: None,
        indexes: None,
    }
}

/// Merge file-scoped `uses` (declared above any `orbital` block,
/// orbital-import Stage A) with one orbital's own `uses` into a single
/// lowered `UseDeclaration` list: file-level entries first, orbital-local
/// ones appended, with the LOCAL alias winning any collision — the
/// file-level entry of the same alias is dropped, never both. This is what
/// lets a top-level `orbital X = Alias.orbitals.Y` reference resolve `Alias`
/// through the existing per-orbital `ctx` loop with zero change to it: every
/// orbital, reference or inline, sees ONE flat `uses` list regardless of
/// where each entry was declared.
pub(crate) fn merge_uses(
    file_uses: &[UseDecl],
    orbital_uses: &[UseDecl],
    aliases: &BTreeMap<String, TypeAliasResolved>,
    entity_names: &HashSet<String>,
) -> Result<Vec<UseDeclaration>, LowerError> {
    let local_aliases: HashSet<&str> = orbital_uses.iter().map(|u| u.alias.as_str()).collect();
    file_uses
        .iter()
        .filter(|u| !local_aliases.contains(u.alias.as_str()))
        .chain(orbital_uses.iter())
        .map(|u| {
            Ok(UseDeclaration {
                lazy: u.lazy.then_some(true),
                from: u.path.clone(),
                alias: u.alias.clone(),
                // Override site, not a declaration — `@config.<key>`
                // legitimately forwards the ENCLOSING orbital's own knob of
                // the same name into the imported organism, so the
                // self-reference guard does not apply here.
                config: lower_config_fields_to_map(
                    &u.config,
                    &u.alias,
                    false,
                    &[],
                    aliases,
                    entity_names,
                )?,
            })
        })
        .collect()
}

fn lower_orbital(
    orb: &Orbital,
    file_uses: &[UseDecl],
    aliases: &BTreeMap<String, TypeAliasResolved>,
    entities: &BTreeMap<String, Vec<(String, TypeExpr, bool)>>,
    entity_names: &HashSet<String>,
    event_aliases: &BTreeMap<String, EventTypeResolved>,
    shared_entity_names: &HashSet<String>,
    source_map: &mut crate::source_map::LoloSourceMap,
    base: &str,
) -> Result<CoreOrbital, LowerError> {
    if orb.reference.is_some() {
        return orbital_ref::lower_orbital_reference(
            orb,
            file_uses,
            aliases,
            entity_names,
            entities,
            event_aliases,
            shared_entity_names,
            source_map,
            base,
        );
    }
    source_map.insert(base.to_string(), orb.span);
    if is_spec_orbital(orb) {
        return lower_spec_orbital(orb, file_uses, aliases, entities, entity_names);
    }
    let entity = orb.entity.as_ref().ok_or(LowerError::MissingEntity)?;
    source_map.insert(format!("{base}.entity"), entity.span);
    for (j, f) in entity.fields.iter().enumerate() {
        source_map.insert(format!("{base}.entity.fields[{j}]"), f.span);
    }
    let entity_def = lower_entity(entity, aliases, entity_names).located(entity.span)?;
    // Entity declarations beyond the first lower into `auxiliary_entities` —
    // the IR slot the validators, binding checks, and mock tier already
    // accept (Gap #22) — so a multi-collection orbital (e.g. a dashboard
    // reading two collections) is expressible.
    let extra_entity_defs: Vec<EntityDefinition> = orb
        .extra_entities
        .iter()
        .map(|e| lower_entity(e, aliases, entity_names).located(e.span))
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

    let merged_uses = merge_uses(file_uses, &orb.uses, aliases, entity_names)?;
    let uses = if merged_uses.is_empty() {
        None
    } else {
        Some(merged_uses)
    };

    let expects = if orb.expects.is_empty() {
        None
    } else {
        orb.expects
            .iter()
            .map(|e| lower_expects_decl(e, false, aliases, entities, entity_names))
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
        traits.push(
            lower_trait_decl(
                t,
                aliases,
                entities,
                entity_names,
                event_aliases,
                intrinsic_fields,
                shared_entity_names,
                source_map,
                &trait_base,
            )
            .located(t.span)?,
        );
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
        let synth = TraitDefinition { dropped_listens: None,
            name: "__Composition".to_string(),
            entity_rebindable: None,
            entity_contract: None,
            // Synthetic carrier for `behavior/*` composition directives, not a
            // real effect surface — downstream passes ignore this trait.
            effect_row: None,
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
                    holds: None,
                }],
                events: Some(vec![EventDefinition {
                    key: "INIT".to_string(),
                    id: None,
                    name: None,
                    description: None,
                    synonyms: None,
                    tier: None,
                    payload_schema: None,
                    payload_entity: None,
                    input: None,
                    listened: None,
                    inbound: None,
                }]),
                transitions: vec![TransitionDefinition {
                    from: StringOrArray::Single("init".to_string()),
                    to: "init".to_string(),
                    event: "INIT".to_string(),
                    event_id: None,
                    guard: None,
                    effects: Some(orb.body.clone()),
                    ensures: None,
                    await_render: None,
                }],
            }
            .into(),
            ticks: None,
            config: None,
            data_entities: None,
            dispatch: None,
            source_declaration: None,
            source_behavior: None,
            source_entity_definition: None,
            id: None,
            linked_entity_id: None,
            entity_ref_ids: None,
            trait_embed_ids: None,
            type_params: None,
            type_args: None,
        };
        traits.push(TraitReference::Inline(Arc::new(synth)));
    }

    let pages = if orb.pages.is_empty() {
        None
    } else {
        Some(
            orb.pages
                .iter()
                .map(|p| lower_page_decl(p).located(p.span))
                .collect::<Result<Vec<_>, LowerError>>()?,
        )
    };

    let types = orbital_types(aliases, entities);

    Ok(CoreOrbital {
        name: orb.name.clone(),
        uses,
        expects,
        spec: false,
        theme: orb.theme.as_ref().map(lower_theme_decl),
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
        os_grants: (!orb.os_grants.is_empty()).then(|| orb.os_grants.clone()),
        design: None,
        id: None,
        types,
        reference: None,
        config: lower_config_fields_to_map(
            &orb.config,
            &orb.name,
            true, // declaration site — the self-reference guard applies.
            &[],
            aliases,
            entity_names,
        )?,
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
        let src = "app MyApp [version: \"1.0.0\", description: \"Desc\", theme: \"linear-clean-light\"]\norbital Foo {\n  entity Bar {\n    id : string!\n  }\n}";
        let s = lower(src);
        match &s.theme {
            Some(orbital_core_base::schema::ThemeRef::Reference(k)) => {
                assert_eq!(k, "linear-clean-light")
            }
            other => panic!("expected app theme reference, got {:?}", other),
        }
        // Formatter must re-emit the header theme modifier or `orb fmt` eats it.
        let ast = parse_str(src).expect("parse");
        let formatted = crate::format::format_program(&ast);
        assert!(
            formatted.contains("theme: \"linear-clean-light\""),
            "formatted output lost the app theme modifier:\n{formatted}"
        );
        let reparsed = parse_str(&formatted).expect("reparse formatted");
        assert_eq!(
            reparsed.app.theme,
            Some(crate::ast::ThemeDecl::Reference(
                "linear-clean-light".to_string()
            ))
        );
    }

    #[test]
    fn lower_inline_theme_block() {
        let src = "orbital Foo {\n  entity Bar {\n    id : string!\n  }\n  theme {\n    name: \"riya-graph-paper\"\n    tokens {\n      colors {\n        primary: \"#e11d48\"\n      }\n    }\n    variants {\n      dark {\n        colors {\n          primary: \"#fb7185\"\n        }\n      }\n    }\n  }\n}";
        let s = lower(src);
        let theme = s.orbitals[0].theme.as_ref().expect("orbital theme lowered");
        match theme {
            orbital_core_base::schema::ThemeRef::Inline(def) => {
                assert_eq!(def.name, "riya-graph-paper");
                assert_eq!(
                    def.tokens.colors.as_ref().unwrap().get("primary"),
                    Some(&"#e11d48".to_string())
                );
                let dark = def.variants.as_ref().unwrap().get("dark").unwrap();
                assert_eq!(
                    dark.colors.as_ref().unwrap().get("primary"),
                    Some(&"#fb7185".to_string())
                );
            }
            other => panic!("expected inline theme, got {:?}", other),
        }
        // Round-trip: the formatter must re-emit the block and it must reparse.
        let ast = parse_str(src).expect("parse");
        let formatted = crate::format::format_program(&ast);
        let reparsed = parse_str(&formatted).unwrap_or_else(|e| {
            panic!("formatted inline theme failed to reparse: {e}\n{formatted}")
        });
        assert_eq!(reparsed.orbitals[0].theme, ast.orbitals[0].theme);
    }

    fn inline_theme(tokens: &str) -> String {
        format!("orbital Foo {{\n  entity Bar {{\n    id : string!\n  }}\n  theme {{\n    name: \"chalk\"\n    tokens {{\n{tokens}\n    }}\n  }}\n}}")
    }

    #[test]
    fn lower_inline_theme_drawing_axes() {
        let src = inline_theme(
            "      diagram {\n        strokeNormal: \"3px\"\n        lineCap: \"round\"\n        fillStyle: \"hatch\"\n      }\n      scene {\n        materialFlat: \"1\"\n      }\n      surface {\n        diagram: \"#1f3a2e\"\n      }",
        );
        let s = lower(&src);
        let orbital_core_base::schema::ThemeRef::Inline(def) =
            s.orbitals[0].theme.as_ref().unwrap()
        else {
            panic!("expected inline theme");
        };
        let diagram = def.tokens.diagram.as_ref().expect("diagram axis lowered");
        assert_eq!(diagram.stroke_normal.as_deref(), Some("3px"));
        assert_eq!(
            diagram.line_cap,
            Some(orbital_core_base::schema::DiagramLineCap::Round)
        );
        assert_eq!(
            diagram.fill_style,
            Some(orbital_core_base::schema::DiagramFillStyle::Hatch)
        );
        assert_eq!(
            def.tokens.scene.as_ref().unwrap().material_flat.as_deref(),
            Some("1")
        );
        assert_eq!(
            def.tokens.surface.as_ref().unwrap().diagram.as_deref(),
            Some("#1f3a2e")
        );
        let ast = parse_str(&src).expect("parse");
        let formatted = crate::format::format_program(&ast);
        let reparsed =
            parse_str(&formatted).unwrap_or_else(|e| panic!("reparse: {e}\n{formatted}"));
        assert_eq!(reparsed.orbitals[0].theme, ast.orbitals[0].theme);
    }

    #[test]
    fn inline_theme_rejects_an_unknown_diagram_value() {
        let err = parse_str(&inline_theme(
            "      diagram {\n        lineCap: \"wavy\"\n      }",
        ))
        .unwrap_err();
        assert!(err.to_string().contains("butt, round, square"), "{err}");
    }

    #[test]
    fn inline_theme_rejects_an_unknown_diagram_key() {
        assert!(parse_str(&inline_theme(
            "      diagram {\n        stroke: \"2px\"\n      }"
        ))
        .is_err());
    }

    #[test]
    fn control_inline_theme_without_drawing_axes_lowers_none() {
        let s = lower(&inline_theme(
            "      colors {\n        primary: \"#000\"\n      }",
        ));
        let orbital_core_base::schema::ThemeRef::Inline(def) =
            s.orbitals[0].theme.as_ref().unwrap()
        else {
            panic!("expected inline theme");
        };
        assert!(def.tokens.diagram.is_none() && def.tokens.scene.is_none());
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
    fn lower_nullable_field_is_its_inner_type_and_not_required() {
        let ent = lowered_entity("orbital Foo {\n  entity Bar {\n    id : string!\n    due : number?\n    kept : number!\n  }\n}");
        let field = |name: &str| {
            ent.fields
                .iter()
                .find(|f| f.name.as_deref() == Some(name))
                .unwrap()
        };
        assert_eq!(
            field("due").field_type,
            orbital_core_base::schema::FieldType::Number
        );
        assert_ne!(field("due").required, Some(true));
        assert_eq!(
            field("kept").required,
            Some(true),
            "control: `!` still requires"
        );
    }

    const SEALED_FIELDS: &str = "orbital Foo { entity Bar [KIND] { id : string!\n arena : Arena?\n v : Verified\n } }";

    fn lower_err(src: &str) -> String {
        let ast = parse_str(src).expect("parse");
        lower_to_orb(&ast).expect_err("lowering is refused").to_string()
    }

    #[test]
    fn a_sealed_type_names_a_runtime_entity_field() {
        let ent = lowered_entity(&SEALED_FIELDS.replace("KIND", "runtime"));
        let field = |name: &str| ent.fields.iter().find(|f| f.name.as_deref() == Some(name)).unwrap();
        assert_eq!(field("arena").by_name_reference(), Some("Arena"));
        assert_eq!(field("v").by_name_reference(), Some("Verified"));
        assert_eq!(
            orbital_core_base::ir::types::field_shape(field("v")).unwrap(),
            orbital_core_base::sealed::sealed_type_ref("Verified")
        );
    }

    #[test]
    fn a_sealed_type_on_a_persistent_entity_is_refused() {
        let err = lower_err(&SEALED_FIELDS.replace("KIND", "persistent"));
        assert!(err.contains("ORB_L_SEALED_FIELD_NOT_RUNTIME") && err.contains("'arena'"), "got {err}");
    }

    #[test]
    fn control_a_misspelt_sealed_type_is_an_unknown_type() {
        let err = lower_err("orbital Foo { entity Bar [runtime] { id : string!\n arena : Arenaa?\n } }");
        assert!(err.contains("Arenaa"), "got {err}");
        assert!(!err.contains("SEALED"), "got {err}");
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
    fn lower_shared_alone_without_persistence_errors() {
        // Owner ruling: nothing about `[shared]` may be inferred — a bare
        // `[shared]` used to silently default to persistence None (persistent
        // downstream); it must now name a persistence mode explicitly.
        let ast = parse_str("orbital Foo { entity Bar [shared] { id : string! } }").expect("parse");
        let result = lower_to_orb(&ast);
        match result.map_err(|e| e.innermost().clone()) {
            Err(LowerError::SharedEntityWithoutPersistenceMode { entity }) => {
                assert_eq!(entity, "Bar");
            }
            other => panic!(
                "expected SharedEntityWithoutPersistenceMode for bare [shared], got: {:?}",
                other
            ),
        }
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
            "orbital O { entity E { id : string! } trait T -> E { state s { EVT -> s (set @id 1) } } }",
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
        let bound = t
            .ticks
            .iter()
            .flatten()
            .find(|tk| tk.after_entry_ms.is_some())
            .expect("a `for Ns` state lowers to a timeout tick (EXEC-TICK-8)");
        assert_eq!(bound.after_entry_ms, Some(30_000));
        assert_eq!(bound.applies_to.as_deref(), Some(&["s".to_string()][..]));
        assert_eq!(serde_json::to_value(&bound.effects).unwrap(), serde_json::json!([["emit", "TIMEOUT"]]));
    }

    #[test]
    fn control_a_state_without_a_bound_lowers_no_timeout_tick() {
        let s = lower("orbital O { entity E { id : string! } trait T -> E { state s { EVT -> s } } }");
        let TraitReference::Inline(t) = &s.orbitals[0].traits[0] else { panic!() };
        assert!(t.ticks.iter().flatten().all(|tk| tk.after_entry_ms.is_none()));
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
    fn a_union_of_type_names_that_are_not_records_is_a_type_union() {
        let field = |src: &str| {
            let s = lower(src);
            let EntityRef::Inline(ent) = &s.orbitals[0].entity else {
                panic!()
            };
            ent.fields[1].clone()
        };
        for src in [
            "type Pairs = Map string string\ntype Who = Viewer | Pairs\norbital O { entity E { id : string!\n who : Who } }",
            "type Pairs = Map string string\norbital O { entity E { id : string!\n who : { at : Viewer | Pairs } } }",
            "orbital O { entity E { id : string!\n who : Map string string? | Viewer } }",
        ] {
            let who = field(src);
            let at = if who.field_type == FieldType::Object {
                who.properties.as_ref().and_then(|p| p.values().next()).cloned().unwrap()
            } else {
                who
            };
            assert_eq!(at.field_type, FieldType::Union, "{src}: {at:?}");
            assert_eq!(at.properties.as_ref().map(|p| p.len()), Some(2), "{src}: {at:?}");
        }
        let mixed = crate::parse_and_lower_str(
            "type Pairs = Map string string\ntype Who = Pairs | nobody\norbital O { entity E { id : string!\n who : Who } }",
        );
        assert!(
            matches!(&mixed, Err(e) if e.to_string().contains("mixes type names")),
            "a union of a type name and a bare word is refused: {mixed:?}"
        );
        let control = field("type Who = viewer | pairs\norbital O { entity E { id : string!\n who : Who } }");
        assert_eq!(control.field_type, FieldType::String, "control: bare words are an enum");
        assert_eq!(control.values, Some(vec!["viewer".to_string(), "pairs".to_string()]));
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
                trait T -> E {
                    state s {
                        INIT -> s
                            (render-ui main { type: \"pagination\", currentPage: @entity.currentPage, totalPages: 1 })
                    }
                }
            }
        ";
        let ast = parse_str(src).expect("parse");
        let result = lower_to_orb(&ast);
        match result.map_err(|e| e.innermost().clone()) {
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
                entity E [persistent, shared] {
                    id : string!
                    count : number = 0
                }
                trait Reader -> E {
                    state s {
                        INIT -> s
                            (render-ui main { type: \"text\", value: @entity.count })
                    }
                }
                trait Writer -> E {
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
                trait Reader -> E {
                    state s {
                        INIT -> s
                            (render-ui main { type: \"text\", value: @entity.count })
                    }
                }
                trait Writer -> E {
                    state s {
                        INCREMENT -> s
                            (set @entity.count 1)
                    }
                }
            }
        ";
        let ast = parse_str(src).expect("parse");
        let result = lower_to_orb(&ast);
        match result.map_err(|e| e.innermost().clone()) {
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
                trait T -> E {
                    state s {
                        INIT -> s
                            (set @entity.currentPage 1)
                            (render-ui main { type: \"pagination\", currentPage: @entity.currentPage, totalPages: 1 })
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
                trait T -> E {
                    state s {
                        INIT -> s
                            (render-ui main { type: \"text\", value: @entity.id })
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
                trait T {
                    state s {
                        INIT -> s
                            (render-ui main { type: \"text\", value: @entity.x })
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
                trait ContactBrowse -> Contact {
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
                trait ContactBrowse -> Contact {
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
                trait ContactBrowse -> Contact {
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
                trait HomeView -> Home {
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
                trait ContactDelete -> Contact {
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
                trait ContactDelete -> Contact {
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
        match result.map_err(|e| e.innermost().clone()) {
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
                trait ContactDelete -> Contact {
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
        match result.map_err(|e| e.innermost().clone()) {
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
                trait ContactDelete -> Contact {
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
        match result.map_err(|e| e.innermost().clone()) {
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
    fn emit_struct_literal_into_a_tagged_union_payload_passes() {
        let src = r#"
            orbital ContactOrbital {
                type ByName = { type : "name", name : string! }
                type ByIndex = { type : "index", index : int! }
                type Pick = ByName | ByIndex
                entity Contact { id : string! }
                trait ContactDelete -> Contact {
                    initial: idle
                    state idle {
                        DELETE -> idle
                            (emit PICKED { pick: { type: "name", name: "x" } })
                    }
                    emits {
                        PICKED { pick : Pick! }
                    }
                }
            }
        "#;
        let ast = parse_str(src).expect("parse");
        assert!(lower_to_orb(&ast).is_ok());
    }

    #[test]
    fn emit_scalar_literal_into_a_tagged_union_payload_errors() {
        let src = r#"
            orbital ContactOrbital {
                type ByName = { type : "name", name : string! }
                type ByIndex = { type : "index", index : int! }
                type Pick = ByName | ByIndex
                entity Contact { id : string! }
                trait ContactDelete -> Contact {
                    initial: idle
                    state idle {
                        DELETE -> idle
                            (emit PICKED { pick: 42 })
                    }
                    emits {
                        PICKED { pick : Pick! }
                    }
                }
            }
        "#;
        let ast = parse_str(src).expect("parse");
        match lower_to_orb(&ast).map_err(|e| e.innermost().clone()) {
            Err(LowerError::EmitPayloadFieldTypeMismatch { actual, .. }) => {
                assert_eq!(actual, "number")
            }
            other => panic!("expected EmitPayloadFieldTypeMismatch, got {:?}", other),
        }
    }

    #[test]
    fn emit_with_binding_skips_type_check() {
        // `id: @payload.id` is a binding — type comes from the F3
        // resolver at L2, not from the literal. The L1 mirror skips
        // the type-compare branch for bindings.
        let src = r#"
            orbital ContactOrbital {
                entity Contact { id : string! }
                trait ContactDelete -> Contact {
                    initial: idle
                    state idle {
                        DELETE -> idle
                            (emit DELETED { id: @payload.id })
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
                trait ArticleEditor -> Article {
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
                trait ArticleEditor -> Article {
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
        match result.map_err(|e| e.innermost().clone()) {
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
    fn set_a_call_key_field_takes_a_cast_and_refuses_a_bare_string_literal() {
        let src = |value: &str| {
            format!(
                r#"
            orbital ArticleOrbital {{
                entity Article {{
                    id : string!
                    callKey : CallKey
                }}
                trait ArticleEditor -> Article {{
                    initial: idle
                    state idle {{
                        INIT -> idle
                            (set @entity.callKey {value})
                    }}
                }}
            }}
        "#
            )
        };
        let ast = parse_str(&src("(as \"search\" CallKey)")).expect("parse");
        assert!(lower_to_orb(&ast).is_ok(), "a cast makes a CallKey");
        let ast = parse_str(&src("\"search\"")).expect("parse");
        match lower_to_orb(&ast).map_err(|e| e.innermost().clone()) {
            Err(LowerError::SetValueTypeMismatch {
                expected_type,
                actual_type,
                ..
            }) => {
                assert_eq!(expected_type, "CallKey");
                assert_eq!(actual_type, "string");
            }
            other => panic!("expected SetValueTypeMismatch, got {:?}", other),
        }
        let ast = parse_str(&src("7")).expect("parse");
        assert!(
            lower_to_orb(&ast).is_err(),
            "control: a number is no CallKey either"
        );
    }

    #[test]
    fn set_a_member_of_a_primitive_union_alias_is_accepted_and_a_non_member_is_not() {
        let src = |value: &str| {
            format!(
                r#"
            orbital ArticleOrbital {{
                type Frame = string | number
                entity Article {{
                    id : string!
                    frame : Frame
                }}
                trait ArticleEditor -> Article {{
                    initial: idle
                    state idle {{
                        INIT -> idle
                            (set @entity.frame {value})
                    }}
                }}
            }}
        "#
            )
        };
        for ok in ["42", "\"front\""] {
            let ast = parse_str(&src(ok)).expect("parse");
            assert!(lower_to_orb(&ast).is_ok(), "{ok} is a member of Frame");
        }
        let ast = parse_str(&src("true")).expect("parse");
        match lower_to_orb(&ast).map_err(|e| e.innermost().clone()) {
            Err(LowerError::SetValueTypeMismatch { actual_type, .. }) => {
                assert_eq!(actual_type, "boolean")
            }
            other => panic!("expected SetValueTypeMismatch, got {:?}", other),
        }
    }

    #[test]
    fn a_let_annotated_by_a_union_alias_naming_a_sealed_type_carries_a_union() {
        let src = |body: &str| {
            format!(
                r#"
            type A2 = {body}
            orbital O {{
                entity E [runtime] {{
                    id : string!
                }}
                trait T -> E {{
                    initial: idle
                    state idle {{
                        INIT -> idle
                            (let ((xs : [A2] [1 2])) (log "x" (array/len @xs)))
                    }}
                }}
                page boot as P -> T
            }}
        "#
            )
        };
        for body in ["int | number | Borrow", "int | number"] {
            let schema = lower_to_orb(&parse_str(&src(body)).expect("parse")).expect("lowers");
            let text = serde_json::to_string(&schema).expect("serializes");
            assert!(text.contains(r#"\"A2\":{\"kind\":\"union\""#), "{body}: {text}");
            assert!(!text.contains(r#"\"A2\":{\"kind\":\"enum\""#), "{body}");
        }
    }

    #[test]
    fn a_width_field_lowers_to_its_width_and_unlowers_to_the_same_name() {
        let src = r#"
            orbital O {
                entity Pixel [runtime] {
                    id : string!
                    r : u8
                    total : i64
                }
                trait T -> Pixel {
                    initial: idle
                    state idle {
                        INIT -> idle
                    }
                }
                page boot as P -> T
            }
        "#;
        let schema = lower_to_orb(&parse_str(src).expect("parse")).expect("lowers");
        let text = serde_json::to_string(&schema).expect("serializes");
        assert!(text.contains(r#""type":"u8""#), "{text}");
        assert!(text.contains(r#""type":"int""#), "control: i64 is int: {text}");
        let printed = crate::format_program(&crate::unlower::unlower(&schema).expect("unlowers"));
        assert!(printed.contains("r : u8"), "{printed}");
    }

    #[test]
    fn a_local_listens_payload_typed_by_a_primitive_union_alias_lowers_to_a_union() {
        let src = r#"
            orbital ArticleOrbital {
                type Frame = string | number
                entity Article {
                    id : string!
                    frame : Frame
                }
                trait ArticleEditor -> Article {
                    initial: idle
                    state idle {
                        FRAMED -> idle
                            (set @entity.frame ?frame)
                    }
                    listens {
                        FRAMED { frame : Frame! }
                    }
                }
            }
        "#;
        let ast = parse_str(src).expect("parse");
        let schema = lower_to_orb(&ast).expect("lowers");
        let orbital_core_base::schema::TraitReference::Inline(trait_def) =
            &schema.orbitals[0].traits[0]
        else {
            panic!("inline trait");
        };
        let events = trait_def.state_machine.events.as_ref().expect("events");
        let framed = events
            .iter()
            .find(|e| e.key == "FRAMED")
            .expect("FRAMED event");
        let frame = framed
            .payload_schema
            .as_ref()
            .and_then(|p| p.iter().find(|f| f.name == "frame"))
            .expect("frame payload field");
        assert_eq!(frame.field_type, "union");
        assert_eq!(frame.values, None);
        let variants: Vec<&str> = frame
            .properties
            .as_ref()
            .expect("variants")
            .iter()
            .map(|v| v.name.as_str())
            .collect();
        assert_eq!(variants, vec!["string", "number"]);
    }

    #[test]
    fn a_union_with_an_sexpr_member_keeps_that_member() {
        // std ItemAction's `navigatesTo : string | SExpr`: a path, or a quoted row lambda.
        let src = r#"
            orbital ListOrbital {
                type Act = { label : string!, go : string | SExpr }
                entity Row {
                    id : string!
                }
                trait RowList -> Row {
                    config {
                        acts : [Act] = []
                    }
                    initial: idle
                    state idle {
                        INIT -> idle
                            (render-ui main { type: typography, content: (str/concat "" (array/len @config.acts)) })
                    }
                }
            }
        "#;
        let ast = parse_str(src).expect("parse");
        let schema = lower_to_orb(&ast).expect("lowers");
        let orbital_core_base::schema::TraitReference::Inline(trait_def) = &schema.orbitals[0].traits[0] else {
            panic!("inline trait");
        };
        let acts = trait_def.config.as_ref().and_then(|c| c.get("acts")).expect("acts knob");
        let go = acts
            .items
            .as_ref()
            .and_then(|i| i.properties.as_ref())
            .and_then(|p| p.get("go"))
            .expect("go member");
        assert_eq!(go.field_type, orbital_core_base::schema::FieldType::Union, "{go:?}");
        assert_eq!(go.values.as_deref(), Some(&["string".to_string(), "SExpr".to_string()][..]));
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
                trait TicketFlow -> Ticket {
                    initial: idle
                    state idle {
                        INIT -> idle
                            (set @entity.status pendng)
                    }
                }
            }
        "#;
        let ast = parse_str(src).expect("parse");
        match lower_to_orb(&ast).map_err(|e| e.innermost().clone()) {
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
                trait TicketFlow -> Ticket {
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
                trait CounterEdit -> Counter {
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
                trait CounterEdit -> Counter {
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
        match result.map_err(|e| e.innermost().clone()) {
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
                trait ListEdit -> List {
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
                trait ArticleView -> Article {
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
                trait BatchRun -> Batch {
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
            "orbital O {\n  expects entity Customer [identity] {\n    role : \"store-manager\" | \"operations-director\"\n  }\n  expects entity OrderRecord {\n    totalAmount : number\n  }\n  expects entity CartItem\n  expects trait Checkout { emits { ORDER_PLACED } }\n  expects page \"/articles/:slug\"\n  entity E { id : string! }\n}",
        );
        let expects = s.orbitals[0].expects.as_ref().expect("expects lowered");
        assert_eq!(expects.len(), 5);
        match &expects[0] {
            ExpectDeclaration::Identity { name, shape, .. } => {
                assert_eq!(name.as_deref(), Some("Customer"));
                let shape = shape.as_ref().expect("identity shape");
                assert_eq!(shape.len(), 1);
                assert_eq!(shape[0].name.as_deref(), Some("role"));
                assert_eq!(
                    shape[0].values.as_deref(),
                    Some(
                        &[
                            "store-manager".to_string(),
                            "operations-director".to_string()
                        ][..]
                    )
                );
            }
            other => panic!("expected identity, got {other:?}"),
        }
        match &expects[1] {
            ExpectDeclaration::Entity { name, shape, .. } => {
                assert_eq!(name, "OrderRecord");
                let shape = shape.as_ref().expect("entity shape");
                assert_eq!(shape.len(), 1);
                assert_eq!(shape[0].name.as_deref(), Some("totalAmount"));
                assert!(matches!(shape[0].field_type, FieldType::Number));
            }
            other => panic!("expected entity, got {other:?}"),
        }
        match &expects[2] {
            ExpectDeclaration::Entity { name, shape, .. } => {
                assert_eq!(name, "CartItem");
                assert!(shape.is_none());
            }
            other => panic!("expected entity, got {other:?}"),
        }
        match &expects[3] {
            ExpectDeclaration::Event {
                trait_name, event, ..
            } => {
                // Verbatim passthrough — no case-folding on either segment.
                assert_eq!(trait_name, "Checkout");
                assert_eq!(event, "ORDER_PLACED");
            }
            other => panic!("expected event, got {other:?}"),
        }
        match &expects[4] {
            // A route pattern, verbatim — `:slug` must survive lowering.
            ExpectDeclaration::Page { path, .. } => assert_eq!(path, "/articles/:slug"),
            other => panic!("expected page, got {other:?}"),
        }
    }

    #[test]
    fn lower_expects_page_protocol_word_checks_the_table() {
        let s = lower("orbital O {\n  expects page boot -> Seed\n  expects page \"/p\"\n  entity E { id : string! }\n}");
        let expects = s.orbitals[0].expects.as_ref().expect("expects lowered");
        assert!(matches!(&expects[0], ExpectDeclaration::Page { protocol: Some(w), path, traits: Some(t), .. }
            if w == "boot" && path.is_empty() && t == &vec!["Seed".to_string()]));
        assert!(matches!(&expects[1], ExpectDeclaration::Page { protocol: None, .. }));
        for bad in ["bogus", "route"] {
            let ast = parse_str(&format!("orbital O {{\n  expects page {bad}\n  entity E {{ id : string! }}\n}}")).expect("parse");
            assert!(lower_to_orb(&ast).is_err(), "{bad}");
        }
    }

    #[test]
    fn lower_expects_absent_is_none() {
        let s = lower("orbital Foo { entity Bar { id : string! } }");
        assert!(s.orbitals[0].expects.is_none());
    }
    #[test]
    fn an_expects_only_orbital_lowers_to_strict_obligations_and_provides_nothing() {
        let s = lower("orbital ShopSpec {\n  expects entity Order { total : number }\n  expects trait Checkout { emits { PLACED } }\n  expects page \"/orders\"\n}");
        let orb = &s.orbitals[0];
        let expects = orb.expects.as_ref().expect("expects lowered");
        assert_eq!(expects.len(), 3);
        assert!(expects.iter().all(|e| e.is_strict()), "{expects:?}");
        assert!(orb.is_spec());
        assert!(orb.traits.is_empty());
        assert!(orb.pages.as_ref().map_or(true, Vec::is_empty));
    }

    #[test]
    fn control_expects_beside_a_real_entity_stay_lenient() {
        let s = lower("orbital O {\n  expects entity Order\n  entity E { id : string! }\n}");
        let expects = s.orbitals[0].expects.as_ref().expect("expects lowered");
        assert!(expects.iter().all(|e| !e.is_strict()));
        assert!(!s.orbitals[0].is_spec());
    }

    #[test]
    fn an_orbital_with_neither_entity_nor_expects_still_needs_an_entity() {
        let ast = parse_str("orbital O {\n}").expect("an empty orbital parses");
        let err = lower_to_orb(&ast).expect_err("an orbital with nothing is not a spec");
        assert!(err.to_string().contains("must define an entity"), "{err}");
    }

    #[test]
    fn an_expects_trait_payload_lowers_to_the_event_expectation_payload() {
        let s = lower("orbital ShopSpec {\n  expects trait Checkout { emits {\n    PLACED { orderId : string!, note : string }\n    CANCELLED\n  } }\n}");
        let expects = s.orbitals[0].expects.as_ref().expect("expects lowered");
        match &expects[0] {
            ExpectDeclaration::Event {
                payload: Some(p), ..
            } => {
                assert_eq!(p.len(), 2);
                assert_eq!(p[0].name, "orderId");
                assert_eq!(p[0].field_type, "string");
                assert_eq!(p[0].required, Some(true));
                assert_ne!(p[1].required, Some(true));
            }
            other => panic!("expected an event with a payload, got {other:?}"),
        }
        assert!(
            matches!(&expects[1], ExpectDeclaration::Event { payload: None, .. }),
            "control: a bare event has no payload"
        );
    }

    #[test]
    fn an_expects_config_lowers_one_knob_obligation_per_knob() {
        let src = "orbital CliSpec {\n  expects config {\n    stdio : string = \"pipe\"\n    retries : number\n  }\n}";
        let s = lower(src);
        let expects = s.orbitals[0].expects.as_ref().expect("expects lowered");
        assert_eq!(expects.len(), 2);
        match &expects[0] {
            ExpectDeclaration::Config { name, knob, strict } => {
                assert_eq!(name, "stdio");
                assert_eq!(knob.field_type, "string");
                assert_eq!(knob.default, Some(SExpression::String(("pipe".to_string()).into())));
                assert!(*strict);
            }
            other => panic!("expected a config obligation, got {other:?}"),
        }
        assert!(
            matches!(&expects[1], ExpectDeclaration::Config { name, knob, .. } if name == "retries" && knob.default.is_none())
        );
        let reformatted = crate::format::format_program(&parse_str(src).expect("parses"));
        assert!(
            reformatted.contains("expects config { stdio : string = pipe }"),
            "{reformatted}"
        );
        let relowered = lower(&reformatted);
        let again = relowered.orbitals[0]
            .expects
            .as_ref()
            .expect("expects re-lowered");
        assert_eq!(again.len(), 2);
        assert!(
            matches!(&again[0], ExpectDeclaration::Config { knob, .. } if knob.default == Some(SExpression::String(("pipe".to_string()).into())))
        );
    }

    #[test]
    fn an_expects_config_knob_carries_no_annotations() {
        let err = parse_str("orbital CliSpec {\n  expects config {\n    stdio : string = \"pipe\"\n      @label \"Streams\"\n  }\n}")
            .expect_err("an annotation describes a provider");
        assert!(err.to_string().contains("expects config"), "{err}");
    }

    #[test]
    fn a_repeated_expects_config_knob_is_a_duplicate() {
        let err = parse_str("orbital CliSpec {\n  expects config { stdio : string }\n  expects config { stdio : string = \"pipe\" }\n}")
            .expect_err("one obligation per knob");
        assert!(err.to_string().contains("stdio"), "{err}");
    }

    #[test]
    fn an_expects_entity_lowers_its_storage_marks_and_access() {
        let s = lower("orbital ShopSpec {\n  expects entity Order [persistent, shared] {\n    @read (= @entity.id \"x\")\n    @update none \"joins another entity\"\n    id : string! @key\n    email : string @unique\n  }\n  expects page \"/orders\" -> Checkout\n  expects trait Checkout { state idle { PAY -> paid } state refunded }\n}");
        let expects = s.orbitals[0].expects.as_ref().expect("expects lowered");
        match &expects[0] {
            ExpectDeclaration::Entity {
                persistence,
                shared,
                read_policy,
                access_waivers,
                shape,
                ..
            } => {
                assert_eq!(*persistence, Some(CorePersistence::Persistent));
                assert!(*shared);
                assert!(read_policy.is_some());
                assert_eq!(
                    access_waivers.as_ref().and_then(|w| w.update.as_deref()),
                    Some("joins another entity")
                );
                let fields = shape.as_ref().expect("shape");
                assert_eq!(fields[0].primary_key, Some(true));
                assert_eq!(fields[1].unique, Some(true));
            }
            other => panic!("expected an entity, got {other:?}"),
        }
        assert!(
            matches!(&expects[1], ExpectDeclaration::Page { traits: Some(t), .. } if t == &vec!["Checkout".to_string()])
        );
        assert!(
            matches!(&expects[2], ExpectDeclaration::Transition { from, event, to, .. } if from == "idle" && event == "PAY" && to == "paid")
        );
        assert!(
            matches!(&expects[3], ExpectDeclaration::State { state, .. } if state == "refunded")
        );
        assert!(
            expects.iter().all(ExpectDeclaration::is_strict),
            "a spec's declarations are strict"
        );
    }

    #[test]
    fn a_spec_names_an_entity_of_a_spec_it_uses_through_the_alias() {
        let s = lower("uses Inner from \"./inner.lolo\"\n\norbital OuterSpec {\n  expects entity Order {\n    id : string!\n    owner : Inner.Customer\n  }\n}");
        let expects = s.orbitals[0].expects.as_ref().expect("expects lowered");
        let ExpectDeclaration::Entity {
            shape: Some(fields),
            ..
        } = &expects[0]
        else {
            panic!("expected an entity with a shape");
        };
        let uses = s.orbitals[0]
            .uses
            .as_ref()
            .expect("the spec keeps its `uses`, which is how it inherits Inner's obligations");
        assert_eq!(uses[0].alias, "Inner");
        let owner = fields
            .iter()
            .find(|f| f.name.as_deref() == Some("owner"))
            .expect("owner");
        assert_eq!(
            owner.relation.as_ref().map(|r| r.entity.as_str()),
            Some("Customer"),
            "the alias-qualified name is the imported spec's entity: {owner:?}"
        );
    }

    #[test]
    fn control_a_qualified_name_whose_head_is_no_uses_alias_is_still_a_projection_error() {
        let ast = parse_str("orbital OuterSpec {\n  expects entity Order {\n    id : string!\n    owner : Other.Customer\n  }\n}").expect("parse");
        let err = lower_to_orb(&ast).expect_err("Other is not an alias");
        assert!(err.to_string().contains("ELOLO_FIELD_PROJECTION"), "{err}");
    }

    #[test]
    fn an_alias_that_is_also_a_declared_type_name_is_ambiguous() {
        let ast = parse_str("uses Order from \"./inner.lolo\"\n\norbital O {\n  expects entity Order {\n    id : string!\n    owner : Order.Customer\n  }\n  entity Order {\n    id : string!\n    Customer : string\n  }\n}").expect("parse");
        let err = lower_to_orb(&ast).expect_err("ambiguous");
        assert!(err.to_string().contains("alias"), "{err}");
    }
}
