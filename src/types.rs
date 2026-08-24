//! Type-system resolver for `.lolo`.
//!
//! Given a parsed `Program`, this module:
//!
//! 1. Collects every `TypeAlias` declaration into a `TypeRegistry` (file-level
//!    plus per-orbital scopes). File-level entries shadow when an orbital
//!    redefines the same name; resolution looks up orbital-scope first.
//! 2. Runs a cycle check across the alias graph (each alias's body edges to
//!    the names it references in `Composition` members or `TypeExpr::Named`
//!    positions). Cycles become `TypeError::CircularType`.
//! 3. Resolves a named type + positional args to a concrete `TypeBody` with
//!    parameter substitution applied and `Composition` members flattened into
//!    a single merged body when they all share a kind.
//! 4. Checks `::` annotations against the body of their attached declaration.
//!
//! Types are a `.lolo`-layer concept only. After checking, the lowerer strips
//! them — the emitted `.orb` is byte-identical to what the same `.lolo` would
//! produce without any type declarations.
//!
//! Scope: Stage 2 implementation is deliberately conservative. It catches the
//! five main error classes (unknown type, cycle, kind mismatch, composition
//! conflict, annotation mismatch) without attempting structural trait
//! subtyping or deep orbital-slot verification — those live in Stage 3 on top
//! of the comprehensive validator.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use crate::ast::{
    Emit, Entity, EntityField, ExpectsKind, Orbital, Page, Persistence, Program, Span, Trait,
    TypeAlias, TypeAnnotation, TypeBody, TypeExpr,
};
use crate::error::{TypeError, TypeKindName};

/// Per-scope alias table. Two of these are built during `Program` resolution:
/// one from `program.type_aliases` (file scope) and one from each orbital's
/// `orb.type_aliases` (orbital scope, shadows file scope).
#[derive(Debug, Clone, Default)]
pub struct TypeRegistry {
    /// Ordered map so diagnostic output is deterministic.
    aliases: BTreeMap<String, TypeAlias>,
}

impl TypeRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, alias: TypeAlias) {
        self.aliases.insert(alias.name.clone(), alias);
    }

    pub fn get(&self, name: &str) -> Option<&TypeAlias> {
        self.aliases.get(name)
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.aliases.keys().map(|s| s.as_str())
    }
}

/// Composite registry: look up orbital-scope first, then file-scope.
pub struct ResolveCtx<'a> {
    pub file: &'a TypeRegistry,
    pub orbital: Option<&'a TypeRegistry>,
}

impl<'a> ResolveCtx<'a> {
    fn lookup(&self, name: &str) -> Option<&'a TypeAlias> {
        if let Some(orb) = self.orbital {
            if let Some(alias) = orb.get(name) {
                return Some(alias);
            }
        }
        self.file.get(name)
    }

    fn known_names(&self) -> Vec<String> {
        let mut all: BTreeSet<String> = self.file.names().map(str::to_owned).collect();
        if let Some(orb) = self.orbital {
            all.extend(orb.names().map(str::to_owned));
        }
        all.into_iter().collect()
    }
}

/// Resolve a concrete body for a named type + positional args.
///
/// The returned `TypeBody` is fully substituted (no free parameters) and any
/// top-level `Composition` has been flattened + merged into the dominant kind.
pub fn resolve_named(
    name: &str,
    args: &[String],
    ctx: &ResolveCtx<'_>,
    ann_span: Span,
) -> Result<TypeBody, TypeError> {
    let alias = ctx.lookup(name).ok_or_else(|| TypeError::UnknownType {
        name: name.to_string(),
        suggestion: closest_name(name, &ctx.known_names()),
        span: ann_span,
    })?;

    let expected_args = alias.type_param.as_ref().map_or(0, |_| 1);
    if args.len() != expected_args {
        return Err(TypeError::WrongTypeArgCount {
            alias: name.to_string(),
            expected: expected_args,
            got: args.len(),
            span: ann_span,
        });
    }

    let mut binding: HashMap<String, TypeExpr> = HashMap::new();
    if let (Some(param), Some(arg)) = (alias.type_param.as_ref(), args.first()) {
        binding.insert(param.clone(), TypeExpr::Named(arg.clone()));
    }

    let resolved = substitute_body(&alias.body, &binding, ctx, ann_span)?;
    flatten_composition(resolved, ctx, ann_span)
}

