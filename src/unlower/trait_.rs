//! Trait definitions and trait references back to the AST.

use super::types::{config_fields, event_fields};
use super::UnlowerError;
use crate::ast::{
    Emit, InboundListen, InboundVerifies, Listen, Span, State, Tick, TickInterval, Trait,
    TraitDispatch, TraitRefData, TraitTypeParam, Transition, TypeAnnotation, TypeExpr,
};
use orbital_core_base::schema::{
    join_event_address, DispatchMode, EmitDefinition, EventScope, ListenDefinition, PayloadField,
    StringOrArray, StringOrNumber, TickDefinition, TraitDefinition, TraitReference,
};
use std::collections::{HashMap, HashSet};

fn scope_name(scope: &EventScope) -> String {
    match scope {
        EventScope::Internal => "internal".to_string(),
        EventScope::External => "external".to_string(),
    }
}

fn empty_trait(name: &str) -> Trait {
    Trait {
        name: name.to_string(),
        type_params: Vec::new(),
        type_annotation: None,
        linked_entity: None,
        linked_entity_span: None,
        entity_rebindable: false,
        entity_binding_description: None,
        entity_binding_synonyms: None,
        dispatch: None,
        retired_words: Vec::new(),
        initial_state: None,
        states: Vec::new(),
        emits: Vec::new(),
        listens: Vec::new(),
        ticks: Vec::new(),
        config: Vec::new(),
        span: Span::dummy(),
        reference: None,
        is_inline_synthetic: false,
    }
}

fn sorted_pairs(map: &orbital_core_base::collections::IndexMap<String, String>) -> Vec<(String, String)> {
    let mut v: Vec<_> = map.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    v.sort();
    v
}

pub(super) fn cross_listen(l: &ListenDefinition) -> Listen {
    let mut mapping: Vec<_> = l
        .payload_mapping
        .iter()
        .flatten()
        .map(|(k, v)| (k.to_string(), v.clone()))
        .collect();
    mapping.sort_by(|a, b| a.0.cmp(&b.0));
    Listen {
        event: join_event_address(&l.event, l.source.as_ref()),
        triggers: l.triggers.clone(),
        guard: l.guard.clone(),
        key: l.key.clone(),
        payload_mapping: mapping,
        payload: Vec::new(),
        description: l.description.clone(),
        synonyms: l.synonyms.clone(),
        tier: l.tier.clone(),
        target_trait: None,
        input: false,
        retired_external: None,
        inbound: None,
        span: Span::dummy(),
    }
}

/// An orbital-scope `type EVENT = Event <payload> "<description>"` the printed
/// emit needs: the alias is the only spelling of an entity-named payload or of
/// a description the event definition does not carry itself.
pub(super) struct EventAlias {
    pub event: String,
    pub payload: TypeExpr,
    pub description: Option<String>,
}

pub(super) fn unlower_dispatch(mode: &DispatchMode) -> TraitDispatch {
    match mode {
        DispatchMode::Client => TraitDispatch::Client,
        DispatchMode::Optimistic => TraitDispatch::Optimistic,
        DispatchMode::Server => TraitDispatch::Server,
    }
}

