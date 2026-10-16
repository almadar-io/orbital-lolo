use super::*;

/// Capture which config knob an `emits { @config.<knob> -> … }` declaration
/// named, while the reference is still spelled out.
///
/// L1 is the only place this is free: the inline phase later resolves the
/// reference to a literal event name and overwrites `event`, so by L2 the
/// provenance is gone. Downstream, a `Some(knob)` marks the emit as an atom's
/// OFFER rather than an authored obligation.
fn definer_knob_of(event: &str) -> Option<String> {
    orbital_core_base::schema::config_ref_event_knob(event).map(str::to_string)
}

/// `EmitDefinition.payload_entity` / `EventDefinition.payload_entity` (G4,
/// `Almadar_Compiler_Gaps.md` §82): a payload alias's body (`type X = Event
/// <TypeExpr>`) names the WHOLE payload as a concrete, known entity — the
/// bare form `resolve_payload_type` flattens FLAT at the payload root (never
/// the generic `@entity`/`$E` sentinel, which has no name to resolve here;
/// see `flatten_bare_entity_payload`'s doc in `orbital-compiler`). Stamping
/// the entity's name is what lets the inline phase re-derive the flattened
/// field list from that entity's CURRENT (post `fields{}`/`extend{}`) shape
/// once a downstream orbital-import or trait-reference override renames or
/// extends it — without this the flattened list is frozen at THIS atom's
/// own compile time forever. `None` for a type-param sentinel, a struct
/// alias, or any other payload shape.
pub(crate) fn bare_entity_alias_name(
    payload: &TypeExpr,
    entities_by_name: &BTreeMap<String, Vec<(String, TypeExpr, bool)>>,
    type_params: &[&str],
) -> Option<String> {
    if type_param_sentinel(payload, type_params).is_some() {
        return None;
    }
    match payload {
        TypeExpr::Named(name) if entities_by_name.contains_key(name) => Some(name.clone()),
        _ => None,
    }
}

/// Lower a field's `type_expr` to its wire `(field_type, type_when)` pair.
/// For a plain type this is just `(type_expr_to_string(te), None)` — the
/// two already agree by construction, since `type_expr_to_string` itself
/// delegates a `Match`'s base type to its `_` arm. For `TypeExpr::Match`
/// this ALSO builds the `PayloadTypeWhen` list from the non-wildcard arms,
/// in authoring order (first-match-wins is `resolve_payload_type_when`'s
/// linear `find_map`, unchanged — see `orbital-core/src/schema/types.rs`).
/// Zero IR change: this is match's ONLY lowered form. Shared by
/// `lower_emit`'s inline scalar branch and the local-`listens` payload
/// lowering in `lower/trait.rs` — one conversion, not two copies.
pub(crate) fn lower_match_field_type(
    te: &TypeExpr,
) -> (String, Option<Vec<orbital_core_base::schema::PayloadTypeWhen>>) {
    let base = type_expr_to_string(te);
    let TypeExpr::Match { knob, arms } = te else {
        return (base, None);
    };
    let conditions: Vec<orbital_core_base::schema::PayloadTypeWhen> = arms
        .iter()
        .filter_map(|arm| {
            arm.pattern
                .spelling()
                .map(|equals| orbital_core_base::schema::PayloadTypeWhen {
                    knob: knob.clone(),
                    equals,
                    ty: type_expr_to_string(&arm.ty),
                })
        })
        .collect();
    (base, (!conditions.is_empty()).then_some(conditions))
}

