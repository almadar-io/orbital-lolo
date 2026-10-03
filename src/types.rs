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
    ConfigField, Emit, Entity, EntityField, ExpectsKind, Orbital, Page, PayloadOfTarget,
    Persistence, Program, Span, Trait, TypeAlias, TypeAnnotation, TypeBody, TypeExpr,
};
use crate::error::{TypeError, TypeKindName};
use orbital_core::schema::SExpression;

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
        TypeExpr::EventPayloadOf(PayloadOfTarget::ConfigKnob(knob)) => format!("@payload({knob})"),
        TypeExpr::EventPayloadOf(PayloadOfTarget::SiblingMember(member)) => {
            format!("@payload(.{member})")
        }
        TypeExpr::NamedEntityRow(knob) => format!("@entity({knob})"),
        TypeExpr::ExpectedIdentityRow => "@user".to_string(),
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
        TypeExpr::FieldOf { ty, field, .. } => format!("{ty}.{field}"),
        TypeExpr::RelationMany(n) => format!("{n}*"),
        TypeExpr::RelationManyNonEmpty(n) => format!("{n}+"),
        TypeExpr::Trait => "trait".to_string(),
        TypeExpr::EntityRef => "entity".to_string(),
        TypeExpr::EventRef => "event".to_string(),
        TypeExpr::Slot => "slot".to_string(),
        TypeExpr::Pattern => "pattern".to_string(),
        TypeExpr::Secret => "secret".to_string(),
        TypeExpr::SExpr => "SExpr".to_string(),
        TypeExpr::Match { knob, arms } => {
            let mut parts: Vec<String> = arms
                .iter()
                .map(|a| format!("{:?}:{}", a.pattern.spelling(), format_type_expr(&a.ty)))
                .collect();
            parts.sort();
            format!("match:{knob}:{{{}}}", parts.join(","))
        }
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
        // Sigil type constructors reference no NAMED type — their argument is a
        // config knob, resolved at compile time, not a type reference.
        TypeExpr::EventPayloadOf(_)
        | TypeExpr::NamedEntityRow(_)
        | TypeExpr::ExpectedIdentityRow => {}
        TypeExpr::RelationMany(n) | TypeExpr::RelationManyNonEmpty(n) => {
            out.insert(n.clone());
        }
        TypeExpr::FieldOf { ty, .. } => {
            out.insert(ty.clone());
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
        // Visit every arm's type — v1 caps arms to Primitive/[Primitive]
        // (no NAMED refs reach here in a well-formed match), but a
        // not-yet-validated match shouldn't hide a reference from cycle
        // detection.
        TypeExpr::Match { arms, .. } => {
            for arm in arms {
                visit_expr_refs(&arm.ty, out);
            }
        }
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
            check_named_payload_ref(
                name.as_str(),
                f.span,
                ctx,
                own_entity_names,
                trait_type_params,
            )?;
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
        // A named TYPE union (`type DecisionMatch = scalar | [float]`) is
        // legal in payload position: the same union authored inline lowers
        // there already (`union_payload_variants`), and an alias must stay
        // transparent to it. A vocabulary (`up | down`) is not a type union
        // and keeps its enum reading — rejected here as before.
        TypeBody::Alias(TypeExpr::Union(parts))
            if crate::lower::entity::is_type_union_with(&parts, |n| {
                ctx.lookup(n)
                    .map(|a| matches!(&a.body, TypeBody::Struct(_)))
            }) =>
        {
            Ok(())
        }
        // `type Step = StepCall | StepResult` parses as a bare-name union; when
        // every name is a struct it is the same type union (a tagged one).
        TypeBody::Enum(names)
            if crate::lower::entity::is_type_union_with(
                &names
                    .iter()
                    .map(|n| TypeExpr::Named(n.clone()))
                    .collect::<Vec<_>>(),
                |n| {
                    ctx.lookup(n)
                        .map(|a| matches!(&a.body, TypeBody::Struct(_)))
                },
            ) && names.iter().all(|n| ctx.lookup(n).is_some()) =>
        {
            Ok(())
        }
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
            // Not an entity name — a route.
            ExpectsKind::Event { .. } | ExpectsKind::Page { .. } => None,
        })
        .collect()
}

