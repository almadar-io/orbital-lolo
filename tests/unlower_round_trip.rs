//! `.orb` → `.lolo` printer gate: IR identity. For a `.lolo` source L,
//! `lower(L) == lower(print(unlower(lower(L))))` modulo identity fields.
//! The monorepo corpus sample is skipped when the checkout has no `tools/` or
//! `packages/` siblings (standalone CI).

use orbital_lolo::{parse_and_lower_str_with, schema_to_lolo, LowerOptions};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};

fn is_identity_key(k: &str) -> bool {
    k == "id" || k == "ledger" || k == "schemaVersion" || k.ends_with("Id") || k.ends_with("Ids")
}

fn normalize(value: Value) -> Value {
    match value {
        Value::Object(map) => Value::Object(
            map.into_iter()
                .filter(|(k, v)| !is_identity_key(k) && !v.is_null())
                .map(|(k, v)| (k, normalize(v)))
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.into_iter().map(normalize).collect()),
        v => v,
    }
}

fn first_diff(a: &Value, b: &Value, path: &str) -> Option<String> {
    match (a, b) {
        (Value::Object(x), Value::Object(y)) => {
            let mut keys: Vec<&String> = x.keys().chain(y.keys()).collect();
            keys.sort();
            keys.dedup();
            for k in keys {
                let p = format!("{path}/{k}");
                match (x.get(k), y.get(k)) {
                    (Some(l), Some(r)) => {
                        if let Some(d) = first_diff(l, r, &p) {
                            return Some(d);
                        }
                    }
                    (l, r) => return Some(format!("{p}: before={l:?} after={r:?}")),
                }
            }
            None
        }
        (Value::Array(x), Value::Array(y)) => {
            for i in 0..x.len().max(y.len()) {
                let p = format!("{path}/{i}");
                match (x.get(i), y.get(i)) {
                    (Some(l), Some(r)) => {
                        if let Some(d) = first_diff(l, r, &p) {
                            return Some(d);
                        }
                    }
                    (l, r) => return Some(format!("{p}: before={l:?} after={r:?}")),
                }
            }
            None
        }
        (l, r) if l == r => None,
        (l, r) => Some(format!("{path}: before={l} after={r}")),
    }
}

fn round_trip(source: &str, options: &LowerOptions) -> Result<(), String> {
    let first =
        parse_and_lower_str_with(source, options).map_err(|e| format!("lower original: {e}"))?;
    let printed = schema_to_lolo(&first).map_err(|e| format!("unlower: {e}"))?;
    let second = parse_and_lower_str_with(&printed, options)
        .map_err(|e| format!("lower printed: {e}\n--- printed ---\n{printed}"))?;
    let a = normalize(serde_json::to_value(&first).unwrap());
    let b = normalize(serde_json::to_value(&second).unwrap());
    match first_diff(&a, &b, "") {
        None => Ok(()),
        Some(d) => Err(format!("IR differs at {d}")),
    }
}

fn collect(dir: &Path, out: &mut Vec<PathBuf>, accept: &dyn Fn(&Path) -> bool) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    let mut entries: Vec<_> = rd.flatten().map(|e| e.path()).collect();
    entries.sort();
    for p in entries {
        if p.is_dir() {
            collect(&p, out, accept);
        } else if p.extension().is_some_and(|e| e == "lolo") && accept(&p) {
            out.push(p);
        }
    }
}