pub(super) fn trait_reference(
    t: &TraitReference,
) -> Result<(Trait, Vec<EventAlias>), UnlowerError> {
    match t {
        TraitReference::Inline(td) => trait_definition(td),
        TraitReference::Reference {
            reference,
            name,
            linked_entity,
            config,
            events,
            fields,
            listens,
            emits_scope,
            type_args,
            dispatch,
            ..
        } => {
            let reference = reference
                .clone()
                .ok_or_else(|| UnlowerError::new("trait reference without a ref"))?;
            let name = name
                .clone()
                .ok_or_else(|| UnlowerError::new("trait reference declaration without a name"))?;
            let mut tr = empty_trait(&name);
            tr.linked_entity = linked_entity.clone();
            tr.dispatch = dispatch.as_ref().map(unlower_dispatch);
            tr.listens = listens.iter().flatten().map(cross_listen).collect();
            if let Some(args) = type_args {
                let mut flat: Vec<String> = Vec::new();
                for (k, v) in sorted_pairs(args) {
                    flat.push(k);
                    flat.push(v);
                }
                let mut it = flat.into_iter();
                if let Some(type_name) = it.next() {
                    tr.type_annotation = Some(TypeAnnotation {
                        type_name,
                        type_args: it.collect(),
                        span: Span::dummy(),
                    });
                }
            }
            tr.reference = Some(TraitRefData {
                reference,
                name_override: None,
                events: events.as_ref().map(sorted_pairs).unwrap_or_default(),
                fields: fields.as_ref().map(sorted_pairs).unwrap_or_default(),
                config: config_fields(config)?,
                emits_scope: emits_scope.as_ref().map(scope_name),
            });
            Ok((tr, Vec::new()))
        }
    }
}

fn emit_of(
    e: &EmitDefinition,
    event_description: Option<&Option<String>>,
) -> Result<(Emit, Option<EventAlias>), UnlowerError> {
    let alias_description = match (&e.description, event_description) {
        (Some(d), Some(own)) if own.as_ref() != Some(d) => Some(d.clone()),
        _ => None,
    };
    let empty_payload = e.payload_schema.as_ref().is_some_and(Vec::is_empty);
    let alias = if e.payload_entity.is_some() || alias_description.is_some() || empty_payload {
        let payload = match (&e.payload_entity, &e.payload_schema) {
            (Some(entity), _) => TypeExpr::Named(entity.clone()),
            (None, Some(fields)) => TypeExpr::Object(
                event_fields(fields)?
                    .into_iter()
                    .map(|f| (f.name, f.type_expr, f.required))
                    .collect(),
            ),
            (None, None) => {
                return Err(UnlowerError::new(format!(
                    "payloadless emit `{}` whose description comes from an event alias",
                    e.event
                )))
            }
        };
        Some(EventAlias {
            event: e.event.clone(),
            payload,
            description: alias_description.clone(),
        })
    } else {
        None
    };
    let payload = match (&alias, &e.payload_schema) {
        (Some(_), _) | (None, None) => Vec::new(),
        (None, Some(fields)) => event_fields(fields)?,
    };
    Ok((
        Emit {
            event: e.event.clone(),
            scope: e.scope.as_ref().map(scope_name),
            payload,
            description: if alias_description.is_some() {
                None
            } else {
                e.description.clone()
            },
            synonyms: e.synonyms.clone(),
            tier: e.tier.clone(),
            span: Span::dummy(),
        },
        alias,
    ))
}

fn tick_of(t: &TickDefinition) -> Result<Tick, UnlowerError> {
    let interval = match &t.interval {
        StringOrNumber::Number(n) => TickInterval::Millis(*n as u64),
        StringOrNumber::String(s) => TickInterval::Cron(s.clone()),
    };
    Ok(Tick {
        name: t.name.clone(),
        interval,
        guard: t.guard.clone(),
        effects: t.effects.clone(),
        applies_to: t.applies_to.clone().unwrap_or_default(),
        background: t.background,
        visible: t.while_visible,
        span: Span::dummy(),
        guard_spans: Vec::new(),
        effect_spans: Vec::new(),
    })
}

fn single_from(
    tr: &orbital_core_base::schema::TransitionDefinition,
) -> Result<String, UnlowerError> {
    match &tr.from {
        StringOrArray::Single(s) => Ok(s.clone()),
        StringOrArray::Multiple(_) => Err(UnlowerError::new(
            "transition with a multi-state `from` (not produced by .lolo lowering)",
        )),
    }
}

