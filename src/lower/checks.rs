use super::*;

/// Owner ruling: nothing about a `[shared]` entity may be inferred. A bare
/// `entity Foo [shared]` with no explicit persistence tag used to default
/// to persistent downstream — now it must say which persistence mode it
/// wants. Called from `lower_entity` for both the primary entity and any
/// trait-owned `data_entities`.
pub(crate) fn check_shared_entity_has_persistence_mode(entity: &Entity) -> Result<(), LowerError> {
    if entity.shared && entity.persistence.is_none() {
        return Err(LowerError::SharedEntityWithoutPersistenceMode {
            entity: entity.name.clone(),
        });
    }
    Ok(())
}

/// Per the explicit-binding rule, every `@entity.X` referenced in a trait's
/// transitions/guards/ticks must be backed by some explicit
/// `(set @entity.X <value>)`. Returns the first violation as a
/// `LowerError::EntityFieldNeverSet` so the lolo CLI surfaces it at parse
/// time. Skipped by the caller when the trait has no `linkedEntity`.
/// `@intrinsic` fields are EXEMPT — they are machinery the trait/runtime owns
/// and may be read without a local setter.
///
/// `entity_shared` defers this check entirely on a `[shared]` entity: several
/// bound traits co-mutate one shared instance, so a sibling trait may own the
/// setter for a field this trait only reads. The genuine "no trait in the
/// whole shared group ever sets X" case is caught post-inline at L2, which
/// sees every trait bound to the shared entity, not just this one.
pub(crate) fn check_entity_fields_have_setters(
    t: &Trait,
    intrinsic_fields: &HashSet<String>,
    entity_shared: bool,
) -> Result<(), LowerError> {
    if entity_shared {
        return Ok(());
    }
    let mut referenced_fields: HashMap<String, String> = HashMap::new();
    let mut set_fields: HashSet<String> = HashSet::new();

    for state in &t.states {
        for tr in &state.transitions {
            // Term-match sugar (Phase 2) expands to one guarded Transition per
            // arm — a no-op for every ordinary transition — so a match arm's
            // `(set @entity.X ...)` is checked exactly like a hand-guarded one.
            for tr in expand_term_match(tr) {
                let event = tr.event.as_str();
                if let Some(guard) = &tr.guard {
                    guard.collect_entity_field_refs(&mut referenced_fields, &mut set_fields, event);
                }
                for effect in &tr.effects {
                    effect.collect_entity_field_refs(
                        &mut referenced_fields,
                        &mut set_fields,
                        event,
                    );
                }
            }
        }
    }
    for tick in &t.ticks {
        if let Some(guard) = &tick.guard {
            guard.collect_entity_field_refs(&mut referenced_fields, &mut set_fields, "TICK");
        }
        for effect in &tick.effects {
            effect.collect_entity_field_refs(&mut referenced_fields, &mut set_fields, "TICK");
        }
    }

    // Sort referenced fields for deterministic error ordering (BTreeMap-like).
    let mut entries: Vec<(&String, &String)> = referenced_fields.iter().collect();
    entries.sort_by(|a, b| a.0.cmp(b.0));
    for (field, first_event) in entries {
        if !set_fields.contains(field) && !intrinsic_fields.contains(field) {
            let err = LowerError::EntityFieldNeverSet {
                trait_name: t.name.clone(),
                field: field.clone(),
                first_event: first_event.clone(),
            };
            return Err(match first_read_span(t, field) {
                Some(span) => LowerError::At {
                    span,
                    exact: true,
                    inner: Box::new(err),
                },
                None => err,
            });
        }
    }
    Ok(())
}

/// The node a parser trail (`[2].content`, `.children[0]`, …) names inside `expr`.
fn node_at<'a>(expr: &'a SExpression, trail: &str) -> Option<&'a SExpression> {
    let mut node = expr;
    let mut rest = trail;
    while !rest.is_empty() {
        if let Some(after) = rest.strip_prefix('[') {
            let close = after.find(']')?;
            let index: usize = after[..close].parse().ok()?;
            node = match node {
                SExpression::Array(items) => items.get(index)?,
                _ => return None,
            };
            rest = &after[close + 1..];
        } else if let Some(after) = rest.strip_prefix('.') {
            let end = after.find(['.', '[']).unwrap_or(after.len());
            node = match node {
                SExpression::Object(map) => map.get(&after[..end])?,
                _ => return None,
            };
            rest = &after[end..];
        } else {
            return None;
        }
    }
    Some(node)
}

/// `@entity.<field>…` read of `field` (same field parse as `collect_entity_field_refs`).
fn reads_field(node: &SExpression, field: &str) -> bool {
    let SExpression::String(s) = node else {
        return false;
    };
    let Some(path) = s.strip_prefix("@entity.") else {
        return false;
    };
    let head = path.split('.').next().unwrap_or("");
    head.split('[').next().unwrap_or(head) == field
}

/// An expression and the source spans its parse recorded, by trail.
type Spanned<'a> = (Option<&'a SExpression>, &'a [(String, Span)]);