fn run_corpus(root: &Path, files: &[PathBuf]) {
    let packages =
        orbital_core_base::behavior_packages::discover(root, root).expect("discover packages");
    let options = LowerOptions { packages };
    let mut failures = Vec::new();
    for f in files {
        let src = fs::read_to_string(f).unwrap();
        if let Err(e) = round_trip(&src, &options) {
            failures.push(format!("{}: {e}", f.display()));
        }
    }
    eprintln!(
        "round-trip: {}/{} passed",
        files.len() - failures.len(),
        files.len()
    );
    assert!(
        failures.is_empty(),
        "{} failures:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

fn monorepo_root() -> Option<PathBuf> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..");
    (root.join("tools/verify-efficacy/fixtures").is_dir()
        && root.join("packages/almadar-std/behaviors/lolo").is_dir())
    .then_some(root)
}

#[test]
fn corpus_efficacy_good_fixtures() {
    let Some(root) = monorepo_root() else { return };
    let mut files = Vec::new();
    collect(
        &root.join("tools/verify-efficacy/fixtures"),
        &mut files,
        &|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("good"))
        },
    );
    assert!(!files.is_empty());
    run_corpus(&root, &files);
}

fn sample_per_dir(base: &Path) -> Vec<PathBuf> {
    let per_dir: usize = std::env::var("UNLOWER_PER_DIR")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(3);
    let mut all = Vec::new();
    collect(base, &mut all, &|_| true);
    let mut by_dir: std::collections::BTreeMap<PathBuf, Vec<PathBuf>> = Default::default();
    for f in all {
        by_dir
            .entry(f.parent().unwrap().to_path_buf())
            .or_default()
            .push(f);
    }
    by_dir
        .into_values()
        .flat_map(|v| v.into_iter().take(per_dir))
        .collect()
}

#[test]
fn corpus_std_behaviors_sample() {
    let Some(root) = monorepo_root() else { return };
    let files = sample_per_dir(&root.join("packages/almadar-std/behaviors/lolo"));
    eprintln!("std sample: {} files", files.len());
    run_corpus(&root, &files);
}

#[test]
#[ignore = "known gaps: struct-member order lost in entity field maps; alias-path vs inline-path entity payloads"]
fn corpus_io_behaviors_sample() {
    let Some(root) = monorepo_root() else { return };
    let base = root.join("packages/almadar-behaviors/behaviors/lolo");
    if !base.is_dir() {
        return;
    }
    let files = sample_per_dir(&base);
    eprintln!("io sample: {} files", files.len());
    run_corpus(&root, &files);
}

#[test]
#[ignore = "debug helper: UNLOWER_FILE=<path> prints the unlowered source"]
fn dump_printed() {
    let Ok(file) = std::env::var("UNLOWER_FILE") else {
        return;
    };
    let root = monorepo_root().expect("monorepo checkout");
    let packages = orbital_core_base::behavior_packages::discover(&root, &root).expect("discover");
    let options = LowerOptions { packages };
    let src = fs::read_to_string(file).unwrap();
    let schema = parse_and_lower_str_with(&src, &options).unwrap();
    println!("{}", schema_to_lolo(&schema).unwrap());
}

fn monorepo_options() -> Option<LowerOptions> {
    let root = monorepo_root()?;
    let packages = orbital_core_base::behavior_packages::discover(&root, &root).ok()?;
    Some(LowerOptions { packages })
}

const IMPORT_HEAD: &str = "uses Notes from \"almadar-behaviors/std-notes\"\n\n";

fn import(body: &str) -> String {
    format!("{IMPORT_HEAD}orbital NoteImport = Notes.orbitals.NoteOrbital {{\n{body}}}\n")
}

#[test]
fn import_body_entity_bracket_round_trips() {
    let Some(options) = monorepo_options() else {
        return;
    };
    for line in [
        "entity Note [persistent: notes]",
        "entity Note [runtime]",
        "entity Note [persistent]",
        "entity Note [persistent: notes, local, shared, mock]",
        "entity Note [local]",
        "entity Note",
    ] {
        round_trip(&import(&format!("  {line}\n")), &options)
            .unwrap_or_else(|e| panic!("{line}: {e}"));
    }
}

#[test]
fn import_body_retype_round_trips() {
    let Some(options) = monorepo_options() else {
        return;
    };
    round_trip(
        &import("  retype {\n    title : text = \"untitled\"\n    pinned : boolean = true\n  }\n"),
        &options,
    )
    .unwrap();
    round_trip(
        &import("  extend {\n    dueDate : date = null\n  }\n  retype {\n    title : text\n  }\n"),
        &options,
    )
    .unwrap();
}

