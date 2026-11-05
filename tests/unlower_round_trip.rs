//! `.orb` → `.lolo` printer gate: IR identity. For a `.lolo` source L,
//! `lower(L) == lower(print(unlower(lower(L))))` modulo identity fields.
//! The monorepo corpus sample is skipped when the checkout has no `tools/` or
//! `packages/` siblings (standalone CI).

use orbital_lolo::{
    format_program, parse_and_lower_str_with, parse_str, schema_to_lolo, LowerOptions,
};
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
    let options = LowerOptions {
        packages,
        ..Default::default()
    };
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
    let root = std::env::var_os("ALMADAR_MONOREPO_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.."));
    root.join("packages/almadar-std/behaviors/lolo")
        .is_dir()
        .then_some(root)
}

fn matching(mut files: Vec<PathBuf>) -> Vec<PathBuf> {
    if let Ok(m) = std::env::var("UNLOWER_MATCH") {
        files.retain(|f| m.split(',').any(|m| f.to_string_lossy().contains(m)));
    }
    files
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

fn first_diff_line(a: &str, b: &str) -> String {
    let (la, lb): (Vec<&str>, Vec<&str>) = (a.lines().collect(), b.lines().collect());
    for i in 0..la.len().max(lb.len()) {
        let (x, y) = (la.get(i), lb.get(i));
        if x != y {
            return format!("line {}: source={x:?} printed={y:?}", i + 1);
        }
    }
    "texts differ only in trailing newline".to_string()
}

fn print_of_parse_is_source(source: &str) -> Result<(), String> {
    let printed = format_program(&parse_str(source).map_err(|e| format!("parse: {e}"))?);
    if printed == source {
        Ok(())
    } else {
        Err(first_diff_line(source, &printed))
    }
}

fn print_divergence_cause(source: &str, printed: &str) -> &'static str {
    let squeeze = |t: &str, commas: bool| -> String {
        t.chars()
            .filter(|c| !c.is_whitespace() && (commas || *c != ','))
            .collect()
    };
    let comments = |t: &str| -> Vec<String> {
        let mut c: Vec<String> = t
            .lines()
            .map(str::trim)
            .filter(|l| l.starts_with(";;") || l.starts_with('#'))
            .map(str::to_string)
            .collect();
        c.sort();
        c
    };
    if squeeze(source, true) == squeeze(printed, true) {
        "layout: same tokens, different whitespace, blank lines or wrapping"
    } else if squeeze(source, false) == squeeze(printed, false) {
        "commas: same tokens, different separators"
    } else if comments(source) != comments(printed) {
        "comments: the printer moves or drops a comment"
    } else {
        let sorted = |t: &str| {
            let mut w: Vec<&str> = t.split_whitespace().collect();
            w.sort_unstable();
            w.join(" ")
        };
        if sorted(source) == sorted(printed) {
            "order: same tokens in a different order"
        } else {
            "tokens: the printed program carries different tokens"
        }
    }
}

fn print_parse_divergences(
    files: &[PathBuf],
) -> std::collections::BTreeMap<&'static str, Vec<String>> {
    let mut by_cause: std::collections::BTreeMap<&'static str, Vec<String>> = Default::default();
    for f in files {
        let src = fs::read_to_string(f).unwrap();
        let Ok(program) = parse_str(&src) else {
            by_cause
                .entry("parse: the source does not parse")
                .or_default()
                .push(f.display().to_string());
            continue;
        };
        let printed = format_program(&program);
        if printed != src {
            by_cause
                .entry(print_divergence_cause(&src, &printed))
                .or_default()
                .push(format!(
                    "{}: {}",
                    f.display(),
                    first_diff_line(&src, &printed)
                ));
        }
    }
    by_cause
}