pub(crate) fn lower_emit(
    e: &Emit,
    aliases: &BTreeMap<String, TypeAliasResolved>,
    entities_by_name: &BTreeMap<String, Vec<(String, TypeExpr, bool)>>,
    event_aliases: &BTreeMap<String, EventTypeResolved>,
    // Increment 3-II: this trait's OWN declared type-parameter names (from
    // its `(V : Type, ...)` header), usable directly in an inline `emits {
    // EVENT { field : V } }` payload field position. Distinct MECHANISM from
    // an event ALIAS's own `type_param` (Increment 3-I, resolved just
    // below) — the two are never conflated as ONE identifier space (a name
    // means something different depending which header declared it). They
    // DO need to be unioned into one `&[&str]` slice, though, at every call
    // site that walks an alias's payload BODY on this trait's behalf
    // (`type_expr_to_payload_fields` below, both the bare-name path and the
    // alias-consistency check): that walk has no way to tell which
    // mechanism produced a given bare `Named(name)` it encounters nested
    // inside a struct-alias member — `type QueueJob = { payload : tpayload!
    // }` reached through `job : QueueJob` on an event payload — so it needs
    // the full set of names THIS trait treats as type parameters, not just
    // the alias's own (§81: the split silently produced a bare "tpayload"
    // string on this path while the inline restatement correctly produced
    // the "$tpayload" sentinel via `trait_type_params` directly).
    trait_type_params: &[&str],
) -> Result<EmitDefinition, LowerError> {
    // Bare-name path: when the entry has no inline payload, look up
    // `type EVENT_NAME = Event { ... }` in the orbital-scope alias registry.
    // If found, the alias supplies payload (and scope, if the entry didn't
    // override it). If not found, the emit is payloadless. Inline payload
    // (next branch) always wins over alias lookup.
    if e.payload.is_empty() {
        if let Some(resolved) = event_aliases.get(&e.event) {
            // Union the alias's OWN type param (3-I) with this trait's
            // declared type params (3-II) — see the doc comment on
            // `trait_type_params` above for why a bare-name alias-body walk
            // needs both names in scope, not just the alias's own.
            let alias_param: Vec<&str> = resolved
                .type_param
                .as_deref()
                .into_iter()
                .chain(trait_type_params.iter().copied())
                .collect();
            let payload = Some(type_expr_to_payload_fields(
                &e.event,
                &resolved.payload,
                aliases,
                entities_by_name,
                &alias_param,
            )?);
            let payload_entity =
                bare_entity_alias_name(&resolved.payload, entities_by_name, &alias_param);
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
                description: e
                    .description
                    .clone()
                    .or_else(|| resolved.description.clone()),
                synonyms: e.synonyms.clone(),
                tier: e.tier.clone(),
                scope,
                payload_schema: payload,
                event_id: None,
                definer_knob: definer_knob_of(&e.event),
                scope_overridden: None,
                payload_entity,
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
            event_id: None,
            definer_knob: definer_knob_of(&e.event),
            scope_overridden: None,
            payload_entity: None,
        });
    }
    // Inline payload path: each field whose type_expr is `TypeExpr::Named`
    // is resolved against the type registry and flattened into prefixed
    // `name.subname` entries; primitive fields stay flat.
    let mut fields: Vec<PayloadField> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for f in &e.payload {
        let projected = f.type_expr.payload_projection();
        let ty = f.type_expr.peel();
        // Increment 3-II: a field referencing this trait's OWN declared type
        // parameter (`candidate : V` / `candidate : [V]`) lowers to the
        // `$V` / `[$V]` sentinel — checked first since a param name never
        // resolves as a struct alias or entity.
        if let Some(sentinel) = type_param_sentinel(ty, trait_type_params) {
            if !seen.insert(f.name.clone()) {
                return Err(LowerError::PayloadFieldCollision {
                    event: e.event.clone(),
                    field: f.name.clone(),
                });
            }
            fields.push(PayloadField {
                values: None,
                name: f.name.clone(),
                field_type: sentinel,
                required: if f.required { Some(true) } else { None },
                description: None,
                properties: None,
                type_when: None,
                entity: None,
                projected_from: projected.clone(),
            });
            continue;
        }
        // Anonymous struct (inline `{ … }` or a `type X = { … }` alias, NOT an
        // entity): keep it as ONE field carrying the nested shape so codegen
        // emits a strict inline object type instead of widening to
        // `EventPayload`. An entity reference is handled the same way below —
        // ONE nested field whose `properties` are the entity's fields — so the
        // emitted member name is a bare identifier (`row`), never a dotted
        // `row.category` that is illegal as a TS member name. This also matches
        // the runtime DataGrid shape `{ id, row: itemData }`.
        if let Some(props) = struct_payload_properties(ty, aliases, trait_type_params) {
            if !seen.insert(f.name.clone()) {
                return Err(LowerError::PayloadFieldCollision {
                    event: e.event.clone(),
                    field: f.name.clone(),
                });
            }
            fields.push(PayloadField {
                values: None,
                name: f.name.clone(),
                field_type: "object".to_string(),
                required: if f.required { Some(true) } else { None },
                description: None,
                properties: Some(props),
                type_when: None,
                entity: None,
                projected_from: projected.clone(),
            });
        } else if let Some(variants) = union_payload_variants(ty, aliases, trait_type_params) {
            // A TYPE union — inline (`a | [b]`) or via a named alias — lowers
            // the same way the alias-restatement path already does: parent
            // tagged `union`, variants riding in `properties`.
            if !seen.insert(f.name.clone()) {
                return Err(LowerError::PayloadFieldCollision {
                    event: e.event.clone(),
                    field: f.name.clone(),
                });
            }
            fields.push(PayloadField {
                values: None,
                name: f.name.clone(),
                field_type: "union".to_string(),
                required: if f.required { Some(true) } else { None },
                description: None,
                properties: Some(variants),
                type_when: None,
                entity: None,
                projected_from: projected.clone(),
            });
        } else if let TypeExpr::Named(type_name) = ty {
            // Scalar entity reference (`row : Note`) — resolve through
            // `entities_by_name` only (not `aliases`; struct aliases are
            // handled above), so `type_name` is a declared entity. Stamp it
            // on THIS wrapping field: the only place the entity identity
            // survives flattening into `object` + `properties`.
            let nested = resolve_payload_type(type_name, aliases, entities_by_name)?;
            if !seen.insert(f.name.clone()) {
                return Err(LowerError::PayloadFieldCollision {
                    event: e.event.clone(),
                    field: f.name.clone(),
                });
            }
            fields.push(PayloadField {
                values: None,
                name: f.name.clone(),
                field_type: "object".to_string(),
                required: if f.required { Some(true) } else { None },
                description: None,
                properties: Some(nested),
                type_when: None,
                entity: Some(type_name.clone()),
                projected_from: projected.clone(),
            });
        } else if let TypeExpr::Array(inner) = ty {
            if let TypeExpr::Named(type_name) = inner.as_ref() {
                if entities_by_name.contains_key(type_name) {
                    // Array-of-entity (`rows : [Note]`) — same flatten as the
                    // scalar arm, tagged `[object]`; the marker names the
                    // ELEMENT entity, not an array type of its own.
                    let nested = resolve_payload_type(type_name, aliases, entities_by_name)?;
                    if !seen.insert(f.name.clone()) {
                        return Err(LowerError::PayloadFieldCollision {
                            event: e.event.clone(),
                            field: f.name.clone(),
                        });
                    }
                    fields.push(PayloadField {
                        values: None,
                        name: f.name.clone(),
                        field_type: "[object]".to_string(),
                        required: if f.required { Some(true) } else { None },
                        description: None,
                        properties: Some(nested),
                        type_when: None,
                        entity: Some(type_name.clone()),
                        projected_from: projected.clone(),
                    });
                    continue;
                }
            }
            if !seen.insert(f.name.clone()) {
                return Err(LowerError::PayloadFieldCollision {
                    event: e.event.clone(),
                    field: f.name.clone(),
                });
            }
            // An array of structs keeps its element shape, as the same line in `listens` does.
            if let Some(elem) = struct_payload_properties(inner, aliases, trait_type_params) {
                fields.push(PayloadField {
                    values: None,
                    name: f.name.clone(),
                    field_type: "[object]".to_string(),
                    required: if f.required { Some(true) } else { None },
                    description: None,
                    properties: Some(elem),
                    type_when: None,
                    entity: None,
                    projected_from: projected.clone(),
                });
                continue;
            }
            let (field_type, type_when) = lower_match_field_type(ty);
            fields.push(PayloadField {
                values: payload_literal_values(ty, aliases),
                name: f.name.clone(),
                field_type,
                required: if f.required { Some(true) } else { None },
                description: None,
                properties: None,
                type_when,
                entity: None,
                projected_from: projected.clone(),
            });
        } else {
            if !seen.insert(f.name.clone()) {
                return Err(LowerError::PayloadFieldCollision {
                    event: e.event.clone(),
                    field: f.name.clone(),
                });
            }
            let (field_type, type_when) = lower_match_field_type(ty);
            fields.push(PayloadField {
                values: payload_literal_values(ty, aliases),
                name: f.name.clone(),
                field_type,
                required: if f.required { Some(true) } else { None },
                description: None,
                properties: None,
                type_when,
                entity: None,
                projected_from: projected.clone(),
            });
        }
    }
    // Alias-consistency check (syntax revision 2026-08-18): when the same
    // event ALSO carries an orbital-scope `type X = Event { … }` alias, the
    // inline restatement must MATCH it — restating is encouraged for
    // readability, silent divergence is the bug the old "inline wins"
    // precedence allowed.
    if let Some(resolved) = event_aliases.get(&e.event) {
        // Same union as the bare-name path above: the alias-consistency
        // check's alias-side derivation must see this trait's OWN type
        // params too, or a `tpayload`-shaped struct member nested inside the
        // alias's payload resolves to a bare name here while the inline
        // restatement (built via `trait_type_params` directly, a few lines
        // up) correctly resolves it to the `$tpayload` sentinel — a false
        // `PayloadAliasMismatch` (§81(b)).
        let alias_param: Vec<&str> = resolved
            .type_param
            .as_deref()
            .into_iter()
            .chain(trait_type_params.iter().copied())
            .collect();
        let alias_fields = type_expr_to_payload_fields(
            &e.event,
            &resolved.payload,
            aliases,
            entities_by_name,
            &alias_param,
        )?;
        if let Some(detail) =
            payload_fields_mismatch_norm(&fields, &alias_fields, entities_by_name, aliases)
        {
            return Err(LowerError::PayloadAliasMismatch {
                event: e.event.clone(),
                detail,
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
        definer_knob: definer_knob_of(&e.event),
        event_id: None,
        scope_overridden: None,
        // Inline restatement (author wrote `emits { EVENT { f: T } }`
        // explicitly) — never the bare-entity form, even when an
        // alias-consistency check above found it matches one.
        payload_entity: None,
    })
}

/// Order-insensitive recursive comparison of two lowered payload shapes.
/// Returns `Some(detail)` naming the first divergence, `None` when they
/// agree. Sentinels (`$v`, `@entity`) compare as their literal strings —
/// alias and inline lower them identically, so honest restatements match.
///
/// Normalizes NAMED field types first: the two lowering paths resolve names
/// against different scopes, so the same authored shape (`data : [ObjectSpec]`,
/// `row : TradeOrder`) can arrive as a bare/bracketed NAME on one side and a
/// flattened `object`/`[object]` + properties on the other. Both entity names
/// (`entities_by_name`) and struct aliases (`aliases`) flatten before
/// comparing — at EVERY nesting depth: the recursive call for a nested
/// `properties` pair (below) threads the same two maps through rather than
/// resolving against an empty registry, which previously left a depth-2+
/// bare name unresolved on one side while the other had already flattened it
/// via its own (correctly recursive) lowering path, reporting a false
/// mismatch (`C-EVENT-ALIAS-NESTED-STRUCT-DEPTH2`).
pub(crate) fn payload_fields_mismatch_norm(
    inline: &[PayloadField],
    alias: &[PayloadField],
    entities_by_name: &BTreeMap<String, Vec<(String, TypeExpr, bool)>>,
    aliases: &BTreeMap<String, TypeAliasResolved>,
) -> Option<String> {
    let flatten_name = |name: &str| -> Option<Vec<PayloadField>> {
        // CYCLE GUARD — the same `RESOLVING` thread-local `lower/entity.rs`
        // and `struct_payload_properties` already share, applied to the
        // ENTITY branch below (which resolves through `entities_by_name`, not
        // through `struct_payload_properties`, so it was the one path the
        // guard did not cover). A relation whose target is still being
        // flattened on this path — `entity Tag { parentId : Tag }`, the
        // ordinary way the corpus models a hierarchy — is an ID REFERENCE,
        // not a nested row; flattening it again both overstates the payload
        // and never terminates (verified: stack overflow, §75).
        if crate::lower::entity::RESOLVING.with(|r| r.borrow().contains(name)) {
            return None;
        }
        // ONE-OWNER FIX (§81): try the ALIAS registry first, via the SAME
        // `struct_payload_properties` cascade every other named-type walk in
        // this file already goes through (`type_expr_to_payload_fields`'s
        // Object branch tries it before `named_entity_fields` too). A name
        // is a VALUE TYPE (`type X = { … }`) here whenever `aliases` resolves
        // it to an `Object` body — recursing through it gets the SAME
        // cyclic-back-reference handling the inline-payload construction
        // path already applies (`resolve_payload_type` → this same
        // function), so a self-referential struct member (`additionalProper
        // ties : X` inside `type X = { …, additionalProperties : X }`)
        // degrades to the identical by-name marker on both comparison sides
        // instead of one side re-deriving it fresh and disagreeing.
        //
        // `entities_by_name` is a DUAL-PURPOSE map (`collect_entity_type_
        // bodies` populates it from BOTH `entity X { … }` declarations AND
        // plain `type X = { … }` struct aliases) — so a struct-alias name is
        // frequently ALSO present there. Before this fix, this closure
        // checked `entities_by_name` FIRST and never reached the alias
        // cascade at all for such a name: `struct_payload_properties`
        // recurses (cyclic-aware); the entities_by_name branch below is a
        // flat, NON-recursive `type_expr_to_string` map with no cycle
        // detection of its own — the exact shape a genuine RELATION wants
        // (an entity self-reference is an ID reference, never expanded) but
        // the WRONG shape for a value-typed struct alias reached the same
        // way. Trying the alias cascade first fixes the struct case without
        // changing the relation case: `struct_payload_properties` returns
        // `None` for a genuine entity name (entities are lowered to an
        // opaque `object` alias body, never `TypeExpr::Object`, so the
        // `Named` match arm's `_ => return None` fires) and this closure
        // falls through to the entities_by_name branch exactly as before.
        if let Some(props) =
            struct_payload_properties(&TypeExpr::Named(name.to_string()), aliases, &[])
        {
            return Some(props);
        }
        entities_by_name.get(name).map(|fields| {
            fields
                .iter()
                .map(|(n, t, req)| PayloadField {
                    values: payload_literal_values(t, aliases),
                    name: n.clone(),
                    field_type: type_expr_to_string(t),
                    required: if *req { Some(true) } else { None },
                    description: None,
                    properties: None,
                    type_when: None,
                    entity: None,
                    projected_from: None,
                })
                .collect()
        })
    };
    // Paired with the name it flattened (`None` when nothing was), so the
    // nested comparison can add that name to `expanding`.
    let normalize = |f: &PayloadField| -> (PayloadField, Option<String>) {
        if f.properties.is_none() {
            let (base, is_array) = match f.field_type.strip_prefix('[') {
                Some(rest) => (rest.trim_end_matches(']'), true),
                None => (f.field_type.as_str(), false),
            };
            if let Some(props) = flatten_name(base) {
                return (
                    PayloadField {
                        values: None,
                        name: f.name.clone(),
                        field_type: if is_array {
                            "[object]".to_string()
                        } else {
                            "object".to_string()
                        },
                        required: f.required,
                        description: None,
                        properties: Some(props),
                        type_when: None,
                        // Comparison-only reconstruction (never emitted) —
                        // `base` may be an entity OR a struct alias name
                        // here; the marker isn't read by this comparator.
                        entity: None,
                        projected_from: None,
                    },
                    Some(base.to_string()),
                );
            }
        }
        (f.clone(), None)
    };
    let an: Vec<(PayloadField, Option<String>)> = inline.iter().map(normalize).collect();
    let bn: Vec<(PayloadField, Option<String>)> = alias.iter().map(normalize).collect();
    let mut a: Vec<&(PayloadField, Option<String>)> = an.iter().collect();
    let mut b: Vec<&(PayloadField, Option<String>)> = bn.iter().collect();
    a.sort_by(|x, y| x.0.name.cmp(&y.0.name));
    b.sort_by(|x, y| x.0.name.cmp(&y.0.name));
    if a.len() != b.len() {
        let names = |v: &[&(PayloadField, Option<String>)]| {
            v.iter()
                .map(|f| f.0.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        };
        return Some(format!(
            "inline declares fields [{}], alias declares [{}]",
            names(&a),
            names(&b)
        ));
    }
    for (pair_a, pair_b) in a.iter().zip(b.iter()) {
        let (fa, flattened) = (&pair_a.0, pair_a.1.as_deref());
        let fb = &pair_b.0;
        if fa.name != fb.name {
            return Some(format!(
                "inline field '{}' vs alias field '{}'",
                fa.name, fb.name
            ));
        }
        if fa.field_type != fb.field_type {
            return Some(format!(
                "field '{}': inline type '{}' vs alias type '{}'",
                fa.name, fa.field_type, fb.field_type
            ));
        }
        if fa.required.unwrap_or(false) != fb.required.unwrap_or(false) {
            return Some(format!("field '{}': required flags disagree", fa.name));
        }
        match (&fa.properties, &fb.properties) {
            (Some(pa), Some(pb)) => {
                // Mark the name whose fields produced this nested pair, so a
                // relation back to it is left as a reference one level down —
                // insert/recurse/remove, the same discipline
                // `struct_payload_properties` uses.
                let guarded = flattened.filter(|n| {
                    crate::lower::entity::RESOLVING
                        .with(|r| r.borrow_mut().insert((*n).to_string()))
                });
                let nested = payload_fields_mismatch_norm(pa, pb, entities_by_name, aliases);
                if let Some(n) = guarded {
                    crate::lower::entity::RESOLVING.with(|r| {
                        r.borrow_mut().remove(n);
                    });
                }
                if let Some(d) = nested {
                    return Some(format!("field '{}': {}", fa.name, d));
                }
            }
            (None, None) => {}
            _ => {
                return Some(format!(
                    "field '{}': nested shape declared on one side only",
                    fa.name
                ));
            }
        }
    }
    None
}

/// Flatten a `TypeExpr` (from a `type X = Event <T>` alias or similar) into
/// a `Vec<PayloadField>`. Shared by `lower_emit`'s bare-name alias-lookup
/// path and the `stateMachine.events[]` builder (so events that only appear
/// in orbital-scope type declarations still carry their payload shape into
/// the IR).
pub(crate) fn type_expr_to_payload_fields(
    event_name: &str,
    payload: &TypeExpr,
    aliases: &BTreeMap<String, TypeAliasResolved>,
    entities_by_name: &BTreeMap<String, Vec<(String, TypeExpr, bool)>>,
    type_params: &[&str],
) -> Result<Vec<PayloadField>, LowerError> {
    // A bare (or array-of) reference to the alias's own type parameter at
    // the payload's top level (`type X (E : Entity) = Event E` /
    // `Event [E]`) lowers to the `$E` sentinel directly — same contract as
    // the nested-field case below.
    if let Some(sentinel) = type_param_sentinel(payload, type_params) {
        return Ok(vec![PayloadField {
            values: None,
            name: "value".to_string(),
            field_type: sentinel,
            required: None,
            description: None,
            properties: None,
            type_when: None,
            entity: None,
            projected_from: None,
        }]);
    }
    match payload {
        TypeExpr::Named(type_name) => resolve_payload_type(type_name, aliases, entities_by_name),
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
                // A field whose type is itself an inline `{ … }` struct (or a
                // struct alias / array of them) carries its nested shape via
                // `properties` — the same contract emits payloads use. Without
                // this, `payload : { id : string … }` in a listens block lost
                // its shape and lowered to a shapeless `object` (found by
                // Gate B of the type-integrity campaign).
                //
                // A field referencing the alias's own type parameter
                // (`row : E` / `row : [E]`) lowers to the `$E` / `[$E]`
                // sentinel instead — checked first since a param name never
                // resolves as a struct alias.
                let sentinel = type_param_sentinel(ftype, type_params);
                let array_elem = match ftype {
                    TypeExpr::Array(inner) => {
                        struct_payload_properties(inner, aliases, type_params)
                    }
                    _ => None,
                };
                let (field_type, properties, entity) = if let Some(s) = sentinel {
                    (s, None, None)
                } else if let Some(elem) = array_elem {
                    ("[object]".to_string(), Some(elem), None)
                } else if let Some(nested) = struct_payload_properties(ftype, aliases, type_params)
                {
                    ("object".to_string(), Some(nested), None)
                } else if let Some(variants) = union_payload_variants(ftype, aliases, type_params) {
                    ("union".to_string(), Some(variants), None)
                } else if let Some((entity_name, entity_fields, is_array)) =
                    named_entity_fields(ftype, entities_by_name, aliases)
                {
                    // An ENTITY named in a field position (scalar OR array of
                    // the element entity) expands to `object`/`[object]` +
                    // its own fields — the same shape `lower_emit`'s inline
                    // path already produced for the identical authored line,
                    // now also carrying the entity's name. Without this the
                    // alias-only path left a raw entity NAME in `field_type`
                    // with no `properties`, so one authored `{ row : E }`
                    // lowered to two different `.orb` shapes depending on
                    // whether an inline restatement happened to exist, and
                    // the raw name reached codegen as a bare `Reference`
                    // (which rendered as `string` in events.ts).
                    // `payload_fields_mismatch_norm` normalized the two for
                    // COMPARISON only; nothing normalized what was emitted.
                    (
                        if is_array { "[object]" } else { "object" }.to_string(),
                        Some(entity_fields),
                        Some(entity_name),
                    )
                } else if is_cyclic_ref(ftype) {
                    (type_expr_to_string(ftype), Some(Vec::new()), None)
                } else {
                    (type_expr_to_string(ftype), None, None)
                };
                flat.push(PayloadField {
                    values: None,
                    name: fname.clone(),
                    field_type,
                    required: if *freq { Some(true) } else { None },
                    description: None,
                    properties,
                    type_when: None,
                    entity,
                    projected_from: None,
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
                values: None,
                name: "value".to_string(),
                field_type: type_expr_to_string(other),
                required: None,
                description: None,
                properties: None,
                type_when: None,
                entity: None,
                projected_from: None,
            }])
        }
    }
}