#[test]
fn import_body_trait_overrides_round_trip() {
    let Some(options) = monorepo_options() else {
        return;
    };
    let body = "  traits {\n    NoteList -> Note {\n      events { OPEN: SHOW }\n      fields { title: \"name\" }\n      config { pageSize: 5 }\n      emitsScope internal\n    }\n    NoteForm {\n      config { submitLabel: \"Save\" }\n    }\n  }\n";
    round_trip(&import(body), &options).unwrap();
}

#[test]
fn control_empty_import_round_trips() {
    let Some(options) = monorepo_options() else {
        return;
    };
    round_trip(&import(""), &options).unwrap();
}

fn recursive_alias_program(payload_field: &str) -> String {
    format!(
        r#"app Probe
type ToolField = {{
  name : string!
  properties : [ToolField]
}}
type ToolInput = {{
  event : string!
  payload : [ToolField]
}}
orbital TaskOrbital {{
  entity Task [persistent: tasks] {{
    id : string!
  }}
  trait TaskBrowse -> Task [interaction] {{
    initial: idle
    state idle {{
      INIT -> idle
    }}
    emits {{
      CONVERSE -> external {{
        {payload_field}
      }}
        @tier "internal"
    }}
  }}
  page "/" -> TaskBrowse [initial]
}}"#
    )
}

#[test]
fn recursive_alias_nested_in_alias_payload_round_trips() {
    let Some(options) = monorepo_options() else {
        return;
    };
    round_trip(&recursive_alias_program("tools : [ToolInput]"), &options).unwrap();
}

#[test]
fn control_recursive_alias_direct_payload_round_trips() {
    let Some(options) = monorepo_options() else {
        return;
    };
    round_trip(&recursive_alias_program("fields : [ToolField]"), &options).unwrap();
}

fn bytes_program(field_type: &str) -> String {
    format!(
        r#"app Probe
orbital BlobOrbital {{
  entity Blob [persistent: blobs] {{
    id : string!
    payload : {field_type}
  }}
  trait BlobBrowse -> Blob [interaction] {{
    initial: idle
    state idle {{
      INIT -> idle
    }}
  }}
  page "/" -> BlobBrowse [initial]
}}"#
    )
}

