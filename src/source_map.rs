//! L2 source map: maps a resolved-schema JSON-pointer-style path — the same
//! `path` strings L2 validators (`orbital-compiler`) attach to their
//! diagnostics, e.g. `orbitals[0 (App)].traits[2 (Foo)].stateMachine.transitions[1].effects[0][1]`
//! — back to the `.lolo` [`Span`] that produced it. Built during lowering
//! (see [`crate::lower::lower_to_orb_with_map`]) and never enters the
//! emitted `.orb` JSON — it's a side channel for diagnostics only.
//!
//! Known degradations (a lookup misses or lands on an ancestor's span, never
//! a wrong one): `normalize_fn_shapes` can reorder a lowered `fn`'s argument
//! list after this map is built, and the inline phase splices resolved
//! subtrees from OTHER `.lolo` files in place of `@trait.X` references —
//! neither carries its originating span forward.

use crate::ast::Span;
use std::collections::HashMap;

/// See module docs.
#[derive(Debug, Clone, Default)]
pub struct LoloSourceMap {
    entries: HashMap<String, Span>,
}

impl LoloSourceMap {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record `span` for `pointer`. First-wins: a pointer already present
    /// (e.g. re-recorded via `parse_sexpr` + `parse_sexpr_or_value` both
    /// calling `record_expr_span` on the same node) keeps its original span.
    pub fn insert(&mut self, pointer: impl Into<String>, span: Span) {
        self.entries.entry(pointer.into()).or_insert(span);
    }

    /// Look up an L2 path. Normalizes `[N (name)]` annotations to `[N]`, then
    /// tries the exact path and progressively shorter prefixes (dropping one
    /// trailing `[N]` or `.key` segment at a time) until a tracked pointer is
    /// found. Returns `None` on a fully-untracked path or one containing a
    /// non-numeric bracket (a shape this map doesn't index).
    pub fn lookup(&self, l2_path: &str) -> Option<Span> {
        self.lookup_detailed(l2_path).map(|(span, _)| span)
    }

    /// Like [`lookup`](Self::lookup), but also reports whether the hit was
    /// exact (`true`) or a longest-prefix fallback to an ancestor's span
    /// (`false`). A fallback span is never wrong — it is the nearest tracked
    /// ancestor — but for subtrees spliced by the inline phase (config
    /// defaults, `@trait.X` references) it can be far from the expression
    /// that actually failed, so callers surface approximate hits differently.
    pub fn lookup_detailed(&self, l2_path: &str) -> Option<(Span, bool)> {
        let normalized = normalize(l2_path)?;
        let mut candidate: &str = &normalized;
        loop {
            if let Some(span) = self.entries.get(candidate) {
                return Some((*span, candidate == normalized));
            }
            candidate = strip_last_segment(candidate)?;
        }
    }
}

/// Drop `" (name)"` annotations inside every `[...]` bracket, keeping only
/// the leading digit run: `orbitals[0 (App)]` -> `orbitals[0]`. Returns
/// `None` if any bracket's content isn't digit-led.
fn normalize(path: &str) -> Option<String> {
    let mut out = String::with_capacity(path.len());
    let mut rest = path;
    while let Some(open) = rest.find('[') {
        out.push_str(&rest[..open]);
        let after_open = &rest[open + 1..];
        let close = after_open.find(']')?;
        let inner = &after_open[..close];
        let digits: String = inner.chars().take_while(|c| c.is_ascii_digit()).collect();
        if digits.is_empty() {
            return None;
        }
        out.push('[');
        out.push_str(&digits);
        out.push(']');
        rest = &after_open[close + 1..];
    }
    out.push_str(rest);
    Some(out)
}

/// Drop the last trailing `[N]` or `.key` segment. `None` when there's
/// nothing left to strip.
fn strip_last_segment(path: &str) -> Option<&str> {
    if path.is_empty() {
        return None;
    }
    if path.ends_with(']') {
        let open = path.rfind('[')?;
        if open == 0 {
            return None;
        }
        return Some(&path[..open]);
    }
    path.rfind('.').map(|dot| &path[..dot])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_strips_name_annotations() {
        assert_eq!(
            normalize("orbitals[0 (App)].traits[2 (Foo)]").as_deref(),
            Some("orbitals[0].traits[2]")
        );
        assert_eq!(normalize("a.b.c").as_deref(), Some("a.b.c"));
        assert_eq!(normalize("a[1][2]").as_deref(), Some("a[1][2]"));
    }

    #[test]
    fn normalize_rejects_non_numeric_bracket() {
        assert_eq!(normalize("orbitals[name=Foo].traits[0]"), None);
    }

    #[test]
    fn strip_last_segment_drops_bracket_then_dot() {
        assert_eq!(strip_last_segment("a.b[2]"), Some("a.b"));
        assert_eq!(strip_last_segment("a.b"), Some("a"));
        assert_eq!(strip_last_segment("a"), None);
        assert_eq!(strip_last_segment("orbitals[0]"), Some("orbitals"));
        assert_eq!(strip_last_segment("orbitals"), None);
    }

    #[test]
    fn lookup_exact_hit() {
        let mut map = LoloSourceMap::new();
        map.insert("orbitals[0].traits[0]", Span::new(5, 1, 1));
        assert_eq!(
            map.lookup("orbitals[0 (App)].traits[0 (Foo)]"),
            Some(Span::new(5, 1, 1))
        );
    }

    #[test]
    fn lookup_longest_prefix_fallback() {
        let mut map = LoloSourceMap::new();
        map.insert(
            "orbitals[0].traits[0].stateMachine.transitions[1]",
            Span::new(9, 3, 1),
        );
        // A deeper path than anything tracked falls back to the nearest
        // tracked ancestor.
        assert_eq!(
            map.lookup("orbitals[0].traits[0].stateMachine.transitions[1].effects[0][2].foo"),
            Some(Span::new(9, 3, 1))
        );
    }

    #[test]
    fn lookup_first_wins() {
        let mut map = LoloSourceMap::new();
        map.insert("a[0]", Span::new(1, 1, 1));
        map.insert("a[0]", Span::new(2, 2, 1));
        assert_eq!(map.lookup("a[0]"), Some(Span::new(1, 1, 1)));
    }

    #[test]
    fn lookup_miss_returns_none() {
        let map = LoloSourceMap::new();
        assert_eq!(map.lookup("orbitals[0].traits[0]"), None);
    }

    #[test]
    fn lookup_detailed_reports_exactness() {
        let mut map = LoloSourceMap::new();
        map.insert(
            "orbitals[0].traits[0].stateMachine.transitions[1]",
            Span::new(9, 3, 1),
        );
        assert_eq!(
            map.lookup_detailed("orbitals[0].traits[0].stateMachine.transitions[1]"),
            Some((Span::new(9, 3, 1), true))
        );
        assert_eq!(
            map.lookup_detailed(
                "orbitals[0].traits[0].stateMachine.transitions[1].effects[0][3].children[0]"
            ),
            Some((Span::new(9, 3, 1), false))
        );
    }
}