/// If `te` names a known ENTITY — scalar `Named(name)` (`row : Note`) or
/// `Array(Named(name))` (`rows : [Note]`, the ELEMENT entity) — return
/// `(name, fields, is_array)`: the entity's name (stamped onto the caller's
/// wrapping `PayloadField.entity`), its fields flattened one level (the
/// payload shape an entity reference in a field position carries), and
/// whether `te` was the array form.
///
/// Deliberately non-propagating, unlike `lower_emit`'s inline path which
/// `?`-propagates `UnknownPayloadType`: a name that is not an entity (a struct
/// alias already handled by `struct_payload_properties`, a type parameter, a
/// primitive) simply yields `None` so the caller keeps its existing string
/// form. Making this arm fatal would reject files that lower fine today.
pub(crate) fn named_entity_fields(
    te: &TypeExpr,
    entities_by_name: &BTreeMap<String, Vec<(String, TypeExpr, bool)>>,
    aliases: &BTreeMap<String, TypeAliasResolved>,
) -> Option<(String, Vec<PayloadField>, bool)> {
    let (name, is_array) = match te {
        TypeExpr::Named(n) => (n.as_str(), false),
        TypeExpr::Array(inner) => match inner.as_ref() {
            TypeExpr::Named(n) => (n.as_str(), true),
            _ => return None,
        },
        _ => return None,
    };
    let fields = entities_by_name.get(name)?;
    Some((
        name.to_string(),
        fields
            .iter()
            .map(|(n, t, req)| PayloadField {
                values: payload_literal_values(t, aliases),
                name: n.clone(),
                field_type: type_expr_to_string(t),
                required: if *req { Some(true) } else { None },
                description: None,
                properties: None,
                type_when: None,
                entity: None,
                projected_from: None,
            })
            .collect(),
        is_array,
    ))
}

