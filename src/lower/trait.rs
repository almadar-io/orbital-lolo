use super::*;

/// Expand a transition's term-match sugar (Phase 2, pattern-matching doc
/// §1(a)) into ordered guarded `Transition`s, or return it unchanged
/// (cloned) when it carries no `term_match`. Pure syntactic desugar: each
/// arm becomes one `Transition` sharing the outer `event`/`target`, with a
/// synthesized `(== <scrutinee> <lit>)` guard mirroring the corpus's own
/// hand-written guard shape (`["==", "@config.op", "set"]` in the .orb) —
/// `_` lowers to the unguarded transition, always last (the parser/L1
/// checks — `check_arm_pattern_reachable`'s "must be last" rule — guarantee
/// `_`, if present, is the final arm). Reused by every pass that reads a
/// trait's per-transition guard/effects (the L1 entity-field/set-value
/// checks below and the main state-machine lowering) so term-match never
/// needs its own parallel code path downstream — the whole point of "pure
/// L1 sugar, zero evaluator/codegen work".
pub(crate) fn expand_term_match(tr: &Transition) -> Vec<Transition> {
    let Some(tm) = &tr.term_match else {
        return vec![tr.clone()];
    };
    let scrutinee_sigil = tm.scrutinee.sigil();
    tm.arms
        .iter()
        .map(|arm| {
            let guard = match &arm.pattern {
                MatchPattern::Wildcard => None,
                other => Some(SExpression::Array(vec![
                    SExpression::String("==".to_string()),
                    SExpression::String(scrutinee_sigil.clone()),
                    other.literal_sexpr(),
                ])),
            };
            Transition {
                event: tr.event.clone(),
                target: tr.target.clone(),
                guard,
                effects: arm.effects.clone(),
                span: arm.span,
                guard_spans: Vec::new(),
                effect_spans: arm.effect_spans.clone(),
                term_match: None,
            }
        })
        .collect()
}

/// True when `arr` is the `["match", scrutinee, [pattern, value], …]` shape
/// `parser::expression::parse_expr_match` (Phase 3b) produces. `match` is a
/// reserved sexpr HEAD (parsed specially, never a legal runtime op) — but
/// the string `"match"` itself is an ordinary identifier everywhere else in
/// the grammar, and a `let` binding pair is ALSO a bare 2-element
/// `[String(name), value]` array (`parser::expression::parse_let_bindings`)
/// that never goes through the reserved-head check (only a call's OPERATOR
/// position is reserved, not a binding NAME) — so `(let ((match (array/
/// first …))) …)`, a real corpus idiom (`std-civics-checks.lolo`,
/// `std-learn-derivation.lolo` both bind a local named `match`), parses to
/// the SAME shape as a zero-arm match. Full shape check, not just the head:
/// at least one arm (head + scrutinee + ≥1 arm = len ≥ 3 — the parser
/// itself enforces ≥1 non-wildcard arm) AND every element past the
/// scrutinee is itself a 2-element `[pattern, value]` array — a let-binding
/// pair is always exactly 2 elements, so it can never satisfy `len >= 3`.
/// Shared by the desugar below and `checks::check_expr_match_types`.
pub(crate) fn is_expr_match_array(arr: &[SExpression]) -> bool {
    matches!(arr.first(), Some(SExpression::String(s)) if s == "match")
        && arr.len() >= 3
        && arr[2..]
            .iter()
            .all(|arm| matches!(arm, SExpression::Array(pair) if pair.len() == 2))
}

