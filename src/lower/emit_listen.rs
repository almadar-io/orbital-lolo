use super::*;

/// Capture which config knob an `emits { @config.<knob> -> … }` declaration
/// named, while the reference is still spelled out.
///
/// L1 is the only place this is free: the inline phase later resolves the
/// reference to a literal event name and overwrites `event`, so by L2 the
/// provenance is gone. Downstream, a `Some(knob)` marks the emit as an atom's
/// OFFER rather than an authored obligation.
fn definer_knob_of(event: &str) -> Option<String> {
    orbital_core::schema::config_ref_event_knob(event).map(str::to_string)
}

pub(crate) fn lower_emit(
    e: &Emit,
    aliases: &BTreeMap<String, TypeAliasResolved>,
    entities_by_name: &BTreeMap<String, Vec<(String, TypeExpr, bool)>>,
    event_aliases: &BTreeMap<String, EventTypeResolved>,
    // Increment 3-II: this trait's OWN declared type-parameter names (from
    // its `(V : Type, ...)` header), usable directly in an inline `emits {
    // EVENT { field : V } }` payload field position. Distinct scope from an
    // event ALIAS's own `type_param` (Increment 3-I, resolved just below) —
    // the two mechanisms are orthogonal and never merged into one slice.
    trait_type_params: &[&str],
) -> Result<EmitDefinition, LowerError> {
    // Bare-name path: when the entry has no inline payload, look up
    // `type EVENT_NAME = Event { ... }` in the orbital-scope alias registry.
    // If found, the alias supplies payload (and scope, if the entry didn't
    // override it). If not found, the emit is payloadless. Inline payload
    // (next branch) always wins over alias lookup.
    if e.payload.is_empty() {
        if let Some(resolved) = event_aliases.get(&e.event) {
            let alias_param: Vec<&str> = resolved.type_param.as_deref().into_iter().collect();
            let payload = Some(type_expr_to_payload_fields(
                &e.event,
                &resolved.payload,
                aliases,
                entities_by_name,
                &alias_param,
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
        });
    }
    // Inline payload path: each field whose type_expr is `TypeExpr::Named`
    // is resolved against the type registry and flattened into prefixed
    // `name.subname` entries; primitive fields stay flat.
    let mut fields: Vec<PayloadField> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for f in &e.payload {
        // Increment 3-II: a field referencing this trait's OWN declared type
        // parameter (`candidate : V` / `candidate : [V]`) lowers to the
        // `$V` / `[$V]` sentinel — checked first since a param name never
        // resolves as a struct alias or entity.
        if let Some(sentinel) = type_param_sentinel(&f.type_expr, trait_type_params) {
            if !seen.insert(f.name.clone()) {
                return Err(LowerError::PayloadFieldCollision {
                    event: e.event.clone(),
                    field: f.name.clone(),
                });
            }
            fields.push(PayloadField {
                name: f.name.clone(),
                field_type: sentinel,
                required: if f.required { Some(true) } else { None },
                description: None,
                properties: None,
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
        if let Some(props) = struct_payload_properties(&f.type_expr, aliases, trait_type_params) {
            if !seen.insert(f.name.clone()) {
                return Err(LowerError::PayloadFieldCollision {
                    event: e.event.clone(),
                    field: f.name.clone(),
                });
            }
            fields.push(PayloadField {
                name: f.name.clone(),
                field_type: "object".to_string(),
                required: if f.required { Some(true) } else { None },
                description: None,
                properties: Some(props),
            });
        } else if let TypeExpr::Named(type_name) = &f.type_expr {
            let nested = resolve_payload_type(type_name, aliases, entities_by_name)?;
            if !seen.insert(f.name.clone()) {
                return Err(LowerError::PayloadFieldCollision {
                    event: e.event.clone(),
                    field: f.name.clone(),
                });
            }
            fields.push(PayloadField {
                name: f.name.clone(),
                field_type: "object".to_string(),
                required: if f.required { Some(true) } else { None },
                description: None,
                properties: Some(nested),
            });
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
                properties: None,
            });
        }
    }
    // Alias-consistency check (syntax revision 2026-08-18): when the same
    // event ALSO carries an orbital-scope `type X = Event { … }` alias, the
    // inline restatement must MATCH it — restating is encouraged for
    // readability, silent divergence is the bug the old "inline wins"
    // precedence allowed.
    if let Some(resolved) = event_aliases.get(&e.event) {
        let alias_param: Vec<&str> = resolved.type_param.as_deref().into_iter().collect();
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
    })
}

/// Order-insensitive recursive comparison of two lowered payload shapes.
/// Returns `Some(detail)` naming the first divergence, `None` when they
/// agree. Sentinels (`$v`, `@entity`) compare as their literal strings —
/// alias and inline lower them identically, so honest restatements match.
pub(crate) fn payload_fields_mismatch(
    inline: &[PayloadField],
    alias: &[PayloadField],
) -> Option<String> {
    payload_fields_mismatch_norm(inline, alias, &BTreeMap::new(), &BTreeMap::new())
}

/// Like [`payload_fields_mismatch`] but normalizes NAMED field types first:
/// the two lowering paths resolve names against different scopes, so the
/// same authored shape (`data : [ObjectSpec]`, `row : TradeOrder`) can
/// arrive as a bare/bracketed NAME on one side and a flattened
/// `object`/`[object]` + properties on the other. Both entity names
/// (`entities_by_name`) and struct aliases (`aliases`) flatten before
/// comparing.
pub(crate) fn payload_fields_mismatch_norm(
    inline: &[PayloadField],
    alias: &[PayloadField],
    entities_by_name: &BTreeMap<String, Vec<(String, TypeExpr, bool)>>,
    aliases: &BTreeMap<String, TypeAliasResolved>,
) -> Option<String> {
    let flatten_name = |name: &str| -> Option<Vec<PayloadField>> {
        if let Some(fields) = entities_by_name.get(name) {
            return Some(
                fields
                    .iter()
                    .map(|(n, t, req)| PayloadField {
                        name: n.clone(),
                        field_type: type_expr_to_string(t),
                        required: if *req { Some(true) } else { None },
                        description: None,
                        properties: None,
                    })
                    .collect(),
            );
        }
        struct_payload_properties(&TypeExpr::Named(name.to_string()), aliases, &[])
    };
    let normalize = |f: &PayloadField| -> PayloadField {
        if f.properties.is_none() {
            let (base, is_array) = match f.field_type.strip_prefix('[') {
                Some(rest) => (rest.trim_end_matches(']'), true),
                None => (f.field_type.as_str(), false),
            };
            if let Some(props) = flatten_name(base) {
                return PayloadField {
                    name: f.name.clone(),
                    field_type: if is_array {
                        "[object]".to_string()
                    } else {
                        "object".to_string()
                    },
                    required: f.required,
                    description: None,
                    properties: Some(props),
                };
            }
        }
        f.clone()
    };
    let an: Vec<PayloadField> = inline.iter().map(normalize).collect();
    let bn: Vec<PayloadField> = alias.iter().map(normalize).collect();
    let mut a: Vec<&PayloadField> = an.iter().collect();
    let mut b: Vec<&PayloadField> = bn.iter().collect();
    a.sort_by(|x, y| x.name.cmp(&y.name));
    b.sort_by(|x, y| x.name.cmp(&y.name));
    if a.len() != b.len() {
        let names = |v: &[&PayloadField]| {
            v.iter().map(|f| f.name.as_str()).collect::<Vec<_>>().join(", ")
        };
        return Some(format!(
            "inline declares fields [{}], alias declares [{}]",
            names(&a),
            names(&b)
        ));
    }
    for (fa, fb) in a.iter().zip(b.iter()) {
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
            return Some(format!(
                "field '{}': required flags disagree",
                fa.name
            ));
        }
        match (&fa.properties, &fb.properties) {
            (Some(pa), Some(pb)) => {
                if let Some(d) = payload_fields_mismatch(pa, pb) {
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
            name: "value".to_string(),
            field_type: sentinel,
            required: None,
            description: None,
            properties: None,
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
                    TypeExpr::Array(inner) => struct_payload_properties(inner, aliases, type_params),
                    _ => None,
                };
                let (field_type, properties) = if let Some(s) = sentinel {
                    (s, None)
                } else if let Some(elem) = array_elem {
                    ("[object]".to_string(), Some(elem))
                } else if let Some(nested) = struct_payload_properties(ftype, aliases, type_params)
                {
                    ("object".to_string(), Some(nested))
                } else if let Some(variants) = union_payload_variants(ftype, aliases, type_params) {
                    ("union".to_string(), Some(variants))
                } else if let Some(entity_fields) = named_entity_fields(ftype, entities_by_name) {
                    // An ENTITY named in a field position expands to
                    // `object` + its own fields — the same shape `lower_emit`'s
                    // inline path already produced for the identical authored
                    // line. Without this the alias-only path left a raw entity
                    // NAME in `field_type` with no `properties`, so one authored
                    // `{ row : E }` lowered to two different `.orb` shapes
                    // depending on whether an inline restatement happened to
                    // exist, and the raw name reached codegen as a bare
                    // `Reference` (which rendered as `string` in events.ts).
                    // `payload_fields_mismatch_norm` normalized the two for
                    // COMPARISON only; nothing normalized what was emitted.
                    ("object".to_string(), Some(entity_fields))
                } else {
                    (type_expr_to_string(ftype), None)
                };
                flat.push(PayloadField {
                    name: fname.clone(),
                    field_type,
                    required: if *freq { Some(true) } else { None },
                    description: None,
                    properties,
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
                properties: None,
            }])
        }
    }
}