/// Declaration order of states such that the lowerer — which sorts the initial
/// state first and emits transitions in declaration order — reproduces `td`.
fn declaration_order(td: &TraitDefinition) -> Result<(Vec<String>, Option<String>), UnlowerError> {
    let sm = &td.state_machine;
    let initial = sm
        .states
        .iter()
        .find(|s| s.is_initial == Some(true))
        .map(|s| s.name.clone());
    let mut group_index: HashMap<String, usize> = HashMap::new();
    for tr in &sm.transitions {
        let from = single_from(tr)?;
        let next = group_index.len();
        group_index.entry(from).or_insert(next);
    }
    let rest: Vec<&String> = sm
        .states
        .iter()
        .filter(|s| Some(&s.name) != initial.as_ref())
        .map(|s| &s.name)
        .collect();
    let mut order: Vec<String> = Vec::with_capacity(sm.states.len());
    let mut placed = initial.is_none();
    for name in rest {
        if !placed {
            if let (Some(init), Some(&gi)) = (initial.as_ref(), group_index.get(name)) {
                if group_index.get(init).is_some_and(|&ii| ii < gi) {
                    order.push(init.clone());
                    placed = true;
                }
            }
        }
        order.push(name.clone());
    }
    if let (false, Some(init)) = (placed, initial.as_ref()) {
        order.push(init.clone());
    }
    let initial_state = match (&initial, order.first()) {
        (Some(i), Some(first)) if i != first => Some(i.clone()),
        _ => None,
    };
    Ok((order, initial_state))
}

/// The IR indices of `state`'s transitions, in the order the trait prints them.
fn state_transitions(td: &TraitDefinition, state: &str) -> Result<Vec<usize>, UnlowerError> {
    let mut out = Vec::new();
    for (i, t) in td.state_machine.transitions.iter().enumerate() {
        if single_from(t)? == state {
            out.push(i);
        }
    }
    Ok(out)
}

/// Every IR transition index, in the order the unlowered trait prints them.
pub(super) fn transition_print_order(td: &TraitDefinition) -> Result<Vec<usize>, UnlowerError> {
    let (order, _) = declaration_order(td)?;
    let mut out = Vec::new();
    for name in &order {
        out.extend(state_transitions(td, name)?);
    }
    Ok(out)
}