/// Recursively desugar expression-level `match` sugar (Phase 3b, pattern-
/// matching doc §3b) — `(match <scrutinee> (<pattern> <value>) … (_
/// <value>))`, parsed verbatim into an ordinary `["match", scrutinee, [pat,
/// val], …]` SExpression array — into nested `(if (== <scrutinee> <lit>)
/// <val> <else>)`, anywhere it appears in an SExpression tree (a value
/// position can nest arbitrarily deep: operator args, let bodies, object/
/// array literal values). Bottom-up: a nested match inside an arm's value
/// desugars before the outer match wraps it, so nesting composes for free.
/// Applied at the point each guard/effect/default is embedded into the
/// output schema — never mutates the parsed AST itself, so the formatter
/// (which reads that AST) still round-trips the original sugar.
pub(crate) fn desugar_expr_match(expr: &SExpression) -> SExpression {
    match expr {
        SExpression::Array(arr) if is_expr_match_array(arr) => {
            let scrutinee = desugar_expr_match(&arr[1]);
            let arms: Vec<(SExpression, SExpression)> = arr[2..]
                .iter()
                .map(|arm| match arm {
                    SExpression::Array(pair) if pair.len() == 2 => {
                        (pair[0].clone(), desugar_expr_match(&pair[1]))
                    }
                    _ => unreachable!("malformed match arm reached desugar"),
                })
                .collect();
            build_nested_if(&scrutinee, &arms)
        }
        SExpression::Array(arr) => SExpression::Array(arr.iter().map(desugar_expr_match).collect()),
        SExpression::Object(obj) => SExpression::Object(
            obj.iter()
                .map(|(k, v)| (k.clone(), desugar_expr_match(v)))
                .collect(),
        ),
        other => other.clone(),
    }
}

/// `notify` is sugar over `render-ui toast` (owner ruling 2026-09-11): the
/// `.orb` IR no longer carries a `notify` effect kind at all, so this runs at
/// lowering, not codegen. `(notify <level> <message>)` — level one of
/// `success|error|warning|info` (or a config/runtime binding, e.g.
/// `@config.severity`, left unchecked since L1 cannot resolve it) — becomes
/// `(render-ui toast { type: alert, variant: <level>, message: <message>,
/// dismissible: true })`. Recurses the same way `desugar_expr_match` does
/// (arrays/objects), so a `notify` nested inside `do`/`if`/`match` bodies is
/// caught too; run this AFTER `desugar_expr_match` so a `match` that yields a
/// `notify` value has already been expanded to the `if` it lands the call in.
pub(crate) fn desugar_notify(expr: &SExpression) -> Result<SExpression, LowerError> {
    const NOTIFY_LEVELS: [&str; 4] = ["success", "error", "warning", "info"];
    match expr {
        SExpression::Array(arr) if matches!(arr.first(), Some(SExpression::String(s)) if s == "notify") =>
        {
            if arr.len() != 3 {
                return Err(LowerError::NotifyInvalidArity { got: arr.len() - 1 });
            }
            let level = desugar_notify(&arr[1])?;
            let message = desugar_notify(&arr[2])?;
            if let SExpression::String(s) = &level {
                if !s.starts_with('@') && !NOTIFY_LEVELS.contains(&s.as_str()) {
                    return Err(LowerError::NotifyInvalidLevel { level: s.clone() });
                }
            }
            let mut props = orbital_core_base::schema::SObject::new();
            props.insert("type".to_string(), SExpression::String("alert".to_string()));
            props.insert("variant".to_string(), level);
            props.insert("message".to_string(), message);
            props.insert("dismissible".to_string(), SExpression::Bool(true));
            Ok(SExpression::Array(vec![
                SExpression::String("render-ui".to_string()),
                SExpression::String("toast".to_string()),
                SExpression::Object(props),
            ]))
        }
        SExpression::Array(arr) => Ok(SExpression::Array(
            arr.iter().map(desugar_notify).collect::<Result<_, _>>()?,
        )),
        SExpression::Object(obj) => Ok(SExpression::Object(
            obj.iter()
                .map(|(k, v)| desugar_notify(v).map(|v| (k.clone(), v)))
                .collect::<Result<_, _>>()?,
        )),
        other => Ok(other.clone()),
    }
}