/// If `te` names a known ENTITY, return that entity's fields flattened one
/// level — the payload shape an entity reference in a field position carries.
///
/// Deliberately non-propagating, unlike `lower_emit`'s inline path which
/// `?`-propagates `UnknownPayloadType`: a name that is not an entity (a struct
/// alias already handled by `struct_payload_properties`, a type parameter, a
/// primitive) simply yields `None` so the caller keeps its existing string
/// form. Making this arm fatal would reject files that lower fine today.
pub(crate) fn named_entity_fields(
    te: &TypeExpr,
    entities_by_name: &BTreeMap<String, Vec<(String, TypeExpr, bool)>>,
) -> Option<Vec<PayloadField>> {
    let TypeExpr::Named(name) = te else {
        return None;
    };
    let fields = entities_by_name.get(name)?;
    Some(
        fields
            .iter()
            .map(|(n, t, req)| PayloadField {
                name: n.clone(),
                field_type: type_expr_to_string(t),
                required: if *req { Some(true) } else { None },
                description: None,
                properties: None,
            })
            .collect(),
    )
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
            TypeExpr::Array(inner) => match struct_payload_properties(inner, aliases, type_params) {
                Some(elem) => ("array".to_string(), "[object]".to_string(), Some(elem)),
                None => ("array".to_string(), format!("[{}]", type_expr_to_string(inner)), None),
            },
            TypeExpr::Named(n) => match struct_payload_properties(p, aliases, type_params) {
                Some(nested) => (n.clone(), "object".to_string(), Some(nested)),
                None => (n.clone(), type_expr_to_string(p), None),
            },
            TypeExpr::Primitive(n) => (n.clone(), type_expr_to_string(p), None),
            _ => return None,
        };
        out.push(PayloadField {
            name,
            field_type,
            required: None,
            description: None,
            properties,
        });
    }
    Some(out)
}

