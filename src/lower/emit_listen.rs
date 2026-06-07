use super::*;

pub(crate) fn lower_emit(
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
        // Anonymous struct (inline `{ … }` or a `type X = { … }` alias, NOT an
        // entity): keep it as ONE field carrying the nested shape so codegen
        // emits a strict inline object type instead of widening to
        // `EventPayload`. Entity references fall through to the dotted-flatten
        // below (rows expose bindable `@payload.rec.field` paths).
        if let Some(props) = struct_payload_properties(&f.type_expr, aliases) {
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
                    properties: None,
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
                properties: None,
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
pub(crate) fn type_expr_to_payload_fields(
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
                    properties: None,
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

/// If `te` is an anonymous struct — an inline `{ … }` object or a
/// `type X = { … }` struct alias (NOT an entity reference) — return its fields
/// as nested `PayloadField`s so codegen can emit a strict inline object type.
/// Returns `None` for entities, primitives, arrays, etc. (those keep the
/// existing flat / dotted-flatten handling). Recurses for nested structs.
pub(crate) fn struct_payload_properties(
    te: &TypeExpr,
    aliases: &BTreeMap<String, TypeAliasResolved>,
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
        let array_elem = match ty {
            TypeExpr::Array(inner) => struct_payload_properties(inner, aliases),
            _ => None,
        };
        let (field_type, properties) = if let Some(elem) = array_elem {
            ("[object]".to_string(), Some(elem))
        } else if let Some(nested) = struct_payload_properties(ty, aliases) {
            ("object".to_string(), Some(nested))
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
    }
}

/// Split the parser's concatenated event string into (bare_event, source).
/// Returns `(event, None)` when the event has no source prefix (local shape).
pub(crate) fn split_listen_source(
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
