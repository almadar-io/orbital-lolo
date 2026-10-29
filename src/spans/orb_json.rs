//! Byte spans of the top-level nodes of `.orb` JSON text, and Source Map v3 text pointing generated code at one.

use std::collections::BTreeMap;
use std::ops::Range;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeKind {
    Entity,
    Trait,
    Page,
    Service,
}

/// Byte ranges (`{` through its `}`) of each orbital's entity, traits, pages (keyed by `path`) and services, by name.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct OrbSpans {
    pub entities: BTreeMap<String, Range<usize>>,
    pub traits: BTreeMap<String, Range<usize>>,
    pub pages: BTreeMap<String, Range<usize>>,
    pub services: BTreeMap<String, Range<usize>>,
}

impl OrbSpans {
    pub fn lookup(&self, kind: NodeKind, name: &str) -> Option<Range<usize>> {
        match kind {
            NodeKind::Entity => &self.entities,
            NodeKind::Trait => &self.traits,
            NodeKind::Page => &self.pages,
            NodeKind::Service => &self.services,
        }
        .get(name)
        .cloned()
    }

    /// The spans in `source`; text that is not an `.orb` object has none.
    pub fn scan(source: &str) -> OrbSpans {
        let mut spans = OrbSpans::default();
        let Some(root) = value_range(source, 0) else {
            return spans;
        };
        each_member(source, root, &mut |key, range| {
            if key == "orbitals" {
                each_element(source, range, &mut |orbital| {
                    each_member(source, orbital, &mut |key, range| match key.as_str() {
                        "entity" => spans.track(source, NodeKind::Entity, range),
                        "traits" => each_element(source, range, &mut |r| {
                            spans.track(source, NodeKind::Trait, r)
                        }),
                        "pages" => each_element(source, range, &mut |r| {
                            spans.track(source, NodeKind::Page, r)
                        }),
                        "services" => each_member(source, range, &mut |_, r| {
                            spans.track(source, NodeKind::Service, r)
                        }),
                        _ => {}
                    });
                });
            }
        });
        spans
    }

    fn track(&mut self, source: &str, kind: NodeKind, range: Range<usize>) {
        let id = if kind == NodeKind::Page {
            "path"
        } else {
            "name"
        };
        let mut name = None;
        each_member(source, range.clone(), &mut |key, value| {
            if key == id {
                name = serde_json::from_str::<String>(&source[value]).ok();
            }
        });
        let Some(name) = name else { return };
        match kind {
            NodeKind::Entity => &mut self.entities,
            NodeKind::Trait => &mut self.traits,
            NodeKind::Page => &mut self.pages,
            NodeKind::Service => &mut self.services,
        }
        .insert(name, range);
    }
}

fn skip_ws(b: &[u8], mut i: usize) -> usize {
    while i < b.len() && b[i].is_ascii_whitespace() {
        i += 1;
    }
    i
}

/// The end of the value starting at `i`.
fn value_end(b: &[u8], i: usize) -> Option<usize> {
    match *b.get(i)? {
        b'"' => string_end(b, i),
        b'{' | b'[' => {
            let (mut depth, mut j) = (0usize, i);
            while j < b.len() {
                match b[j] {
                    b'"' => j = string_end(b, j)? - 1,
                    b'{' | b'[' => depth += 1,
                    b'}' | b']' => {
                        depth -= 1;
                        if depth == 0 {
                            return Some(j + 1);
                        }
                    }
                    _ => {}
                }
                j += 1;
            }
            None
        }
        _ => Some(
            i + b[i..]
                .iter()
                .take_while(|c| !c.is_ascii_whitespace() && !matches!(c, b',' | b'}' | b']'))
                .count(),
        ),
    }
}

fn string_end(b: &[u8], i: usize) -> Option<usize> {
    let mut j = i + 1;
    while j < b.len() {
        match b[j] {
            b'\\' => j += 1,
            b'"' => return Some(j + 1),
            _ => {}
        }
        j += 1;
    }
    None
}

fn value_range(source: &str, from: usize) -> Option<Range<usize>> {
    let b = source.as_bytes();
    let start = skip_ws(b, from);
    Some(start..value_end(b, start)?).filter(|r| b[r.start] == b'{')
}

fn each_member(source: &str, object: Range<usize>, f: &mut dyn FnMut(String, Range<usize>)) {
    let b = source.as_bytes();
    if b.get(object.start) != Some(&b'{') {
        return;
    }
    let mut i = skip_ws(b, object.start + 1);
    while i < object.end && b[i] == b'"' {
        let Some(key_end) = string_end(b, i) else {
            return;
        };
        let Ok(key) = serde_json::from_str::<String>(&source[i..key_end]) else {
            return;
        };
        i = skip_ws(b, key_end);
        if b.get(i) != Some(&b':') {
            return;
        }
        i = skip_ws(b, i + 1);
        let Some(end) = value_end(b, i) else { return };
        f(key, i..end);
        i = skip_ws(b, end);
        if b.get(i) == Some(&b',') {
            i = skip_ws(b, i + 1);
        }
    }
}

