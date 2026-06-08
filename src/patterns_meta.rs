//! Patterns-registry metadata accessor for the lolo lower step.
//!
//! orbital-lolo runs BEFORE orbital-compiler, so it can't import from the
//! compiler's `embedded.rs` (would be a circular dep). This module reads
//! the same `packages/almadar-patterns/src/patterns-registry.json` via
//! `include_str!` and exposes the slice of metadata the L1 lolo checks
//! actually need — currently just "is this prop name an event-bearing
//! prop somewhere in the registry?".
//!
//! Kept tiny intentionally. The compiler-side `embedded.rs` reads the
//! full registry shape; lolo only needs the subset that drives parse-
//! time rules. If a future L1 check needs more (e.g. `emitPayloadSchema`
//! for L1.3's payload-literal mismatch), extend the parsed shape here
//! rather than reaching into orbital-compiler.

use std::collections::HashSet;
use std::sync::OnceLock;

const PATTERNS_JSON: &str = include_str!("baked/patterns-registry.json");

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

        let Ok(parsed) = serde_json::from_str::<serde_json::Value>(PATTERNS_JSON) else {
            // Registry malformed at compile-time-embedded path. The set
            // stays empty; lolo's existing reachability check then
            // collects no event-name literals at all (regress to a
            // permissive null check rather than emit false errors). A
            // separate sanity test below catches this in CI.
            return names;
        };
        let Some(patterns) = parsed.get("patterns").and_then(|v| v.as_object()) else {
            return names;
        };
        for (_pattern_name, pattern_value) in patterns {
            let Some(props_schema) = pattern_value
                .get("propsSchema")
                .and_then(|v| v.as_object())
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
                        if let Some(field) = prop_value.get("eventField").and_then(|v| v.as_str())
                        {
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