pub(crate) fn struct_payload_properties(
    te: &TypeExpr,
    aliases: &BTreeMap<String, TypeAliasResolved>,
    type_params: &[&str],
) -> Option<Vec<PayloadField>> {
    let items: &Vec<(String, TypeExpr, bool)> = match te {
        TypeExpr::Object(items) => items,
        TypeExpr::Named(n) => match aliases.get(n) {
            Some(TypeAliasResolved::Type(TypeExpr::Object(items))) => items,
            _ => return None,
        },
        _ => return None,
    };
    let mut props = Vec::with_capacity(items.len());
    for (name, ty, req) in items {
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
        } else {
            (type_expr_to_string(ty), None)
        };
        props.push(PayloadField {
            name: name.clone(),
            field_type,
            required: if *req { Some(true) } else { None },
            description: None,
            properties,
        });
    }
    Some(props)
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
                properties: None,
            })
            .collect());
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
        event_id: None,
        triggers_id: None,
    }
}

/// Split the parser's concatenated event string into (bare_event, source).
/// Returns `(event, None)` when the event has no source prefix (local shape).
pub(crate) fn split_listen_source(
    raw: &str,
) -> (String, Option<orbital_core::schema::ListenSource>) {
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
                        trait_id: None,
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
                    orbital_id: None,
                    trait_id: None,
                }),
            )
        }
    }
}

pub(crate) fn lower_tick(t: &Tick) -> TickDefinition {
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