/// Apply a single-parameter substitution to a `TypeBody` (recursively into
/// `Composition` members and trait-linked-entity / orbital slots).
fn substitute_body(
    body: &TypeBody,
    binding: &HashMap<String, TypeExpr>,
    ctx: &ResolveCtx<'_>,
    ann_span: Span,
) -> Result<TypeBody, TypeError> {
    match body {
        TypeBody::Enum(v) => Ok(TypeBody::Enum(v.clone())),
        TypeBody::Struct(fs) => Ok(TypeBody::Struct(fs.clone())),
        TypeBody::Alias(te) => Ok(TypeBody::Alias(substitute_expr(te, binding))),
        TypeBody::Entity {
            persistence,
            collection,
            fields,
        } => Ok(TypeBody::Entity {
            persistence: persistence.clone(),
            collection: collection.clone(),
            fields: fields
                .iter()
                .map(|f| EntityField {
                    type_expr: substitute_expr(&f.type_expr, binding),
                    ..f.clone()
                })
                .collect(),
        }),
        TypeBody::Trait {
            upstream,
            linked_entity,
        } => Ok(TypeBody::Trait {
            upstream: upstream.clone(),
            linked_entity: linked_entity.as_ref().map(|e| substitute_expr(e, binding)),
        }),
        TypeBody::Page { path, trait_types } => Ok(TypeBody::Page {
            path: path.clone(),
            trait_types: trait_types
                .iter()
                .map(|e| substitute_expr(e, binding))
                .collect(),
        }),
        TypeBody::Orbital {
            entity,
            trait_types,
            page_type,
        } => Ok(TypeBody::Orbital {
            entity: entity.as_ref().map(|e| substitute_expr(e, binding)),
            trait_types: trait_types
                .iter()
                .map(|e| substitute_expr(e, binding))
                .collect(),
            page_type: page_type.as_ref().map(|e| substitute_expr(e, binding)),
        }),
        TypeBody::Event {
            payload,
            scope,
            description,
        } => Ok(TypeBody::Event {
            payload: substitute_expr(payload, binding),
            scope: *scope,
            description: description.clone(),
        }),
        TypeBody::Composition(parts) => {
            let mut subbed = Vec::with_capacity(parts.len());
            for part in parts {
                subbed.push(substitute_body(part, binding, ctx, ann_span)?);
            }
            Ok(TypeBody::Composition(subbed))
        }
    }
}

/// Apply a substitution to a `TypeExpr` — only `Named(name)` where `name`
/// matches a binding key is rewritten.
fn substitute_expr(expr: &TypeExpr, binding: &HashMap<String, TypeExpr>) -> TypeExpr {
    match expr {
        TypeExpr::Named(name) => binding
            .get(name)
            .cloned()
            .unwrap_or_else(|| TypeExpr::Named(name.clone())),
        TypeExpr::Array(inner) => TypeExpr::Array(Box::new(substitute_expr(inner, binding))),
        TypeExpr::Optional(inner) => TypeExpr::Optional(Box::new(substitute_expr(inner, binding))),
        TypeExpr::Union(parts) => {
            TypeExpr::Union(parts.iter().map(|p| substitute_expr(p, binding)).collect())
        }
        TypeExpr::Object(fields) => TypeExpr::Object(
            fields
                .iter()
                .map(|(k, t, req)| (k.clone(), substitute_expr(t, binding), *req))
                .collect(),
        ),
        other => other.clone(),
    }
}

/// Flatten a top-level `Composition` into a single merged body. Nested
/// `Composition` levels are recursively flattened; each leaf must share the
/// kind of its siblings. Entity compositions merge field lists and error on
/// collisions between differing field types.
fn flatten_composition(
    body: TypeBody,
    ctx: &ResolveCtx<'_>,
    span: Span,
) -> Result<TypeBody, TypeError> {
    match body {
        TypeBody::Composition(parts) => {
            let flat = flatten_parts(parts, span)?;
            // Expand leaf `Alias(Named(X))` references into the concrete body
            // of X so kind-checks + field-merges see Entity/Trait/Page bodies
            // instead of nominal aliases.
            let flat = expand_leaf_aliases(flat, ctx, span)?;
            if flat.is_empty() {
                return Err(TypeError::AnnotationMismatch {
                    type_name: "<composition>".into(),
                    what: "empty composition".into(),
                    span,
                });
            }
            let leader_kind = kind_of(&flat[0]);
            for p in &flat[1..] {
                let k = kind_of(p);
                if k != leader_kind {
                    return Err(TypeError::KindMismatch {
                        expected: kind_label(leader_kind),
                        actual: kind_label(k),
                        span,
                    });
                }
            }
            match leader_kind {
                TypeKindIndex::Entity => merge_entity_composition(flat, span),
                TypeKindIndex::Trait => Ok(flat.into_iter().next().unwrap()),
                TypeKindIndex::Page => Ok(flat.into_iter().next().unwrap()),
                TypeKindIndex::Orbital => Ok(flat.into_iter().next().unwrap()),
                _ => Err(TypeError::KindMismatch {
                    expected: TypeKindName::Entity,
                    actual: kind_label(leader_kind),
                    span,
                }),
            }
        }
        other => Ok(other),
    }
}

fn flatten_parts(parts: Vec<TypeBody>, span: Span) -> Result<Vec<TypeBody>, TypeError> {
    let mut out = Vec::new();
    for p in parts {
        match p {
            TypeBody::Composition(nested) => out.extend(flatten_parts(nested, span)?),
            other => out.push(other),
        }
    }
    Ok(out)
}

/// Expand any leaf `TypeBody::Alias(TypeExpr::Named(name))` in the part list
/// into the referenced alias's concrete body. This is what lets
/// `type Auditable = Timestamped & Entity { extra }` compose: `Timestamped`
/// is parsed as `Alias(Named("Timestamped"))` and must expand to its actual
/// `Entity` body before kind-checks + field-merge run.
///
/// Recursion depth is bounded by `detect_cycles` which runs at program-entry
/// — a forward reference caught there will short-circuit this expansion.
fn expand_leaf_aliases(
    parts: Vec<TypeBody>,
    ctx: &ResolveCtx<'_>,
    span: Span,
) -> Result<Vec<TypeBody>, TypeError> {
    let mut out = Vec::new();
    for p in parts {
        match &p {
            TypeBody::Alias(TypeExpr::Named(name)) => {
                let resolved = resolve_named(name, &[], ctx, span)?;
                match resolved {
                    TypeBody::Composition(nested) => {
                        let expanded = expand_leaf_aliases(nested, ctx, span)?;
                        out.extend(expanded);
                    }
                    other => out.push(other),
                }
            }
            _ => out.push(p),
        }
    }
    Ok(out)
}

