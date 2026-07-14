use super::*;

/// Dispatch a `Trait` AST node to either an inline definition or a reference,
/// returning the `TraitReference` shape that the core schema expects.
pub(crate) fn lower_trait_decl(
    t: &Trait,
    aliases: &BTreeMap<String, TypeAliasResolved>,
    entities: &BTreeMap<String, Vec<(String, TypeExpr, bool)>>,
    entity_names: &HashSet<String>,
    event_aliases: &BTreeMap<String, EventTypeResolved>,
    intrinsic_fields: &HashSet<String>,
    shared_entity_names: &HashSet<String>,
) -> Result<TraitReference, LowerError> {
    if let Some(ref_data) = &t.reference {
        lower_trait_reference(t, ref_data, aliases, entity_names)
    } else {
        Ok(TraitReference::Inline(lower_trait(
            t,
            aliases,
            entities,
            entity_names,
            event_aliases,
            intrinsic_fields,
            shared_entity_names,
        )?))
    }
}

/// Build a `TraitReference::Reference` from a parsed `trait Name = Alias.X { ... }`.
pub(crate) fn lower_trait_reference(
    t: &Trait,
    ref_data: &TraitRefData,
    aliases: &BTreeMap<String, TypeAliasResolved>,
    entity_names: &HashSet<String>,
) -> Result<TraitReference, LowerError> {
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

    // Lower call-site config entries. Each `ConfigField` is either a plain
    // wiring entry (`type_expr = Primitive("unknown")`, only `default` set)
    // or a fully-annotated re-declaration. Both lower to `SchemaConfigField`
    // via the same path as atom config, but plain wiring entries skip the
    // expensive `lower_type_expr` resolution — they carry no structural
    // type information (`field_type = "unknown"`, no items/properties/values).
    let config = if ref_data.config.is_empty() {
        None
    } else {
        let mut map: HashMap<String, SchemaConfigField> = HashMap::new();
        for cf in &ref_data.config {
            // NOTE: unlike the atom-level declared-config guard below (a knob
            // defaulting to its own value IS self-referential), a call-site
            // override `key: @config.key` on a trait Reference is a legitimate
            // wrapper-forward: it forwards the consuming orbital's `key` knob
            // into the imported trait. The consumer's config resolution folds
            // it to the effective value — the resolve/inline Reference arm
            // (`apply_overrides_to_trait`) on the compiled path and
            // `buildResolvedTraitConfigs` on the runtime path. Rejecting it
            // here (the earlier L1 ruling) broke knob-forwarding atoms such as
            // std-browse's `DataGrid1 { config { fields: @config.fields } }`.
            let is_plain_wiring = matches!(&cf.type_expr, crate::ast::TypeExpr::Primitive(s) if s == "unknown")
                && cf.label.is_none()
                && cf.description.is_none()
                && cf.synonyms.is_none()
                && cf.tier.is_none();
            let schema_field = if is_plain_wiring {
                SchemaConfigField {
                    field_type: "unknown".to_string(),
                    default: cf.default.clone(),
                    label: None,
                    description: None,
                    synonyms: None,
                    tier: None,
                    values: None,
                    items: None,
                    properties: None,
                    required: None,
                }
            } else {
                // Declaration-form entries (an alias trait declaring its OWN
                // typed knob) lower with the real alias/entity context — the
                // same path atom config declarations take — so named struct
                // aliases (e.g. `[StepCard]`) resolve to full `items`/
                // `properties` schemas instead of the string fallback.
                let (_, _, items, properties, _) =
                    lower_type_expr(&cf.type_expr, aliases, entity_names);
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
                    required: None,
                }
            };
            map.insert(cf.name.clone(), schema_field);
        }
        Some(map)
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

    Ok(TraitReference::Reference {
        reference: Some(ref_data.reference.clone()),
        name: Some(t.name.clone()),
        linked_entity: t.linked_entity.clone(),
        config,
        events,
        fields,
        listens,
        emits_scope,
        event_ids: None,
        linked_entity_id: None,
        ref_id: None,
    })
}