/// If `te` is a reference to one of `type_params` — `Named(param)` or
/// `Array(Named(param))` — return the compiler's substitution sentinel
/// (`"$param"` / `"[$param]"`). `None` when `type_params` is empty or `te`
/// doesn't reference any of them. Shared by both type-parameter mechanisms:
/// a `type X (E : Entity) = ...` alias's own single param (Increment 3-I,
/// callers pass a one-element slice) and a trait's own `(V : Type, ...)`
/// header (Increment 3-II, callers pass the full declared set). The inline
/// phase (`orbital-compiler`) resolves the sentinel against either the using
/// trait's bound entity (3-I / Entity-kind default) or an explicit call-site
/// `:: <param> <arg>` argument (3-II); a surviving sentinel with neither
/// becomes `ORB_T_UNRESOLVED_TYPE_PARAM`.
pub(crate) fn type_param_sentinel(te: &TypeExpr, type_params: &[&str]) -> Option<String> {
    if type_params.is_empty() {
        return None;
    }
    match te {
        TypeExpr::Named(n) if type_params.contains(&n.as_str()) => Some(format!("${n}")),
        TypeExpr::Array(inner) => match inner.as_ref() {
            TypeExpr::Named(n) if type_params.contains(&n.as_str()) => Some(format!("[${n}]")),
            _ => None,
        },
        _ => None,
    }
}

/// If `te` is an anonymous struct — an inline `{ … }` object or a
/// `type X = { … }` struct alias (NOT an entity reference) — return its fields
/// as nested `PayloadField`s so codegen can emit a strict inline object type.
/// Returns `None` for entities, primitives, arrays, etc. (those keep the
/// existing flat / dotted-flatten handling). Recurses for nested structs.
/// The variants of a TYPE union in PAYLOAD position, one `PayloadField` each.
///
/// A payload field typed `string | number | boolean | [string] | FileValue` is a
/// union, not a vocabulary, and `type_expr_to_string` answers `"string"` for
/// EVERY union — so before this the field lowered as a plain string and the
/// render-ui binding checker reported the trait "declares it as 'string'",
/// contradicting the `.lolo` it came from.
///
/// Uses the ONE union predicate (`entity::is_type_union`) so payload position
/// agrees with entity position; a vocabulary (`up | down | left | right`) is not
/// a union here either and keeps falling through to the enum/string reading.
///
/// No schema change: `PayloadField.properties` is a `Vec<PayloadField>` whose
/// elements carry names, so the variants ride in it exactly as a struct's
/// members do, tagged by `field_type: "union"` on the parent.
pub(crate) fn union_payload_variants(
    te: &TypeExpr,
    aliases: &BTreeMap<String, TypeAliasResolved>,
    type_params: &[&str],
) -> Option<Vec<PayloadField>> {
    let parts: &Vec<TypeExpr> = match te {
        TypeExpr::Union(parts) => parts,
        TypeExpr::Named(n) => match aliases.get(n) {
            Some(TypeAliasResolved::Type(TypeExpr::Union(parts))) => parts,
            _ => return None,
        },
        _ => return None,
    };
    if !crate::lower::entity::is_type_union(parts, aliases) {
        return None;
    }
    let mut out = Vec::with_capacity(parts.len());
    for p in parts {
        let (name, field_type, properties) = match p {
            TypeExpr::Array(inner) => {
                match struct_payload_properties(inner, aliases, type_params) {
                    Some(elem) => ("array".to_string(), "[object]".to_string(), Some(elem)),
                    None if is_cyclic_ref(p) => (
                        "array".to_string(),
                        format!("[{}]", type_expr_to_string(inner)),
                        Some(Vec::new()),
                    ),
                    None => (
                        "array".to_string(),
                        format!("[{}]", type_expr_to_string(inner)),
                        None,
                    ),
                }
            }
            TypeExpr::Named(n) => match struct_payload_properties(p, aliases, type_params) {
                Some(nested) => (n.clone(), "object".to_string(), Some(nested)),
                None if is_cyclic_ref(p) => (n.clone(), type_expr_to_string(p), Some(Vec::new())),
                None => (n.clone(), type_expr_to_string(p), None),
            },
            TypeExpr::Primitive(n) => (n.clone(), type_expr_to_string(p), None),
            _ => return None,
        };
        out.push(PayloadField {
            values: None,
            name,
            field_type,
            required: None,
            description: None,
            properties,
            type_when: None,
            entity: None,
            projected_from: None,
        });
    }
    Some(out)
}

/// True when `ty` is `Named(name)` or `Array(Named(name))` and `name` is
/// currently mid-expansion on this thread (`lower/entity.rs`'s `RESOLVING`
/// cycle guard, shared with this module's own struct/union walkers below).
/// Distinguishes "a `None` here was a CYCLE" from "a `None` here is a
/// genuine miss" (an entity name, an unresolved identifier, a primitive) —
/// the two must degrade differently: a cycle keeps its by-name reference
/// (`properties: Some([])`, the marker `resolve.rs`'s
/// `payload_field_to_oir_type` reads as `OirType::TypeRef`), a genuine miss
/// stays the historical bare string (`properties: None`).
fn is_cyclic_ref(ty: &TypeExpr) -> bool {
    let name = match ty {
        TypeExpr::Named(n) => n.as_str(),
        TypeExpr::Array(inner) => match inner.as_ref() {
            TypeExpr::Named(n) => n.as_str(),
            _ => return false,
        },
        _ => return false,
    };
    crate::lower::entity::RESOLVING.with(|r| r.borrow().contains(name))
}

pub(crate) fn struct_payload_properties(
    te: &TypeExpr,
    aliases: &BTreeMap<String, TypeAliasResolved>,
    type_params: &[&str],
) -> Option<Vec<PayloadField>> {
    let (guard_name, items): (Option<&str>, &Vec<(String, TypeExpr, bool)>) = match te.peel() {
        TypeExpr::Object(items) => (None, items),
        TypeExpr::Named(n) => match aliases.get(n) {
            Some(TypeAliasResolved::Type(TypeExpr::Object(items))) => (Some(n.as_str()), items),
            _ => return None,
        },
        _ => return None,
    };
    // CYCLE GUARD — the payload-position twin of `lower/entity.rs`'s
    // RESOLVING guard (same thread-local, same rule): re-entering a struct
    // alias that's still being expanded would recurse forever (`type
    // MenuItem = { subMenu : [MenuItem] }`; verified: stack overflow before
    // this existed). Only a NAMED entry has a name to guard on — the literal
    // `TypeExpr::Object` form (`lower/mod.rs`'s orbital-scope `types` map
    // calls this with the alias's own body directly, not a `Named` reference
    // to it) has none at THIS frame; its recursion still terminates one
    // level down, the next time the same alias is reached through a `Named`
    // child.
    if let Some(n) = guard_name {
        if !crate::lower::entity::RESOLVING.with(|r| r.borrow_mut().insert(n.to_string())) {
            return None; // cycle — caller degrades to the by-name marker
        }
    }
    let mut props = Vec::with_capacity(items.len());
    for (name, ty, req) in items {
        let projected = ty.payload_projection();
        let ty = ty.peel();
        // An array-of-struct field (`[StructAlias]` / `[{ … }]`) carries the
        // ELEMENT shape: `field_type` is `[object]` (signals array) and
        // `properties` holds the element's fields — the resolve phase wraps it
        // in `OirType::Array(Struct{…})` so codegen emits a strict `{ … }[]`
        // instead of a dangling named reference. A direct struct field is
        // `object` + `properties`; everything else stays a flat type string.
        // A field referencing the alias's own type parameter lowers to the
        // `$E` / `[$E]` sentinel — checked first (a param name never
        // resolves as a struct alias).
        let sentinel = type_param_sentinel(ty, type_params);
        let array_elem = match ty {
            TypeExpr::Array(inner) => struct_payload_properties(inner, aliases, type_params),
            _ => None,
        };
        let (field_type, properties) = if let Some(s) = sentinel {
            (s, None)
        } else if let Some(elem) = array_elem {
            ("[object]".to_string(), Some(elem))
        } else if let Some(nested) = struct_payload_properties(ty, aliases, type_params) {
            ("object".to_string(), Some(nested))
        } else if let Some(variants) = union_payload_variants(ty, aliases, type_params) {
            ("union".to_string(), Some(variants))
        } else if let Some(enum_ty) = enum_alias_payload_type(ty, aliases) {
            (enum_ty, None)
        } else if is_cyclic_ref(ty) {
            // The by-name back-reference marker: `field_type` keeps the
            // bare/array name string, `properties` is Some-but-EMPTY (never
            // produced by a genuine miss) so `resolve.rs` can tell this apart
            // from an unresolved identifier and emit `OirType::TypeRef`.
            (type_expr_to_string(ty), Some(Vec::new()))
        } else {
            (type_expr_to_string(ty), None)
        };
        props.push(PayloadField {
            values: payload_literal_values(ty, aliases),
            name: name.clone(),
            field_type,
            required: if *req { Some(true) } else { None },
            description: None,
            properties,
            type_when: None,
            entity: None,
            projected_from: projected,
        });
    }
    if let Some(n) = guard_name {
        crate::lower::entity::RESOLVING.with(|r| {
            r.borrow_mut().remove(n);
        });
    }
    Some(props)
}