fn each_element(source: &str, array: Range<usize>, f: &mut dyn FnMut(Range<usize>)) {
    let b = source.as_bytes();
    if b.get(array.start) != Some(&b'[') {
        return;
    }
    let mut i = skip_ws(b, array.start + 1);
    while i < array.end && b[i] != b']' {
        let Some(end) = value_end(b, i) else { return };
        f(i..end);
        i = skip_ws(b, end);
        if b.get(i) == Some(&b',') {
            i = skip_ws(b, i + 1);
        }
    }
}

/// Zero-based line and column (in characters) of byte `offset` in `source`.
pub fn line_col(source: &str, offset: usize) -> (u32, u32) {
    let (mut line, mut col) = (0u32, 0u32);
    for (i, ch) in source.char_indices() {
        if i >= offset {
            break;
        }
        if ch == '\n' {
            line += 1;
            col = 0;
        } else {
            col += 1;
        }
    }
    (line, col)
}

fn vlq(out: &mut String, value: i64) {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut v = if value < 0 {
        ((-value) as u64) << 1 | 1
    } else {
        (value as u64) << 1
    };
    loop {
        let mut digit = (v & 31) as usize;
        v >>= 5;
        if v > 0 {
            digit |= 32;
        }
        out.push(ALPHABET[digit] as char);
        if v == 0 {
            return;
        }
    }
}

/// A Source Map v3 sending every line of `generated` to the start of `range` in `orb_source` (read from `orb_path`).
pub fn source_map_v3(
    generated: &str,
    orb_source: &str,
    orb_path: &str,
    range: &Range<usize>,
) -> String {
    let (line, col) = line_col(orb_source, range.start);
    let mut mappings = String::new();
    for generated_line in 0..generated.lines().count().max(1) {
        if generated_line > 0 {
            mappings.push(';');
        }
        for field in if generated_line == 0 {
            [0, 0, line as i64, col as i64]
        } else {
            [0; 4]
        } {
            vlq(&mut mappings, field);
        }
    }
    serde_json::json!({
        "version": 3,
        "sources": [orb_path],
        "sourcesContent": [orb_source],
        "names": [],
        "mappings": mappings,
    })
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCHEMA: &str = r#"{
  "name": "std-modal",
  "orbitals": [
    {
      "name": "ModalRecordOrbital",
      "entity": { "name": "ModalRecord", "fields": [] },
      "traits": [ {"name": "ModalRecordModal", "states": ["a}", "b\"{"]}, {"name": "Second"} ],
      "pages": [ {"path": "/modal", "name": "ModalPage"} ],
      "services": { "mail": { "name": "mail" } }
    }
  ]
}"#;

    #[test]
    fn scans_each_node_kind_by_its_id_and_the_range_is_the_whole_object() {
        let spans = OrbSpans::scan(SCHEMA);
        for (found, name) in [
            (spans.lookup(NodeKind::Entity, "ModalRecord"), "ModalRecord"),
            (
                spans.lookup(NodeKind::Trait, "ModalRecordModal"),
                "ModalRecordModal",
            ),
            (spans.lookup(NodeKind::Trait, "Second"), "Second"),
            (spans.lookup(NodeKind::Service, "mail"), "mail"),
        ] {
            let range = found.unwrap_or_else(|| panic!("{name} has a span"));
            let text = &SCHEMA[range];
            assert!(
                text.starts_with('{') && text.ends_with('}') && text.contains(name),
                "{text}"
            );
        }
        assert!(
            SCHEMA[spans.lookup(NodeKind::Page, "/modal").unwrap()].contains("ModalPage"),
            "pages are keyed by path"
        );
        assert_eq!(
            spans.lookup(NodeKind::Page, "ModalPage"),
            None,
            "control: a page is not keyed by its name"
        );
        assert_eq!(
            spans.traits.len(),
            2,
            "braces and quotes inside strings do not end a trait early"
        );
    }

    #[test]
    fn text_that_is_not_an_orb_object_has_no_spans() {
        for text in ["", "{}", "[1]", "app x []", "{\"orbitals\": [}"] {
            assert_eq!(OrbSpans::scan(text), OrbSpans::default(), "{text}");
        }
    }

    #[test]
    fn line_col_counts_lines_and_characters() {
        let s = "line0\nline1\nline2";
        assert_eq!(
            [0, 5, 6, 12].map(|o| line_col(s, o)),
            [(0, 0), (0, 5), (1, 0), (2, 0)]
        );
    }

    #[test]
    fn source_map_points_every_generated_line_at_the_node_start() {
        let map: serde_json::Value =
            serde_json::from_str(&source_map_v3("a\nb\nc\n", "x\ny\n  z", "p.orb", &(6..7)))
                .unwrap();
        assert_eq!(map["version"], 3);
        assert_eq!(map["sources"][0], "p.orb");
        assert_eq!(map["sourcesContent"][0], "x\ny\n  z");
        assert_eq!(
            map["mappings"], "AAEE;AAAA;AAAA",
            "line 2 col 2 once, then zero deltas"
        );
        let one: serde_json::Value =
            serde_json::from_str(&source_map_v3("", "x", "p.orb", &(0..1))).unwrap();
        assert_eq!(
            one["mappings"], "AAAA",
            "control: empty output still maps one line"
        );
    }
}
