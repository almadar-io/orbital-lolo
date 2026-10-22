//! L2 source map: maps a resolved-schema JSON-pointer-style path — the same
//! `path` strings L2 validators (`orbital-compiler`) attach to their
//! diagnostics, e.g. `orbitals[0 (App)].traits[2 (Foo)].stateMachine.transitions[1].effects[0][1]`
//! — back to the `.lolo` [`Span`] that produced it. Built during lowering
//! (see [`crate::lower::lower_to_orb_with_map`]) and never enters the
//! emitted `.orb` JSON — it is a side channel for diagnostics and native source inspection.
//!
//! Known degradations (a lookup misses or lands on an ancestor's span, never
//! a wrong one): `normalize_fn_shapes` can reorder a lowered `fn`'s argument
//! list after this map is built, and the inline phase splices resolved
//! subtrees from OTHER `.lolo` files in place of `@trait.X` references —
//! neither carries its originating span forward.

use crate::ast::Span;
use std::collections::HashMap;

#[derive(Debug, PartialEq, Eq)]
pub struct SourceRange {
    pub byte_length: usize,
    pub end_line: u32,
    pub end_column: u32,
}

pub fn source_range(source: &str, span: Span) -> Option<SourceRange> {
    source_range_until(source, span, None)
}

pub fn source_scope_range(source: &str, span: Span) -> Option<SourceRange> {
    source_range_until(source, span, Some(span.scope_end?))
}

fn source_range_until(source: &str, span: Span, end: Option<(u32, u32)>) -> Option<SourceRange> {
    if span.line == 0 || span.col == 0 {
        return None;
    }
    let mut chars = source.chars();
    let mut line = 1;
    let mut column = 1;
    while line != span.line || column != span.col {
        let c = chars.next()?;
        if c == '\n' {
            line += 1;
            column = 1;
        } else {
            column += 1;
        }
    }
    let mut byte_length = 0;
    let mut remaining = span.len;
    while end
        .map(|end| (line, column) != end)
        .unwrap_or(remaining > 0)
    {
        let c = chars.next()?;
        byte_length += c.len_utf8();
        if c == '\n' {
            line += 1;
            column = 1;
        } else {
            column += 1;
        }
        if remaining > 0 {
            remaining -= 1;
        }
    }
    Some(SourceRange {
        byte_length,
        end_line: line,
        end_column: column,
    })
}

/// See module docs.
#[derive(Debug, Clone, Default)]
pub struct LoloSourceMap {
    entries: HashMap<String, Span>,
}

impl LoloSourceMap {
    pub fn entries(&self) -> impl Iterator<Item = (&str, Span)> {
        self.entries
            .iter()
            .map(|(path, span)| (path.as_str(), *span))
    }

    pub fn new() -> Self {
        Self::default()
    }

    /// Record `span` for `pointer`. First-wins: a pointer already present
    /// (e.g. re-recorded via `parse_sexpr` + `parse_sexpr_or_value` both
    /// calling `record_expr_span` on the same node) keeps its original span.
    pub fn insert(&mut self, pointer: impl Into<String>, span: Span) {
        self.entries.entry(pointer.into()).or_insert(span);
    }

    /// Record the `uses` line that declares import `path`.
    pub fn insert_import(&mut self, path: &str, span: Span) {
        self.insert(format!("uses({path})"), span);
    }

    /// The `uses` line declaring import `path`: the path exactly as written, or
    /// the one import whose trailing name is `path` — the loader resolves `uses`
    /// by trailing name, and reports a missing behavior by that name.
    pub fn import_span(&self, path: &str) -> Option<Span> {
        if let Some(span) = self.entries.get(&format!("uses({path})")) {
            return Some(*span);
        }
        let mut by_name = self.entries.iter().filter_map(|(key, span)| {
            let declared = key.strip_prefix("uses(")?.strip_suffix(')')?;
            (declared.rsplit('/').next() == Some(path)).then_some(*span)
        });
        let first = by_name.next()?;
        by_name.next().is_none().then_some(first)
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
    fn source_range_converts_scalar_spans_to_original_utf8_bytes() {
        assert_eq!(
            source_range(";; 🌱\n  čelo next", Span::new(2, 3, 4)),
            Some(SourceRange {
                byte_length: 5,
                end_line: 2,
                end_column: 7
            })
        );
        assert_eq!(
            source_range("\"🌱\nč\"", Span::new(1, 1, 5)),
            Some(SourceRange {
                byte_length: 9,
                end_line: 2,
                end_column: 3
            })
        );
        assert_eq!(source_range("x", Span::new(2, 1, 1)), None);
    }

    #[test]
    fn source_scope_ranges_keep_full_bodies_beyond_token_length() {
        let source = format!("state s {{\n{}\n}}", " ".repeat(70_000));
        let mut span = Span::new(1, 1, 5);
        span.scope_end = Some((3, 2));
        let scope = source_scope_range(&source, span).unwrap();
        assert_eq!(scope.byte_length, source.len());
        assert!(scope.byte_length > usize::from(u16::MAX));
        assert_eq!(source_range(&source, span).unwrap().byte_length, 5);
    }

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