#[test]
fn bytes_entity_field_lowers_to_bytes_and_round_trips() {
    let Some(options) = monorepo_options() else {
        return;
    };
    let schema = parse_and_lower_str_with(&bytes_program("bytes"), &options).unwrap();
    let json = serde_json::to_string(&schema).unwrap();
    assert!(json.contains(r#""type":"bytes""#), "bytes field must keep its tag: {json}");
    round_trip(&bytes_program("bytes"), &options).unwrap();
}

#[test]
fn control_string_entity_field_does_not_lower_to_bytes() {
    let Some(options) = monorepo_options() else {
        return;
    };
    let schema = parse_and_lower_str_with(&bytes_program("string"), &options).unwrap();
    assert!(!serde_json::to_string(&schema).unwrap().contains(r#""type":"bytes""#));
}

const SPEC_PROGRAM: &str = "app shop-spec \"1.0.0\"\n\norbital ShopSpec {\n  expects entity Order { total : number }\n  expects trait Checkout { emits { PLACED } }\n  expects page \"/orders\"\n}\n";

#[test]
fn a_spec_orbital_round_trips_without_growing_an_entity() {
    let Some(options) = monorepo_options() else {
        return;
    };
    round_trip(SPEC_PROGRAM, &options).unwrap();
    let printed = schema_to_lolo(&parse_and_lower_str_with(SPEC_PROGRAM, &options).unwrap()).unwrap();
    assert!(!printed.contains("entity ShopSpec"), "{printed}");
    assert!(printed.contains("expects entity Order"), "{printed}");
}

#[test]
fn a_spec_event_payload_round_trips() {
    let Some(options) = monorepo_options() else {
        return;
    };
    let source = "app shop-spec \"1.0.0\"\n\norbital ShopSpec {\n  expects trait Checkout { emits { PLACED { orderId : string!, note : string } } }\n}\n";
    round_trip(source, &options).unwrap();
    let printed = schema_to_lolo(&parse_and_lower_str_with(source, &options).unwrap()).unwrap();
    assert!(printed.contains("orderId : string!"), "{printed}");
}

#[test]
fn a_spec_with_storage_access_pages_and_edges_round_trips() {
    let Some(options) = monorepo_options() else {
        return;
    };
    let source = "app shop-spec \"1.0.0\"\n\norbital ShopSpec {\n  expects entity Order [persistent, shared] {\n    @read (= @entity.id \"x\")\n    @update none \"joins another entity\"\n    id : string! @key\n    email : string @unique\n  }\n  expects page \"/orders\" -> Checkout, Receipt\n  expects trait Checkout { state idle { PAY -> paid } }\n  expects trait Checkout { state refunded }\n}\n";
    round_trip(source, &options).unwrap();
}

const PROPOSITION_SPEC: &str = "app shop-spec \"1.0.0\"\n\norbital ShopSpec {\n  expects entity Order [persistent] {\n    total : number\n    paid : number\n    @invariant (>= @entity.total 0)\n    @invariant (<= @entity.paid @entity.total)\n  }\n  expects trait Checkout { state paid @holds (= @entity.status \"paid\") }\n}\n";

#[test]
fn spec_invariants_and_holds_lower_into_the_ir_and_round_trip() {
    let Some(options) = monorepo_options() else {
        return;
    };
    let schema = parse_and_lower_str_with(PROPOSITION_SPEC, &options).unwrap();
    let json = serde_json::to_value(&schema).unwrap();
    let expects = &json["orbitals"][0]["expects"];
    let order = expects.as_array().unwrap().iter().find(|e| e["kind"] == "entity").unwrap();
    assert_eq!(order["invariants"].as_array().map(Vec::len), Some(2), "{order}");
    let paid = expects.as_array().unwrap().iter().find(|e| e["kind"] == "state").unwrap();
    assert_eq!(paid["holds"], serde_json::json!(["=", "@entity.status", "paid"]), "{paid}");
    round_trip(PROPOSITION_SPEC, &options).unwrap();
    let printed = schema_to_lolo(&schema).unwrap();
    assert!(printed.contains("@invariant (<= @entity.paid @entity.total)"), "{printed}");
    assert!(printed.contains("state paid @holds"), "{printed}");
}

#[test]
fn a_spec_without_propositions_carries_no_invariant_fields() {
    let Some(options) = monorepo_options() else {
        return;
    };
    let json = serde_json::to_string(&parse_and_lower_str_with(SPEC_PROGRAM, &options).unwrap()).unwrap();
    assert!(!json.contains("invariants") && !json.contains("holds"), "{json}");
}

#[test]
fn propositions_outside_a_spec_are_refused() {
    let Some(options) = monorepo_options() else {
        return;
    };
    let in_body = "app shop \"1.0.0\"\n\norbital Shop {\n  entity Cart [runtime] { id : string }\n  expects entity Order { total : number\n    @invariant (>= @entity.total 0)\n  }\n  trait CartView -> Cart [interaction] {\n    state idle {\n      INIT -> idle\n    }\n  }\n  page \"/cart\" -> CartView\n}\n";
    let err = parse_and_lower_str_with(in_body, &options).unwrap_err().to_string();
    assert!(err.contains("ELOLO_INVARIANT_SPEC_ONLY"), "{err}");
    let on_entity = "app shop \"1.0.0\"\n\norbital Shop {\n  entity Order [runtime] {\n    total : number\n    @invariant (>= @entity.total 0)\n  }\n}\n";
    let err = parse_and_lower_str_with(on_entity, &options).unwrap_err().to_string();
    assert!(err.contains("ELOLO_INVARIANT_SPEC_ONLY"), "{err}");
}
