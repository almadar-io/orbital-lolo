//! L1 rung of the payload invariant.
//!
//! An event's declared payload type IS its access path. This pass catches, at
//! parse time, the case a single file can decide on its own: a
//! `persist create|update <Entity>` whose `emit.success` names an event that
//! declares a field the persisted row does not have — so every read of it is
//! `undefined` at runtime.
//!
//! **Scope is deliberately narrower than L2's.** L1 sees one file, so it only
//! judges a call site whose entity is a LITERAL naming an entity declared in
//! that same file. It stays silent on:
//!
//! - `@rebindable` traits and `@entity` / `$E` sentinels — the bound entity is
//!   not fixed until a consumer binds it, which is inherently post-inline (L2).
//! - `@config.`-bound entity names — resolved by config folding, also L2.
//! - the legacy 3-element `persist` form, which carries no action keyword.
//! - `fetch` / `ref` — their `{ data, totalCount }` wrapper is a contract L2
//!   checks against the registry; nothing here adds certainty L1 has.
//!
//! Anything L1 reports, L2 would also report. The value is the feedback
//! arriving while the author is still in the file.
//!
//! The contract itself is NOT restated here — `orbital-core`'s
//! `effects::emit_contract` owns it, so L1 and L2 answer "what does this
//! operator emit?" from one place (they are sibling crates; L1 cannot reach
//! the operator registry L2 uses). A test in `orbital-compiler`
//! (`tests/emit_contract_drift.rs`) asserts that table still agrees with
//! `canonical-operators.json`.

use orbital_core::effects::{success_payload, undeclared_reads, CallVariant, ProducedPayload};
use orbital_core::schema::{SExpression, TraitDefinition, TraitReference};
use std::collections::BTreeMap;

use crate::ast::TypeExpr;
use crate::error::LowerError;

/// Walk one lowered trait's effects and report the first declared-but-never-
/// emitted payload field.
///
/// `entities` is the file's own entity table (name → fields), the same map the
/// rest of lowering uses.
pub(crate) fn check_trait_payload_contracts(
    trait_ref: &TraitReference,
    entities: &BTreeMap<String, Vec<(String, TypeExpr, bool)>>,
) -> Result<(), LowerError> {
    let TraitReference::Inline(trait_def) = trait_ref else {
        return Ok(());
    };
    let emit_payloads = declared_payloads(trait_def);
    if emit_payloads.is_empty() {
        return Ok(());
    }
    for transition in &trait_def.state_machine.transitions {
        let Some(effects) = &transition.effects else {
            continue;
        };
        for effect in effects {
            if let Some(err) = check_effect(effect, &emit_payloads, entities) {
                return Err(err);
            }
        }
    }
    Ok(())
}

/// A payload field whose declared TYPE is still a sentinel — `@entity`, its
/// array form, or a `$E` type parameter — is not decidable at L1: the entity it
/// stands for is only fixed once a consumer binds the trait, which happens in
/// the compiler's inline phase.
///
/// Skipping these is not an optimization, it is correctness. A bare
/// `type X = Event @entity` lowers to the synthetic single carrier
/// `value: @entity`; comparing the NAME `value` against the entity's own fields
/// would report every generic atom in the corpus as broken (verified: it flagged
/// all 13 `@entity`-declared atoms before this guard existed).
fn is_unresolved_sentinel(field_type: &str) -> bool {
    field_type == "@entity"
        || field_type == "[@entity]"
        || field_type.starts_with('$')
        || (field_type.starts_with("[$") && field_type.ends_with(']'))
}

/// `{ event -> declared field names }` for this trait's `emits` block, with
/// sentinel-typed fields dropped (see [`is_unresolved_sentinel`]).
fn declared_payloads(trait_def: &TraitDefinition) -> BTreeMap<&str, Vec<&str>> {
    trait_def
        .emits
        .as_deref()
        .unwrap_or(&[])
        .iter()
        .filter_map(|e| {
            let schema = e.payload_schema.as_deref()?;
            let names: Vec<&str> = schema
                .iter()
                .filter(|f| !is_unresolved_sentinel(&f.field_type))
                .map(|f| f.name.as_str())
                .collect();
            Some((e.event.as_str(), names))
        })
        .collect()
}

fn check_effect(
    effect: &SExpression,
    emit_payloads: &BTreeMap<&str, Vec<&str>>,
    entities: &BTreeMap<String, Vec<(String, TypeExpr, bool)>>,
) -> Option<LowerError> {
    let SExpression::Array(arr) = effect else {
        return None;
    };
    let SExpression::String(op) = arr.first()? else {
        return None;
    };
    if op != "persist" {
        return None;
    }
    // 4-element form only: ["persist", <action>, <Entity>, data, …]. The legacy
    // 3-element form has no action keyword and is left to L2.
    let SExpression::String(action) = arr.get(1)? else {
        return None;
    };
    let produced = success_payload(op, CallVariant::PersistAction(action.as_str()));
    if !matches!(produced, ProducedPayload::EntityRow) {
        return None;
    }
    let SExpression::String(entity_name) = arr.get(2)? else {
        return None;
    };
    // A `@config.`-bound or otherwise non-literal entity is not decidable here.
    let entity_fields = entities.get(entity_name)?;
    let success_event = emit_success_event(arr.as_slice())?;
    let declared = emit_payloads.get(success_event.as_str())?;

    let field_names: Vec<&str> = entity_fields.iter().map(|(n, _, _)| n.as_str()).collect();
    let missing = undeclared_reads(declared, &produced, &field_names);
    let first = missing.first()?;
    Some(LowerError::PayloadNotProduced {
        event: success_event,
        field: (*first).to_string(),
        operator: format!("persist {action}"),
        entity: entity_name.clone(),
    })
}

/// `emit.success`'s event name, scanning from the back for the options object.
fn emit_success_event(args: &[SExpression]) -> Option<String> {
    args.iter().rev().find_map(|value| {
        let SExpression::Object(obj) = value else {
            return None;
        };
        let SExpression::Object(emit_obj) = obj.get("emit")? else {
            return None;
        };
        match emit_obj.get("success")? {
            SExpression::String(s) => Some(s.clone()),
            _ => None,
        }
    })
}
