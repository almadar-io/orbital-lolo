//! Patterns-registry metadata accessor for the lolo lower step.
//!
//! orbital-lolo runs BEFORE orbital-compiler, so it can't import from the
//! compiler's `embedded.rs` (would be a circular dep). This module reads
//! the same patterns registry L2 does, through the one loader in
//! `orbital_core::language_data` (the installed `@almadar/core`), and exposes the slice of metadata the L1 lolo checks
//! actually need — currently just "is this prop name an event-bearing
//! prop somewhere in the registry?".
//!
//! Kept tiny intentionally. The compiler-side `embedded.rs` reads the
//! full registry shape; lolo only needs the subset that drives parse-
//! time rules. If a future L1 check needs more (e.g. `emitPayloadSchema`
//! for L1.3's payload-literal mismatch), extend the parsed shape here
//! rather than reaching into orbital-compiler.

use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

/// The installed patterns registry. Lowering preflights
/// `language_data()` and returns a structured error before any accessor here
/// runs; see `orbital_core::language_data::language_data_after_preflight`.
pub(crate) fn patterns_json() -> &'static str {
    orbital_core::language_data::language_data_after_preflight()
        .patterns
        .as_str()
}

fn parse_registry() -> serde_json::Value {
    serde_json::from_str(patterns_json())
        .unwrap_or_else(|e| panic!("installed patterns-registry.json is corrupt ({e}) — reinstall @almadar/core"))
}

/// Set of object-key names that, per the registry, identify a string
/// value as a bus event-name reference. Two sources contribute:
///
/// 1. **Top-level prop names** marked `kind: "event"`, `"event-ref"`,
///    `"event-listen"`, or `"callback"`. The prop value (string at the
///    top of the render-ui pattern object) IS the event name.
///    Examples: `action`, `submitEvent`, `cancelEvent`, `tabChangeEvent`,
///    `pageChangeEvent`, `onTabChange`, `mapClickEvent`, ...
///
/// 2. **Inner-field names** from `kind: "event-list"` entries' `eventField`
///    metadata (defaults to `"event"`). The outer prop holds an array of
///    descriptor objects; the named field on each descriptor holds the
///    event name. Example: `itemActions: [{ event: "VIEW" }, ...]` — the
///    outer prop is `itemActions` (event-list), the inner key is `event`.
///
/// The set is the UNION of both sources across all patterns. Computed
/// once via OnceLock. Pure source-tagged — every entry derives from a
/// registry `kind` mark that pattern-sync wrote because a component
/// author typed the prop as `EventEmit<P>` / `EventKey` / a function
/// (callback) / an array of descriptors with an event field. No
/// hardcoded backstops, no name heuristics: a typo in the registry
/// shows up as a missing entry, and the upstream component types are
/// the canonical fix.
static EVENT_PROP_NAMES: OnceLock<HashSet<String>> = OnceLock::new();

pub fn event_prop_names() -> &'static HashSet<String> {
    EVENT_PROP_NAMES.get_or_init(|| {
        let mut names: HashSet<String> = HashSet::new();

        let parsed = parse_registry();
        let Some(patterns) = parsed.get("patterns").and_then(|v| v.as_object()) else {
            panic!("installed patterns-registry.json has no `patterns` object — reinstall @almadar/core");
        };
        for (_pattern_name, pattern_value) in patterns {
            let Some(props_schema) = pattern_value.get("propsSchema").and_then(|v| v.as_object())
            else {
                continue;
            };
            for (prop_name, prop_value) in props_schema {
                let Some(kind) = prop_value.get("kind").and_then(|v| v.as_str()) else {
                    continue;
                };
                match kind {
                    "event" | "event-ref" | "event-listen" | "callback" => {
                        names.insert(prop_name.clone());
                    }
                    "event-list" => {
                        // Descriptor-array prop. The event-name lives on
                        // an inner field of each item; pattern-sync
                        // records the field name in `eventField`.
                        if let Some(field) = prop_value.get("eventField").and_then(|v| v.as_str()) {
                            names.insert(field.to_string());
                        }
                    }
                    _ => {}
                }
            }
        }
        names
    })
}