/// A payload field's closed value set: a literal union (inline or via an alias), a single
/// literal, or an enum alias — element-wise for an array.
pub(crate) fn payload_literal_values(
    ty: &TypeExpr,
    aliases: &BTreeMap<String, TypeAliasResolved>,
) -> Option<Vec<String>> {
    // As on an entity field: a union of literals and bare words (not type aliases) is a closed set;
    // a bare word may spell a primitive (`date | currency`), but an all-primitive union
    // (`string | number`) is a type union.
    let literals = |parts: &[TypeExpr]| -> Option<Vec<String>> {
        let word = |p: &TypeExpr| {
            matches!(p, TypeExpr::StringLiteral(_))
                || matches!(p, TypeExpr::Named(n) if !aliases.contains_key(n))
        };
        if !parts.iter().any(word) {
            return None;
        }
        parts
            .iter()
            .map(|p| match p {
                TypeExpr::StringLiteral(s) => Some(s.clone()),
                TypeExpr::Named(n) if !aliases.contains_key(n) => Some(n.clone()),
                TypeExpr::Primitive(n) => Some(n.clone()),
                _ => None,
            })
            .collect::<Option<Vec<String>>>()
            .filter(|v| !v.is_empty())
    };
    match ty.peel() {
        TypeExpr::StringLiteral(s) => Some(vec![s.clone()]),
        TypeExpr::Union(parts) => literals(parts),
        TypeExpr::Named(n) => match aliases.get(n) {
            Some(TypeAliasResolved::Enum(values)) => Some(values.clone()),
            Some(TypeAliasResolved::Type(TypeExpr::Union(parts))) => literals(parts),
            _ => None,
        },
        TypeExpr::Array(inner) => payload_literal_values(inner, aliases),
        _ => None,
    }
}

/// In a payload an enum travels as its literal value — a `string`, as the entity path lowers
/// it — so a field typed by a literal-union alias must not leave the alias name behind.
fn enum_alias_payload_type(
    ty: &TypeExpr,
    aliases: &BTreeMap<String, TypeAliasResolved>,
) -> Option<String> {
    let literal_union = |parts: &[TypeExpr]| {
        !parts.is_empty()
            && parts
                .iter()
                .all(|p| matches!(p, TypeExpr::StringLiteral(_)))
    };
    match ty.peel() {
        TypeExpr::Named(n) => match aliases.get(n) {
            Some(TypeAliasResolved::Enum(_)) => Some("string".to_string()),
            Some(TypeAliasResolved::Type(TypeExpr::Union(parts))) if literal_union(parts) => {
                Some("string".to_string())
            }
            _ => None,
        },
        TypeExpr::Union(parts) if literal_union(parts) => Some("string".to_string()),
        TypeExpr::Array(inner) => {
            enum_alias_payload_type(inner, aliases).map(|elem| format!("[{elem}]"))
        }
        _ => None,
    }
}

/// Resolve a named type referenced from an emit payload into a flat
/// `Vec<PayloadField>`. Looks up first the orbital-local alias registry
/// (for `type Foo = Entity { ... }` declarations) and then the same-orbital
/// entity declarations (so `emits { X :: MyEntity }` works when `MyEntity`
/// is the orbital's own entity). Unknown names surface as `LowerError::Type`;
/// the type resolver (`check_program`) catches most of these before lowering.
///
/// Delegates the entity's own field list to `struct_payload_properties` (the
/// SAME struct/array-of-struct/union/cyclic-marker cascade a `type X = {
/// ... }` alias body runs through) rather than a bespoke flat mapping — an
/// entity field that is itself a named-struct-alias or array-of-named-
/// struct-alias (e.g. `AstarScene.nodes : [GraphNodeOut]`, whose element
/// type `badge : Badge` needs its OWN `properties`) used to flatten to a
/// bare type string (`"[GraphNodeOut]"`, `properties: None`) one level
/// short of the entity-field path (`lower/entity.rs`), which already keeps
/// nested `properties` correctly. Wrapping the entity's fields in a
/// synthetic `TypeExpr::Object` reuses that ONE cascade instead of
/// duplicating it — `struct_payload_properties` always returns `Some` for
/// an `Object` variant (no cycle guard needed for the synthetic, unnamed
/// wrapper), so `unwrap_or_default` never actually falls through.
fn resolve_payload_type(
    name: &str,
    aliases: &BTreeMap<String, TypeAliasResolved>,
    entities_by_name: &BTreeMap<String, Vec<(String, TypeExpr, bool)>>,
) -> Result<Vec<PayloadField>, LowerError> {
    if let Some(fields) = entities_by_name.get(name) {
        return Ok(
            struct_payload_properties(&TypeExpr::Object(fields.clone()), aliases, &[])
                .unwrap_or_default(),
        );
    }
    Err(LowerError::UnknownPayloadType {
        name: name.to_string(),
    })
}