/// Build the nested `if`/`==` chain for one expr-match's (already-desugared)
/// arms. The last arm — `_` or, when the vocabulary is exhaustively covered
/// without a wildcard (Phase 2/3b share this allowance), the final
/// non-wildcard arm — needs no guard: it's the unconditional base case, so
/// it's returned bare rather than wrapped in a redundant `(if true …)`.
fn build_nested_if(scrutinee: &SExpression, arms: &[(SExpression, SExpression)]) -> SExpression {
    let (head, rest) = arms
        .split_first()
        .expect("expr-match desugar needs at least one arm (parser enforces this)");
    let (pattern, value) = head;
    let is_wildcard = matches!(pattern, SExpression::String(s) if s == "_");
    if is_wildcard || rest.is_empty() {
        return value.clone();
    }
    let cond = SExpression::Array(vec![
        SExpression::String("==".to_string()),
        scrutinee.clone(),
        pattern.clone(),
    ]);
    let else_branch = build_nested_if(scrutinee, rest);
    SExpression::Array(vec![
        SExpression::String("if".to_string()),
        cond,
        value.clone(),
        else_branch,
    ])
}

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
    source_map: &mut crate::source_map::LoloSourceMap,
    base: &str,
) -> Result<TraitReference, LowerError> {
    if let Some(span) = t.linked_entity_span {
        source_map.insert(format!("{base}.linkedEntity"), span);
    }
    if let Some(ref_data) = &t.reference {
        // Trait REFERENCES don't declare their own transitions/ticks (those
        // live in the imported atom's `.lolo`, a different source file), so
        // there's nothing to map here.
        lower_trait_reference(t, ref_data, aliases, entity_names)
    } else {
        let lowered = TraitReference::Inline(lower_trait(
            t,
            aliases,
            entities,
            entity_names,
            event_aliases,
            intrinsic_fields,
            shared_entity_names,
            source_map,
            base,
        )?);
        // L1 rung of the payload invariant — runs on the LOWERED trait so it
        // sees the same `PayloadField[]` the compiler will, and only judges
        // call sites this file can decide alone (see `payload_contract`).
        super::payload_contract::check_trait_payload_contracts(&lowered, entities)?;
        super::render_prop_types::check_trait_render_prop_types(&lowered, entity_names)?;
        Ok(lowered)
    }
}

/// The lowered override surface of one trait-reference body (`events fields config
/// listens emitsScope`) — shared by `trait X = A.traits.Y { … }` and an orbital import's
/// `traits { Y { … } }` entries.
pub(crate) struct CallSiteOverrides {
    pub events: Option<HashMap<String, String>>,
    pub listens: Option<Vec<ListenDefinition>>,
    pub emits_scope: Option<EventScope>,
    pub config: Option<HashMap<String, SchemaConfigField>>,
    pub fields: Option<HashMap<String, String>>,
}

