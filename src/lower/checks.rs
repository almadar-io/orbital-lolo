use super::*;

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
            let event = tr.event.as_str();
            if let Some(guard) = &tr.guard {
                guard.collect_entity_field_refs(&mut referenced_fields, &mut set_fields, event);
            }
            for effect in &tr.effects {
                effect.collect_entity_field_refs(&mut referenced_fields, &mut set_fields, event);
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
                    &emit_payloads,
                    &config_fields,
                    &HashSet::new(),
 aliases,
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
                &emit_payloads,
                &config_fields,
                &HashSet::new(),
 aliases,
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
    emit_payloads: &HashMap<&str, &[EventField]>,
    config_fields: &HashMap<&str, &TypeExpr>,
    let_bindings: &HashSet<String>,
    aliases: &BTreeMap<String, TypeAliasResolved>,
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
                    aliases,
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
                    aliases,
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
                                aliases,
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
 aliases,
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
 aliases,
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
    aliases: &BTreeMap<String, TypeAliasResolved>,
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
                if !members.is_empty() && !orbital_core::enum_value_is_member(s, &members) {
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
                    // Resolve `@payload.X` against the declared payload schema
                    // for the firing event. A `listens` entry wins; otherwise
                    // fall back to an `emits`-declared payload so emit-sourced
                    // payload fields are type-checked the same way.
                    let payload = listen_payloads
                        .get(event)
                        .or_else(|| emit_payloads.get(event))?;
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

    if !orbital_core::set_value_type_compatible(&declared_str, &actual_str) {
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
            Some(TypeAliasResolved::Enum(members)) if members.len() >= 2 => {
                Some(members.clone())
            }
            Some(TypeAliasResolved::Type(inner)) => enum_members_of(inner, aliases),
            _ => None,
        },
        TypeExpr::Optional(inner) => enum_members_of(inner, aliases),
        _ => None,
    }
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
pub(crate) fn check_button_transitions(orbitals: &[CoreOrbital]) -> Result<(), LowerError> {
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
pub(crate) fn check_emit_payload_literals(orbitals: &[CoreOrbital]) -> Result<(), LowerError> {
    for orbital in orbitals {
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
                    check_emit_literal_recursive(effect, &emit_schemas, &orbital.name, &td.name)?;
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
                if let Some(actual) = orbital_core::literal_value_type(value) {
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

// The shallow literal-type classifier is `orbital_core::literal_value_type` —
// the single upstream source both this L1 check and the compiler L2 emit-payload
// backstop call, so they never drift. It correctly defers operator applications
// (`(array/find …)`, `(object/get …)`) to the expression inferencer instead of
// mis-typing them as `"array"` (CG-EMIT-PAYLOAD-SEXPR).

/// Parse-time checks for `uses`-imported trait/page references against the
/// baked behavior-contract slice (`crate::registry`): unknown trait/page refs
/// (3.2), rebind row-constraints (3.3), and typed config overrides (3.4).
/// Operates on the lowered (pre-inline) orbitals; a `None` registry lookup
/// (local `.orb` import or behavior absent from the slice) skips silently, and
/// the compiler re-verifies post-inline as a backstop.
pub(crate) fn check_behavior_contracts(orbitals: &[CoreOrbital]) -> Result<(), LowerError> {
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
        for ent_ref in std::iter::once(&orbital.entity)
            .chain(orbital.auxiliary_entities.iter().flatten())
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
            let Some(beh) = crate::registry::get_behavior(key) else {
                continue; // local/unknown import — not in the slice; skip.
            };

            match kind {
                "traits" => {
                    let Some(tc) = beh.trait_contract(member) else {
                        // Baked slice can lag the live registries in dev (a
                        // freshly authored trait) — skip; the compiler's
                        // post-inline validation is authoritative there.
                        if crate::registry::dev_registries_live() {
                            continue;
                        }
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
                            .filter(|r| !fields.contains(*r))
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
                    // Skipped in dev for the same reason as the unknown-member
                    // checks above: the baked contracts slice is a publish-time
                    // artifact and lags the live registries. Absence was already
                    // tolerated here; a STALE TYPE has the identical cause and
                    // was not, so changing a knob's type in the workspace made
                    // `orb emit orb` fail on every consumer until the next
                    // publish + rebake. The compiler re-checks this post-inline
                    // against the live registries (`validation/config_type.rs`
                    // via the inliner's fold at `phases/inline/trait.rs:299`),
                    // so nothing is lost — see the doc on `dev_registries_live`.
                    let skip_stale_contract_types = crate::registry::dev_registries_live();
                    if let Some(cfg) = config {
                        for (k, cf) in cfg {
                            if skip_stale_contract_types {
                                continue;
                            }
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
                                let got = orbital_core::literal_value_type(v)
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
                        if crate::registry::dev_registries_live() {
                            continue;
                        }
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
            TypeAliasResolved::Type(TypeExpr::Object(vec![
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
            ])),
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
