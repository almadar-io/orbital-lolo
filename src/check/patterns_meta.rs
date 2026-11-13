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

use orbital_core_base::language::registry::patterns;

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
        for pattern in patterns().values() {
            for (prop_name, prop) in &pattern.props_schema {
                match prop.kind.as_deref() {
                    Some("event" | "event-ref" | "event-listen" | "callback") => {
                        names.insert(prop_name.clone());
                    }
                    Some("event-list") => {
                        if let Some(field) = prop.event_field.as_deref() {
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

/// A prop the registry declares single-record (types carry `object`, never
/// `array`): `entity_kind` when it renders the trait's linked entity, and its
/// registry `types` (a scalar of one of them fills it, as in `string | object`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordProp {
    pub entity_kind: bool,
    pub types: Vec<String>,
}

static RECORD_PROPS: OnceLock<HashMap<(String, String), RecordProp>> = OnceLock::new();

pub fn record_prop(pattern: &str, prop: &str) -> Option<RecordProp> {
    RECORD_PROPS
        .get_or_init(|| {
            let mut out = HashMap::new();
            for (pattern_name, pattern_value) in patterns() {
                for (prop_name, prop_value) in &pattern_value.props_schema {
                    let types = &prop_value.types;
                    if types.iter().any(|t| t == "object") && !types.iter().any(|t| t == "array") {
                        out.insert(
                            (pattern_name.clone(), prop_name.clone()),
                            RecordProp {
                                entity_kind: prop_value.kind.as_deref() == Some("entity"),
                                types: types.clone(),
                            },
                        );
                    }
                }
            }
            out
        })
        .get(&(pattern.to_string(), prop.to_string()))
        .cloned()
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