fn merge_entity_composition(parts: Vec<TypeBody>, span: Span) -> Result<TypeBody, TypeError> {
    let mut merged_fields: Vec<EntityField> = Vec::new();
    let mut field_owners: HashMap<String, Vec<String>> = HashMap::new();
    let mut persistence: Option<Persistence> = None;
    let mut collection: Option<String> = None;
    for (idx, body) in parts.into_iter().enumerate() {
        let TypeBody::Entity {
            persistence: p,
            collection: c,
            fields,
        } = body
        else {
            unreachable!("flatten_composition pre-checks kind");
        };
        if let Some(p) = p {
            persistence = Some(p);
        }
        if let Some(c) = c {
            collection = Some(c);
        }
        for f in fields {
            let owner_label = format!("part[{idx}]");
            field_owners
                .entry(f.name.clone())
                .or_default()
                .push(owner_label);
            if let Some(existing) = merged_fields.iter().find(|m| m.name == f.name) {
                // Collision — conservative Stage-2 rule: types must match via
                // structural equality on the string form of their `TypeExpr`.
                if format_type_expr(&existing.type_expr) != format_type_expr(&f.type_expr) {
                    return Err(TypeError::CompositionConflict {
                        field: f.name.clone(),
                        types: field_owners.remove(&f.name).unwrap_or_default(),
                        span,
                    });
                }
            } else {
                merged_fields.push(f);
            }
        }
    }
    Ok(TypeBody::Entity {
        persistence,
        collection,
        fields: merged_fields,
    })
}

