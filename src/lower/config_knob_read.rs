//! G-ORB-027: every config knob a trait declares is read somewhere in its
//! orbital — an effect, a guard, a render tree (a hoisted render child counts:
//! it is a sibling trait of the same orbital), or another knob's default. A
//! declared knob nothing reads is dead surface: a call site sets it and nothing
//! changes. Judged on the lowered orbital, before inline substitutes `@config.X`.

use orbital_core_base::schema::{Orbital, TraitReference};
use std::sync::Arc;

use crate::error::LowerError;

/// `@config.<knob>` as a value; `@payload(<knob>)` / `@entity(<knob>)` as a
/// dependent type (`actionPayload : @payload action` types itself by `action`);
/// or a type-level `match @config.<knob>` arm condition, which lowers to
/// `{ "knob": "<knob>", … }`.
fn reads_knob(text: &str, knob: &str) -> bool {
    let value = format!("@config.{knob}");
    let as_value = text.match_indices(&value).any(|(i, _)| {
        text[i + value.len()..]
            .chars()
            .next()
            .is_none_or(|c| !(c.is_ascii_alphanumeric() || c == '_'))
    });
    as_value
        || text.contains(&format!("\"knob\":\"{knob}\""))
        || ["@payload", "@entity"].iter().any(|root| {
            text.contains(&format!("{root}({knob})")) || text.contains(&format!("{root}(.{knob})"))
        })
}

pub(crate) fn check_orbital_config_knobs_read(orbital: &Orbital) -> Result<(), LowerError> {
    let mut unread = Vec::new();
    for (index, trait_ref) in orbital.traits.iter().enumerate() {
        let TraitReference::Inline(def) = trait_ref else {
            continue;
        };
        let Some(config) = def.config.as_ref() else {
            continue;
        };
        for knob in config.keys() {
            // The orbital with this one declaration removed: whatever reads the knob.
            let mut rest = orbital.clone();
            if let TraitReference::Inline(own) = &mut rest.traits[index] {
                if let Some(own_config) = Arc::make_mut(own).config.as_mut() {
                    own_config.shift_remove(knob);
                }
            }
            let text = serde_json::to_string(&rest).unwrap_or_default();
            if !reads_knob(&text, knob) {
                unread.push(format!("{}.{}", def.name, knob));
            }
        }
    }
    unread.sort();
    if unread.is_empty() {
        Ok(())
    } else {
        Err(LowerError::ConfigKnobUnread { unread })
    }
}