pub(crate) fn lower_listen(l: &Listen) -> ListenDefinition {
    let payload_mapping = if l.payload_mapping.is_empty() {
        None
    } else {
        Some(
            l.payload_mapping
                .iter()
                .map(|(k, v)| (k.clone(), desugar_expr_match(v)))
                .collect::<orbital_core_base::schema::SObject>(),
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
        guard: l.guard.as_ref().map(desugar_expr_match),
        source,
        event_id: None,
        triggers_id: None,
    }
}

/// Split the parser's concatenated event string into (bare_event, source).
pub(crate) fn split_listen_source(
    raw: &str,
) -> (String, Option<orbital_core_base::schema::ListenSource>) {
    orbital_core_base::schema::split_event_address(raw)
}

pub(crate) fn lower_tick(t: &Tick) -> Result<TickDefinition, LowerError> {
    let interval = match &t.interval {
        TickInterval::Millis(n) => StringOrNumber::Number(*n as f64),
        TickInterval::Cron(s) => StringOrNumber::String(s.clone()),
    };
    Ok(TickDefinition {
        name: t.name.clone(),
        interval,
        guard: t.guard.as_ref().map(desugar_expr_match),
        effects: t
            .effects
            .iter()
            .map(|e| desugar_notify(&desugar_expr_match(e)))
            .collect::<Result<Vec<_>, LowerError>>()?,
        applies_to: None,
        description: None,
        background: t.background,
        while_visible: t.visible,
    })
}

#[cfg(test)]
mod recursive_struct_payload_tests {
    use super::*;

    fn menu_item_aliases() -> BTreeMap<String, TypeAliasResolved> {
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
        aliases
    }

    /// Feature test 1 (payload half): a payload field typed directly by a
    /// self-referential struct alias must TERMINATE (no infinite recursion —
    /// `struct_payload_properties` previously had no cycle guard on this
    /// path, verified: hung/stack-overflowed before this fix) and carry the
    /// by-name back-reference marker (`properties: Some([])`) at the
    /// recursion point, mirroring `lower/entity.rs`'s entity-side shape.
    #[test]
    fn recursive_struct_payload_field_terminates_with_backref_marker() {
        let aliases = menu_item_aliases();
        let props =
            struct_payload_properties(&TypeExpr::Named("MenuItem".to_string()), &aliases, &[])
                .expect("MenuItem is a struct alias");
        let sub_menu = props
            .iter()
            .find(|f| f.name == "subMenu")
            .expect("subMenu field present");
        assert_eq!(sub_menu.field_type, "[MenuItem]");
        assert!(
            matches!(&sub_menu.properties, Some(p) if p.is_empty()),
            "the recursion point carries the EMPTY (not absent) properties marker"
        );

        // The guard must release after each top-level call, so resolving the
        // SAME alias twice in a row (e.g. two payload fields both typed
        // MenuItem) doesn't see a leaked guard entry and wrongly treat the
        // second call as a cycle too.
        let props2 =
            struct_payload_properties(&TypeExpr::Named("MenuItem".to_string()), &aliases, &[])
                .expect("guard release lets a second use re-expand");
        assert_eq!(props2.len(), 2, "second call fully expands MenuItem again");
    }

    /// `type_expr_to_payload_fields`'s `Object` branch (an inline `{ … }`
    /// event payload with a FIELD typed by the recursive alias — the shape
    /// `emits { EVENT { row : MenuItem } }` produces) must also terminate
    /// and carry the same by-name marker.
    #[test]
    fn type_expr_to_payload_fields_object_branch_terminates_on_recursive_field() {
        let aliases = menu_item_aliases();
        let entities: BTreeMap<String, Vec<(String, TypeExpr, bool)>> = BTreeMap::new();
        let payload = TypeExpr::Object(vec![(
            "row".to_string(),
            TypeExpr::Named("MenuItem".to_string()),
            true,
        )]);
        let fields = type_expr_to_payload_fields("ItemPicked", &payload, &aliases, &entities, &[])
            .expect("payload resolves");
        let row = fields.iter().find(|f| f.name == "row").expect("row field");
        assert_eq!(row.field_type, "object");
        let props = row
            .properties
            .as_ref()
            .expect("row carries MenuItem's fields");
        let sub_menu = props
            .iter()
            .find(|f| f.name == "subMenu")
            .expect("subMenu present");
        assert_eq!(sub_menu.field_type, "[MenuItem]");
        assert!(matches!(&sub_menu.properties, Some(p) if p.is_empty()));
    }

    /// `C-EVENT-ALIAS-NESTED-STRUCT-DEPTH2`: an event payload with a
    /// depth-2 nested struct alias (`containers : [ContainerItem]` where
    /// A self-referential relation — `entity Tag { parentId : Tag }`, the
    /// ordinary way the corpus models a hierarchy — must not send the payload
    /// comparator into unbounded recursion.
    ///
    /// `payload_fields_mismatch_norm` normalizes a field whose type NAMES an
    /// entity by inlining that entity's fields. On `Tag` that produces another
    /// `parentId : Tag`, which the nested comparison then normalizes again:
    /// before the cycle guard this overflowed the stack outright (10 corpus
    /// behaviors could not be compiled at all, §75 in
    /// `docs/Almadar_Compiler_Gaps.md`), and no stack size helped because the
    /// recursion has no bottom. A relation is an ID REFERENCE, not a nested
    /// row, so the guard leaves it as its named type on the second encounter.
    #[test]
    fn a_self_referential_relation_terminates() {
        let mut entities_by_name: BTreeMap<String, Vec<(String, TypeExpr, bool)>> = BTreeMap::new();
        entities_by_name.insert(
            "Tag".to_string(),
            vec![
                (
                    "id".to_string(),
                    TypeExpr::Primitive("string".to_string()),
                    true,
                ),
                (
                    "name".to_string(),
                    TypeExpr::Primitive("string".to_string()),
                    true,
                ),
                // The recursion: a Tag field whose type is Tag.
                (
                    "parentId".to_string(),
                    TypeExpr::Named("Tag".to_string()),
                    false,
                ),
            ],
        );
        let aliases: BTreeMap<String, TypeAliasResolved> = BTreeMap::new();

        let side = vec![PayloadField {
            values: None,
            name: "row".to_string(),
            field_type: "Tag".to_string(),
            required: Some(true),
            description: None,
            properties: None,
            type_when: None,
            entity: None,
            projected_from: None,
        }];

        // Terminating at all is the assertion; agreeing is the consequence.
        assert_eq!(
            payload_fields_mismatch_norm(&side, &side, &entities_by_name, &aliases),
            None,
            "a row typed by a self-referential entity matches itself"
        );
    }

    /// `ContainerItem` itself has `ports : [PortItem]`, both plain struct
    /// aliases — no entity involved) must round-trip when the trait's
    /// `emits { }` block RESTATES the alias's payload inline (the shape
    /// `lolo-service`-generated service atoms author, and any hand-authored
    /// atom that restates a `type X = Event { … }` alias for readability).
    ///
    /// Before the fix, `payload_fields_mismatch_norm`'s recursive
    /// nested-`properties` comparison called `payload_fields_mismatch`,
    /// which hardcoded EMPTY `entities_by_name`/`aliases` maps instead of
    /// threading the real ones through — so `ports`'s bare `[PortItem]`
    /// name (produced by `lower_emit`'s inline-payload loop, which has no
    /// array-of-named-struct branch and falls back to a bare type string)
    /// could not be re-flattened at depth 2 the way it already was at depth
    /// 1, and a real match false-mismatched as
    /// `ORB_EMIT_PAYLOAD_FIELD_TYPE_MISMATCH`-shaped `PayloadAliasMismatch`.
    #[test]
    fn depth2_nested_struct_alias_inline_restatement_matches_alias() {
        let port_item = TypeAliasResolved::Type(TypeExpr::Object(vec![
            (
                "privatePort".to_string(),
                TypeExpr::Primitive("number".to_string()),
                true,
            ),
            (
                "publicPort".to_string(),
                TypeExpr::Primitive("number".to_string()),
                true,
            ),
        ]));
        let container_item = TypeAliasResolved::Type(TypeExpr::Object(vec![
            (
                "id".to_string(),
                TypeExpr::Primitive("string".to_string()),
                true,
            ),
            (
                "ports".to_string(),
                TypeExpr::Array(Box::new(TypeExpr::Named("PortItem".to_string()))),
                true,
            ),
        ]));
        let mut aliases: BTreeMap<String, TypeAliasResolved> = BTreeMap::new();
        aliases.insert("PortItem".to_string(), port_item);
        aliases.insert("ContainerItem".to_string(), container_item);

        let mut event_aliases: BTreeMap<String, EventTypeResolved> = BTreeMap::new();
        event_aliases.insert(
            "ListDone".to_string(),
            EventTypeResolved {
                payload: TypeExpr::Object(vec![(
                    "containers".to_string(),
                    TypeExpr::Array(Box::new(TypeExpr::Named("ContainerItem".to_string()))),
                    true,
                )]),
                scope: None,
                description: None,
                type_param: None,
            },
        );

        let entities_by_name: BTreeMap<String, Vec<(String, TypeExpr, bool)>> = BTreeMap::new();

        // The trait's `emits { ListDone -> internal { containers : [ContainerItem] } }`
        // restatement — same field/type as the alias, authored inline.
        let emit = Emit {
            event: "ListDone".to_string(),
            scope: Some("internal".to_string()),
            payload: vec![EventField {
                name: "containers".to_string(),
                type_expr: TypeExpr::Array(Box::new(TypeExpr::Named("ContainerItem".to_string()))),
                required: true,
                span: Span::dummy(),
            }],
            description: None,
            synonyms: None,
            tier: None,
            span: Span::dummy(),
        };

        // Before the fix: `Err(LowerError::PayloadAliasMismatch)` — the
        // depth-2 `ports` field false-mismatched even though both sides
        // name the same real type. After the fix: the honest restatement
        // lowers cleanly.
        let def = lower_emit(&emit, &aliases, &entities_by_name, &event_aliases, &[]).expect(
            "an honest inline restatement of the alias must lower, not PayloadAliasMismatch",
        );
        let payload = def.payload_schema.expect("payload present");
        assert!(payload.iter().any(|f| f.name == "containers"));

        // Direct unit coverage of the actual fix: the mismatch comparator
        // itself must treat the alias's fully-resolved depth-2 shape and an
        // inline restatement that leaves the same depth-2 field as a bare
        // name (`lower_emit`'s inline-payload loop has no array-of-named-
        // struct branch, so it falls back to a type string exactly like
        // this) as EQUIVALENT, not a mismatch — the recursive comparison
        // must resolve `[PortItem]` against real `aliases`, not an empty map.
        let alias_fields = type_expr_to_payload_fields(
            "ListDone",
            &event_aliases.get("ListDone").unwrap().payload,
            &aliases,
            &entities_by_name,
            &[],
        )
        .expect("alias payload resolves");
        let inline_fields_bare_at_depth2 = vec![PayloadField {
            values: None,
            name: "containers".to_string(),
            field_type: "[object]".to_string(),
            required: Some(true),
            description: None,
            properties: Some(vec![
                PayloadField {
                    values: None,
                    name: "id".to_string(),
                    field_type: "string".to_string(),
                    required: Some(true),
                    description: None,
                    properties: None,
                    type_when: None,
                    entity: None,
                    projected_from: None,
                },
                PayloadField {
                    values: None,
                    name: "ports".to_string(),
                    field_type: "[PortItem]".to_string(),
                    required: Some(true),
                    description: None,
                    properties: None,
                    type_when: None,
                    entity: None,
                    projected_from: None,
                },
            ]),
            type_when: None,
            entity: None,
            projected_from: None,
        }];
        assert_eq!(
            payload_fields_mismatch_norm(
                &inline_fields_bare_at_depth2,
                &alias_fields,
                &entities_by_name,
                &aliases,
            ),
            None,
            "a depth-2 bare struct-alias name must still resolve as equivalent to the alias's fully-expanded shape"
        );
    }
    /// §81(a): a DIRECT (non-collection) self-referential struct alias
    /// reached through an entity field, itself reached through an event
    /// payload — `std-service-llm`'s minimal repro (`entity ServiceLlm {
    /// schema : ServiceLlmSchema }`, `ServiceLlmSchema { additionalProperties
    /// : ServiceLlmSchema, items : ServiceLlmSchema }`, `Loaded { data :
    /// [ServiceLlm] }`).
    ///
    /// `entities_by_name` here ALSO carries "S" (mirroring
    /// `collect_entity_type_bodies`'s real dual population: a plain `type X
    /// = { … }` struct alias contributes to `entities_by_name` too, not just
    /// `entity X { … }`) — without that second registration the bug does
    /// not reproduce, which is exactly why the fix has to live in
    /// `flatten_name`'s DISPATCH ORDER rather than in the entity branch's
    /// shallow mapping itself.
    fn direct_self_ref_aliases_and_entities() -> (
        BTreeMap<String, TypeAliasResolved>,
        BTreeMap<String, Vec<(String, TypeExpr, bool)>>,
    ) {
        let s_fields = vec![
            (
                "id".to_string(),
                TypeExpr::Primitive("string".to_string()),
                true,
            ),
            (
                "additionalProperties".to_string(),
                TypeExpr::Named("S".to_string()),
                false,
            ),
            ("items".to_string(), TypeExpr::Named("S".to_string()), false),
        ];
        let mut aliases: BTreeMap<String, TypeAliasResolved> = BTreeMap::new();
        aliases.insert(
            "S".to_string(),
            TypeAliasResolved::Type(TypeExpr::Object(s_fields.clone())),
        );

        let mut entities_by_name: BTreeMap<String, Vec<(String, TypeExpr, bool)>> = BTreeMap::new();
        // "S" dual-registered exactly like `collect_entity_type_bodies` does
        // for every `type X = { … }` struct alias.
        entities_by_name.insert("S".to_string(), s_fields);
        entities_by_name.insert(
            "E".to_string(),
            vec![
                (
                    "id".to_string(),
                    TypeExpr::Primitive("string".to_string()),
                    true,
                ),
                ("schema".to_string(), TypeExpr::Named("S".to_string()), true),
            ],
        );
        (aliases, entities_by_name)
    }

    fn loaded_event_aliases() -> BTreeMap<String, EventTypeResolved> {
        let mut event_aliases: BTreeMap<String, EventTypeResolved> = BTreeMap::new();
        event_aliases.insert(
            "Loaded".to_string(),
            EventTypeResolved {
                payload: TypeExpr::Object(vec![(
                    "data".to_string(),
                    TypeExpr::Array(Box::new(TypeExpr::Named("E".to_string()))),
                    true,
                )]),
                scope: None,
                description: None,
                type_param: None,
            },
        );
        event_aliases
    }

    #[test]
    fn direct_self_referential_struct_via_entity_and_event_matches_alias() {
        let (aliases, entities_by_name) = direct_self_ref_aliases_and_entities();
        let event_aliases = loaded_event_aliases();
        let emit = Emit {
            event: "Loaded".to_string(),
            scope: Some("internal".to_string()),
            payload: vec![EventField {
                name: "data".to_string(),
                type_expr: TypeExpr::Array(Box::new(TypeExpr::Named("E".to_string()))),
                required: true,
                span: Span::dummy(),
            }],
            description: None,
            synonyms: None,
            tier: None,
            span: Span::dummy(),
        };

        let def = lower_emit(&emit, &aliases, &entities_by_name, &event_aliases, &[])
            .expect("an honest restatement of a direct self-referential struct member must lower, not PayloadAliasMismatch");
        let data = def
            .payload_schema
            .expect("payload present")
            .into_iter()
            .find(|f| f.name == "data")
            .expect("data field");
        let schema = data
            .properties
            .expect("data carries E's fields")
            .into_iter()
            .find(|f| f.name == "schema")
            .expect("schema field");
        let additional_properties = schema
            .properties
            .expect("schema carries S's fields")
            .into_iter()
            .find(|f| f.name == "additionalProperties")
            .expect("additionalProperties field");
        assert_eq!(additional_properties.field_type, "S");
        assert!(
            matches!(&additional_properties.properties, Some(p) if p.is_empty()),
            "the recursion point keeps the EMPTY-properties back-reference marker"
        );
    }

    /// The fix must not blind the comparator to a REAL mismatch: a
    /// restatement that changes a nested field's type is still caught.
    #[test]
    fn direct_self_referential_struct_real_mismatch_still_caught() {
        let (aliases, entities_by_name) = direct_self_ref_aliases_and_entities();
        let event_aliases = loaded_event_aliases();
        // Restate "schema" as a bare `string` — a genuine divergence from
        // the entity's declared `S`-typed field, not a false positive.
        let mut wrong_e_fields = entities_by_name.get("E").unwrap().clone();
        wrong_e_fields[1].1 = TypeExpr::Primitive("string".to_string());
        let emit = Emit {
            event: "Loaded".to_string(),
            scope: Some("internal".to_string()),
            payload: vec![EventField {
                name: "data".to_string(),
                type_expr: TypeExpr::Array(Box::new(TypeExpr::Object(wrong_e_fields))),
                required: true,
                span: Span::dummy(),
            }],
            description: None,
            synonyms: None,
            tier: None,
            span: Span::dummy(),
        };

        let result = lower_emit(&emit, &aliases, &entities_by_name, &event_aliases, &[]);
        assert!(
            matches!(result, Err(LowerError::PayloadAliasMismatch { .. })),
            "a genuinely different nested field type must still be rejected, got {result:?}"
        );
    }

    /// §81(b): a trait type parameter used as a struct MEMBER, where the
    /// struct is reached through an event payload — `std-service-queue`'s
    /// minimal repro (`type QueueJob = { payload : tpayload! }`, `type
    /// ServiceQueueDequeueDone = Event { job : QueueJob }`, on a trait
    /// declaring `(tpayload : Entity)`). Both derivation paths must resolve
    /// "payload" to the SAME `$tpayload` sentinel.
    #[test]
    fn trait_type_param_nested_in_struct_alias_member_matches_alias() {
        let queue_job_fields = vec![
            (
                "id".to_string(),
                TypeExpr::Primitive("string".to_string()),
                true,
            ),
            (
                "payload".to_string(),
                TypeExpr::Named("tpayload".to_string()),
                true,
            ),
        ];
        let mut aliases: BTreeMap<String, TypeAliasResolved> = BTreeMap::new();
        aliases.insert(
            "QueueJob".to_string(),
            TypeAliasResolved::Type(TypeExpr::Object(queue_job_fields)),
        );
        let entities_by_name: BTreeMap<String, Vec<(String, TypeExpr, bool)>> = BTreeMap::new();

        let mut event_aliases: BTreeMap<String, EventTypeResolved> = BTreeMap::new();
        event_aliases.insert(
            "ServiceQueueDequeueDone".to_string(),
            EventTypeResolved {
                payload: TypeExpr::Object(vec![(
                    "job".to_string(),
                    TypeExpr::Named("QueueJob".to_string()),
                    true,
                )]),
                scope: None,
                description: None,
                type_param: None,
            },
        );

        let emit = Emit {
            event: "ServiceQueueDequeueDone".to_string(),
            scope: Some("internal".to_string()),
            payload: vec![EventField {
                name: "job".to_string(),
                type_expr: TypeExpr::Named("QueueJob".to_string()),
                required: true,
                span: Span::dummy(),
            }],
            description: None,
            synonyms: None,
            tier: None,
            span: Span::dummy(),
        };

        // The consuming trait declares `(tpayload : Entity, …)` — this is
        // the `trait_type_params` slice `lower_emit` receives from the
        // trait's own header.
        let trait_type_params = ["tpayload"];
        let def = lower_emit(
            &emit,
            &aliases,
            &entities_by_name,
            &event_aliases,
            &trait_type_params,
        )
        .expect("an honest restatement of a trait-type-param-shaped struct member must lower, not PayloadAliasMismatch");
        let job = def
            .payload_schema
            .expect("payload present")
            .into_iter()
            .find(|f| f.name == "job")
            .expect("job field");
        let payload = job
            .properties
            .expect("job carries QueueJob's fields")
            .into_iter()
            .find(|f| f.name == "payload")
            .expect("payload field");
        assert_eq!(
            payload.field_type, "$tpayload",
            "the trait type param sentinel must survive nested inside a struct-alias member"
        );
    }
}

/// B1-V item C: `PayloadField.entity` — the marker that survives an
/// entity-typed payload field's flatten into `object`/`[object]` +
/// `properties`, so a downstream consumer (`plan-user-crud-flow.ts`'s
/// `wholeRow`) can tell "this field IS a Note row" from "this field is an
/// anonymous struct that happens to share Note's shape" without inferring
/// it from name or shape.
#[cfg(test)]
mod entity_payload_marker_tests {
    use super::*;

    fn note_entities() -> BTreeMap<String, Vec<(String, TypeExpr, bool)>> {
        let mut entities = BTreeMap::new();
        entities.insert(
            "Note".to_string(),
            vec![
                (
                    "id".to_string(),
                    TypeExpr::Primitive("string".to_string()),
                    true,
                ),
                (
                    "title".to_string(),
                    TypeExpr::Primitive("string".to_string()),
                    true,
                ),
            ],
        );
        entities
    }

    /// The plan's literal example: `emits { DELETE { id: string!, row: Note } }`.
    #[test]
    fn scalar_entity_field_is_stamped_on_lower_emit() {
        let entities_by_name = note_entities();
        let aliases: BTreeMap<String, TypeAliasResolved> = BTreeMap::new();
        let event_aliases: BTreeMap<String, EventTypeResolved> = BTreeMap::new();
        let emit = Emit {
            event: "DELETE".to_string(),
            scope: None,
            payload: vec![
                EventField {
                    name: "id".to_string(),
                    type_expr: TypeExpr::Primitive("string".to_string()),
                    required: true,
                    span: Span::dummy(),
                },
                EventField {
                    name: "row".to_string(),
                    type_expr: TypeExpr::Named("Note".to_string()),
                    required: false,
                    span: Span::dummy(),
                },
            ],
            description: None,
            synonyms: None,
            tier: None,
            span: Span::dummy(),
        };
        let def = lower_emit(&emit, &aliases, &entities_by_name, &event_aliases, &[])
            .expect("DELETE lowers");
        let payload = def.payload_schema.expect("payload present");
        let id = payload.iter().find(|f| f.name == "id").expect("id field");
        assert_eq!(
            id.entity, None,
            "a primitive field never carries an entity marker"
        );
        let row = payload.iter().find(|f| f.name == "row").expect("row field");
        assert_eq!(row.field_type, "object");
        assert_eq!(
            row.entity.as_deref(),
            Some("Note"),
            "row IS a Note instance"
        );
        assert!(
            row.properties.is_some(),
            "row still carries Note's flattened fields"
        );
    }

    /// The array form: `emits { BULK_DELETE { rows: [Note] } }` — the
    /// element entity is stamped, `field_type` is `[object]` not `object`.
    #[test]
    fn array_of_entity_field_is_stamped_on_lower_emit() {
        let entities_by_name = note_entities();
        let aliases: BTreeMap<String, TypeAliasResolved> = BTreeMap::new();
        let event_aliases: BTreeMap<String, EventTypeResolved> = BTreeMap::new();
        let emit = Emit {
            event: "BULK_DELETE".to_string(),
            scope: None,
            payload: vec![EventField {
                name: "rows".to_string(),
                type_expr: TypeExpr::Array(Box::new(TypeExpr::Named("Note".to_string()))),
                required: true,
                span: Span::dummy(),
            }],
            description: None,
            synonyms: None,
            tier: None,
            span: Span::dummy(),
        };
        let def = lower_emit(&emit, &aliases, &entities_by_name, &event_aliases, &[])
            .expect("BULK_DELETE lowers");
        let payload = def.payload_schema.expect("payload present");
        let rows = payload
            .iter()
            .find(|f| f.name == "rows")
            .expect("rows field");
        assert_eq!(rows.field_type, "[object]");
        assert_eq!(
            rows.entity.as_deref(),
            Some("Note"),
            "the marker names the ELEMENT entity, not an array type of its own"
        );
        assert!(rows.properties.is_some());
    }

    /// The alias-restatement path (`type EVENT = Event { row : Note }`,
    /// `type_expr_to_payload_fields`'s `Object` branch / `named_entity_fields`)
    /// must stamp the same marker as the inline `emits` path — one authored
    /// shape, one lowered shape, entity identity included.
    #[test]
    fn scalar_entity_field_is_stamped_via_alias_object_branch() {
        let entities_by_name = note_entities();
        let aliases: BTreeMap<String, TypeAliasResolved> = BTreeMap::new();
        let payload = TypeExpr::Object(vec![(
            "row".to_string(),
            TypeExpr::Named("Note".to_string()),
            true,
        )]);
        let fields =
            type_expr_to_payload_fields("NoteDelete", &payload, &aliases, &entities_by_name, &[])
                .expect("payload resolves");
        let row = fields.iter().find(|f| f.name == "row").expect("row field");
        assert_eq!(row.field_type, "object");
        assert_eq!(row.entity.as_deref(), Some("Note"));
    }
}

#[cfg(test)]
mod struct_payload_enum_alias_tests {
    use super::*;

    fn aliases() -> BTreeMap<String, TypeAliasResolved> {
        let mut aliases = BTreeMap::new();
        aliases.insert(
            "ToolKind".to_string(),
            TypeAliasResolved::Enum(vec!["input".to_string(), "read".to_string()]),
        );
        aliases.insert(
            "QuotedKind".to_string(),
            TypeAliasResolved::Type(TypeExpr::Union(vec![
                TypeExpr::StringLiteral("input".to_string()),
                TypeExpr::StringLiteral("read".to_string()),
            ])),
        );
        aliases.insert(
            "Shape".to_string(),
            TypeAliasResolved::Type(TypeExpr::Union(vec![
                TypeExpr::Named("Point".to_string()),
                TypeExpr::Named("Other".to_string()),
            ])),
        );
        aliases.insert(
            "Other".to_string(),
            TypeAliasResolved::Type(TypeExpr::Object(vec![(
                "y".to_string(),
                TypeExpr::Primitive("number".to_string()),
                true,
            )])),
        );
        aliases.insert(
            "Point".to_string(),
            TypeAliasResolved::Type(TypeExpr::Object(vec![(
                "x".to_string(),
                TypeExpr::Primitive("number".to_string()),
                true,
            )])),
        );
        aliases.insert(
            "Activity".to_string(),
            TypeAliasResolved::Type(TypeExpr::Object(vec![
                (
                    "kind".to_string(),
                    TypeExpr::Named("ToolKind".to_string()),
                    false,
                ),
                (
                    "kinds".to_string(),
                    TypeExpr::Array(Box::new(TypeExpr::Named("ToolKind".to_string()))),
                    false,
                ),
                ("at".to_string(), TypeExpr::Named("Point".to_string()), true),
                (
                    "label".to_string(),
                    TypeExpr::Primitive("string".to_string()),
                    true,
                ),
                (
                    "quoted".to_string(),
                    TypeExpr::Named("QuotedKind".to_string()),
                    false,
                ),
                (
                    "inline".to_string(),
                    TypeExpr::Union(vec![
                        TypeExpr::StringLiteral("a".to_string()),
                        TypeExpr::StringLiteral("b".to_string()),
                    ]),
                    false,
                ),
                (
                    "shape".to_string(),
                    TypeExpr::Named("Shape".to_string()),
                    false,
                ),
            ])),
        );
        aliases
    }

    fn field(props: &[PayloadField], name: &str) -> PayloadField {
        props.iter().find(|f| f.name == name).cloned().expect(name)
    }

    #[test]
    fn an_enum_alias_struct_field_travels_as_string_not_a_dangling_name() {
        let aliases = aliases();
        let props =
            struct_payload_properties(&TypeExpr::Named("Activity".to_string()), &aliases, &[])
                .expect("Activity is a struct alias");
        assert_eq!(field(&props, "kind").field_type, "string");
        assert_eq!(field(&props, "kind").properties, None);
        assert_eq!(field(&props, "kinds").field_type, "[string]");
        assert_eq!(field(&props, "quoted").field_type, "string");
        assert_eq!(field(&props, "inline").field_type, "string");
    }

    #[test]
    fn a_closed_payload_field_carries_its_values() {
        let aliases = aliases();
        let props =
            struct_payload_properties(&TypeExpr::Named("Activity".to_string()), &aliases, &[])
                .expect("Activity is a struct alias");
        let ab = Some(vec!["a".to_string(), "b".to_string()]);
        let input_read = Some(vec!["input".to_string(), "read".to_string()]);
        assert_eq!(field(&props, "inline").values, ab);
        assert_eq!(field(&props, "kind").values, input_read);
        assert_eq!(field(&props, "kinds").values, input_read);
        assert_eq!(field(&props, "quoted").values, input_read);
    }

    #[test]
    fn a_bare_word_union_is_a_closed_set_even_where_a_word_spells_a_primitive() {
        let aliases = aliases();
        let ty = TypeExpr::Union(vec![
            TypeExpr::Primitive("date".to_string()),
            TypeExpr::Named("currency".to_string()),
            TypeExpr::Primitive("number".to_string()),
        ]);
        assert_eq!(
            payload_literal_values(&ty, &aliases),
            Some(vec![
                "date".to_string(),
                "currency".to_string(),
                "number".to_string()
            ])
        );
    }

    #[test]
    fn control_an_all_primitive_union_is_a_type_union_not_a_set() {
        let aliases = aliases();
        let ty = TypeExpr::Union(vec![
            TypeExpr::Primitive("string".to_string()),
            TypeExpr::Primitive("number".to_string()),
            TypeExpr::Primitive("boolean".to_string()),
        ]);
        assert_eq!(payload_literal_values(&ty, &aliases), None);
        assert_eq!(
            payload_literal_values(
                &TypeExpr::Union(vec![
                    TypeExpr::Named("Point".to_string()),
                    TypeExpr::Named("Other".to_string())
                ]),
                &aliases
            ),
            None
        );
    }

    #[test]
    fn control_an_open_payload_field_carries_no_values() {
        let aliases = aliases();
        let props =
            struct_payload_properties(&TypeExpr::Named("Activity".to_string()), &aliases, &[])
                .expect("Activity is a struct alias");
        assert_eq!(field(&props, "label").values, None);
        assert_eq!(field(&props, "shape").values, None);
        assert_eq!(field(&props, "at").values, None);
    }

    #[test]
    fn control_a_union_of_struct_aliases_stays_a_union() {
        let aliases = aliases();
        let props =
            struct_payload_properties(&TypeExpr::Named("Activity".to_string()), &aliases, &[])
                .expect("Activity is a struct alias");
        assert_eq!(field(&props, "shape").field_type, "union");
    }

    #[test]
    fn control_struct_alias_and_primitive_fields_are_unchanged() {
        let aliases = aliases();
        let props =
            struct_payload_properties(&TypeExpr::Named("Activity".to_string()), &aliases, &[])
                .expect("Activity is a struct alias");
        let at = field(&props, "at");
        assert_eq!(at.field_type, "object");
        assert_eq!(at.properties.map(|p| p.len()), Some(1));
        assert_eq!(field(&props, "label").field_type, "string");
    }
}