/// Props the registry declares object-only (types carry `object`, never
/// `array`): each takes ONE record. The value is whether the prop is
/// `kind: "entity"` (it renders the trait's linked entity).
static RECORD_PROPS: OnceLock<HashMap<(String, String), bool>> = OnceLock::new();

pub fn record_prop(pattern: &str, prop: &str) -> Option<bool> {
    RECORD_PROPS
        .get_or_init(|| {
            let mut out = HashMap::new();
            let parsed = parse_registry();
            let Some(patterns) = parsed.get("patterns").and_then(|v| v.as_object()) else {
                panic!("installed patterns-registry.json has no `patterns` object — reinstall @almadar/core");
            };
            for (pattern_name, pattern_value) in patterns {
                let Some(props) = pattern_value.get("propsSchema").and_then(|v| v.as_object()) else {
                    continue;
                };
                for (prop_name, prop_value) in props {
                    let types: Vec<&str> = prop_value
                        .get("types")
                        .and_then(|v| v.as_array())
                        .map(|a| a.iter().filter_map(|t| t.as_str()).collect())
                        .unwrap_or_default();
                    if types.contains(&"object") && !types.contains(&"array") {
                        let entity_kind = prop_value.get("kind").and_then(|v| v.as_str()) == Some("entity");
                        out.insert((pattern_name.clone(), prop_name.clone()), entity_kind);
                    }
                }
            }
            out
        })
        .get(&(pattern.to_string(), prop.to_string()))
        .copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_carries_legacy_event_kinds() {
        // Sanity: pattern-sync should have written `kind: "event"` on
        // the foundational event-bearing props (action, submitEvent,
        // cancelEvent — driven by the EventKey alias on Button.action,
        // Form.submitEvent, Form.cancelEvent in @almadar/ui). If any
        // of these go missing, that's a registry regression — the fix
        // is to re-publish @almadar/patterns from the latest @almadar/ui
        // typings, NOT to add a hardcoded backstop here.
        let names = event_prop_names();
        assert!(
            names.contains("action"),
            "registry should mark Button.action as kind: event"
        );
        assert!(
            names.contains("submitEvent"),
            "registry should mark Form.submitEvent as kind: event"
        );
        assert!(
            names.contains("cancelEvent"),
            "registry should mark Form.cancelEvent as kind: event"
        );
    }

    #[test]
    fn includes_event_list_inner_field_names() {
        // event-list props (e.g. data-list.itemActions: DataListItemAction[])
        // record the inner field that holds the event name. The set
        // must include those inner field names so the recursive walk
        // catches `[{ event: "VIEW" }]` correctly.
        let names = event_prop_names();
        assert!(
            names.contains("event"),
            "registry's event-list eventField default should expose 'event' as an inner descriptor key"
        );
    }

    #[test]
    fn includes_f1_extracted_event_ref_names() {
        // Pattern-sync writes `kind: "event-ref"` for tabChangeEvent /
        // pageChangeEvent / etc. via the EventEmit<P> brand on the
        // @almadar/ui Tabs and Pagination components. If F1 is wired
        // through correctly, these names land in the set.
        let names = event_prop_names();
        assert!(
            names.contains("tabChangeEvent"),
            "F1 EventEmit registry plumbing should expose tabChangeEvent"
        );
        assert!(
            names.contains("pageChangeEvent"),
            "F1 EventEmit registry plumbing should expose pageChangeEvent"
        );
    }

    #[test]
    fn includes_callback_prop_names() {
        // Pattern-sync writes `kind: "callback"` for function-typed props
        // (onTabChange, onPageChange, etc.). The L1.1 reachability check
        // must treat these as event-bearing too because schema authors
        // bind them to event names at the call site.
        let names = event_prop_names();
        assert!(
            names.contains("onTabChange"),
            "F1 callback-prop plumbing should expose onTabChange"
        );
    }
}