pub(crate) fn lower_call_site_overrides(
    ref_data: &TraitRefData,
    listens: &[Listen],
    aliases: &BTreeMap<String, TypeAliasResolved>,
    entity_names: &HashSet<String>,
) -> CallSiteOverrides {
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
        listens.iter().filter(|l| l.event.contains('.')).collect();
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
                    default: cf.default.as_ref().map(desugar_expr_match),
                    label: None,
                    description: None,
                    synonyms: None,
                    tier: None,
                    values: None,
                    items: None,
                    properties: None,
                    required: None,
                    ref_id: None,
                    forwarded_from: None,
                    nested_forwarded_from: Vec::new(),
                    payload_for: None,
                    projected_from: None,
                }
            } else {
                // Declaration-form entries (an alias trait declaring its OWN
                // typed knob) lower with the real alias/entity context — the
                // same path atom config declarations take — so named struct
                // aliases (e.g. `[StepCard]`) resolve to full `items`/
                // `properties` schemas instead of the string fallback.
                let (_, _, items, properties, _) =
                    lower_type_expr(cf.type_expr.peel(), aliases, entity_names);
                SchemaConfigField {
                    field_type: type_expr_to_config_string(cf.type_expr.peel()),
                    default: canonicalize_unbound_trait_default(
                        cf.type_expr.peel(),
                        cf.default.as_ref().map(desugar_expr_match),
                    ),
                    label: cf.label.clone(),
                    description: cf.description.clone(),
                    synonyms: cf.synonyms.clone(),
                    tier: cf.tier.clone(),
                    values: type_expr_to_config_values(cf.type_expr.peel()),
                    items,
                    properties,
                    required: None,
                    ref_id: None,
                    forwarded_from: None,
                    nested_forwarded_from: Vec::new(),
                    payload_for: None,
                    projected_from: cf.type_expr.projection(),
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

    CallSiteOverrides {
        events,
        listens,
        emits_scope,
        config,
        fields,
    }
}

/// Build a `TraitReference::Reference` from a parsed `trait Name = Alias.X { ... }`.
pub(crate) fn lower_trait_reference(
    t: &Trait,
    ref_data: &TraitRefData,
    aliases: &BTreeMap<String, TypeAliasResolved>,
    entity_names: &HashSet<String>,
) -> Result<TraitReference, LowerError> {
    // Increment 3-II: reinterpret the trait's `::` annotation as call-site
    // type-parameter arguments — `[type_name] ++ type_args`, chunked into
    // (name, value) pairs — UNLESS `type_name` resolves as a known LOCAL type
    // alias, in which case the annotation keeps its pre-existing
    // alias-conformance meaning (handled entirely by `check_trait_annotation`
    // in `types.rs`; nothing to lower here) and no `typeArgs` are emitted.
    // `check_trait_annotation` already skipped the alias-conformance check
    // for exactly the branch taken here (same `aliases.contains_key` test —
    // this file's `aliases` map mirrors that check's `ResolveCtx` lookup).
    // An odd flat-pair count is a structurally malformed annotation — no
    // source-trait visibility is needed to catch it, so it errors here at
    // L1 rather than waiting on the L2 inline phase.
    let type_args = match &t.type_annotation {
        Some(ann) if !aliases.contains_key(&ann.type_name) => {
            let mut flat: Vec<String> = Vec::with_capacity(1 + ann.type_args.len());
            flat.push(ann.type_name.clone());
            flat.extend(ann.type_args.iter().cloned());
            if flat.len() % 2 != 0 {
                return Err(LowerError::OddTypeArgCount {
                    trait_name: t.name.clone(),
                    args: flat,
                });
            }
            let mut map: HashMap<String, String> = HashMap::new();
            for pair in flat.chunks(2) {
                map.insert(pair[0].clone(), pair[1].clone());
            }
            Some(map)
        }
        _ => None,
    };
    let CallSiteOverrides {
        events,
        listens,
        emits_scope,
        config,
        fields,
    } = lower_call_site_overrides(ref_data, &t.listens, aliases, entity_names);

    Ok(TraitReference::Reference {
        local: t.local,
        id: None,
        reference: Some(ref_data.reference.clone()),
        name: Some(
            ref_data
                .name_override
                .clone()
                .unwrap_or_else(|| t.name.clone()),
        ),
        linked_entity: t.linked_entity.clone(),
        config,
        events,
        fields,
        listens,
        emits_scope,
        event_ids: None,
        linked_entity_id: None,
        ref_id: None,
        type_args,
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
    source_map: &mut crate::source_map::LoloSourceMap,
    base: &str,
) -> Result<TraitDefinition, LowerError> {
    // Increment 3-II: this trait's own declared type-parameter names, usable
    // directly in its `emits`/`listens`/`config` type positions.
    let trait_type_params: Vec<&str> = t.type_params.iter().map(|p| p.name.as_str()).collect();
    source_map.insert(base.to_string(), t.span);

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
            // Term-match sugar (Phase 2) expands to one ordinary Transition
            // per arm, in authoring order, BEFORE anything below runs — see
            // `expand_term_match`. A no-op (`vec![tr.clone()]`) for every
            // transition that doesn't use `match`.
            for tr in expand_term_match(tr) {
                // Index into `transitions` this push lands at — used both as the
                // TransitionDefinition's own array index (matching L2's
                // `stateMachine.transitions[k]` path) and to key the source-map
                // inserts below.
                let k = transitions.len();
                source_map.insert(format!("{base}.stateMachine.transitions[{k}]"), tr.span);
                for (suffix, sp) in &tr.guard_spans {
                    source_map.insert(
                        format!("{base}.stateMachine.transitions[{k}].guard{suffix}"),
                        *sp,
                    );
                }
                for (e_idx, spans) in tr.effect_spans.iter().enumerate() {
                    for (suffix, sp) in spans {
                        source_map.insert(
                            format!(
                                "{base}.stateMachine.transitions[{k}].effects[{e_idx}]{suffix}"
                            ),
                            *sp,
                        );
                    }
                }
                transitions.push(TransitionDefinition {
                    from: StringOrArray::Single(state.name.clone()),
                    to: tr.target.clone(),
                    event: tr.event.clone(),
                    event_id: None,
                    guard: tr.guard.as_ref().map(desugar_expr_match),
                    effects: if tr.effects.is_empty() {
                        None
                    } else {
                        Some(
                            tr.effects
                                .iter()
                                .map(|e| desugar_notify(&desugar_expr_match(e)))
                                .collect::<Result<Vec<_>, LowerError>>()?,
                        )
                    },
                });
            }
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
                    event_id: None,
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
                .map(|f| {
                    let projected = f.type_expr.payload_projection();
                    let ty = f.type_expr.peel();
                    // Increment 3-II: a field referencing this trait's OWN
                    // declared type parameter lowers to the `$V` / `[$V]`
                    // sentinel — checked first, same as the emit-payload path.
                    let sentinel =
                        emit_listen::type_param_sentinel(ty, &trait_type_params);
                    // Inline `{ … }` struct shapes (and struct aliases /
                    // arrays of them) keep their nested `properties` — the
                    // same contract emits payloads carry. Without this a
                    // typed `payload : { id : string … }` listen lowered to
                    // a shapeless `object` (Gate B finding).
                    let array_elem = match ty {
                        crate::ast::TypeExpr::Array(inner) => {
                            emit_listen::struct_payload_properties(
                                inner,
                                aliases,
                                &trait_type_params,
                            )
                        }
                        _ => None,
                    };
                    let union_items = emit_listen::union_list_items(ty, aliases, &trait_type_params);
                    let (field_type, properties, type_when, entity) = if let Some(s) = sentinel {
                        (s, None, None, None)
                    } else if union_items.is_some() {
                        ("array".to_string(), None, None, None)
                    } else if let Some(elem) = array_elem {
                        ("[object]".to_string(), Some(elem), None, None)
                    } else if let Some(nested) = emit_listen::struct_payload_properties(
                        ty,
                        aliases,
                        &trait_type_params,
                    ) {
                        ("object".to_string(), Some(nested), None, None)
                    } else if let Some((entity_name, entity_fields, is_array)) =
                        emit_listen::named_entity_fields(ty, entities, aliases)
                    {
                        // Expand an entity reference exactly as the `emits`
                        // side does. `emits { EVT { row : E } }` and
                        // `listens { EVT { row : E } }` are the SAME authored
                        // line and must lower to the same payload; before this
                        // they disagreed (`object` + properties vs a raw entity
                        // name), so one event carried two shapes depending on
                        // which block declared it. Scalar or array (the
                        // element entity), the name is stamped either way.
                        (
                            if is_array { "[object]" } else { "object" }.to_string(),
                            Some(entity_fields),
                            None,
                            Some(entity_name),
                        )
                    } else if let Some(variants) =
                        emit_listen::union_payload_variants(ty, aliases, &trait_type_params)
                            .filter(|v| v.iter().all(|m| m.properties.is_none()))
                    {
                        // A union of primitive kinds lowers as on the `emits` side: the
                        // parent is tagged `union`, the member kinds ride in `properties`.
                        // A union with a struct variant keeps the alias-named field its
                        // member reads (`?shape.width`) are checked against.
                        ("union".to_string(), Some(variants), None, None)
                    } else {
                        let (field_type, type_when) =
                            emit_listen::lower_match_field_type(ty);
                        (field_type, None, type_when, None)
                    };
                    let is_union = field_type == "union";
                    PayloadField { values: if is_union { None } else { emit_listen::payload_literal_values(ty, aliases) },
                        name: f.name.clone(),
                        field_type,
                        required: if f.required { Some(true) } else { None },
                        description: None,
                        properties,
                        type_when,
                        entity,
                        projected_from: projected,
                        items: union_items,
                    }
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
        .map(|e| lower_emit(e, aliases, entities, event_aliases, &trait_type_params))
        .collect::<Result<Vec<_>, _>>()?;
    let mut emit_payloads: HashMap<String, Vec<PayloadField>> = HashMap::new();
    // G4 (`Almadar_Compiler_Gaps.md` §82): the emit-payload fallback below
    // (precedence 3) copies `emit_payloads[k]`'s field list onto this
    // event's OWN `payloadSchema` — the `payload_entity` marker (when the
    // source emit carries one) must travel with it, or the copy stops
    // tracking the entity once an orbital-import/trait-reference override
    // renames or extends it.
    let mut emit_payload_entities: HashMap<String, Option<String>> = HashMap::new();
    for def in &lowered_emits {
        if let Some(p) = &def.payload_schema {
            if !p.is_empty() {
                emit_payloads
                    .entry(def.event.clone())
                    .or_insert_with(|| p.clone());
                emit_payload_entities
                    .entry(def.event.clone())
                    .or_insert_with(|| def.payload_entity.clone());
            }
        }
    }

    // Declared external inputs (`EVENT -> external { … }`), marked on their
    // own event definition.
    let external_inputs: HashSet<String> = t
        .listens
        .iter()
        .filter(|l| l.external)
        .map(|l| l.event.clone())
        .collect();
    let inbound_handlers: HashMap<String, InboundDeclaration> = t
        .listens
        .iter()
        .filter_map(|l| {
            l.inbound.as_ref().map(|i| {
                (
                    l.event.clone(),
                    InboundDeclaration {
                        name: i.name.clone(),
                        answers: i.answers.clone(),
                        verifies: i.verifies.as_ref().map(|v| InboundVerification {
                            header: v.header.clone(),
                            credential: v.credential.clone(),
                        }),
                    },
                )
            })
        })
        .collect();
    let declared_inputs: HashSet<String> = t
        .listens
        .iter()
        .filter(|l| !l.event.contains('.'))
        .map(|l| l.event.clone())
        .collect();

    let events: Option<Vec<EventDefinition>> = if event_keys.is_empty() {
        None
    } else {
        Some(
            event_keys
                .into_iter()
                .map(|k| -> Result<EventDefinition, LowerError> {
                    let is_external = external_inputs.contains(&k);
                    let is_listened = declared_inputs.contains(&k);
                    let inbound = inbound_handlers.get(&k).cloned();
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
                    let mut payload_entity: Option<String> = None;
                    let payload = if let Some(fields) = listen_payloads.remove(&k) {
                        // Alias-consistency (syntax revision 2026-08-18): a
                        // listen restating a payload the orbital-scope alias
                        // also types must MATCH it — same rule as emits.
                        if let Some(resolved) = event_aliases.get(&k) {
                            // Union with this trait's OWN type params — same
                            // fix as `lower_emit`'s alias-consistency check
                            // (§81(b)): a `tpayload`-shaped struct member
                            // nested inside the alias's payload body must
                            // resolve to the SAME sentinel here as it does on
                            // the local listen restatement (which sees
                            // `trait_type_params` directly), or an honest
                            // restatement false-mismatches.
                            let alias_param: Vec<&str> = resolved
                                .type_param
                                .as_deref()
                                .into_iter()
                                .chain(trait_type_params.iter().copied())
                                .collect();
                            let alias_fields = type_expr_to_payload_fields(
                                &k,
                                &resolved.payload,
                                aliases,
                                entities,
                                &alias_param,
                            )?;
                            if let Some(detail) = emit_listen::payload_fields_mismatch_norm(
                                &fields,
                                &alias_fields,
                                entities,
                                aliases,
                            ) {
                                return Err(LowerError::PayloadAliasMismatch {
                                    event: k.clone(),
                                    detail,
                                });
                            }
                        }
                        Some(fields)
                    } else if let Some(resolved) = event_aliases.get(&k) {
                        // Same union as above — the bare alias-only path.
                        let alias_param: Vec<&str> = resolved
                            .type_param
                            .as_deref()
                            .into_iter()
                            .chain(trait_type_params.iter().copied())
                            .collect();
                        payload_entity =
                            bare_entity_alias_name(&resolved.payload, entities, &alias_param);
                        Some(type_expr_to_payload_fields(
                            &k,
                            &resolved.payload,
                            aliases,
                            entities,
                            &alias_param,
                        )?)
                    } else if let Some(fields) = emit_payloads.remove(&k) {
                        payload_entity = emit_payload_entities.remove(&k).flatten();
                        Some(fields)
                    } else {
                        None
                    };
                    Ok(EventDefinition {
                        key: k,
                        id: None,
                        name: Some(name),
                        description,
                        synonyms,
                        tier,
                        payload_schema: payload,
                        payload_entity,
                        external: is_external.then_some(true),
                        listened: is_listened.then_some(true),
                        inbound,
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
        Some(
            t.ticks
                .iter()
                .enumerate()
                .map(|(ti, tk)| {
                    source_map.insert(format!("{base}.ticks[{ti}]"), tk.span);
                    for (suffix, sp) in &tk.guard_spans {
                        source_map.insert(format!("{base}.ticks[{ti}].guard{suffix}"), *sp);
                    }
                    for (e_idx, spans) in tk.effect_spans.iter().enumerate() {
                        for (suffix, sp) in spans {
                            source_map.insert(
                                format!("{base}.ticks[{ti}].effects[{e_idx}]{suffix}"),
                                *sp,
                            );
                        }
                    }
                    lower_tick(tk)
                })
                .collect::<Result<Vec<_>, LowerError>>()?,
        )
    };

    // `scope` is required in the .orb schema (V2 Phase 6); an undeclared
    // marker defaults to `instance` per the language spec.
    let scope = Some(match t.scope.as_ref() {
        Some(crate::ast::TraitScope::Collection) => orbital_core_base::schema::TraitScope::Collection,
        Some(crate::ast::TraitScope::Instance) | None => orbital_core_base::schema::TraitScope::Instance,
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
    let config = lower_config_fields_to_map(
        &t.config,
        &t.name,
        true, // declaration site — the self-reference guard applies.
        &trait_type_params,
        aliases,
        entity_names,
    )?;

    // Type-level `match` (§25) exhaustiveness/correctness — unconditional,
    // unlike the entity-binding checks below: a match knob needs no
    // linkedEntity.
    check_match_types(t, aliases)?;

    // Term-level `match` (Phase 2) exhaustiveness/correctness — also
    // unconditional here: `@payload.<f>` scrutinees need no linkedEntity,
    // and an `@entity.<f>` scrutinee on a trait with none is itself the
    // MatchScrutineeUndeclared case, not a reason to skip the check.
    check_term_match_types(t, entities, aliases)?;

    // Expression-level `match` (Phase 3b) exhaustiveness/correctness —
    // walks every guard/effect/config-default this trait owns hunting for
    // the sugar (`is_expr_match_array`) and validates each occurrence found;
    // also unconditional (a `@payload.<f>`/`@config.<k>` scrutinee needs no
    // linkedEntity, and an unresolvable `@entity.<f>` scrutinee just widens
    // to the open-vocabulary `_`-required case rather than erroring here).
    check_expr_match_types(t, entities, aliases)?;

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
        check_set_value_types(t, entities, aliases)?;
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
        effect_row: infer_effect_row(t)?,
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
        local: t.local,
        source_behavior: None,
        source_entity_definition: None,
        id: None,
        linked_entity_id: None,
        entity_ref_ids: None,
        trait_embed_ids: None,
        type_params: if t.type_params.is_empty() {
            None
        } else {
            Some(
                t.type_params
                    .iter()
                    .map(|p| orbital_core_base::schema::TraitTypeParamDef {
                        name: p.name.clone(),
                        kind: p.kind.clone(),
                        default: p.default.clone(),
                    })
                    .collect(),
            )
        },
        type_args: None,
    })
}

/// Infer the `@rebindable` binding's field contract from the trait's own
/// `@entity.<field>` usage. Reuses `collect_entity_field_refs` (the single
/// SExpr walker — no second walker) across all transitions/guards/ticks:
/// reads become `requires`, `(set @entity.X ...)` writes become `provides`.
/// `id`/`createdAt`/`updatedAt` are already excluded from reads by the
/// collector (auto-managed fields every entity has). Both lists sorted +
/// deduped for a deterministic `.orb`.
/// Infer the trait's effect row from its own effects — the one answer to "what
/// does composing this trait bring along?" that a consumer can read without
/// opening the atom.
///
/// Mirrors `infer_entity_contract` deliberately: same traversal (states →
/// transitions → `expand_term_match`, then ticks), same "inferred, never
/// authored" discipline, and it reuses the single canonical walker
/// (`collect_effect_uses`) rather than growing a second one. Guards are NOT
/// walked — a guard decides whether an effect runs, it is not itself an effect.
///
/// A `@config.*` resource stays unresolved here by design: L1 knows one file and
/// cannot see a call site's config. The compiler re-resolves those post-inline.
/// Returns `None` for a trait that produces no effects, so traits without one
/// stay byte-identical in the `.orb`.
pub(crate) fn infer_effect_row(t: &Trait) -> Result<Option<EffectRow>, LowerError> {
    let mut builder = EffectRowBuilder::new();

    for state in &t.states {
        for tr in &state.transitions {
            for tr in expand_term_match(tr) {
                // Desugar `notify` before it reaches the row so the row's
                // `kind` never lies — it reports `render-ui`/`toast`, same
                // as the transition's own lowered effects.
                let desugared: Vec<SExpression> = tr
                    .effects
                    .iter()
                    .map(|e| desugar_notify(&desugar_expr_match(e)))
                    .collect::<Result<_, _>>()?;
                builder.add_effects(EffectSite::Transition, &desugared);
            }
        }
    }

    if !t.ticks.is_empty() {
        builder.declare_ticks();
    }
    for tick in &t.ticks {
        let desugared: Vec<SExpression> = tick
            .effects
            .iter()
            .map(|e| desugar_notify(&desugar_expr_match(e)))
            .collect::<Result<_, _>>()?;
        builder.add_effects(EffectSite::Tick, &desugared);
    }

    Ok(builder.finish())
}

pub(crate) fn infer_entity_contract(
    t: &Trait,
    intrinsic_fields: &HashSet<String>,
) -> EntityFieldContract {
    let mut referenced_fields: HashMap<String, String> = HashMap::new();
    let mut set_fields: HashSet<String> = HashSet::new();
    for state in &t.states {
        for tr in &state.transitions {
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