fn format_type_expr(te: &TypeExpr) -> String {
    match te {
        TypeExpr::Primitive(s) => format!("prim:{s}"),
        TypeExpr::BoundEntityRow => "@entity".to_string(),
        TypeExpr::Named(s) => format!("named:{s}"),
        TypeExpr::StringLiteral(s) => format!("lit:{s:?}"),
        TypeExpr::Array(inner) => format!("[{}]", format_type_expr(inner)),
        TypeExpr::Map(key, value) => {
            format!("map:{}:{}", format_type_expr(key), format_type_expr(value))
        }
        TypeExpr::Object(fields) => {
            let mut parts: Vec<String> = fields
                .iter()
                .map(|(k, v, req)| {
                    let bang = if *req { "!" } else { "" };
                    format!("{k}{bang}:{}", format_type_expr(v))
                })
                .collect();
            parts.sort();
            format!("{{{}}}", parts.join(","))
        }
        TypeExpr::Union(parts) => {
            let mut p: Vec<String> = parts.iter().map(format_type_expr).collect();
            p.sort();
            p.join("|")
        }
        TypeExpr::Optional(inner) => format!("{}?", format_type_expr(inner)),
        TypeExpr::RelationMany(n) => format!("{n}*"),
        TypeExpr::RelationManyNonEmpty(n) => format!("{n}+"),
        TypeExpr::Trait => "trait".to_string(),
        TypeExpr::EntityRef => "entity".to_string(),
        TypeExpr::EventRef => "event".to_string(),
        TypeExpr::Slot => "slot".to_string(),
        TypeExpr::Pattern => "pattern".to_string(),
        TypeExpr::Secret => "secret".to_string(),
        TypeExpr::SExpr => "SExpr".to_string(),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TypeKindIndex {
    Enum,
    Struct,
    Alias,
    Entity,
    Trait,
    Page,
    Orbital,
    Event,
}

fn kind_of(body: &TypeBody) -> TypeKindIndex {
    match body {
        TypeBody::Enum(_) => TypeKindIndex::Enum,
        TypeBody::Struct(_) => TypeKindIndex::Struct,
        TypeBody::Alias(_) => TypeKindIndex::Alias,
        TypeBody::Entity { .. } => TypeKindIndex::Entity,
        TypeBody::Trait { .. } => TypeKindIndex::Trait,
        TypeBody::Page { .. } => TypeKindIndex::Page,
        TypeBody::Orbital { .. } => TypeKindIndex::Orbital,
        TypeBody::Event { .. } => TypeKindIndex::Event,
        TypeBody::Composition(parts) => parts.first().map_or(TypeKindIndex::Alias, kind_of),
    }
}

fn kind_label(k: TypeKindIndex) -> TypeKindName {
    match k {
        TypeKindIndex::Enum => TypeKindName::Enum,
        TypeKindIndex::Struct => TypeKindName::Struct,
        TypeKindIndex::Alias => TypeKindName::Alias,
        TypeKindIndex::Entity => TypeKindName::Entity,
        TypeKindIndex::Trait => TypeKindName::Trait,
        TypeKindIndex::Page => TypeKindName::Page,
        TypeKindIndex::Orbital => TypeKindName::Orbital,
        TypeKindIndex::Event => TypeKindName::Event,
    }
}

/// DFS cycle-detect across the alias graph. Edges: an alias's body references
/// other alias names via `TypeBody::Composition` leaves or `TypeExpr::Named`
/// positions. On finding a cycle, the offending chain is returned.
pub fn detect_cycles(ctx: &ResolveCtx<'_>) -> Result<(), TypeError> {
    let mut visiting: BTreeSet<String> = BTreeSet::new();
    let mut visited: BTreeSet<String> = BTreeSet::new();
    // Collect every alias from both scopes to start from.
    let all: Vec<String> = ctx.known_names();
    for name in &all {
        let mut path: Vec<String> = Vec::new();
        dfs_cycle(name, ctx, &mut visiting, &mut visited, &mut path)?;
    }
    Ok(())
}

fn dfs_cycle(
    name: &str,
    ctx: &ResolveCtx<'_>,
    visiting: &mut BTreeSet<String>,
    visited: &mut BTreeSet<String>,
    path: &mut Vec<String>,
) -> Result<(), TypeError> {
    if visited.contains(name) {
        return Ok(());
    }
    if visiting.contains(name) {
        // Found a cycle; build the reporting chain from path tail.
        let cycle_start = path.iter().position(|n| n == name).unwrap_or(0);
        let mut cycle: Vec<String> = path[cycle_start..].to_vec();
        cycle.push(name.to_string());
        return Err(TypeError::CircularType {
            cycle,
            span: Span::dummy(),
        });
    }
    let alias = match ctx.lookup(name) {
        Some(a) => a,
        None => return Ok(()),
    };
    visiting.insert(name.to_string());
    path.push(name.to_string());
    for referenced in collect_refs(&alias.body) {
        dfs_cycle(&referenced, ctx, visiting, visited, path)?;
    }
    path.pop();
    visiting.remove(name);
    visited.insert(name.to_string());
    Ok(())
}

fn collect_refs(body: &TypeBody) -> BTreeSet<String> {
    let mut refs = BTreeSet::new();
    visit_refs(body, &mut refs);
    refs
}

fn visit_refs(body: &TypeBody, out: &mut BTreeSet<String>) {
    match body {
        TypeBody::Enum(_) | TypeBody::Struct(_) => {}
        TypeBody::Alias(te) => visit_expr_refs(te, out),
        TypeBody::Entity { fields, .. } => {
            for f in fields {
                visit_expr_refs(&f.type_expr, out);
            }
        }
        TypeBody::Trait {
            linked_entity: Some(te),
            ..
        } => visit_expr_refs(te, out),
        TypeBody::Trait { .. } => {}
        TypeBody::Page { trait_types, .. } => {
            for t in trait_types {
                visit_expr_refs(t, out);
            }
        }
        TypeBody::Orbital {
            entity,
            trait_types,
            page_type,
        } => {
            if let Some(e) = entity {
                visit_expr_refs(e, out);
            }
            for t in trait_types {
                visit_expr_refs(t, out);
            }
            if let Some(p) = page_type {
                visit_expr_refs(p, out);
            }
        }
        TypeBody::Event { payload, .. } => visit_expr_refs(payload, out),
        TypeBody::Composition(parts) => {
            for p in parts {
                visit_refs(p, out);
            }
        }
    }
}

fn visit_expr_refs(te: &TypeExpr, out: &mut BTreeSet<String>) {
    match te {
        TypeExpr::Named(n) => {
            // Primitive names also flow through Named during type-body parsing
            // in some paths; filter the obvious ones out.
            if !is_primitive(n) {
                out.insert(n.clone());
            }
        }
        TypeExpr::Array(inner) | TypeExpr::Optional(inner) => visit_expr_refs(inner, out),
        TypeExpr::Map(key, value) => {
            visit_expr_refs(key, out);
            visit_expr_refs(value, out);
        }
        TypeExpr::Union(parts) => {
            for p in parts {
                visit_expr_refs(p, out);
            }
        }
        TypeExpr::Object(fields) => {
            for (_, v, _) in fields {
                visit_expr_refs(v, out);
            }
        }
        TypeExpr::RelationMany(n) | TypeExpr::RelationManyNonEmpty(n) => {
            out.insert(n.clone());
        }
        // `@entity` references no NAMED type — nothing to collect.
        TypeExpr::BoundEntityRow => {}
        TypeExpr::Primitive(_)
        | TypeExpr::StringLiteral(_)
        | TypeExpr::Trait
        | TypeExpr::EntityRef
        | TypeExpr::EventRef
        | TypeExpr::Slot
        | TypeExpr::Pattern
        | TypeExpr::Secret
        | TypeExpr::SExpr => {}
    }
}

fn is_primitive(name: &str) -> bool {
    // Single source of truth: the lowerer's primitive set (Gap
    // G-LOLO-TYPENAME-SILENT-FALLBACK — the checker and lowerer must agree on
    // what a primitive is, else a name the lowerer maps is flagged unknown here,
    // or vice versa).
    crate::lower::primitive_field_type(name).is_some()
}

/// Validate every type-name reference inside an entity- or config-field
/// `type_expr` resolves to a known primitive, type alias, or entity name. An
/// unrecognized name (a typo'd alias, an undeclared type) surfaces as
/// `UnknownType` with a nearest-name suggestion — instead of silently lowering
/// to a bare `string` and erasing the declared contract (Gap
/// G-LOLO-TYPENAME-SILENT-FALLBACK). Primitives are filtered out by
/// `visit_expr_refs`; the remaining refs must be aliases or entity names.
fn check_field_type_refs(
    type_expr: &TypeExpr,
    span: Span,
    ctx: &ResolveCtx<'_>,
    entity_names: &BTreeSet<String>,
    trait_type_params: &BTreeSet<String>,
) -> Result<(), TypeError> {
    let mut refs = BTreeSet::new();
    visit_expr_refs(type_expr, &mut refs);
    for r in &refs {
        if ctx.lookup(r).is_some() || entity_names.contains(r) || trait_type_params.contains(r) {
            continue;
        }
        let mut haystack = ctx.known_names();
        haystack.extend(entity_names.iter().cloned());
        return Err(TypeError::UnknownType {
            name: r.clone(),
            suggestion: closest_name(r, &haystack),
            span,
        });
    }
    Ok(())
}

pub(crate) fn closest_name(needle: &str, haystack: &[String]) -> Option<String> {
    // Levenshtein, basic.
    let mut best: Option<(usize, &String)> = None;
    for candidate in haystack {
        let d = levenshtein(needle, candidate);
        match best {
            None => best = Some((d, candidate)),
            Some((prev, _)) if d < prev => best = Some((d, candidate)),
            _ => {}
        }
    }
    best.filter(|(d, _)| *d <= 3).map(|(_, n)| n.clone())
}

fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let m = a.len();
    let n = b.len();
    if m == 0 {
        return n;
    }
    if n == 0 {
        return m;
    }
    let mut prev: Vec<usize> = (0..=n).collect();
    let mut curr = vec![0usize; n + 1];
    for i in 1..=m {
        curr[0] = i;
        for j in 1..=n {
            let cost = if a[i - 1] == b[j - 1] { 0 } else { 1 };
            curr[j] = std::cmp::min(
                std::cmp::min(curr[j - 1] + 1, prev[j] + 1),
                prev[j - 1] + cost,
            );
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    prev[n]
}

// ----- Annotation checking -----

/// Check that every named-type reference on an `Emit` payload's inline
/// fields (`name :: TypeName` inside the brace-form field list, recorded as
/// `TypeExpr::Named` on an `EventField.type_expr`) resolves to an Entity- or
/// Struct-shaped alias (or the orbital's own entity by name). Unknown names
/// surface as `UnknownType`; resolved kinds other than Entity/Struct surface
/// as `KindMismatch`.
pub fn check_emit_payload_types(
    emit: &Emit,
    ctx: &ResolveCtx<'_>,
    own_entity_names: &BTreeSet<String>,
    trait_type_params: &BTreeSet<String>,
) -> Result<(), TypeError> {
    for f in &emit.payload {
        if let TypeExpr::Named(name) = &f.type_expr {
            check_named_payload_ref(name.as_str(), f.span, ctx, own_entity_names, trait_type_params)?;
        }
    }
    Ok(())
}

fn check_named_payload_ref(
    name: &str,
    span: Span,
    ctx: &ResolveCtx<'_>,
    own_entity_names: &BTreeSet<String>,
    trait_type_params: &BTreeSet<String>,
) -> Result<(), TypeError> {
    if own_entity_names.contains(name) || trait_type_params.contains(name) {
        return Ok(());
    }
    let resolved = resolve_named(name, &[], ctx, span)?;
    match resolved {
        TypeBody::Entity { .. } | TypeBody::Struct(_) => Ok(()),
        _ => Err(TypeError::KindMismatch {
            expected: TypeKindName::Entity,
            actual: kind_label(kind_from_alias(ctx, name)),
            span,
        }),
    }
}

/// Check an `::` annotation on an entity body. Returns on success or maps
/// mismatches to typed errors.
pub fn check_entity_annotation(
    ann: &TypeAnnotation,
    entity: &Entity,
    ctx: &ResolveCtx<'_>,
) -> Result<(), TypeError> {
    let resolved = resolve_named(&ann.type_name, &ann.type_args, ctx, ann.span)?;
    let TypeBody::Entity {
        fields: required_fields,
        ..
    } = resolved
    else {
        return Err(TypeError::KindMismatch {
            expected: TypeKindName::Entity,
            actual: kind_label(kind_from_alias(ctx, &ann.type_name)),
            span: ann.span,
        });
    };
    for rf in required_fields {
        let present = entity.fields.iter().any(|b| b.name == rf.name);
        if !present {
            return Err(TypeError::AnnotationMismatch {
                type_name: ann.type_name.clone(),
                what: format!("body missing field {:?} required by type", rf.name),
                span: ann.span,
            });
        }
    }
    Ok(())
}

/// Check an `::` annotation on a trait body (reference form). For Stage 2 we
/// verify the resolved kind is Trait, and when the type carries a concrete
/// `upstream` / `linked_entity`, the body agrees with both.
///
/// Increment 3-II reinterpretation: on a trait REFERENCE (`trt.reference` is
/// `Some`), the `::` surface is ambiguous between the pre-existing
/// alias-conformance meaning (`:: AliasName arg`) and NEW call-site
/// type-parameter arguments (`:: v string`, keyed onto the referenced source
/// trait's OWN declared params — see `lower::trait::lower_trait_reference`).
/// The source trait's parameter list is not visible here (it may live in
/// another behavior package entirely, resolved only at compile time by the
/// L2 inline phase — see `docs/Almadar_LOLO.md`). The disambiguation this
/// layer CAN make: `ann.type_name` resolving as a genuine known type alias in
/// `ctx` is unambiguous — keep the existing conformance check. When it does
/// NOT resolve as a known alias, on a reference we defer entirely rather than
/// raising `UnknownType` — it is a legitimate param-binding candidate that
/// only the compiler can confirm or reject once the source trait is loaded.
/// Inline (non-reference) trait declarations are UNAFFECTED: `::` always
/// means alias-conformance there (a trait's OWN params use the `(v : Type)`
/// declaration syntax instead).
pub fn check_trait_annotation(
    ann: &TypeAnnotation,
    trt: &Trait,
    ctx: &ResolveCtx<'_>,
) -> Result<(), TypeError> {
    if trt.reference.is_some() && ctx.lookup(&ann.type_name).is_none() {
        return Ok(());
    }
    let resolved = resolve_named(&ann.type_name, &ann.type_args, ctx, ann.span)?;
    let TypeBody::Trait {
        upstream: type_upstream,
        linked_entity: type_linked,
    } = resolved
    else {
        return Err(TypeError::KindMismatch {
            expected: TypeKindName::Trait,
            actual: kind_label(kind_from_alias(ctx, &ann.type_name)),
            span: ann.span,
        });
    };
    if let Some(expected_upstream) = type_upstream {
        let body_upstream = trt.reference.as_ref().map(|r| r.reference.clone());
        if body_upstream.as_deref() != Some(expected_upstream.as_str()) {
            return Err(TypeError::AnnotationMismatch {
                type_name: ann.type_name.clone(),
                what: format!(
                    "trait upstream ref {:?} does not match type's {:?}",
                    body_upstream.unwrap_or_default(),
                    expected_upstream
                ),
                span: ann.span,
            });
        }
    }
    if let Some(TypeExpr::Named(expected_entity)) = type_linked {
        let body_linked = trt.linked_entity.as_deref().unwrap_or_default();
        if body_linked != expected_entity {
            return Err(TypeError::AnnotationMismatch {
                type_name: ann.type_name.clone(),
                what: format!(
                    "trait linkedEntity {body_linked:?} does not match type's {expected_entity:?}"
                ),
                span: ann.span,
            });
        }
    }
    Ok(())
}

/// Check an `::` annotation on a page. Stage 2: only the kind is verified.
pub fn check_page_annotation(
    ann: &TypeAnnotation,
    _page: &Page,
    ctx: &ResolveCtx<'_>,
) -> Result<(), TypeError> {
    let resolved = resolve_named(&ann.type_name, &ann.type_args, ctx, ann.span)?;
    if !matches!(resolved, TypeBody::Page { .. }) {
        return Err(TypeError::KindMismatch {
            expected: TypeKindName::Page,
            actual: kind_label(kind_from_alias(ctx, &ann.type_name)),
            span: ann.span,
        });
    }
    Ok(())
}

/// Check an `::` annotation on an orbital. Stage 2: only the kind is verified.
pub fn check_orbital_annotation(
    ann: &TypeAnnotation,
    _orb: &Orbital,
    ctx: &ResolveCtx<'_>,
) -> Result<(), TypeError> {
    let resolved = resolve_named(&ann.type_name, &ann.type_args, ctx, ann.span)?;
    if !matches!(resolved, TypeBody::Orbital { .. }) {
        return Err(TypeError::KindMismatch {
            expected: TypeKindName::Orbital,
            actual: kind_label(kind_from_alias(ctx, &ann.type_name)),
            span: ann.span,
        });
    }
    Ok(())
}

/// Every entity name an orbital's `expects entity` / `expects identity`
/// declaration promises. The named entity is supplied by a sibling orbital at
/// compose time, so its name is a legal field-type reference (`field :
/// Customer`, `[Customer]`) even with no local `entity` block. L1 mirror of
/// the compiler's `expectations::expected_entity_names`.
pub(crate) fn expected_entity_names(orb: &Orbital) -> Vec<&str> {
    orb.expects
        .iter()
        .filter_map(|e| match &e.kind {
            ExpectsKind::Entity { name, .. } => Some(name.as_str()),
            ExpectsKind::Identity { name, .. } => name.as_deref(),
            ExpectsKind::Event { .. } => None,
        })
        .collect()
}

/// Walk the whole program and run resolution + annotation checks. Runs after
/// the lowerer has collected both scopes; failures are returned as the first
/// error encountered.
pub fn check_program(program: &Program) -> Result<(), TypeError> {
    let mut file = TypeRegistry::new();
    for ta in &program.type_aliases {
        file.insert(ta.clone());
    }
    // All declared entity names — legal targets for a field's relation/type
    // reference (`field : OtherEntity`), alongside primitives and type aliases.
    // `expects` names count too: the declaration is a promise the entity
    // exists at compose time.
    let mut entity_names: BTreeSet<String> = program
        .orbitals
        .iter()
        .flat_map(|o| o.entity.iter().chain(o.extra_entities.iter()))
        .map(|e| e.name.clone())
        .collect();
    for o in &program.orbitals {
        entity_names.extend(expected_entity_names(o).into_iter().map(str::to_string));
    }

    // Pre-build per-orbital scoped registries so cycle detection + annotation
    // checks see both scopes consistently.
    for orb in &program.orbitals {
        let mut orbital = TypeRegistry::new();
        for ta in &orb.type_aliases {
            orbital.insert(ta.clone());
        }
        let ctx = ResolveCtx {
            file: &file,
            orbital: Some(&orbital),
        };
        detect_cycles(&ctx)?;
        if let Some(ann) = &orb.type_annotation {
            check_orbital_annotation(ann, orb, &ctx)?;
        }
        for entity in orb.entity.iter().chain(orb.extra_entities.iter()) {
            if let Some(ann) = &entity.type_annotation {
                check_entity_annotation(ann, entity, &ctx)?;
            }
            for f in &entity.fields {
                check_field_type_refs(&f.type_expr, f.span, &ctx, &entity_names, &BTreeSet::new())?;
            }
        }
        for trt in &orb.traits {
            if let Some(ann) = &trt.type_annotation {
                check_trait_annotation(ann, trt, &ctx)?;
            }
            // Increment 3-II: a trait's own declared type-parameter names
            // (`(v : Type, ...)`) are legal bare references in its config /
            // emit-payload type positions — they are neither aliases nor
            // entity names, so without this exclusion every reference would
            // spuriously raise `UnknownType`.
            let trait_type_params: BTreeSet<String> =
                trt.type_params.iter().map(|p| p.name.clone()).collect();
            for cf in &trt.config {
                check_field_type_refs(&cf.type_expr, cf.span, &ctx, &entity_names, &trait_type_params)?;
            }
            // Validate emit-payload type references against the registry.
            // Entity- or struct-kind only. Other kinds (trait/page/orbital)
            // are a KindMismatch. Missing names propagate UnknownType.
            // Also permits the orbital's own entity names (primary and
            // extras), which are not in the type registry but are legal
            // payload references, and any of this trait's own declared
            // type-parameter names.
            let own_entity_names: BTreeSet<String> = orb
                .entity
                .iter()
                .chain(orb.extra_entities.iter())
                .map(|e| e.name.clone())
                .collect();
            for emit in &trt.emits {
                check_emit_payload_types(emit, &ctx, &own_entity_names, &trait_type_params)?;
            }
        }
        for page in &orb.pages {
            if let Some(ann) = &page.type_annotation {
                check_page_annotation(ann, page, &ctx)?;
            }
        }
    }

    // Also run cycle detection for file-scope-only aliases (when a program
    // has zero orbitals, the loop above does nothing).
    if program.orbitals.is_empty() {
        let ctx = ResolveCtx {
            file: &file,
            orbital: None,
        };
        detect_cycles(&ctx)?;
    }
    Ok(())
}

fn kind_from_alias(ctx: &ResolveCtx<'_>, name: &str) -> TypeKindIndex {
    ctx.lookup(name)
        .map(|a| kind_of(&a.body))
        .unwrap_or(TypeKindIndex::Alias)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse_str;

    fn check(src: &str) -> Result<(), TypeError> {
        let program = parse_str(src).expect("parse");
        check_program(&program)
    }

    #[test]
    fn ok_entity_shape_alias() {
        let src = r#"
type Auditable = Entity {
  id : string!
  createdAt : string
}

orbital OrbA {
  entity Rec :: Auditable [persistent: recs] {
    id : string!
    createdAt : string
    note : string
  }
  trait RecTrait -> Rec [interaction] {
    state idle { INIT -> idle (ref Rec) }
  }
  page "/recs" -> RecTrait
}
"#;
        check(src).unwrap();
    }

    #[test]
    fn err_unknown_type() {
        let src = r#"
orbital OrbX {
  entity Foo :: DoesNotExist [persistent: foos] {
    id : string!
  }
  trait FooTrait -> Foo [interaction] { state idle { INIT -> idle (ref Foo) } }
  page "/foos" -> FooTrait
}
"#;
        let err = check(src).unwrap_err();
        assert!(matches!(err, TypeError::UnknownType { .. }));
    }

    #[test]
    fn err_unknown_entity_field_type_typo() {
        // G-LOLO-TYPENAME-SILENT-FALLBACK: a typo'd field type name must be
        // rejected (not silently lowered to `string`).
        let src = r#"
orbital OrbY {
  entity Order [persistent: orders] {
    id : string!
    status : Statsu
  }
  trait OrderTrait -> Order [interaction] { state idle { INIT -> idle (ref Order) } }
  page "/orders" -> OrderTrait
}
"#;
        let err = check(src).unwrap_err();
        assert!(matches!(err, TypeError::UnknownType { .. }), "got {err:?}");
    }

    #[test]
    fn ok_expects_entity_name_is_a_legal_field_type() {
        // `expects entity Customer` promises the entity exists at compose
        // time, so its name is a legal field-type reference standalone —
        // both the bare and the array form.
        let src = r#"
orbital OrbE {
  expects entity Customer
  entity Order [persistent: orders] {
    id : string!
    customer : Customer
    watchers : [Customer]
  }
  trait OrderTrait -> Order [interaction] { state idle { INIT -> idle (ref Order) } }
  page "/orders" -> OrderTrait
}
"#;
        check(src).unwrap();
    }

    #[test]
    fn err_field_type_neither_declared_nor_expected() {
        let src = r#"
orbital OrbF {
  expects entity Customer
  entity Order [persistent: orders] {
    id : string!
    customer : Nonexistent
  }
  trait OrderTrait -> Order [interaction] { state idle { INIT -> idle (ref Order) } }
  page "/orders" -> OrderTrait
}
"#;
        let err = check(src).unwrap_err();
        assert!(matches!(err, TypeError::UnknownType { .. }), "got {err:?}");
    }

    #[test]
    fn ok_documented_string_primitives() {
        // email/url/phone/uuid are recognized (lower to string), not "unknown".
        let src = r#"
orbital OrbZ {
  entity Contact [persistent: contacts] {
    id   : uuid!
    mail : email
    site : url
    tel  : phone
  }
  trait ContactTrait -> Contact [interaction] { state idle { INIT -> idle (ref Contact) } }
  page "/contacts" -> ContactTrait
}
"#;
        check(src).unwrap();
    }

    #[test]
    fn ok_named_alias_field_resolves() {
        // a valid named alias as a field type resolves — not falsely rejected by
        // the new unknown-type check.
        let src = r#"
type Addr = Entity {
  street : string!
}

orbital OrbW {
  entity Item [persistent: items] {
    id : string!
    addr : Addr
  }
  trait ItemTrait -> Item [interaction] { state idle { INIT -> idle (ref Item) } }
  page "/items" -> ItemTrait
}
"#;
        check(src).unwrap();
    }

    #[test]
    fn err_missing_required_field() {
        let src = r#"
type Required = Entity {
  id : string!
  name : string!
}

orbital OrbY {
  entity Foo :: Required [persistent: foos] {
    id : string!
  }
  trait FooTrait -> Foo [interaction] { state idle { INIT -> idle (ref Foo) } }
  page "/foos" -> FooTrait
}
"#;
        let err = check(src).unwrap_err();
        assert!(matches!(err, TypeError::AnnotationMismatch { .. }));
    }

    #[test]
    fn err_kind_mismatch_composition() {
        let src = r#"
type Foo = Entity { id : string! }
type Bar = Trait -> Foo
type Mixed = Foo & Bar

orbital OrbZ {
  entity Rec :: Mixed [persistent: recs] { id : string! }
  trait R -> Rec [interaction] { state s { INIT -> s (ref Rec) } }
  page "/rs" -> R
}
"#;
        let err = check(src).unwrap_err();
        assert!(matches!(err, TypeError::KindMismatch { .. }));
    }

    #[test]
    fn ok_entity_composition_merges_fields() {
        let src = r#"
type Timestamped = Entity {
  id : string!
  createdAt : string
}
type Auditable = Timestamped & Entity {
  createdBy : string
}

orbital OrbC {
  entity Rec :: Auditable [persistent: recs] {
    id : string!
    createdAt : string
    createdBy : string
    note : string
  }
  trait RTrait -> Rec [interaction] {
    state idle {
      INIT -> idle
        (ref Rec)
    }
  }
  page "/recs" -> RTrait
}
"#;
        check(src).unwrap();
    }

    #[test]
    fn err_unknown_type_with_suggestion() {
        let src = r#"
type Auditable = Entity {
  id : string!
}

orbital OrbW {
  entity Foo :: Auditible [persistent: foos] {
    id : string!
  }
  trait FooTrait -> Foo [interaction] {
    state idle {
      INIT -> idle
        (ref Foo)
    }
  }
  page "/foos" -> FooTrait
}
"#;
        let err = check(src).unwrap_err();
        match err {
            TypeError::UnknownType { suggestion, .. } => {
                assert_eq!(suggestion.as_deref(), Some("Auditable"));
            }
            other => panic!("expected UnknownType, got {other:?}"),
        }
    }

    #[test]
    fn ok_parenthesized_kinded_param_applies_like_bare_param() {
        // `(e : Entity)` must be bindable within the alias body exactly like
        // the legacy bare-ident param form — a `::` call site supplying a
        // concrete entity name resolves without an UnknownType error.
        let src = r#"
type Wrapped (e : Entity) = Entity {
  id : string!
  payload : E
}

orbital OrbP {
  expects entity Inner
  entity Outer :: Wrapped Inner [persistent: outers] {
    id : string!
    payload : Inner
  }
  trait OuterTrait -> Outer [interaction] { state idle { INIT -> idle (ref Outer) } }
  page "/outers" -> OuterTrait
}
"#;
        check(src).unwrap();
    }

    #[test]
    fn err_cycle() {
        // Two aliases reference each other through entity fields.
        let src = r#"
type A = Entity { other : B }
type B = Entity { back : A }

orbital Dummy {
  entity E [runtime] { id : string! }
  trait T -> E [interaction] { state s { INIT -> s (ref E) } }
  page "/es" -> T
}
"#;
        let err = check(src).unwrap_err();
        assert!(matches!(err, TypeError::CircularType { .. }));
    }
}