pub(super) fn trait_definition(
    td: &TraitDefinition,
) -> Result<(Trait, Vec<EventAlias>), UnlowerError> {
    let mut tr = empty_trait(&td.name);
    tr.linked_entity = td.linked_entity.clone();
    tr.entity_rebindable = td.entity_rebindable == Some(true);
    tr.dispatch = td.dispatch.as_ref().map(unlower_dispatch);
    tr.type_params = td
        .type_params
        .iter()
        .flatten()
        .map(|p| TraitTypeParam {
            name: p.name.clone(),
            kind: p.kind.clone(),
            default: p.default.clone(),
        })
        .collect();

    let sm = &td.state_machine;
    let (order, initial_state) = declaration_order(td)?;
    tr.initial_state = initial_state;
    for name in &order {
        let def = sm
            .states
            .iter()
            .find(|s| &s.name == name)
            .ok_or_else(|| UnlowerError::new(format!("state `{name}` vanished")))?;
        let mut transitions = Vec::new();
        for i in state_transitions(td, name)? {
            let t = &sm.transitions[i];
            transitions.push(Transition {
                event: t.event.clone(),
                target: t.to.clone(),
                guard: t.guard.clone(),
                ensures: t.ensures.clone().unwrap_or_default(),
                effects: t.effects.clone().unwrap_or_default(),
                span: Span::dummy(),
                guard_spans: Vec::new(),
                effect_spans: Vec::new(),
                term_match: None,
            });
        }
        tr.states.push(State {
            name: name.clone(),
            time_bound: def.time_bound_ms,
            transitions,
            holds: def.holds.clone(),
            span: Span::dummy(),
        });
    }

    let emits: &[EmitDefinition] = td.emits.as_deref().map(Vec::as_slice).unwrap_or(&[]);
    let mut aliases = Vec::new();
    for e in emits {
        let own = sm
            .events
            .iter()
            .flatten()
            .find(|ev| ev.key == e.event)
            .map(|ev| &ev.description);
        let (emit, alias) = emit_of(e, own)?;
        tr.emits.push(emit);
        aliases.extend(alias);
    }
    for l in td.listens.iter().flatten() {
        tr.listens.push(cross_listen(l));
    }

    let transition_events: HashSet<&str> =
        sm.transitions.iter().map(|t| t.event.as_str()).collect();
    for ev in sm.events.iter().flatten() {
        let emit = emits.iter().find(|e| e.event == ev.key);
        let in_transitions = transition_events.contains(ev.key.as_str());
        let emit_payload: Option<&Vec<PayloadField>> = emit.and_then(|e| e.payload_schema.as_ref());
        let payload_differs = match (&ev.payload_schema, emit_payload) {
            (Some(own), Some(from_emit)) => own != from_emit,
            (Some(_), None) => true,
            (None, _) => false,
        };
        let annotations_differ = match emit {
            Some(e) => {
                (ev.description.is_some() && ev.description != e.description)
                    || (ev.synonyms.is_some() && ev.synonyms != e.synonyms)
                    || (ev.tier.is_some() && ev.tier != e.tier)
            }
            None => ev.description.is_some() || ev.synonyms.is_some() || ev.tier.is_some(),
        };
        let alias_only = emit.is_none()
            && in_transitions
            && ev.listened != Some(true)
            && ev.input != Some(true)
            && ev.inbound.is_none()
            && ev.synonyms.is_none()
            && ev.tier.is_none();
        if let (true, Some(fields)) = (alias_only, &ev.payload_schema) {
            let payload = match &ev.payload_entity {
                Some(entity) => TypeExpr::Named(entity.clone()),
                None => TypeExpr::Object(
                    event_fields(fields)?
                        .into_iter()
                        .map(|f| (f.name, f.type_expr, f.required))
                        .collect(),
                ),
            };
            aliases.push(EventAlias {
                event: ev.key.clone(),
                payload,
                description: ev.description.clone(),
            });
            continue;
        }
        if emit.is_none() && ev.payload_schema.as_ref().is_some_and(Vec::is_empty) {
            aliases.push(EventAlias {
                event: ev.key.clone(),
                payload: TypeExpr::Object(Vec::new()),
                description: None,
            });
        }
        let sole_source = emit.is_none() && !in_transitions;
        let aliased = emit.is_some_and(|e| e.payload_entity.is_some())
            && ev.payload_entity == emit.and_then(|e| e.payload_entity.clone());
        if ev.input == Some(true)
            || ev.listened == Some(true)
            || ev.inbound.is_some()
            || sole_source
            || (payload_differs && !aliased)
            || annotations_differ
        {
            let payload = match (&ev.payload_schema, aliased) {
                (Some(fields), false) => event_fields(fields)?,
                _ => Vec::new(),
            };
            tr.listens.push(Listen {
                event: ev.key.clone(),
                triggers: None,
                guard: None,
                key: None,
                payload_mapping: Vec::new(),
                payload,
                description: ev.description.clone(),
                synonyms: ev.synonyms.clone(),
                tier: ev.tier.clone(),
                target_trait: None,
                input: ev.input == Some(true),
                retired_external: None,
                inbound: ev.inbound.as_ref().map(|i| InboundListen {
                    name: i.name.clone(),
                    answers: i.answers.clone(),
                    verifies: i.verifies.as_ref().map(|v| InboundVerifies {
                        header: v.header.clone(),
                        credential: v.credential.clone(),
                    }),
                }),
                span: Span::dummy(),
            });
        }
    }

    for t in td.ticks.iter().flatten() {
        tr.ticks.push(tick_of(t)?);
    }
    tr.config = config_fields(&td.config)?;
    Ok((tr, aliases))
}
