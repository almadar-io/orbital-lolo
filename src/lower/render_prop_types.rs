//! L1 rung of the record-prop rule. A render-ui prop the pattern declares
//! object-only (`form-section.initialData`, `detail-panel.entity`) takes ONE
//! record: a `@payload.X` whose event declares `X : [T]` does not fit it, nor
//! (for an `entity`-kind prop) a row of an entity other than the trait's own.
//!
//! Like `payload_contract`, L1 judges only what this file decides alone: a
//! literal linked entity and a payload field the transition's event declares.
//! `@rebindable` / `$E` / `@entity` sentinels stay silent until inline (L2).

use std::collections::HashSet;

use orbital_core_base::schema::{PayloadField, SExpression, TraitDefinition, TraitReference};
use orbital_core_base::type_compat::{
    bound_shape_of_payload_field, record_prop_binding_mismatch, RecordBindingMismatch,
};

use crate::error::LowerError;
use crate::patterns_meta::record_prop;

pub(crate) fn check_trait_render_prop_types(
    trait_ref: &TraitReference,
    entity_names: &HashSet<String>,
) -> Result<(), LowerError> {
    let TraitReference::Inline(trait_def) = trait_ref else {
        return Ok(());
    };
    let Some(events) = &trait_def.state_machine.events else {
        return Ok(());
    };
    for transition in &trait_def.state_machine.transitions {
        let Some(payload) = events
            .iter()
            .find(|e| e.key == transition.event)
            .and_then(|e| e.payload_schema.as_deref())
        else {
            continue;
        };
        for effect in transition.effects.iter().flatten() {
            walk_effect(effect, trait_def, payload, entity_names)?;
        }
    }
    Ok(())
}

fn walk_effect(
    expr: &SExpression,
    trait_def: &TraitDefinition,
    payload: &[PayloadField],
    entity_names: &HashSet<String>,
) -> Result<(), LowerError> {
    match expr {
        SExpression::Array(items) => {
            if matches!(items.first(), Some(SExpression::String(op)) if op == "render-ui") {
                if let Some(pattern) = items.get(2) {
                    walk_pattern(pattern, trait_def, payload, entity_names)?;
                }
            }
            for item in items {
                walk_effect(item, trait_def, payload, entity_names)?;
            }
        }
        SExpression::Object(map) => {
            for value in map.values() {
                walk_effect(value, trait_def, payload, entity_names)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn walk_pattern(
    pattern: &SExpression,
    trait_def: &TraitDefinition,
    payload: &[PayloadField],
    entity_names: &HashSet<String>,
) -> Result<(), LowerError> {
    let SExpression::Object(obj) = pattern else {
        return Ok(());
    };
    if let Some(SExpression::String(pattern_type)) = obj.get("type") {
        for (prop, value) in obj {
            let Some(entity_kind) = record_prop(pattern_type, prop) else {
                continue;
            };
            let SExpression::String(binding) = value else {
                continue;
            };
            let Some(field) = binding
                .strip_prefix("@payload.")
                .map(|r| r.split('.').next().unwrap_or(r))
            else {
                continue;
            };
            let Some(declared) = payload.iter().find(|d| d.name == field) else {
                continue;
            };
            let Some(bound) =
                bound_shape_of_payload_field(&declared.field_type, declared.entity.as_deref())
            else {
                continue;
            };
            let linked = trait_def
                .linked_entity
                .as_deref()
                .filter(|l| entity_names.contains(*l));
            let expected = if entity_kind { linked } else { None };
            if let Some(m) =
                record_prop_binding_mismatch(&bound, expected, |n| entity_names.contains(n))
            {
                let (expected_label, actual) = match m {
                    RecordBindingMismatch::CollectionIntoRecord => (
                        linked.unwrap_or("object").to_string(),
                        format!("a collection ({})", declared.field_type),
                    ),
                    RecordBindingMismatch::WrongEntity { expected, actual } => {
                        (expected, format!("a {actual} row"))
                    }
                };
                return Err(LowerError::RenderPropNotARecord {
                    trait_name: trait_def.name.clone(),
                    pattern: pattern_type.clone().into_inner(),
                    prop: prop.clone(),
                    binding: binding.clone().into_inner(),
                    expected: expected_label,
                    actual,
                });
            }
        }
    }
    for value in obj.values() {
        match value {
            SExpression::Array(items) => {
                for item in items {
                    walk_pattern(item, trait_def, payload, entity_names)?;
                }
            }
            SExpression::Object(_) => walk_pattern(value, trait_def, payload, entity_names)?,
            _ => {}
        }
    }
    Ok(())
}