/// Where `field` is first read: the first transition (or tick) in declaration
/// order that reads it, and within it the earliest recorded source position.
fn first_read_span(t: &Trait, field: &str) -> Option<Span> {
    let earliest = |parts: &[Spanned<'_>]| {
        parts
            .iter()
            .flat_map(|(expr, spans)| {
                spans.iter().filter_map(move |(trail, span)| {
                    expr.and_then(|e| node_at(e, trail))
                        .filter(|n| reads_field(n, field))
                        .map(|_| *span)
                })
            })
            .min_by_key(|s| (s.line, s.col))
    };
    for state in &t.states {
        for tr in &state.transitions {
            for tr in expand_term_match(tr) {
                let mut parts: Vec<Spanned<'_>> =
                    vec![(tr.guard.as_ref(), tr.guard_spans.as_slice())];
                parts.extend(
                    tr.effects
                        .iter()
                        .zip(&tr.effect_spans)
                        .map(|(e, s)| (Some(e), s.as_slice())),
                );
                if let Some(span) = earliest(&parts) {
                    return Some(span);
                }
            }
        }
    }
    for tick in &t.ticks {
        let mut parts: Vec<Spanned<'_>> = vec![(tick.guard.as_ref(), tick.guard_spans.as_slice())];
        parts.extend(
            tick.effects
                .iter()
                .zip(&tick.effect_spans)
                .map(|(e, s)| (Some(e), s.as_slice())),
        );
        if let Some(span) = earliest(&parts) {
            return Some(span);
        }
    }
    None
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
pub(crate) fn check_set_value_types(
    t: &Trait,
    entities: &BTreeMap<String, Vec<(String, TypeExpr, bool)>>,
    aliases: &BTreeMap<String, TypeAliasResolved>,
) -> Result<(), LowerError> {
    let Some(linked) = &t.linked_entity else {
        return Ok(());
    };
    let Some(entity_fields) = entities.get(linked) else {
        return Ok(());
    };
    let entity_names: HashSet<String> = entities.keys().cloned().collect();

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

    // Parallel per-emitted-event payload schema lookup. Keyed by the LOCAL
    // emit event name. A `(set @entity.X ?payloadField)` firing on an
    // emit-declared event resolves `@payload.X` against this map when the
    // event has no matching `listens` entry. `Emit.payload` and
    // `Listen.payload` are both `Vec<EventField>` in the lolo AST, so the
    // same `?field` resolution path consumes either source unchanged.
    let mut emit_payloads: HashMap<&str, &[EventField]> = HashMap::new();
    for emit in &t.emits {
        if !emit.payload.is_empty() {
            emit_payloads.insert(emit.event.as_str(), emit.payload.as_slice());
        }
    }

    // Per-config-field type lookup.
    let config_fields: HashMap<&str, &TypeExpr> = t
        .config
        .iter()
        .map(|cf| (cf.name.as_str(), cf.type_expr.peel()))
        .collect();

    // Per-entity-field type lookup on the linked entity.
    let entity_field_types: HashMap<&str, &TypeExpr> = entity_fields
        .iter()
        .map(|(name, ty, _required)| (name.as_str(), ty))
        .collect();

    for state in &t.states {
        for tr in &state.transitions {
            // Term-match sugar (Phase 2) expands to one guarded Transition
            // per arm — a no-op for every ordinary transition. Phase 3c: a
            // destructuring match additionally refines the scrutinee's owner
            // to each arm's selected variant, index-aligned with the
            // expansion, so member reads inside an arm are checked against
            // that variant's fields.
            let refinements = match &tr.term_match {
                Some(tm) => term_match_arm_refinements(
                    t,
                    &tr.event,
                    tm,
                    Some(entity_fields),
                    &listen_payloads,
                    &emit_payloads,
                    aliases,
                ),
                None => vec![],
            };
            for (i, tr) in expand_term_match(tr).iter().enumerate() {
                let refinement = refinements.get(i).and_then(Option::as_ref);
                for effect in &tr.effects {
                    check_set_in_expr(
                        effect,
                        &tr.event,
                        t,
                        &entity_field_types,
                        &listen_payloads,
                        &emit_payloads,
                        &config_fields,
                        &HashSet::new(),
                        refinement,
                        aliases,
                        &entity_names,
                    )?;
                }
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
                &emit_payloads,
                &config_fields,
                &HashSet::new(),
                None,
                aliases,
                &entity_names,
            )?;
        }
    }
    Ok(())
}

/// Recursively search for `["set", "@entity.X", <value>]` SExpressions
/// and validate each one. Descends into nested arrays and objects so
/// `(set ...)` calls inside `(if cond ...)` / `(do ...)` / nested
/// render-ui patterns are caught. `refinement` (Phase 3c) is the enclosing
/// term-match arm's owner→variant refinement, `None` outside a
/// destructuring match arm.
#[allow(clippy::too_many_arguments)]
fn check_set_in_expr(
    expr: &SExpression,
    event: &str,
    t: &Trait,
    entity_fields: &HashMap<&str, &TypeExpr>,
    listen_payloads: &HashMap<&str, &[EventField]>,
    emit_payloads: &HashMap<&str, &[EventField]>,
    config_fields: &HashMap<&str, &TypeExpr>,
    let_bindings: &HashSet<String>,
    refinement: Option<&ArmRefinement>,
    aliases: &BTreeMap<String, TypeAliasResolved>,
    entity_names: &HashSet<String>,
) -> Result<(), LowerError> {
    if let SExpression::Array(arr) = expr {
        // `(let ((name init)…) body)` — record the bound names so a later
        // `(set @entity.X name)` defers to L2 instead of being mis-typed as a
        // string literal. Recurse into the body with the augmented set; the
        // initializer expressions are checked with the original set.
        // A multi-statement let body without `(do …)` is now caught at parse
        // time as `ParseError::TooManyArgs` (C-LET-SINGLE-BODY-DIAG moved
        // upstream from lowering to parsing).
        let is_let =
            matches!(&arr.first(), Some(SExpression::String(s)) if s == "let") && arr.len() == 3;
        if is_let {
            if let SExpression::Array(bindings) = &arr[1] {
                let mut nested = let_bindings.clone();
                for binding in bindings.iter() {
                    if let SExpression::Array(pair) = binding {
                        if let Some(SExpression::String(name)) = pair.first() {
                            nested.insert(name.clone());
                        }
                    }
                }
                check_set_in_expr(
                    &arr[1],
                    event,
                    t,
                    entity_fields,
                    listen_payloads,
                    emit_payloads,
                    config_fields,
                    let_bindings,
                    refinement,
                    aliases,
                    entity_names,
                )?;
                return check_set_in_expr(
                    &arr[2],
                    event,
                    t,
                    entity_fields,
                    listen_payloads,
                    emit_payloads,
                    config_fields,
                    &nested,
                    refinement,
                    aliases,
                    entity_names,
                );
            }
        }
        if arr.len() >= 3 {
            if let (SExpression::String(op), SExpression::String(target)) = (&arr[0], &arr[1]) {
                if op == "set" && target.starts_with("@entity.") {
                    let path = &target[8..];
                    let field = path
                        .split('.')
                        .next()
                        .unwrap_or("")
                        .split('[')
                        .next()
                        .unwrap_or("");
                    if !field.is_empty() {
                        if let Some(declared) = entity_fields.get(field) {
                            check_one_set_value(
                                t,
                                field,
                                declared,
                                &arr[2],
                                event,
                                listen_payloads,
                                emit_payloads,
                                config_fields,
                                entity_fields,
                                let_bindings,
                                refinement,
                                aliases,
                                entity_names,
                            )?;
                        }
                    }
                }
            }
        }
        for item in arr {
            check_set_in_expr(
                item,
                event,
                t,
                entity_fields,
                listen_payloads,
                emit_payloads,
                config_fields,
                let_bindings,
                refinement,
                aliases,
                entity_names,
            )?;
        }
    } else if let SExpression::Object(obj) = expr {
        for value in obj.values() {
            check_set_in_expr(
                value,
                event,
                t,
                entity_fields,
                listen_payloads,
                emit_payloads,
                config_fields,
                let_bindings,
                refinement,
                aliases,
                entity_names,
            )?;
        }
    }
    Ok(())
}

/// Resolve the value's type from its SExpression form and compare against
/// the declared entity field type. Returns the first mismatch as a
/// `LowerError::SetValueTypeMismatch`. Bindings whose source isn't
/// declared in the trait (cross-orbital, unknown event, etc.) skip
/// silently — those defer to L2's F3 binding resolver.
#[allow(clippy::too_many_arguments)]
fn check_one_set_value(
    t: &Trait,
    field: &str,
    declared: &TypeExpr,
    value: &SExpression,
    event: &str,
    listen_payloads: &HashMap<&str, &[EventField]>,
    emit_payloads: &HashMap<&str, &[EventField]>,
    config_fields: &HashMap<&str, &TypeExpr>,
    entity_fields: &HashMap<&str, &TypeExpr>,
    let_bindings: &HashSet<String>,
    refinement: Option<&ArmRefinement>,
    aliases: &BTreeMap<String, TypeAliasResolved>,
    entity_names: &HashSet<String>,
) -> Result<(), LowerError> {
    // ENUM-UNENFORCED (Gap G-LOLO-ENUM-UNENFORCED): when the field declares an
    // enum (an inline `"a" | "b"` union) and the value is a plain string
    // literal, reject a value that is not one of the declared members. Runs
    // before the type-compat resolution below (both would call the value a
    // `string`; only this one knows the member set). Same predicate
    // (`orbital_core::enum_value_is_member`) the validator uses on the lowered
    // `.orb` — one rule, two call sites.
    if let SExpression::String(s) = value {
        if !s.starts_with('@') && !s.starts_with('?') {
            if let Some(members) = enum_members_of(declared, aliases) {
                if !members.is_empty()
                    && !orbital_core_base::type_compat::enum_value_is_member(s, &members)
                {
                    return Err(LowerError::EnumValueNotMember {
                        orbital: t.name.clone(),
                        trait_name: t.name.clone(),
                        field: field.to_string(),
                        value: s.clone(),
                        suggestion: crate::types::closest_name(s, &members),
                        members: members.join(", "),
                    });
                }
            }
        }
    }

    let declared_str = type_expr_to_compat_str(declared);

    // Resolve value's type. Returns (actual_type_str, value_source).
    // None when the value is a binding/operator we don't handle at L1
    // (defer to L2).
    let resolved = match value {
        SExpression::Number(_) => Some(("number".to_string(), "literal".to_string())),
        SExpression::Bool(_) => Some(("boolean".to_string(), "literal".to_string())),
        SExpression::Object(_) => {
            let as_value = orbital_core_base::value::Value::from(value.clone());
            let kind = if orbital_core_base::schema::behavior_value::is_trait_value(&as_value) {
                "trait"
            } else if orbital_core_base::schema::behavior_value::is_behavior_ref_value(&as_value) {
                "behavior"
            } else {
                "object"
            };
            Some((kind.to_string(), "literal".to_string()))
        }
        SExpression::Array(_) => {
            // Could be an array literal OR an SExpression operator call.
            // Defer to L2 — distinguishing them at parse time without
            // canonical-operators metadata is brittle.
            None
        }
        SExpression::Null => None,
        SExpression::String(s) => {
            // Phase 3c flow refinement: a member read under the enclosing
            // arm's refined OWNER resolves against the SELECTED variant's
            // fields, not the conservative union. A member the variant
            // doesn't declare is a hard error — the read can never see it in
            // this arm. Runs FIRST so the refined path never falls through
            // to the generic sigil branches (which would defer a dotted path
            // to L2 silently). Paths nesting deeper than one member keep
            // today's defer-to-L2 posture.
            if let Some((r, rest)) = refinement.and_then(|r| {
                s.strip_prefix(&r.owner)
                    .and_then(|x| x.strip_prefix('.'))
                    .map(|rest| (r, rest))
            }) {
                match resolve_top_segment(rest) {
                    Some(member) => match r.fields.iter().find(|(n, _, _)| *n == member) {
                        Some((_, ty, _)) => {
                            Some((type_expr_to_compat_str(ty), format!("{}.{member}", r.owner)))
                        }
                        None => {
                            let mut names: Vec<&str> =
                                r.fields.iter().map(|(n, _, _)| n.as_str()).collect();
                            names.sort_unstable();
                            return Err(LowerError::MatchRefinedMemberUnknown {
                                trait_name: t.name.clone(),
                                scrutinee: r.scrutinee.clone(),
                                arm: r.arm.clone(),
                                owner: r.owner.clone(),
                                member,
                                variant: r.variant.clone(),
                                fields: names.join(", "),
                            });
                        }
                    },
                    None => None,
                }
            } else if let Some(rest) = s.strip_prefix("@payload.") {
                resolve_top_segment(rest).and_then(|f| {
                    // Resolve `@payload.X` against the declared payload schema
                    // for the firing event. A `listens` entry wins; otherwise
                    // fall back to an `emits`-declared payload so emit-sourced
                    // payload fields are type-checked the same way.
                    let payload = listen_payloads
                        .get(event)
                        .or_else(|| emit_payloads.get(event))?;
                    let declared_payload_field = payload.iter().find(|pf| pf.name == f)?;
                    Some((
                        type_expr_to_compat_str(declared_payload_field.type_expr.peel()),
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
                // A bare identifier that is bound by an enclosing `(let …)`
                // has its type determined by the binding's initializer, which
                // L1 can't reliably infer (often an operator call). Defer to
                // L2 rather than false-classifying it as a string literal.
                if let_bindings.contains(s) {
                    None
                } else {
                    Some(("string".to_string(), "literal".to_string()))
                }
            }
        }
    };

    let Some((actual_str, value_source)) = resolved else {
        return Ok(());
    };

    if let (SExpression::Object(_), Some(record)) =
        (value, record_slot(declared, aliases, entity_names))
    {
        if record_literal_fits(record, value, aliases, entity_names)? {
            return Ok(());
        }
        return Err(LowerError::SetValueTypeMismatch {
            orbital: t.name.clone(),
            trait_name: t.name.clone(),
            field: field.to_string(),
            expected_type: declared_str,
            actual_type: actual_str,
            value_source,
        });
    }

    let compatible = |declared: &str, actual: &str| {
        orbital_core_base::type_compat::set_value_type_compatible(declared, actual)
    };
    let union_members = type_union_member_strs(declared, aliases);
    let accepted = match &union_members {
        _ if declared_str == actual_str => true,
        Some(members) => members.iter().any(|m| compatible(m, &actual_str)),
        None => compatible(&declared_str, &actual_str),
    };
    if !accepted {
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

/// The named record type (a type alias or a program type) a field is declared as, when it is one.
fn record_slot<'a>(
    ty: &'a TypeExpr,
    aliases: &BTreeMap<String, TypeAliasResolved>,
    entity_names: &HashSet<String>,
) -> Option<&'a TypeExpr> {
    match ty {
        TypeExpr::Optional(inner) => record_slot(inner, aliases, entity_names),
        TypeExpr::Named(n)
            if matches!(aliases.get(n), Some(TypeAliasResolved::Type(..)))
                || (!aliases.contains_key(n)
                    && !entity_names.contains(n)
                    && orbital_core_base::language::program_types::program_type(n).is_some()) =>
        {
            Some(ty)
        }
        _ => None,
    }
}

/// The member types (as compat strings) of a field typed by a TYPE union — an inline
/// `string | number` or a named alias of one — or `None` for anything else. A value is
/// assignable to the field when it fits any one member.
fn type_union_member_strs(
    ty: &TypeExpr,
    aliases: &BTreeMap<String, TypeAliasResolved>,
) -> Option<Vec<String>> {
    match ty {
        TypeExpr::Union(parts) if crate::lower::entity::is_type_union(parts, aliases) => {
            Some(parts.iter().map(type_expr_to_compat_str).collect())
        }
        TypeExpr::Named(n) => match aliases.get(n) {
            Some(TypeAliasResolved::Type(inner, _)) => type_union_member_strs(inner, aliases),
            _ => None,
        },
        TypeExpr::Optional(inner) => type_union_member_strs(inner, aliases),
        _ => None,
    }
}

/// The declared enum members of a field type, or `None` when it is not a
/// string-enum. Mirrors the `TypeExpr::Union` lowering in `entity.rs` exactly —
/// including its `is_type_union` gate — so the membership set checked here is
/// the one the `.orb` actually carries. Unwraps an `Optional` wrapper so
/// `("a"|"b")?` still resolves its members.
///
/// The `is_type_union` call is what keeps this honest. Without it, a field typed
/// `string | number | FileValue` lowered as a UNION while this read it as a
/// vocabulary, so `(set @entity.v "hello")` was rejected with *"hello" is not a
/// declared member of v's enum [string, number, FileValue]* — a diagnostic that
/// contradicts the field's own emitted type. One reading, one predicate.
fn enum_members_of(
    ty: &TypeExpr,
    aliases: &BTreeMap<String, TypeAliasResolved>,
) -> Option<Vec<String>> {
    match ty {
        TypeExpr::Union(parts) if !crate::lower::entity::is_type_union(parts, aliases) => parts
            .iter()
            .map(|p| match p {
                TypeExpr::Named(n) => Some(n.clone()),
                TypeExpr::Primitive(n) => Some(n.clone()),
                TypeExpr::StringLiteral(s) => Some(s.clone()),
                _ => None,
            })
            .collect(),
        // A field typed by a NAMED alias carries its members in the alias map,
        // not inline. Resolving one level here is what lets the check see
        // `v : Status` at all; a type-union alias resolves to `Type(Union)` and
        // falls through to `None`, exactly like the inline case above.
        TypeExpr::Named(n) => match aliases.get(n) {
            Some(TypeAliasResolved::Enum(members)) if members.len() >= 2 => Some(members.clone()),
            Some(TypeAliasResolved::Type(inner, _)) => enum_members_of(inner, aliases),
            _ => None,
        },
        TypeExpr::Optional(inner) => enum_members_of(inner, aliases),
        _ => None,
    }
}

/// L1 exhaustiveness/correctness checks for type-level `match` (§25). Runs
/// unconditionally per trait — a match knob needs no `linkedEntity` — over
/// every `emits`/local-`listens` payload field. Carries ZERO knob-name or
/// pattern-name knowledge: every rule below is over the match's own
/// structure (arm count, arm spellings, arm types) and the trait's own
/// declared `config`, never a specific knob's name.
pub(crate) fn check_match_types(
    t: &Trait,
    aliases: &BTreeMap<String, TypeAliasResolved>,
) -> Result<(), LowerError> {
    let config: HashMap<&str, &ConfigField> =
        t.config.iter().map(|cf| (cf.name.as_str(), cf)).collect();
    for e in &t.emits {
        for f in &e.payload {
            check_one_match_field(t, f.type_expr.peel(), &config, aliases)?;
        }
    }
    for l in &t.listens {
        for f in &l.payload {
            check_one_match_field(t, f.type_expr.peel(), &config, aliases)?;
        }
    }
    Ok(())
}

fn check_one_match_field(
    t: &Trait,
    type_expr: &TypeExpr,
    config: &HashMap<&str, &ConfigField>,
    aliases: &BTreeMap<String, TypeAliasResolved>,
) -> Result<(), LowerError> {
    let TypeExpr::Match { knob, arms } = type_expr else {
        return Ok(());
    };
    let Some(cf) = config.get(knob.as_str()) else {
        return Err(LowerError::MatchKnobUndeclared {
            trait_name: t.name.clone(),
            knob: knob.clone(),
        });
    };
    let vocabulary = closed_vocabulary_of(cf.type_expr.peel(), aliases);
    let scrutinee = format!("@config.{knob}");

    let mut seen_spellings: HashSet<String> = HashSet::new();
    let mut wildcard_seen = false;
    for arm in arms {
        check_arm_pattern_reachable(
            &t.name,
            &scrutinee,
            knob,
            &arm.pattern,
            &vocabulary,
            &mut seen_spellings,
            &mut wildcard_seen,
        )?;
        check_match_arm_type(t, knob, &arm.ty)?;
    }
    if !wildcard_seen {
        return Err(LowerError::MatchWildcardRequired {
            trait_name: t.name.clone(),
            scrutinee,
        });
    }
    Ok(())
}

/// Structural pattern-reachability check for ONE match arm: rejects an arm
/// following `_` (already unreachable), a duplicate literal spelling, or a
/// spelling outside the scrutinee's closed vocabulary. Shared by type-level
/// `match` (`check_one_match_field`, §25) and Phase 2's term-level `match`
/// (`check_one_term_match`) — both dispatch the SAME `MatchPattern`
/// vocabulary; only what an arm carries (a type vs effects) differs, and
/// that's the caller's concern, not this function's. `scrutinee` is the
/// full spelling for the outer error text (`@config.op` / `@entity.op`);
/// `owner_label` is the bare name used in the "not one of `X`'s declared
/// values" detail (`op`, either way) — matching each check family's
/// pre-existing wording exactly. Threads `seen_spellings`/`wildcard_seen`
/// through by reference so callers can layer their own per-arm checks (a
/// type check, here) around this one without re-deriving state.
#[allow(clippy::too_many_arguments)]
fn check_arm_pattern_reachable(
    trait_name: &str,
    scrutinee: &str,
    owner_label: &str,
    pattern: &MatchPattern,
    vocabulary: &Option<HashSet<String>>,
    seen_spellings: &mut HashSet<String>,
    wildcard_seen: &mut bool,
) -> Result<(), LowerError> {
    if *wildcard_seen {
        return Err(LowerError::MatchArmUnreachable {
            trait_name: trait_name.to_string(),
            scrutinee: scrutinee.to_string(),
            detail: "an arm follows the `_` wildcard, which already matches everything — `_` \
                     must be last"
                .to_string(),
        });
    }
    match pattern {
        MatchPattern::Wildcard => *wildcard_seen = true,
        other => {
            let spelling = other
                .spelling()
                .expect("non-wildcard MatchPattern always has a spelling");
            if !seen_spellings.insert(spelling.clone()) {
                return Err(LowerError::MatchArmDuplicate {
                    trait_name: trait_name.to_string(),
                    scrutinee: scrutinee.to_string(),
                    spelling,
                });
            }
            if let Some(vocab) = vocabulary {
                if !vocab.contains(&spelling) {
                    let mut members: Vec<&str> = vocab.iter().map(String::as_str).collect();
                    members.sort_unstable();
                    return Err(LowerError::MatchArmUnreachable {
                        trait_name: trait_name.to_string(),
                        scrutinee: scrutinee.to_string(),
                        detail: format!(
                            "arm {spelling:?} is not one of `{owner_label}`'s declared values \
                             [{}]",
                            members.join(", ")
                        ),
                    });
                }
            }
        }
    }
    Ok(())
}

/// v1 caps match arm types to `Primitive`/`[Primitive]` — no post-selection
/// expansion path exists yet for objects/unions/aliases/`Map`/nested
/// `match`/sigils/`$param`.
fn check_match_arm_type(t: &Trait, knob: &str, ty: &TypeExpr) -> Result<(), LowerError> {
    let supported = match ty {
        TypeExpr::Primitive(_) => true,
        TypeExpr::Array(inner) => matches!(inner.as_ref(), TypeExpr::Primitive(_)),
        _ => false,
    };
    if supported {
        Ok(())
    } else {
        Err(LowerError::MatchArmTypeUnsupported {
            trait_name: t.name.clone(),
            knob: knob.to_string(),
            ty: type_expr_to_string(ty),
        })
    }
}

/// A scrutinee's closed vocabulary of literal spellings, or `None` when its
/// declared type is open (nothing to check arm reachability/exhaustiveness
/// against). A boolean is the two-value vocabulary `{"true","false"}`;
/// everything else defers to `enum_members_of` — the SAME reading `(set
/// @entity.X "literal")`'s enum-membership check uses, so a field's closed
/// vocabulary is read identically wherever it's checked. Shared by
/// type-level `match` (a config knob's type) and Phase 2's term-level
/// `match` (an entity/payload field's type).
fn closed_vocabulary_of(
    ty: &TypeExpr,
    aliases: &BTreeMap<String, TypeAliasResolved>,
) -> Option<HashSet<String>> {
    match ty {
        TypeExpr::Primitive(p) if p == "boolean" || p == "bool" => Some(
            ["true".to_string(), "false".to_string()]
                .into_iter()
                .collect(),
        ),
        _ => enum_members_of(ty, aliases).map(|members| members.into_iter().collect()),
    }
}

/// The variants of a TAGGED STRUCT UNION field type, as `(variant name,
/// variant fields)` in declaration order — `None` for anything else (a
/// string enum, a primitive, a primitive/mixed type union, an inline
/// struct). Resolution mirrors `enum_members_of`'s posture exactly: unwrap
/// `Optional`, resolve `Named` through the alias map, and read `A | B`
/// through `is_type_union` (the single reading of a union) — then demand
/// every member be a NAMED STRUCT alias, the shape `struct_union_variants`
/// lowers to `FieldType::Union` + `values`/`properties` on the wire.
fn tagged_struct_union_variants<'a>(
    ty: &TypeExpr,
    aliases: &'a BTreeMap<String, TypeAliasResolved>,
) -> Option<Vec<(String, &'a Vec<(String, TypeExpr, bool)>)>> {
    match ty {
        TypeExpr::Optional(inner) => tagged_struct_union_variants(inner, aliases),
        TypeExpr::Named(n) => match aliases.get(n) {
            Some(TypeAliasResolved::Type(inner, _)) => tagged_struct_union_variants(inner, aliases),
            _ => None,
        },
        TypeExpr::Union(parts) if crate::lower::entity::is_type_union(parts, aliases) => {
            let mut out = Vec::with_capacity(parts.len());
            for p in parts {
                let TypeExpr::Named(name) = p else {
                    return None;
                };
                let Some(TypeAliasResolved::Type(TypeExpr::Object(fields), _)) = aliases.get(name)
                else {
                    return None;
                };
                out.push((name.clone(), fields));
            }
            Some(out)
        }
        _ => None,
    }
}

/// The per-variant literals of a tagged struct union's discriminating member
/// `disc`, as `(literal, variant name)` in declaration order. Every variant
/// must declare `disc` as a single-valued closed member (a string-literal
/// type — the discriminator contract `lower_type_expr` documents on
/// `TypeExpr::StringLiteral`), and every literal must select exactly one
/// variant. `Err(detail)` names the first violation.
fn union_discriminant_literals(
    variants: &[(String, &Vec<(String, TypeExpr, bool)>)],
    disc: &str,
) -> Result<Vec<(String, String)>, String> {
    let mut out: Vec<(String, String)> = Vec::with_capacity(variants.len());
    for (vname, fields) in variants {
        let Some((_, ty, _)) = fields.iter().find(|(n, _, _)| n == disc) else {
            return Err(format!(
                "variant {vname:?} has no member {disc:?}, so {disc:?} can't be the union's \
                 discriminant"
            ));
        };
        let lit = match ty {
            TypeExpr::StringLiteral(s) => s.clone(),
            other => {
                return Err(format!(
                    "variant {vname:?}'s member {disc:?} is typed `{}` — a discriminant must \
                     be a single-valued closed member (a string-literal type like \
                     `\"draw-shape\"`)",
                    type_expr_to_string(other)
                ));
            }
        };
        if let Some((_, prior)) = out.iter().find(|(l, _)| *l == lit) {
            return Err(format!(
                "variants {prior:?} and {vname:?} both spell {disc:?} = {lit:?} — the \
                 discriminant must select exactly one variant"
            ));
        }
        out.push((lit, vname.clone()));
    }
    Ok(out)
}

/// Arm-scoped flow refinement (Phase 3c): inside arm N of a term match whose
/// scrutinee is the dotted destructuring form, the OWNER value
/// (`@payload.item` in `match @payload.item.type`) is known to be the
/// variant arm N selects, so member reads under it resolve against that
/// variant's declared fields instead of the conservative union. L1
/// validation-time only — zero `.orb` change, zero L2 change, zero runtime
/// change.
struct ArmRefinement<'a> {
    /// The refined owner's sigil spelling (`@payload.item`).
    owner: String,
    /// The arm's literal spelling, for error text.
    arm: String,
    /// Full scrutinee spelling, for error text.
    scrutinee: String,
    variant: String,
    fields: &'a Vec<(String, TypeExpr, bool)>,
}

/// Per-arm refinements for one transition's term match, aligned index-for-
/// index with `expand_term_match`'s output. All-`None` when the scrutinee is
/// flat (no destructuring → no variant selection to refine) or doesn't
/// resolve to a valid tagged union with a valid discriminant (those cases
/// already hard-errored in `check_term_match_types`, which runs first — this
/// only degrades, never re-reports). A `_` arm is also `None`: the wildcard
/// selects no single variant, so its reads keep the conservative posture.
fn term_match_arm_refinements<'a>(
    t: &Trait,
    event: &str,
    tm: &TermMatch,
    entity_fields: Option<&'a Vec<(String, TypeExpr, bool)>>,
    listen_payloads: &HashMap<&str, &'a [EventField]>,
    emit_payloads: &HashMap<&str, &'a [EventField]>,
    aliases: &'a BTreeMap<String, TypeAliasResolved>,
) -> Vec<Option<ArmRefinement<'a>>> {
    let unrefined = |tm: &TermMatch| tm.arms.iter().map(|_| None).collect::<Vec<_>>();
    let Some(disc) = tm.scrutinee.member() else {
        return unrefined(tm);
    };
    let Ok(base_ty) = resolve_term_scrutinee_type(
        t,
        event,
        &tm.scrutinee,
        entity_fields,
        listen_payloads,
        emit_payloads,
    ) else {
        return unrefined(tm);
    };
    let Some(variants) = tagged_struct_union_variants(base_ty, aliases) else {
        return unrefined(tm);
    };
    let Ok(discs) = union_discriminant_literals(&variants, disc) else {
        return unrefined(tm);
    };
    tm.arms
        .iter()
        .map(|arm| {
            let spelling = arm.pattern.spelling()?;
            let (_, vname) = discs.iter().find(|(lit, _)| *lit == spelling)?;
            let (_, fields) = variants.iter().find(|(n, _)| n == vname)?;
            Some(ArmRefinement {
                owner: tm.scrutinee.owner_sigil(),
                arm: spelling,
                scrutinee: tm.scrutinee.sigil(),
                variant: vname.clone(),
                fields,
            })
        })
        .collect()
}

/// L1 exhaustiveness/correctness checks for transition-position term-level
/// `match` (Phase 2, pattern-matching doc §1(a)). Parallel to
/// `check_match_types` but dispatches on a VALUE (`@entity.<f>`/
/// `@payload.<f>`) rather than a config-resolved TYPE: same arm-pattern
/// vocabulary and duplicate/unreachable machinery
/// (`check_arm_pattern_reachable`), different scrutinee resolution (an
/// entity/payload field's declared type, not a config knob's) and a
/// different exhaustiveness rule — `_` is required only when the
/// scrutinee's type is OPEN; a CLOSED type (declared enum / boolean) may
/// skip `_` when the arms already cover every member (`MatchInexhaustive`
/// otherwise). Unlike Phase 1, a term match has no L2 fallback (zero
/// orbital-compiler involvement per the design), so every scrutinee either
/// resolves here or hard-errors — no "defer to L2" branch.
pub(crate) fn check_term_match_types(
    t: &Trait,
    entities: &BTreeMap<String, Vec<(String, TypeExpr, bool)>>,
    aliases: &BTreeMap<String, TypeAliasResolved>,
) -> Result<(), LowerError> {
    let entity_fields: Option<&Vec<(String, TypeExpr, bool)>> = t
        .linked_entity
        .as_deref()
        .and_then(|name| entities.get(name));

    // Per-firing-event payload schema lookup, same construction as
    // `check_set_value_types`'s: a `listens` entry wins; `emits` is the
    // fallback for emit-sourced payload fields.
    let mut listen_payloads: HashMap<&str, &[EventField]> = HashMap::new();
    for listen in &t.listens {
        let key = listen.triggers.as_deref().unwrap_or(&listen.event);
        if !listen.payload.is_empty() {
            listen_payloads.insert(key, listen.payload.as_slice());
        }
    }
    let mut emit_payloads: HashMap<&str, &[EventField]> = HashMap::new();
    for emit in &t.emits {
        if !emit.payload.is_empty() {
            emit_payloads.insert(emit.event.as_str(), emit.payload.as_slice());
        }
    }

    for state in &t.states {
        for tr in &state.transitions {
            let Some(tm) = &tr.term_match else {
                continue;
            };
            check_one_term_match(
                t,
                &tr.event,
                tm,
                entity_fields,
                &listen_payloads,
                &emit_payloads,
                aliases,
            )?;
        }
    }
    Ok(())
}

/// Resolve a term-match scrutinee's BASE field declared type — `@entity.<f>`
/// against the linked entity's fields, `@payload.<f>` against the firing
/// event's declared payload. The dotted `<member>` segment (Phase 3a) plays
/// no part here; callers layer variant resolution on top. Shared by
/// `check_one_term_match` (which surfaces the errors) and
/// `term_match_arm_refinements` (which silently degrades — the same errors
/// already fired in `check_term_match_types`, which runs first).
fn resolve_term_scrutinee_type<'a>(
    t: &Trait,
    event: &str,
    scrutinee: &MatchScrutinee,
    entity_fields: Option<&'a Vec<(String, TypeExpr, bool)>>,
    listen_payloads: &HashMap<&str, &'a [EventField]>,
    emit_payloads: &HashMap<&str, &'a [EventField]>,
) -> Result<&'a TypeExpr, LowerError> {
    let sigil = scrutinee.sigil();
    match scrutinee {
        MatchScrutinee::Entity { field, .. } => {
            let Some(fields) = entity_fields else {
                return Err(LowerError::MatchScrutineeUndeclared {
                    trait_name: t.name.clone(),
                    scrutinee: sigil,
                    detail: "this trait has no `linkedEntity`, so `@entity.*` has no field to \
                             resolve"
                        .to_string(),
                });
            };
            let Some((_, ty, _)) = fields.iter().find(|(name, _, _)| name == field) else {
                return Err(LowerError::MatchScrutineeUndeclared {
                    trait_name: t.name.clone(),
                    scrutinee: sigil,
                    detail: format!(
                        "field {field:?} is not declared on entity {:?}",
                        t.linked_entity.as_deref().unwrap_or("")
                    ),
                });
            };
            Ok(ty)
        }
        MatchScrutinee::Payload { field, .. } => {
            let Some(payload) = listen_payloads
                .get(event)
                .or_else(|| emit_payloads.get(event))
            else {
                return Err(LowerError::MatchScrutineeUndeclared {
                    trait_name: t.name.clone(),
                    scrutinee: sigil,
                    detail: format!(
                        "event {event:?} has no declared payload in this file \
                         (`listens`/`emits`)"
                    ),
                });
            };
            let Some(pf) = payload.iter().find(|pf| pf.name == *field) else {
                return Err(LowerError::MatchScrutineeUndeclared {
                    trait_name: t.name.clone(),
                    scrutinee: sigil,
                    detail: format!("field {field:?} is not declared on event {event:?}'s payload"),
                });
            };
            Ok(pf.type_expr.peel())
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn check_one_term_match(
    t: &Trait,
    event: &str,
    tm: &TermMatch,
    entity_fields: Option<&Vec<(String, TypeExpr, bool)>>,
    listen_payloads: &HashMap<&str, &[EventField]>,
    emit_payloads: &HashMap<&str, &[EventField]>,
    aliases: &BTreeMap<String, TypeAliasResolved>,
) -> Result<(), LowerError> {
    let scrutinee = tm.scrutinee.sigil();
    let resolved_ty = resolve_term_scrutinee_type(
        t,
        event,
        &tm.scrutinee,
        entity_fields,
        listen_payloads,
        emit_payloads,
    )?;

    // Dotted destructuring scrutinee (Phase 3a): the base field must be a
    // tagged struct union whose variants all carry `<member>` as their
    // single-valued closed discriminant; the closed vocabulary is then the
    // discriminant literal set (total coverage or `_`). A flat scrutinee
    // keeps Phase 2's field-type vocabulary.
    let vocabulary = match tm.scrutinee.member() {
        Some(disc) => {
            let Some(variants) = tagged_struct_union_variants(resolved_ty, aliases) else {
                return Err(LowerError::MatchScrutineeNotUnion {
                    trait_name: t.name.clone(),
                    scrutinee,
                    detail: format!(
                        "field {:?}'s declared type is not a tagged struct union (a `A | B` \
                         union of named struct aliases), so member {disc:?} has no variants \
                         to dispatch over",
                        tm.scrutinee.field()
                    ),
                });
            };
            let discs = union_discriminant_literals(&variants, disc).map_err(|detail| {
                LowerError::MatchDiscriminantInvalid {
                    trait_name: t.name.clone(),
                    scrutinee: scrutinee.clone(),
                    detail,
                }
            })?;
            Some(discs.into_iter().map(|(lit, _)| lit).collect())
        }
        None => closed_vocabulary_of(resolved_ty, aliases),
    };

    // Bare owner label for the "not one of `X`'s declared values" detail —
    // the dotted spelling when destructuring, the field name otherwise.
    let owner_label = match tm.scrutinee.member() {
        Some(m) => format!("{}.{m}", tm.scrutinee.field()),
        None => tm.scrutinee.field().to_string(),
    };

    let mut seen_spellings: HashSet<String> = HashSet::new();
    let mut wildcard_seen = false;
    for arm in &tm.arms {
        check_arm_pattern_reachable(
            &t.name,
            &scrutinee,
            &owner_label,
            &arm.pattern,
            &vocabulary,
            &mut seen_spellings,
            &mut wildcard_seen,
        )?;
    }
    if wildcard_seen {
        return Ok(());
    }
    match &vocabulary {
        Some(vocab) => {
            let mut missing: Vec<&str> = vocab
                .iter()
                .filter(|m| !seen_spellings.contains(m.as_str()))
                .map(String::as_str)
                .collect();
            if missing.is_empty() {
                Ok(())
            } else {
                missing.sort_unstable();
                Err(LowerError::MatchInexhaustive {
                    trait_name: t.name.clone(),
                    scrutinee,
                    missing: missing.join(", "),
                })
            }
        }
        None => Err(LowerError::MatchWildcardRequired {
            trait_name: t.name.clone(),
            scrutinee,
        }),
    }
}

/// Per-trait resolution context for expression-level `match` (Phase 3b)
/// exhaustiveness checks — the same lookup maps `check_set_value_types`
/// builds, gathered once and threaded through the recursive tree walk below
/// instead of rebuilt per occurrence.
struct ExprMatchCtx<'a> {
    trait_name: &'a str,
    entity_fields: Option<&'a Vec<(String, TypeExpr, bool)>>,
    listen_payloads: &'a HashMap<&'a str, &'a [EventField]>,
    emit_payloads: &'a HashMap<&'a str, &'a [EventField]>,
    config_fields: &'a HashMap<&'a str, &'a TypeExpr>,
    aliases: &'a BTreeMap<String, TypeAliasResolved>,
}

/// L1 exhaustiveness/correctness checks for expression-level `match` (Phase
/// 3b, pattern-matching doc §3b) — s-expression sugar legal in VALUE
/// positions, parsed verbatim into an ordinary `["match", scrutinee, [pat,
/// val], …]` SExpression array (`parser::expression::parse_expr_match`) and
/// desugared to nested `if`/`==` at lowering (`desugar_expr_match`). Walks
/// every guard/effect/config-default SExpression this trait owns hunting
/// for that shape (`is_expr_match_array`) and validates each one found —
/// reuses the SAME arm-pattern vocabulary and reachability machinery as
/// Phase 1/2 (`check_arm_pattern_reachable`, `closed_vocabulary_of`); only
/// scrutinee resolution differs (a value reference read anywhere in the
/// trait, not a config-knob TYPE or a fixed per-transition field).
pub(crate) fn check_expr_match_types(
    t: &Trait,
    entities: &BTreeMap<String, Vec<(String, TypeExpr, bool)>>,
    aliases: &BTreeMap<String, TypeAliasResolved>,
) -> Result<(), LowerError> {
    let entity_fields: Option<&Vec<(String, TypeExpr, bool)>> = t
        .linked_entity
        .as_deref()
        .and_then(|name| entities.get(name));

    let mut listen_payloads: HashMap<&str, &[EventField]> = HashMap::new();
    for listen in &t.listens {
        let key = listen.triggers.as_deref().unwrap_or(&listen.event);
        if !listen.payload.is_empty() {
            listen_payloads.insert(key, listen.payload.as_slice());
        }
    }
    let mut emit_payloads: HashMap<&str, &[EventField]> = HashMap::new();
    for emit in &t.emits {
        if !emit.payload.is_empty() {
            emit_payloads.insert(emit.event.as_str(), emit.payload.as_slice());
        }
    }
    let config_fields: HashMap<&str, &TypeExpr> = t
        .config
        .iter()
        .map(|cf| (cf.name.as_str(), cf.type_expr.peel()))
        .collect();

    let ctx = ExprMatchCtx {
        trait_name: &t.name,
        entity_fields,
        listen_payloads: &listen_payloads,
        emit_payloads: &emit_payloads,
        config_fields: &config_fields,
        aliases,
    };

    for state in &t.states {
        for tr in &state.transitions {
            for tr in expand_term_match(tr) {
                if let Some(guard) = &tr.guard {
                    check_expr_matches_in(guard, &tr.event, &ctx)?;
                }
                for effect in &tr.effects {
                    check_expr_matches_in(effect, &tr.event, &ctx)?;
                }
            }
        }
    }
    for tick in &t.ticks {
        if let Some(guard) = &tick.guard {
            check_expr_matches_in(guard, "TICK", &ctx)?;
        }
        for effect in &tick.effects {
            check_expr_matches_in(effect, "TICK", &ctx)?;
        }
    }
    for cf in &t.config {
        if let Some(default) = &cf.default {
            check_expr_matches_in(default, "", &ctx)?;
        }
    }
    for listen in &t.listens {
        if let Some(guard) = &listen.guard {
            check_expr_matches_in(guard, &listen.event, &ctx)?;
        }
        for (_, mapping) in &listen.payload_mapping {
            check_expr_matches_in(mapping, &listen.event, &ctx)?;
        }
    }
    Ok(())
}

/// Recursively hunt an SExpression tree for `is_expr_match_array` shapes and
/// validate each one found, descending into every array/object child
/// (including a validated match's own scrutinee and arm values) so a match
/// nested anywhere — an operator arg, a let body, a nested arm value — is
/// caught, mirroring `check_set_in_expr`'s walk style.
fn check_expr_matches_in(
    expr: &SExpression,
    event: &str,
    ctx: &ExprMatchCtx,
) -> Result<(), LowerError> {
    if let SExpression::Array(arr) = expr {
        if crate::lower::is_expr_match_array(arr) {
            check_one_expr_match(arr, event, ctx)?;
        }
        for item in arr {
            check_expr_matches_in(item, event, ctx)?;
        }
    } else if let SExpression::Object(obj) = expr {
        for value in obj.values() {
            check_expr_matches_in(value, event, ctx)?;
        }
    }
    Ok(())
}

/// Validate one expression-match occurrence: `arr` is `["match", scrutinee,
/// [pattern, value], …]`. Resolves the scrutinee's closed vocabulary (when
/// it's a reference whose declared type is known and closed) and runs the
/// shared arm-reachability check per arm, then the same exhaustiveness rule
/// Phase 2's term match uses — `_` present is always fine; otherwise a
/// closed vocabulary must be fully covered (`MatchInexhaustive` naming what's
/// missing) and an open/unresolvable one requires `_` (`MatchWildcardRequired`).
fn check_one_expr_match(
    arr: &[SExpression],
    event: &str,
    ctx: &ExprMatchCtx,
) -> Result<(), LowerError> {
    let scrutinee_expr = &arr[1];
    let scrutinee_label = expr_match_scrutinee_label(scrutinee_expr);
    let vocabulary = resolve_expr_match_vocabulary(scrutinee_expr, event, ctx);

    let mut seen_spellings: HashSet<String> = HashSet::new();
    let mut wildcard_seen = false;
    for arm in &arr[2..] {
        let SExpression::Array(pair) = arm else {
            continue; // malformed — the parser never produces this; skip defensively
        };
        if pair.len() != 2 {
            continue;
        }
        let pattern = pattern_from_literal_sexpr(&pair[0]);
        check_arm_pattern_reachable(
            ctx.trait_name,
            &scrutinee_label,
            &scrutinee_label,
            &pattern,
            &vocabulary,
            &mut seen_spellings,
            &mut wildcard_seen,
        )?;
    }
    if wildcard_seen {
        return Ok(());
    }
    match &vocabulary {
        Some(vocab) => {
            let mut missing: Vec<&str> = vocab
                .iter()
                .filter(|m| !seen_spellings.contains(m.as_str()))
                .map(String::as_str)
                .collect();
            if missing.is_empty() {
                Ok(())
            } else {
                missing.sort_unstable();
                Err(LowerError::MatchInexhaustive {
                    trait_name: ctx.trait_name.to_string(),
                    scrutinee: scrutinee_label,
                    missing: missing.join(", "),
                })
            }
        }
        None => Err(LowerError::MatchWildcardRequired {
            trait_name: ctx.trait_name.to_string(),
            scrutinee: scrutinee_label,
        }),
    }
}

/// Reconstruct the `MatchPattern` an arm's literal-encoded atom represents —
/// the inverse of `MatchPattern::literal_sexpr`. Only ever called on atoms
/// `parse_expr_match` produced via that exact method, so every case here is
/// exhaustive by construction (a float, an array, or an object can never
/// reach this — `parse_match_pattern` rejects those at parse time).
fn pattern_from_literal_sexpr(e: &SExpression) -> MatchPattern {
    match e {
        SExpression::String(s) if s == "_" => MatchPattern::Wildcard,
        SExpression::String(s) => MatchPattern::Str(s.clone()),
        SExpression::Bool(b) => MatchPattern::Bool(*b),
        SExpression::Number(n) => MatchPattern::Int(*n as i64),
        _ => unreachable!("parse_expr_match only ever encodes MatchPattern::literal_sexpr atoms"),
    }
}

/// Human-readable label for an expr-match's scrutinee, used in error text —
/// the sigil path for a reference, or a generic `<literal>` tag for a
/// literal scrutinee (which carries no vocabulary to check against anyway).
fn expr_match_scrutinee_label(expr: &SExpression) -> String {
    match expr {
        SExpression::String(s) => s.clone(),
        _ => "<literal>".to_string(),
    }
}

/// Resolve an expr-match scrutinee's closed vocabulary, or `None` when it's
/// open/unresolvable (a literal scrutinee, an unknown field, or a
/// `@payload.<f>` whose firing event has no declared payload in this file —
/// each degrades to requiring `_` rather than erroring, matching L1's
/// "defer, don't false-positive" posture elsewhere in this file).
fn resolve_expr_match_vocabulary(
    scrutinee: &SExpression,
    event: &str,
    ctx: &ExprMatchCtx,
) -> Option<HashSet<String>> {
    let SExpression::String(s) = scrutinee else {
        return None;
    };
    if let Some(f) = s.strip_prefix("@entity.").and_then(resolve_top_segment) {
        let fields = ctx.entity_fields?;
        let (_, ty, _) = fields.iter().find(|(name, _, _)| *name == f)?;
        return closed_vocabulary_of(ty, ctx.aliases);
    }
    if let Some(f) = s.strip_prefix("@payload.").and_then(resolve_top_segment) {
        let payload = ctx
            .listen_payloads
            .get(event)
            .or_else(|| ctx.emit_payloads.get(event))?;
        let pf = payload.iter().find(|pf| pf.name == f)?;
        return closed_vocabulary_of(pf.type_expr.peel(), ctx.aliases);
    }
    if let Some(f) = s.strip_prefix("@config.").and_then(resolve_top_segment) {
        let ty = ctx.config_fields.get(f.as_str())?;
        return closed_vocabulary_of(ty, ctx.aliases);
    }
    None
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
pub(crate) fn resolve_top_segment(path: &str) -> Option<String> {
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
pub(crate) fn check_listens_integrity(orbitals: &[CoreOrbital]) -> Result<(), LowerError> {
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
    // An imported orbital's upstream traits arrive at inline, so a source
    // naming one is the compiler's post-inline check too.
    let imported_orbitals: HashSet<&str> = orbitals
        .iter()
        .filter(|o| o.reference.is_some())
        .map(|o| o.name.as_str())
        .collect();
    // A source orbital this file does not contain (one orbital lowered on its own) is the composed
    // schema's check, as in the compiler's `validate_listens_integrity`.
    let orbitals_in_file: HashSet<&str> = orbitals.iter().map(|o| o.name.as_str()).collect();
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
                TraitReference::Reference {
                    name, reference, ..
                } => {
                    let trait_name = name.clone().or_else(|| {
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
                    Some(ListenSource::Trait { trait_name, .. }) => {
                        (orbital.name.clone(), trait_name.clone())
                    }
                    Some(ListenSource::Orbital {
                        orbital: o,
                        trait_name,
                        ..
                    }) => (o.clone(), trait_name.clone()),
                    Some(ListenSource::Any) | None => continue,
                };
                // Defer to the compiler's post-inline validator when the
                // source trait is a reference — its emits aren't
                // visible at this stage.
                if reference_traits.contains(&(source_orbital.clone(), source_trait.clone())) {
                    continue;
                }
                if imported_orbitals.contains(source_orbital.as_str())
                    && !trait_emits.contains_key(&(source_orbital.clone(), source_trait.clone()))
                {
                    continue;
                }
                if !orbitals_in_file.contains(source_orbital.as_str()) {
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

/// True when `host` embeds `child` in `orbital`, directly or through other traits.
fn embeds_transitively(
    embeds: &HashMap<(String, String), HashSet<String>>,
    orbital: &str,
    host: &str,
    child: &str,
) -> bool {
    let mut seen: HashSet<&str> = HashSet::from([host]);
    let mut stack = vec![host];
    while let Some(current) = stack.pop() {
        let Some(children) = embeds.get(&(orbital.to_string(), current.to_string())) else {
            continue;
        };
        for next in children {
            if next == child {
                return true;
            }
            if seen.insert(next.as_str()) {
                stack.push(next.as_str());
            }
        }
    }
    false
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
pub(crate) fn check_button_transitions(orbitals: &[CoreOrbital]) -> Result<(), LowerError> {
    // Index (orbital, trait) -> transition events.
    let mut trait_transitions: HashMap<(String, String), HashSet<String>> = HashMap::new();
    // Track reference-trait names so we can skip them as targets.
    let mut reference_traits: HashSet<(String, String)> = HashSet::new();
    // (orbital, trait) -> the traits it embeds via `@trait.<Name>`.
    let mut embeds: HashMap<(String, String), HashSet<String>> = HashMap::new();

    for orbital in orbitals {
        for trait_ref in &orbital.traits {
            match trait_ref {
                TraitReference::Inline(td) => {
                    embeds.insert(
                        (orbital.name.clone(), td.name.clone()),
                        orbital_core_base::schema::trait_embed_children(td),
                    );
                    let entry = trait_transitions
                        .entry((orbital.name.clone(), td.name.clone()))
                        .or_default();
                    for trans in &td.state_machine.transitions {
                        entry.insert(trans.event.clone());
                    }
                }
                TraitReference::Reference {
                    name, reference, ..
                } => {
                    let trait_name = name.clone().or_else(|| {
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
            let TraitReference::Inline(td) = trait_ref else {
                continue;
            };
            let entry = trait_listens
                .entry((orbital.name.clone(), td.name.clone()))
                .or_default();
            if let Some(listens) = &td.listens {
                for listen in listens {
                    let (source_orbital, source_trait) = match &listen.source {
                        Some(ListenSource::Trait { trait_name, .. }) => {
                            (orbital.name.clone(), trait_name.clone())
                        }
                        Some(ListenSource::Orbital {
                            orbital: o,
                            trait_name,
                            ..
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
            let TraitReference::Inline(td) = trait_ref else {
                continue;
            };
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
                        // A host that embeds the source hears the button on its own embed-chain
                        // scope; a route back would deliver the same click twice.
                        if embeds_transitively(&embeds, t_orbital, t_trait, source_trait) {
                            continue;
                        }
                        let target_key = (t_orbital.clone(), t_trait.clone());
                        let has_listen = trait_listens
                            .get(&target_key)
                            .map(|listens| {
                                listens.contains(&(
                                    orbital.name.clone(),
                                    source_trait.clone(),
                                    action.clone(),
                                ))
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

// L1 navigate-target reachability was removed (RABIT-DEMOTE-EDIT-WORKSPACE-INVISIBLE):
// it collected `pages[].path` per parse-unit, so a single orbital extracted from a
// multi-orbital project failed on a legitimate sibling-route navigate. Reachability is
// authoritatively re-checked post-inline by L2 (`validation/effect/navigate.rs` →
// `ORB_EFF_NAVIGATE_TARGET_UNREACHABLE`), which sees project-wide pages — so cross-orbital
// navigates resolve and genuine typos still error, just one phase later. The route
// canonicaliser this check depended on went with it (L2 owns route matching now).

/// L1.3 check: every `(emit EVT { ... })` literal payload conforms to
/// the declared `emits { EVT { ... } }` block on the same trait.
///
/// Three classes of mismatch caught at parse time (parse-time mirror of
/// L2's `EmitPayloadMissingRequired` / `EmitPayloadExtraField` /
/// `EmitPayloadFieldTypeMismatch` validators in
/// `orbital-compiler/src/phases/validation/circuit/emit_payload.rs`):
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
pub(crate) fn check_emit_payload_literals(
    orbitals: &[CoreOrbital],
    entity_names: &[HashSet<String>],
) -> Result<(), LowerError> {
    for (orbital, entity_names) in orbitals.iter().zip(entity_names) {
        for trait_ref in &orbital.traits {
            let TraitReference::Inline(td) = trait_ref else {
                continue;
            };

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
            let trait_emits_walk = |effects: &[SExpression]| -> Result<(), LowerError> {
                for effect in effects {
                    check_emit_literal_recursive(
                        effect,
                        &emit_schemas,
                        &orbital.name,
                        &td.name,
                        entity_names,
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
    entity_names: &HashSet<String>,
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
                                entity_names,
                            )?;
                        }
                        // Unknown event names handled by the L2 validator's
                        // EffectEmitUndeclaredEvent — not duplicating that
                        // class at L1 in this commit.
                    }
                }
            }
            for item in arr {
                check_emit_literal_recursive(
                    item,
                    emit_schemas,
                    orbital_name,
                    trait_name,
                    entity_names,
                )?;
            }
        }
        SExpression::Object(obj) => {
            for value in obj.values() {
                check_emit_literal_recursive(
                    value,
                    emit_schemas,
                    orbital_name,
                    trait_name,
                    entity_names,
                )?;
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
    entity_names: &HashSet<String>,
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
                if let Some(actual) = orbital_core_eval::literal_type::literal_value_type(value) {
                    let declared_core = expected.trim_end_matches('!');
                    let program_record = actual == "object"
                        && !entity_names.contains(declared_core)
                        && orbital_core_base::language::program_types::program_type(declared_core)
                            .is_some();
                    let compatible = if program_record {
                        record_literal_fits(
                            &TypeExpr::Named(declared_core.to_string()),
                            value,
                            &BTreeMap::new(),
                            entity_names,
                        )?
                    } else {
                        emit_primitive_compatible(expected, actual, entity_names)
                    };
                    if !compatible {
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

// The shallow literal-type classifier is `orbital_core::literal_value_type` —
// the single upstream source both this L1 check and the compiler L2 emit-payload
// backstop call, so they never drift. It correctly defers operator applications
// (`(array/find …)`, `(object/get …)`) to the expression inferencer instead of
// mis-typing them as `"array"` (CG-EMIT-PAYLOAD-SEXPR).

/// Parse-time checks for `uses`-imported trait/page references against the
/// loaded behavior-contract ladder (`crate::registry::LoloContracts`): unknown
/// trait/page refs (3.2), rebind row-constraints (3.3), and typed config
/// overrides (3.4). Operates on the lowered (pre-inline) orbitals; a `None`
/// lookup (local `.orb` import or behavior absent from every loaded package)
/// skips silently, and the compiler re-verifies post-inline as a backstop. A
/// name found in two non-std packages is a hard `AmbiguousBehavior` error.
pub(crate) fn check_behavior_contracts(
    orbitals: &[CoreOrbital],
    contracts: &crate::registry::LoloContracts,
) -> Result<(), LowerError> {
    for orbital in orbitals {
        // alias (`Modal`) → behavior key (`std-modal`), from the `uses` table.
        let mut alias_key: HashMap<String, String> = HashMap::new();
        if let Some(uses) = &orbital.uses {
            for u in uses {
                alias_key.insert(
                    u.alias.clone(),
                    crate::registry::behavior_key_from_path(&u.from).to_string(),
                );
            }
        }
        // Per-entity field sets — primary + auxiliary inline entities, keyed by
        // name, so a trait's row-constraints resolve against the entity it is
        // actually rebound to (`linkedEntity`), not the first-declared block.
        // Only inline definitions are known here; an `extends`/reference target
        // (or a compose-time `expects entity`) is absent from the map and its
        // check is skipped (compiler backstop covers it).
        let mut entity_fields_by_name: HashMap<String, HashSet<String>> = HashMap::new();
        for ent_ref in
            std::iter::once(&orbital.entity).chain(orbital.auxiliary_entities.iter().flatten())
        {
            if let EntityRef::Inline(ed) = ent_ref {
                entity_fields_by_name.insert(
                    ed.name.clone(),
                    ed.fields.iter().filter_map(|f| f.name.clone()).collect(),
                );
            }
        }

        // Knob name → declared type, from THIS orbital's own inline trait config
        // declarations. A plain-wiring override that binds `@config.X` carries no
        // type of its own — the parser tags it `"unknown"`, which is the
        // structural discriminator for wiring-vs-declaration and must stay — so
        // without this map check 3.4 below has no `got` side and silently
        // no-ops on every reference. Same prepass shape as
        // `entity_fields_by_name` above.
        let mut knob_types: HashMap<String, String> = HashMap::new();
        for tref in &orbital.traits {
            if let TraitReference::Inline(td) = tref {
                if let Some(cfg) = &td.config {
                    for (k, cf) in cfg {
                        if cf.field_type != "unknown" {
                            knob_types.insert(k.clone(), cf.field_type.clone());
                        }
                    }
                }
            }
        }

        for tref in &orbital.traits {
            let TraitReference::Reference {
                reference: Some(ref_str),
                linked_entity,
                config,
                fields: field_remap,
                ..
            } = tref
            else {
                continue;
            };
            // "Alias.kind.Member"
            let parts: Vec<&str> = ref_str.split('.').collect();
            if parts.len() != 3 {
                continue;
            }
            let (alias, kind, member) = (parts[0], parts[1], parts[2]);
            let Some(key) = alias_key.get(alias) else {
                continue;
            };
            let beh = match contracts.get_behavior(key) {
                Ok(Some(beh)) => beh,
                Ok(None) => continue, // local/unknown import — not loaded; skip.
                Err(amb) => {
                    return Err(LowerError::AmbiguousBehavior {
                        name: amb.name,
                        package_a: amb.package_a,
                        package_b: amb.package_b,
                    })
                }
            };

            match kind {
                "traits" => {
                    let Some(tc) = beh.trait_contract(member) else {
                        return Err(LowerError::UnknownTraitRef {
                            reference: ref_str.clone(),
                            member: member.to_string(),
                            behavior: key.clone(),
                        });
                    };
                    // 3.3 row-constraints (inline-entity targets only).
                    if let Some((ent, fields)) = linked_entity
                        .as_ref()
                        .and_then(|ent| entity_fields_by_name.get(ent).map(|f| (ent, f)))
                    {
                        let missing: Vec<String> = tc
                            .requires
                            .iter()
                            .filter(|r| {
                                let target =
                                    field_remap.as_ref().and_then(|m| m.get(*r)).unwrap_or(*r);
                                !fields.contains(target)
                            })
                            .cloned()
                            .collect();
                        if !missing.is_empty() {
                            return Err(LowerError::RebindMissingFields {
                                entity: ent.clone(),
                                member: member.to_string(),
                                behavior: key.clone(),
                                missing,
                            });
                        }
                    }
                    // 3.4 typed config overrides (clear scalar mismatches only).
                    // NOTE: unknown-config-key detection (USES-CONFIG-KEY-SILENT)
                    // was attempted here but reverted — as a hard LowerError it
                    // blocks format/emit and breaks formatter fixtures that pass
                    // nested config to exercise the formatter; it belongs in the
                    // validate phase, not lower. Deferred (P1).
                    if let Some(cfg) = config {
                        for (k, cf) in cfg {
                            // For plain wiring entries the value lives in
                            // `cf.default`; skip entries with no default
                            // (annotation-only re-declarations are valid).
                            if let Some(v) = cf.default.as_ref() {
                                // A literal types itself; a `@config.X` binding
                                // resolves through the knob map. Before this,
                                // `literal_value_type` returned None for every
                                // `@`-string, so the tuple destructure failed
                                // and the whole check was skipped — a
                                // number-typed knob wired to a string-typed prop
                                // validated clean and only surfaced when
                                // something RESOLVED the reference downstream.
                                let got = orbital_core_eval::literal_type::literal_value_type(v)
                                    .or_else(|| config_ref_knob_type(v, &knob_types));
                                if let (Some(expected), Some(got)) = (tc.config.get(k), got) {
                                    if !config_type_compatible(got, expected) {
                                        return Err(LowerError::ConfigTypeMismatch {
                                            reference: ref_str.clone(),
                                            key: k.clone(),
                                            expected: expected.clone(),
                                            got: got.to_string(),
                                        });
                                    }
                                }
                            }
                        }
                    }
                }
                "pages" => {
                    if !beh.has_page(member) {
                        return Err(LowerError::UnknownPageRef {
                            reference: ref_str.clone(),
                            member: member.to_string(),
                            behavior: key.clone(),
                        });
                    }
                }
                _ => {}
            }
        }

        // The `Alias.<traits|pages|orbitals>` ladder above only visits
        // `orbital.traits` (trait references) — an unresolved
        // `orbital Local = Alias.orbitals.Upstream { … }` declaration is a
        // reference on the ORBITAL ITSELF (`CoreOrbital.reference`), so it
        // needs its own visit into the same "orbitals" member check. Fourth
        // member of the ladder family.
        if let Some(ref_object) = &orbital.reference {
            let ref_str = &ref_object.reference;
            let parts: Vec<&str> = ref_str.split('.').collect();
            if parts.len() == 3 {
                let (alias, kind, member) = (parts[0], parts[1], parts[2]);
                if kind == "orbitals" {
                    if let Some(key) = alias_key.get(alias) {
                        let beh = match contracts.get_behavior(key) {
                            Ok(Some(beh)) => Some(beh),
                            Ok(None) => None,
                            Err(amb) => {
                                return Err(LowerError::AmbiguousBehavior {
                                    name: amb.name,
                                    package_a: amb.package_a,
                                    package_b: amb.package_b,
                                })
                            }
                        };
                        if let Some(beh) = beh {
                            let Some(oc) = beh.orbital_contract(member) else {
                                return Err(LowerError::UnknownOrbitalRef {
                                    reference: ref_str.clone(),
                                    member: member.to_string(),
                                    behavior: key.clone(),
                                });
                            };
                            // Typed config overrides — same lenient
                            // scalar-only check the trait arm runs.
                            if let Some(cfg) = &ref_object.config {
                                for (k, cf) in cfg {
                                    if let Some(v) = cf.default.as_ref() {
                                        let got =
                                            orbital_core_eval::literal_type::literal_value_type(v)
                                                .or_else(|| config_ref_knob_type(v, &knob_types));
                                        if let (Some(expected), Some(got)) = (oc.config.get(k), got)
                                        {
                                            if !config_type_compatible(got, expected) {
                                                return Err(LowerError::ConfigTypeMismatch {
                                                    reference: ref_str.clone(),
                                                    key: k.clone(),
                                                    expected: expected.clone(),
                                                    got: got.to_string(),
                                                });
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

/// Lenient config value↔declared-type compatibility: returns `false` only for
/// clear scalar contradictions (a number where a string/boolean is declared,
/// etc.); permissive for arrays/objects/`pattern`/`slot`/`SExpr`/unknown
/// declared types so the check never false-positives on a knob it can't reason
/// about.
/// Resolve a `@config.X` binding to the declared type of knob `X` on this
/// orbital, so a reference can be type-checked like a literal.
///
/// Scoped to `@config` deliberately. `@trait` is a sentinel the render-ui
/// binding checker already exempts, and `@user`/`@item`/`@row`/`@callsitePayload`
/// have no scope handler yet — each needs its own measured pass before it can
/// reject anything.
///
/// A nested or indexed path (`@config.articles[0].title`) resolves to the knob's
/// INNER shape, which this map does not carry, so it returns None rather than
/// compare against the container's type and produce a false positive.
fn config_ref_knob_type<'a>(
    value: &SExpression,
    knob_types: &'a HashMap<String, String>,
) -> Option<&'a str> {
    let SExpression::String(s) = value else {
        return None;
    };
    let key = s.strip_prefix("@config.")?;
    if key.contains('.') || key.contains('[') {
        return None;
    }
    knob_types.get(key).map(String::as_str)
}

fn config_type_compatible(got: &str, expected: &str) -> bool {
    match got {
        "number" => !matches!(
            expected,
            "string" | "boolean" | "pattern" | "slot" | "icon" | "component" | "event"
        ),
        "boolean" => !matches!(
            expected,
            "string"
                | "number"
                | "int"
                | "float"
                | "pattern"
                | "slot"
                | "icon"
                | "component"
                | "event"
        ),
        "string" => !matches!(expected, "number" | "int" | "float" | "boolean"),
        "array" => !matches!(
            expected,
            "number" | "int" | "float" | "boolean" | "string" | "event"
        ),
        "object" => !matches!(
            expected,
            "number" | "int" | "float" | "boolean" | "string" | "array" | "event"
        ),
        _ => true,
    }
}

#[cfg(test)]
mod recursive_type_safety_tests {
    use super::*;
    use crate::ast::TypeExpr;
    use std::collections::BTreeMap;

    /// Safety test (feature tests 2 & 3): a field declared by a recursive
    /// STRUCT alias must never be readable as a string enum — a
    /// `(set @entity.x "MenuItem")` literal must not be silently accepted as
    /// a legal "enum member" just because the alias re-enters itself.
    /// `enum_members_of` resolves one level through `TypeAliasResolved::Type`
    /// and the inner shape is `Object`, which every arm falls through to
    /// `None` for — this pins that behavior rather than changing it.
    #[test]
    fn struct_alias_field_is_never_an_enum() {
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
        assert_eq!(
            enum_members_of(&TypeExpr::Named("MenuItem".to_string()), &aliases),
            None,
            "a self-referential struct alias must never resolve to enum members"
        );
    }

    /// Safety half's companion: the fix above must not regress the ordinary
    /// case — `type Direction = up | down | left | right` (no aliases
    /// involved) still reads as a plain string enum.
    #[test]
    fn plain_string_enum_still_reads_as_enum() {
        let aliases: BTreeMap<String, TypeAliasResolved> = BTreeMap::new();
        let direction = TypeExpr::Union(vec![
            TypeExpr::Named("up".to_string()),
            TypeExpr::Named("down".to_string()),
            TypeExpr::Named("left".to_string()),
            TypeExpr::Named("right".to_string()),
        ]);
        assert_eq!(
            enum_members_of(&direction, &aliases),
            Some(vec![
                "up".to_string(),
                "down".to_string(),
                "left".to_string(),
                "right".to_string(),
            ])
        );
    }
}

/// Orbital-import Stage B — the "orbitals" arm `check_behavior_contracts`
/// adds to the `Alias.<traits|pages|orbitals>` ladder, exercised against the
/// installed `@almadar/std` package's contract slice (Phase 2: no embedded
/// fallback) via `std-agent-memory`/`AgentMemoryOrbital` (no upstream
/// declares orbital-level `config {}` yet, so the config-type-mismatch case
/// is covered separately at the pure `config_type_compatible` level below).
#[cfg(test)]
mod orbital_reference_contract_tests {
    use crate::lower::{lower_to_orb_with, LowerOptions};
    use crate::parser::parse_str;

    fn std_options() -> LowerOptions {
        let pkg = orbital_test_support::installed_package(
            "almadar-std",
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")),
        );
        LowerOptions {
            packages: vec![pkg],
            ..Default::default()
        }
    }

    const UNKNOWN_ORBITAL: &str = r#"
uses Memory from "std/behaviors/std-agent-memory"

orbital PfMemory = Memory.orbitals.Nope {
}
"#;

    const KNOWN_ORBITAL: &str = r#"
uses Memory from "std/behaviors/std-agent-memory"

orbital PfMemory = Memory.orbitals.AgentMemoryOrbital {
}
"#;

    #[test]
    fn unknown_orbital_member_always_errors_known_member_always_passes() {
        let options = std_options();
        let unknown_ast = parse_str(UNKNOWN_ORBITAL).expect("parses");
        let known_ast = parse_str(KNOWN_ORBITAL).expect("parses");

        let unknown = lower_to_orb_with(&unknown_ast, &options);
        let known = lower_to_orb_with(&known_ast, &options);

        let err = unknown.expect_err(
            "Memory.orbitals.Nope names an orbital std-agent-memory does not declare — must error",
        );
        let msg = err.to_string();
        assert!(
            msg.contains("Nope"),
            "error names the offending member: {msg}"
        );
        assert!(
            msg.contains("std-agent-memory"),
            "error names the behavior: {msg}"
        );

        known.expect(
            "Memory.orbitals.AgentMemoryOrbital is a real orbital on std-agent-memory — must pass",
        );
    }
}

/// `config_type_compatible` is the same lenient scalar-only comparator both
/// the trait arm and the new orbital arm call — proven directly because the
/// corpus has zero orbitals declaring `config {}` today (verified: every
/// `OrbitalContract.config` derived from the installed `.lolo` is
/// empty), so an end-to-end `orbital X = Alias.orbitals.Y { config { ... } }`
/// mismatch fixture has no real upstream knob to target yet.
#[cfg(test)]
mod config_type_compatible_tests {
    use super::config_type_compatible;

    #[test]
    fn number_knob_rejects_a_string_value() {
        assert!(!config_type_compatible("string", "number"));
    }

    #[test]
    fn string_knob_accepts_a_string_value() {
        assert!(config_type_compatible("string", "string"));
    }

    #[test]
    fn boolean_knob_rejects_a_number_value() {
        assert!(!config_type_compatible("number", "boolean"));
    }

    #[test]
    fn array_and_object_typed_knobs_are_permissive_of_unknown_declared_types() {
        // "unknown"/"pattern"/"slot" declared types fall through the `_ => true`
        // arm deliberately — the check never false-positives on a knob shape
        // it can't reason about.
        assert!(config_type_compatible("array", "unknown"));
        assert!(config_type_compatible("object", "pattern"));
    }
}

#[cfg(test)]
mod button_transition_embed_tests {
    use super::*;

    fn trait_json(
        name: &str,
        handles: Option<&str>,
        renders: Option<&str>,
        embeds: Option<&str>,
        listens: Option<&str>,
    ) -> serde_json::Value {
        let mut transitions = vec![serde_json::json!(
            { "from": "idle", "to": "idle", "event": "INIT", "effects": renders.map_or_else(
                Vec::new,
                |action| vec![serde_json::json!(["render-ui", "main", { "type": "button", "action": action, "label": "Go" }])]
            ) }
        )];
        if let Some(event) = handles {
            transitions.push(serde_json::json!({ "from": "idle", "to": "idle", "event": event }));
        }
        if let Some(child) = embeds {
            transitions[0]["effects"] = serde_json::json!([[
                "render-ui", "main", { "type": "stack", "children": [format!("@trait.{child}")] }
            ]]);
        }
        let mut t = serde_json::json!({
            "name": name, "linkedEntity": "Row", "category": "interaction",
            "stateMachine": { "states": [], "events": [], "transitions": transitions }
        });
        if let Some(source) = listens {
            t["listens"] = serde_json::json!([{
                "event": "REQUEST", "triggers": "REQUEST",
                "source": { "kind": "trait", "trait": source }
            }]);
        }
        t
    }

    fn check(traits: Vec<serde_json::Value>) -> Result<(), LowerError> {
        let orbital: CoreOrbital = serde_json::from_value(serde_json::json!({
            "name": "O",
            "entity": { "name": "Row", "persistence": "persistent",
                "fields": [{ "name": "id", "type": "string", "required": true }] },
            "traits": traits, "pages": []
        }))
        .expect("orbital deserializes");
        check_button_transitions(&[orbital])
    }

    #[test]
    fn unembedded_sibling_without_a_route_is_unreachable() {
        assert!(matches!(
            check(vec![
                trait_json("Panel", None, Some("REQUEST"), None, None),
                trait_json("Host", Some("REQUEST"), None, None, None),
            ]),
            Err(LowerError::ButtonTransitionUnreachable { .. })
        ));
    }

    #[test]
    fn unembedded_sibling_with_a_route_is_reachable() {
        assert!(check(vec![
            trait_json("Panel", None, Some("REQUEST"), None, None),
            trait_json("Host", Some("REQUEST"), None, None, Some("Panel")),
        ])
        .is_ok());
    }

    #[test]
    fn host_that_embeds_the_source_hears_it_without_a_route() {
        assert!(check(vec![
            trait_json("Panel", None, Some("REQUEST"), None, None),
            trait_json("Host", Some("REQUEST"), None, Some("Panel"), None),
        ])
        .is_ok());
    }

    #[test]
    fn host_that_embeds_the_source_transitively_hears_it_without_a_route() {
        assert!(check(vec![
            trait_json("Panel", None, Some("REQUEST"), None, None),
            trait_json("Mid", None, None, Some("Panel"), None),
            trait_json("Host", Some("REQUEST"), None, Some("Mid"), None),
        ])
        .is_ok());
    }

    #[test]
    fn host_that_embeds_a_different_trait_still_needs_the_route() {
        assert!(matches!(
            check(vec![
                trait_json("Panel", None, Some("REQUEST"), None, None),
                trait_json("Other", None, None, None, None),
                trait_json("Host", Some("REQUEST"), None, Some("Other"), None),
            ]),
            Err(LowerError::ButtonTransitionUnreachable { .. })
        ));
    }
}

#[cfg(test)]
mod emit_program_type_literal_tests {
    use crate::lower::lower_to_orb;
    use crate::parser::parse_str;

    fn lower_emit(value: &str) -> Result<(), crate::error::LowerError> {
        lower_emit_with("", value)
    }

    fn lower_emit_with(prelude: &str, value: &str) -> Result<(), crate::error::LowerError> {
        let src = format!(
            r#"
            {prelude}
            orbital TicketOrbital {{
                entity Ticket {{ id : string! }}
                trait TicketMint -> Ticket {{
                    initial: idle
                    state idle {{
                        MINT -> idle
                            (emit MINT_REQUESTED {{ viewer: {value} }})
                    }}
                    emits {{
                        MINT_REQUESTED {{ viewer : Viewer! }}
                    }}
                }}
            }}
        "#
        );
        let ast = parse_str(&src).expect("parse");
        lower_to_orb(&ast)
            .map(|_| ())
            .map_err(|e| e.innermost().clone())
    }

    #[test]
    fn a_record_literal_fills_a_program_type_slot() {
        lower_emit(r#"{ id: "anonymous", role: "anonymous", permissions: [], claims: [] }"#)
            .expect("a record literal is a Viewer at L1; its shape is L2's check");
    }

    #[test]
    fn a_literal_missing_a_required_field_never_fills_a_program_type_slot() {
        match lower_emit(r#"{ id: "anonymous", role: "anonymous", permissions: [] }"#) {
            Err(crate::error::LowerError::EmitPayloadFieldTypeMismatch { actual, .. }) => {
                assert_eq!(actual, "object")
            }
            other => panic!("expected EmitPayloadFieldTypeMismatch, got {other:?}"),
        }
    }

    #[test]
    fn a_literal_with_a_wrong_typed_field_never_fills_a_program_type_slot() {
        match lower_emit(r#"{ id: 5, role: "anonymous", permissions: [], claims: [] }"#) {
            Err(crate::error::LowerError::EmitPayloadFieldTypeMismatch { .. }) => {}
            other => panic!("expected EmitPayloadFieldTypeMismatch, got {other:?}"),
        }
    }

    #[test]
    fn a_local_entity_shadows_a_program_type_at_the_emit_literal_site() {
        let shadow = r#"type Viewer = Entity { id : string! }"#;
        lower_emit_with(shadow, r#"{ id: "anonymous" }"#)
            .expect("Viewer is the local entity here, not the program type");
    }

    #[test]
    fn a_scalar_literal_never_fills_a_program_type_slot() {
        match lower_emit("5") {
            Err(crate::error::LowerError::EmitPayloadFieldTypeMismatch { actual, .. }) => {
                assert_eq!(actual, "number")
            }
            other => panic!("expected EmitPayloadFieldTypeMismatch, got {other:?}"),
        }
        match lower_emit(r#""anonymous""#) {
            Err(crate::error::LowerError::EmitPayloadFieldTypeMismatch { actual, .. }) => {
                assert_eq!(actual, "string")
            }
            other => panic!("expected EmitPayloadFieldTypeMismatch, got {other:?}"),
        }
    }
}

#[cfg(test)]
mod set_program_type_literal_tests {
    use crate::lower::lower_to_orb;
    use crate::parser::parse_str;

    fn lower_set(tag: &str, value: &str) -> Result<(), crate::error::LowerError> {
        lower_set_with("", tag, value)
    }

    fn lower_set_with(
        prelude: &str,
        tag: &str,
        value: &str,
    ) -> Result<(), crate::error::LowerError> {
        let src = format!(
            r#"
            {prelude}
            orbital TicketOrbital {{
                entity Ticket {{
                    id : string!
                    who : Viewer
                }}
                trait TicketMint -> {tag}Ticket [lifecycle, instance] {{
                    initial: idle
                    state idle {{
                        MINT -> idle
                            (set @entity.who {value})
                    }}
                }}
            }}
        "#
        );
        let ast = parse_str(&src).expect("parse");
        lower_to_orb(&ast)
            .map(|_| ())
            .map_err(|e| e.innermost().clone())
    }

    #[test]
    fn a_record_literal_fills_a_program_typed_entity_field() {
        for tag in ["", "@rebindable "] {
            lower_set(tag, r#"{ id: "anonymous", role: "anonymous", permissions: [], claims: [] }"#)
                .unwrap_or_else(|e| panic!("{tag:?}: {e:?}"));
        }
    }

    #[test]
    fn a_scalar_literal_never_fills_a_program_typed_entity_field() {
        match lower_set("", "5") {
            Err(crate::error::LowerError::SetValueTypeMismatch { actual_type, .. }) => {
                assert_eq!(actual_type, "number")
            }
            other => panic!("expected SetValueTypeMismatch, got {other:?}"),
        }
    }

    #[test]
    fn a_literal_missing_a_required_field_never_fills_a_program_typed_entity_field() {
        match lower_set("", r#"{ id: "anonymous", role: "anonymous", permissions: [] }"#) {
            Err(crate::error::LowerError::SetValueTypeMismatch { .. }) => {}
            other => panic!("expected SetValueTypeMismatch, got {other:?}"),
        }
    }

    #[test]
    fn a_literal_with_a_wrong_typed_field_never_fills_a_program_typed_entity_field() {
        match lower_set("", r#"{ id: 5, role: "anonymous", permissions: [], claims: [] }"#) {
            Err(crate::error::LowerError::SetValueTypeMismatch { .. }) => {}
            other => panic!("expected SetValueTypeMismatch, got {other:?}"),
        }
    }

    #[test]
    fn a_local_entity_shadows_a_program_type_at_the_set_literal_site() {
        let shadow = r#"type Viewer = Entity { id : string! }"#;
        lower_set_with(shadow, "", r#"{ id: "anonymous" }"#)
            .expect("Viewer is the local entity here, not the program type");
        assert!(lower_set("", r#"{ id: "anonymous" }"#).is_err());
    }
}
