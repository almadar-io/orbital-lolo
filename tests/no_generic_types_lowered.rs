//! There are no generic types in the language: a lowered program declares no `any`, `json`, `unknown`, bare
//! `object` or bare `array`. A field is an object with its `properties` or a map of its `items`, an array of its
//! `items`, a union of its variants or one named type; anything else is a generic type that slipped through
//! lowering. The one spelling of `"unknown"` that remains is the call-site wiring entry of a trait reference
//! (`config { fields: @config.fields }`): a value for a knob the trait declares, carrying no type of its own
//! (`ConfigField::is_declaration_form`).
//!
//! The monorepo corpus sample is skipped when the checkout has no `packages/` sibling (standalone CI).

use orbital_lolo::{parse_and_lower_str, parse_and_lower_str_with, LowerOptions};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};

const GENERIC: [&str; 3] = ["any", "json", "unknown"];

/// `payload : @payload event` (a struct member whose type is the payload of the event its sibling names)
/// lowers to a property-less `object`: the lowering erases the dependent type after the L1 sibling check
/// (`check_payload_sibling_refs`). There is no honest static type for it, so it is a tracked erasure with a
/// ratchet, not a pass: the count over the std behaviors may only fall (owner design: a payload-of carrier).
const PAYLOAD_OF_ERASURES: usize = 7;

fn has(map: &serde_json::Map<String, Value>, key: &str) -> bool {
    match map.get(key) {
        None | Some(Value::Null) => false,
        Some(Value::Object(o)) => !o.is_empty(),
        Some(Value::Array(a)) => !a.is_empty(),
        Some(_) => true,
    }
}

/// Every position in `value` whose `type` is a generic one.
fn generic_types(value: &Value, path: &str, out: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            if let Some(Value::String(ty)) = map.get("type") {
                let wiring = ty == "unknown"
                    && !has(map, "label")
                    && !has(map, "description")
                    && !has(map, "synonyms")
                    && !has(map, "tier");
                if GENERIC.contains(&ty.as_str()) && !wiring {
                    out.push(format!("{path}: {ty}"));
                }
                let shaped = has(map, "properties") || has(map, "items") || has(map, "values");
                if ty == "object" && !shaped && has(map, "relation") {
                    // a relation field is typed by its target
                } else if ty == "object" && !shaped && map.contains_key("name") {
                    out.push(format!("{path}: bare object"));
                }
                if ty == "array" && !has(map, "items") && map.contains_key("name") {
                    out.push(format!("{path}: bare array"));
                }
                if ty == "union" && !has(map, "properties") && !has(map, "values") {
                    out.push(format!("{path}: union naming nothing"));
                }
            }
            for (k, v) in map {
                generic_types(v, &format!("{path}/{k}"), out);
            }
        }
        Value::Array(items) => {
            for (i, v) in items.iter().enumerate() {
                generic_types(v, &format!("{path}[{i}]"), out);
            }
        }
        _ => {}
    }
}

fn lowered(source: &str) -> Value {
    serde_json::to_value(parse_and_lower_str(source).unwrap_or_else(|e| panic!("{e}"))).unwrap()
}

fn found(source: &str) -> Vec<String> {
    let mut out = Vec::new();
    generic_types(&lowered(source), "", &mut out);
    out
}

const PROGRAM: &str = r#"
type Point = { x : number!, y : number! }
type Tree = { label : string!, children : [Tree] }
type Circle = { kind : "circle", r : number! }
type Rect = { kind : "rect", w : number!, h : number! }
type Shape = Circle | Rect

orbital Doc {
  entity Doc [runtime] {
    id : string!
    tags : [string] = []
    labels : Map string string = {}
    origin : Point
    tree : Tree
    pair : [number, number] = [0, 0]
    shape : Shape
    when : date
  }

  trait Walker -> Doc [interaction] {
    state idle {
      INIT -> idle
        (set @entity.id (json/stringify @config.limit))
    }
    emits {
      MOVED -> external {
        to : Point
        path : [Point]
      }
    }
    config { limit : number = 3 }
  }
}
"#;

#[test]
fn a_program_of_every_declared_type_lowers_with_no_generic_type() {
    assert_eq!(found(PROGRAM), Vec::<String>::new());
}

#[test]
fn control_the_scan_sees_a_generic_type() {
    let mut out = Vec::new();
    let bad = serde_json::json!({
        "entity": { "fields": [
            { "name": "a", "type": "any" },
            { "name": "b", "type": "object" },
            { "name": "c", "type": "array" },
            { "name": "d", "type": "object", "properties": { "x": { "type": "number" } } },
            { "name": "e", "type": "unknown", "label": "E" }
        ] }
    });
    generic_types(&bad, "", &mut out);
    assert_eq!(out.len(), 4, "{out:?}");
    let mut wiring = Vec::new();
    generic_types(&serde_json::json!({ "config": { "k": { "type": "unknown", "default": 1 } } }), "", &mut wiring);
    assert!(wiring.is_empty(), "the call-site wiring entry is not a declaration: {wiring:?}");
}

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    let mut entries: Vec<_> = rd.flatten().map(|e| e.path()).collect();
    entries.sort();
    for p in entries {
        if p.is_dir() {
            collect(&p, out);
        } else if p.extension().is_some_and(|e| e == "lolo") {
            out.push(p);
        }
    }
}

/// Lowers every behavior under `relative` and returns (generic types, `@payload <sibling>` erasures, files scanned).
fn scan(relative: &str) -> Option<(Vec<String>, Vec<String>, usize)> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let base = root.join(relative);
    if !base.is_dir() {
        return None;
    }
    let packages = orbital_core_base::behavior_packages::discover(&root, &root).expect("discover packages");
    let options = LowerOptions { packages, ..Default::default() };
    let mut files = Vec::new();
    collect(&base, &mut files);
    let mut report = Vec::new();
    let mut erasures = Vec::new();
    let mut scanned = 0;
    for file in files {
        let source = fs::read_to_string(&file).unwrap();
        let schema = match parse_and_lower_str_with(&source, &options) {
            Ok(schema) => schema,
            Err(e) => {
                report.push(format!("{}: does not lower: {e}", file.display()));
                continue;
            }
        };
        scanned += 1;
        let mut out = Vec::new();
        generic_types(&serde_json::to_value(&schema).unwrap(), "", &mut out);
        for hit in out {
            let line = format!("{}: {hit}", file.display());
            if hit.ends_with("bare object") {
                erasures.push(line);
            } else {
                report.push(line);
            }
        }
    }
    Some((report, erasures, scanned))
}

#[test]
fn the_std_behaviors_lower_with_no_generic_type() {
    let Some((report, erasures, scanned)) = scan("packages/almadar-std/behaviors/lolo") else {
        return;
    };
    assert!(scanned > 400, "the corpus must not shrink to nothing ({scanned})");
    assert!(report.is_empty(), "{} generic types:\n{}", report.len(), report.join("\n"));
    assert_eq!(erasures.len(), PAYLOAD_OF_ERASURES, "the tracked `@payload <sibling>` erasures changed:\n{}", erasures.join("\n"));
}

#[test]
fn the_io_behaviors_lower_with_no_generic_type() {
    let Some((report, erasures, scanned)) = scan("packages/almadar-behaviors/behaviors/lolo") else {
        return;
    };
    assert!(scanned > 1000, "the corpus must not shrink to nothing ({scanned})");
    assert!(report.is_empty(), "{} generic types:\n{}", report.len(), report.join("\n"));
    assert_eq!(erasures.len(), 0, "the io behaviors erase no payload type:\n{}", erasures.join("\n"));
}