fn run_unlower_equivalence(root: &Path, files: &[PathBuf]) {
    let packages =
        orbital_core_base::behavior_packages::discover(root, root).expect("discover packages");
    let options = LowerOptions {
        packages,
        ..Default::default()
    };
    let failures: Vec<String> = files
        .iter()
        .filter_map(|f| {
            round_trip(&fs::read_to_string(f).unwrap(), &options)
                .err()
                .map(|e| format!("{}: unlower(lower(s)): {e}", f.display()))
        })
        .collect();
    eprintln!(
        "unlower(lower(s)): {} files, {} failures",
        files.len(),
        failures.len()
    );
    assert!(
        failures.is_empty(),
        "{} failures:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

fn corpus_files() -> Option<(PathBuf, Vec<PathBuf>)> {
    let root = monorepo_root()?;
    let mut files = Vec::new();
    for base in [
        "packages/almadar-std/behaviors/lolo",
        "packages/almadar-behaviors/behaviors/lolo",
    ] {
        files.extend(sample_per_dir(&root.join(base)));
    }
    Some((root.clone(), matching(files)))
}

/// Samples `UNLOWER_PER_DIR` (default 3) files per directory; `UNLOWER_PER_DIR=100000` walks the whole corpus.
#[test]
fn corpus_unlower_of_lower_is_equivalent_over_every_file() {
    let Some((root, files)) = corpus_files() else {
        return;
    };
    run_unlower_equivalence(&root, &files);
}

#[test]
fn corpus_print_of_parse_is_a_fixed_point_over_every_file() {
    let Some((_, files)) = corpus_files() else {
        return;
    };
    let print = |src: &str| -> Result<String, String> {
        Ok(format_program(
            &parse_str(src).map_err(|e| format!("parse: {e}"))?,
        ))
    };
    let failures: Vec<String> = files
        .iter()
        .filter_map(|f| {
            let src = fs::read_to_string(f).unwrap();
            let outcome = print(&src).and_then(|once| {
                let twice = print(&once)?;
                if twice == once {
                    Ok(())
                } else {
                    Err(first_diff_line(&once, &twice))
                }
            });
            outcome.err().map(|e| {
                format!(
                    "{}: print(parse(print(parse(s)))) != print(parse(s)): {e}",
                    f.display()
                )
            })
        })
        .collect();
    eprintln!(
        "print fixed point: {} files, {} failures",
        files.len(),
        failures.len()
    );
    assert!(
        failures.is_empty(),
        "{} failures:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

#[test]
#[ignore = "tool: A1 measurement: UNLOWER_MATCH narrows it; lists every corpus file whose print(parse(s)) differs from s, by cause"]
fn corpus_print_of_parse_divergence_by_cause() {
    let Some((_, files)) = corpus_files() else {
        return;
    };
    let by_cause = print_parse_divergences(&files);
    let total: usize = by_cause.values().map(Vec::len).sum();
    for (cause, rows) in &by_cause {
        eprintln!("== {cause}: {} files", rows.len());
        for row in rows {
            eprintln!("{row}");
        }
    }
    eprintln!("print(parse(s)) != s: {total} of {} files", files.len());
    assert_eq!(total, 0, "print(parse(s)) must be s over the corpus");
}

#[test]
fn print_of_parse_flags_a_source_the_printer_would_change() {
    let canonical = "app control [\n  version: \"1.0.0\",\n  description: \"probe\"\n]\n";
    let program = parse_str(canonical).expect("parses");
    let printed = format_program(&program);
    assert!(
        print_of_parse_is_source(&printed).is_ok(),
        "printer output is a fixed point"
    );
    let messy = printed.replace("\n  description", "\n\n\n  description");
    assert_ne!(messy, printed, "control: the source differs from its print");
    assert!(print_of_parse_is_source(&messy).is_err());
}

#[test]
fn corpus_std_behaviors_sample() {
    let Some(root) = monorepo_root() else { return };
    let files = matching(sample_per_dir(
        &root.join("packages/almadar-std/behaviors/lolo"),
    ));
    eprintln!("std sample: {} files", files.len());
    run_corpus(&root, &files);
}

#[test]
fn corpus_io_behaviors_sample() {
    let Some(root) = monorepo_root() else { return };
    let base = root.join("packages/almadar-behaviors/behaviors/lolo");
    if !base.is_dir() {
        return;
    }
    let files = matching(sample_per_dir(&base));
    eprintln!("io sample: {} files", files.len());
    run_corpus(&root, &files);
}

#[test]
#[ignore = "tool: debug helper: UNLOWER_FILE=<path> prints the unlowered source"]
fn dump_printed() {
    let Ok(file) = std::env::var("UNLOWER_FILE") else {
        return;
    };
    let root = monorepo_root().expect("monorepo checkout");
    let packages = orbital_core_base::behavior_packages::discover(&root, &root).expect("discover");
    let options = LowerOptions {
        packages,
        ..Default::default()
    };
    let src = fs::read_to_string(file).unwrap();
    let schema = parse_and_lower_str_with(&src, &options).unwrap();
    println!("{}", schema_to_lolo(&schema).unwrap());
}

fn monorepo_options() -> Option<LowerOptions> {
    let root = monorepo_root()?;
    let packages = orbital_core_base::behavior_packages::discover(&root, &root).ok()?;
    Some(LowerOptions {
        packages,
        ..Default::default()
    })
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
        "entity Note [persistent: notes, browser, shared]",
        "entity Note [browser]",
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
        &import(
            "  retype {\n    title : string = \"untitled\"\n    pinned : boolean = true\n  }\n",
        ),
        &options,
    )
    .unwrap();
    round_trip(
        &import(
            "  extend {\n    dueDate : date = null\n  }\n  retype {\n    title : string\n  }\n",
        ),
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
fn import_body_trait_dispatch_overrides_round_trip() {
    let Some(options) = monorepo_options() else {
        return;
    };
    let body = "  traits {\n    NoteList [optimistic]\n    NoteForm -> Note [server] {\n      config { submitLabel: \"Save\" }\n    }\n  }\n";
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
  trait TaskBrowse -> Task {{
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

fn json_value_program(arguments: &str) -> String {
    format!(
        r#"app Probe
type JsonValue = string | number | boolean | [JsonValue] | Map string JsonValue
type ToolArguments = {arguments}
type ToolCall = {{ id : string!, arguments : ToolArguments }}
orbital CallOrbital {{
  entity Call [runtime] {{
    id : string!
  }}
  trait CallBrowse -> Call {{
    initial: idle
    state idle {{
      INIT -> idle
    }}
    emits {{
      CALLED -> external {{
        calls : [ToolCall]
      }}
    }}
  }}
  page "/" -> CallBrowse [initial]
}}"#
    )
}

#[test]
fn recursive_primitive_union_alias_round_trips_as_declared() {
    let Some(options) = monorepo_options() else {
        return;
    };
    let source = json_value_program("Map string JsonValue");
    round_trip(&source, &options).unwrap();
    let schema = parse_and_lower_str_with(&source, &options).unwrap();
    let printed = schema_to_lolo(&schema).unwrap();
    assert!(
        printed.contains(
            "type JsonValue = string | number | boolean | [JsonValue] | Map string JsonValue"
        ),
        "{printed}"
    );
}

#[test]
fn control_alias_without_union_round_trips() {
    let Some(options) = monorepo_options() else {
        return;
    };
    round_trip(&json_value_program("Map string number"), &options).unwrap();
}

fn app_config_program(knob: &str) -> String {
    format!(
        r#"app Probe
config {{
  {knob}
}}
type Mode = "list" | "grid"
type NavLink = {{ label : string!, href : string! }}
type JsonValue = string | number | boolean | [JsonValue] | Map string JsonValue
orbital LinkOrbital {{
  entity Link [runtime] {{
    id : string!
  }}
  trait LinkBrowse -> Link {{
    initial: idle
    state idle {{
      INIT -> idle
    }}
  }}
  page "/" -> LinkBrowse [initial]
}}"#
    )
}

#[test]
fn app_config_alias_knobs_round_trip() {
    let Some(options) = monorepo_options() else {
        return;
    };
    for knob in [
        "mode : Mode = grid",
        "home : NavLink",
        "links : [NavLink] = []",
        "extra : Map string JsonValue",
    ] {
        round_trip(&app_config_program(knob), &options).unwrap_or_else(|e| panic!("{knob}: {e}"));
    }
}

#[test]
fn a_file_alias_used_by_the_app_config_and_an_inline_orbital_prints_once_and_round_trips() {
    let Some(options) = monorepo_options() else {
        return;
    };
    let source = app_config_program("home : NavLink").replace("    id : string!\n", "    id : string!\n    shortcut : NavLink\n");
    assert!(source.contains("shortcut : NavLink"), "{source}");
    round_trip(&source, &options).unwrap();
    let schema = parse_and_lower_str_with(&source, &options).unwrap();
    let printed = schema_to_lolo(&schema).unwrap();
    assert_eq!(printed.matches("type NavLink").count(), 1, "{printed}");
}

#[test]
fn control_app_config_primitive_knob_round_trips() {
    let Some(options) = monorepo_options() else {
        return;
    };
    round_trip(&app_config_program("pageSize : number = 5"), &options).unwrap();
}

fn spec_payload_program(claims: &str) -> String {
    format!(
        r#"app ProbeSpec
type KeyClaim = {{ name : string!, value : string! }}
orbital ProbeSpecOrbital {{
  expects trait Verifier {{
    emits {{
      VERIFIED {{ owner : string!  {claims} }}
    }}
  }}
}}"#
    )
}

#[test]
fn spec_expects_payload_struct_alias_round_trips() {
    let Some(options) = monorepo_options() else {
        return;
    };
    round_trip(&spec_payload_program("claims : [KeyClaim]!"), &options).unwrap();
}

#[test]
fn control_spec_expects_payload_primitive_round_trips() {
    let Some(options) = monorepo_options() else {
        return;
    };
    round_trip(&spec_payload_program("claims : [string]!"), &options).unwrap();
}

fn bytes_program(field_type: &str) -> String {
    format!(
        r#"app Probe
orbital BlobOrbital {{
  entity Blob [persistent: blobs] {{
    id : string!
    payload : {field_type}
  }}
  trait BlobBrowse -> Blob {{
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
    assert!(
        json.contains(r#""type":"bytes""#),
        "bytes field must keep its tag: {json}"
    );
    round_trip(&bytes_program("bytes"), &options).unwrap();
}

#[test]
fn control_string_entity_field_does_not_lower_to_bytes() {
    let Some(options) = monorepo_options() else {
        return;
    };
    let schema = parse_and_lower_str_with(&bytes_program("string"), &options).unwrap();
    assert!(!serde_json::to_string(&schema)
        .unwrap()
        .contains(r#""type":"bytes""#));
}

#[test]
fn a_program_type_field_prints_by_name_and_round_trips() {
    let Some(options) = monorepo_options() else {
        return;
    };
    let source = bytes_program("Viewer!");
    let printed = schema_to_lolo(&parse_and_lower_str_with(&source, &options).unwrap()).unwrap();
    assert!(
        printed.contains("payload : Viewer!"),
        "Viewer must print by name:\n{printed}"
    );
    round_trip(&source, &options).unwrap();
}

#[test]
fn control_an_inline_record_stays_inline() {
    let Some(options) = monorepo_options() else {
        return;
    };
    let source = bytes_program("{ name : string!, value : number }");
    let printed = schema_to_lolo(&parse_and_lower_str_with(&source, &options).unwrap()).unwrap();
    assert!(
        printed.contains("payload : { name : string!, value : number }"),
        "got:\n{printed}"
    );
    round_trip(&source, &options).unwrap();
}

const SPEC_PROGRAM: &str = "app shop-spec [version: \"1.0.0\"]\n\norbital ShopSpec {\n  expects entity Order { total : number }\n  expects trait Checkout { emits { PLACED } }\n  expects page \"/orders\"\n}\n";

#[test]
fn a_spec_orbital_round_trips_without_growing_an_entity() {
    let Some(options) = monorepo_options() else {
        return;
    };
    round_trip(SPEC_PROGRAM, &options).unwrap();
    let printed =
        schema_to_lolo(&parse_and_lower_str_with(SPEC_PROGRAM, &options).unwrap()).unwrap();
    assert!(!printed.contains("entity ShopSpec"), "{printed}");
    assert!(printed.contains("expects entity Order"), "{printed}");
}

#[test]
fn a_spec_event_payload_round_trips() {
    let Some(options) = monorepo_options() else {
        return;
    };
    let source = "app shop-spec [version: \"1.0.0\"]\n\norbital ShopSpec {\n  expects trait Checkout { emits { PLACED { orderId : string!, note : string } } }\n}\n";
    round_trip(source, &options).unwrap();
    let printed = schema_to_lolo(&parse_and_lower_str_with(source, &options).unwrap()).unwrap();
    assert!(printed.contains("orderId : string!"), "{printed}");
}

#[test]
fn a_spec_with_storage_access_pages_and_edges_round_trips() {
    let Some(options) = monorepo_options() else {
        return;
    };
    let source = "app shop-spec [version: \"1.0.0\"]\n\norbital ShopSpec {\n  expects entity Order [persistent, shared] {\n    @read (= @entity.id \"x\")\n    @update none \"joins another entity\"\n    id : string! @key\n    email : string @unique\n  }\n  expects page \"/orders\" -> Checkout, Receipt\n  expects trait Checkout { state idle { PAY -> paid } }\n  expects trait Checkout { state refunded }\n}\n";
    round_trip(source, &options).unwrap();
}

#[test]
fn a_spec_protocol_page_expectation_round_trips() {
    let Some(options) = monorepo_options() else {
        return;
    };
    let source = "app shop-spec [version: \"1.0.0\"]\n\norbital ShopSpec {\n  expects page boot -> Seed, Tick\n  expects page \"/orders\" -> Checkout\n}\n";
    round_trip(source, &options).unwrap();
    let schema = parse_and_lower_str_with(source, &options).unwrap();
    let json = serde_json::to_value(&schema).unwrap();
    let expects = json["orbitals"][0]["expects"].as_array().unwrap();
    assert_eq!(expects[0]["protocol"], "boot", "{expects:?}");
    assert!(
        expects[1].get("protocol").is_none(),
        "control: a route carries no protocol {expects:?}"
    );
    let printed = schema_to_lolo(&schema).unwrap();
    assert!(
        printed.contains("expects page boot -> Seed, Tick"),
        "{printed}"
    );
    assert!(
        printed.contains("expects page \"/orders\" -> Checkout"),
        "{printed}"
    );
}

const PROPOSITION_SPEC: &str = "app shop-spec [version: \"1.0.0\"]\n\norbital ShopSpec {\n  expects entity Order [persistent] {\n    total : number\n    paid : number\n    @invariant (>= @entity.total 0)\n    @invariant (<= @entity.paid @entity.total)\n  }\n  expects trait Checkout { state paid @holds (= @entity.status \"paid\") }\n}\n";

#[test]
fn spec_invariants_and_holds_lower_into_the_ir_and_round_trip() {
    let Some(options) = monorepo_options() else {
        return;
    };
    let schema = parse_and_lower_str_with(PROPOSITION_SPEC, &options).unwrap();
    let json = serde_json::to_value(&schema).unwrap();
    let expects = &json["orbitals"][0]["expects"];
    let order = expects
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["kind"] == "entity")
        .unwrap();
    assert_eq!(
        order["invariants"].as_array().map(Vec::len),
        Some(2),
        "{order}"
    );
    let paid = expects
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["kind"] == "state")
        .unwrap();
    assert_eq!(
        paid["holds"],
        serde_json::json!(["=", "@entity.status", "paid"]),
        "{paid}"
    );
    round_trip(PROPOSITION_SPEC, &options).unwrap();
    let printed = schema_to_lolo(&schema).unwrap();
    assert!(
        printed.contains("@invariant (<= @entity.paid @entity.total)"),
        "{printed}"
    );
    assert!(printed.contains("state paid @holds"), "{printed}");
}

#[test]
fn a_spec_without_propositions_carries_no_invariant_fields() {
    let Some(options) = monorepo_options() else {
        return;
    };
    let json =
        serde_json::to_string(&parse_and_lower_str_with(SPEC_PROGRAM, &options).unwrap()).unwrap();
    assert!(
        !json.contains("invariants") && !json.contains("holds"),
        "{json}"
    );
}

const WALLET: &str = "app wallet [version: \"1.0.0\"]\n\norbital Wallet {\n  entity Account [persistent: accounts] {\n    balance : number\n    @invariant (>= @entity.balance 0)\n  }\n  trait Wallet -> Account {\n    state active {\n      @holds (>= @entity.balance 0)\n      INIT -> active\n        (set @entity.balance 100)\n      SPEND -> active when (<= @payload.amount @entity.balance)\n        @ensures (= @entity.balance (- @fromEntity.balance @payload.amount))\n        (persist update Account @entity.id { balance: (- @entity.balance @payload.amount) })\n    }\n  }\n  page \"/wallet\" -> Wallet\n}\n";

#[test]
fn propositions_on_real_declarations_lower_into_the_ir_and_round_trip() {
    let Some(options) = monorepo_options() else {
        return;
    };
    let schema = parse_and_lower_str_with(WALLET, &options).unwrap();
    let json = serde_json::to_value(&schema).unwrap();
    let orbital = &json["orbitals"][0];
    assert_eq!(
        orbital["entity"]["invariants"],
        serde_json::json!([[">=", "@entity.balance", 0.0]]),
        "{}",
        orbital["entity"]
    );
    let sm = &orbital["traits"][0]["stateMachine"];
    let active = sm["states"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["name"] == "active")
        .unwrap();
    assert_eq!(
        active["holds"],
        serde_json::json!([">=", "@entity.balance", 0.0]),
        "{active}"
    );
    let spend = sm["transitions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["event"] == "SPEND")
        .unwrap();
    assert_eq!(
        spend["ensures"],
        serde_json::json!([[
            "=",
            "@entity.balance",
            ["-", "@fromEntity.balance", "@payload.amount"]
        ]]),
        "{spend}"
    );
    let init = sm["transitions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["event"] == "INIT")
        .unwrap();
    assert!(
        init.get("ensures").is_none(),
        "control: a transition without @ensures carries none: {init}"
    );
    round_trip(WALLET, &options).unwrap();
    let printed = schema_to_lolo(&schema).unwrap();
    assert!(
        printed.contains("@holds (>= @entity.balance 0)"),
        "{printed}"
    );
    assert!(
        printed.contains("@ensures (= @entity.balance (- @fromEntity.balance ?amount))"),
        "{printed}"
    );
}

#[test]
fn propositions_on_a_spec_transition_round_trip() {
    let Some(options) = monorepo_options() else {
        return;
    };
    let spec = "app wallet-spec [version: \"1.0.0\"]\n\norbital WalletSpec {\n  expects trait Wallet { state active { SPEND -> active @ensures (= @entity.balance (- @fromEntity.balance @payload.amount)) } }\n  expects trait Wallet { state idle { GO -> active } }\n}\n";
    let schema = parse_and_lower_str_with(spec, &options).unwrap();
    let json = serde_json::to_value(&schema).unwrap();
    let edges: Vec<&serde_json::Value> = json["orbitals"][0]["expects"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["kind"] == "transition")
        .collect();
    let spend = edges.iter().find(|e| e["event"] == "SPEND").unwrap();
    assert_eq!(
        spend["ensures"].as_array().map(Vec::len),
        Some(1),
        "{spend}"
    );
    let go = edges.iter().find(|e| e["event"] == "GO").unwrap();
    assert!(go.get("ensures").is_none(), "control: {go}");
    round_trip(spec, &options).unwrap();
}

#[test]
fn propositions_in_an_ordinary_orbital_are_accepted() {
    let Some(options) = monorepo_options() else {
        return;
    };
    let in_body = "app shop [version: \"1.0.0\"]\n\norbital Shop {\n  entity Cart [runtime] { id : string }\n  expects entity Order { total : number\n    @invariant (>= @entity.total 0)\n  }\n  trait CartView -> Cart {\n    state idle {\n      INIT -> idle\n    }\n  }\n  page \"/cart\" -> CartView\n}\n";
    let schema = parse_and_lower_str_with(in_body, &options).unwrap();
    let json = serde_json::to_string(&schema).unwrap();
    assert!(json.contains("invariants"), "{json}");
    assert!(
        !json.contains("\"strict\":true"),
        "an in-body expects stays lenient: {json}"
    );
    round_trip(in_body, &options).unwrap();
}

#[test]
fn propositions_ensures_outside_a_transition_is_refused() {
    let Some(options) = monorepo_options() else {
        return;
    };
    let on_state = "app w [version: \"1.0.0\"]\n\norbital W {\n  entity A [runtime] { n : number }\n  trait T -> A {\n    state s {\n      @ensures (>= @entity.n 0)\n      INIT -> s\n    }\n  }\n  page \"/w\" -> T\n}\n";
    let err = parse_and_lower_str_with(on_state, &options)
        .unwrap_err()
        .to_string();
    assert!(err.contains("@ensures"), "{err}");
}

fn field_knob_program(types: &str, head: &str, knobs: &str, states: &str) -> String {
    let reads = knobs
        .lines()
        .filter_map(|l| l.trim().split(" :").next())
        .filter(|n| !n.is_empty())
        .map(|n| format!("@config.{n}"))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        r#"app Probe
{types}
orbital ContactOrbital {{
  entity Contact [persistent: contacts] {{
    id : string!
    name : string
    age : number
  }}
  trait Cards {head} -> Contact {{
    config {{
{knobs}
    }}
    initial: idle
    state idle {{
      INIT -> idle
        (render-ui main {{ type: stack, children: [{reads}] }})
{states}
    }}
  }}
  page "/" -> Cards [initial]
}}"#
    )
}

fn knob_config(source: &str, options: &LowerOptions) -> serde_json::Value {
    let schema = parse_and_lower_str_with(source, options).unwrap();
    serde_json::to_value(&schema).unwrap()["orbitals"][0]["traits"][0]["config"].clone()
}

#[test]
fn a_field_ref_knob_type_round_trips() {
    let Some(options) = monorepo_options() else {
        return;
    };

    let entity = field_knob_program(
        "",
        "",
        "      titleFields : [field @entity string] = [\"name\"]\n      groupByField : field @entity = \"age\"\n      ratingField : field @entity number?",
        "",
    );
    round_trip(&entity, &options).unwrap();
    let config = knob_config(&entity, &options);
    assert_eq!(config["titleFields"]["type"], "[field]", "{config}");
    assert_eq!(config["titleFields"]["items"]["type"], "field", "{config}");
    assert_eq!(
        config["titleFields"]["items"]["record"], "@entity",
        "{config}"
    );
    assert_eq!(
        config["titleFields"]["items"]["items"]["type"], "string",
        "{config}"
    );
    assert_eq!(config["groupByField"]["type"], "field", "{config}");
    assert_eq!(config["groupByField"]["record"], "@entity", "{config}");
    assert!(
        config["groupByField"].get("items").is_none(),
        "any field has no value type: {config}"
    );
    assert_eq!(config["ratingField"]["items"]["nullable"], true, "{config}");
    let printed = schema_to_lolo(&parse_and_lower_str_with(&entity, &options).unwrap()).unwrap();
    assert!(printed.contains("[field @entity string]"), "{printed}");
    assert!(
        printed.contains("groupByField : field @entity"),
        "{printed}"
    );

    let alias = field_knob_program(
        "type Point = { x : number, label : string }",
        "",
        "      column : field Contact number = \"age\"\n      axis : field Point number = \"x\"",
        "",
    );
    round_trip(&alias, &options).unwrap();
    let config = knob_config(&alias, &options);
    assert_eq!(config["column"]["record"], "Contact", "{config}");
    assert_eq!(config["axis"]["record"], "Point", "{config}");
    assert_eq!(
        config["axis"]["properties"]["x"]["type"], "number",
        "{config}"
    );

    let payload = field_knob_program(
        "",
        "",
        "      event : event = SELECT\n      coalesceKey : field (@payload @config.event)",
        "      SELECT -> idle",
    );
    round_trip(&payload, &options).unwrap();
    let config = knob_config(&payload, &options);
    assert_eq!(
        config["coalesceKey"]["record"], "@payload(event)",
        "{config}"
    );
    let printed = schema_to_lolo(&parse_and_lower_str_with(&payload, &options).unwrap()).unwrap();
    assert!(
        printed.contains("field (@payload @config.event)"),
        "{printed}"
    );

    let variable = field_knob_program("", "(r : Entity)", "      column : field r number", "");
    round_trip(&variable, &options).unwrap();
    let config = knob_config(&variable, &options);
    assert_eq!(config["column"]["record"], "$r", "{config}");
}

#[test]
fn a_knob_named_field_is_still_a_name() {
    let Some(options) = monorepo_options() else {
        return;
    };
    let source = field_knob_program(
        "",
        "",
        "      field : string = \"x\"\n      other : field @entity",
        "",
    );
    round_trip(&source, &options).unwrap();
    let config = knob_config(&source, &options);
    assert_eq!(config["field"]["type"], "string", "{config}");
    assert_eq!(config["other"]["type"], "field", "{config}");
}

#[test]
fn a_list_of_a_union_round_trips_through_the_printer() {
    let source = r#"app Probe
type Adv = { id : number }
type Holder = { via : [(Adv | string)] }
orbital TaskOrbital {
  entity Task [persistent: tasks] {
    id : string!
  }
  trait TaskPersistor -> Task {
    initial: idle
    state idle {
      INIT -> idle
      DO_SCAN -> idle
    }
    listens {
      DO_SCAN -> input { via : [(Adv | string)], holder : Holder }
    }
  }
  page "/" -> TaskPersistor [initial]
}"#;
    let Some(options) = monorepo_options() else {
        return;
    };
    round_trip(source, &options).unwrap();
}

fn nullable_items_program(element: &str) -> String {
    format!(
        r#"app Probe
type Tree = string | number | [Tree{element}] | Map string Tree{element}
orbital TreeOrbital {{
  entity Tree [runtime] {{
    id : string!
  }}
  trait TreeBrowse -> Tree {{
    initial: idle
    state idle {{
      INIT -> idle
    }}
    emits {{
      TREE_READ -> external {{
        value : Tree
      }}
    }}
  }}
  page "/" -> TreeBrowse [initial]
}}"#
    )
}

#[test]
fn a_list_of_nullable_items_round_trips() {
    let Some(options) = monorepo_options() else {
        return;
    };
    let source = nullable_items_program("?");
    round_trip(&source, &options).unwrap();
    let schema = parse_and_lower_str_with(&source, &options).unwrap();
    let printed = schema_to_lolo(&schema).unwrap();
    assert!(printed.contains("[Tree?] | Map string Tree?"), "{printed}");
}

#[test]
fn control_a_list_of_plain_items_round_trips() {
    let Some(options) = monorepo_options() else {
        return;
    };
    round_trip(&nullable_items_program(""), &options).unwrap();
}

fn local_binding_program(binding: &str) -> String {
    format!(
        r#"app Probe
type Entry = {{ mode : string }}
orbital TableOrbital {{
  entity Table [runtime] {{
    id : string!
    table : Map string Entry
  }}
  trait TableBrowse -> Table {{
    initial: idle
    state idle {{
      INIT -> idle
      RESET -> idle
        (set @entity.table (let (({binding} {{}})) @empty))
    }}
  }}
  page "/" -> TableBrowse [initial]
}}"#
    )
}

#[test]
fn an_optional_map_and_a_map_of_optional_values_round_trip_apart() {
    let Some(options) = monorepo_options() else {
        return;
    };
    for knob in ["(Map string string)?", "Map string string?"] {
        let source = format!(
            "app Probe\norbital MapOrbital {{\n  entity Cell [runtime] {{\n    id : string!\n    tokens : {knob}\n  }}\n  trait Run -> Cell {{\n    initial: idle\n    state idle {{\n      INIT -> idle\n        (set @entity.tokens null)\n    }}\n  }}\n  page boot as MapStart -> Run\n}}\n"
        );
        round_trip(&source, &options).unwrap_or_else(|e| panic!("{knob}: {e}"));
    }
}

#[test]
fn a_typed_let_binding_prints_as_an_annotation() {
    let Some(options) = monorepo_options() else {
        return;
    };
    let source = local_binding_program("empty : Map string Entry");
    round_trip(&source, &options).unwrap();
    let schema = parse_and_lower_str_with(&source, &options).unwrap();
    let printed = schema_to_lolo(&schema).unwrap();
    assert!(
        printed.contains("(empty : Map string Entry {})"),
        "{printed}"
    );
}

#[test]
fn control_an_untyped_let_binding_round_trips() {
    let Some(options) = monorepo_options() else {
        return;
    };
    round_trip(&local_binding_program("empty"), &options).unwrap();
}

fn nested_fn_program(first: &str, second: &str) -> String {
    format!(
        r#"app Probe
orbital SumOrbital {{
  entity Sum [runtime] {{
    id : string!
    items : [number]
    total : number = 0
  }}
  trait SumBrowse -> Sum {{
    initial: idle
    state idle {{
      INIT -> idle
        (set @entity.items [1, 2])
      RUN -> idle
        (set @entity.total (array/reduce @entity.items 0 (fn s t (array/reduce @entity.items @s (fn {first} {second} (+ @{first} @{second}))))))
    }}
  }}
  page "/" -> SumBrowse [initial]
}}"#
    )
}

#[test]
fn a_lambda_that_shadows_an_outer_parameter_round_trips() {
    let Some(options) = monorepo_options() else {
        return;
    };
    round_trip(&nested_fn_program("s", "t"), &options).unwrap();
}

#[test]
fn control_a_lambda_with_fresh_parameters_round_trips() {
    let Some(options) = monorepo_options() else {
        return;
    };
    round_trip(&nested_fn_program("a", "b"), &options).unwrap();
}

fn file_and_local_types_program() -> &'static str {
    r#"app Probe [
  version: "1.0.0"
]
type Spec = { label : string! }
orbital ProbeOrbital {
  type Local = { count : number! }
  entity ProbeData [runtime] {
    id : string!
    spec : Spec
    local : Local
  }
  trait ProbeView -> ProbeData [client] {
    initial: idle
    state idle {
      INIT -> idle
        (render-ui main { type: typography, content: "probe" })
    }
  }
  page "/" -> ProbeView
}"#
}

#[test]
fn single_orbital_file_types_print_once_at_file_scope() {
    let Some(options) = monorepo_options() else {
        return;
    };
    let schema = parse_and_lower_str_with(file_and_local_types_program(), &options).unwrap();
    let printed = schema_to_lolo(&schema).unwrap();
    assert_eq!(printed.matches("type Spec =").count(), 1, "{printed}");
    let orbital_at = printed.find("orbital ProbeOrbital").unwrap();
    assert!(
        printed.find("type Spec =").unwrap() < orbital_at,
        "{printed}"
    );
    round_trip(file_and_local_types_program(), &options).unwrap();
}

#[test]
fn control_single_orbital_local_types_stay_inside_the_orbital() {
    let Some(options) = monorepo_options() else {
        return;
    };
    let schema = parse_and_lower_str_with(file_and_local_types_program(), &options).unwrap();
    let printed = schema_to_lolo(&schema).unwrap();
    assert_eq!(printed.matches("type Local =").count(), 1, "{printed}");
    let orbital_at = printed.find("orbital ProbeOrbital").unwrap();
    assert!(
        printed.find("type Local =").unwrap() > orbital_at,
        "{printed}"
    );
}

fn field_ref_program(value: &str) -> String {
    format!(
        r#"app Probe [
  version: "1.0.0"
]
orbital ProbeOrbital {{
  entity ProbeData [runtime] {{
    id : string!
    name : string
  }}
  trait ProbeView -> ProbeData [client] {{
    config {{
      relatedEntity : entity = ProbeData
      pick : (field @entity {value})?
    }}
    initial: idle
    state idle {{
      INIT -> idle
        (render-ui main {{ type: typography, content: (object/get @entity @config.pick "") }})
        (fetch @config.relatedEntity {{ emit: {{ success: LOADED, failure: FAILED }} }})
      LOADED -> idle
      FAILED -> idle
    }}
  }}
  page "/" -> ProbeView
}}"#
    )
}

#[test]
fn a_field_ref_to_a_knob_named_entity_round_trips() {
    let Some(options) = monorepo_options() else {
        return;
    };
    round_trip(
        &field_ref_program("(@entity @config.relatedEntity)"),
        &options,
    )
    .unwrap();
}

#[test]
fn control_a_field_ref_to_a_primitive_round_trips() {
    let Some(options) = monorepo_options() else {
        return;
    };
    round_trip(&field_ref_program("string"), &options).unwrap();
}

const INDEX_PROGRAM: &str = r#"app indexprobe [version: "1.0.0"]
orbital EdgeOrbital {
  entity Edge [persistent: edges] {
    @index { fields: ["graphId"] }
    @index { fields: ["graphId", "relation"], orderBy: "weight:desc" }
    id : string!
    graphId : string!
    relation : string
    weight : number
  }
}
"#;

#[test]
fn entity_indexes_unlower_to_index_directives_and_round_trip() {
    let Some(options) = monorepo_options() else {
        return;
    };
    let schema = parse_and_lower_str_with(INDEX_PROGRAM, &options).unwrap();
    let printed = schema_to_lolo(&schema).unwrap();
    assert_eq!(printed.matches("@index").count(), 2, "{printed}");
    round_trip(INDEX_PROGRAM, &options).unwrap();
}

#[test]
fn control_an_entity_without_indexes_unlowers_without_index_directives() {
    let Some(options) = monorepo_options() else {
        return;
    };
    let source = INDEX_PROGRAM
        .lines()
        .filter(|l| !l.contains("@index"))
        .collect::<Vec<_>>()
        .join("\n");
    let printed = schema_to_lolo(&parse_and_lower_str_with(&source, &options).unwrap()).unwrap();
    assert!(!printed.contains("@index"), "{printed}");
    round_trip(&source, &options).unwrap();
}

const DERIVED_PROGRAM: &str = r#"app derivedprobe [version: "1.0.0"]
orbital PersonOrbital {
  entity Person [persistent: people] {
    id : string!
    email : string!
    emailKey : string! @derived (str/lower (object/get @entity email))
  }
}
"#;

#[test]
fn a_derived_field_unlowers_to_the_annotation_and_round_trips() {
    let Some(options) = monorepo_options() else {
        return;
    };
    let schema = parse_and_lower_str_with(DERIVED_PROGRAM, &options).unwrap();
    let printed = schema_to_lolo(&schema).unwrap();
    assert!(printed.contains("@derived"), "{printed}");
    round_trip(DERIVED_PROGRAM, &options).unwrap();
}

#[test]
fn control_an_entity_without_a_derived_field_unlowers_without_the_annotation() {
    let Some(options) = monorepo_options() else {
        return;
    };
    let source = DERIVED_PROGRAM
        .lines()
        .filter(|l| !l.contains("@derived"))
        .collect::<Vec<_>>()
        .join("\n");
    let printed = schema_to_lolo(&parse_and_lower_str_with(&source, &options).unwrap()).unwrap();
    assert!(!printed.contains("@derived"), "{printed}");
    round_trip(&source, &options).unwrap();
}

fn event_alias_entity_payload_program(event: &str) -> String {
    format!(
        r#"app Probe
orbital ItemOrbital {{
  type Action = {{ id : string!, name : string! }}
  {event}
  entity Item [persistent: items] {{
    id : string!
    actions : [Action] = []
  }}
  trait ItemList -> Item {{
    initial: idle
    state idle {{
      INIT -> idle
    }}
    emits {{
      ItemLoaded
        @tier "internal"
    }}
  }}
  page "/" -> ItemList [initial]
}}"#
    )
}

#[test]
fn an_event_alias_payload_holding_an_entity_with_alias_array_round_trips() {
    let Some(options) = monorepo_options() else {
        return;
    };
    round_trip(
        &event_alias_entity_payload_program(
            "type ItemLoaded = Event { data : [Item], totalCount : number! }",
        ),
        &options,
    )
    .unwrap();
}

#[test]
fn control_an_inline_payload_holding_an_entity_with_alias_array_round_trips() {
    let Some(options) = monorepo_options() else {
        return;
    };
    round_trip(
        &event_alias_entity_payload_program("")
            .replace("ItemLoaded\n", "ItemLoaded -> internal {\n        data : [Item]\n        totalCount : number!\n      }\n"),
        &options,
    )
    .unwrap();
}

fn probe_program(types: &str, fields: &str, trait_body: &str) -> String {
    format!(
        r#"app Probe
orbital ItemOrbital {{
  {types}
  entity Item [persistent: items] {{
    id : string!
    {fields}
  }}
  trait ItemFlow -> Item {{
    initial: idle
    state idle {{
      INIT -> idle
      {trait_body}
    }}
  }}
  page "/" -> ItemFlow [initial]
}}"#
    )
}

fn assert_round_trips(source: &str) {
    let Some(options) = monorepo_options() else {
        return;
    };
    round_trip(source, &options).unwrap();
}

#[test]
fn a_struct_alias_keeps_its_authored_member_order() {
    assert_round_trips(&probe_program(
        "type Nav = { zlabel : string!, abadge : string, mhref : string! }",
        "nav : [Nav] = []",
        "",
    ));
}

#[test]
fn a_struct_alias_with_sorted_members_round_trips_too() {
    assert_round_trips(&probe_program(
        "type Nav = { abadge : string, mhref : string!, zlabel : string! }",
        "nav : [Nav] = []",
        "",
    ));
}

#[test]
fn an_alias_only_event_does_not_become_a_listened_event() {
    assert_round_trips(&probe_program(
        "type Saved = Event { id : string! }",
        "",
        "Saved -> idle",
    ));
}

#[test]
fn an_alias_only_event_with_an_empty_payload_round_trips() {
    assert_round_trips(&probe_program(
        "type Saved = Event { }",
        "",
        "Saved -> idle",
    ));
}

#[test]
fn control_a_declared_listen_with_a_payload_round_trips() {
    assert_round_trips(
        r#"app Probe
orbital ItemOrbital {
  entity Item [persistent: items] {
    id : string!
  }
  trait ItemFlow -> Item {
    initial: idle
    state idle {
      Saved -> idle
    }
    listens {
      Saved {
        id : string!
      }
    }
  }
  page "/" -> ItemFlow [initial]
}"#,
    );
}

#[test]
fn control_a_file_scoped_type_shared_by_two_orbitals_round_trips() {
    assert_round_trips(
        r#"app Probe
type Shared = { name : string! }
orbital OneOrbital {
  entity One [persistent: ones] {
    id : string!
    shared : Shared
  }
  trait OneFlow -> One {
    initial: idle
    state idle {
      INIT -> idle
    }
  }
  page "/" -> OneFlow [initial]
}
orbital TwoOrbital {
  entity Two [persistent: twos] {
    id : string!
    shared : Shared
  }
  trait TwoFlow -> Two {
    initial: idle
    state idle {
      INIT -> idle
    }
  }
  page "/two" -> TwoFlow [initial]
}"#,
    );
}

#[test]
fn a_single_object_children_slot_stays_a_single_object() {
    assert_round_trips(&probe_program(
        "",
        "",
        "SHOW -> idle\n        (render-ui main { type: card, children: { type: typography, content: \"hi\" } })",
    ));
}

#[test]
fn control_an_array_children_slot_round_trips() {
    assert_round_trips(&probe_program(
        "",
        "",
        "SHOW -> idle\n        (render-ui main { type: card, children: [{ type: typography, content: \"hi\" }] })",
    ));
}

#[test]
fn a_list_call_around_an_object_keeps_its_list_head() {
    assert_round_trips(&probe_program(
        "",
        "rows : [string] = []",
        "ADD -> idle\n        (set @entity.rows (array/concat @entity.rows (list { id: \"a\" })))",
    ));
}

#[test]
fn control_a_bare_bracket_around_an_object_round_trips() {
    assert_round_trips(&probe_program(
        "",
        "rows : [string] = []",
        "ADD -> idle\n        (set @entity.rows (array/concat @entity.rows [{ id: \"a\" }]))",
    ));
}

#[test]
fn object_keys_named_like_literals_print_quoted() {
    assert_round_trips(&probe_program(
        "",
        "",
        "SHOW -> idle\n        (render-ui main { type: badge, labels: { \"true\": \"Done\", \"false\": \"Open\", \"null\": \"None\" } })",
    ));
}

#[test]
fn an_orbital_local_type_is_not_hoisted_to_file_scope() {
    let source = r#"app Probe
orbital OneOrbital {
  type Local = { name : string! }
  entity One [persistent: ones] {
    id : string!
    local : Local
  }
  trait OneFlow -> One {
    initial: idle
    state idle {
      INIT -> idle
    }
  }
  page "/" -> OneFlow [initial]
}
orbital TwoOrbital {
  type Local = { name : string! }
  entity Two [persistent: twos] {
    id : string!
    local : Local
  }
  trait TwoFlow -> Two {
    initial: idle
    state idle {
      INIT -> idle
    }
  }
  page "/two" -> TwoFlow [initial]
}"#;
    assert_round_trips(source);
}

const PAYLOAD_OF_SOURCE: &str = r#"orbital ItemOrbital {
  type Item = {
    id : string!
    action : event
    actionPayload : @payload action
  }
  entity Row [runtime] {
    id : string!
  }
  trait Items -> Row {
    initial: idle
    state idle {
      INIT -> idle when @config.items
        (render-ui main { type: box  padding: md })
      SAVE -> idle
        (notify info "saved")
    }
    emits {
      SAVE {
        amount : number!
      }
    }
    config {
      items : [Item] = [{ id: "a", action: "SAVE", actionPayload: { amount: 1 } }]
    }
  }
  page "/" -> Items [initial]
}"#;

#[test]
fn a_payload_of_sibling_member_round_trips_byte_identically() {
    let Some(options) = monorepo_options() else {
        return;
    };
    round_trip(PAYLOAD_OF_SOURCE, &options).unwrap();
    let first = parse_and_lower_str_with(PAYLOAD_OF_SOURCE, &options).unwrap();
    let items =
        &serde_json::to_value(&first).unwrap()["orbitals"][0]["traits"][0]["config"]["items"];
    assert_eq!(
        items["items"]["properties"]["actionPayload"]["record"], "@payload(.action)",
        "{items}"
    );
    let printed = schema_to_lolo(&first).unwrap();
    assert!(
        printed.contains("actionPayload : @payload action"),
        "{printed}"
    );
    let again = schema_to_lolo(&parse_and_lower_str_with(&printed, &options).unwrap()).unwrap();
    assert_eq!(printed, again);
}

#[test]
fn control_a_plain_object_member_prints_as_an_object() {
    let Some(options) = monorepo_options() else {
        return;
    };
    let source = PAYLOAD_OF_SOURCE.replace(
        "actionPayload : @payload action",
        "actionPayload : { amount : number }",
    );
    round_trip(&source, &options).unwrap();
    let printed = schema_to_lolo(&parse_and_lower_str_with(&source, &options).unwrap()).unwrap();
    assert!(!printed.contains("@payload"), "{printed}");
}

#[test]
fn an_expects_entity_self_mark_round_trips() {
    let Some(options) = monorepo_options() else {
        return;
    };
    let source = "app shop-spec [version: \"1.0.0\"]\n\norbital ShopSpec {\n  expects entity Order [persistent, self] { total : number }\n  expects entity Cart [persistent] { total : number }\n}\n";
    round_trip(source, &options).unwrap();
    let schema = parse_and_lower_str_with(source, &options).unwrap();
    let json = serde_json::to_value(&schema).unwrap();
    let expects = json["orbitals"][0]["expects"].as_array().unwrap();
    assert_eq!(expects[0]["self"], true, "{expects:?}");
    assert!(
        expects[1].get("self").is_none(),
        "control: an unmarked expectation carries no self {expects:?}"
    );
    let printed = schema_to_lolo(&schema).unwrap();
    assert!(
        printed.contains("expects entity Order [persistent, self]"),
        "{printed}"
    );
    assert!(
        printed.contains("expects entity Cart [persistent]"),
        "{printed}"
    );
}

#[test]
fn the_self_mark_is_refused_where_it_means_nothing() {
    use orbital_lolo::parse_str;
    for source in [
        "orbital A {\n  expects entity [identity, self]\n}",
        "orbital A {\n  entity Thing [self] {\n    id : string!\n  }\n}",
    ] {
        assert!(parse_str(source).is_err(), "{source}");
    }
}

// --- A1: lossless lowering residuals (G-ORB-410, -284, -406, -513, -389) ---

#[test]
fn a_tuple_slot_holding_an_inline_struct_round_trips() {
    assert_round_trips(&probe_program(
        "type Base = { version : number!, entries : [[string, { repo : string!, body : string! }]]! }",
        "base : Base",
        "",
    ));
}

#[test]
fn control_a_tuple_of_primitives_round_trips() {
    assert_round_trips(&probe_program(
        "type Base = { version : number!, entries : [[string, number]]! }",
        "base : Base",
        "",
    ));
}

#[test]
fn a_union_variant_listing_a_struct_alias_round_trips() {
    assert_round_trips(&probe_program(
        "type Col = { key : string }\n  type Knob = { default : string | [Col] }",
        "knob : Knob",
        "",
    ));
}

#[test]
fn control_a_union_variant_listing_an_inline_struct_round_trips() {
    assert_round_trips(&probe_program(
        "type Knob = { default : string | [{ key : string }] }",
        "knob : Knob",
        "",
    ));
}

#[test]
fn a_union_alias_with_an_inline_struct_variant_round_trips() {
    assert_round_trips(&probe_program(
        "type Ret = string | { kind : string }\n  type Entry = { returns : Ret }",
        "entry : Entry",
        "",
    ));
}

#[test]
fn control_a_union_alias_with_a_struct_alias_variant_round_trips() {
    assert_round_trips(&probe_program(
        "type Kind = { kind : string }\n  type Ret = string | Kind\n  type Entry = { returns : Ret }",
        "entry : Entry",
        "",
    ));
}

fn quoted_sigil_program(literal: &str) -> String {
    format!(
        r#"app Probe
orbital ItemOrbital {{
  entity Item [persistent: items] {{
    id : string!
    name : string = ""
  }}
  trait ItemFlow -> Item {{
    initial: idle
    state idle {{
      INIT -> idle
        (set @entity.name (json/stringify (quote {literal})))
    }}
  }}
  page "/" -> ItemFlow [initial]
}}"#
    )
}

#[test]
fn a_quoted_string_ending_in_a_dot_after_a_sigil_round_trips() {
    assert_round_trips(&quoted_sigil_program("\"@payload.\""));
}

#[test]
fn control_a_quoted_clean_sigil_path_round_trips() {
    assert_round_trips(&quoted_sigil_program("\"@payload.x\""));
}

#[test]
fn a_quoted_string_with_an_empty_sigil_segment_round_trips() {
    assert_round_trips(&quoted_sigil_program("\"@payload..x\""));
}

#[test]
fn a_relation_entity_field_agrees_with_the_payload_it_feeds() {
    assert_round_trips(
        r#"app Probe
orbital CategoryOrbital {
  entity Category [persistent: categories] {
    id : string!
  }
  trait CategoryFlow -> Category {
    initial: idle
    state idle {
      INIT -> idle
    }
  }
  page "/categories" -> CategoryFlow
}
orbital ItemOrbital {
  entity Item [persistent: items] {
    id : string!
    categoryId : Category
  }
  trait ItemFlow -> Item {
    initial: idle
    state idle {
      INIT -> idle
    }
    emits {
      ItemLoaded -> internal {
        data : [Item]!
      }
    }
  }
  page "/" -> ItemFlow [initial]
}"#,
    );
}

#[test]
fn control_a_string_entity_field_feeds_a_string_payload() {
    assert_round_trips(
        &"app Probe
orbital ItemOrbital {
  entity Item [persistent: items] {
    id : string!
    categoryId : string = \"\"
  }
  trait ItemFlow -> Item {
    initial: idle
    state idle {
      INIT -> idle
    }
    emits {
      ItemLoaded -> internal {
        data : [Item]!
      }
    }
  }
  page \"/\" -> ItemFlow [initial]
}"
        .to_string(),
    );
}

fn config_alias_program(types: &str, config: &str) -> String {
    format!(
        r#"app Probe
orbital ItemOrbital {{
  {types}
  entity Item [persistent: items] {{
    id : string!
  }}
  trait ItemFlow -> Item {{
    initial: idle
    state idle {{
      INIT -> idle
        (set @entity.id (json/stringify @config.wheres))
    }}
    config {{
      {config}
    }}
  }}
  page "/" -> ItemFlow [initial]
}}"#
    )
}

#[test]
fn a_field_reference_member_of_a_config_alias_round_trips() {
    assert_round_trips(&config_alias_program(
        "type Where = { field : field (@entity @config.rowEntity) (string | number)!, op : string }",
        "wheres : [Where] = []\n      rowEntity : entity = Item",
    ));
}

#[test]
fn control_a_plain_member_of_a_config_alias_round_trips() {
    assert_round_trips(&config_alias_program(
        "type Where = { field : string!, op : string }",
        "wheres : [Where] = []",
    ));
}

#[test]
fn a_program_type_united_with_a_map_in_a_struct_alias_round_trips() {
    assert_round_trips(&probe_program(
        "type Req = { viewer : (Viewer | (Map string string))? }",
        "queue : [Req] = []",
        "",
    ));
}

#[test]
fn control_a_program_type_member_of_a_struct_alias_round_trips() {
    assert_round_trips(&probe_program(
        "type Req = { viewer : Viewer? }",
        "queue : [Req] = []",
        "",
    ));
}

#[test]
fn an_event_alias_named_by_a_field_type_round_trips() {
    assert_round_trips(
        r#"app Probe
orbital ItemOrbital {
  type Occurred = Event { data : string, id : string }
  entity Item [persistent: items] {
    id : string!
  }
  trait ItemFlow -> Item {
    initial: idle
    state idle {
      Occurred -> idle
        (set @entity.id (json/stringify @config.key))
    }
    emits {
      Occurred -> internal {
        data : string
        id : string
      }
    }
    config {
      key : field Occurred = id
    }
  }
  page "/" -> ItemFlow [initial]
}"#,
    );
}

#[test]
fn a_struct_alias_with_an_event_member_in_a_config_knob_round_trips() {
    assert_round_trips(&config_alias_program(
        "type Step = { event : event, label : string }",
        "wheres : [Step] = []",
    ));
}

#[test]
fn an_event_member_reached_by_an_entity_field_is_refused_at_lowering() {
    let Some(options) = monorepo_options() else {
        return;
    };
    let source = probe_program(
        "type Step = { event : event, label : string }",
        "steps : [Step] = []",
        "",
    );
    let error = parse_and_lower_str_with(&source, &options).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("ELOLO_CONFIG_TYPE_FORBIDDEN_IN_ENTITY"),
        "{error}"
    );
}

const TAGGED_UNION_TYPES: &str = r#"type Leaf = { kind : "leaf", name : string }
  type Not = { kind : "not", of : Pred! }
  type Pred = Leaf | Not"#;

#[test]
fn a_tagged_union_back_reference_in_an_entity_field_round_trips() {
    assert_round_trips(&probe_program(TAGGED_UNION_TYPES, "pred : Pred", ""));
}

#[test]
fn a_tagged_union_back_reference_in_a_config_knob_round_trips() {
    assert_round_trips(&config_alias_program(
        &format!("{TAGGED_UNION_TYPES}\n  type Holder = {{ pred : Pred }}"),
        "wheres : [Holder] = []",
    ));
}

#[test]
fn a_tagged_union_back_reference_in_an_emit_payload_round_trips() {
    assert_round_trips(&format!(
        r#"app Probe
orbital ItemOrbital {{
  {TAGGED_UNION_TYPES}
  entity Item [persistent: items] {{
    id : string!
  }}
  trait ItemFlow -> Item {{
    initial: idle
    state idle {{
      INIT -> idle
    }}
    emits {{
      Judged -> internal {{
        pred : Pred
        not : Not
      }}
    }}
  }}
  page "/" -> ItemFlow [initial]
}}"#
    ));
}

#[test]
fn control_a_union_without_a_back_reference_round_trips() {
    assert_round_trips(&probe_program(
        "type Leaf = { kind : \"leaf\", name : string }\n  type Other = { kind : \"other\", n : number }\n  type Pred = Leaf | Other",
        "pred : Pred",
        "",
    ));
}

fn map_alias_payload_program(payload: &str) -> String {
    format!(
        r#"app Probe
orbital ItemOrbital {{
  type Details = Map string (string | number)
  entity Item [persistent: items] {{
    id : string!
    details : Details
  }}
  trait ItemFlow -> Item {{
    initial: idle
    state idle {{
      INIT -> idle
    }}
    emits {{
      Deployed -> internal {{
        {payload}
      }}
    }}
  }}
  page "/" -> ItemFlow [initial]
}}"#
    )
}

#[test]
fn a_named_map_alias_payload_field_round_trips() {
    assert_round_trips(&map_alias_payload_program("details : Details"));
}

#[test]
fn control_an_inline_map_payload_field_round_trips() {
    assert_round_trips(&map_alias_payload_program(
        "details : Map string (string | number)",
    ));
}

const MODAL_SELF_REFERENCE: &str = r#"app Probe
orbital ItemOrbital {
  type ModalRecord = { id : string!, seedRow : ModalRecord }
  entity Item [persistent: items] {
    id : string!
  }
  trait ItemFlow -> Item {
    initial: idle
    state idle {
      INIT -> idle
    }
    emits {
      Opened -> internal {
        seedRow : ModalRecord
      }
      Closed -> internal {
        record : ModalRecord
      }
    }
  }
  page "/" -> ItemFlow [initial]
}"#;

#[test]
fn an_inline_emit_payload_and_an_event_alias_carry_the_same_projection() {
    assert_round_trips(MODAL_SELF_REFERENCE);
    assert_round_trips(
        &MODAL_SELF_REFERENCE
            .replace(
                "      Opened -> internal {\n        seedRow : ModalRecord\n      }\n",
                "      Opened -> internal\n",
            )
            .replace(
                "  entity Item",
                "  type Opened = Event { seedRow : ModalRecord }\n  entity Item",
            ),
    );
}

#[test]
fn a_nullable_union_variant_round_trips() {
    assert_round_trips(&probe_program(
        "type Node = { value : string | number | boolean? }",
        "node : Node",
        "",
    ));
}

#[test]
fn control_a_union_without_a_nullable_variant_round_trips() {
    assert_round_trips(&probe_program(
        "type Node = { value : string | number | boolean }",
        "node : Node",
        "",
    ));
}

#[test]
fn a_field_reference_member_of_an_alias_nested_in_a_config_alias_round_trips() {
    assert_round_trips(&config_alias_program(
        "type Where = { field : field (@entity @config.rowEntity) (string | number)!, op : string }\n  type Spec = { label : string, where : [Where] }",
        "specs : [Spec] = []\n      rowEntity : entity = Item",
    ).replace("@config.wheres", "[@config.specs, @config.rowEntity]"));
}

#[test]
fn control_a_plain_member_of_an_alias_nested_in_a_config_alias_round_trips() {
    assert_round_trips(&config_alias_program(
        "type Where = { field : string!, op : string }\n  type Spec = { label : string, where : [Where] }",
        "specs : [Spec] = []",
    ).replace("@config.wheres", "@config.specs"));
}

#[test]
fn a_not_found_page_and_a_declared_relation_display_round_trip() {
    let source = probe_program("", "owner : Item @display id", "").replace(
        "  page \"/\" -> ItemFlow [initial]",
        "  page \"/\" -> ItemFlow [initial]\n  page \"/missing\" as Missing -> ItemFlow [notFound]",
    );
    assert_round_trips(&source);
}

fn lowered_knob(types: &str, config: &str) -> Option<Value> {
    let options = monorepo_options()?;
    let schema = parse_and_lower_str_with(&config_alias_program(types, config), &options).expect("lowers");
    let json = serde_json::to_value(&schema).expect("serializes");
    Some(json["orbitals"][0]["traits"][0]["config"]["wheres"].clone())
}

#[test]
fn a_config_knob_typed_by_a_union_alias_carries_the_union_variant_names() {
    let Some(knob) = lowered_knob("type Env = { anchor : string }\n  type HostPart = string | Env", "wheres : HostPart = \"h\"") else {
        return;
    };
    assert_eq!(knob["values"], serde_json::json!(["string", "Env"]), "{knob}");
    assert!(knob["properties"]["Env"].is_object(), "{knob}");
    let inline = lowered_knob("type Env = { anchor : string }", "wheres : string | Env = \"h\"").expect("lowers");
    assert_eq!(inline["values"], serde_json::json!(["string", "Env"]), "control: an inline union already carried them");
    let literal = lowered_knob("type Mode = \"a\" | \"b\"", "wheres : Mode = \"a\"").expect("lowers");
    assert!(literal.get("properties").is_none_or(Value::is_null), "control: a literal enum alias stays an enum: {literal}");
}