/// Walk the whole program and run resolution + annotation checks. Runs after
/// the lowerer has collected both scopes; failures are returned as the first
/// error encountered.
/// Gate C's L1 half: the compiled-path validator (`ORB_T_GENERIC_TYPE_DEPRECATED`,
/// hard error since 2026-09-01) rejects generic types in every declaration
/// position the `.orb` carries — but the L1 lowerer ERASES an authored scalar
/// `json`/`any` entity field to `string` before that gate can see it, so the
/// authoring site was accepted-and-lying. Rejecting the SPELLING here (rung 1
/// of the prevention ladder) closes that loophole at its source; `scalar`,
/// `node`, and shaped types are the sanctioned vocabulary.
fn reject_generic_types(te: &TypeExpr, position: &str, span: Span) -> Result<(), TypeError> {
    match te.peel() {
        TypeExpr::Primitive(n) | TypeExpr::Named(n)
            if n == "json" || n == "any" || n == "object" || n == "array" =>
        {
            Err(TypeError::GenericTypeForbidden {
                name: n.clone(),
                position: position.to_string(),
                span,
            })
        }
        TypeExpr::Array(t) | TypeExpr::Optional(t) => reject_generic_types(t, position, span),
        TypeExpr::Map(k, v) => {
            reject_generic_types(k, position, span)?;
            reject_generic_types(v, position, span)
        }
        TypeExpr::Object(fields) => {
            for (_, t, _) in fields {
                reject_generic_types(t, position, span)?;
            }
            Ok(())
        }
        TypeExpr::Union(parts) => {
            for t in parts {
                reject_generic_types(t, position, span)?;
            }
            Ok(())
        }
        TypeExpr::Match { arms, .. } => {
            for arm in arms {
                reject_generic_types(&arm.ty, position, arm.span)?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

fn reject_generic_alias_body(ta: &TypeAlias) -> Result<(), TypeError> {
    let position = format!("type alias {:?}", ta.name);
    match &ta.body {
        TypeBody::Alias(te) => reject_generic_types(te, &position, ta.span),
        TypeBody::Struct(fields) => {
            for (_, te, _) in fields {
                reject_generic_types(te, &position, ta.span)?;
            }
            Ok(())
        }
        TypeBody::Entity { fields, .. } => {
            for f in fields {
                reject_generic_types(&f.type_expr, &position, f.span)?;
            }
            Ok(())
        }
        TypeBody::Event { payload, .. } => reject_generic_types(payload, &position, ta.span),
        _ => Ok(()),
    }
}

/// Validate every `@payload <sibling>` member of a `type X = { ... }` struct
/// body: `sibling` must be a field declared in the SAME struct, typed
/// `event`. Structurally decidable from the alias body alone — rung 1 of the
/// prevention ladder, same posture as `reject_generic_types` — so this runs
/// wherever that does, before the dependent type erases to a shapeless
/// object at lowering (`lower::entity::lower_type_expr`).
fn check_payload_sibling_refs(ta: &TypeAlias) -> Result<(), TypeError> {
    let TypeBody::Struct(fields) = &ta.body else {
        return Ok(());
    };
    for (member, te, _) in fields {
        let TypeExpr::EventPayloadOf(PayloadOfTarget::SiblingMember(sibling)) = te else {
            continue;
        };
        match fields.iter().find(|(name, _, _)| name == sibling) {
            None => {
                return Err(TypeError::PayloadSiblingUnresolved {
                    member: member.clone(),
                    sibling: sibling.clone(),
                    reason: "names a sibling that does not exist on this struct".to_string(),
                    span: ta.span,
                });
            }
            Some((_, TypeExpr::EventRef, _)) => {}
            Some(_) => {
                return Err(TypeError::PayloadSiblingUnresolved {
                    member: member.clone(),
                    sibling: sibling.clone(),
                    reason: "names a sibling that is not typed `event`".to_string(),
                    span: ta.span,
                });
            }
        }
    }
    Ok(())
}

/// Payload field NAMES declared for each event, gathered from every `emits`
/// block in the program. First declaration wins — the same anchor rule
/// `orbital-compiler::phases::inline::sigil_types::build_payload_index` uses
/// on the resolved schema, applied here at L1 against the AST so a literal
/// `@payload <sibling>` instance can be checked before the dependent type
/// erases to a shapeless object at lowering.
fn collect_event_payload_fields(program: &Program) -> BTreeMap<String, BTreeSet<String>> {
    let mut idx: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for orb in &program.orbitals {
        for trt in &orb.traits {
            for emit in &trt.emits {
                idx.entry(emit.event.clone())
                    .or_insert_with(|| emit.payload.iter().map(|f| f.name.clone()).collect());
            }
        }
    }
    idx
}

/// The `(member, sibling)` pair of a struct's `@payload <sibling>` member,
/// when it has one. `check_payload_sibling_refs` already proved `sibling`
/// exists and is `event`-typed by the time this is called.
fn payload_sibling_member(fields: &[(String, TypeExpr, bool)]) -> Option<(&str, &str)> {
    fields.iter().find_map(|(name, te, _)| match te {
        TypeExpr::EventPayloadOf(PayloadOfTarget::SiblingMember(sibling)) => {
            Some((name.as_str(), sibling.as_str()))
        }
        _ => None,
    })
}

/// Unwrap `[T]` / `?T` and resolve a named struct alias to its field list.
/// `None` for anything that isn't (transitively) a struct alias — a bare
/// inline `{ ... }` config type isn't covered (no call site outside the 3
/// sanctioned `@almadar/ui` molecules authors this dependent type inline
/// today; extend here if that changes).
fn struct_alias_fields<'a>(
    te: &TypeExpr,
    ctx: &ResolveCtx<'a>,
) -> Option<&'a [(String, TypeExpr, bool)]> {
    match te.peel() {
        TypeExpr::Array(inner) | TypeExpr::Optional(inner) => struct_alias_fields(inner, ctx),
        TypeExpr::Named(name) => match &ctx.lookup(name)?.body {
            TypeBody::Struct(fields) => Some(fields.as_slice()),
            _ => None,
        },
        _ => None,
    }
}

fn sexpr_as_object(e: &SExpression) -> Option<&HashMap<String, SExpression>> {
    match e {
        SExpression::Object(o) => Some(o),
        _ => None,
    }
}

/// B2: validate a struct-typed config knob's LITERAL default instances (one
/// object, or an array of them) against the dependent `@payload <sibling>`
/// member's ACTUALLY DECLARED event payload — the value this dependent type
/// exists for (the recursive generic alias it replaces validated nothing).
/// Resolvable only where the sibling's literal value on that SAME instance is
/// a plain string — an instance whose sibling value isn't a literal (e.g.
/// itself forwarded via `@config.*`) defers, same posture as the top-level
/// knob form only resolving a literal default.
fn check_payload_sibling_literals(
    cf: &ConfigField,
    ctx: &ResolveCtx<'_>,
    payload_index: &BTreeMap<String, BTreeSet<String>>,
) -> Result<(), TypeError> {
    let Some(struct_fields) = struct_alias_fields(&cf.type_expr, ctx) else {
        return Ok(());
    };
    let Some((member, sibling)) = payload_sibling_member(struct_fields) else {
        return Ok(());
    };
    let Some(default) = &cf.default else {
        return Ok(());
    };
    let items: Vec<&HashMap<String, SExpression>> = match default {
        SExpression::Array(items) => items.iter().filter_map(sexpr_as_object).collect(),
        SExpression::Object(obj) => vec![obj],
        _ => Vec::new(),
    };
    for item in items {
        let Some(SExpression::String(event_name)) = item.get(sibling) else {
            continue;
        };
        let Some(declared_fields) = payload_index.get(event_name) else {
            continue;
        };
        let Some(payload_value) = item.get(member).and_then(sexpr_as_object) else {
            continue;
        };
        for key in payload_value.keys() {
            if !declared_fields.contains(key) {
                return Err(TypeError::PayloadSiblingUnresolved {
                    member: member.to_string(),
                    sibling: sibling.to_string(),
                    reason: format!(
                        "carries a literal instance whose field {key:?} is not on {event_name:?}'s declared payload"
                    ),
                    span: cf.span,
                });
            }
        }
    }
    Ok(())
}

/// The declared type at `trail` (`""`, `"[0].event"`) inside a value of type `te`.
fn type_at_trail<'a>(te: &'a TypeExpr, trail: &str, ctx: &ResolveCtx<'a>) -> Option<&'a TypeExpr> {
    let te = te.peel();
    let te = match te {
        TypeExpr::Optional(inner) => inner.as_ref(),
        TypeExpr::Named(name) => match &ctx.lookup(name)?.body {
            TypeBody::Alias(inner) => return type_at_trail(inner, trail, ctx),
            _ => te,
        },
        _ => te,
    };
    if trail.is_empty() {
        return Some(te);
    }
    if let Some(rest) = trail.strip_prefix('[') {
        let (_, rest) = rest.split_once(']')?;
        return match te {
            TypeExpr::Array(inner) => type_at_trail(inner, rest, ctx),
            _ => None,
        };
    }
    let rest = trail.strip_prefix('.')?;
    let end = rest.find(['.', '[']).unwrap_or(rest.len());
    let (key, rest) = rest.split_at(end);
    let member = match te {
        TypeExpr::Object(fields) => fields.iter().find(|(n, _, _)| n == key).map(|(_, t, _)| t),
        TypeExpr::Named(name) => match &ctx.lookup(name)?.body {
            TypeBody::Struct(fields) => fields.iter().find(|(n, _, _)| n == key).map(|(_, t, _)| t),
            _ => None,
        },
        _ => None,
    }?;
    type_at_trail(member, rest, ctx)
}

/// A dotted event atom (`Trait.EVENT`, `Orbital.Trait.EVENT`) in a declared
/// config default must sit at an `event`-typed position. A call-site override
/// carries no declared type here; its merged knob is checked at L2.
fn check_event_ref_positions(cf: &ConfigField, ctx: &ResolveCtx<'_>) -> Result<(), TypeError> {
    if matches!(cf.type_expr.peel(), TypeExpr::Primitive(p) if p == "unknown") {
        return Ok(());
    }
    for (trail, span) in &cf.event_refs {
        if !matches!(type_at_trail(&cf.type_expr, trail, ctx), Some(TypeExpr::EventRef)) {
            return Err(TypeError::EventRefPosition {
                knob: cf.name.clone(),
                trail: trail.clone(),
                span: *span,
            });
        }
    }
    Ok(())
}

pub fn check_program(program: &Program) -> Result<(), TypeError> {
    let mut file = TypeRegistry::new();
    for ta in &program.type_aliases {
        reject_generic_alias_body(ta)?;
        check_payload_sibling_refs(ta)?;
        file.insert(ta.clone());
    }
    let payload_index = collect_event_payload_fields(program);
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
            reject_generic_alias_body(ta)?;
            check_payload_sibling_refs(ta)?;
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
                reject_generic_types(
                    &f.type_expr,
                    &format!("entity {:?} field {:?}", entity.name, f.name),
                    f.span,
                )?;
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
            // Gap C: a declared param default must resolve — a primitive
            // name (incl. `scalar`), an entity name, or a struct-alias name
            // declared in this atom's own file. Same value vocabulary a
            // call-site `::` arg accepts (`resolve_type_arg_value` in the
            // compiler mirrors this at L2).
            for p in &trt.type_params {
                if let Some(default) = &p.default {
                    if is_primitive(default)
                        || entity_names.contains(default)
                        || ctx.lookup(default).is_some()
                    {
                        continue;
                    }
                    let mut haystack = ctx.known_names();
                    haystack.extend(entity_names.iter().cloned());
                    return Err(TypeError::UnknownType {
                        name: default.clone(),
                        suggestion: closest_name(default, &haystack),
                        span: trt.span,
                    });
                }
            }
            for cf in &trt.config {
                reject_generic_types(
                    &cf.type_expr,
                    &format!("trait {:?} config {:?}", trt.name, cf.name),
                    cf.span,
                )?;
                check_field_type_refs(
                    &cf.type_expr,
                    cf.span,
                    &ctx,
                    &entity_names,
                    &trait_type_params,
                )?;
                check_payload_sibling_literals(cf, &ctx, &payload_index)?;
                check_event_ref_positions(cf, &ctx)?;
            }
            for emit in &trt.emits {
                for f in &emit.payload {
                    reject_generic_types(
                        &f.type_expr,
                        &format!(
                            "trait {:?} emit {:?} payload field {:?}",
                            trt.name, emit.event, f.name
                        ),
                        f.span,
                    )?;
                }
            }
            for listen in &trt.listens {
                for f in &listen.payload {
                    reject_generic_types(
                        &f.type_expr,
                        &format!(
                            "trait {:?} listen {:?} payload field {:?}",
                            trt.name, listen.event, f.name
                        ),
                        f.span,
                    )?;
                }
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
        for exp in &orb.expects {
            let shape = match &exp.kind {
                crate::ast::ExpectsKind::Identity { shape, .. }
                | crate::ast::ExpectsKind::Entity { shape, .. } => shape.as_ref(),
                _ => None,
            };
            for f in shape.into_iter().flatten() {
                reject_generic_types(
                    &f.type_expr,
                    &format!("expects contract field {:?}", f.name),
                    f.span,
                )?;
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

    /// Gate C's L1 half: authored `json`/`any`/bare `object` are rejected at
    /// the type system, in every declaration position — closing the erasure
    /// loophole where a scalar `json` entity field lowered to `string` before
    /// the compiled-path gate could see it.
    #[test]
    fn err_generic_types_rejected_in_every_authoring_position() {
        let positions = [
            ("data : json", "entity Order [persistent: orders] {\n    id : string!\n    data : json\n  }"),
            ("rows : [any]", "entity Order [persistent: orders] {\n    id : string!\n    rows : [any]\n  }"),
            ("alias member", "entity Order [persistent: orders] { id : string! }\n  type Node = { name : string!, children : [object] }"),
        ];
        for (label, body) in positions {
            let src = format!(
                r#"
orbital OrbG {{
  {body}
  trait OrderTrait -> Order [interaction] {{ state idle {{ INIT -> idle (ref Order) }} }}
  page "/orders" -> OrderTrait
}}
"#
            );
            let err = check(&src).unwrap_err();
            assert!(
                matches!(err, TypeError::GenericTypeForbidden { .. }),
                "{label}: got {err:?}"
            );
        }
    }

    #[test]
    fn err_generic_type_rejected_in_payload_and_config() {
        let src = r#"
orbital OrbH {
  entity Rec [persistent: recs] { id : string! }
  trait RecTrait -> Rec [interaction] {
    config { extra : json }
    state idle { INIT -> idle (ref Rec) }
  }
  page "/recs" -> RecTrait
}
"#;
        let err = check(src).unwrap_err();
        assert!(
            matches!(err, TypeError::GenericTypeForbidden { .. }),
            "config: got {err:?}"
        );

        let src = r#"
orbital OrbI {
  entity Rec [persistent: recs] { id : string! }
  trait RecTrait -> Rec [interaction] {
    emits { DONE -> external { value : any! } }
    state idle { INIT -> idle (ref Rec) }
  }
  page "/recs" -> RecTrait
}
"#;
        let err = check(src).unwrap_err();
        assert!(
            matches!(err, TypeError::GenericTypeForbidden { .. }),
            "emit payload: got {err:?}"
        );
    }

    /// Gap: bare `array` (no element type) erased silently to a `type:
    /// "array"` payload field with no element info — `.lolo` has no bare
    /// `array` type (only `[T]`, `Almadar_LOLO.md` §"Type expressions"), so
    /// this is the same erasure loophole `json`/`any`/`object` closed above,
    /// covering the FOUR authoring positions the corpus actually has bare
    /// `array` in: entity field, config knob, emit payload, listen payload.
    #[test]
    fn err_bare_array_rejected_in_every_authoring_position() {
        let positions = [
            (
                "entity field",
                "entity Rec [persistent: recs] { id : string!\n    files : array! }",
                "trait RecTrait -> Rec [interaction] { state idle { INIT -> idle (ref Rec) } }",
            ),
            (
                "config knob",
                "entity Rec [persistent: recs] { id : string! }",
                "trait RecTrait -> Rec [interaction] {\n    config { files : array }\n    state idle { INIT -> idle (ref Rec) }\n  }",
            ),
            (
                "emit payload",
                "entity Rec [persistent: recs] { id : string! }",
                "trait RecTrait -> Rec [interaction] {\n    emits { UPLOAD -> external { files : array! } }\n    state idle { INIT -> idle (ref Rec) }\n  }",
            ),
            (
                "listen payload",
                "entity Rec [persistent: recs] { id : string! }",
                "trait RecTrait -> Rec [interaction] {\n    listens { UPLOAD { files : array! } }\n    state idle { INIT -> idle (ref Rec) }\n  }",
            ),
        ];
        for (label, entity, trait_body) in positions {
            let src = format!(
                r#"
orbital OrbArr {{
  {entity}
  {trait_body}
  page "/recs" -> RecTrait
}}
"#
            );
            let err = check(&src).unwrap_err();
            assert!(
                matches!(err, TypeError::GenericTypeForbidden { ref name, .. } if name == "array"),
                "{label}: got {err:?}"
            );
        }
    }

    /// The properly-spelled `[T]` array survives unchanged — only the bare
    /// (elementless) spelling is rejected.
    #[test]
    fn ok_typed_array_field_still_passes() {
        let src = r#"
orbital OrbArrOk {
  entity Rec [persistent: recs] { id : string!
    files : [string]! }
  trait RecTrait -> Rec [interaction] {
    listens { UPLOAD { files : [string]! } }
    state idle { INIT -> idle (ref Rec) }
  }
  page "/recs" -> RecTrait
}
"#;
        assert!(check(src).is_ok());
    }

    /// The sanctioned vocabulary must keep passing: `scalar` (bounded
    /// primitive) and shaped types. `opaque` is REMOVED from the language
    /// (L-8 closure) — see `err_opaque_is_unknown_type_in_every_position`.
    #[test]
    fn ok_scalar_and_shaped_types_pass() {
        let src = r#"
orbital OrbJ {
  entity Rec [persistent: recs] {
    id : string!
    value : scalar
    meta : Map string scalar
    point : { x : number!, y : number! }
  }
  trait RecTrait -> Rec [interaction] { state idle { INIT -> idle (ref Rec) } }
  page "/recs" -> RecTrait
}
"#;
        check(src).unwrap();
    }

    /// `opaque` is REMOVED from the language (L-8 closure, 2026-09-07): it is
    /// no longer a recognized primitive, so every type position that used to
    /// accept it now sees an unresolved `TypeExpr::Named("opaque")` and must
    /// reject it as `UnknownType` — never silently lower to `string`. Covers
    /// every position the old `opaque` doc claimed: entity field, array
    /// element, `Map` value, config knob, and event payload field.
    #[test]
    fn err_opaque_is_unknown_type_in_every_position() {
        let cases = [
            ("entity field", "snapshot : opaque"),
            ("array element", "snapshot : [opaque]"),
            ("map value", "snapshot : Map string opaque"),
        ];
        for (label, field) in cases {
            let src = format!(
                r#"
orbital OrbK {{
  entity Rec [persistent: recs] {{
    id : string!
    {field}
  }}
  trait RecTrait -> Rec [interaction] {{ state idle {{ INIT -> idle (ref Rec) }} }}
  page "/recs" -> RecTrait
}}
"#
            );
            let err = check(&src).unwrap_err();
            assert!(
                matches!(err, TypeError::UnknownType { ref name, .. } if name == "opaque"),
                "{label}: got {err:?}"
            );
        }

        let src = r#"
orbital OrbL {
  entity Rec [persistent: recs] { id : string! }
  trait RecTrait -> Rec [interaction] {
    config { extra : opaque }
    state idle { INIT -> idle (ref Rec) }
  }
  page "/recs" -> RecTrait
}
"#;
        let err = check(src).unwrap_err();
        assert!(
            matches!(err, TypeError::UnknownType { ref name, .. } if name == "opaque"),
            "config knob: got {err:?}"
        );

        let src = r#"
orbital OrbM {
  entity Rec [persistent: recs] { id : string! }
  trait RecTrait -> Rec [interaction] {
    emits { DONE -> external { value : opaque } }
    state idle { INIT -> idle (ref Rec) }
  }
  page "/recs" -> RecTrait
}
"#;
        let err = check(src).unwrap_err();
        assert!(
            matches!(err, TypeError::UnknownType { ref name, .. } if name == "opaque"),
            "emit payload field: got {err:?}"
        );
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
    fn ok_type_union_alias_in_payload_position() {
        let src = r#"
orbital OrbU {
  type DecisionMatch = scalar | [float]
  entity Rec { id : string! }
  trait R -> Rec [interaction] {
    state s { INIT -> s (ref Rec) }
    emits {
      DECIDED -> external { matched : DecisionMatch!, label : string }
    }
  }
  page "/rs" -> R
}
"#;
        check(src).unwrap();
    }

    #[test]
    fn ok_tagged_struct_union_alias_bare_in_payload_position() {
        let src = r#"
type StepCall = { type: "call", tool: string! }
type StepResult = { type: "result", ok: boolean! }
type Step = StepCall | StepResult
orbital OrbT {
  entity Rec { id : string! }
  trait R -> Rec [interaction] {
    state s { INIT -> s (ref Rec) }
    emits {
      STEPPED -> external { step : Step! }
    }
  }
  page "/rs" -> R
}
"#;
        check(src).unwrap();
    }

    #[test]
    fn ok_tagged_struct_union_alias_in_orbital_scope_bare_in_payload_position() {
        let src = r#"
orbital OrbT2 {
  type StepCall = { type: "call", tool: string! }
  type StepResult = { type: "result", ok: boolean! }
  type Step = StepCall | StepResult
  entity Rec { id : string! }
  trait R -> Rec [interaction] {
    state s { INIT -> s (ref Rec) }
    emits {
      STEPPED -> external { step : Step! }
    }
  }
  page "/rs" -> R
}
"#;
        check(src).unwrap();
    }

    #[test]
    fn err_vocabulary_alias_in_payload_position_still_rejected() {
        let src = r#"
orbital OrbV {
  type Direction = up | down | left | right
  entity Rec { id : string! }
  trait R -> Rec [interaction] {
    state s { INIT -> s (ref Rec) }
    emits {
      MOVED -> external { dir : Direction! }
    }
  }
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