pub(crate) fn lower_trait(
    t: &Trait,
    aliases: &BTreeMap<String, TypeAliasResolved>,
    entities: &BTreeMap<String, Vec<(String, TypeExpr, bool)>>,
    entity_names: &HashSet<String>,
    event_aliases: &BTreeMap<String, EventTypeResolved>,
    intrinsic_fields: &HashSet<String>,
    shared_entity_names: &HashSet<String>,
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
            time_bound_ms: state.time_bound,
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
        if state.time_bound.is_some() {
            // The duration is carried on this state's `time_bound_ms` (set
            // above). We do NOT add a synthetic TIMEOUT transition here
            // unless the user omitted one. The user typically writes one
            // explicitly.
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
                    properties: None,
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
            event_annotations.insert(
                l.event.clone(),
                (l.description.clone(), l.synonyms.clone(), l.tier.clone()),
            );
        }
    }
    for e in &t.emits {
        if e.description.is_some() || e.synonyms.is_some() || e.tier.is_some() {
            event_annotations.insert(
                e.event.clone(),
                (e.description.clone(), e.synonyms.clone(), e.tier.clone()),
            );
        }
    }

    // Lower the emits once up front so the `stateMachine.events[]` builder can
    // source an event's payload from its outbound `emits { }` declaration when
    // nothing else supplies one. Reused below for the trait's `emits` array —
    // one lowering, one source of truth (event payload === emit payload).
    let lowered_emits: Vec<EmitDefinition> = t
        .emits
        .iter()
        .map(|e| lower_emit(e, aliases, entities, event_aliases))
        .collect::<Result<Vec<_>, _>>()?;
    let mut emit_payloads: HashMap<String, Vec<PayloadField>> = HashMap::new();
    for def in &lowered_emits {
        if let Some(p) = &def.payload_schema {
            if !p.is_empty() {
                emit_payloads
                    .entry(def.event.clone())
                    .or_insert_with(|| p.clone());
            }
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
                    let (description, synonyms, tier) = event_annotations
                        .get(&k)
                        .cloned()
                        .unwrap_or((None, None, None));
                    // Payload resolution precedence for stateMachine.events[]:
                    //   1. Explicit local listen payload (e.g. `listens { X { f: T } }`)
                    //   2. Orbital-scope `type X = Event <T>` declaration
                    // Without (2) the render-ui validator can't resolve
                    // `@payload.*` bindings on events that are only typed at
                    // orbital scope, even though the author declared the shape.
                    //   3. Outbound `emits { K -> external { … } }` payload —
                    //      last-resort source for an emit-only event (e.g. a
                    //      render-ui callback like `onDayClick: DAY_CLICK`) that
                    //      no listen/alias typed. Placed last so events already
                    //      typed via listen/alias are untouched; the validator
                    //      enforces emit↔event payload parity, so this only fills
                    //      holes, never contradicts.
                    let payload = if let Some(fields) = listen_payloads.remove(&k) {
                        Some(fields)
                    } else if let Some(resolved) = event_aliases.get(&k) {
                        Some(type_expr_to_payload_fields(
                            &k,
                            &resolved.payload,
                            aliases,
                            entities,
                        )?)
                    } else if let Some(fields) = emit_payloads.remove(&k) {
                        Some(fields)
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

    // Reuse the emits lowered up front (see the `stateMachine.events[]`
    // payload-precedence block above). `lower_emit` already ran per entry.
    let emits = if lowered_emits.is_empty() {
        None
    } else {
        Some(lowered_emits)
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
            // Same-key self-reference guard (L1-only; see LowerError docs).
            if let Some(SExpression::String(s)) = &cf.default {
                if s.strip_prefix("@config.") == Some(cf.name.as_str()) {
                    return Err(LowerError::ConfigDefaultSelfReferential {
                        trait_name: t.name.clone(),
                        key: cf.name.clone(),
                    });
                }
            }
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
                    required: if cf.required { Some(true) } else { None },
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
        let entity_shared = t
            .linked_entity
            .as_deref()
            .is_some_and(|name| shared_entity_names.contains(name));
        check_entity_fields_have_setters(t, intrinsic_fields, entity_shared)?;
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
        id: None,
        linked_entity_id: None,
    })
}

/// Infer the `@rebindable` binding's field contract from the trait's own
/// `@entity.<field>` usage. Reuses `collect_entity_field_refs` (the single
/// SExpr walker — no second walker) across all transitions/guards/ticks:
/// reads become `requires`, `(set @entity.X ...)` writes become `provides`.
/// `id`/`createdAt`/`updatedAt` are already excluded from reads by the
/// collector (auto-managed fields every entity has). Both lists sorted +
/// deduped for a deterministic `.orb`.
pub(crate) fn infer_entity_contract(
    t: &Trait,
    intrinsic_fields: &HashSet<String>,
) -> EntityFieldContract {
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
